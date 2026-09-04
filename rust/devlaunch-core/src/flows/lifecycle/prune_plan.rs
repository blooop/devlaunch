//! `dl --prune`: the plan.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::delete::VolumeRefusal;
use super::delete_guard::Insisted;
use super::locations::{
    Unlocatable, WorkspaceLocations, canonical, subdirectories, workspace_locations,
};
use super::notices::{LifecycleNotice, extend_with_cache};
use super::prune_status::{Decision, KeptBecause, Promotion, RepoAt, clone_status, decide};
use crate::clients::devpod::{ListingUnreadable, Workspace};
use crate::domain::locks::LockError;
use crate::domain::metadata::{MetadataStorage, WorktreeFilter};
use crate::domain::model::WorktreeInfo;
use crate::domain::workspace_state::NonEmpty;
use crate::flows::agent_worktrees::{self, WorktreeSweep};
use crate::flows::disk_usage::{self, DiskUsage};
use crate::flows::kept_copies::KeptCopies;
use crate::flows::launch_locks::LaunchLocks;
use crate::flows::listing::{self};
use crate::flows::repo_manager::{CacheNotice, present};
use crate::flows::workspace_clone::WorkspaceCloneManager;
use crate::notices::Notices;

/// One clone directory this run will remove, what it frees, and why it may.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reclaimable {
    pub path: PathBuf,
    pub owner: String,
    pub repo: String,
    pub usage: DiskUsage,
    /// How the second pass knows whether `--force` was answering *this* directory.
    pub promotion: Promotion,
}

/// One workspace's volumes this run will reclaim, and the names it will reclaim.
///
/// **The names ride on the record, never as a plan-wide list.** That is this map's
/// second precedent, and the reason is the one [`PrunePlan`] gives for having no
/// `force` field: a list beside the entries is a list the acting pass can read
/// against the wrong entry, and the plan and the act would then disagree about
/// which volumes belonged to which workspace.
///
/// [`NonEmpty`] rather than a `Vec`, so an entry naming nothing has no
/// representation: a copy that names no volume is not in the plan at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclaimableVolumes {
    /// The workspace devpod no longer lists, whose copy named these.
    pub workspace_id: String,
    /// The volume names devlaunch copied out of devpod's create result at the tail
    /// of the last completed `up` of this workspace.
    pub names: NonEmpty<String>,
}

/// One workspace's volumes the acting pass would not remove, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumesKept {
    pub workspace_id: String,
    pub because: VolumesKeptBecause,
}

/// Why one workspace's volumes are staying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumesKeptBecause {
    /// devpod lists this workspace again. It did not when the plan was printed, and
    /// a live workspace's volumes are not leftovers — the same shrinking-only
    /// direction the clone re-classification moves in.
    ListedAgain,
    /// docker would not release them. The copy is kept, so the retry survives.
    Refused(VolumeRefusal),
}

/// One clone directory this run will leave standing, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub path: PathBuf,
    pub because: KeptBecause,
}

/// Everything one `dl --prune` will do, settled before anything is asked.
///
/// The two lists are built by one pass over one `decide` call each, so a
/// directory cannot be in both and cannot be in neither.
///
/// There is deliberately no `force` field. It was one, and a plan-wide boolean is
/// exactly the shape `decide` refuses to have beside a status: the pass that acts
/// read it and skipped its safety re-check for every directory, including the ones
/// `--force` had promoted nothing about. What `--force` answered rides on each
/// [`Reclaimable`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunePlan {
    // Private, all of them: a plan is [`prune_plan`]'s answer, and fields a caller
    // could fill would let one be assembled from a root and a classification that
    // never met — the mismatch [`ClonePlacement`] exists to make inexpressible.
    pub(super) root: PathBuf,
    /// Biggest first: the report's job is to be acted on, and "which of these is
    /// worth reclaiming" is the comparative question. Path breaks ties so two runs
    /// over an unchanged cache read alike.
    pub(super) removing: Vec<Reclaimable>,
    pub(super) keeping: Vec<Kept>,
    /// Worktree records whose directory is definitively not there any more.
    pub(super) stale_records: Vec<WorktreeInfo>,
    /// The volumes of the workspaces devpod has forgotten, from devlaunch's own
    /// copies of what devpod substituted. A second enumeration beside the clone
    /// walk rather than a column on it: see [`prune_plan`].
    pub(super) reclaiming: Vec<ReclaimableVolumes>,
    /// The agent git worktrees inside the clones this run is *keeping*
    /// (devlaunch#426). Only the kept ones, which is what stops their bytes
    /// being counted twice: a clone this run removes already accounts for
    /// everything inside it.
    pub(super) worktrees: WorktreeSweep,
    /// The workspaces whose launch lock names nothing devpod lists any more, from
    /// devlaunch's own lock directory. A third enumeration beside the clone walk
    /// and the copies, for the reason the second one is one: see [`prune_plan`].
    ///
    /// Ids and not paths, because the store owns the spelling of the path and a
    /// plan carrying one would be a second place that decides where a lock lives.
    pub(super) locks: Vec<String>,
}

impl PrunePlan {
    /// Whether this run would change nothing at all.
    pub fn nothing_to_do(&self) -> bool {
        self.removing.is_empty()
            && self.stale_records.is_empty()
            && self.reclaiming.is_empty()
            && self.locks.is_empty()
            && self.worktrees.nothing_to_do()
    }

    /// What removing the *clone directories* would free.
    ///
    /// **The agent worktrees are deliberately not in it** (devlaunch#442 review,
    /// S2). Their bytes have their own sentence, because they are a different
    /// claim: every one of them is inside a clone this run has just said it is
    /// keeping, so folding them in here made the headline number describe
    /// directories that are not going, and then said the same bytes twice. Ask
    /// [`Self::worktrees`] for that figure.
    pub fn clones_freed(&self) -> DiskUsage {
        disk_usage::total_usage(self.removing.iter().map(|it| it.usage.clone()))
    }

    /// The directory the plan's candidates were scanned under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directories this run will remove.
    pub fn removing(&self) -> &[Reclaimable] {
        &self.removing
    }

    /// The directories this run will leave standing, and why.
    pub fn keeping(&self) -> &[Kept] {
        &self.keeping
    }

    /// The records this run will drop for directories already gone.
    pub fn stale_records(&self) -> &[WorktreeInfo] {
        &self.stale_records
    }

    /// The volumes this run will reclaim from devlaunch's kept copies.
    pub fn reclaiming(&self) -> &[ReclaimableVolumes] {
        &self.reclaiming
    }

    /// The agent git worktrees inside the clones this run is keeping.
    pub fn worktrees(&self) -> &WorktreeSweep {
        &self.worktrees
    }

    /// The workspaces whose launch lock this run will reclaim.
    pub fn reclaiming_locks(&self) -> &[String] {
        &self.locks
    }
}

/// The directory `--prune` scans, canonicalised once.
///
/// The clone root as the manager reports it. Asking the manager rather than
/// rebuilding `<cache>/repos` here is what keeps the directories scanned, the
/// locks taken and the workspace sources compared answering to one root, so they
/// cannot drift into scanning one tree while serialising against another or
/// comparing against a third. Since #467 that root is derived from the cache
/// directory and nothing a user writes can move it, so the three agree by
/// construction as well as by plumbing.
///
/// Absent is not a failure — a fresh install has no repos directory yet, and
/// resolving one that is not there is what says so.
pub(crate) fn clone_root(clones: &WorkspaceCloneManager<'_>) -> PathBuf {
    let repos_dir = clones.repos_dir();
    canonical(&repos_dir.to_string_lossy()).unwrap_or_else(|| repos_dir.to_path_buf())
}

/// The tree a maintenance command scans and where devpod's workspaces sit in it,
/// resolved together.
///
/// [`prune_plan`] and [`reconcile_plan`](super::reconcile_plan) classify a
/// directory by joining two facts: the root the candidates are scanned under,
/// and where every live
/// workspace's source resolves *against that same root*. Taken as two parameters
/// the pair could be built from two different roots — and the join would then
/// mis-classify a clone, reading a healthy one whose workspace was placed against
/// the other root as sourced by nobody, which is an orphan, which is a deletion.
/// One constructor derives both halves from one root, so a mismatched pair has no
/// representation.
#[derive(Debug, Clone, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub struct ClonePlacement {
    pub(super) root: PathBuf,
    pub(super) locations: WorkspaceLocations,
}

impl ClonePlacement {
    /// Resolve `workspaces` against the tree `clones` manages (see
    /// [`clone_root`]). The only way to build one.
    pub fn resolve(clones: &WorkspaceCloneManager<'_>, workspaces: &[Workspace]) -> Self {
        let root = clone_root(clones);
        let locations = workspace_locations(workspaces, &root);
        Self { root, locations }
    }

    /// The live workspaces this command cannot place, or nothing when every one
    /// of them placed itself. See `WorkspaceLocations::unlocatable`.
    pub fn unlocatable(&self) -> Option<NonEmpty<Unlocatable>> {
        self.locations.unlocatable()
    }
}

/// Why a prune could not be carried out.
#[derive(Debug)]
pub enum PruneError {
    /// A repository's lock could not be taken, so its clones were neither weighed
    /// nor removed. Fatal rather than skipped: a scan that silently left out a
    /// repository would report a plan that is not the plan.
    Lock(LockError),
    /// devpod's listing could not be read, so nothing can be called unreferenced.
    Listing(ListingUnreadable),
}

/// Classify every clone directory under the cache, one repository at a time.
///
/// Every candidate path is canonical without ever being resolved individually — a
/// resolved root (see [`clone_root`]) plus real directory names, symlinks skipped.
///
/// The per-repo lock is held while a repository's clones are looked at, because
/// [`WorkspaceCloneManager`] populates a clone fully before it returns and without
/// this a scan can weigh — or delete — a directory `git clone` is still writing
/// into. It closes that window and not a wider one: devpod only learns about a
/// clone *after* the lock is released, so a clone whose launch has finished cloning
/// and not yet registered a workspace is briefly indistinguishable from a stale
/// one.
///
/// # The volumes are a second enumeration, not a column on the first
///
/// `--prune` also reclaims the docker volumes of workspaces devpod has forgotten,
/// read from devlaunch's own copies ([`crate::flows::kept_copies`]), and the domain
/// of that pass is **the set of copies** rather than the set of clone directories.
/// A copy whose clone the user deleted by hand names volumes no clone-shaped walk
/// will ever reach, and reasoning over an enumeration that does not cover what it
/// affects is the defect class devlaunch#445 exists to close. The precondition per
/// copy is one thing: no workspace devpod lists carries that id.
///
/// `--prune` is the surface because it already owns "the workspace is gone and its
/// leftovers are ours", and because without this every prune that removed an
/// orphaned clone *manufactured* a permanent orphan — devpod's record died at the
/// outside delete, so once the clone went the volumes were unreachable forever. It
/// still deletes no workspace, no container and no image.
///
/// # And the launch locks are a third, for the same argument
///
/// The per-workspace launch locks ([`crate::flows::launch_locks`]) are enumerated
/// the same way and against the same precondition, and the reason they are not a
/// column on either pass above is the reason the volumes are not a column on the
/// clone walk: a lock exists for workspaces that have no clone under the cache at
/// all, and for workspaces that never completed an `up` and so left no copy. Until
/// devlaunch#575 nothing reclaimed one short of `--purge` removing the whole cache
/// directory.
#[allow(clippy::too_many_arguments)]
pub fn prune_plan(
    clones: &WorkspaceCloneManager<'_>,
    storage: &MetadataStorage,
    workspaces: &[Workspace],
    copies: &KeptCopies,
    launch_locks: &LaunchLocks,
    placement: &ClonePlacement,
    insisted: Insisted,
    notices: &mut dyn Notices<LifecycleNotice>,
) -> Result<PrunePlan, PruneError> {
    let ClonePlacement { root, locations } = placement;
    let mut removing: Vec<Reclaimable> = Vec::new();
    let mut keeping: Vec<Kept> = Vec::new();
    let mut worktrees = WorktreeSweep::default();
    let mut cache_notices = Vec::new();
    let record_for = records_by_directory(clones, storage, &mut cache_notices);
    let listed_at = sources_by_workspace(workspaces);
    let git = clones.repo_manager().git();
    for owner_dir in subdirectories(root) {
        for repo_dir in subdirectories(&owner_dir) {
            let (Some(owner), Some(repo)) = (leaf_of(&owner_dir), leaf_of(&repo_dir)) else {
                continue;
            };
            let bare_path = clones.repo_manager().bare_dir(&owner, &repo);
            let bare = canonical(&bare_path.to_string_lossy());
            let _lock = clones
                .repo_manager()
                .hold_repo_lock(&owner, &repo)
                .map_err(PruneError::Lock)?;
            for clone in subdirectories(&repo_dir) {
                if bare.as_deref() == Some(clone.as_path()) {
                    // Never a candidate and never reported. Nothing sources it and
                    // no record names it, so every rule above would call it an
                    // orphan — and it is the copy every clone of this repository
                    // hardlinks its git objects out of, which is the reason the
                    // next clone is fast.
                    continue;
                }
                let status = clone_status(
                    &git,
                    &clone,
                    RepoAt {
                        owner: &owner,
                        repo: &repo,
                        bare: &bare_path,
                    },
                    locations,
                    &record_for,
                    &listed_at,
                );
                match decide(status, insisted.clones) {
                    Decision::Remove { usage, promotion } => removing.push(Reclaimable {
                        path: clone,
                        owner: owner.clone(),
                        repo: repo.clone(),
                        usage,
                        promotion,
                    }),
                    Decision::Keep(because) => {
                        // The agent worktrees inside a clone are swept only
                        // where the clone itself is staying — a clone that is
                        // going already accounts for everything inside it — and
                        // the sweep runs here, under the same repository lock
                        // the classification was taken under.
                        worktrees.record(agent_worktrees::sweep_clone(
                            &git,
                            &clone,
                            &owner,
                            &repo,
                            bare.as_deref(),
                            insisted.worktrees,
                        ));
                        keeping.push(Kept {
                            path: clone,
                            because,
                        });
                    }
                }
            }
        }
    }
    removing.sort_by(|left, right| {
        right
            .usage
            .known_bytes()
            .cmp(&left.usage.known_bytes())
            .then_with(|| left.path.cmp(&right.path))
    });
    let stale_records = records_for_absent_directories(clones, storage, &mut cache_notices);
    extend_with_cache(notices, cache_notices);
    Ok(PrunePlan {
        root: root.to_path_buf(),
        removing,
        keeping,
        stale_records,
        reclaiming: reclaimable_volumes(copies, workspaces),
        worktrees,
        locks: reclaimable_locks(launch_locks, workspaces),
    })
}

/// The workspaces holding a launch lock that devpod no longer lists.
///
/// The same one precondition [`reclaimable_volumes`] asks, of the same listing, for
/// the same reason -- and it is the whole of what this pass decides. Whether the
/// lock is *free* is not asked here and could not usefully be: the answer would be
/// taken before the user has answered the question and acted on afterwards, so it
/// is asked where it can be acted on, at the moment of the unlink
/// ([`crate::domain::locks::reclaim`]).
fn reclaimable_locks(launch_locks: &LaunchLocks, workspaces: &[Workspace]) -> Vec<String> {
    let live: HashSet<&str> = workspaces.iter().map(|it| it.id.as_str()).collect();
    launch_locks
        .keyed()
        .into_iter()
        .filter(|workspace_id| !live.contains(workspace_id.as_str()))
        .collect()
}

/// The copies whose workspace devpod no longer lists, each carrying its own names.
///
/// The one precondition, and it is asked of `workspaces` rather than of the disk:
/// devpod's listing is the only thing that can say a workspace is still alive, and
/// a copy for a live one names volumes that are in use. Answering it from the
/// listing the caller already has is what lets the acting pass ask it again for
/// free, under the second listing it already pays for.
fn reclaimable_volumes(copies: &KeptCopies, workspaces: &[Workspace]) -> Vec<ReclaimableVolumes> {
    let live: HashSet<&str> = workspaces.iter().map(|it| it.id.as_str()).collect();
    copies
        .copied()
        .into_iter()
        .filter(|workspace_id| !live.contains(workspace_id.as_str()))
        .filter_map(|workspace_id| {
            let names = copies.volumes(&workspace_id)?;
            Some(ReclaimableVolumes {
                workspace_id,
                names,
            })
        })
        .collect()
}

/// `metadata.json`'s worktree records, keyed by the directory each names.
///
/// Which directory a record names is
/// [`WorkspaceCloneManager::resolve_clone_path`]'s question and not this
/// function's, and asking it here instead was the shape devlaunch#174 was:
/// `local_path` read raw is one of *two* answers a record can give, and the other
/// one is what the delete acts on. The consequence here is the dangerous direction
/// rather than the merely inconsistent one — a record that missed its clone leaves
/// that clone with no record at all, which drops it out of
/// [`CloneStatus::Disputed`] and into [`CloneStatus::Orphaned`], which is a
/// deletion.
///
/// A record dl cannot name a directory for at all is left out. It cannot be matched
/// to a candidate by definition, and there is no path it could be filed under that
/// would not be a guess.
pub(super) fn records_by_directory(
    clones: &WorkspaceCloneManager<'_>,
    storage: &MetadataStorage,
    notices: &mut dyn Notices<CacheNotice>,
) -> HashMap<PathBuf, WorktreeInfo> {
    let mut records = HashMap::new();
    for record in storage.list_worktrees(WorktreeFilter::All) {
        let Some(directory) = clones.resolve_clone_path(record, notices) else {
            continue;
        };
        if let Some(resolved) = canonical(&directory.to_string_lossy()) {
            records.insert(resolved, record.clone());
        }
    }
    records
}

/// The worktree records whose directory is definitively not there any more.
///
/// `metadata.json` is append-mostly and nothing has ever pruned it: 49 records for
/// 17 live workspaces on the reference host. These are the ones that describe
/// nothing at all.
///
/// "Definitively" is [`present`]'s distinction and it is load-bearing here too: a
/// directory this process is not allowed to look at is still a directory, and
/// dropping its record would lose the only note of where a clone lives. A record dl
/// cannot name a directory for is not dropped either — "dl could not work out where
/// this is" is not "this is not there", and only the second is a reason to forget
/// it.
fn records_for_absent_directories(
    clones: &WorkspaceCloneManager<'_>,
    storage: &MetadataStorage,
    notices: &mut dyn Notices<CacheNotice>,
) -> Vec<WorktreeInfo> {
    storage
        .list_worktrees(WorktreeFilter::All)
        .into_iter()
        .filter(|record| {
            clones
                .resolve_clone_path(record, notices)
                .is_some_and(|directory| !present(&directory))
        })
        .cloned()
        .collect()
}

/// Every listed workspace's source, as the `SOURCE` column renders it.
pub(super) fn sources_by_workspace(workspaces: &[Workspace]) -> HashMap<String, String> {
    workspaces
        .iter()
        .map(|workspace| {
            (
                workspace.id.clone(),
                listing::describe_source(&workspace.source).detail,
            )
        })
        .collect()
}

pub(super) fn leaf_of(path: &Path) -> Option<String> {
    Some(path.file_name()?.to_string_lossy().into_owned())
}
