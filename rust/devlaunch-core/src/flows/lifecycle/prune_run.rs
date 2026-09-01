//! `dl --prune`: the acting pass.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use super::delete::{VolumeSweep, sweep_volumes};
use super::locations::{Unlocatable, canonical, workspace_locations};
use super::notices::{LifecycleNotice, extend_with_cache, extend_with_store};
use super::prune_plan::{
    PruneError, PrunePlan, Reclaimable, ReclaimableVolumes, VolumesKept, VolumesKeptBecause,
    records_by_directory, sources_by_workspace,
};
use super::prune_status::{Decision, KeptBecause, RepoAt, clone_status, decide};
use crate::clients::devpod::Workspace;
use crate::domain::metadata::MetadataStorage;
use crate::domain::model::WorktreeInfo;
use crate::domain::workspace_state::NonEmpty;
use crate::flows::agent_worktrees::{self, WorktreeReport};
use crate::flows::disk_usage::{self, DiskUsage};
use crate::flows::kept_copies::KeptCopies;
use crate::flows::listing::CommandContext;
use crate::flows::repo_manager::{Refusal, TreeSweep, remove_tree_as_far_as_it_goes};
use crate::flows::workspace_clone::WorkspaceCloneManager;
use crate::notices::Notices;
use crate::runner::Runner;

/// One directory the plan meant to remove that the second pass would not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Withheld {
    pub path: PathBuf,
    /// Why it is staying — and it is worth saying that this was not so when the
    /// plan was printed.
    pub because: KeptBecause,
}

/// What the acting pass did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneReport {
    pub removed: Vec<Reclaimable>,
    pub withheld: Vec<Withheld>,
    /// Directories that would not come away. Not empty means the run is
    /// unfinished, and the clones that *did* go are still gone — which is why this
    /// is a report and not an abort.
    pub refused: Vec<Refusal>,
    /// The workspaces whose volumes went, and which volumes. Their copies are
    /// dropped: the removal is what proved them pointless.
    pub reclaimed: Vec<ReclaimableVolumes>,
    /// The workspaces whose volumes stayed, and why. Their copies are kept, so
    /// every one of these is retryable.
    pub volumes_kept: Vec<VolumesKept>,
    /// What the run did about the agent worktrees inside the clones it kept.
    pub worktrees: WorktreeReport,
}

impl PruneReport {
    /// What the *clone directories* this run removed actually freed — with the
    /// figures the plan measured, so what a person is told they got back is what
    /// they said yes to.
    ///
    /// The agent worktrees are not in it, for the reason
    /// [`PrunePlan::clones_freed`] gives: they are inside clones this run kept,
    /// and [`WorktreeReport::freed`] is where their bytes are stated.
    pub fn clones_freed(&self) -> DiskUsage {
        disk_usage::total_usage(self.removed.iter().map(|it| it.usage.clone()))
    }

    pub fn finished(&self) -> bool {
        self.refused.is_empty()
            && self.worktrees.refused.is_empty()
            // A part-removed environment is a directory the user was told would
            // go and which is still there, in pieces. `withheld` is absent from
            // this list deliberately -- a changed mind leaves nothing half done
            // -- and a refusal is the opposite of that.
            && self.worktrees.refused_derivatives.is_empty()
            && self.worktrees.forget_refused.is_empty()
    }
}

/// How the acting pass ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneOutcome {
    /// A live workspace's source could not be followed, so nothing was removed:
    /// no clone is unreferenced while a workspace is unaccounted for.
    Unlocatable(NonEmpty<Unlocatable>),
    /// Boxed because the report is an order of magnitude the larger arm and
    /// this enum is returned by value: `clippy::large_enum_variant`.
    Acted(Box<PruneReport>),
}

/// Carry out `plan`: remove the directories, then forget them.
///
/// **Every directory is classified again, under the lock, immediately before it
/// goes**, and only what this pass *also* finds removable is removed. The report a
/// user answered was taken before they answered it, and everything it rests on can
/// have moved in between: a container writes into a clone, or a launch that was
/// mid-clone when the plan was printed finishes and registers a workspace for one
/// of these exact directories — the clone path for `(owner, repo, branch)` is
/// deterministic, so a concurrent launch reuses the very directory in the plan.
/// Re-asking only "has it grown unsaved work" caught the first and not the second,
/// and the difference was somebody's running workspace.
///
/// That is why this pass pays a second `devpod list`. It is the one question whose
/// answer cannot be re-derived from disk, it is O(1) rather than per workspace, and
/// it is paid only after a user has said yes to a deletion.
///
/// `--force` is re-applied per directory, from the promotion the plan recorded for
/// that directory rather than from a flag over the whole run, so insisting past one
/// clone's unsaved work does not turn the re-probe off for the others. The approved
/// set can therefore shrink between the report and the act, and can never grow —
/// the direction that costs a command rather than a morning's work.
pub fn prune_clones(
    context: &mut CommandContext<'_>,
    clones: &WorkspaceCloneManager<'_>,
    storage: &mut MetadataStorage,
    copies: &KeptCopies,
    plan: &PrunePlan,
    notices: &mut dyn Notices<LifecycleNotice>,
) -> Result<PruneOutcome, PruneError> {
    let workspaces = context
        .refreshed_workspaces()
        .map_err(PruneError::Listing)?;
    let locations = workspace_locations(&workspaces, &plan.root);
    if let Some(unlocatable) = locations.unlocatable() {
        return Ok(PruneOutcome::Unlocatable(unlocatable));
    }
    let listed_at = sources_by_workspace(&workspaces);
    let mut cache_notices = Vec::new();
    let record_for = records_by_directory(clones, storage, &mut cache_notices);
    let git = clones.repo_manager().git();

    // Grouped so one lock scope covers a repository's whole share of the plan, and
    // sorted so two runs over an unchanged cache take the locks in the same order.
    let mut by_repo: BTreeMap<(String, String), Vec<&Reclaimable>> = BTreeMap::new();
    for reclaimable in &plan.removing {
        by_repo
            .entry((reclaimable.owner.clone(), reclaimable.repo.clone()))
            .or_default()
            .push(reclaimable);
    }

    let mut report = PruneReport {
        removed: Vec::new(),
        withheld: Vec::new(),
        refused: Vec::new(),
        reclaimed: Vec::new(),
        volumes_kept: Vec::new(),
        worktrees: WorktreeReport::default(),
    };
    let mut forget: Vec<WorktreeInfo> = Vec::new();
    for ((owner, repo), reclaimables) in by_repo {
        let _lock = clones
            .repo_manager()
            .hold_repo_lock(&owner, &repo)
            .map_err(PruneError::Lock)?;
        let bare_path = clones.repo_manager().bare_dir(&owner, &repo);
        for reclaimable in reclaimables {
            let status = clone_status(
                &git,
                &reclaimable.path,
                RepoAt {
                    owner: &owner,
                    repo: &repo,
                    bare: &bare_path,
                },
                &locations,
                &record_for,
                &listed_at,
            );
            match decide(status, reclaimable.promotion.insistence()) {
                Decision::Keep(because) => {
                    report.withheld.push(Withheld {
                        path: reclaimable.path.clone(),
                        because,
                    });
                    continue;
                }
                Decision::Remove { .. } => {}
            }
            // A clone directory is one unit of work here: only the arm that says it
            // is entirely gone counts it as removed and drops its record. The two
            // refusal arms are alike to this caller — a directory half removed is
            // still a directory somebody has to deal with — so they share one arm.
            match remove_tree_as_far_as_it_goes(&reclaimable.path) {
                TreeSweep::Everything => {
                    report.removed.push((*reclaimable).clone());
                    if let Some(record) = record_for.get(&reclaimable.path) {
                        forget.push(record.clone());
                    }
                }
                TreeSweep::WhatItCould(refused) | TreeSweep::Nothing(refused) => {
                    report.refused.extend(refused.iter().cloned());
                }
            }
        }
    }
    // Outside every repo lock too, and for a stronger reason than the record drop
    // below: these volumes belong to workspaces with no clone directory in this
    // plan at all, so no repository lock is about them.
    reclaim_volumes(context.runner(), copies, plan, &workspaces, &mut report);
    // The agent worktrees inside the clones this run is keeping. A second pass
    // over a disjoint set of directories — the sweep only ever covers clones the
    // plan keeps, and the loop above only ever removes whole clones — so it
    // takes each repository's lock again rather than sharing the loop, which is
    // holding a lock for as short a time as the work needs.
    for found in plan.worktrees.clones() {
        let _lock = clones
            .repo_manager()
            .hold_repo_lock(found.owner(), found.repo())
            .map_err(PruneError::Lock)?;
        let bare = canonical(
            &clones
                .repo_manager()
                .bare_dir(found.owner(), found.repo())
                .to_string_lossy(),
        );
        agent_worktrees::reclaim(&git, found, bare.as_deref(), &mut report.worktrees);
    }
    // Outside every repo lock, because the repo lock is what protects the
    // *directory* work and a record drop touches only `metadata.json`, which has a
    // lock of its own. Keeping it out means a repository is held for exactly as
    // long as its clones are being looked at and removed.
    for record in forget.iter().chain(plan.stale_records.iter()) {
        forget_clone(storage, record, notices);
    }
    extend_with_cache(notices, cache_notices);
    Ok(PruneOutcome::Acted(Box::new(report)))
}

/// Remove the volumes the plan named, and drop the copies the removal made
/// pointless.
///
/// **The precondition is re-asked here, under the second listing the acting pass
/// already pays for**, exactly as each clone directory is re-classified before it
/// goes. The plan was taken before the user answered it, and a workspace can come
/// back to devpod's listing in between — a launch of that very id, or a
/// `--reconcile` — at which point its volumes are a live workspace's and not
/// leftovers. The approved set can therefore shrink and can never grow, which is
/// the direction that costs a command rather than somebody's disk.
///
/// The removal itself is [`sweep_volumes`], the same one the delete path uses, so
/// there is one place in devlaunch where a name reaches `docker volume rm` and one
/// place where docker's answer becomes a verdict.
fn reclaim_volumes(
    runner: &dyn Runner,
    copies: &KeptCopies,
    plan: &PrunePlan,
    workspaces: &[Workspace],
    report: &mut PruneReport,
) {
    let live: HashSet<&str> = workspaces.iter().map(|it| it.id.as_str()).collect();
    for reclaimable in &plan.reclaiming {
        if live.contains(reclaimable.workspace_id.as_str()) {
            report.volumes_kept.push(VolumesKept {
                workspace_id: reclaimable.workspace_id.clone(),
                because: VolumesKeptBecause::ListedAgain,
            });
            continue;
        }
        match sweep_volumes(runner, Some(reclaimable.names.clone())) {
            // Dropped once, on proof. This is the deliberate divergence from
            // `verdict_cache`'s "nothing ever deletes a marker": there a delete
            // would be a second unproven mechanism, here it is conditioned on the
            // removal that made the copy pointless.
            VolumeSweep::Removed => {
                copies.forget(&reclaimable.workspace_id);
                report.reclaimed.push(reclaimable.clone());
            }
            // Kept, so the retry survives — a volume some container still holds is
            // one this run could not have taken anyway.
            VolumeSweep::Refused(refusal) => report.volumes_kept.push(VolumesKept {
                workspace_id: reclaimable.workspace_id.clone(),
                because: VolumesKeptBecause::Refused(refusal),
            }),
            // A machine with no docker never made these volumes, so there is
            // nothing here to have failed and nothing to say — the silence the
            // delete path keeps for the same arm. `NothingNamed` is unreachable:
            // the names ride on the entry and a `NonEmpty` has no empty state.
            VolumeSweep::NoDocker | VolumeSweep::NothingNamed => {}
        }
    }
}

/// Drop one worktree record.
///
/// Removing a clone without this is what left `metadata.json` describing workspaces
/// that stopped existing years ago; a record kept for a directory that is gone is
/// not a safety margin, it is the thing that made the file unreadable as a
/// description of anything.
pub(super) fn forget_clone(
    storage: &mut MetadataStorage,
    record: &WorktreeInfo,
    notices: &mut dyn Notices<LifecycleNotice>,
) {
    match storage.remove_worktree(&record.owner, &record.repo, &record.branch) {
        Ok(store_notices) => extend_with_store(notices, store_notices),
        Err(error) => notices.say(LifecycleNotice::RecordNotDropped {
            path: record.local_path.clone(),
            refusal: error,
        }),
    }
}
