//! `dl --reconcile`: re-pointing the records the id-scheme change orphaned.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::locations::{
    SourcePlaces, SourceSite, canonical, is_populated_clone, site_of, source_places,
};
use super::notices::{LifecycleNotice, extend_with_cache, extend_with_store};
use super::prune_plan::{ClonePlacement, leaf_of};
use super::refresh::{Refresh, RefreshReason};
use crate::clients::devpod::Workspace;
use crate::clients::devpod_home::{DevpodHome, RepointFailure};
use crate::domain::metadata::{MetadataStorage, RecordUpdate, WorktreeFilter};
use crate::domain::model::WorktreeInfo;
use crate::domain::workspace_state::NonEmpty;
use crate::flows::listing::CommandContext;
use crate::flows::workspace_clone::WorkspaceCloneManager;
use crate::notices::Notices;

/// The clone-directory leaf `branch` had under the pre-#81 scheme: the branch with
/// everything git allows and a path component does not collapsed to dashes.
///
/// Kept because it is the only thing connecting devpod's stale record to a branch —
/// the id it was addressed by is exactly what changed, so the leaf is what
/// survived. This is `dl`'s own history rather than a guess at another tool's
/// format, and it is frozen: a third naming would be a third function, never an
/// edit to this one.
pub(super) fn legacy_leaf(branch: &str) -> String {
    let flattened: String = branch
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    flattened.trim_matches('-').to_owned()
}

/// Every directory name a record's clone has been called by a build of `dl`.
///
/// Three, because `dl` has named that directory three ways and a devpod record can
/// be holding any of them: the current hashed leaf, the branch itself (the original
/// layout), and the branch flattened for the filesystem. Matching on all three is
/// what lets one command repair hosts that stopped at different versions.
fn leaf_spellings(record: &WorktreeInfo) -> [String; 3] {
    [
        record.workspace_id.clone(),
        record.branch.clone(),
        legacy_leaf(&record.branch),
    ]
}

/// An orphaned devpod record, and the clone it can be re-pointed at.
///
/// `record` is carried rather than looked up again at write time so that the plan
/// the user consented to and the change that is applied cannot be built from two
/// different reads of `metadata.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adoptable {
    pub workspace_id: String,
    /// The devpod context this workspace belongs to: ids are unique per context,
    /// so it is half of the workspace's address on disk.
    pub context: String,
    pub sourced_at: String,
    pub clone: PathBuf,
    pub record: WorktreeInfo,
}

/// Why `dl` will not repair one orphaned devpod record.
///
/// Refusing costs a line in a report. Guessing costs a workspace, so every
/// ambiguity fails towards the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotAdopted {
    /// No clone of that repository answers to this name.
    NoCloneAnswers,
    /// More than one clone answers to this name, so none of them can.
    ///
    /// The legacy spelling is not injective: `feature/auth`, `feature auth` and
    /// `feature:auth` were all the directory `feature-auth`, so one devpod record
    /// can name two branches' clones and picking one would be a coin flip decided
    /// by listing order.
    NameAnsweredByManyClones(NonEmpty<PathBuf>),
    /// More than one orphan matches this clone, so none of them can: picking one
    /// would be a coin flip and the loser would still be broken with nothing said
    /// about why.
    CloneWantedByManyWorkspaces { clone: PathBuf, workspaces: usize },
}

/// An orphaned devpod record `dl` will not repair, and why not.
///
/// Reported, never deleted. `dl` has no way to know whether a workspace whose clone
/// is gone is finished with, and the two mistakes are not equal: leaving a dead
/// record costs a line in `devpod list`, removing a live one costs whatever was in
/// the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unadoptable {
    pub workspace_id: String,
    pub sourced_at: String,
    pub because: NotAdopted,
}

/// What `--reconcile` would change, and what it would only report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcilePlan {
    // Private for [`PrunePlan`]'s reason: an adoption list a caller could fill
    // would not be the one [`reconcile_plan`]'s contested-in-both-directions
    // matching produced.
    root: PathBuf,
    pub(super) adopting: Vec<Adoptable>,
    pub(super) reporting: Vec<Unadoptable>,
}

impl ReconcilePlan {
    pub fn nothing_to_do(&self) -> bool {
        self.adopting.is_empty() && self.reporting.is_empty()
    }

    /// The directory the orphans were placed under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The devpod records this run will re-point.
    pub fn adopting(&self) -> &[Adoptable] {
        &self.adopting
    }

    /// The orphans this run will only name, and why.
    pub fn reporting(&self) -> &[Unadoptable] {
        &self.reporting
    }
}

/// The workspaces devpod sources inside a repository's clone tree, at no clone.
///
/// The same test `--prune` classifies [`Misplaced`] by, and deliberately the same
/// one: a directory with no `.git` in it is devlaunch#88's published diagnostic, and
/// it covers both shapes the reporting host had — a folder that is simply gone, and
/// the config-only stub devpod rebuilds from its cache when it finds the recorded
/// source absent. The second is the dangerous one, because it exists and so passes
/// every check that only asks whether the source is there.
///
/// Each answer carries the resolved source path, so the leaf the join is made on is
/// read off the same value the site was decided from.
fn orphaned_workspaces(workspaces: &[Workspace], root: &Path) -> Vec<(Workspace, PathBuf)> {
    let mut orphans = Vec::new();
    for workspace in workspaces {
        let SourcePlaces::Placeable(paths) = source_places(&workspace.source) else {
            continue;
        };
        for source in paths {
            let Some(resolved) = canonical(&source) else {
                continue;
            };
            if let SourceSite::InARepositoryOnly { .. } = site_of(&resolved, root) {
                orphans.push((workspace.clone(), resolved));
            }
        }
    }
    orphans
}

/// Match every orphaned devpod record to a clone, or say why it cannot be.
///
/// **The join is by path and never by id.** The id is what the scheme change moved,
/// so it connects nothing across the two record sets; the source path devpod kept
/// still names owner and repo exactly, and its leaf still names the branch in one of
/// the three spellings `dl` has written (see [`leaf_spellings`]).
///
/// Two candidates are refused rather than resolved, in both directions. A clone a
/// live workspace already opens — at it *or under it*, which is what
/// `WorkspaceLocations::holder` is for — is not a candidate at all: adopting it
/// would point two workspaces at one directory and leave the working one sharing its
/// checkout with a dead one. The rest is [`NotAdopted`]'s three arms.
pub fn reconcile_plan(
    clones: &WorkspaceCloneManager<'_>,
    storage: &MetadataStorage,
    workspaces: &[Workspace],
    placement: &ClonePlacement,
    notices: &mut dyn Notices<LifecycleNotice>,
) -> ReconcilePlan {
    let ClonePlacement { root, locations } = placement;
    let orphans = orphaned_workspaces(workspaces, root);
    let mut cache_notices = Vec::new();

    // Every clone this cache has a record for that is a real checkout and that no
    // live workspace is already opening, indexed by the directory names a devpod
    // record could be calling it. *Every* clone a name answers to and not the last
    // one written, because a name two clones answer to has to be visible as
    // contested rather than silently resolved to one of them.
    let mut candidates: BTreeMap<(String, String, String), Vec<PathBuf>> = BTreeMap::new();
    let mut records: HashMap<PathBuf, WorktreeInfo> = HashMap::new();
    for record in storage.list_worktrees(WorktreeFilter::All) {
        let Some(clone) = clones.resolve_clone_path(record, &mut cache_notices) else {
            continue;
        };
        let Some(resolved) = canonical(&clone.to_string_lossy()) else {
            continue;
        };
        if !is_populated_clone(&resolved) || locations.holder(&resolved).is_some() {
            continue;
        }
        records.insert(resolved.clone(), record.clone());
        for spelling in leaf_spellings(record) {
            let answers = candidates
                .entry((record.owner.clone(), record.repo.clone(), spelling))
                .or_default();
            // One record spells its leaf the same way twice whenever the branch
            // needs no flattening, and that is one answer, not a contest.
            if !answers.contains(&resolved) {
                answers.push(resolved.clone());
            }
        }
    }
    extend_with_cache(notices, cache_notices);

    // Which orphans want which clone, before any of them gets one: a clone two
    // orphans match has to be visible as contested from both sides.
    let mut wanted: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut matched: HashMap<String, PathBuf> = HashMap::new();
    let mut contested: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for (workspace, source) in &orphans {
        let Some((owner, repo)) = repository_of(source, root) else {
            continue;
        };
        let Some(leaf) = leaf_of(source) else {
            continue;
        };
        let answers = candidates
            .get(&(owner, repo, leaf))
            .cloned()
            .unwrap_or_default();
        match answers.len() {
            0 => {}
            1 => {
                wanted
                    .entry(answers[0].clone())
                    .or_default()
                    .push(workspace.id.clone());
                matched.insert(workspace.id.clone(), answers[0].clone());
            }
            _ => {
                let mut sorted = answers;
                sorted.sort();
                contested.insert(workspace.id.clone(), sorted);
            }
        }
    }

    let mut adopting = Vec::new();
    let mut reporting = Vec::new();
    for (workspace, source) in &orphans {
        let sourced_at = source.display().to_string();
        let refuse = |because| Unadoptable {
            workspace_id: workspace.id.clone(),
            sourced_at: sourced_at.clone(),
            because,
        };
        if let Some(answers) = contested.get(&workspace.id) {
            let Some(answers) = NonEmpty::of(answers.iter().cloned()) else {
                continue;
            };
            reporting.push(refuse(NotAdopted::NameAnsweredByManyClones(answers)));
            continue;
        }
        let Some(clone) = matched.get(&workspace.id) else {
            reporting.push(refuse(NotAdopted::NoCloneAnswers));
            continue;
        };
        let claimants = wanted.get(clone).map_or(0, Vec::len);
        if claimants > 1 {
            reporting.push(refuse(NotAdopted::CloneWantedByManyWorkspaces {
                clone: clone.clone(),
                workspaces: claimants,
            }));
            continue;
        }
        let Some(record) = records.get(clone) else {
            continue;
        };
        adopting.push(Adoptable {
            workspace_id: workspace.id.clone(),
            context: workspace.context.clone(),
            sourced_at,
            clone: clone.clone(),
            record: record.clone(),
        });
    }
    ReconcilePlan {
        root: root.to_path_buf(),
        adopting,
        reporting,
    }
}

/// The `(owner, repo)` a resolved source names, read off its position under `root`.
fn repository_of(source: &Path, root: &Path) -> Option<(String, String)> {
    let relative = source.strip_prefix(root).ok()?;
    let mut parts = relative.components();
    let owner = parts.next()?.as_os_str().to_string_lossy().into_owned();
    let repo = parts.next()?.as_os_str().to_string_lossy().into_owned();
    Some((owner, repo))
}

/// What became of one of the plan's adoptions.
///
/// Each carries its own ending, so a caller reads how an adoption went straight
/// off the arm. The report used to be two lists — re-pointed and refused — and the
/// binary re-derived each adoption's ending by scanning the refusal list for an
/// absence: quadratic, and an inference ("not refused") standing in for a record
/// ("re-pointed") that was already being kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adoption {
    /// devpod's record now points at the clone, and metadata carries the id.
    /// Boxed because an [`Adoptable`] carries the whole [`WorktreeInfo`] and the
    /// refusal arm is a fraction of its size (clippy::large_enum_variant).
    Repointed(Box<Adoptable>),
    /// devpod's record could not be re-pointed. The plan's other adoptions went
    /// on: one unrepairable record must not cost the rest their repair.
    Refused {
        workspace_id: String,
        failure: RepointFailure,
    },
    /// devpod's record was re-pointed and metadata was not, so the id has the
    /// one copy a finished adoption leaves two of. Either the row was gone when
    /// the metadata lock was taken — another run removed the workspace while
    /// the plan sat at its prompt, and writing would have put the row back — or
    /// the store refused the write, and then a notice names the refusal. Half
    /// an adoption, reported as one: [`ReconcileReport::finished`] is false
    /// here, because the alternative is a line reading "Re-pointed" about a
    /// record dl never touched.
    Unrecorded { workspace_id: String },
}

/// What applying a reconcile plan did: one [`Adoption`] per adoption the plan
/// asked for, in the order they were attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    // Private for [`ReconcilePlan`]'s reason: a report a caller could assemble is
    // one whose endings need not be the attempts'.
    adoptions: Vec<Adoption>,
}

impl ReconcileReport {
    /// Each adoption with what became of it, in the order they were attempted.
    pub fn adoptions(&self) -> &[Adoption] {
        &self.adoptions
    }

    /// The adoptions that landed. Only this module's tests read it; the binary
    /// renders each [`Adoption`] from [`ReconcileReport::adoptions`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn repointed(&self) -> impl Iterator<Item = &Adoptable> {
        self.adoptions.iter().filter_map(|adoption| match adoption {
            Adoption::Repointed(adoptable) => Some(adoptable.as_ref()),
            Adoption::Refused { .. } | Adoption::Unrecorded { .. } => None,
        })
    }

    /// Whether every adoption landed. The one distinction an exit code carries.
    ///
    /// A `match` rather than the `matches!` this used to be, so an arm added to
    /// [`Adoption`] has to say which side of that distinction it falls on
    /// instead of defaulting to "landed" — which is how a re-point that wrote
    /// no metadata came to leave this true.
    pub fn finished(&self) -> bool {
        self.adoptions.iter().all(|adoption| match adoption {
            Adoption::Repointed(_) => true,
            Adoption::Refused { .. } | Adoption::Unrecorded { .. } => false,
        })
    }
}

/// Carry out `plan`'s adoptions.
///
/// devpod's record is re-pointed first and metadata's id written second, and that
/// order is the recoverable one. Stopping between them leaves a workspace that opens
/// the right clone and a record that has not yet said so, which the next run
/// repairs; the other order would leave `dl` following a record to a workspace still
/// sourced at a dead path, which is the fault this command exists to clear.
pub fn apply_reconciliation(
    context: &mut CommandContext<'_>,
    refresh: &mut Refresh<'_>,
    storage: &mut MetadataStorage,
    devpod_home: &DevpodHome,
    plan: &ReconcilePlan,
    notices: &mut dyn Notices<LifecycleNotice>,
) -> ReconcileReport {
    let mut adoptions = Vec::new();
    for adoptable in &plan.adopting {
        if let Err(failure) = devpod_home.repoint(
            &adoptable.context,
            &adoptable.workspace_id,
            &adoptable.clone,
        ) {
            adoptions.push(Adoption::Refused {
                workspace_id: adoptable.workspace_id.clone(),
                failure,
            });
            continue;
        }
        // The second copy of the id, which is what stops this happening again:
        // after this the workspace is reachable from the record, so the next
        // derivation change costs nothing.
        //
        // Written into the record the metadata lock reloaded rather than into a
        // copy taken while the plan was being confirmed. The confirmation
        // prompt puts an unbounded wait between the read and the write, and a
        // whole-record write would carry every other field back across it.
        // `Absent` is the workspace having been removed while the plan sat
        // there, and re-inserting the record would undo that removal.
        //
        // Which arm comes back is what the report says, and that is the point
        // of there being arms: an adoption is "re-pointed" when both writes
        // happened, and the two endings where the second one did not are
        // `Unrecorded`. Reporting them as `Repointed` — which discarding the
        // answer amounts to — prints a line about a record dl never touched.
        let ending = match storage.update_worktree(
            &adoptable.record.owner,
            &adoptable.record.repo,
            &adoptable.record.branch,
            |record| record.devpod_workspace_id = Some(adoptable.workspace_id.clone()),
        ) {
            Ok((RecordUpdate::Applied, store_notices)) => {
                extend_with_store(notices, store_notices);
                Adoption::Repointed(Box::new(adoptable.clone()))
            }
            Ok((RecordUpdate::Absent, store_notices)) => {
                extend_with_store(notices, store_notices);
                Adoption::Unrecorded {
                    workspace_id: adoptable.workspace_id.clone(),
                }
            }
            Err(error) => {
                notices.say(LifecycleNotice::RecordNotDropped {
                    path: adoptable.record.local_path.clone(),
                    refusal: error,
                });
                Adoption::Unrecorded {
                    workspace_id: adoptable.workspace_id.clone(),
                }
            }
        };
        adoptions.push(ending);
    }
    // devpod's records just changed, so any listing dl is holding describes the
    // world before the repair.
    context.forget_workspaces();
    refresh.ask(context.runner(), RefreshReason::Forced);
    ReconcileReport { adoptions }
}
