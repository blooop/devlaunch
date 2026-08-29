//! `dl --purge`: the workspaces devlaunch created, and its whole cache directory.

use std::path::{Path, PathBuf};

use super::delete::{
    SweepOccasion, VolumeRefusal, VolumeSweep, devcontainer_volumes, sweep_volumes,
};
use crate::clients::devpod::{self, Call, ListingUnreadable, NotRun};
use crate::clients::devpod_home::DevpodHome;
use crate::domain::workspace_state::NonEmpty;
use crate::flows::listing::{self, CommandContext, WorkspaceOwnership};
use crate::flows::repo_manager::{Refusal, TreeSweep, present, remove_tree_as_far_as_it_goes};
use crate::runner::Exit;

/// What a purge would take, settled before the question is asked.
///
/// A value for the reason [`WorkspaceOwnership`] is one: the count a user approves
/// and the set that actually dies must come from the same object, and here they
/// cannot disagree — [`purge_all_data`] deletes exactly `ownership.mine`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgePlan {
    // Private like [`PrunePlan`]'s fields: a plan a caller could assemble is a
    // count approved for one set and a delete acting on another.
    /// Everything devlaunch stores on this machine. Removed whole.
    cache_dir: PathBuf,
    /// The workspaces devlaunch made, and the ones it did not. The second half is
    /// *named* rather than merely excluded from the count: a user who asked for a
    /// clean slate and gets survivors should learn it while saying no is still an
    /// option, rather than from a later `dl --ls`.
    pub(super) ownership: WorkspaceOwnership,
}

impl PurgePlan {
    /// The cache directory the purge removes whole.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// The workspaces devlaunch made, and the ones it did not.
    pub fn ownership(&self) -> &WorkspaceOwnership {
        &self.ownership
    }
}

/// What a purge would do. One `devpod list`, read before anything is destroyed.
///
/// A listing devpod could not answer is an error rather than an empty plan: a
/// purge that quietly did nothing used to look exactly like a purge that had
/// nothing to do.
pub fn purge_plan(
    context: &mut CommandContext<'_>,
    cache_dir: &Path,
) -> Result<PurgePlan, ListingUnreadable> {
    let workspaces = context.workspaces()?;
    Ok(PurgePlan {
        cache_dir: cache_dir.to_path_buf(),
        ownership: listing::workspace_ownership(&workspaces, cache_dir),
    })
}

/// Something a purge is about to do, or has just failed to do.
///
/// Handed over as it happens rather than collected, because "Deleting workspace X"
/// is said *before* the round trip that may take a while, and a report assembled
/// afterwards cannot say it in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurgeStep {
    /// About to ask devpod to delete this workspace.
    Deleting { workspace_id: String },
    /// devpod refused, and the purge carried on: one failed delete must not cost
    /// the rest of the cache its removal. The step is the failure's one report —
    /// it used to be doubled as a [`LifecycleNotice`](super::LifecycleNotice)
    /// too, and the binary carried a filter whose whole job was to drop the
    /// second copy.
    NotDeleted {
        workspace_id: String,
        exit: Exit,
        stderr: String,
    },
    /// The workspace went and the named docker volumes its devcontainer created
    /// are still there. Said for the reason
    /// [`LifecycleNotice::VolumesNotRemoved`](super::LifecycleNotice::VolumesNotRemoved)
    /// is said on the `rm` path — a purge promises a clean slate, and disk it
    /// could not reclaim is the one thing worth naming about it. The three sweeps
    /// that went fine say nothing.
    VolumesNotRemoved {
        workspace_id: String,
        occasion: SweepOccasion,
        refusal: VolumeRefusal,
    },
}

/// How a purge ended.
///
/// Five arms where Python has an exit code and four print sites. The two refusal
/// arms are the devlaunch#182 distinction — "one clone stayed behind" and "not a
/// byte of it moved" used to arrive at the caller as the same value, and the second
/// printed the first's sentence — and they are kept apart here rather than
/// re-derived from a refusal list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurgeOutcome {
    /// Nothing of devlaunch's on this machine: no workspaces of its own, and no
    /// cache directory.
    NothingToPurge,
    /// The workspaces went; there was no cache directory to remove.
    ///
    /// Its own arm rather than [`PurgeOutcome::NothingToPurge`], because a purge
    /// that deleted four workspaces has not done nothing. Python reached the same
    /// exit code by a branch that printed neither sentence.
    NoCacheDirectory,
    /// The cache directory is gone.
    Removed { cache_dir: PathBuf },
    /// Some of the cache came away. These refused.
    RemovedWhatItCould {
        cache_dir: PathBuf,
        refused: NonEmpty<Refusal>,
    },
    /// None of the cache came away. These refused.
    RemovedNothing {
        cache_dir: PathBuf,
        refused: NonEmpty<Refusal>,
    },
}

impl PurgeOutcome {
    /// Whether the cache is gone. The one distinction an exit code can carry.
    pub fn finished(&self) -> bool {
        matches!(
            self,
            Self::NothingToPurge | Self::NoCacheDirectory | Self::Removed { .. }
        )
    }
}

/// Delete the workspaces devlaunch created, then its cache directory.
///
/// Ownership-scoped: only `plan.ownership.mine`, which is the set whose *source*
/// this is about to delete anyway (see
/// [`listing::is_devlaunch_clone`](crate::flows::listing::is_devlaunch_clone)).
/// Anything else keeps working afterwards, because nothing a purge touches backs
/// it.
///
/// The plan is the one the caller printed the count from, so the confirmation the
/// user answered and the set actually deleted cannot disagree.
///
/// One failed `devpod delete` does not stop the run: the cache directory is the
/// larger half of what a purge frees, and a workspace devpod would not let go of
/// is a line in the report rather than a reason to leave gigabytes on disk. A
/// devpod that could not be *run at all* is a different matter and propagates —
/// nothing after this point would work either.
///
/// A cache that does not come away completely is reported rather than raised: see
/// [`remove_tree_as_far_as_it_goes`] for why it is removed as far as it goes.
///
/// **The ordering here is a property and not a habit.** The workspaces are deleted
/// first, which sweeps each one's volumes from devpod's own live record, and only
/// then is the cache removed — and that cache holds devlaunch's copies of those
/// same names ([`crate::flows::kept_copies`]). Interrupted between the two, the
/// copies that are lost belong to workspaces already swept, so nothing is lost that
/// was not already reclaimed. The reverse ordering is the one that loses bytes
/// permanently: it would destroy every copy and leave the volumes standing, which
/// is exactly the orphan population devlaunch#451 declined to reclaim.
pub fn purge_all_data(
    context: &mut CommandContext<'_>,
    plan: &PurgePlan,
    devpod_home: Option<&DevpodHome>,
    on_step: &mut dyn FnMut(PurgeStep),
) -> Result<PurgeOutcome, NotRun> {
    for workspace in &plan.ownership.mine {
        on_step(PurgeStep::Deleting {
            workspace_id: workspace.id.clone(),
        });
        // Named before the delete for the reason [`workspace_delete`] names them
        // before its own: devpod's record goes with the workspace.
        let named = devcontainer_volumes(devpod_home, &workspace.id);
        let answer = devpod::capture(context.runner(), &purge_delete_call(&workspace.id))?;
        if !answer.succeeded() {
            on_step(PurgeStep::NotDeleted {
                workspace_id: workspace.id.clone(),
                exit: answer.exit,
                stderr: answer.stderr().to_owned(),
            });
            // No sweep: the container is still there holding the volumes, so the
            // removal would refuse anyway, and a second report of one failure is
            // exactly what the `NotDeleted` arm exists to avoid.
            continue;
        }
        if let VolumeSweep::Refused(refusal) = sweep_volumes(context.runner(), named) {
            on_step(PurgeStep::VolumesNotRemoved {
                workspace_id: workspace.id.clone(),
                occasion: SweepOccasion::DevpodResult,
                refusal,
            });
        }
    }
    if !plan.ownership.mine.is_empty() {
        context.forget_workspaces();
    }

    // `present`, not an existence check: a cache that is there but unreachable
    // must be reached for, not reported as nothing to do. A cache whose *parent*
    // cannot be traversed used to come out as "No data to purge." and exit 0 with
    // the cache fully intact — a clean sweep reported over untouched data.
    if !present(&plan.cache_dir) {
        return Ok(if plan.ownership.mine.is_empty() {
            PurgeOutcome::NothingToPurge
        } else {
            PurgeOutcome::NoCacheDirectory
        });
    }
    let cache_dir = plan.cache_dir.clone();
    Ok(match remove_tree_as_far_as_it_goes(&cache_dir) {
        TreeSweep::Everything => PurgeOutcome::Removed { cache_dir },
        TreeSweep::WhatItCould(refused) => PurgeOutcome::RemovedWhatItCould { cache_dir, refused },
        TreeSweep::Nothing(refused) => PurgeOutcome::RemovedNothing { cache_dir, refused },
    })
}

/// `devpod delete <id> --force`, captured — argv-exact.
///
/// `--force` here is devpod's, not dl's: the workspace is being destroyed along
/// with the directory it opens, so a container devpod cannot reach cleanly must
/// not leave a record behind. Captured because a refusal is reported and stepped
/// over rather than shown live.
fn purge_delete_call(workspace_id: &str) -> Call {
    Call::new(["delete", workspace_id, "--force"])
}
