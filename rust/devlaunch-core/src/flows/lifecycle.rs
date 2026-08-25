//! Stopping, deleting and cleaning up: everything that removes something.
//!
//! Ported from the lifecycle half of `dl.py` — `workspace_stop`,
//! `workspace_delete` and its unsaved-work guard, `get_workspace_state`,
//! `resolve_known_workspace`, `prune_command`, `purge_all_data`,
//! `reconcile_command`, `update_cache_background` and `sweep_repo_fetches`. See
//! docs/rust-rewrite-plan.md (M6); this is also `wf`'s consumption surface
//! (#250), which is why everything here is public rather than module-private and
//! why the names are the ones a caller outside dl would reach for.
//!
//! Everything reachable here is **binary surface — not part of the frozen wf API
//! (#251 §7)**, except the three §7 names (`list`, `remove`, `up`): the `dl`
//! binary is a separate crate and every sentence a user reads is written there,
//! so a rendering layer that could not name these typed results would not be a
//! rendering layer. The distinction is what stays frozen at the end of M6.
//!
//! # Three commands remove things, and none of them decides what is finished
//!
//! - **`dl <ws> rm`** deletes one workspace, its clone and the named docker
//!   volumes its devcontainer created ([`VolumeSweep`]), and refuses when the
//!   clone holds work that exists nowhere else. That refusal is the one judgement
//!   dl makes on its own account (see [`crate::domain::workspace_state`]).
//! - **`dl --prune`** removes the clone *directories* no live workspace opens. It
//!   never touches a devpod workspace, a container, an image or a volume, and it
//!   leaves every bare cache alone. **Still true of volumes** after devlaunch#325,
//!   and not an oversight: it deletes no workspace, so there is no workspace whose
//!   volumes it could be taking.
//! - **`dl --purge`** deletes the workspaces devlaunch created — volumes and all —
//!   and its whole cache directory. Ownership-scoped
//!   ([`crate::flows::listing::workspace_ownership`]), and it names what it leaves
//!   standing. It does *not* share `rm`'s delete: it issues its own captured
//!   `devpod delete --force` per workspace, which is why the volume sweep is wired
//!   into it explicitly rather than inherited.
//!
//! `dl --reconcile` is the fourth of the family and removes nothing at all: it
//! re-points devpod records the id-scheme change orphaned (devlaunch#88), and an
//! orphan it cannot adopt is reported and left where it is.
//!
//! # The plan is a value, and the question is the binary's
//!
//! Every one of these commands prints what it would do, asks, and then acts. The
//! report a user answers and the set that actually dies must come from the *same
//! object*, because the difference between them is somebody's directory — so the
//! classification is a value ([`PrunePlan`], [`PurgePlan`], [`ReconcilePlan`])
//! that core hands over, the `y/N` question belongs to the binary, and the acting
//! pass takes the plan back. `-y` never reaches core: it is the binary's answer to
//! its own question.
//!
//! Nothing here prints and nothing here is a sentence. Every refusal, notice and
//! outcome is a typed value carrying exactly what the line it replaces
//! interpolated; the words and the exit codes are the `dl` binary's (#251).
//!
//! # Every mutation forgets the snapshot
//!
//! A flow that changes what `devpod list` would say takes the
//! [`CommandContext`] **mutably** and calls
//! [`CommandContext::forget_workspaces`] — Python's
//! `invalidate_workspace_list_cache()`. Taking it mutably is the point: a flow
//! that mutates devpod cannot be handed a shared reference, so it cannot quietly
//! leave a stale snapshot behind.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::clients::devpod::{
    self, Call, ContainerState, ListingUnreadable, NotRun, StatusUnreadable, Workspace,
    WorkspaceSource,
};
use crate::clients::devpod_home::{DevpodHome, RepointFailure, sole_workspace_result};
use crate::clients::docker;
use crate::clients::git::Git;
use crate::domain::locks::{self, LockError};
use crate::domain::metadata::{self, MetadataStorage, RecordUpdate, WorktreeFilter};
use crate::domain::model::WorktreeInfo;
use crate::domain::workspace_state::{self, CouldNotTell, Losses, NonEmpty, Unsaved};
use crate::flows::completion_cache;
use crate::flows::disk_usage::{self, DiskUsage};
use crate::flows::listing::{
    self, ClonePathResolver, CommandContext, WorkspaceOwnership, json_as_python_writes_it,
};
use crate::flows::repo_manager::{
    BACKGROUND_FETCH_TIMEOUT, CacheNotice, Fetched, LazyFetchError, Refusal, Removal,
    RepositoryManager, present, remove_tree_as_far_as_it_goes,
};
use crate::flows::workspace_clone::{RemoveWorkspaceError, Removed, WorkspaceCloneManager};
use crate::notices::{Notices, Wrapped};
use crate::runner::{DetachOutcome, Exit, Invocation, OsFailure, Runner};
use crate::timing;

// ===========================================================================
// notices
// ===========================================================================

/// Something a lifecycle flow did that the `dl` binary may want to report.
///
/// One vocabulary for the whole family, because a single command produces notices
/// from several of these flows. Every arm is one `logging.*` call Python made,
/// carrying what that line interpolated; nothing here is a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleNotice {
    /// The workspace's local clone was removed with it.
    CloneRemoved { workspace_id: String },
    /// devpod let go of the workspace and the clone could not be removed. The
    /// workspace is gone either way, so this is a notice rather than a failure.
    ///
    /// Carries the refusal itself, not a rendering of it: Python interpolates the
    /// exception (`Failed to remove local clone: {e}`) and the words for each arm
    /// are the `dl` binary's to choose.
    CloneNotRemoved {
        workspace_id: String,
        refusal: RemoveWorkspaceError,
    },
    /// devpod let go of the workspace and the named docker volumes its
    /// devcontainer created are still on this machine.
    ///
    /// A notice for the reason [`LifecycleNotice::CloneNotRemoved`] is one: the
    /// workspace is gone either way, and the disk left behind is a thing to say
    /// rather than a delete to fail. Carries the refusal itself and not a
    /// rendering of it — and can only be built from a refusal, because
    /// [`VolumeRefusal`] holds no arm for a sweep that went fine.
    VolumesNotRemoved {
        workspace_id: String,
        refusal: VolumeRefusal,
    },
    /// A clone directory went and its `metadata.json` record could not be
    /// dropped. Named by the path, which is what the record described.
    ///
    /// Carries the refusal itself, for the reason
    /// [`LifecycleNotice::CloneNotRemoved`] does: Python's line is `Could not drop
    /// the record for {path}: {e}`, and the `{e}` is the binary's to write.
    RecordNotDropped {
        path: PathBuf,
        refusal: metadata::MetadataError,
    },
    /// This command is addressing a devpod workspace named by the record rather
    /// than the one this build derives (devlaunch#88).
    AddressingRecordedWorkspace {
        recorded: String,
        derived: String,
        owner: String,
        repo: String,
        branch: String,
    },
    /// Something one of the storage flows reported on the way through.
    Cache(CacheNotice),
}

/// A lifecycle channel, as a storage flow's — for the callers that hand it down
/// rather than collecting a vector, which is what keeps a storage flow's own line in
/// the place Python logged it.
fn as_cache<'a>(
    notices: &'a mut dyn Notices<LifecycleNotice>,
) -> Wrapped<'a, CacheNotice, LifecycleNotice> {
    Wrapped::new(notices, LifecycleNotice::Cache)
}

/// Collect the notices one of the storage flows produced.
fn extend_with_cache(notices: &mut dyn Notices<LifecycleNotice>, cache: Vec<CacheNotice>) {
    notices.say_all(cache.into_iter().map(LifecycleNotice::Cache));
}

/// Collect the notices a `metadata.json` write produced.
fn extend_with_store(notices: &mut dyn Notices<LifecycleNotice>, store: Vec<metadata::Notice>) {
    extend_with_cache(
        notices,
        store.into_iter().map(CacheNotice::Metadata).collect(),
    );
}

// ===========================================================================
// the detached refresh child
// ===========================================================================

/// How to re-run *this build* as a detached child.
///
/// Supplied by the binary, and that is the whole reason it is a parameter: core
/// must never ask the OS who it is. `std::env::current_exe()` inside a library
/// answers `wf` when wf links it, and `python` when the harness drives it — so the
/// one process that knows which program it is hands the answer down.
///
/// Python spells the same thing as `[sys.executable, "-m", "devlaunch.dl"]`, which
/// is why the leading arguments are a list rather than nothing: the Python build's
/// re-invocation needs two of them and the Rust binary's needs none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfInvocation {
    program: String,
    leading_args: Vec<String>,
}

impl SelfInvocation {
    /// `program`, run with no leading arguments — the Rust binary's own shape.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            leading_args: Vec::new(),
        }
    }

    /// `program`, run with these arguments in front of the command — Python's
    /// `-m devlaunch.dl`. Only this module's tests build that shape; the Rust
    /// binary is [`SelfInvocation::new`] with no leading arguments.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_leading_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.leading_args = args.into_iter().map(Into::into).collect();
        self
    }

    /// The argv of the refresh child, argv-exact: `--update-cache`, and `--force`
    /// when the parent's refresh was forced.
    ///
    /// `--force` is passed on rather than re-decided by the child, because the
    /// child re-checks the TTL and would otherwise skip a refresh that follows a
    /// workspace change — where the cache is wrong however new it is.
    pub(crate) fn refresh_child(&self, reason: RefreshReason) -> Invocation {
        let mut invocation = Invocation::new(&self.program)
            .with_args(self.leading_args.iter().cloned())
            .with_arg(UPDATE_CACHE_FLAG);
        if let RefreshReason::Forced = reason {
            invocation = invocation.with_arg(FORCE_FLAG);
        }
        invocation
    }
}

/// The flag that puts a `dl` run in refresh-child mode.
pub(crate) const UPDATE_CACHE_FLAG: &str = "--update-cache";

/// The flag that tells a refresh — parent or child — to ignore the TTL.
pub(crate) const FORCE_FLAG: &str = "--force";

/// Why a background refresh is being asked for.
///
/// Named arms rather than Python's `force: bool`, because at the call site `True`
/// says nothing about *what* is being overridden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshReason {
    /// This command has just changed what the cache describes — a workspace
    /// created, stopped or deleted — so the cache is wrong however recently it was
    /// written.
    Forced,
    /// A command that only reads the cache is keeping it warm. The TTL decides.
    IfStale,
}

/// What asking for a background refresh turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshSpawn {
    /// A detached child is running. `pid` is here so a test can observe it;
    /// nothing in devlaunch waits on it.
    Spawned { pid: u32 },
    /// This command already spawned one. At most one per command.
    AlreadySpawned,
    /// The cache is new enough to leave alone. Deliberately does **not** consume
    /// the one spawn: it means "not needed yet", not "already done", so a later
    /// forced call can still get its refresh.
    CacheStillFresh,
    /// The child could not be started. Survivable: completions are a convenience,
    /// and a command that worked must not fail over one that could not be warmed.
    NotStarted(SpawnRefused),
}

/// Why the refresh child never started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnRefused {
    /// This build's own program is not where it said it was.
    ProgramNotFound,
    /// The OS refused the fork or exec.
    Blocked(crate::runner::OsFailure),
}

/// The one background refresh a command may spawn, and everything needed to spawn
/// it.
///
/// A value the binary makes one of per command, for the reason
/// [`CommandContext`] is one: Python held the latch in a module-level dict and had
/// to remember that a dl process is one command, so per-process state was
/// per-invocation state. Here a second command is a second `Refresh` and the reset
/// is structural.
///
/// It is threaded *into* the mutating flows rather than left beside them, so "a
/// command that changed the workspace list forces a refresh" is something a caller
/// cannot forget: [`workspace_stop`] and [`workspace_delete`] cannot be called
/// without one.
pub struct Refresh<'a> {
    updater: &'a SelfInvocation,
    /// The completion cache whose mtime decides whether an unforced refresh is
    /// worth spawning.
    cache_path: &'a Path,
    spawned: bool,
}

impl<'a> Refresh<'a> {
    pub fn new(updater: &'a SelfInvocation, cache_path: &'a Path) -> Self {
        Self {
            updater,
            cache_path,
            spawned: false,
        }
    }

    /// Whether this command has already spawned its refresh.
    pub fn spawned(&self) -> bool {
        self.spawned
    }

    /// Allow one more refresh, because the world changed *again* after the last one
    /// was spawned.
    ///
    /// The latch it clears is not a rate limit; it is "one command, one child", and
    /// what it protects against is a command that warmed the cache on its way in
    /// spawning a second child on its way out to describe the same world. That is not
    /// this. `dl <ws> --rm` is the one command that changes the workspace list
    /// **twice** — the launch may create a workspace and force the refresh that
    /// records it, and the removal then deletes it — so the child already spawned is
    /// indexing a world with a workspace in it that is about to be gone. Leaving the
    /// latch shut means the completion cache goes on offering a deleted workspace
    /// until its TTL expires.
    ///
    /// Deliberately no argument and no accounting: it re-arms by one, and the caller
    /// has to have a second state change to justify calling it. A caller that calls
    /// it without one gets the double spawn the latch exists to prevent, which is why
    /// this is a named method with this docstring rather than a `pub` field.
    pub fn rearm(&mut self) {
        self.spawned = false;
    }

    /// Refresh the completion cache in a detached process, if it is worth it.
    ///
    /// Skipped entirely when this command already spawned one, and — under
    /// [`RefreshReason::IfStale`] — when the cache is still fresh.
    pub fn ask(&mut self, runner: &dyn Runner, reason: RefreshReason) -> RefreshSpawn {
        if self.spawned {
            return RefreshSpawn::AlreadySpawned;
        }
        if let RefreshReason::IfStale = reason
            && completion_cache::completion_cache_is_fresh(self.cache_path)
        {
            return RefreshSpawn::CacheStillFresh;
        }
        // Latched before the spawn, as Python latches it: a spawn that the OS then
        // refuses must not leave a later call trying again, because whatever
        // refused it will refuse the next one too.
        self.spawned = true;
        match runner.detach(&self.updater.refresh_child(reason)) {
            DetachOutcome::Started { pid } => RefreshSpawn::Spawned { pid },
            DetachOutcome::ProgramNotFound => {
                RefreshSpawn::NotStarted(SpawnRefused::ProgramNotFound)
            }
            DetachOutcome::NotStarted(failure) => {
                RefreshSpawn::NotStarted(SpawnRefused::Blocked(failure))
            }
        }
    }
}

/// What the detached refresh child has to do.
///
/// The TTL is re-checked in the child as well as in the parent that spawned it:
/// two parents can both see a stale cache before either child has written one, and
/// the second sweep would be pure waste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildWork {
    /// Rewrite the completion cache, then sweep the bare-clone cache's fetches.
    ///
    /// Both or neither, and in that order: the completion cache is what the user's
    /// next keystroke reads, while the fetch sweep is for the launch after that.
    /// They are on the same hour, so a child that gets past the TTL does both.
    RefreshAndSweep,
    /// Another child got there first inside the TTL.
    NothingToDo,
}

/// Whether the refresh child has anything to do.
pub fn child_work(cache_path: &Path, reason: RefreshReason) -> ChildWork {
    match reason {
        RefreshReason::Forced => ChildWork::RefreshAndSweep,
        RefreshReason::IfStale if completion_cache::completion_cache_is_fresh(cache_path) => {
            ChildWork::NothingToDo
        }
        RefreshReason::IfStale => ChildWork::RefreshAndSweep,
    }
}

// ===========================================================================
// the fetch sweep
// ===========================================================================

/// What the sweep did about one repository.
///
/// Every arm is one of Python's `logging.debug` lines or the silence between them,
/// and the sweep is the one flow with nobody to complain to — it is a detached
/// child with no terminal attached — so these exist to be *counted* and to be
/// visible under `DEVLAUNCH_TIMING`, not to be printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SweptRepo {
    /// The interval had elapsed and the fetch worked.
    Fetched { owner: String, repo: String },
    /// The interval had not elapsed. Nothing was asked of the remote.
    NotDue { owner: String, repo: String },
    /// Another dl run holds this repository's lock, so the sweep stepped over it.
    ///
    /// **It never waits.** The lock is taken non-blockingly, so a repository some
    /// launch is mid-clone in is skipped rather than queued for — a sweep that
    /// waited would be taxing the very path it exists to keep clear. The interval
    /// brings it round again.
    Contended { owner: String, repo: String },
    /// The fetch was attempted and failed — an unreachable remote, a cache entry
    /// whose clone has been deleted underneath it, or a fetch that ran out of
    /// time. Stepped over, so one bad repository cannot cost the rest theirs.
    Failed {
        owner: String,
        repo: String,
        error: LazyFetchError,
    },
    /// The lock file itself could not be opened, so nothing was attempted.
    ///
    /// Carries the lock's own refusal rather than a rendering of it: which step
    /// failed (a parent directory, the open, the `flock`) is what a reader acts on,
    /// and the words are the `dl` binary's.
    LockUnavailable {
        owner: String,
        repo: String,
        refusal: LockError,
    },
}

/// Everything one sweep did.
#[derive(Debug, Default)]
pub struct SweepReport {
    pub(crate) repos: Vec<SweptRepo>,
    pub notices: Vec<LifecycleNotice>,
}

/// Bring the bare-clone cache up to date, one repository at a time.
///
/// The freshness fetch — `+refs/heads/*` plus tags plus prune — is a network call
/// of unbounded duration, and it used to run on the launch path under the per-repo
/// lock whenever the interval had elapsed. Whoever drew that straw paid for
/// everyone's freshness, and any concurrent launch of the same repository queued
/// behind them (devlaunch#149). Out here it costs a launch nothing: this is the
/// detached child, spawned and forgotten, with nobody waiting on its exit.
///
/// Three rules make it safe to run alongside real work, and only two of them are
/// free:
///
/// - **It never waits**, because the lock is taken with
///   [`locks::run_if_lock_free`].
/// - **It never holds a repository for long** — and saying "background defers to
///   foreground" would overstate the first rule, because the lock this takes is
///   the one a launch *blocks* on. So the honest statement is the asymmetric one:
///   the sweep never queues for a launch, but a launch can queue for the sweep.
///   What keeps that survivable is that the wait has an upper bound rather than
///   the network's — [`BACKGROUND_FETCH_TIMEOUT`], without which a remote that
///   accepts a connection and then goes quiet holds the repository for as long as
///   the kernel keeps the socket.
/// - **It never complains.** Every failure is an arm of `SweptRepo` and the loop
///   carries on.
///
/// The interval itself is unchanged and still recorded in the one shared place
/// (`last_fetched` in metadata), which is what lets the launch path go on
/// consulting it: whichever side fetches first, the other sees a fresh clock and
/// does nothing.
pub fn sweep_repo_fetches(
    repos: &RepositoryManager<'_>,
    storage: &mut MetadataStorage,
) -> SweepReport {
    // The pairs are collected first: `lazy_fetch` needs the store mutably, and the
    // listing borrows it.
    let managed: Vec<(String, String)> = repos
        .list_repositories(storage)
        .into_iter()
        .map(|repository| (repository.owner, repository.repo))
        .collect();
    let mut report = SweepReport::default();
    for (owner, repo) in managed {
        let lock_path = repos.lock_path(&owner, &repo);
        let mut cache_notices = Vec::new();
        let swept = locks::run_if_lock_free(&lock_path, || {
            repos.lazy_fetch(
                storage,
                &owner,
                &repo,
                Some(BACKGROUND_FETCH_TIMEOUT),
                &mut cache_notices,
            )
        });
        extend_with_cache(&mut report.notices, cache_notices);
        report.repos.push(match swept {
            Err(refusal) => SweptRepo::LockUnavailable {
                owner,
                repo,
                refusal,
            },
            Ok(None) => SweptRepo::Contended { owner, repo },
            Ok(Some(Ok(Fetched::Fetched))) => SweptRepo::Fetched { owner, repo },
            Ok(Some(Ok(Fetched::Skipped))) => SweptRepo::NotDue { owner, repo },
            Ok(Some(Err(error))) => SweptRepo::Failed { owner, repo, error },
        });
    }
    report
}

// ===========================================================================
// which workspace, and what state
// ===========================================================================

/// The container state devpod reports for one workspace.
///
/// Charged to the `devpod-up` stage, as Python's `@timing.staged("devpod-up")`
/// charges it: this round trip is what a warm attach spends instead of building a
/// container, and a summary that left it uncharged would show a launch with a gap
/// in it. A stage *guard* rather than
/// [`timing::stage_result`](crate::timing::stage_result), because an unreadable
/// answer is Python's `None` return and not an exception — the stage completed,
/// devpod just had nothing to say.
pub fn workspace_state(
    runner: &dyn Runner,
    workspace_id: &str,
) -> Result<ContainerState, StatusUnreadable> {
    let mut stage = timing::stage(timing::Stage::DevpodUp);
    let answer = devpod::status(runner, workspace_id);
    // Python's `@timing.staged("devpod-up") get_workspace_state` returns `None`
    // for a devpod that ran and refused, gave non-JSON, or omitted `state` — the
    // stage stays `ok`. Only a devpod that could not be run at all raises
    // (`DevpodNotInstalled`, or another spawn `OSError`) and the decorator marks
    // the stage `failed`. `NotRun` is that case; mark it so the timing document
    // does not report `ok` for a launch step devpod never performed (P12/C8).
    if matches!(answer, Err(StatusUnreadable::NotRun(_))) {
        stage.fail();
    }
    answer
}

/// Which devpod workspace a triple is, and what devpod said about it.
///
/// A sum rather than Python's `(workspace_id, Optional[state])` pair, whose
/// docstring has to promise that *state is None exactly when devpod knows no
/// workspace for this triple, in which case workspace_id is the derived id*. Both
/// halves of that promise are the type here: there is no way to build a
/// [`KnownWorkspace::Unknown`] carrying a recorded id, and no way to read a state
/// off one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KnownWorkspace {
    /// devpod knows this workspace and reported this state. The two come from one
    /// round trip: asking "which id" and then "what state" separately is how a
    /// command ends up addressing one workspace and reporting another's state.
    Known {
        workspace_id: String,
        state: ContainerState,
    },
    /// devpod knows no workspace for this triple. `derived` is the id a create
    /// would use.
    Unknown { derived: String },
}

impl KnownWorkspace {
    /// The id every later step addresses, whichever arm this is.
    ///
    /// Only this module's tests read it; the flows match the arms directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn workspace_id(&self) -> &str {
        match self {
            Self::Known { workspace_id, .. } => workspace_id,
            Self::Unknown { derived } => derived,
        }
    }

    /// The state devpod gave, or nothing when it knows no such workspace.
    ///
    /// Only this module's tests (via [`Self::is_running`]) read it; the flows
    /// match the arms directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn state(&self) -> Option<&ContainerState> {
        match self {
            Self::Known { state, .. } => Some(state),
            Self::Unknown { .. } => None,
        }
    }

    /// Whether a launch may attach straight away.
    ///
    /// Only this module's tests read it; the flows match the arms directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_running(&self) -> bool {
        matches!(self.state(), Some(state) if state.is_running())
    }
}

/// Which devpod workspace `(owner, repo, branch)` is, asked of devpod first.
///
/// devlaunch#88. The id `dl` hands devpod used to be derived on every command and
/// written down nowhere, so the derivation was the only copy of it that existed.
/// PR #81 moved that derivation and every workspace created under the old one
/// became unaddressable in the same instant — 36 of 39 on the reporting host.
/// Nothing was lost and nothing was corrupted; `dl` simply began asking devpod
/// about ids devpod had never been given. The record is the second copy, and this
/// is where it is read.
///
/// **Derived first, record second, and that order is a trade rather than a
/// shortcut.** Reading the record means loading `metadata.json` under its lock,
/// parsing it and running the id-scheme migration's version check — three things
/// devlaunch#145 deliberately took off the warm attach path, which is the path a
/// user waits on. Asking devpod about the derived id is already paid for by the
/// status round trip this function is built around, so the record is consulted
/// only once devpod has *denied* the derived id, which is the only case in which
/// it can say anything new.
///
/// That ordering is why `recorded_id` is a closure rather than an
/// `Option<String>`: a parameter would have to be computed by the caller before
/// the call, which is exactly the metadata read this defers. Passing the *lookup*
/// makes "the warm path reads no metadata" a property of the signature.
///
/// A stored id devpod also denies is not used. `metadata.json` is append-mostly
/// and nothing prunes it, so a record naming a workspace deleted months ago is
/// ordinary; addressing it would substitute one absent workspace for another and
/// lose the derived id a create needs.
///
/// **A devpod that could not be run at all is not a denial.** Python's
/// `get_workspace_state` folds a non-zero exit into `None` but *raises*
/// `DevpodNotInstalled` (and its siblings) out of `run_devpod`, so a host with no
/// devpod on it ends the command here rather than being told its workspace is
/// unknown. Reading the two the same way is worse than a wrong message: the cold
/// path it sends a launch down fetches a branch and builds a workspace clone on a
/// host that cannot open it, and leaves both behind for the exit-127 to be
/// discovered after. So the error is the runner's, and it travels.
pub(crate) fn resolve_known_workspace(
    runner: &dyn Runner,
    triple: (&str, &str, &str),
    derived: &str,
    recorded_id: impl FnOnce() -> Option<String>,
    notices: &mut dyn Notices<LifecycleNotice>,
) -> Result<KnownWorkspace, NotRun> {
    match workspace_state(runner, derived) {
        Ok(state) => {
            return Ok(KnownWorkspace::Known {
                workspace_id: derived.to_owned(),
                state,
            });
        }
        Err(StatusUnreadable::NotRun(not_run)) => return Err(not_run),
        // devpod ran and refused, answered something unparsable, or answered
        // without a state: Python's three `None`s, and all three mean "devpod
        // knows no workspace by this name".
        Err(_) => {}
    }
    let unknown = || {
        Ok(KnownWorkspace::Unknown {
            derived: derived.to_owned(),
        })
    };
    let Some(recorded) = recorded_id() else {
        return unknown();
    };
    if recorded == derived {
        return unknown();
    }
    let state = match workspace_state(runner, &recorded) {
        Ok(state) => state,
        Err(StatusUnreadable::NotRun(not_run)) => return Err(not_run),
        Err(_) => return unknown(),
    };
    let (owner, repo, branch) = triple;
    notices.say(LifecycleNotice::AddressingRecordedWorkspace {
        recorded: recorded.clone(),
        derived: derived.to_owned(),
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        branch: branch.to_owned(),
    });
    Ok(KnownWorkspace::Known {
        workspace_id: recorded,
        state,
    })
}

/// The devpod workspace id `metadata.json` holds for a triple, if any.
///
/// `None` covers both "no record" and "a record from before this field was ever
/// written", which are the same answer to the only question asked of it: there is
/// nothing here to follow, so the derivation stands.
///
/// A cache dl cannot read answers `None` too, and that is one level up: a store
/// that could not be opened never becomes a [`MetadataStorage`], so the caller
/// holding one has already handled the failure — and the way it handles it is to
/// pass a closure that answers `None`, because a lookup that failed must not be
/// able to stop a command that would otherwise have worked.
pub(crate) fn recorded_devpod_workspace_id(
    storage: &MetadataStorage,
    owner: &str,
    repo: &str,
    branch: &str,
) -> Option<String> {
    storage
        .get_worktree(owner, repo, branch)?
        .devpod_workspace_id
        .clone()
}

// ===========================================================================
// stop
// ===========================================================================

/// How a stop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    Stopped,
    /// devpod refused. Its own diagnostics are already on the user's stderr — the
    /// call inherits this process's streams, as Python's does — so there is
    /// nothing to carry but the ending.
    DevpodRefused {
        exit: Exit,
    },
}

/// Stop a workspace: `devpod stop <id>`.
///
/// A stopped workspace still appears in `devpod list`, with different details, so
/// the snapshot is forgotten either way — and the completion cache is wrong
/// regardless of age, which is why the refresh is [`RefreshReason::Forced`].
pub fn workspace_stop(
    context: &mut CommandContext<'_>,
    refresh: &mut Refresh<'_>,
    workspace_id: &str,
) -> Result<StopOutcome, NotRun> {
    let exit = devpod::run(context.runner(), &stop_call(workspace_id))?;
    context.forget_workspaces();
    refresh.ask(context.runner(), RefreshReason::Forced);
    Ok(if exit.is_success() {
        StopOutcome::Stopped
    } else {
        StopOutcome::DevpodRefused { exit }
    })
}

/// `devpod stop <id>` — argv-exact.
fn stop_call(workspace_id: &str) -> Call {
    Call::new(["stop", workspace_id])
}

// ===========================================================================
// the delete guard
// ===========================================================================

/// Whether the caller typed `--force`.
///
/// Named arms rather than a bool, because `--force` means two different things on
/// this path and both are decisions: it carries a delete past the unsaved-work
/// guard, *and* it makes an already-absent workspace count as deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Insistence {
    Insisted,
    NotInsisted,
}

/// Why `dl <ws> rm` will not delete this workspace.
///
/// Two arms and no third, because [`Unsaved`] has three and one of them is
/// permission. Each carries what its sentence interpolates and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovalRefused {
    /// The clone holds work that exists nowhere else.
    WouldLose {
        workspace_id: String,
        losses: Losses,
    },
    /// dl could not establish what the clone holds. Refused for not knowing, and
    /// it says which: the work is still on disk and nothing has shown that it
    /// exists anywhere else, which is the same standing as unpushed work and gets
    /// the same refusal and the same way past it (devlaunch#171).
    CouldNotTell {
        workspace_id: String,
        cause: CouldNotTell,
    },
}

/// What the guard decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Guarded {
    /// Nothing dl can establish would be lost, or the caller insisted.
    MayRemove,
    Refused(RemovalRefused),
}

/// Whether `dl <ws> rm` may go ahead.
///
/// The one thing dl refuses on its own account. It is not a judgement about
/// whether the work is finished — dl has no way to know that — but about whether
/// this clone is the only place the work exists.
///
/// Total over [`Unsaved`]'s three arms, and only one of them is permission
/// (devlaunch#171). Python needed `unhandled_unsaved()` behind an `isinstance`
/// chain so that an answer the guard did not name would raise rather than slide
/// through an `else` into a deletion; here the `match` is exhaustive at compile
/// time, so a fourth arm breaks this function instead.
///
/// `--force` is checked *after* the answer is read, not instead of reading it, so
/// the refusal a forced delete carried past is still available to the caller — and
/// so a future `--force` that wanted to report what it overrode has it.
pub fn guard_removal(workspace_id: &str, unsaved: Unsaved, insistence: Insistence) -> Guarded {
    let refusal = match unsaved {
        Unsaved::NothingToLose => return Guarded::MayRemove,
        Unsaved::WouldLose(losses) => RemovalRefused::WouldLose {
            workspace_id: workspace_id.to_owned(),
            losses,
        },
        Unsaved::CouldNotTell(cause) => RemovalRefused::CouldNotTell {
            workspace_id: workspace_id.to_owned(),
            cause,
        },
    };
    match insistence {
        Insistence::Insisted => Guarded::MayRemove,
        Insistence::NotInsisted => Guarded::Refused(refusal),
    }
}

/// [`ClonePathResolver`] over the production clone manager.
///
/// The listing's resolver trait cannot carry the [`CacheNotice`]s
/// [`WorkspaceCloneManager::resolve_clone_path`] produces — it is read from a
/// row-building loop that has nowhere to put them — so they are collected here and
/// drained by whoever built this. That keeps one answer to "which directory is
/// this record's clone in" across the listing, this guard and the delete itself,
/// which is the whole of devlaunch#174: they used to name it separately and could
/// disagree, with the guard clearing an absent directory while the delete removed
/// the one holding the work.
pub struct CloneDirectories<'a, 'r> {
    clones: &'a WorkspaceCloneManager<'r>,
    notices: RefCell<Vec<CacheNotice>>,
}

impl<'a, 'r> CloneDirectories<'a, 'r> {
    pub fn of(clones: &'a WorkspaceCloneManager<'r>) -> Self {
        Self {
            clones,
            notices: RefCell::new(Vec::new()),
        }
    }

    /// The notices resolving produced, leaving none behind.
    pub fn take_notices(&self) -> Vec<CacheNotice> {
        std::mem::take(&mut self.notices.borrow_mut())
    }
}

impl ClonePathResolver for CloneDirectories<'_, '_> {
    fn clone_path(&self, record: &WorktreeInfo) -> Option<PathBuf> {
        self.clones
            .resolve_clone_path(record, &mut *self.notices.borrow_mut())
    }
}

/// What deleting `workspace_id` would destroy, as far as dl can establish.
///
/// [`listing::unsaved_work_in`]'s reader, wired to the production resolver. The
/// answer for a workspace dl has no record of is [`Unsaved::NothingToLose`], which
/// is the honest answer rather than a permissive one: those are workspaces opened
/// from a path or a URL that dl never cloned and does not manage, so it has no
/// clone of its own to protect and no business inspecting somebody's checkout to
/// find one.
pub fn unsaved_work_in(
    clones: &WorkspaceCloneManager<'_>,
    storage: &MetadataStorage,
    git: &Git<'_>,
    cache_dir: &Path,
    workspace_id: &str,
    notices: &mut dyn Notices<LifecycleNotice>,
) -> Unsaved {
    let directories = CloneDirectories::of(clones);
    let view = listing::DlView {
        cache_dir,
        storage,
        clones: &directories,
    };
    let unsaved = listing::unsaved_work_in(git, &view, workspace_id);
    extend_with_cache(notices, directories.take_notices());
    unsaved
}

// ===========================================================================
// delete
// ===========================================================================

/// What became of the docker volumes a deleted workspace's devcontainer created.
///
/// Every arm is an outcome of a delete that **succeeded** — the workspace is gone
/// in all four — which is why this rides inside [`DeleteOutcome::Deleted`] rather
/// than being able to fail it. Reporting a failure here would send the caller
/// looking for a workspace that is not there, which is the same reasoning the
/// clone arm beside it uses.
///
/// Three of the four are silent, and the line is whether there is anything a user
/// could act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeSweep {
    /// docker removed the volumes named, or found them already gone — which
    /// `docker volume rm --force` counts as removed, so a repository whose
    /// devcontainer never declared one of these names lands here rather than in a
    /// refusal on every delete.
    Removed,
    /// Nothing was named, so docker was not run at all: devpod's record of what it
    /// substituted into this workspace's devcontainer is not there to read, which
    /// is what an `up` that never finished leaves behind.
    NothingNamed,
    /// No docker on this machine. Silent on purpose, and the reason it is its own
    /// arm: a host with no docker never made these volumes, so there is nothing
    /// here to have failed.
    NoDocker,
    /// The volumes are still on this machine, and this is why.
    Refused(VolumeRefusal),
}

/// Why a deleted workspace's docker volumes are still on this machine.
///
/// Apart from [`VolumeSweep`]'s three silent arms rather than among them, so that
/// [`LifecycleNotice::VolumesNotRemoved`] cannot be built from an outcome that
/// went fine. Neither arm is a sentence: the words are the `dl` binary's, as they
/// are for every other refusal core hands over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeRefusal {
    /// docker ran and would not remove them — a volume some other container still
    /// holds is the case this exists for. `stderr` is docker's own words.
    Docker { exit: Exit, stderr: String },
    /// docker never answered: the OS would not start it, or it was killed. The
    /// errno and nothing else, for the same reason [`NotRun::Blocked`] carries
    /// one.
    NotRun { failure: OsFailure },
}

/// How a delete ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// devpod let go of the workspace. `clone` says what became of the local
    /// clone: `Ok` with which no-op or removal happened, or `Err` when the
    /// removal was attempted and refused (the workspace is gone regardless, which
    /// is why this is still `Deleted` and the refusal rides in the field rather
    /// than failing the delete). The `Err` was a fourth meaning crammed into the
    /// old `Removed::Nothing`; it is its own channel now.
    ///
    /// `volumes` is the same bargain for the named docker volumes the workspace's
    /// devcontainer created — see [`VolumeSweep`].
    Deleted {
        clone: Result<Removed, RemoveWorkspaceError>,
        volumes: VolumeSweep,
    },
    /// devpod refused, and the local clone was **kept** so the delete stays
    /// retryable. devpod re-parses the workspace's `devcontainer.json` to tear the
    /// container down, so a config that has since moved makes deletion fail — and
    /// removing the clone regardless strands the workspace for good, because devpod
    /// can then never find the config to retry with.
    DevpodRefused { exit: Exit },
}

/// Delete a workspace and its local clone (if any).
///
/// The clone is removed only once devpod has actually let go of the workspace —
/// see [`DeleteOutcome::DevpodRefused`] for why.
///
/// [`Insistence::Insisted`] passes devpod's own `--ignore-not-found`, which makes a
/// workspace devpod does not have count as deleted, so a forced remove is "ensure
/// absent" the way `rm -f` is. The clone cleanup still runs on that path: a stale
/// clone with no workspace is exactly what a half-finished delete leaves, and what
/// a cold-bench reset (devlaunch#140) must clear.
#[allow(clippy::too_many_arguments)]
pub fn workspace_delete(
    context: &mut CommandContext<'_>,
    refresh: &mut Refresh<'_>,
    clones: &WorkspaceCloneManager<'_>,
    storage: &mut MetadataStorage,
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
    insistence: Insistence,
    notices: &mut dyn Notices<LifecycleNotice>,
) -> Result<DeleteOutcome, NotRun> {
    // Named *before* the delete, and that ordering is the whole of why this is two
    // steps: `devpod delete` takes devpod's own record of the workspace away with
    // the workspace, and that record is the only place the substituted volume
    // names live. Named afterwards, this would find nothing every time and look
    // like a working cleanup.
    let named = devcontainer_volumes(devpod_home, workspace_id);
    let exit = devpod::run(context.runner(), &delete_call(workspace_id, insistence))?;
    // Unconditionally: a delete that reports failure may still have got far enough
    // to change what devpod lists.
    context.forget_workspaces();
    if !exit.is_success() {
        refresh.ask(context.runner(), RefreshReason::Forced);
        return Ok(DeleteOutcome::DevpodRefused { exit });
    }

    // Streamed rather than collected and appended, because the storage flow's own
    // line comes *first* in Python: `Removed workspace clone: <path>` is logged
    // inside the removal, and `Removed local clone for <id>` after it returns.
    let removal = clones.remove_workspace_by_id(storage, workspace_id, &mut as_cache(notices));
    let clone = match removal {
        Ok(removed) => {
            // Exhaustive rather than `if let`: only the clone actually removed gets
            // the `Removed local clone` line, and a new no-op arm must be a compile
            // error here rather than silently join the silent ones.
            match removed {
                Removed::Clone => notices.say(LifecycleNotice::CloneRemoved {
                    workspace_id: workspace_id.to_owned(),
                }),
                Removed::NothingRecorded
                | Removed::DirectoryNotNamed
                | Removed::DirectoryAbsent => {}
            }
            Ok(removed)
        }
        Err(error) => {
            // The workspace is gone whatever happened to the clone, so this is a
            // notice rather than the delete failing: reporting failure would send
            // the caller looking for a workspace that is not there. The refusal is
            // carried in the outcome too, so a caller reads it without re-deriving
            // it from the notice stream.
            notices.say(LifecycleNotice::CloneNotRemoved {
                workspace_id: workspace_id.to_owned(),
                refusal: error.clone(),
            });
            Err(error)
        }
    };
    let volumes = sweep_volumes(context.runner(), named);
    if let VolumeSweep::Refused(refusal) = &volumes {
        notices.say(LifecycleNotice::VolumesNotRemoved {
            workspace_id: workspace_id.to_owned(),
            refusal: refusal.clone(),
        });
    }
    refresh.ask(context.runner(), RefreshReason::Forced);
    Ok(DeleteOutcome::Deleted { clone, volumes })
}

/// Remove the volumes `named`, and say what became of them.
///
/// The one place a [`docker::Answer`] becomes a [`VolumeSweep`], so every removal
/// path draws the same line in the same place: nothing to name and no docker to
/// name it with are silent, and a docker that was asked and did not deliver is
/// not. It reports nothing itself — `rm` says its piece as a
/// [`LifecycleNotice`] and `--purge` as a [`PurgeStep`], and neither vocabulary
/// belongs to the removal.
fn sweep_volumes(runner: &dyn Runner, named: Option<NonEmpty<String>>) -> VolumeSweep {
    let Some(names) = named else {
        return VolumeSweep::NothingNamed;
    };
    match docker::remove_volumes(runner, &names) {
        docker::Answer::Ran { exit, .. } if exit.is_success() => VolumeSweep::Removed,
        docker::Answer::Ran { exit, stderr } => {
            VolumeSweep::Refused(VolumeRefusal::Docker { exit, stderr })
        }
        docker::Answer::NotInstalled => VolumeSweep::NoDocker,
        docker::Answer::NotStarted(failure) => {
            VolumeSweep::Refused(VolumeRefusal::NotRun { failure })
        }
    }
}

/// The named docker volumes one workspace's devcontainer created, or nothing where
/// devpod's own record cannot say.
///
/// **One source for both names, and it is devpod's create result** — the file
/// devpod writes on its way out of a successful `up`, recording what it
/// substituted into the devcontainer. Both volumes are named from variables devpod
/// expanded, so the record is by definition the answer:
///
/// - `${localWorkspaceFolderBasename}-pixi`, from this repository's own
///   `.devcontainer/devcontainer.json` mount for the `.pixi` cache;
/// - `dind-var-lib-docker-${devcontainerId}`, from the `docker-in-docker` feature.
///
/// Deriving the basename from the clone directory devlaunch chose instead would be
/// a second answer to a question devpod has already answered, and the two can
/// disagree — a workspace opened before a rename, say. Neither *value* is guessed:
/// a substitution that is not in the record names no volume. The two name
/// **templates** are still this repository's own devcontainer and the
/// `docker-in-docker` feature's, so a `mounts` entry naming some third volume is
/// not swept — devlaunch#325's scope, and the follow-up that would end it is
/// reading the mount sources out of the recorded merged config instead.
///
/// [`sole_workspace_result`] is what finds the file, rather than a contexts walk of
/// its own: an id under two contexts must answer nothing, and `devpod delete`
/// resolves that id against the *current* context, so a walk that picked one would
/// remove a living workspace's volumes. Sharing the walk makes the rule identical
/// by construction instead of by comment.
///
/// `None` rather than an empty list, so a caller cannot read "nothing to remove"
/// as "removed nothing" — an `up` that died in its lifecycle hooks leaves the
/// workspace record with no result beside it, which is exactly this case.
fn devcontainer_volumes(
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
) -> Option<NonEmpty<String>> {
    let result = sole_workspace_result(devpod_home, workspace_id)?;
    let recorded = parse_substitutions(&std::fs::read(result).ok()?);
    NonEmpty::of(recorded.volume_names())
}

/// What devpod recorded substituting into one workspace's devcontainer.
///
/// Only the two fields the volume names are built from. Both optional because
/// devpod omits an empty one, and a record written by a devpod that never learned
/// about `${devcontainerId}` has neither.
#[derive(Debug, Default, PartialEq, Eq)]
struct Substitutions {
    /// `SubstitutionContext.LocalWorkspaceFolder` — the host directory devpod
    /// opened, whose basename is what `${localWorkspaceFolderBasename}` expanded
    /// to.
    local_workspace_folder: Option<String>,
    /// `SubstitutionContext.DevContainerID` — what `${devcontainerId}` expanded
    /// to.
    devcontainer_id: Option<String>,
}

impl Substitutions {
    /// The volume names these substitutions imply, in the order they were
    /// declared. Empty where neither field was recorded.
    fn volume_names(&self) -> Vec<String> {
        let basename = self
            .local_workspace_folder
            .as_deref()
            .map(Path::new)
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned());
        [
            basename.map(|basename| format!("{basename}-pixi")),
            self.devcontainer_id
                .as_deref()
                .map(|id| format!("dind-var-lib-docker-{id}")),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// Read the two substituted values off devpod's create result.
///
/// Total over anything the file could hold: bytes that are not JSON, or JSON of
/// another shape, answer the empty set rather than an error. Nothing here is worth
/// a diagnostic — a result devlaunch cannot read is a result it removes no volumes
/// from, which is exactly what it did before this existed.
fn parse_substitutions(bytes: &[u8]) -> Substitutions {
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Substitutions::default();
    };
    let context = &document["SubstitutionContext"];
    Substitutions {
        local_workspace_folder: non_empty_string(&context["LocalWorkspaceFolder"]),
        devcontainer_id: non_empty_string(&context["DevContainerID"]),
    }
}

/// The string this value holds, where it holds a non-empty one. Empty is dropped
/// so a blank field cannot build the volume name `-pixi`.
fn non_empty_string(value: &serde_json::Value) -> Option<String> {
    match value.as_str() {
        Some(text) if !text.is_empty() => Some(text.to_owned()),
        _ => None,
    }
}

/// `devpod delete <id> [--ignore-not-found]` — argv-exact.
fn delete_call(workspace_id: &str, insistence: Insistence) -> Call {
    match insistence {
        Insistence::Insisted => Call::new(["delete", workspace_id, "--ignore-not-found"]),
        Insistence::NotInsisted => Call::new(["delete", workspace_id]),
    }
}

// ===========================================================================
// purge
// ===========================================================================

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
    ownership: WorkspaceOwnership,
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
    /// it used to be doubled as a [`LifecycleNotice`] too, and the binary carried
    /// a filter whose whole job was to drop the second copy.
    NotDeleted {
        workspace_id: String,
        exit: Exit,
        stderr: String,
    },
    /// The workspace went and the named docker volumes its devcontainer created
    /// are still there. Said for the reason
    /// [`LifecycleNotice::VolumesNotRemoved`] is said on the `rm` path — a purge
    /// promises a clean slate, and disk it could not reclaim is the one thing
    /// worth naming about it. The three sweeps that went fine say nothing.
    VolumesNotRemoved {
        workspace_id: String,
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
        Removal::Everything => PurgeOutcome::Removed { cache_dir },
        Removal::WhatItCould(refused) => PurgeOutcome::RemovedWhatItCould { cache_dir, refused },
        Removal::Nothing(refused) => PurgeOutcome::RemovedNothing { cache_dir, refused },
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

// ===========================================================================
// where a workspace is on this disk
// ===========================================================================

/// Every place on this machine a source could name — possibly none.
///
/// Empty is a real answer and not a shrug: an image or container workspace opens
/// no directory on this disk, so there is nothing to compare and no clone it could
/// be holding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourcePlaces {
    Placeable(Vec<String>),
    /// The source opens a folder here and devlaunch cannot say which one. Kept
    /// apart from `Placeable(vec![])` because reading them alike is how a live
    /// workspace contributed no path *and* no alarm while the command printed that
    /// it stops for exactly that.
    Unplaceable {
        payload: String,
    },
}

/// Whether `text` names a repository somewhere else rather than something on this
/// disk.
///
/// Said once, here, because both readers — `--prune`'s placement pass and
/// `--reconcile`'s orphan sweep — have to agree about it or a workspace is a
/// remote to one command and a directory to the other.
///
/// **The two mistakes are not equal, so the test is deliberately narrow.** A
/// remote read as a path is devlaunch#224: relative-looking text, resolved against
/// the current directory, so a workspace lands inside whichever repository the
/// person running `dl` happened to be standing in and is misreported there —
/// wrong, and toward refusing. A path read as a remote drops a directory out of the
/// referenced set, which is how `--prune` would come to call a live clone
/// unreferenced — wrong, and toward loss. So only the two shapes that are never
/// also written as a relative directory count: a URL scheme
/// (`[A-Za-z][A-Za-z0-9+.-]*://`), and `user@host:` where nothing before the colon
/// is a `/`. Text that is merely host-shaped (`github.com/owner/repo`) does not
/// count, because it is a perfectly good relative path, and `devpod up ./some-repo`
/// is the case that arm exists for.
///
/// `file://` matches the scheme form, and it is the one scheme that does name a
/// directory on this machine — but never usably: the callers resolve plain paths,
/// so it only ever produced `<cwd>/file:/…` garbage. Contributing nothing is
/// strictly less wrong.
pub(crate) fn names_a_remote(text: &str) -> bool {
    has_url_scheme(text) || is_scp_like(text)
}

/// `^[A-Za-z][A-Za-z0-9+.\-]*://`
fn has_url_scheme(text: &str) -> bool {
    let Some(rest) = text.strip_prefix(|c: char| c.is_ascii_alphabetic()) else {
        return false;
    };
    let Some(at) = rest.find("://") else {
        return false;
    };
    rest[..at]
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-')
}

/// `^[^/@:\s]+@[^/:\s]+:` — an scp-like remote. Any `/` before the colon
/// disqualifies it, because a directory literally named that way is possible and
/// nobody's spelling.
fn is_scp_like(text: &str) -> bool {
    let Some((user, rest)) = text.split_once('@') else {
        return false;
    };
    if user.is_empty() || user.chars().any(|c| "/@:".contains(c) || c.is_whitespace()) {
        return false;
    }
    let Some((host, _)) = rest.split_once(':') else {
        return false;
    };
    !host.is_empty() && !host.chars().any(|c| "/:".contains(c) || c.is_whitespace())
}

/// Where on this machine `source` could be. Total over the arms.
///
/// A `gitRepository` counts *when it carries a path*, even though devlaunch only
/// ever hands devpod a local path, and the reason is which way the mistake runs.
/// `devpod up <path-to-a-repo>` records that arm with a path in it, and a path this
/// function does not return is a directory `--prune` will call unreferenced.
/// [`listing::is_devlaunch_clone`](crate::flows::listing::is_devlaunch_clone)
/// refuses the same arm on purpose — but refusing there means declining to delete
/// somebody else's *workspace*, which is the opposite direction, so its answer must
/// not be reused here.
pub(crate) fn source_places(source: &WorkspaceSource) -> SourcePlaces {
    match source {
        WorkspaceSource::LocalFolder(path) => SourcePlaces::Placeable(vec![path.clone()]),
        WorkspaceSource::GitRepository(url) => SourcePlaces::Placeable(if names_a_remote(url) {
            Vec::new()
        } else {
            vec![url.clone()]
        }),
        // An image or container workspace: nothing here, nothing at risk.
        WorkspaceSource::Unrecognised(_) => SourcePlaces::Placeable(Vec::new()),
        WorkspaceSource::UnreadableLocalFolder(payload) => SourcePlaces::Unplaceable {
            payload: json_as_python_writes_it(&serde_json::Value::Object(payload.clone())),
        },
    }
}

/// One live workspace whose source could not be followed, and the text that could
/// not be followed.
///
/// Not a warning above a report: a workspace whose source cannot be followed could
/// be opening *any* of the candidates, so while one exists there is no directory
/// either command can honestly call unreferenced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unlocatable {
    pub workspace_id: String,
    /// The source text, or devpod's own source object where there was no text to
    /// follow.
    pub detail: String,
}

/// A live workspace devpod records inside a repository's clone tree, at something
/// that is not a clone.
///
/// devlaunch#88's measured shape. On that ticket's host 36 of 39 devpod records
/// named a folder that was gone (35) or a config-only stub devpod itself wrote from
/// cache (1), while the real checkout sat beside it under the new id scheme. The
/// two records cannot be joined by workspace id — the id is exactly what changed —
/// so the join is made from the path instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Misplaced {
    pub workspace_id: String,
    pub sourced_at: String,
}

/// Where devpod's workspaces are on this disk, and which ones are unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceLocations {
    /// Resolved source path to the workspace that opens it.
    by_path: IndexMap<PathBuf, String>,
    /// `unlocatable` is not an empty result with a note on it — see
    /// [`Unlocatable`].
    unlocatable: Vec<Unlocatable>,
    /// The same refusal made narrow, keyed by `(owner, repo)`. A workspace devpod
    /// records at a non-clone *inside one repository's clone tree* can only be
    /// confused with that repository's clones, so it disputes those and leaves
    /// every other repository prunable — which is what keeps `--prune` usable on
    /// the host devlaunch#88 describes rather than merely safe on it.
    misplaced: BTreeMap<(String, String), Misplaced>,
}

impl WorkspaceLocations {
    /// The live workspaces this command cannot place, or nothing when every one of
    /// them placed itself.
    ///
    /// A [`NonEmpty`] rather than a list, because the caller's response to it is to
    /// stop, and "stop, and here are no reasons" is not a thing to say.
    pub fn unlocatable(&self) -> Option<NonEmpty<Unlocatable>> {
        NonEmpty::of(self.unlocatable.iter().cloned())
    }

    /// The live workspace `candidate` holds the checkout for, if any.
    ///
    /// At **or under**, not equal to, and the direction matters in the only way
    /// this command's mistakes matter. `devpod up <clone>/subproject` records the
    /// subdirectory, and a clone whose subdirectory a live workspace opens is a
    /// clone that live workspace needs — deleting it takes the workspace with it.
    /// Equality answered no and deleted the parent.
    ///
    /// The containment is between two canonical paths, which is what keeps it from
    /// being the lexical prefix test the reporting surface uses:
    /// `<clone>-scratch` is not under `<clone>`, and a symlinked source has already
    /// been resolved before it gets here.
    pub fn holder(&self, candidate: &Path) -> Option<&str> {
        if let Some(held_by) = self.by_path.get(candidate) {
            return Some(held_by);
        }
        self.by_path
            .iter()
            .find(|(source, _)| source.ancestors().skip(1).any(|above| above == candidate))
            .map(|(_, held_by)| held_by.as_str())
    }

    /// The workspace disputing every clone of `(owner, repo)`, if any.
    pub(crate) fn misplaced_in(&self, owner: &str, repo: &str) -> Option<&Misplaced> {
        self.misplaced.get(&(owner.to_owned(), repo.to_owned()))
    }
}

/// Where a resolved source sits with respect to devlaunch's clone tree.
///
/// Read off the path rather than derived from an id, because on devlaunch#88's host
/// the id is what went wrong and the path is what survived: devpod's stale record
/// still says `<root>/blooop/devlaunch/<old-leaf>`, which names the repository
/// exactly even though the leaf and the workspace id match nothing any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceSite {
    /// Not in devlaunch's clone tree, so no clone answers for it.
    Outside,
    /// At or under `clone`, a directory that holds a checkout.
    InAClone { clone: PathBuf },
    /// In `(owner, repo)`'s clone tree but at no clone of it.
    InARepositoryOnly { owner: String, repo: String },
    /// In the clone tree above any repository, so it names none.
    TooShallow,
}

/// [`SourceSite`] for one resolved source.
///
/// The clone is the *third* component under the root and the source may be
/// deeper — `devpod up <clone>/subproject` is a live workspace whose source is
/// inside a clone, and the clone is what answers for it.
pub(crate) fn site_of(source: &Path, root: &Path) -> SourceSite {
    let Ok(relative) = source.strip_prefix(root) else {
        return SourceSite::Outside;
    };
    let parts: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    match parts.as_slice() {
        [] | [_] => SourceSite::TooShallow,
        [owner, repo] => SourceSite::InARepositoryOnly {
            owner: owner.clone(),
            repo: repo.clone(),
        },
        [owner, repo, leaf, ..] => {
            let clone = root.join(owner).join(repo).join(leaf);
            if is_populated_clone(&clone) {
                SourceSite::InAClone { clone }
            } else {
                SourceSite::InARepositoryOnly {
                    owner: owner.clone(),
                    repo: repo.clone(),
                }
            }
        }
    }
}

/// Resolve every live workspace's source to a real directory on this disk.
///
/// Both sides of the comparison this feeds are canonical, and that is the whole
/// point rather than tidiness. A cache reached through a symlink — somebody moved
/// theirs, or `/tmp` is a link on their machine — makes a lexical comparison say
/// that *no* clone is referenced, which is a total-loss bug in the one direction
/// that cannot be undone. The candidates are canonical by construction (see
/// [`prune_plan`]); this canonicalises the other side.
///
/// Three ways a workspace fails to place itself, and they are not one thing: a
/// source devlaunch cannot read at all, and a source that named a folder no
/// filesystem call will accept, both mean the workspace could be opening *any*
/// candidate and stop the command; a source that lands inside a repository's clone
/// tree on something with no `.git` in it means the workspace could be opening any
/// of *that repository's* clones, and disputes only those.
pub(crate) fn workspace_locations(workspaces: &[Workspace], root: &Path) -> WorkspaceLocations {
    let mut located = WorkspaceLocations::default();
    for workspace in workspaces {
        let places = match source_places(&workspace.source) {
            SourcePlaces::Unplaceable { payload } => {
                located.unlocatable.push(Unlocatable {
                    workspace_id: workspace.id.clone(),
                    detail: payload,
                });
                continue;
            }
            SourcePlaces::Placeable(paths) => paths,
        };
        for source in places {
            let Some(resolved) = canonical(&source) else {
                located.unlocatable.push(Unlocatable {
                    workspace_id: workspace.id.clone(),
                    detail: source,
                });
                continue;
            };
            match site_of(&resolved, root) {
                SourceSite::Outside | SourceSite::InAClone { .. } => {
                    located.by_path.insert(resolved, workspace.id.clone());
                }
                SourceSite::InARepositoryOnly { owner, repo } => {
                    located.misplaced.insert(
                        (owner, repo),
                        Misplaced {
                            workspace_id: workspace.id.clone(),
                            sourced_at: resolved.display().to_string(),
                        },
                    );
                }
                SourceSite::TooShallow => located.unlocatable.push(Unlocatable {
                    workspace_id: workspace.id.clone(),
                    detail: source,
                }),
            }
        }
    }
    located
}

/// Whether `path` is a checkout rather than a place one used to be.
///
/// `.git`'s presence, which is devlaunch#88's own published diagnostic
/// (`[ -d "$p/.git" ] || echo BROKEN`). It is what separates a devpod record that
/// still describes something from one the id-scheme change left behind — a folder
/// that is gone, or the config-only stub devpod reconstitutes from its cache,
/// neither of which any clone can be matched to.
///
/// A door this process cannot open reads as **not** a populated clone, and that is
/// the safe direction rather than the tidy one. Answering "yes" would say devpod's
/// workspace is at *this* clone and nowhere else, which leaves the repository's
/// other clones prunable; answering "no" says which clone of the repository the
/// workspace wants cannot be established, which disputes all of them and keeps
/// them.
///
/// `.git` may be a directory or a *file* (`git clone --separate-git-dir`), so this
/// asks whether anything is there rather than whether a directory is.
pub(crate) fn is_populated_clone(path: &Path) -> bool {
    std::fs::metadata(path.join(".git")).is_ok()
}

/// `path` with every symlink resolved, or nothing when it could not be followed.
///
/// `None` means "cannot tell", never "somewhere else": every caller here is
/// deciding whether a directory is referenced, and answering that from a lookup
/// that failed is how a live clone becomes an orphan.
///
/// A path that is not *there* is not a failure — this canonicalises as much of it
/// as exists and leaves the rest, which is the right answer for a workspace whose
/// source has been deleted, and there are hosts where that is most of them
/// (devlaunch#88). [`std::fs::canonicalize`] refuses such a path outright, which is
/// why this walks up to the deepest ancestor that resolves and re-appends the rest
/// — Python's `Path.resolve(strict=False)`, said out loud.
///
/// What lands in `None` is text no filesystem call will accept as a path at all: a
/// NUL byte, which a hand-edited or truncated `metadata.json` can put in a record,
/// and the empty string, which names no directory (Python read it as `Path(".")`
/// and resolved it to the working directory — the cwd-shaped answer devlaunch#224
/// is about).
pub(crate) fn canonical(path: &str) -> Option<PathBuf> {
    // A NUL byte is refused *here* rather than at the first syscall, because Rust
    // — unlike Python, where `Path(text)` raises `ValueError` before the `lstat` —
    // lets one into a `PathBuf` quite happily. Without this the walk below climbs
    // past every failing `canonicalize` to a readable ancestor and hands back a
    // path with the NUL still in it, which no later call can use either.
    if path.is_empty() || path.contains('\0') {
        return None;
    }
    let absolute = std::path::absolute(path).ok()?;
    let mut trailing: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = absolute;
    loop {
        if let Ok(real) = std::fs::canonicalize(&cursor) {
            let mut resolved = real;
            for part in trailing.iter().rev() {
                resolved.push(part);
            }
            return Some(resolved);
        }
        let name = cursor.file_name()?.to_owned();
        let parent = cursor.parent()?.to_path_buf();
        if parent == cursor {
            return None;
        }
        trailing.push(name);
        cursor = parent;
    }
}

/// The real directories directly under `path`, sorted, symlinks not followed.
///
/// A symlinked entry is skipped rather than followed. Following one would put a
/// candidate outside the cache entirely, and unlinking the link instead would
/// report a clone as reclaimed while it sat on another volume — the same two wrong
/// answers [`remove_tree_as_far_as_it_goes`] refuses for a symlinked root. Skipping
/// is that refusal one step earlier, and it is also what keeps every candidate's
/// path canonical without a resolve that could fail.
///
/// A directory that cannot be listed yields nothing: there is no such thing as a
/// clone this process can delete but not see, so the safe reading of a closed door
/// is that there is nothing behind it to remove.
fn subdirectories(path: &Path) -> Vec<PathBuf> {
    let Ok(listed) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = listed
        .flatten()
        .filter(|entry| matches!(entry.file_type(), Ok(kind) if kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

// ===========================================================================
// prune: what one clone directory is
// ===========================================================================

/// What removing a clone would destroy or risk — the two answers `--prune` acts
/// on.
///
/// The one place devlaunch#171's three answers become the two `decide` acts on:
/// something to say, or nothing. "Could not tell" arrives here as an *objection*
/// rather than as an absence, so the clone is kept for the same reason unpushed
/// work keeps one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Objection {
    /// Deleting it would destroy this. Carries the losses, so the description a
    /// report prints is derived rather than passed along as text.
    WouldLose(Losses),
    /// git could not be asked about it, and this is what it said.
    CouldNotTell(CouldNotTell),
}

/// What removing `unsaved`'s clone would cost, or nothing when it would cost
/// nothing.
///
/// Total over [`Unsaved`]'s arms, so a fourth answer stops the build rather than
/// falling through into a deletion.
pub fn objection(unsaved: &Unsaved) -> Option<Objection> {
    match unsaved {
        Unsaved::NothingToLose => None,
        Unsaved::WouldLose(losses) => Some(Objection::WouldLose(losses.clone())),
        Unsaved::CouldNotTell(cause) => Some(Objection::CouldNotTell(cause.clone())),
    }
}

/// Which arm one clone directory is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloneStatus {
    /// A live devpod workspace opens this exact clone directory.
    Referenced { workspace_id: String },
    /// Nothing opens this directory and no record ties it to a live workspace.
    ///
    /// `unsaved` sits inside this arm rather than beside the classification because
    /// it is only ever actionable here: "unsaved work on a clone that is staying
    /// anyway" is a sentence this type cannot say. `usage` is here for the same
    /// reason and earns its place twice over — the walk behind it is O(files) with
    /// no ceiling, and this is the only arm whose bytes anybody is going to get
    /// back, so putting it here is what keeps the other two arms from being walked
    /// at all.
    Orphaned { unsaved: Unsaved, usage: DiskUsage },
    /// devpod lists a workspace whose records and devlaunch's disagree about where
    /// it is.
    ///
    /// devlaunch#88's shape, and the reason `--prune` does not wait on #88: under
    /// that state a healthy clone at the *new* path is sourced by nobody. Read as
    /// an orphan it would be deleted; read as referenced it would silently hide
    /// disk. It is neither — it is two records disagreeing, and the answer to a
    /// disagreement is to keep the directory and say so.
    Disputed {
        workspace_id: String,
        sourced_at: String,
    },
}

/// Which arm `clone` is, asked in the order that fails towards keeping it.
///
/// devpod's own listing is consulted first, and by containment rather than by the
/// lexical predicate the reporting surface uses: the question is whether any live
/// workspace's source is at or under *this* directory.
///
/// Then the two ways devpod's records and devlaunch's can disagree, both
/// devlaunch#88's shape: a live workspace somewhere in *this repository's* clone
/// tree that is not a clone (36 of 39 workspaces on #88's host), and this
/// directory's own record naming a workspace devpod still lists and sources
/// elsewhere. A record naming a workspace devpod has *forgotten* is not a
/// disagreement — it is the ordinary stale clone this command exists for.
///
/// The unsaved probe and the disk walk run last and only on the arm that could be
/// removed. Together they are the expensive half of a scan (593 ms of git over 37
/// clones on the reference host, plus a walk with no ceiling), and asking them
/// about a directory no answer could affect is time spent to learn nothing.
pub(crate) fn clone_status(
    git: &Git<'_>,
    clone: &Path,
    owner: &str,
    repo: &str,
    locations: &WorkspaceLocations,
    record_for: &HashMap<PathBuf, WorktreeInfo>,
    listed_at: &HashMap<String, String>,
) -> CloneStatus {
    if let Some(workspace_id) = locations.holder(clone) {
        return CloneStatus::Referenced {
            workspace_id: workspace_id.to_owned(),
        };
    }
    if let Some(misplaced) = locations.misplaced_in(owner, repo) {
        return CloneStatus::Disputed {
            workspace_id: misplaced.workspace_id.clone(),
            sourced_at: misplaced.sourced_at.clone(),
        };
    }
    if let Some(record) = record_for.get(clone)
        && let Some(elsewhere) = listed_at.get(&record.workspace_id)
    {
        return CloneStatus::Disputed {
            workspace_id: record.workspace_id.clone(),
            sourced_at: elsewhere.clone(),
        };
    }
    CloneStatus::Orphaned {
        unsaved: workspace_state::holds_unsaved_work(git, clone),
        usage: disk_usage::exclusive_usage(clone),
    }
}

/// Why one clone directory is staying.
///
/// A sum rather than Python's `because: str`, so the sentence is the binary's and
/// each arm carries what that sentence interpolates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeptBecause {
    /// A live workspace still opens it.
    StillOpened { workspace_id: String },
    /// It holds something, and `--force` was not typed.
    Objected(Objection),
    /// devpod lists the workspace this directory's record names, and sources it
    /// somewhere else. See devlaunch#88.
    RecordsDisagree {
        workspace_id: String,
        sourced_at: String,
    },
}

/// Nothing objected to removing this directory, or `--force` carried it past an
/// objection.
///
/// Carried on the decision rather than read from a plan-wide `--force` flag, and
/// that difference is a deletion. A plan-wide boolean says "the user insisted"
/// about every directory in the plan, including the ones nothing objected to — so
/// the later re-probe, which exists to catch work written while the user was
/// reading the report, was skipped for clones `--force` had not promoted at all. A
/// promotion belongs to the directory it promoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Promotion {
    Unopposed,
    Insisted { despite: Objection },
}

impl Promotion {
    /// The insistence the second pass re-applies to *this* directory alone.
    fn insistence(&self) -> Insistence {
        match self {
            Self::Unopposed => Insistence::NotInsisted,
            Self::Insisted { .. } => Insistence::Insisted,
        }
    }
}

/// What `--prune` does about one clone directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// This directory goes, this is what it gives back, and this is what was
    /// insisted past to get here.
    ///
    /// The bytes travel with the decision rather than beside it, so "what this run
    /// reclaims" is a total over the things it is actually removing and cannot be
    /// assembled from a different set than the one that dies.
    Remove {
        usage: DiskUsage,
        promotion: Promotion,
    },
    Keep(KeptBecause),
}

/// What `--prune` does about one clone. The only such place.
///
/// Total over [`CloneStatus`]'s arms, and deliberately the single point at which
/// anything becomes deletable: there is no boolean beside a status that a later
/// caller could read without having answered which arm it is, and no path from a
/// directory devlaunch could not classify to one it would remove.
///
/// `--force` promotes exactly one arm. It is not a general override:
/// [`CloneStatus::Referenced`] and [`CloneStatus::Disputed`] are not "refusals to
/// be insisted past", they are devlaunch saying the directory is still in use or
/// that its own records disagree, and there is nothing for a user to mean by
/// insisting.
pub(crate) fn decide(status: CloneStatus, insistence: Insistence) -> Decision {
    match status {
        CloneStatus::Referenced { workspace_id } => {
            Decision::Keep(KeptBecause::StillOpened { workspace_id })
        }
        CloneStatus::Orphaned { unsaved, usage } => match objection(&unsaved) {
            None => Decision::Remove {
                usage,
                promotion: Promotion::Unopposed,
            },
            Some(objected) => match insistence {
                Insistence::Insisted => Decision::Remove {
                    usage,
                    promotion: Promotion::Insisted { despite: objected },
                },
                Insistence::NotInsisted => Decision::Keep(KeptBecause::Objected(objected)),
            },
        },
        CloneStatus::Disputed {
            workspace_id,
            sourced_at,
        } => Decision::Keep(KeptBecause::RecordsDisagree {
            workspace_id,
            sourced_at,
        }),
    }
}

// ===========================================================================
// prune: the plan
// ===========================================================================

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
    root: PathBuf,
    /// Biggest first: the report's job is to be acted on, and "which of these is
    /// worth reclaiming" is the comparative question. Path breaks ties so two runs
    /// over an unchanged cache read alike.
    removing: Vec<Reclaimable>,
    keeping: Vec<Kept>,
    /// Worktree records whose directory is definitively not there any more.
    stale_records: Vec<WorktreeInfo>,
}

impl PrunePlan {
    /// Whether this run would change nothing at all.
    pub fn nothing_to_do(&self) -> bool {
        self.removing.is_empty() && self.stale_records.is_empty()
    }

    /// What the whole run would free.
    pub fn freed(&self) -> DiskUsage {
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
}

/// The directory `--prune` scans, canonicalised once.
///
/// `repos_dir` as the clone manager reports it. Taking it from there rather than
/// rebuilding `<cache>/repos` is what keeps the directories scanned, the locks
/// taken and the workspace sources compared answering to the same configuration: a
/// `config.toml` that moves `repos_dir` moves all three, and they cannot drift into
/// scanning one tree while serialising against another or comparing against a
/// third.
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
/// [`prune_plan`] and [`reconcile_plan`] classify a directory by joining two
/// facts: the root the candidates are scanned under, and where every live
/// workspace's source resolves *against that same root*. Taken as two parameters
/// the pair could be built from two different roots — and the join would then
/// mis-classify a clone, reading a healthy one whose workspace was placed against
/// the other root as sourced by nobody, which is an orphan, which is a deletion.
/// One constructor derives both halves from one root, so a mismatched pair has no
/// representation.
#[derive(Debug, Clone, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub struct ClonePlacement {
    root: PathBuf,
    locations: WorkspaceLocations,
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
pub fn prune_plan(
    clones: &WorkspaceCloneManager<'_>,
    storage: &MetadataStorage,
    workspaces: &[Workspace],
    placement: &ClonePlacement,
    insistence: Insistence,
    notices: &mut dyn Notices<LifecycleNotice>,
) -> Result<PrunePlan, PruneError> {
    let ClonePlacement { root, locations } = placement;
    let mut removing: Vec<Reclaimable> = Vec::new();
    let mut keeping: Vec<Kept> = Vec::new();
    let mut cache_notices = Vec::new();
    let record_for = records_by_directory(clones, storage, &mut cache_notices);
    let listed_at = sources_by_workspace(workspaces);
    let git = clones.repo_manager().git();
    for owner_dir in subdirectories(root) {
        for repo_dir in subdirectories(&owner_dir) {
            let (Some(owner), Some(repo)) = (leaf_of(&owner_dir), leaf_of(&repo_dir)) else {
                continue;
            };
            let bare = canonical(
                &clones
                    .repo_manager()
                    .bare_dir(&owner, &repo)
                    .to_string_lossy(),
            );
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
                    &owner,
                    &repo,
                    locations,
                    &record_for,
                    &listed_at,
                );
                match decide(status, insistence) {
                    Decision::Remove { usage, promotion } => removing.push(Reclaimable {
                        path: clone,
                        owner: owner.clone(),
                        repo: repo.clone(),
                        usage,
                        promotion,
                    }),
                    Decision::Keep(because) => keeping.push(Kept {
                        path: clone,
                        because,
                    }),
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
    })
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
fn records_by_directory(
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
fn sources_by_workspace(workspaces: &[Workspace]) -> HashMap<String, String> {
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

fn leaf_of(path: &Path) -> Option<String> {
    Some(path.file_name()?.to_string_lossy().into_owned())
}

// ===========================================================================
// prune: the acting pass
// ===========================================================================

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
}

impl PruneReport {
    /// What this run actually freed — a total over the things it removed, with the
    /// figures the plan measured, so what a person is told they got back is what
    /// they said yes to.
    pub fn freed(&self) -> DiskUsage {
        disk_usage::total_usage(self.removed.iter().map(|it| it.usage.clone()))
    }

    pub fn finished(&self) -> bool {
        self.refused.is_empty()
    }
}

/// How the acting pass ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneOutcome {
    /// A live workspace's source could not be followed, so nothing was removed:
    /// no clone is unreferenced while a workspace is unaccounted for.
    Unlocatable(NonEmpty<Unlocatable>),
    Acted(PruneReport),
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
    };
    let mut forget: Vec<WorktreeInfo> = Vec::new();
    for ((owner, repo), reclaimables) in by_repo {
        let _lock = clones
            .repo_manager()
            .hold_repo_lock(&owner, &repo)
            .map_err(PruneError::Lock)?;
        for reclaimable in reclaimables {
            let status = clone_status(
                &git,
                &reclaimable.path,
                &owner,
                &repo,
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
                Removal::Everything => {
                    report.removed.push((*reclaimable).clone());
                    if let Some(record) = record_for.get(&reclaimable.path) {
                        forget.push(record.clone());
                    }
                }
                Removal::WhatItCould(refused) | Removal::Nothing(refused) => {
                    report.refused.extend(refused.iter().cloned());
                }
            }
        }
    }
    // Outside every repo lock, because the repo lock is what protects the
    // *directory* work and a record drop touches only `metadata.json`, which has a
    // lock of its own. Keeping it out means a repository is held for exactly as
    // long as its clones are being looked at and removed.
    for record in forget.iter().chain(plan.stale_records.iter()) {
        forget_clone(storage, record, notices);
    }
    extend_with_cache(notices, cache_notices);
    Ok(PruneOutcome::Acted(report))
}

/// Drop one worktree record.
///
/// Removing a clone without this is what left `metadata.json` describing workspaces
/// that stopped existing years ago; a record kept for a directory that is gone is
/// not a safety margin, it is the thing that made the file unreadable as a
/// description of anything.
fn forget_clone(
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

// ===========================================================================
// reconcile
// ===========================================================================

/// The clone-directory leaf `branch` had under the pre-#81 scheme: the branch with
/// everything git allows and a path component does not collapsed to dashes.
///
/// Kept because it is the only thing connecting devpod's stale record to a branch —
/// the id it was addressed by is exactly what changed, so the leaf is what
/// survived. This is `dl`'s own history rather than a guess at another tool's
/// format, and it is frozen: a third naming would be a third function, never an
/// edit to this one.
fn legacy_leaf(branch: &str) -> String {
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
    adopting: Vec<Adoptable>,
    reporting: Vec<Unadoptable>,
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

#[cfg(test)]
mod tests {
    //! What the lifecycle commands do, at three seams.
    //!
    //! **The argv seam.** Every devpod call's whole argv, through the fake runner,
    //! because a rewritten body cannot preserve `devpod delete <id>
    //! --ignore-not-found` by accident and the flags are what devpod acts on.
    //!
    //! **Real filesystem, real git.** The prune classification, the delete guard
    //! and the partial-removal walk are all decided by the state of real
    //! directories and by a real `git status` / `git log --not --remotes`. A faked
    //! spawn answers a clean exit with empty output, which reads as "this clone
    //! holds nothing" — the answer that deletes. Written the other way round,
    //! Python's own central guard passed while guarding nothing and the clone with
    //! two unpushed commits in it was removed, so these tests build a local bare
    //! repository standing in for GitHub (a local path is a real git remote) and
    //! let git run.
    //!
    //! **Permissions, verified rather than assumed.** Every refusal case goes
    //! through `refusing_writes`/`refusing_reads`, which apply the mode and then
    //! *try the write*: root is refused by nothing, and a stored-but-ignored mode
    //! is ordinary on bind and overlay mounts. Where the filesystem does not deny,
    //! the test steps aside instead of asserting something it cannot reproduce.
    //!
    //! Ported from `test/unit/test_purge_partial_removal.py`,
    //! `test/unit/test_purge_ownership.py`'s purge-action classes,
    //! `test/unit/test_workspace_listing.py::TestPurgeWillNotActOnAListItCouldNotRead`,
    //! `test/test_workspace_state.py`'s `TestTheDeleteGuard` and
    //! `TestForcedRemoveIsEnsureAbsent`, `test/test_dl.py`'s
    //! `TestBackgroundRefreshSpawning`, `TestRefreshChildRechecksFreshness` and
    //! `TestWorkspaceCommandsRefreshOnceAfterwards`,
    //! `test/unit/test_updater_fetch_sweep.py`,
    //! `test/unit/test_stored_workspace_id.py`,
    //! `test/unit/test_prune_orphaned_clones.py`,
    //! `test/unit/test_workspace_source_placement.py` and
    //! `test/unit/test_reconcile_orphaned_workspaces.py`.
    //!
    //! `remove_tree_as_far_as_it_goes` lives in `flows::repo_manager` beside the
    //! cleanup it is the counterpart of, and is tested from here: `dl --purge` is
    //! its only caller and the three-armed answer exists for the purge's three
    //! headlines.

    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use devlaunch_runner::{
        CapturedText, DetachOutcome, Invocation as RawInvocation, Outcome, ProcessRunner, SpawnSpec,
    };
    use devlaunch_test_support::{FakeRunner, Response};

    use super::*;
    use crate::clients::devpod_home::{ScratchHome, devpod_home_with};
    use crate::clients::{devpod, docker};
    use crate::domain::metadata::MetadataStorage;
    use crate::domain::model::{BaseRepository, Timestamp, WorktreeInfo};
    use crate::domain::workspace_id::WorkspaceId;
    use crate::flows::repo_manager::tests::{refusing_reads, refusing_writes, run_git};
    use crate::flows::repo_manager::{RefusalReason, RemoveTreeError, bare_dir};
    use crate::flows::workspace_clone::GitLfs;

    /// A devpod home whose create result for `workspace_id` records what devpod
    /// substituted into that workspace's devcontainer.
    ///
    /// The shape is devpod's own: `SubstitutionContext` beside `ContainerDetails`
    /// and `MergedConfig` in `workspace_result.json`, with the field spellings
    /// devpod's `config.SubstitutionContext` serialises. Read off the pinned
    /// devpod binary's struct tags rather than assumed, because every volume name
    /// below is built from these two strings.
    fn devpod_home_recording(
        workspace_id: &str,
        local_workspace_folder: &str,
        devcontainer_id: &str,
    ) -> ScratchHome {
        let home = devpod_home_with(&[("default", workspace_id, Some(()))]);
        let result = home.result("default", workspace_id);
        std::fs::write(
            &result,
            serde_json::json!({
                "ContainerDetails": { "Id": "container-id" },
                "MergedConfig": {},
                "SubstitutionContext": {
                    "LocalWorkspaceFolder": local_workspace_folder,
                    "ContainerWorkspaceFolder": "/workspaces/whatever",
                    "DevContainerID": devcontainer_id,
                },
            })
            .to_string(),
        )
        .expect("a create result");
        home
    }

    #[test]
    fn a_devpod_that_cannot_be_run_fails_the_stage_but_one_that_refuses_does_not() {
        // Python's `@timing.staged("devpod-up") get_workspace_state` returns None
        // for a devpod that ran and refused, gave non-JSON, or omitted `state`, so
        // the stage stays `ok`; only a devpod that could not be run at all raises
        // (`DevpodNotInstalled`) and the decorator marks the stage `failed`.
        // Rust's `NotRun` is that case and nothing else (P12/C8).
        let _serialized = timing::exclusive();

        fn devpod_up_outcome(runner: &dyn devlaunch_runner::Runner) -> &'static str {
            timing::install(Some(timing::Registry::start(
                timing::Mode::Document,
                timing::Seam::default(),
                0.0,
            )));
            let _ = workspace_state(runner, "dl-ws");
            let report = timing::emit().expect("a report");
            let document = report.document().expect("a document");
            document
                .stages
                .iter()
                .find(|stage| stage.stage == "devpod-up")
                .expect("a devpod-up stage")
                .outcome
        }

        let missing = FakeRunner::new();
        missing.script(["devpod"], Response::ProgramNotFound);
        assert_eq!(
            devpod_up_outcome(&missing),
            "failed",
            "a devpod that could not be run must fail the stage"
        );

        let refused = FakeRunner::new();
        refused.script(["devpod"], Response::failed(1, "no such workspace\n"));
        assert_eq!(
            devpod_up_outcome(&refused),
            "ok",
            "a devpod that ran and refused is Python's None return — the stage stays ok"
        );

        timing::install(None);
    }

    // ------------------------------------------------------------ test doubles

    /// devpod from the fake, everything else from real processes.
    ///
    /// The shim's arrangement, in-process. git has to be real here for the reason
    /// the module docs give; devpod has to be fake because the listing is the
    /// fixture, and because the two passes of `--prune` are meant to be able to see
    /// different worlds.
    struct Devpod {
        fake: FakeRunner,
        processes: ProcessRunner,
        /// See [`timing::exclusive`]. Last field, so it is dropped last.
        _serialized: timing::Exclusive,
    }

    impl Devpod {
        /// The runner, and the timing exclusion for as long as it lives.
        ///
        /// Every devpod call through it is spanned against the **process-global**
        /// registry (`clients::devpod` names each round trip), and
        /// [`workspace_state`] opens the `devpod-up` stage — so a test holding one
        /// of these would otherwise write into whatever document a concurrent
        /// measured test had installed. In the fixture rather than per test, so a
        /// new test cannot forget it.
        fn new() -> Self {
            Self {
                fake: FakeRunner::new(),
                processes: ProcessRunner,
                _serialized: timing::exclusive(),
            }
        }

        /// What `devpod list --output json` answers from now on.
        fn lists(&self, entries: &[serde_json::Value]) {
            let listing = serde_json::Value::Array(entries.to_vec()).to_string();
            self.fake.clear_scripts();
            self.fake
                .script(["devpod", "list"], Response::stdout(listing));
        }

        /// devpod has a workspace of this name, so `stop` and `delete` address
        /// something. The scripted listing is what `--ls` reads; this is the state
        /// machine underneath it that the other verbs act on.
        fn knows(&self, workspace_id: &str) {
            self.fake.add_workspace(
                workspace_id,
                devlaunch_test_support::WorkspaceState::Running,
            );
        }

        /// devpod refuses the listing outright.
        fn cannot_list(&self, stderr: &str) {
            self.fake.clear_scripts();
            self.fake
                .script(["devpod", "list"], Response::failed(1, stderr));
        }

        fn devpod_argvs(&self) -> Vec<Vec<String>> {
            self.fake.args_to(devpod::PROGRAM)
        }

        /// Every id `devpod delete` was called about, in order.
        fn deleted(&self) -> Vec<String> {
            self.devpod_argvs()
                .into_iter()
                .filter(|argv| argv.first().map(String::as_str) == Some("delete"))
                .filter_map(|argv| argv.get(1).cloned())
                .collect()
        }

        /// Every docker call's argv tail, in order.
        fn docker_argvs(&self) -> Vec<Vec<String>> {
            self.fake.args_to(docker::PROGRAM)
        }

        fn detached(&self) -> Vec<Vec<String>> {
            self.fake
                .calls()
                .into_iter()
                .filter_map(|call| match call {
                    devlaunch_test_support::Call::Detach(invocation) => Some(invocation.argv()),
                    _ => None,
                })
                .collect()
        }
    }

    /// Which programs this fixture answers for itself. Everything else — git,
    /// above all — is really run, which is what the repo_manager fixtures need.
    ///
    /// `docker` is on the list for the reason `devpod` is: a delete spawns it now,
    /// and a unit test that reached the developer's own docker daemon would be
    /// removing real volumes named after a fixture.
    fn faked(program: &str) -> bool {
        program == devpod::PROGRAM || program == docker::PROGRAM
    }

    impl Runner for Devpod {
        fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
            if faked(&spec.invocation.program) {
                self.fake.capture(spec)
            } else {
                self.processes.capture(spec)
            }
        }

        fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
            if faked(&spec.invocation.program) {
                self.fake.passthrough(spec)
            } else {
                self.processes.passthrough(spec)
            }
        }

        fn session(&self, spec: &SpawnSpec, on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
            if faked(&spec.invocation.program) {
                self.fake.session(spec, on_stderr_line)
            } else {
                self.processes.session(spec, on_stderr_line)
            }
        }

        /// Every detached spawn is recorded and never started: the refresh child is
        /// a whole second `dl` run, and a unit test that really forked one would be
        /// running an unrelated program against the developer's own cache.
        fn detach(&self, what: &RawInvocation) -> DetachOutcome {
            self.fake.detach(what)
        }
    }

    // ---------------------------------------------------------------- helpers

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary directory")
    }

    fn commit(work: &Path, message: &str) {
        run_git(work, &["add", "-A"]);
        run_git(work, &["commit", "-m", message]);
    }

    /// One `devpod list --output json` element, sourced at a local folder.
    fn listed(workspace_id: &str, source: &Path) -> serde_json::Value {
        serde_json::json!({
            "id": workspace_id,
            "source": { "localFolder": source.display().to_string() },
            "provider": { "name": "docker" },
            "ide": { "name": "none" },
            "context": "default",
            "lastUsed": "2026-08-08T11:43:27Z",
        })
    }

    /// One element whose source is whatever devpod happened to write.
    fn listed_with(workspace_id: &str, source: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": workspace_id,
            "source": source,
            "provider": { "name": "docker" },
            "ide": { "name": "none" },
            "context": "default",
            "lastUsed": "2026-08-08T11:43:27Z",
        })
    }

    fn one_workspace(id: &str, source: serde_json::Value) -> Workspace {
        devpod::parse_workspaces(&serde_json::json!([listed_with(id, source)]).to_string())
            .expect("a listing")
            .remove(0)
    }

    /// A completion cache file with a chosen age, so freshness is a fixture rather
    /// than a race with the clock.
    fn a_completion_cache(dir: &Path, age: Duration) -> PathBuf {
        let path = dir.join("completions.json");
        std::fs::write(&path, "{}").expect("a completion cache");
        let when = SystemTime::now() - age;
        let times = std::fs::FileTimes::new().set_modified(when);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("the cache file")
            .set_times(times)
            .expect("an mtime");
        path
    }

    fn fresh_cache(dir: &Path) -> PathBuf {
        a_completion_cache(dir, Duration::from_secs(1))
    }

    fn stale_cache(dir: &Path) -> PathBuf {
        a_completion_cache(
            dir,
            completion_cache::COMPLETION_CACHE_TTL + Duration::from_secs(60),
        )
    }

    fn ignoring() -> Vec<LifecycleNotice> {
        Vec::new()
    }

    /// The cache `--prune` and `--reconcile` are pointed at, and the devpod that
    /// answers about it.
    ///
    /// One real clone of each kind the classification has an arm for, plus the bare
    /// cache every one of them was made from. Everything lives under one temp
    /// directory, so the directories scanned, the metadata file and `repos_dir` all
    /// agree without being patched into agreement — a fixture whose clones sit
    /// outside the directory under test is how a guard comes to run zero times.
    struct World {
        dir: tempfile::TempDir,
        cache: PathBuf,
        repos_dir: PathBuf,
        repo_dir: PathBuf,
        origin: PathBuf,
        bare: PathBuf,
        storage: MetadataStorage,
        devpod: Devpod,
    }

    const OWNER: &str = "o";
    const REPO: &str = "r";

    impl World {
        /// A cache with a bare clone of a one-commit remote and nothing else.
        fn empty() -> Self {
            let dir = temp_dir();
            let root = dir.path().to_path_buf();
            let cache = root.join("cache").join("devlaunch");
            let repos_dir = cache.join("repos");
            let repo_dir = repos_dir.join(OWNER).join(REPO);
            std::fs::create_dir_all(&repo_dir).expect("the repo directory");

            let seed = root.join("seed");
            std::fs::create_dir_all(&seed).expect("the seed directory");
            run_git(&root, &["init", "-b", "main", &seed.display().to_string()]);
            std::fs::write(seed.join("README.md"), "seed\n").expect("a README");
            commit(&seed, "seed");
            let origin = root.join("origin.git");
            run_git(
                &root,
                &[
                    "clone",
                    "--bare",
                    &seed.display().to_string(),
                    &origin.display().to_string(),
                ],
            );
            let bare = repo_dir.join(".bare");
            run_git(
                &root,
                &[
                    "clone",
                    "--bare",
                    &origin.display().to_string(),
                    &bare.display().to_string(),
                ],
            );
            let (storage, _) =
                MetadataStorage::open(cache.join("metadata.json")).expect("a metadata store");
            let devpod = Devpod::new();
            devpod.lists(&[]);
            Self {
                dir,
                cache,
                repos_dir,
                repo_dir,
                origin,
                bare,
                storage,
                devpod,
            }
        }

        fn tmp(&self) -> &Path {
            self.dir.path()
        }

        /// One real workspace clone, fully pushed, at `leaf`, on `branch`.
        fn clone_at(&self, leaf: &str, branch: &str) -> PathBuf {
            let clone = self.repo_dir.join(leaf);
            run_git(
                self.tmp(),
                &[
                    "clone",
                    &self.bare.display().to_string(),
                    &clone.display().to_string(),
                ],
            );
            run_git(
                &clone,
                &[
                    "remote",
                    "set-url",
                    "origin",
                    &self.origin.display().to_string(),
                ],
            );
            // `-B` rather than `-b`: a clone of a `main`-headed remote already has
            // `main`, and the fixture asks for that branch by name like any other.
            run_git(&clone, &["checkout", "-B", branch]);
            std::fs::write(clone.join(format!("{branch}.txt")), "work\n").expect("a tracked file");
            commit(&clone, branch);
            run_git(&clone, &["push", "-u", "origin", branch]);
            clone
        }

        /// A worktree record naming `clone`, with `leaf` as its workspace id.
        fn record(&mut self, leaf: &str, branch: &str, clone: &Path) -> WorktreeInfo {
            let record = WorktreeInfo::new(OWNER, REPO, branch, clone.to_path_buf(), leaf);
            self.storage
                .add_worktree(record.clone())
                .expect("the record is written");
            record
        }

        fn branches_on_record(&self) -> Vec<String> {
            let mut branches: Vec<String> = self
                .storage
                .list_worktrees(WorktreeFilter::All)
                .into_iter()
                .map(|record| record.branch.clone())
                .collect();
            branches.sort();
            branches
        }
    }

    /// The clone manager these tests drive.
    ///
    /// A free function over the two fields it needs rather than a method on the
    /// fixture, because a method borrows the whole fixture for the manager's
    /// lifetime — and every caller then also needs `storage` mutably.
    fn clones_for<'r>(repos_dir: &Path, runner: &'r dyn Runner) -> WorkspaceCloneManager<'r> {
        WorkspaceCloneManager::new(
            repos_dir,
            Duration::from_secs(3600),
            Git::new(runner),
            GitLfs::NotInstalled,
        )
    }

    /// The plan `--prune` would print.
    fn plan_for(world: &World, insistence: Insistence) -> PrunePlan {
        let clones = clones_for(&world.repos_dir, &world.devpod);
        let mut context = CommandContext::new(&world.devpod);
        let workspaces = context.workspaces().expect("a listing");
        let placement = ClonePlacement::resolve(&clones, &workspaces);
        prune_plan(
            &clones,
            &world.storage,
            &workspaces,
            &placement,
            insistence,
            &mut ignoring(),
        )
        .expect("a plan")
    }

    /// The paths the plan would remove, in the order it would report them.
    fn removing(plan: &PrunePlan) -> Vec<PathBuf> {
        plan.removing.iter().map(|it| it.path.clone()).collect()
    }

    /// Why the plan keeps `path`, or a failure saying it is not in the plan at all.
    ///
    /// Every assertion about a directory *surviving* goes through here rather than
    /// through an existence check, because "it is still there" is true of a clone
    /// kept for the right reason and of one kept by a guard that was never asked.
    fn kept_because(plan: &PrunePlan, path: &Path) -> KeptBecause {
        let mut found: Vec<&Kept> = plan
            .keeping
            .iter()
            .filter(|kept| kept.path == path)
            .collect();
        assert_eq!(
            found.len(),
            1,
            "expected exactly one report line for {}: {:?}",
            path.display(),
            plan.keeping
        );
        found.remove(0).because.clone()
    }

    // =======================================================================
    // a refused path does not stop the rest (devlaunch#131, #182)
    // =======================================================================

    /// A devlaunch cache with a clone in it the container's user would have
    /// written: `stuck` holds a file, and sealing `stuck` makes that file
    /// impossible for us to unlink.
    struct SealableCache {
        dir: tempfile::TempDir,
        root: PathBuf,
        completions: PathBuf,
        metadata: PathBuf,
        other_clone: PathBuf,
        stuck: PathBuf,
    }

    fn a_sealable_cache() -> SealableCache {
        let dir = temp_dir();
        let root = dir.path().join("devlaunch");
        let other_clone = root
            .join("repos")
            .join("blooop")
            .join("bencher")
            .join("bencher-main-ii41");
        let stuck = root
            .join("repos")
            .join("blooop")
            .join("e2e-repo")
            .join("e2e-purge-devlaunchs");
        std::fs::create_dir_all(&other_clone).expect("a clone that will go");
        std::fs::write(other_clone.join("README.md"), "a clone that will go\n").expect("a README");
        let completions = root.join("completions.json");
        let metadata = root.join("metadata.json");
        std::fs::write(&completions, "{}").expect("a completion cache");
        std::fs::write(&metadata, "{}").expect("a metadata file");
        std::fs::create_dir_all(&stuck).expect("the stuck clone");
        std::fs::write(stuck.join("pixi.lock"), "written by the container's user\n")
            .expect("a file we will not be able to unlink");
        SealableCache {
            dir,
            root,
            completions,
            metadata,
            other_clone,
            stuck,
        }
    }

    /// The refusals of a removal, whichever arm carries them.
    fn refused_paths(removal: &Removal) -> Vec<PathBuf> {
        match removal {
            Removal::Everything => Vec::new(),
            Removal::WhatItCould(refused) | Removal::Nothing(refused) => {
                refused.iter().map(|it| it.path.clone()).collect()
            }
        }
    }

    /// Whether `path` is on disk, where "cannot tell" counts as there — the same
    /// distinction the code under test makes, because both would be reporting
    /// "gone" about something present.
    fn still_there(path: &Path) -> bool {
        present(path)
    }

    #[test]
    fn a_cache_nothing_refuses_goes_completely() {
        let cache = a_sealable_cache();
        assert_eq!(
            remove_tree_as_far_as_it_goes(&cache.root),
            Removal::Everything
        );
        assert!(!cache.root.exists());
    }

    #[test]
    fn a_tree_that_was_never_there_is_a_clean_sweep_not_a_refusal() {
        // A purge run twice is not a failure the second time, and is not a removal
        // that refused nothing while removing nothing either: there is nothing left
        // under that name, which is what the first arm means.
        let dir = temp_dir();
        assert_eq!(
            remove_tree_as_far_as_it_goes(&dir.path().join("never-existed")),
            Removal::Everything
        );
    }

    #[test]
    fn everything_removable_is_removed_and_only_the_obstruction_is_named() {
        // The fault devlaunch#131 measured: one EACCES abandoned the entire cache.
        let cache = a_sealable_cache();
        let Some(_sealed) = refusing_writes(&cache.stuck) else {
            return; // this filesystem does not deny; nothing here can be reproduced
        };
        let removal = remove_tree_as_far_as_it_goes(&cache.root);

        assert!(
            !cache.completions.exists(),
            "a completion cache is removable"
        );
        assert!(!cache.metadata.exists(), "metadata.json is removable");
        assert!(!cache.other_clone.exists(), "another clone is removable");
        assert!(
            cache.stuck.join("pixi.lock").exists(),
            "the sealed file stays"
        );
        assert_eq!(
            refused_paths(&removal),
            [cache.stuck.as_path()],
            "every directory from the cache root down to the sealed one also fails \
             to go, and saying so five times buries the one fact"
        );
        assert!(
            matches!(removal, Removal::WhatItCould(_)),
            "the partial arm has to mean something went: {removal:?}"
        );
    }

    #[test]
    fn the_directory_is_blamed_rather_than_each_file_in_it() {
        // Unlinking needs write permission on the *directory*, not on the file, so a
        // clone owned by the container's user refuses every one of its children
        // separately — and none of them is an ancestor of another, so ancestor
        // suppression alone catches none of them.
        let cache = a_sealable_cache();
        for name in ["README.md", "pyproject.toml", "config"] {
            std::fs::write(cache.stuck.join(name), "also written by the container\n")
                .expect("another file");
        }
        std::fs::create_dir(cache.stuck.join("objects")).expect("an objects directory");
        let Some(_sealed) = refusing_writes(&cache.stuck) else {
            return;
        };
        assert_eq!(
            refused_paths(&remove_tree_as_far_as_it_goes(&cache.root)),
            [cache.stuck.as_path()]
        );
    }

    #[test]
    fn two_separate_obstructions_are_both_listed() {
        // Suppressing ancestors must not suppress siblings.
        let cache = a_sealable_cache();
        let second = cache
            .root
            .join("repos")
            .join("blooop")
            .join("other")
            .join("clone");
        std::fs::create_dir_all(&second).expect("a second clone");
        std::fs::write(second.join("held"), "also stuck\n").expect("a held file");
        let (Some(_one), Some(_two)) = (refusing_writes(&second), refusing_writes(&cache.stuck))
        else {
            return;
        };
        let mut refused = refused_paths(&remove_tree_as_far_as_it_goes(&cache.root));
        refused.sort();
        let mut expected = vec![cache.stuck.clone(), second];
        expected.sort();
        assert_eq!(refused, expected);
    }

    #[test]
    fn a_separately_sealed_ancestor_is_reported_as_well() {
        // Where "ancestors are not listed" stops being the right rule. The outer one
        // does not fail *because* of the inner one — clearing the inner would leave
        // the outer exactly as stuck — so each is a separate piece of work, and a
        // person told only about the inner one would fix it and find the purge still
        // refusing.
        let cache = a_sealable_cache();
        let outer = cache.root.join("repos").join("outer");
        let inner = outer.join("middle").join("inner");
        std::fs::create_dir_all(&inner).expect("the inner directory");
        std::fs::write(inner.join("file"), "x").expect("a file");
        // Deepest first: sealing a parent would make sealing its child fail.
        let (Some(_in), Some(_out)) = (refusing_writes(&inner), refusing_writes(&outer)) else {
            return;
        };
        let mut refused = refused_paths(&remove_tree_as_far_as_it_goes(&cache.root));
        refused.sort();
        let mut expected = vec![inner, outer];
        expected.sort();
        assert_eq!(refused, expected);
    }

    #[test]
    fn a_path_whose_parent_is_writable_is_blamed_itself() {
        // Attribution walks up only as far as the permissions justify: without this,
        // a refusal in a perfectly writable directory would be blamed on an ancestor
        // that has nothing wrong with it.
        let cache = a_sealable_cache();
        let held = cache.root.join("repos").join("blooop").join("held-open");
        std::fs::create_dir_all(&held).expect("the held directory");
        std::fs::write(held.join("inner"), "x\n").expect("a file");
        let Some(_sealed) = refusing_writes(&held) else {
            return;
        };
        assert_eq!(
            refused_paths(&remove_tree_as_far_as_it_goes(&cache.root)),
            [held]
        );
    }

    #[test]
    fn a_cache_root_that_refuses_everything_reports_that_nothing_went() {
        // devlaunch#182's case: the root itself is what will not let go. Nothing
        // under it can be unlinked either, since unlinking an entry needs write
        // permission on the directory holding it — so the whole cache is standing
        // afterwards and the honest answer names no removal at all.
        let dir = temp_dir();
        let root = dir.path().join("devlaunch");
        std::fs::create_dir_all(root.join("repos")).expect("an empty repos directory");
        std::fs::write(root.join("metadata.json"), "{}").expect("a metadata file");
        std::fs::write(root.join("completions.json"), "{}").expect("a completion cache");
        let Some(_sealed) = refusing_writes(&root) else {
            return;
        };
        let removal = remove_tree_as_far_as_it_goes(&root);
        assert!(
            matches!(removal, Removal::Nothing(_)),
            "nothing came away: {removal:?}"
        );
        assert_eq!(refused_paths(&removal), [root.as_path()]);
        assert_eq!(
            std::fs::read_to_string(root.join("metadata.json")).expect("still there"),
            "{}"
        );
        assert!(root.join("repos").is_dir());
    }

    #[test]
    fn a_sealed_root_over_clones_that_did_go_is_still_a_partial_success() {
        // The arm is decided by what moved, not by where the obstruction is. A
        // sealed root refuses its own entries and nothing deeper, so the clones
        // under it go. Reading "the root refused" as "nothing came away" would tell
        // somebody their clones survived when they did not — the same class of lie
        // as devlaunch#182, pointed the other way.
        let dir = temp_dir();
        let root = dir.path().join("devlaunch");
        let clone = root
            .join("repos")
            .join("blooop")
            .join("bencher")
            .join("bencher-main-ii41");
        std::fs::create_dir_all(&clone).expect("a clone");
        std::fs::write(clone.join("README.md"), "a clone that will go\n").expect("a README");
        let Some(_sealed) = refusing_writes(&root) else {
            return;
        };
        let removal = remove_tree_as_far_as_it_goes(&root);
        assert!(
            matches!(removal, Removal::WhatItCould(_)),
            "the clones under a sealed root are still removable: {removal:?}"
        );
        assert!(!clone.exists());
    }

    #[test]
    fn a_root_that_cannot_even_be_looked_at_removed_nothing() {
        // "Cannot tell" is not a partial success either: the lstat is refused before
        // a single path is attempted, so there is nothing this could have removed.
        let dir = temp_dir();
        let home = dir.path().join("cachehome");
        let root = home.join("devlaunch");
        std::fs::create_dir_all(&root).expect("the cache");
        std::fs::write(root.join("metadata.json"), "still here").expect("a metadata file");
        let Some(_sealed) = refusing_reads(&home) else {
            return;
        };
        let removal = remove_tree_as_far_as_it_goes(&root);
        assert!(
            matches!(removal, Removal::Nothing(_)),
            "nothing was attempted: {removal:?}"
        );
        assert_eq!(refused_paths(&removal), [root]);
    }

    #[test]
    fn a_symlinked_root_is_refused_and_left_where_it_is() {
        // Refused, not followed and not quietly unlinked. Unlinking only the link
        // reports a clean sweep over clones that are still on disk on another
        // volume, and following it empties a directory the caller never named. A
        // cache root is a symlink because somebody moved their cache, so both
        // answers cost them the same thing by opposite routes.
        //
        // Needs no permissions, so it holds as root too — which matters, because it
        // is the arm a container running as root would otherwise never exercise.
        let dir = temp_dir();
        let target = dir.path().join("elsewhere");
        std::fs::create_dir_all(target.join("repos")).expect("somebody's cache");
        std::fs::write(target.join("metadata.json"), "somebody's cache").expect("their metadata");
        std::fs::write(target.join("repos").join("work.txt"), "somebody's work")
            .expect("their work");
        let link = dir.path().join("cache").join("devlaunch");
        std::fs::create_dir_all(link.parent().expect("a parent")).expect("the cache parent");
        std::os::unix::fs::symlink(&target, &link).expect("a symlink");

        let removal = remove_tree_as_far_as_it_goes(&link);

        let Removal::Nothing(refused) = &removal else {
            panic!("expected a removal that removed nothing, got {removal:?}");
        };
        assert_eq!(refused.len(), 1);
        let refusal = refused.iter().next().expect("one refusal");
        assert_eq!(refusal.path, link);
        // The advice a report gives is `sudo rm -rf <cache>`, which would remove the
        // link and nothing else, so the reason has to carry the real location.
        assert_eq!(
            refusal.reason,
            RefusalReason::RootIsSymlink {
                points_at: Some(target.clone())
            }
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("the link")
                .file_type()
                .is_symlink(),
            "the link is left where it is, not silently removed"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("metadata.json")).expect("their metadata"),
            "somebody's cache"
        );
        assert!(target.join("repos").join("work.txt").exists());
    }

    #[test]
    fn a_symlink_inside_the_tree_is_unlinked_not_followed() {
        let cache = a_sealable_cache();
        let outside = cache.dir.path().join("outside");
        std::fs::create_dir_all(&outside).expect("a directory outside the tree");
        std::fs::write(outside.join("precious.txt"), "not devlaunch's").expect("their file");
        std::os::unix::fs::symlink(&outside, cache.root.join("repos").join("link"))
            .expect("a link to a directory");
        std::os::unix::fs::symlink(
            outside.join("precious.txt"),
            cache.root.join("repos").join("file-link"),
        )
        .expect("a link to a file");

        assert_eq!(
            remove_tree_as_far_as_it_goes(&cache.root),
            Removal::Everything
        );
        assert!(!cache.root.exists());
        assert_eq!(
            std::fs::read_to_string(outside.join("precious.txt")).expect("still there"),
            "not devlaunch's"
        );
    }

    #[test]
    fn a_dangling_symlink_is_removed_without_complaint() {
        let cache = a_sealable_cache();
        std::os::unix::fs::symlink(
            cache.root.join("never-existed"),
            cache.root.join("repos").join("broken"),
        )
        .expect("a dangling link");
        assert_eq!(
            remove_tree_as_far_as_it_goes(&cache.root),
            Removal::Everything
        );
        assert!(!cache.root.exists());
    }

    #[test]
    fn an_unreadable_directory_is_reported_rather_than_skipped() {
        // A directory that cannot even be listed must not pass for empty: without
        // reporting the scan failure the tree would be walked as though it held
        // nothing, the rmdir would fail on it, and the contents would be neither
        // removed nor mentioned.
        let cache = a_sealable_cache();
        let opaque = cache.root.join("repos").join("blooop").join("opaque");
        std::fs::create_dir_all(&opaque).expect("the opaque directory");
        std::fs::write(opaque.join("inside"), "unreadable\n").expect("a file inside");
        let Some(_sealed) = refusing_reads(&opaque) else {
            return;
        };
        let refused = refused_paths(&remove_tree_as_far_as_it_goes(&cache.root));
        assert!(
            refused.contains(&opaque),
            "the directory it could not read must be named: {refused:?}"
        );
    }

    #[test]
    fn an_unlistable_but_empty_directory_is_not_reported() {
        // The other half of the case above, and it goes the opposite way: the scan
        // fails, but the directory is empty so the rmdir afterwards succeeds and
        // there is nothing left to report. Treating the scan failure as the refusal
        // named a path that is not there, and — through the ancestor rule — could
        // have silenced a genuine refusal above it.
        let cache = a_sealable_cache();
        let opaque = cache.root.join("repos").join("blooop").join("opaque");
        std::fs::create_dir_all(&opaque).expect("the opaque directory");
        let Some(_sealed) = refusing_reads(&opaque) else {
            return;
        };
        assert_eq!(
            remove_tree_as_far_as_it_goes(&cache.root),
            Removal::Everything
        );
        assert!(!cache.root.exists());
    }

    #[test]
    fn every_refused_path_is_still_on_disk_afterwards() {
        let cache = a_sealable_cache();
        let Some(_sealed) = refusing_writes(&cache.stuck) else {
            return;
        };
        let refused = refused_paths(&remove_tree_as_far_as_it_goes(&cache.root));
        assert!(!refused.is_empty(), "the sealed directory must refuse");
        for path in refused {
            assert!(
                still_there(&path),
                "{} was reported as refused but is gone",
                path.display()
            );
        }
    }

    #[test]
    fn the_two_invariants_hold_over_randomised_trees() {
        // Hand-built cases check the shapes somebody thought of. This checks the
        // rest, and two invariants are the whole contract:
        //
        // - **nothing survives unsaid** — a tree still on disk with an empty refusal
        //   list is a purge claiming a clean sweep it did not have, and is the only
        //   failure here that costs anybody anything;
        // - **nothing is said that is not there** — naming a path the user then
        //   cannot find is how a report stops being believed.
        //
        // A third, which only symlinks can break: **nothing outside the tree is
        // touched.** Every trial plants links to a canary directory alongside the
        // tree, and the canary's contents are checked afterwards.
        //
        // Seeded, so a failure here is reproducible rather than a rumour.
        let dir = temp_dir();
        let canary = dir.path().join("canary");
        std::fs::create_dir_all(&canary).expect("the canary");
        std::fs::write(canary.join("precious"), "outside the tree").expect("the canary's file");
        let mut rng = Seeded::new(20260808);

        for trial in 0..60 {
            let root = dir.path().join(format!("tree{trial}"));
            std::fs::create_dir_all(&root).expect("a tree root");
            let mut made = vec![root.clone()];
            for _ in 0..rng.upto(11) {
                let parent = made[rng.upto(made.len())].clone();
                let child = parent.join(format!("d{}", rng.upto(4)));
                let _ = std::fs::create_dir(&child);
                if !made.contains(&child) {
                    made.push(child);
                }
            }
            for directory in made.clone() {
                for _ in 0..rng.upto(3) {
                    let _ = std::fs::write(directory.join(format!("f{}", rng.upto(4))), "x");
                }
                let roll = rng.upto(100);
                let link = directory.join(format!("l{}", rng.upto(3)));
                if roll < 15 {
                    let _ = std::os::unix::fs::symlink(&canary, &link);
                } else if roll < 25 {
                    let _ = std::os::unix::fs::symlink(canary.join("precious"), &link);
                } else if roll < 30 {
                    let _ = std::os::unix::fs::symlink(dir.path().join("nowhere"), &link);
                }
            }
            // Deepest first: sealing a parent would make sealing its child fail.
            let mut deepest = made.clone();
            deepest.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
            let mut sealed = Vec::new();
            for directory in deepest {
                if rng.upto(4) == 0 {
                    let mode = [0o000u32, 0o100, 0o300, 0o400, 0o500][rng.upto(5)];
                    if let Some(denied) = denying(&directory, mode) {
                        sealed.push(denied);
                    }
                }
            }

            let refused = refused_paths(&remove_tree_as_far_as_it_goes(&root));
            let survived = still_there(&root);
            let complaint = format!("trial {trial}: survives={survived} refused={refused:?}");
            // Permissions restored before the assertions, so a failure does not also
            // wreck the temp directory's cleanup.
            drop(sealed);
            assert_eq!(survived, !refused.is_empty(), "{complaint}");
            for path in &refused {
                assert!(
                    still_there(path),
                    "trial {trial}: reported {}, which is not there",
                    path.display()
                );
            }
            let unique: std::collections::HashSet<&PathBuf> = refused.iter().collect();
            assert_eq!(unique.len(), refused.len(), "trial {trial}: duplicates");
            assert_eq!(
                std::fs::read_to_string(canary.join("precious")).expect("the canary"),
                "outside the tree",
                "trial {trial}: a symlink was followed out of the tree"
            );
            let _ = std::process::Command::new("chmod")
                .args(["-R", "u+rwx", &root.display().to_string()])
                .status();
        }
    }

    /// A directory whose mode this test tightened, restored when this drops.
    ///
    /// `repo_manager`'s `refusing_writes` verifies one specific mode; the randomised
    /// trial needs five of them, and needs the restore to happen even when the
    /// assertion under it fails.
    struct Denying {
        path: PathBuf,
        was: u32,
    }

    impl Drop for Denying {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.was));
        }
    }

    fn denying(path: &Path, mode: u32) -> Option<Denying> {
        use std::os::unix::fs::PermissionsExt as _;
        // SAFETY: a bare `geteuid` syscall, which cannot fail and touches nothing.
        if unsafe { libc::geteuid() } == 0 {
            return None;
        }
        let was = std::fs::metadata(path).ok()?.permissions().mode();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).ok()?;
        Some(Denying {
            path: path.to_path_buf(),
            was,
        })
    }

    /// A tiny deterministic generator, so a failed trial is reproducible.
    struct Seeded(u64);

    impl Seeded {
        fn new(seed: u64) -> Self {
            Self(seed)
        }

        fn upto(&mut self, bound: usize) -> usize {
            // xorshift64*, which is plenty for choosing directory shapes.
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            if bound == 0 {
                0
            } else {
                (self.0 % bound as u64) as usize
            }
        }
    }

    // =======================================================================
    // purge: only what devlaunch made, and it names what it leaves
    // =======================================================================

    /// The recorded six-workspace listing, rehomed under `cache_dir`.
    ///
    /// The two foreign workspaces are interleaved with the four clones rather than
    /// appended, because a split that happened to keep listing order would pass a
    /// test where they were not.
    fn six_workspaces(cache_dir: &Path) -> Vec<serde_json::Value> {
        let repos = cache_dir.join("repos").join("blooop");
        vec![
            listed(
                "bencher-test1-mxvm",
                &repos.join("bencher").join("bencher-test1-mxvm"),
            ),
            listed(
                "bencher-main-ii41",
                &repos.join("bencher").join("bencher-main-ii41"),
            ),
            listed("devlaunch", Path::new("/home/dev/projects/devlaunch")),
            listed(
                "devlaunch-main-3j1t",
                &repos.join("devlaunch").join("devlaunch-main-3j1t"),
            ),
            listed(
                "devlaunch-t1-d7bw",
                &repos.join("devlaunch").join("devlaunch-t1-d7bw"),
            ),
            listed(
                "pythontemplate",
                Path::new("/home/dev/projects/python_template"),
            ),
        ]
    }

    const CLONED_BY_DEVLAUNCH: [&str; 4] = [
        "bencher-test1-mxvm",
        "bencher-main-ii41",
        "devlaunch-main-3j1t",
        "devlaunch-t1-d7bw",
    ];

    /// A cache directory with something in it worth removing.
    fn a_cache_directory(dir: &Path) -> PathBuf {
        let cache = dir.join("devlaunch");
        std::fs::create_dir_all(cache.join("repos")).expect("a repos directory");
        std::fs::write(cache.join("completions.json"), "{}").expect("a completion cache");
        cache
    }

    fn purge(devpod: &Devpod, cache_dir: &Path) -> PurgeOutcome {
        let mut context = CommandContext::new(devpod);
        let plan = purge_plan(&mut context, cache_dir).expect("a plan");
        purge_all_data(&mut context, &plan, None, &mut |_| {}).expect("devpod ran")
    }

    /// `--purge` does not share `workspace_delete` — it issues its own captured
    /// `devpod delete --force` per workspace — so the volumes have to be wired here
    /// too rather than inherited. This is the test that says so.
    #[test]
    fn a_purge_removes_the_volumes_of_every_workspace_it_deleted() {
        let dir = temp_dir();
        let cache = a_cache_directory(dir.path());
        let clone = cache.join("repos").join("o").join("r").join("r-main-aa");
        let devpod = Devpod::new();
        devpod.lists(&[listed("r-main-aa", &clone)]);
        devpod.knows("r-main-aa");
        let home = devpod_home_recording("r-main-aa", "/host/clones/opened-as", "dc9a8b7c");
        let mut context = CommandContext::new(&devpod);
        let plan = purge_plan(&mut context, &cache).expect("a plan");

        purge_all_data(&mut context, &plan, Some(&home), &mut |_| {}).expect("devpod ran");

        assert_eq!(
            devpod.docker_argvs(),
            [[
                "volume",
                "rm",
                "--force",
                "opened-as-pixi",
                "dind-var-lib-docker-dc9a8b7c",
            ]]
        );
    }

    #[test]
    fn a_purge_leaves_the_volumes_of_a_workspace_devpod_would_not_delete() {
        // The container is still there holding them, so removing its volumes would
        // fail anyway — and a purge that reported a removal it never made would be
        // worse than one that says the delete failed and stops there.
        let dir = temp_dir();
        let cache = a_cache_directory(dir.path());
        let clone = cache.join("repos").join("o").join("r").join("r-main-aa");
        let devpod = Devpod::new();
        devpod.lists(&[listed("r-main-aa", &clone)]);
        devpod.fake.script(
            ["devpod", "delete"],
            Response::failed(1, "container is busy\n"),
        );
        // devpod *has* the workspace, so the scripted refusal is the only reason the
        // delete fails: a workspace the fake never heard of would refuse anyway and
        // the test would pass without saying anything.
        devpod.knows("r-main-aa");
        let home = devpod_home_recording("r-main-aa", "/host/clones/opened-as", "dc9a8b7c");
        let mut context = CommandContext::new(&devpod);
        let plan = purge_plan(&mut context, &cache).expect("a plan");

        purge_all_data(&mut context, &plan, Some(&home), &mut |_| {}).expect("devpod ran");

        assert_eq!(devpod.docker_argvs(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn a_purge_says_which_workspaces_volumes_it_could_not_remove() {
        let dir = temp_dir();
        let cache = a_cache_directory(dir.path());
        let clone = cache.join("repos").join("o").join("r").join("r-main-aa");
        let devpod = Devpod::new();
        devpod.lists(&[listed("r-main-aa", &clone)]);
        devpod.fake.script(
            ["docker", "volume", "rm"],
            Response::failed(1, "volume is in use\n"),
        );
        devpod.knows("r-main-aa");
        let home = devpod_home_recording("r-main-aa", "/host/clones/opened-as", "dc9a8b7c");
        let mut steps = Vec::new();
        let mut context = CommandContext::new(&devpod);
        let plan = purge_plan(&mut context, &cache).expect("a plan");

        purge_all_data(&mut context, &plan, Some(&home), &mut |step| {
            steps.push(step)
        })
        .expect("devpod ran");

        assert_eq!(
            steps,
            vec![
                PurgeStep::Deleting {
                    workspace_id: "r-main-aa".to_owned(),
                },
                PurgeStep::VolumesNotRemoved {
                    workspace_id: "r-main-aa".to_owned(),
                    refusal: VolumeRefusal::Docker {
                        exit: Exit::Code(1),
                        stderr: "volume is in use\n".to_owned(),
                    },
                },
            ]
        );
    }

    /// A purge on a machine with no docker is a purge that behaves exactly as it did
    /// before this existed: nothing added here may fail on a host that never had a
    /// volume to leak.
    #[test]
    fn a_purge_on_a_machine_with_no_docker_says_nothing_about_it() {
        let dir = temp_dir();
        let cache = a_cache_directory(dir.path());
        let clone = cache.join("repos").join("o").join("r").join("r-main-aa");
        let devpod = Devpod::new();
        devpod.lists(&[listed("r-main-aa", &clone)]);
        devpod.fake.script_missing("docker");
        devpod.knows("r-main-aa");
        let home = devpod_home_recording("r-main-aa", "/host/clones/opened-as", "dc9a8b7c");
        let mut steps = Vec::new();
        let mut context = CommandContext::new(&devpod);
        let plan = purge_plan(&mut context, &cache).expect("a plan");

        let outcome = purge_all_data(&mut context, &plan, Some(&home), &mut |step| {
            steps.push(step)
        })
        .expect("devpod ran");

        assert_eq!(outcome, PurgeOutcome::Removed { cache_dir: cache });
        assert_eq!(
            steps,
            vec![PurgeStep::Deleting {
                workspace_id: "r-main-aa".to_owned(),
            }]
        );
    }

    #[test]
    fn a_purge_deletes_the_clones_devlaunch_made_and_leaves_the_rest() {
        let dir = temp_dir();
        let cache = a_cache_directory(dir.path());
        let devpod = Devpod::new();
        devpod.lists(&six_workspaces(&cache));

        let outcome = purge(&devpod, &cache);

        assert_eq!(devpod.deleted(), CLONED_BY_DEVLAUNCH);
        assert!(!cache.exists(), "the cache goes too");
        assert_eq!(outcome, PurgeOutcome::Removed { cache_dir: cache });
    }

    #[test]
    fn a_purge_asks_devpod_to_delete_with_force_and_nothing_else() {
        // argv-exact. `--force` here is devpod's: the directory the workspace opens
        // is about to be deleted, so a container devpod cannot reach cleanly must
        // not leave a record behind.
        let dir = temp_dir();
        let cache = a_cache_directory(dir.path());
        let clone = cache
            .join("repos")
            .join("blooop")
            .join("r")
            .join("r-main-aa");
        let devpod = Devpod::new();
        devpod.lists(&[listed("r-main-aa", &clone)]);

        purge(&devpod, &cache);

        assert_eq!(
            devpod.devpod_argvs(),
            [
                vec!["list", "--output", "json"],
                vec!["delete", "r-main-aa", "--force"],
            ]
        );
    }

    #[test]
    fn the_plan_counts_only_what_will_be_deleted_and_names_the_survivors() {
        // It used to say six DevPod workspaces and mean six, two of them somebody
        // else's; it now says four and means four — and the survivors are named
        // while saying no is still an option.
        let dir = temp_dir();
        let cache = a_cache_directory(dir.path());
        let devpod = Devpod::new();
        devpod.lists(&six_workspaces(&cache));
        let mut context = CommandContext::new(&devpod);

        let plan = purge_plan(&mut context, &cache).expect("a plan");

        assert_eq!(
            plan.ownership
                .mine
                .iter()
                .map(|it| it.id.as_str())
                .collect::<Vec<_>>(),
            CLONED_BY_DEVLAUNCH
        );
        assert_eq!(
            plan.ownership
                .foreign
                .iter()
                .map(|it| it.id.as_str())
                .collect::<Vec<_>>(),
            ["devlaunch", "pythontemplate"]
        );
    }

    #[test]
    fn pointing_the_cache_elsewhere_makes_a_purge_recognise_nothing() {
        // The scratch-XDG recipe protects `--purge` for real: XDG_CACHE_HOME does not
        // scope `devpod list`, so a scratch run used to see — and delete — every real
        // workspace.
        let dir = temp_dir();
        let real_cache = a_cache_directory(&dir.path().join("real"));
        let scratch = dir.path().join("scratch").join("devlaunch");
        let devpod = Devpod::new();
        devpod.lists(&six_workspaces(&real_cache));

        let outcome = purge(&devpod, &scratch);

        assert_eq!(devpod.deleted(), Vec::<String>::new());
        assert_eq!(outcome, PurgeOutcome::NothingToPurge);
        assert!(real_cache.exists());
    }

    #[test]
    fn a_purge_with_nothing_of_its_own_and_no_cache_has_nothing_to_purge() {
        let dir = temp_dir();
        let devpod = Devpod::new();
        devpod.lists(&[listed(
            "pythontemplate",
            Path::new("/home/dev/projects/python_template"),
        )]);

        let outcome = purge(&devpod, &dir.path().join("never-made"));

        assert_eq!(outcome, PurgeOutcome::NothingToPurge);
        assert_eq!(devpod.deleted(), Vec::<String>::new());
    }

    #[test]
    fn a_purge_that_deleted_workspaces_but_had_no_cache_is_not_nothing_to_purge() {
        // Python reached the same exit code by a branch that printed neither
        // sentence, so the distinction had no representation. Four workspaces went;
        // that is not nothing.
        let dir = temp_dir();
        let cache = dir.path().join("devlaunch");
        let devpod = Devpod::new();
        devpod.lists(&[listed(
            "r-main-aa",
            &cache
                .join("repos")
                .join("blooop")
                .join("r")
                .join("r-main-aa"),
        )]);

        let outcome = purge(&devpod, &cache);

        assert_eq!(outcome, PurgeOutcome::NoCacheDirectory);
        assert_eq!(devpod.deleted(), ["r-main-aa"]);
    }

    #[test]
    fn a_purge_will_not_act_on_a_list_it_could_not_read() {
        // The caller the ticket is named for: a purge that quietly did nothing used
        // to look exactly like a purge that had nothing to do.
        let dir = temp_dir();
        let cache = a_cache_directory(dir.path());
        let devpod = Devpod::new();
        devpod.cannot_list("context not found\n");
        let mut context = CommandContext::new(&devpod);

        let refused = purge_plan(&mut context, &cache);

        match refused {
            Err(ListingUnreadable::Failed { stderr, .. }) => {
                assert!(stderr.contains("context not found"), "{stderr}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(devpod.deleted(), Vec::<String>::new());
        assert!(
            cache.exists(),
            "a purge that could not read the list must not half-run"
        );
    }

    #[test]
    fn a_purge_forgets_the_workspace_list_once_it_has_deleted_something() {
        let dir = temp_dir();
        let cache = a_cache_directory(dir.path());
        let clone = cache
            .join("repos")
            .join("blooop")
            .join("r")
            .join("r-main-aa");
        let devpod = Devpod::new();
        devpod.lists(&[listed("r-main-aa", &clone)]);
        let mut context = CommandContext::new(&devpod);
        let plan = purge_plan(&mut context, &cache).expect("a plan");

        purge_all_data(&mut context, &plan, None, &mut |_| {}).expect("devpod ran");
        devpod.lists(&[]);
        assert_eq!(
            context.workspaces().expect("a listing"),
            Vec::new(),
            "the snapshot the plan was built from must not answer a later read"
        );
    }

    #[test]
    fn a_workspace_devpod_would_not_delete_is_reported_and_the_cache_still_goes() {
        // One failed delete must not cost the rest of the cache its removal — and it
        // must not pass in silence either.
        let dir = temp_dir();
        let cache = a_cache_directory(dir.path());
        let clone = cache
            .join("repos")
            .join("blooop")
            .join("r")
            .join("r-main-aa");
        let devpod = Devpod::new();
        devpod.lists(&[listed("r-main-aa", &clone)]);
        devpod.fake.script(
            ["devpod", "delete"],
            Response::failed(1, "container is busy\n"),
        );
        let mut steps = Vec::new();
        let mut context = CommandContext::new(&devpod);
        let plan = purge_plan(&mut context, &cache).expect("a plan");

        let outcome = purge_all_data(&mut context, &plan, None, &mut |step| steps.push(step))
            .expect("devpod ran");

        assert_eq!(outcome, PurgeOutcome::Removed { cache_dir: cache });
        // The step is the failure's one report — no notice doubles it.
        assert!(matches!(
            steps.as_slice(),
            [
                PurgeStep::Deleting { .. },
                PurgeStep::NotDeleted { workspace_id, stderr, .. },
            ] if workspace_id == "r-main-aa" && stderr.contains("container is busy")
        ));
    }

    #[test]
    fn a_purge_says_which_of_the_three_removals_happened() {
        // devlaunch#182: the exit status deliberately stays two-valued, so the arm is
        // the only place the difference between "one clone stayed" and "nothing
        // moved" is carried.
        let cache = a_sealable_cache();
        let devpod = Devpod::new();
        devpod.lists(&[]);
        let Some(_sealed) = refusing_writes(&cache.stuck) else {
            return;
        };

        let outcome = purge(&devpod, &cache.root);

        let PurgeOutcome::RemovedWhatItCould { refused, .. } = &outcome else {
            panic!("expected a partial removal, got {outcome:?}");
        };
        assert_eq!(
            refused
                .iter()
                .map(|it| it.path.as_path())
                .collect::<Vec<_>>(),
            [cache.stuck.as_path()]
        );
        assert!(
            !outcome.finished(),
            "a clone the user was told would go is still there"
        );
        assert!(
            !cache.metadata.exists(),
            "the partial arm has to mean something went"
        );
    }

    #[test]
    fn a_purge_of_a_symlinked_cache_does_not_report_success() {
        let dir = temp_dir();
        let target = dir.path().join("elsewhere");
        std::fs::create_dir_all(&target).expect("somebody's cache");
        std::fs::write(target.join("metadata.json"), "somebody's cache").expect("their metadata");
        let root = dir.path().join("cache").join("devlaunch");
        std::fs::create_dir_all(root.parent().expect("a parent")).expect("the cache parent");
        std::os::unix::fs::symlink(&target, &root).expect("a symlink");
        let devpod = Devpod::new();
        devpod.lists(&[]);

        let outcome = purge(&devpod, &root);

        assert!(
            matches!(outcome, PurgeOutcome::RemovedNothing { .. }),
            "{outcome:?}"
        );
        assert!(!outcome.finished());
        assert_eq!(
            std::fs::read_to_string(target.join("metadata.json")).expect("their metadata"),
            "somebody's cache"
        );
    }

    #[test]
    fn a_cache_that_cannot_be_looked_at_is_not_mistaken_for_absent() {
        // A cache whose *parent* cannot be traversed used to come out as "No data to
        // purge." and exit 0 with the cache fully intact — a clean sweep reported
        // over untouched data, which is the one failure the whole change prevents.
        let dir = temp_dir();
        let home = dir.path().join("cachehome");
        let root = home.join("devlaunch");
        std::fs::create_dir_all(&root).expect("the cache");
        std::fs::write(root.join("metadata.json"), "still here").expect("a metadata file");
        let devpod = Devpod::new();
        devpod.lists(&[]);
        let Some(_sealed) = refusing_reads(&home) else {
            return;
        };

        let outcome = purge(&devpod, &root);

        assert!(
            matches!(outcome, PurgeOutcome::RemovedNothing { .. }),
            "{outcome:?}"
        );
        assert_ne!(outcome, PurgeOutcome::NothingToPurge);
    }

    // =======================================================================
    // stop and delete: argv-exact, and the clone follows the workspace
    // =======================================================================

    struct Stopping {
        dir: tempfile::TempDir,
        devpod: Devpod,
        updater: SelfInvocation,
        cache_path: PathBuf,
    }

    fn a_stopping_world() -> Stopping {
        let dir = temp_dir();
        let cache_path = fresh_cache(dir.path());
        let devpod = Devpod::new();
        devpod.lists(&[]);
        devpod.knows("myws");
        Stopping {
            dir,
            devpod,
            updater: SelfInvocation::new("dl"),
            cache_path,
        }
    }

    #[test]
    fn a_stop_asks_devpod_to_stop_that_workspace_and_nothing_else() {
        let world = a_stopping_world();
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&world.updater, &world.cache_path);

        let outcome = workspace_stop(&mut context, &mut refresh, "myws").expect("devpod ran");

        assert_eq!(outcome, StopOutcome::Stopped);
        assert_eq!(world.devpod.devpod_argvs(), [vec!["stop", "myws"]]);
    }

    #[test]
    fn a_stop_forces_exactly_one_refresh_and_forgets_the_listing() {
        // The cache is wrong regardless of age, and a *stale* cache buys no second
        // sweep: the one refresh a stop gets is the one that runs after the stop.
        let world = a_stopping_world();
        let stale = stale_cache(world.dir.path());
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&world.updater, &stale);

        workspace_stop(&mut context, &mut refresh, "myws").expect("devpod ran");
        workspace_stop(&mut context, &mut refresh, "myws").expect("devpod ran");

        assert_eq!(
            world.devpod.detached(),
            [vec!["dl", "--update-cache", "--force"]]
        );
    }

    #[test]
    fn a_stop_devpod_refused_says_so() {
        let world = a_stopping_world();
        world
            .devpod
            .fake
            .script(["devpod", "stop"], Response::exited(1));
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&world.updater, &world.cache_path);

        assert_eq!(
            workspace_stop(&mut context, &mut refresh, "myws").expect("devpod ran"),
            StopOutcome::DevpodRefused {
                exit: Exit::Code(1)
            }
        );
    }

    #[test]
    fn a_plain_delete_names_the_workspace_and_no_flags() {
        let world = a_stopping_world();
        let mut world_cache = World::empty();
        let clones = clones_for(&world_cache.repos_dir, &world_cache.devpod);
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&world.updater, &world.cache_path);

        let outcome = workspace_delete(
            &mut context,
            &mut refresh,
            &clones,
            &mut world_cache.storage,
            None,
            "myws",
            Insistence::NotInsisted,
            &mut ignoring(),
        )
        .expect("devpod ran");

        assert_eq!(
            outcome,
            DeleteOutcome::Deleted {
                clone: Ok(Removed::NothingRecorded),
                volumes: VolumeSweep::NothingNamed,
            }
        );
        assert_eq!(world.devpod.devpod_argvs(), [vec!["delete", "myws"]]);
    }

    #[test]
    fn a_forced_delete_passes_devpods_own_ignore_not_found() {
        // `rm -f` semantics: the contract is the state afterwards, not the work done.
        // A cold-launch bench reset runs this before *every* timed run, including the
        // first, where there is nothing to remove yet.
        let world = a_stopping_world();
        let mut world_cache = World::empty();
        let clones = clones_for(&world_cache.repos_dir, &world_cache.devpod);
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&world.updater, &world.cache_path);

        workspace_delete(
            &mut context,
            &mut refresh,
            &clones,
            &mut world_cache.storage,
            None,
            "myws",
            Insistence::Insisted,
            &mut ignoring(),
        )
        .expect("devpod ran");

        assert_eq!(
            world.devpod.devpod_argvs(),
            [vec!["delete", "myws", "--ignore-not-found"]]
        );
    }

    #[test]
    fn a_delete_devpod_refused_keeps_the_local_clone() {
        // devpod re-parses the workspace's devcontainer.json to tear the container
        // down, so removing the clone regardless strands the workspace for good.
        let mut world = World::empty();
        let clone = world.clone_at("r-main-aa", "main");
        world.record("r-main-aa", "main", &clone);
        world
            .devpod
            .fake
            .script(["devpod", "delete"], Response::exited(1));
        let clones = clones_for(&world.repos_dir, &world.devpod);
        let updater = SelfInvocation::new("dl");
        let cache_path = fresh_cache(world.tmp());
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&updater, &cache_path);

        let outcome = workspace_delete(
            &mut context,
            &mut refresh,
            &clones,
            &mut world.storage,
            None,
            "r-main-aa",
            Insistence::NotInsisted,
            &mut ignoring(),
        )
        .expect("devpod ran");

        assert_eq!(
            outcome,
            DeleteOutcome::DevpodRefused {
                exit: Exit::Code(1)
            }
        );
        assert!(
            clone.exists(),
            "the clone stays so the delete stays retryable"
        );
        assert_eq!(world.branches_on_record(), ["main"]);
    }

    #[test]
    fn a_delete_devpod_allowed_takes_the_clone_and_its_record() {
        let mut world = World::empty();
        let clone = world.clone_at("r-main-aa", "main");
        world.record("r-main-aa", "main", &clone);
        world.devpod.knows("r-main-aa");
        let clones = clones_for(&world.repos_dir, &world.devpod);
        let updater = SelfInvocation::new("dl");
        let cache_path = fresh_cache(world.tmp());
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&updater, &cache_path);
        let mut notices = Vec::new();

        let outcome = workspace_delete(
            &mut context,
            &mut refresh,
            &clones,
            &mut world.storage,
            None,
            "r-main-aa",
            Insistence::NotInsisted,
            &mut notices,
        )
        .expect("devpod ran");

        assert_eq!(
            outcome,
            DeleteOutcome::Deleted {
                clone: Ok(Removed::Clone),
                volumes: VolumeSweep::NothingNamed,
            }
        );
        assert!(!clone.exists());
        assert_eq!(world.branches_on_record(), Vec::<String>::new());
        assert!(notices.contains(&LifecycleNotice::CloneRemoved {
            workspace_id: "r-main-aa".to_owned()
        }));
    }

    #[test]
    fn a_clone_that_could_not_be_removed_reports_the_refusal_and_not_a_rendering_of_it() {
        // The workspace is gone whatever happened to the clone, so this is a notice
        // and the delete still succeeds. What the notice carries is the refusal
        // itself: a symlinked root has a `points_at` worth naming, and choosing the
        // words for it is the binary's job, not core's.
        let mut world = World::empty();
        let elsewhere = world.tmp().join("moved-clone");
        let clone = world.repo_dir.join("r-main-aa");
        std::fs::create_dir_all(&elsewhere).expect("the real directory");
        std::os::unix::fs::symlink(&elsewhere, &clone).expect("a symlinked clone");
        world.record("r-main-aa", "main", &clone);
        world.devpod.knows("r-main-aa");
        let clones = clones_for(&world.repos_dir, &world.devpod);
        let updater = SelfInvocation::new("dl");
        let cache_path = fresh_cache(world.tmp());
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&updater, &cache_path);
        let mut notices = Vec::new();

        let outcome = workspace_delete(
            &mut context,
            &mut refresh,
            &clones,
            &mut world.storage,
            None,
            "r-main-aa",
            Insistence::NotInsisted,
            &mut notices,
        )
        .expect("devpod ran");

        // The removal errored, so the clone outcome is the refusal itself — its own
        // channel now, not a `Removed::Nothing` that could not tell an error from a
        // no-op.
        assert!(
            matches!(
                &outcome,
                DeleteOutcome::Deleted {
                    clone: Err(RemoveWorkspaceError::DirectoryLeft(
                        RemoveTreeError::RootIsSymlink { .. }
                    )),
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(
            notices,
            vec![LifecycleNotice::CloneNotRemoved {
                workspace_id: "r-main-aa".to_owned(),
                refusal: RemoveWorkspaceError::DirectoryLeft(RemoveTreeError::RootIsSymlink {
                    path: clone,
                    points_at: Some(elsewhere),
                }),
            }]
        );
    }

    #[test]
    fn a_record_that_could_not_be_dropped_reports_the_step_that_refused() {
        // Python's line is `Could not drop the record for {path}: {e}`, and the
        // `{e}` is the binary's to write: a temp file that could not be made, a
        // lock that could not be taken and a rename that failed read differently
        // to whoever has to fix them, so the notice carries which one it was.
        let mut world = World::empty();
        let clone = world.repo_dir.join("r-main-aa");
        let record = world.record("r-main-aa", "main", &clone);
        let cache = world.cache.clone();
        let Some(_denied) = refusing_writes(&cache) else {
            // Root is refused by nothing, and a mode this filesystem ignores is
            // ordinary on bind and overlay mounts.
            return;
        };
        let mut notices = Vec::new();

        forget_clone(&mut world.storage, &record, &mut notices);

        assert!(
            matches!(
                notices.as_slice(),
                [LifecycleNotice::RecordNotDropped {
                    path,
                    refusal: metadata::MetadataError::CreateTemp { directory, .. },
                }] if path == &clone && directory == &cache
            ),
            "{notices:?}"
        );
    }

    // =======================================================================
    // delete: the volumes the workspace's devcontainer created
    // =======================================================================

    /// A world with a recorded clone for `r-main-aa`, ready to be deleted, plus a
    /// devpod home recording what devpod substituted into its devcontainer.
    ///
    /// The clone directory and the recorded `LocalWorkspaceFolder` are deliberately
    /// *different* leaves: the pixi volume is named after what devpod recorded, and
    /// a test that made them the same could not tell the two sources apart.
    struct Deleting {
        world: World,
        home: ScratchHome,
        updater: SelfInvocation,
        cache_path: PathBuf,
    }

    fn a_world_ready_to_delete() -> Deleting {
        let mut world = World::empty();
        let clone = world.clone_at("r-main-aa", "main");
        world.record("r-main-aa", "main", &clone);
        world.devpod.knows("r-main-aa");
        let home = devpod_home_recording("r-main-aa", "/host/clones/opened-as", "dc9a8b7c");
        let cache_path = fresh_cache(world.tmp());
        Deleting {
            world,
            home,
            updater: SelfInvocation::new("dl"),
            cache_path,
        }
    }

    impl Deleting {
        /// Delete `r-main-aa`, collecting the notices it produced.
        fn delete(&mut self) -> (DeleteOutcome, Vec<LifecycleNotice>) {
            let clones = clones_for(&self.world.repos_dir, &self.world.devpod);
            let mut context = CommandContext::new(&self.world.devpod);
            let mut refresh = Refresh::new(&self.updater, &self.cache_path);
            let mut notices = Vec::new();
            let outcome = workspace_delete(
                &mut context,
                &mut refresh,
                &clones,
                &mut self.world.storage,
                Some(&self.home),
                "r-main-aa",
                Insistence::NotInsisted,
                &mut notices,
            )
            .expect("devpod ran");
            (outcome, notices)
        }
    }

    /// **The test that would have caught devlaunch#324.** Nothing in devlaunch ever
    /// ran a volume command, so every removal path left both of these behind — 39
    /// orphans and 37 GB on the machine the leak was measured on.
    #[test]
    fn a_delete_removes_both_volumes_the_workspaces_devcontainer_created() {
        let mut deleting = a_world_ready_to_delete();

        let (outcome, notices) = deleting.delete();

        assert_eq!(
            deleting.world.devpod.docker_argvs(),
            [[
                "volume",
                "rm",
                "--force",
                // `${localWorkspaceFolderBasename}-pixi`, from the basename devpod
                // recorded opening — not from the clone directory, which is
                // `r-main-aa`.
                "opened-as-pixi",
                // `dind-var-lib-docker-${devcontainerId}`, from the id devpod
                // recorded deriving.
                "dind-var-lib-docker-dc9a8b7c",
            ]]
        );
        assert_eq!(
            outcome,
            DeleteOutcome::Deleted {
                clone: Ok(Removed::Clone),
                volumes: VolumeSweep::Removed,
            }
        );
        // Silent: a removal that worked has nothing to tell anybody.
        assert!(
            !notices
                .iter()
                .any(|notice| matches!(notice, LifecycleNotice::VolumesNotRemoved { .. })),
            "{notices:?}"
        );
    }

    /// The order is the fix, not a detail: `devpod delete` takes devpod's record of
    /// the workspace away with the workspace, so the names have to be read first.
    #[test]
    fn the_volumes_are_removed_after_devpod_has_let_go_of_the_workspace() {
        let mut deleting = a_world_ready_to_delete();

        deleting.delete();

        let argvs: Vec<Vec<String>> = deleting
            .world
            .devpod
            .fake
            .calls()
            .into_iter()
            .filter(|call| !matches!(call, devlaunch_test_support::Call::Detach(_)))
            .map(|call| call.argv())
            .collect();
        assert_eq!(
            argvs,
            [
                vec!["devpod", "delete", "r-main-aa"],
                vec![
                    "docker",
                    "volume",
                    "rm",
                    "--force",
                    "opened-as-pixi",
                    "dind-var-lib-docker-dc9a8b7c",
                ],
            ]
        );
    }

    #[test]
    fn a_machine_with_no_docker_still_deletes_the_workspace_and_its_clone_cleanly() {
        // The whole reason the sweep is best-effort: nothing added here may fail on
        // a host that never had a volume to leak.
        let mut deleting = a_world_ready_to_delete();
        deleting.world.devpod.fake.script_missing("docker");
        let clone = deleting.world.repo_dir.join("r-main-aa");

        let (outcome, notices) = deleting.delete();

        assert_eq!(
            outcome,
            DeleteOutcome::Deleted {
                clone: Ok(Removed::Clone),
                volumes: VolumeSweep::NoDocker,
            }
        );
        assert!(!clone.exists(), "the clone went with the workspace");
        assert_eq!(deleting.world.branches_on_record(), Vec::<String>::new());
        // Not a word about docker: this machine never made these volumes. The two
        // lines it *does* say are the clone's, in the order they happened.
        assert_eq!(
            notices,
            vec![
                LifecycleNotice::Cache(CacheNotice::WorkspaceCloneRemoved { path: clone }),
                LifecycleNotice::CloneRemoved {
                    workspace_id: "r-main-aa".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn a_docker_that_would_not_remove_them_is_a_notice_and_not_a_failed_delete() {
        // A volume another container still holds is the case this exists for. The
        // workspace is gone regardless, so reporting failure would send the caller
        // looking for a workspace that is not there.
        let mut deleting = a_world_ready_to_delete();
        deleting.world.devpod.fake.script(
            ["docker", "volume", "rm"],
            Response::failed(1, "volume is in use\n"),
        );
        let refusal = VolumeRefusal::Docker {
            exit: Exit::Code(1),
            stderr: "volume is in use\n".to_owned(),
        };

        let (outcome, notices) = deleting.delete();

        assert_eq!(
            outcome,
            DeleteOutcome::Deleted {
                clone: Ok(Removed::Clone),
                volumes: VolumeSweep::Refused(refusal.clone()),
            }
        );
        // The refusal itself, not a rendering of it: docker's words are docker's and
        // the sentence around them is the binary's.
        assert!(
            notices.contains(&LifecycleNotice::VolumesNotRemoved {
                workspace_id: "r-main-aa".to_owned(),
                refusal,
            }),
            "{notices:?}"
        );
    }

    #[test]
    fn a_workspace_whose_up_never_finished_names_nothing_and_runs_no_docker() {
        // devpod writes its create result on the way *out* of a successful `up`, so
        // an `up` that died in its lifecycle hooks leaves the record with no result
        // beside it. Nothing to name is not the same as removing nothing, and it is
        // certainly not a docker call with a made-up name in it.
        let mut deleting = a_world_ready_to_delete();
        deleting.home = devpod_home_with(&[("default", "r-main-aa", None)]);

        let (outcome, notices) = deleting.delete();

        assert_eq!(
            outcome,
            DeleteOutcome::Deleted {
                clone: Ok(Removed::Clone),
                volumes: VolumeSweep::NothingNamed,
            }
        );
        assert_eq!(
            deleting.world.devpod.docker_argvs(),
            Vec::<Vec<String>>::new()
        );
        assert!(
            !notices
                .iter()
                .any(|notice| matches!(notice, LifecycleNotice::VolumesNotRemoved { .. })),
            "{notices:?}"
        );
    }

    /// One id under two contexts: devpod's ids are unique per context, so a record
    /// found twice cannot say which workspace's volumes these are — and guessing
    /// would remove the other one's. The ambiguity is the answer, as it is for
    /// [`create_record`].
    ///
    /// Both shapes, because the ambiguity is read off the record devpod writes on
    /// the way *in* and not off whose `up` finished. Keying it on the create result
    /// instead answers with the one context that completed — while `devpod delete`
    /// resolves the id against the *current* context, so the volumes named would be
    /// the other, living workspace's.
    #[test]
    fn one_id_in_two_contexts_names_nothing() {
        for second_up_finished in [false, true] {
            let home = devpod_home_recording("myws", "/host/clones/opened-as", "dc9a8b7c");
            let record = home.record("work", "myws");
            std::fs::create_dir_all(record.parent().expect("a parent"))
                .expect("a record directory");
            std::fs::write(&record, "{}").expect("a record");
            if second_up_finished {
                std::fs::write(home.result("work", "myws"), "{}").expect("a create result");
            }

            assert!(
                devcontainer_volumes(Some(&home), "myws").is_none(),
                "second context finished its up: {second_up_finished}"
            );
        }
    }

    /// Each name is built from its own recorded field, so a devpod that recorded
    /// one and not the other still gets the volume it can name. Both spellings are
    /// devpod's own, which is why they are written out here rather than assembled.
    #[test]
    fn each_recorded_substitution_names_its_own_volume() {
        let both = Substitutions {
            local_workspace_folder: Some("/host/clones/repo-branch-abcd".to_owned()),
            devcontainer_id: Some("f00d".to_owned()),
        };
        assert_eq!(
            both.volume_names(),
            ["repo-branch-abcd-pixi", "dind-var-lib-docker-f00d"]
        );

        let no_id = Substitutions {
            devcontainer_id: None,
            ..both
        };
        assert_eq!(no_id.volume_names(), ["repo-branch-abcd-pixi"]);

        // A blank field names nothing: `-pixi` and `dind-var-lib-docker-` are not
        // volumes anybody meant, and asking docker about them is asking about
        // somebody else's disk.
        let blank = parse_substitutions(
            serde_json::json!({
                "SubstitutionContext": { "LocalWorkspaceFolder": "", "DevContainerID": "" },
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(blank.volume_names(), Vec::<String>::new());
    }

    /// A create result devlaunch cannot read removes no volumes, and says nothing
    /// about it: that is exactly the behaviour of every build before this existed,
    /// and it is not worth a diagnostic on a delete that otherwise worked.
    #[test]
    fn a_create_result_that_is_not_the_expected_shape_names_nothing() {
        for bytes in [
            &b"not json at all"[..],
            b"[]",
            br#"{"SubstitutionContext": "a string"}"#,
            b"{}",
        ] {
            assert_eq!(
                parse_substitutions(bytes),
                Substitutions::default(),
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    // =======================================================================
    // the delete guard
    // =======================================================================

    fn losses_of(unsaved: &Unsaved) -> String {
        match unsaved {
            Unsaved::WouldLose(losses) => losses.describe(),
            other => panic!("expected losses, got {other:?}"),
        }
    }

    #[test]
    fn nothing_to_lose_is_the_only_answer_that_is_permission() {
        assert_eq!(
            guard_removal("ws", Unsaved::NothingToLose, Insistence::NotInsisted),
            Guarded::MayRemove
        );
    }

    #[test]
    fn work_saved_nowhere_else_stops_the_delete_and_names_what_it_is() {
        let losses = Losses::one(workspace_state::Loss::Unpushed(NonEmpty::one(
            "abc1234 later".to_owned(),
        )));
        let guarded = guard_removal(
            "ws",
            Unsaved::WouldLose(losses.clone()),
            Insistence::NotInsisted,
        );
        assert_eq!(
            guarded,
            Guarded::Refused(RemovalRefused::WouldLose {
                workspace_id: "ws".to_owned(),
                losses
            })
        );
    }

    #[test]
    fn an_answer_git_would_not_give_stops_the_delete_too() {
        // devlaunch#171: "could not tell" refuses exactly as "would lose" does. The
        // files are still on disk and nothing has established that they exist
        // anywhere else.
        let cause = CouldNotTell::GitCouldNotRead {
            clone: PathBuf::from("/x"),
            reason: "not a repository".to_owned(),
        };
        let guarded = guard_removal(
            "ws",
            Unsaved::CouldNotTell(cause.clone()),
            Insistence::NotInsisted,
        );
        assert_eq!(
            guarded,
            Guarded::Refused(RemovalRefused::CouldNotTell {
                workspace_id: "ws".to_owned(),
                cause
            })
        );
    }

    #[test]
    fn force_gets_past_both_refusals() {
        // The caller who means it is not blocked by dl declining to guess.
        for unsaved in [
            Unsaved::WouldLose(Losses::one(workspace_state::Loss::Uncommitted(
                NonEmpty::one("?? scratch.md".to_owned()),
            ))),
            Unsaved::CouldNotTell(CouldNotTell::DirectoryUnknown {
                workspace_id: "ws".to_owned(),
            }),
        ] {
            assert_eq!(
                guard_removal("ws", unsaved, Insistence::Insisted),
                Guarded::MayRemove
            );
        }
    }

    /// The guard's answer about `workspace_id`, over a real cache and real git.
    fn guard_reads(world: &World, workspace_id: &str) -> Unsaved {
        let clones = clones_for(&world.repos_dir, &world.devpod);
        let git = Git::new(&world.devpod);
        unsaved_work_in(
            &clones,
            &world.storage,
            &git,
            &world.cache,
            workspace_id,
            &mut ignoring(),
        )
    }

    #[test]
    fn a_clean_recorded_clone_has_nothing_to_lose() {
        let mut world = World::empty();
        let clone = world.clone_at("r-main-aa", "main");
        world.record("r-main-aa", "main", &clone);
        assert_eq!(guard_reads(&world, "r-main-aa"), Unsaved::NothingToLose);
    }

    #[test]
    fn an_unpushed_commit_in_a_recorded_clone_would_be_lost() {
        let mut world = World::empty();
        let clone = world.clone_at("r-main-aa", "main");
        world.record("r-main-aa", "main", &clone);
        std::fs::write(clone.join("more.txt"), "more\n").expect("a file");
        commit(&clone, "more");

        assert!(losses_of(&guard_reads(&world, "r-main-aa")).contains("unpushed commit"));
    }

    #[test]
    fn a_commit_on_a_branch_the_clone_is_not_on_would_be_lost_too() {
        // The reader half of #471: the widened probe has to reach the guard that
        // actually destroys things, not only the function that answers. An agent
        // that committed on a side branch and checked the main one back out leaves
        // exactly this clone, and `rm` used to take it without asking.
        let mut world = World::empty();
        let clone = world.clone_at("r-main-aa", "main");
        world.record("r-main-aa", "main", &clone);
        run_git(&clone, &["checkout", "-b", "wip"]);
        std::fs::write(clone.join("wip.txt"), "an hour of work\n").expect("a file");
        commit(&clone, "wip");
        run_git(&clone, &["checkout", "main"]);

        assert!(losses_of(&guard_reads(&world, "r-main-aa")).contains("unpushed commit"));
    }

    #[test]
    fn a_clone_dl_has_no_record_for_answers_nothing_to_lose() {
        // What the guard does *not* cover, pinned so no README can overstate it. A
        // clone under dl's cache with no metadata record: `--ls --json` reports what
        // it holds, but `rm` does not refuse, because "dl has no record of a clone
        // here" is `NothingToLose`. No work is destroyed — the delete reads the same
        // absent record and removes nothing — but it is not a refusal either.
        let world = World::empty();
        let clone = world.clone_at("r-main-aa", "main");
        std::fs::write(clone.join("an-hour-of-work.md"), "half a plan\n").expect("their work");

        assert_eq!(guard_reads(&world, "r-main-aa"), Unsaved::NothingToLose);
        assert!(clone.join("an-hour-of-work.md").exists());
    }

    #[test]
    fn a_stale_record_does_not_let_the_delete_past_the_guard() {
        // devlaunch#174, at the surface it destroys things from. The guard read the
        // recorded path; the delete fell back to the derived one when that path was
        // not on disk. So a record pointing somewhere stale had the guard answering
        // `NothingToLose` about an absent directory — correctly, nothing absent holds
        // anything — while the delete removed the derived directory, which held an
        // unpushed commit. Exit 0, no `--force`, nothing logged.
        //
        // The only assertion that pins it is one where those two would differ.
        let mut world = World::empty();
        let derived_id = WorkspaceId::new(OWNER, REPO, "feature")
            .expect("a safe triple")
            .value();
        let derived = world.clone_at(&derived_id, "feature");
        std::fs::write(derived.join("more.txt"), "more\n").expect("a file");
        commit(&derived, "more");
        let stale = world.repo_dir.join("moved-away");
        assert!(!stale.exists(), "the premise is that the record is stale");
        world.record("r-feature-aaa", "feature", &stale);

        assert!(
            losses_of(&guard_reads(&world, "r-feature-aaa")).contains("unpushed commit"),
            "the guard has to inspect the directory the delete will remove"
        );
    }

    #[test]
    fn a_record_no_directory_can_be_derived_from_stops_the_delete() {
        // A record holding a ref the id validator refuses — a hand-edited or
        // truncated metadata.json — resolves to no directory at all. That is not
        // `NothingToLose`: dl has established nothing about it, which is
        // devlaunch#171's rule one layer further out.
        let mut world = World::empty();
        let stale = world.repo_dir.join("not-on-disk");
        world.record("r-evil-aaa", "--evil", &stale);

        let unsaved = guard_reads(&world, "r-evil-aaa");

        let Unsaved::CouldNotTell(cause) = &unsaved else {
            panic!("expected a refusal, got {unsaved:?}");
        };
        assert!(
            cause.describe().contains("r-evil-aaa"),
            "{}",
            cause.describe()
        );
    }

    #[test]
    fn a_clone_git_cannot_read_stops_the_delete_too() {
        // devlaunch#171 itself. A directory that is there and is not a repository git
        // can read holds whatever files are in it, and with no repository to consult
        // nothing has established that they exist anywhere else.
        let mut world = World::empty();
        let broken = world.repo_dir.join("r-broken-aa");
        std::fs::create_dir_all(broken.join(".git")).expect("a directory that is not a clone");
        std::fs::write(broken.join("scratch.md"), "an agent's notes\n").expect("their work");
        world.record("r-broken-aa", "broken", &broken);

        let unsaved = guard_reads(&world, "r-broken-aa");

        assert!(matches!(unsaved, Unsaved::CouldNotTell(_)), "{unsaved:?}");
        assert!(broken.join("scratch.md").exists());
    }

    #[test]
    fn a_clone_already_removed_by_hand_is_still_deletable() {
        // The reason the "not there" arm answers `NothingToLose` rather than
        // refusing: clearing up after a half-finished delete must not need `--force`.
        let mut world = World::empty();
        let clone = world.clone_at("r-main-aa", "main");
        world.record("r-main-aa", "main", &clone);
        std::fs::remove_dir_all(&clone).expect("removed by hand");

        assert_eq!(guard_reads(&world, "r-main-aa"), Unsaved::NothingToLose);
    }

    // =======================================================================
    // the detached refresh child
    // =======================================================================

    #[test]
    fn a_fresh_cache_costs_no_subprocess() {
        let dir = temp_dir();
        let cache = fresh_cache(dir.path());
        let devpod = Devpod::new();
        let updater = SelfInvocation::new("dl");
        let mut refresh = Refresh::new(&updater, &cache);

        assert_eq!(
            refresh.ask(&devpod, RefreshReason::IfStale),
            RefreshSpawn::CacheStillFresh
        );
        assert_eq!(devpod.detached(), Vec::<Vec<String>>::new());
        assert!(!refresh.spawned());
    }

    #[test]
    fn a_stale_cache_spawns_one_refresh_with_the_update_cache_flag() {
        let dir = temp_dir();
        let cache = stale_cache(dir.path());
        let devpod = Devpod::new();
        let updater = SelfInvocation::new("dl");
        let mut refresh = Refresh::new(&updater, &cache);

        assert!(matches!(
            refresh.ask(&devpod, RefreshReason::IfStale),
            RefreshSpawn::Spawned { .. }
        ));
        assert_eq!(devpod.detached(), [vec!["dl", "--update-cache"]]);
    }

    #[test]
    fn a_cache_that_is_not_there_at_all_spawns_a_refresh() {
        let dir = temp_dir();
        let devpod = Devpod::new();
        let updater = SelfInvocation::new("dl");
        let never_written = dir.path().join("never-written.json");
        let mut refresh = Refresh::new(&updater, &never_written);

        assert!(matches!(
            refresh.ask(&devpod, RefreshReason::IfStale),
            RefreshSpawn::Spawned { .. }
        ));
    }

    #[test]
    fn a_forced_refresh_ignores_the_ttl_and_tells_the_child_so() {
        let dir = temp_dir();
        let cache = fresh_cache(dir.path());
        let devpod = Devpod::new();
        let updater = SelfInvocation::new("dl");
        let mut refresh = Refresh::new(&updater, &cache);

        refresh.ask(&devpod, RefreshReason::Forced);

        assert_eq!(devpod.detached(), [vec!["dl", "--update-cache", "--force"]]);
    }

    #[test]
    fn only_one_refresh_is_spawned_per_command() {
        let dir = temp_dir();
        let cache = stale_cache(dir.path());
        let devpod = Devpod::new();
        let updater = SelfInvocation::new("dl");
        let mut refresh = Refresh::new(&updater, &cache);

        refresh.ask(&devpod, RefreshReason::IfStale);
        assert_eq!(
            refresh.ask(&devpod, RefreshReason::IfStale),
            RefreshSpawn::AlreadySpawned
        );
        assert_eq!(
            refresh.ask(&devpod, RefreshReason::Forced),
            RefreshSpawn::AlreadySpawned
        );
        assert_eq!(devpod.detached().len(), 1);
    }

    #[test]
    fn skipping_on_freshness_does_not_use_up_the_one_spawn() {
        // A TTL skip means "not needed yet", not "already done".
        let dir = temp_dir();
        let cache = fresh_cache(dir.path());
        let devpod = Devpod::new();
        let updater = SelfInvocation::new("dl");
        let mut refresh = Refresh::new(&updater, &cache);

        refresh.ask(&devpod, RefreshReason::IfStale);
        refresh.ask(&devpod, RefreshReason::Forced);

        assert_eq!(devpod.detached().len(), 1);
    }

    #[test]
    fn two_commands_do_not_share_one_refresh_latch() {
        // Python held it in a module-level dict and reset it in `main()`; a second
        // command is a second value here.
        let dir = temp_dir();
        let cache = fresh_cache(dir.path());
        let devpod = Devpod::new();
        let updater = SelfInvocation::new("dl");

        Refresh::new(&updater, &cache).ask(&devpod, RefreshReason::Forced);
        Refresh::new(&updater, &cache).ask(&devpod, RefreshReason::Forced);

        assert_eq!(devpod.detached().len(), 2);
    }

    #[test]
    fn a_spawn_the_os_refuses_is_survivable_and_not_retried() {
        let dir = temp_dir();
        let cache = stale_cache(dir.path());
        let devpod = Devpod::new();
        devpod.fake.script_missing("dl");
        let updater = SelfInvocation::new("dl");
        let mut refresh = Refresh::new(&updater, &cache);

        assert_eq!(
            refresh.ask(&devpod, RefreshReason::IfStale),
            RefreshSpawn::NotStarted(SpawnRefused::ProgramNotFound)
        );
        assert_eq!(
            refresh.ask(&devpod, RefreshReason::Forced),
            RefreshSpawn::AlreadySpawned,
            "whatever refused the fork will refuse the next one too"
        );
    }

    #[test]
    fn the_child_argv_is_whatever_the_binary_says_it_is() {
        // Core never asks the OS who it is: `current_exe()` inside a library answers
        // `wf` when wf links it. Python's build spells its own re-invocation
        // `[sys.executable, "-m", "devlaunch.dl", "--update-cache"]`, which the
        // leading arguments are for.
        let python =
            SelfInvocation::new("/usr/bin/python3").with_leading_args(["-m", "devlaunch.dl"]);
        assert_eq!(
            python.refresh_child(RefreshReason::IfStale).argv(),
            ["/usr/bin/python3", "-m", "devlaunch.dl", "--update-cache"]
        );
        assert_eq!(
            python.refresh_child(RefreshReason::Forced).argv(),
            [
                "/usr/bin/python3",
                "-m",
                "devlaunch.dl",
                "--update-cache",
                "--force"
            ]
        );
    }

    #[test]
    fn the_refresh_child_rechecks_freshness_for_itself() {
        // Two parents can both see a stale cache before either child has written one,
        // and the second sweep would be pure waste.
        let dir = temp_dir();
        let fresh = fresh_cache(dir.path());
        assert_eq!(
            child_work(&fresh, RefreshReason::IfStale),
            ChildWork::NothingToDo
        );
        assert_eq!(
            child_work(&fresh, RefreshReason::Forced),
            ChildWork::RefreshAndSweep,
            "a forced refresh follows a workspace change: age says nothing about it"
        );
        let stale = stale_cache(dir.path());
        assert_eq!(
            child_work(&stale, RefreshReason::IfStale),
            ChildWork::RefreshAndSweep
        );
    }

    // =======================================================================
    // the fetch sweep
    // =======================================================================

    /// `git fetch origin +refs/heads/*:refs/heads/* --tags --prune` — the broad
    /// sweep, and the sweep's alone.
    const BROAD_FETCH: [&str; 6] = [
        "git",
        "fetch",
        "origin",
        "+refs/heads/*:refs/heads/*",
        "--tags",
        "--prune",
    ];

    fn hours_ago(hours: i64) -> Timestamp {
        let now = jiff::Zoned::now();
        let then = now
            .checked_sub(jiff::Span::new().hours(hours))
            .expect("a time two hours ago");
        Timestamp::from_civil(then.datetime())
    }

    /// A world whose git calls are faked, so a fetch's argv is observable without a
    /// remote to reach.
    struct Sweeping {
        /// Held for its `Drop`: the whole world below lives under it.
        _dir: tempfile::TempDir,
        repos_dir: PathBuf,
        bare: PathBuf,
        storage: MetadataStorage,
        fake: FakeRunner,
    }

    fn a_sweeping_cache() -> Sweeping {
        let dir = temp_dir();
        let repos_dir = dir.path().join("repos");
        let bare = bare_dir(&repos_dir, OWNER, REPO);
        std::fs::create_dir_all(&bare).expect("the bare directory");
        std::fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").expect("a HEAD");
        let (storage, _) =
            MetadataStorage::open(dir.path().join("metadata.json")).expect("a metadata store");
        Sweeping {
            _dir: dir,
            repos_dir,
            bare,
            storage,
            fake: FakeRunner::new(),
        }
    }

    impl Sweeping {
        fn recorded(
            &mut self,
            owner: &str,
            repo: &str,
            last_fetched: Option<Timestamp>,
        ) -> PathBuf {
            let bare = bare_dir(&self.repos_dir, owner, repo);
            std::fs::create_dir_all(&bare).expect("the bare directory");
            std::fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").expect("a HEAD");
            let mut recorded = BaseRepository::new(
                owner,
                repo,
                &format!("https://github.com/{owner}/{repo}.git"),
                bare.clone(),
            );
            recorded.last_fetched = last_fetched;
            self.storage.add_repository(recorded).expect("recorded");
            bare
        }

        fn last_fetched(&self, owner: &str, repo: &str) -> Option<Timestamp> {
            self.storage
                .get_repository(owner, repo)
                .expect("a record")
                .last_fetched
                .clone()
        }

        fn fetches(&self) -> Vec<devlaunch_test_support::Call> {
            self.fake
                .calls()
                .into_iter()
                .filter(|call| call.args().first().map(String::as_str) == Some("fetch"))
                .collect()
        }
    }

    #[test]
    fn a_repository_past_its_interval_gets_the_broad_fetch_under_a_deadline() {
        let mut cache = a_sweeping_cache();
        cache.recorded(OWNER, REPO, Some(hours_ago(2)));
        let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

        let report = sweep_repo_fetches(&manager, &mut cache.storage);

        assert_eq!(
            report.repos,
            [SweptRepo::Fetched {
                owner: OWNER.to_owned(),
                repo: REPO.to_owned()
            }]
        );
        let fetches = cache.fetches();
        assert_eq!(fetches.len(), 1, "{fetches:?}");
        let devlaunch_test_support::Call::Capture(spec) = &fetches[0] else {
            panic!("a fetch is captured, not inherited: {:?}", fetches[0]);
        };
        assert_eq!(spec.invocation.argv(), BROAD_FETCH);
        assert_eq!(spec.invocation.cwd.as_deref(), Some(cache.bare.as_path()));
        assert_eq!(
            spec.timeout,
            Some(BACKGROUND_FETCH_TIMEOUT),
            "the fetch the sweep runs under the lock is given a deadline"
        );
    }

    #[test]
    fn fetching_advances_the_shared_fetch_clock() {
        // `last_fetched` is shared with the launch path, so a sweep is what stops a
        // launch reaching for the same fetch a second time.
        let mut cache = a_sweeping_cache();
        let stale = hours_ago(2);
        cache.recorded(OWNER, REPO, Some(stale.clone()));
        let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

        sweep_repo_fetches(&manager, &mut cache.storage);

        assert_ne!(cache.last_fetched(OWNER, REPO), Some(stale));
    }

    #[test]
    fn a_repository_within_its_interval_is_left_alone() {
        // The interval is the whole point: this is not a fetch-every-command.
        let mut cache = a_sweeping_cache();
        cache.recorded(OWNER, REPO, Some(Timestamp::now()));
        let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

        let report = sweep_repo_fetches(&manager, &mut cache.storage);

        assert_eq!(
            report.repos,
            [SweptRepo::NotDue {
                owner: OWNER.to_owned(),
                repo: REPO.to_owned()
            }]
        );
        assert_eq!(cache.fetches().len(), 0);
    }

    #[test]
    fn a_repository_another_run_is_holding_is_skipped_rather_than_queued_for() {
        // A launch holds the repo lock while it clones. The sweep must neither wait
        // for it nor fetch behind its back — it comes back next hour.
        let mut cache = a_sweeping_cache();
        let stale = hours_ago(2);
        cache.recorded(OWNER, REPO, Some(stale.clone()));
        let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));
        let lock_path = manager.lock_path(OWNER, REPO);
        let held = locks::hold_lock(&lock_path).expect("the lock");

        let report = sweep_repo_fetches(&manager, &mut cache.storage);
        drop(held);

        assert_eq!(
            report.repos,
            [SweptRepo::Contended {
                owner: OWNER.to_owned(),
                repo: REPO.to_owned()
            }]
        );
        assert_eq!(cache.fetches().len(), 0);
        assert_eq!(
            cache.last_fetched(OWNER, REPO),
            Some(stale),
            "nothing was fetched, so nothing may claim it was"
        );
    }

    #[test]
    fn the_prune_scan_blocks_on_a_repository_another_process_is_holding() {
        // `dl --prune`'s scan takes each repo's lock, blocking, before it weighs
        // the clones under it (prune_plan → hold_repo_lock), so it never walks a
        // directory a cold launch is still cloning into. Unlike the hourly sweep —
        // which declines a held lock (`run_if_lock_free`) and comes back later —
        // the scan must WAIT, the way Python's
        // test_it_blocks_while_another_process_holds_the_repository_lock proves.
        // Pinned at the acquisition itself, against a real second process, because
        // driving prune_plan across a thread would carry the fake runner (which is
        // neither Send nor free of the process-global timing lock).
        use std::process::{Child, Command, Stdio};
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        const BOUND: Duration = Duration::from_secs(10);

        fn spawn_holder(lock: &Path) -> Child {
            Command::new("sh")
                .arg("-c")
                .arg(r#"exec 9>"$1"; flock --exclusive 9; exec sleep 300"#)
                .arg("sh")
                .arg(lock)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("a shell and flock(1) from util-linux")
        }

        fn wait_until(mut condition: impl FnMut() -> bool) {
            let deadline = Instant::now() + BOUND;
            while Instant::now() < deadline {
                if condition() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("the condition never held within {BOUND:?}");
        }

        let cache = a_sweeping_cache();
        let repos_dir = cache.repos_dir.clone();
        let lock_path = {
            let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));
            manager.lock_path(OWNER, REPO)
        };
        std::fs::create_dir_all(lock_path.parent().expect("a parent")).expect("the repo directory");

        let mut holder = spawn_holder(&lock_path);
        wait_until(|| {
            locks::run_if_lock_free(&lock_path, || ())
                .expect("no lock error")
                .is_none()
        });

        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            // A fresh manager over the same repos_dir; the acquisition is the scan's.
            let runner = ProcessRunner::new();
            let manager = RepositoryManager::new(&repos_dir, Git::new(&runner));
            let _lock = manager.hold_repo_lock(OWNER, REPO).expect("acquired");
            tx.send(()).expect("the parent listens");
        });

        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "the scan acquired the repo lock while another process held it"
        );

        holder.kill().expect("kill the holder");
        holder.wait().expect("reap the holder");

        rx.recv_timeout(BOUND)
            .expect("the scan acquired the lock once it was free");
        worker.join().expect("the worker finished");
    }

    #[test]
    fn one_bad_repository_does_not_cost_the_next_one_its_refresh() {
        // A detached child has nobody to complain to, so it complains to nobody — and
        // a failure that ended the loop would give the first slow remote every other
        // repository's refresh.
        let mut cache = a_sweeping_cache();
        let stale = hours_ago(2);
        let first = cache.recorded(OWNER, "first", Some(stale.clone()));
        cache.recorded(OWNER, "second", Some(stale.clone()));
        cache.fake.script(
            ["git", "fetch"],
            Response::failed(128, "fatal: no route to host\n"),
        );
        let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

        let report = sweep_repo_fetches(&manager, &mut cache.storage);

        assert_eq!(
            cache.fetches().len(),
            2,
            "the second repository was still swept"
        );
        assert!(
            report
                .repos
                .iter()
                .all(|swept| matches!(swept, SweptRepo::Failed { .. })),
            "{:?}",
            report.repos
        );
        assert_eq!(
            cache.last_fetched(OWNER, "first"),
            Some(stale.clone()),
            "a fetch that failed must not claim the clock"
        );
        assert_eq!(cache.last_fetched(OWNER, "second"), Some(stale));
        assert!(first.exists(), "the clone is left where it is");
    }

    #[test]
    fn a_fetch_that_hits_its_deadline_is_one_more_thing_to_step_over() {
        let mut cache = a_sweeping_cache();
        cache.recorded(OWNER, "first", Some(hours_ago(2)));
        cache.recorded(OWNER, "second", Some(hours_ago(2)));
        cache.fake.script(["git", "fetch"], Response::TimedOut);
        let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

        let report = sweep_repo_fetches(&manager, &mut cache.storage);

        assert_eq!(cache.fetches().len(), 2);
        assert!(
            report.repos.iter().all(|swept| matches!(
                swept,
                SweptRepo::Failed {
                    error: LazyFetchError::Fetch(
                        crate::flows::repo_manager::FetchRepoError::TimedOut { .. }
                    ),
                    ..
                }
            )),
            "{:?}",
            report.repos
        );
    }

    #[test]
    fn a_cache_with_no_repositories_sweeps_nothing() {
        let mut cache = a_sweeping_cache();
        let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));
        let report = sweep_repo_fetches(&manager, &mut cache.storage);
        assert!(report.repos.is_empty());
        assert_eq!(cache.fetches().len(), 0);
    }

    // =======================================================================
    // which workspace a triple is (devlaunch#88, #145)
    // =======================================================================

    /// A devpod that knows exactly these workspaces, in these states.
    fn devpod_knowing(known: &[(&str, &str)]) -> FakeRunner {
        let fake = FakeRunner::new();
        for (id, state) in known {
            fake.script(
                ["devpod", "status", id],
                Response::stdout(format!("{{\"state\": \"{state}\"}}")),
            );
        }
        fake.script(
            ["devpod", "status"],
            Response::failed(1, "workspace not found\n"),
        );
        fake
    }

    #[test]
    fn a_workspace_devpod_knows_answers_from_the_derivation_and_reads_no_record() {
        // #145's promise: a launch of a workspace devpod already knows must not load
        // metadata.json. Here the closure is the record read, and it is never called.
        let devpod = devpod_knowing(&[("r-main-aaa", "Running")]);
        let mut asked = false;

        let resolved = resolve_known_workspace(
            &devpod,
            (OWNER, REPO, "main"),
            "r-main-aaa",
            || {
                asked = true;
                Some("something-else".to_owned())
            },
            &mut ignoring(),
        );

        assert_eq!(
            resolved,
            Ok(KnownWorkspace::Known {
                workspace_id: "r-main-aaa".to_owned(),
                state: ContainerState::Running
            })
        );
        assert!(!asked, "the warm path reads no metadata");
    }

    #[test]
    fn a_workspace_created_under_the_old_scheme_is_still_addressable() {
        // The regression PR #81 caused: the record was written by a dl whose
        // derivation produced a different id, and following the derivation reaches a
        // workspace devpod has never heard of.
        let devpod = devpod_knowing(&[("r-main-old", "Stopped")]);
        let mut notices = Vec::new();

        let resolved = resolve_known_workspace(
            &devpod,
            (OWNER, REPO, "main"),
            "r-main-new",
            || Some("r-main-old".to_owned()),
            &mut notices,
        );

        assert_eq!(
            resolved,
            Ok(KnownWorkspace::Known {
                workspace_id: "r-main-old".to_owned(),
                state: ContainerState::Stopped
            })
        );
        assert_eq!(
            notices,
            [LifecycleNotice::AddressingRecordedWorkspace {
                recorded: "r-main-old".to_owned(),
                derived: "r-main-new".to_owned(),
                owner: OWNER.to_owned(),
                repo: REPO.to_owned(),
                branch: "main".to_owned(),
            }]
        );
    }

    #[test]
    fn a_record_that_agrees_with_the_derivation_changes_nothing() {
        let devpod = devpod_knowing(&[]);
        let resolved = resolve_known_workspace(
            &devpod,
            (OWNER, REPO, "main"),
            "r-main-new",
            || Some("r-main-new".to_owned()),
            &mut ignoring(),
        );
        assert_eq!(
            resolved,
            Ok(KnownWorkspace::Unknown {
                derived: "r-main-new".to_owned()
            })
        );
    }

    #[test]
    fn a_stored_id_devpod_also_denies_is_not_used() {
        // metadata.json is append-mostly, so a record naming a workspace deleted
        // months ago is ordinary. The answer has to be the derived id — the one a
        // create would use — not a workspace that is doubly gone.
        let devpod = devpod_knowing(&[]);
        let resolved = resolve_known_workspace(
            &devpod,
            (OWNER, REPO, "main"),
            "r-main-new",
            || Some("r-main-old".to_owned()),
            &mut ignoring(),
        );
        assert_eq!(
            resolved,
            Ok(KnownWorkspace::Unknown {
                derived: "r-main-new".to_owned()
            })
        );
    }

    #[test]
    fn no_record_at_all_falls_back_to_the_derivation() {
        // Also the answer for a cache dl could not read: a lookup that failed must not
        // stop a command that would otherwise have worked.
        let devpod = devpod_knowing(&[]);
        let resolved = resolve_known_workspace(
            &devpod,
            (OWNER, REPO, "main"),
            "r-main-new",
            || None,
            &mut ignoring(),
        );
        let resolved = resolved.expect("devpod ran and denied it");
        assert_eq!(
            resolved,
            KnownWorkspace::Unknown {
                derived: "r-main-new".to_owned()
            }
        );
        assert!(resolved.state().is_none());
        assert_eq!(resolved.workspace_id(), "r-main-new");
        assert!(!resolved.is_running());
    }

    #[test]
    fn a_devpod_nobody_can_run_is_not_a_workspace_nobody_knows() {
        // Python's `get_workspace_state` folds a non-zero exit into `None` but
        // *raises* `DevpodNotInstalled`, so this is the point a devpod-less host's
        // command ends. Answering `Unknown` instead sends a launch down the cold
        // path, which clones a repository the host cannot open a container from
        // and leaves it and its record behind (dl/tests/launch.rs pins the
        // observable half).
        let missing = FakeRunner::new().with_missing("devpod");
        let mut asked = false;

        let resolved = resolve_known_workspace(
            &missing,
            (OWNER, REPO, "main"),
            "r-main-new",
            || {
                asked = true;
                Some("r-main-old".to_owned())
            },
            &mut ignoring(),
        );

        assert_eq!(resolved, Err(NotRun::NotInstalled));
        assert!(
            !asked,
            "a devpod that cannot be run makes the record irrelevant, so it is not read"
        );
    }

    #[test]
    fn a_devpod_that_refused_the_derived_id_is_a_denial_and_not_a_failure() {
        // The other side of the line above: devpod ran, said it has no such
        // workspace, and that is the cold path -- exit code and all.
        let devpod = devpod_knowing(&[]);
        let resolved = resolve_known_workspace(
            &devpod,
            (OWNER, REPO, "main"),
            "r-main-new",
            || None,
            &mut ignoring(),
        );
        assert_eq!(
            resolved,
            Ok(KnownWorkspace::Unknown {
                derived: "r-main-new".to_owned()
            })
        );
    }

    #[test]
    fn the_recorded_id_comes_off_the_record_for_the_triple() {
        let mut world = World::empty();
        let clone = world.clone_at("r-main-aa", "main");
        let mut record = world.record("r-main-aa", "main", &clone);
        assert_eq!(
            recorded_devpod_workspace_id(&world.storage, OWNER, REPO, "main"),
            None,
            "every record written before the field existed has it empty"
        );
        record.devpod_workspace_id = Some("r-main-old".to_owned());
        world.storage.add_worktree(record).expect("rewritten");
        assert_eq!(
            recorded_devpod_workspace_id(&world.storage, OWNER, REPO, "main"),
            Some("r-main-old".to_owned())
        );
        assert_eq!(
            recorded_devpod_workspace_id(&world.storage, OWNER, REPO, "other"),
            None
        );
    }

    #[test]
    fn asking_devpod_for_a_state_charges_the_devpod_up_stage() {
        // Python's `@timing.staged("devpod-up")`, which is what stops a warm attach
        // showing a gap where its one round trip was.
        let devpod = devpod_knowing(&[("ws", "Running")]);
        assert_eq!(workspace_state(&devpod, "ws"), Ok(ContainerState::Running));
        assert_eq!(
            devpod.args_to("devpod"),
            [["status", "ws", "--output", "json"]]
        );
    }

    // =======================================================================
    // where a workspace is on this disk
    // =======================================================================

    #[test]
    fn a_url_scheme_and_an_scp_remote_name_somewhere_else() {
        for remote in [
            "https://github.com/o/r.git",
            "http://github.com/o/r",
            "ssh://git@github.com/o/r",
            "git://example.com/o/r",
            "file:///srv/repos/o/r",
            "git@github.com:o/r.git",
            "user@host:path",
        ] {
            assert!(names_a_remote(remote), "{remote}");
        }
    }

    #[test]
    fn text_that_is_also_a_perfectly_good_relative_path_does_not() {
        // The two mistakes are not equal. A path read as a remote drops a directory
        // out of the referenced set, which is how `--prune` would come to call a live
        // clone unreferenced — wrong, and toward loss. So only the shapes that are
        // never also written as a directory count.
        for path in [
            "github.com/o/r",
            "./some-repo",
            "/srv/repos/o/r",
            "some-repo",
            "a/b://c",
            "./a@b:c",
            "b:c",
            "",
        ] {
            assert!(!names_a_remote(path), "{path}");
        }
    }

    #[test]
    fn a_git_source_carrying_a_path_still_places_itself() {
        // `devpod up <path-to-a-repo>` records the gitRepository arm with a path in
        // it, and a path this does not return is a directory `--prune` will call
        // unreferenced (devlaunch#224 is the other direction).
        assert_eq!(
            source_places(&WorkspaceSource::GitRepository("/srv/repos/o/r".to_owned())),
            SourcePlaces::Placeable(vec!["/srv/repos/o/r".to_owned()])
        );
        assert_eq!(
            source_places(&WorkspaceSource::GitRepository(
                "https://github.com/o/r.git".to_owned()
            )),
            SourcePlaces::Placeable(Vec::new())
        );
    }

    #[test]
    fn a_source_that_opens_no_folder_here_is_not_the_same_as_one_dl_cannot_read() {
        // Reading them alike is how a live workspace contributed no path *and* no
        // alarm while the command printed that it stops for exactly that.
        let image = one_workspace("ws", serde_json::json!({ "image": "ubuntu:24.04" }));
        assert_eq!(
            source_places(&image.source),
            SourcePlaces::Placeable(Vec::new())
        );
        let unreadable = one_workspace("ws", serde_json::json!({ "localFolder": 7 }));
        assert!(matches!(
            source_places(&unreadable.source),
            SourcePlaces::Unplaceable { .. }
        ));
    }

    #[test]
    fn where_a_source_sits_is_read_off_its_position_under_the_root() {
        let dir = temp_dir();
        let root = dir.path().join("repos");
        let clone = root.join("o").join("r").join("r-main-aa");
        std::fs::create_dir_all(clone.join(".git")).expect("a clone with a .git");

        assert_eq!(site_of(Path::new("/elsewhere"), &root), SourceSite::Outside);
        assert_eq!(site_of(&root, &root), SourceSite::TooShallow);
        assert_eq!(site_of(&root.join("o"), &root), SourceSite::TooShallow);
        assert_eq!(
            site_of(&root.join("o").join("r"), &root),
            SourceSite::InARepositoryOnly {
                owner: "o".to_owned(),
                repo: "r".to_owned()
            }
        );
        assert_eq!(
            site_of(&clone, &root),
            SourceSite::InAClone {
                clone: clone.clone()
            }
        );
        // `devpod up <clone>/subproject`: the clone is what answers for it.
        assert_eq!(
            site_of(&clone.join("subproject"), &root),
            SourceSite::InAClone {
                clone: clone.clone()
            }
        );
        // devlaunch#88's shape: a folder that is gone, or the config-only stub devpod
        // rebuilds from cache. Neither is a clone, so which clone of the repository
        // the workspace wants is unanswerable.
        assert_eq!(
            site_of(&root.join("o").join("r").join("old-leaf"), &root),
            SourceSite::InARepositoryOnly {
                owner: "o".to_owned(),
                repo: "r".to_owned()
            }
        );
    }

    #[test]
    fn a_clone_whose_git_is_a_file_is_still_a_clone() {
        // `git clone --separate-git-dir` is a layout git supports, and it leaves
        // `.git` as a file. Asking whether a *directory* is there would read it as a
        // place a clone used to be.
        let dir = temp_dir();
        let clone = dir.path().join("clone");
        std::fs::create_dir_all(&clone).expect("the clone");
        std::fs::write(clone.join(".git"), "gitdir: /elsewhere/r.git\n").expect("a gitfile");
        assert!(is_populated_clone(&clone));
    }

    #[test]
    fn a_path_that_is_not_there_still_canonicalises_as_far_as_it_goes() {
        // Which is the ordinary case here: on devlaunch#88's host most devpod records
        // named a folder that had been deleted.
        let dir = temp_dir();
        let real = std::fs::canonicalize(dir.path()).expect("a real temp directory");
        assert_eq!(
            canonical(&dir.path().join("gone").join("deeper").to_string_lossy()),
            Some(real.join("gone").join("deeper"))
        );
    }

    #[test]
    fn a_symlinked_cache_root_and_its_clones_resolve_to_the_same_place() {
        // A lexical comparison here says that *no* clone is referenced, which is a
        // total-loss bug in the one direction that cannot be undone.
        let dir = temp_dir();
        let real = dir.path().join("real");
        let clone = real.join("o").join("r").join("r-main-aa");
        std::fs::create_dir_all(&clone).expect("the clone");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("a symlink");

        assert_eq!(
            canonical(&link.join("o").join("r").join("r-main-aa").to_string_lossy()),
            canonical(&clone.to_string_lossy())
        );
    }

    #[test]
    fn text_no_filesystem_call_will_accept_cannot_be_followed() {
        assert_eq!(canonical(""), None);
        assert_eq!(canonical("has\0a\0nul"), None);
    }

    #[test]
    fn a_live_workspace_opening_part_of_a_clone_still_holds_the_clone() {
        // `devpod up <clone>/subproject` records the subdirectory, and deleting the
        // clone takes the workspace with it. Equality answered no and deleted the
        // parent.
        let dir = temp_dir();
        let root = std::fs::canonicalize(dir.path()).expect("a real directory");
        let clone = root.join("o").join("r").join("r-main-aa");
        std::fs::create_dir_all(clone.join(".git")).expect("a clone");
        std::fs::create_dir_all(clone.join("subproject")).expect("a subproject");
        let workspaces = vec![one_workspace(
            "ws",
            serde_json::json!({ "localFolder": clone.join("subproject").display().to_string() }),
        )];

        let locations = workspace_locations(&workspaces, &root);

        assert_eq!(locations.holder(&clone), Some("ws"));
        // A lexical prefix test would answer yes here, and this is not under it.
        let sibling = root.join("o").join("r").join("r-main-aa-scratch");
        assert_eq!(locations.holder(&sibling), None);
    }

    #[test]
    fn a_source_that_cannot_be_followed_is_named_rather_than_dropped() {
        let dir = temp_dir();
        let root = std::fs::canonicalize(dir.path()).expect("a real directory");
        let workspaces = vec![
            one_workspace("unreadable", serde_json::json!({ "localFolder": [] })),
            one_workspace("nul", serde_json::json!({ "localFolder": "has\0a\0nul" })),
            one_workspace(
                "shallow",
                serde_json::json!({ "localFolder": root.display().to_string() }),
            ),
        ];

        let locations = workspace_locations(&workspaces, &root);
        let unlocatable = locations.unlocatable().expect("three of them");

        assert_eq!(
            unlocatable
                .iter()
                .map(|it| it.workspace_id.clone())
                .collect::<Vec<_>>(),
            ["unreadable", "nul", "shallow"]
        );
    }

    // =======================================================================
    // prune: which clone directories go
    // =======================================================================

    /// A cache holding one clone directory of every kind the classification has an
    /// arm for, all of them real repositories.
    ///
    /// `referenced` is sourced by a live workspace. `orphan-clean` is sourced by
    /// nobody and holds nothing unpushed. `orphan-dirty` is sourced by nobody and
    /// holds both an unpushed commit and an uncommitted file — 13 of the reference
    /// host's 37 stale clones were in that state, two of them with real work in them.
    /// `disputed` has a metadata record naming a workspace devpod still lists but
    /// sources somewhere else entirely, which is devlaunch#88's shape.
    struct Four {
        world: World,
        referenced: PathBuf,
        orphan_clean: PathBuf,
        orphan_dirty: PathBuf,
        disputed: PathBuf,
    }

    fn four_clones() -> Four {
        let mut world = World::empty();
        let mut made = Vec::new();
        for (leaf, branch) in [
            ("referenced", "ref"),
            ("orphan-clean", "clean"),
            ("orphan-dirty", "dirty"),
            ("disputed", "disp"),
        ] {
            let clone = world.clone_at(leaf, branch);
            world.record(leaf, branch, &clone);
            made.push(clone);
        }
        let dirty = made[2].clone();
        std::fs::write(dirty.join("later.txt"), "later\n").expect("a file");
        commit(&dirty, "later"); // committed, never pushed
        std::fs::write(dirty.join("scratch.md"), "an agent's notes\n").expect("never even added");

        world.devpod.lists(&[
            listed("referenced", &made[0]),
            listed("disputed", &world.tmp().join("somewhere").join("else")),
        ]);
        Four {
            referenced: made[0].clone(),
            orphan_clean: made[1].clone(),
            orphan_dirty: made[2].clone(),
            disputed: made[3].clone(),
            world,
        }
    }

    #[test]
    fn the_four_arms_are_classified_from_the_disk_and_from_devpods_own_listing() {
        let four = four_clones();
        let plan = plan_for(&four.world, Insistence::NotInsisted);

        assert_eq!(
            removing(&plan),
            [four.orphan_clean.as_path()],
            "only the clone nothing references and nothing would lose"
        );
        assert_eq!(
            kept_because(&plan, &four.referenced),
            KeptBecause::StillOpened {
                workspace_id: "referenced".to_owned()
            }
        );
        assert!(matches!(
            kept_because(&plan, &four.orphan_dirty),
            KeptBecause::Objected(Objection::WouldLose(_))
        ));
        assert!(matches!(
            kept_because(&plan, &four.disputed),
            KeptBecause::RecordsDisagree { .. }
        ));
    }

    #[test]
    fn an_orphan_whose_only_work_is_on_a_branch_it_is_not_on_is_kept() {
        // #471 at the reader that removes clones by the dozen rather than one at a
        // time, which is where the blindness cost the most: `orphan-clean` is the
        // arm this plan removes, and a commit sitting on a branch the clone is not
        // checked out on has to move it off that arm on its own.
        let four = four_clones();
        run_git(&four.orphan_clean, &["checkout", "-b", "wip"]);
        std::fs::write(four.orphan_clean.join("wip.txt"), "an hour of work\n").expect("a file");
        commit(&four.orphan_clean, "wip");
        run_git(&four.orphan_clean, &["checkout", "clean"]);

        let plan = plan_for(&four.world, Insistence::NotInsisted);

        assert_eq!(
            removing(&plan),
            [] as [&Path; 0],
            "nothing is safe to remove"
        );
        assert!(matches!(
            kept_because(&plan, &four.orphan_clean),
            KeptBecause::Objected(Objection::WouldLose(_))
        ));
    }

    #[test]
    fn the_bare_cache_is_never_a_candidate_and_never_reported() {
        // Nothing sources it and no record names it, so every rule would call it an
        // orphan — and it is the copy every clone hardlinks its git objects out of.
        let four = four_clones();
        let plan = plan_for(&four.world, Insistence::NotInsisted);
        let bare = canonical(&four.world.bare.to_string_lossy()).expect("the bare directory");
        assert!(!removing(&plan).contains(&bare));
        assert!(
            plan.keeping.iter().all(|kept| kept.path != bare),
            "not reported either: {:?}",
            plan.keeping
        );
    }

    #[test]
    fn force_promotes_the_clone_holding_work_and_nothing_else() {
        // `--force` is not a general override: Referenced and Disputed are devlaunch
        // saying the directory is still in use or that its own records disagree, and
        // there is nothing for a user to mean by insisting.
        let four = four_clones();
        let plan = plan_for(&four.world, Insistence::Insisted);

        let mut going = removing(&plan);
        going.sort();
        let mut expected = vec![four.orphan_clean.clone(), four.orphan_dirty.clone()];
        expected.sort();
        assert_eq!(going, expected);
        assert!(matches!(
            kept_because(&plan, &four.referenced),
            KeptBecause::StillOpened { .. }
        ));
        assert!(matches!(
            kept_because(&plan, &four.disputed),
            KeptBecause::RecordsDisagree { .. }
        ));
    }

    #[test]
    fn what_force_is_answering_rides_on_the_directory_it_answers_for() {
        // Without it the plan reads the same for a clone holding an afternoon's
        // uncommitted work as for an empty one, and the confirmation cannot say what
        // it costs.
        let four = four_clones();
        let plan = plan_for(&four.world, Insistence::Insisted);

        let promotions: Vec<(PathBuf, Promotion)> = plan
            .removing
            .iter()
            .map(|it| (it.path.clone(), it.promotion.clone()))
            .collect();
        let clean = promotions
            .iter()
            .find(|(path, _)| *path == four.orphan_clean)
            .expect("the clean orphan");
        assert_eq!(clean.1, Promotion::Unopposed);
        let dirty = promotions
            .iter()
            .find(|(path, _)| *path == four.orphan_dirty)
            .expect("the dirty orphan");
        assert!(matches!(
            dirty.1,
            Promotion::Insisted {
                despite: Objection::WouldLose(_)
            }
        ));
    }

    #[test]
    fn a_clone_git_will_not_answer_about_is_kept_with_nothing_typed() {
        // Since devlaunch#171 a directory git cannot read as a repository is a
        // `CouldNotTell` rather than "holds nothing", so it objects — and `--force` is
        // what removes it.
        let world = World::empty();
        let broken = world.repo_dir.join("was-never-a-clone");
        std::fs::create_dir_all(&broken).expect("a directory that is not a clone");
        std::fs::write(broken.join("something.txt"), "x\n").expect("a file in it");

        let kept = plan_for(&world, Insistence::NotInsisted);
        assert!(matches!(
            kept_because(&kept, &broken),
            KeptBecause::Objected(Objection::CouldNotTell(_))
        ));
        assert!(removing(&kept).is_empty());

        let forced = plan_for(&world, Insistence::Insisted);
        assert_eq!(removing(&forced), [broken]);
    }

    #[test]
    fn a_symlink_standing_where_a_clone_would_be_is_left_alone() {
        // Following one would put a candidate outside the cache entirely, and
        // unlinking the link instead would report a clone as reclaimed while it sat on
        // another volume.
        let world = World::empty();
        let outside = world.tmp().join("outside");
        std::fs::create_dir_all(&outside).expect("somebody else's directory");
        std::os::unix::fs::symlink(&outside, world.repo_dir.join("a-link")).expect("a symlink");

        let plan = plan_for(&world, Insistence::Insisted);

        assert!(removing(&plan).is_empty());
        assert!(plan.keeping.is_empty(), "{:?}", plan.keeping);
        assert!(outside.exists());
    }

    #[test]
    fn a_machine_with_no_clone_directories_has_nothing_to_prune() {
        let dir = temp_dir();
        let devpod = Devpod::new();
        devpod.lists(&[]);
        let clones = WorkspaceCloneManager::new(
            dir.path().join("repos"),
            Duration::from_secs(3600),
            Git::new(&devpod),
            GitLfs::NotInstalled,
        );
        let (storage, _) =
            MetadataStorage::open(dir.path().join("metadata.json")).expect("a store");
        let mut context = CommandContext::new(&devpod);
        let workspaces = context.workspaces().expect("a listing");
        let placement = ClonePlacement::resolve(&clones, &workspaces);

        let plan = prune_plan(
            &clones,
            &storage,
            &workspaces,
            &placement,
            Insistence::NotInsisted,
            &mut ignoring(),
        )
        .expect("a plan");

        assert!(plan.nothing_to_do());
    }

    #[test]
    fn a_workspace_devpod_records_at_a_stub_disputes_that_repositorys_clones() {
        // devlaunch#88's shape, and the reason `--prune` does not wait on it: read as
        // an orphan the healthy clone would be deleted, read as referenced it would
        // silently hide disk.
        let mut world = World::empty();
        let clone = world.clone_at("r-clean-aa", "clean");
        world.record("r-clean-aa", "clean", &clone);
        let stub = world.repo_dir.join("old-leaf");
        std::fs::create_dir_all(&stub).expect("a config-only stub with no .git");
        world.devpod.lists(&[listed("stale", &stub)]);

        let plan = plan_for(&world, Insistence::Insisted);

        assert!(removing(&plan).is_empty(), "{:?}", removing(&plan));
        assert!(matches!(
            kept_because(&plan, &clone),
            KeptBecause::RecordsDisagree { workspace_id, .. } if workspace_id == "stale"
        ));
    }

    #[test]
    fn only_that_repositorys_clones_are_disputed() {
        // What keeps this command usable on devlaunch#88's host rather than merely
        // safe on it.
        let mut world = World::empty();
        let clone = world.clone_at("r-clean-aa", "clean");
        world.record("r-clean-aa", "clean", &clone);
        let other_repo = world.repos_dir.join(OWNER).join("other");
        std::fs::create_dir_all(&other_repo).expect("a second repository");
        let elsewhere = other_repo.join("other-clean-aa");
        std::fs::create_dir_all(elsewhere.join(".git")).expect("a clone of the other repository");
        let stub = world.repo_dir.join("old-leaf");
        std::fs::create_dir_all(&stub).expect("a stub in the first repository");
        world.devpod.lists(&[listed("stale", &stub)]);

        let plan = plan_for(&world, Insistence::Insisted);

        assert_eq!(
            removing(&plan),
            [elsewhere],
            "the other repository is still prunable"
        );
    }

    #[test]
    fn a_source_that_cannot_be_followed_stops_the_whole_command() {
        // While one exists there is no directory this command can honestly call
        // unreferenced.
        let mut world = World::empty();
        let clone = world.clone_at("r-clean-aa", "clean");
        world.record("r-clean-aa", "clean", &clone);
        world.devpod.lists(&[listed_with(
            "unreadable",
            serde_json::json!({ "localFolder": 7 }),
        )]);
        let clones = clones_for(&world.repos_dir, &world.devpod);
        let root = clone_root(&clones);
        let mut context = CommandContext::new(&world.devpod);
        let workspaces = context.workspaces().expect("a listing");

        let locations = workspace_locations(&workspaces, &root);

        let unlocatable = locations.unlocatable().expect("one of them");
        assert_eq!(unlocatable.len(), 1);
        assert_eq!(
            unlocatable.iter().next().expect("it").workspace_id,
            "unreadable"
        );
    }

    #[test]
    fn a_source_that_is_simply_gone_does_not_stop_the_command() {
        let world = World::empty();
        world
            .devpod
            .lists(&[listed("stale", &world.tmp().join("deleted-long-ago"))]);
        let clones = clones_for(&world.repos_dir, &world.devpod);
        let root = clone_root(&clones);
        let mut context = CommandContext::new(&world.devpod);
        let workspaces = context.workspaces().expect("a listing");

        assert!(
            workspace_locations(&workspaces, &root)
                .unlocatable()
                .is_none()
        );
    }

    #[test]
    fn the_biggest_reclaim_is_reported_first() {
        let world = World::empty();
        let small = world.clone_at("small", "small");
        let big = world.clone_at("big", "big");
        // Two megabytes nothing else links to, so the reclaimed figure is a number a
        // test can name. Excluded, because an untracked file is unsaved work and the
        // clone would be kept rather than reclaimed.
        std::fs::write(
            big.join(".git").join("info").join("exclude"),
            "payload.bin\n",
        )
        .expect("an exclude file");
        std::fs::write(big.join("payload.bin"), vec![0u8; 2 * 1024 * 1024]).expect("a payload");

        let plan = plan_for(&world, Insistence::NotInsisted);

        assert_eq!(removing(&plan), [big, small]);
        assert!(plan.freed().known_bytes() > 2 * 1024 * 1024);
    }

    #[test]
    fn a_record_for_a_directory_that_is_already_gone_is_dropped() {
        let mut world = World::empty();
        let gone = world.repo_dir.join("r-gone-aaa");
        world.record("r-gone-aaa", "gone", &gone);

        let plan = plan_for(&world, Insistence::NotInsisted);

        assert_eq!(
            plan.stale_records
                .iter()
                .map(|it| it.branch.clone())
                .collect::<Vec<_>>(),
            ["gone"]
        );
        assert!(
            !plan.nothing_to_do(),
            "a run whose only work is the records"
        );
    }

    #[test]
    fn a_record_whose_directory_cannot_be_looked_at_is_kept() {
        // "dl could not look" is not "this is not there", and only the second is a
        // reason to forget a record — it is the only note of where a clone lives.
        let mut world = World::empty();
        let hidden = world.repo_dir.join("hidden");
        std::fs::create_dir_all(hidden.join("r-main-aa")).expect("a clone inside");
        world.record("r-main-aa", "main", &hidden.join("r-main-aa"));
        let Some(_sealed) = refusing_reads(&hidden) else {
            return;
        };

        let plan = plan_for(&world, Insistence::NotInsisted);

        assert!(plan.stale_records.is_empty(), "{:?}", plan.stale_records);
    }

    #[test]
    fn the_acting_pass_removes_the_directories_and_forgets_their_records() {
        let mut four = four_clones();
        let plan = plan_for(&four.world, Insistence::NotInsisted);
        let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
        let mut context = CommandContext::new(&four.world.devpod);

        let outcome = prune_clones(
            &mut context,
            &clones,
            &mut four.world.storage,
            &plan,
            &mut ignoring(),
        )
        .expect("the pass ran");

        let PruneOutcome::Acted(report) = &outcome else {
            panic!("expected the pass to act, got {outcome:?}");
        };
        assert_eq!(
            report
                .removed
                .iter()
                .map(|it| it.path.clone())
                .collect::<Vec<_>>(),
            [four.orphan_clean.clone()]
        );
        assert!(report.finished());
        assert!(!four.orphan_clean.exists());
        assert!(four.referenced.exists());
        assert!(four.orphan_dirty.exists());
        assert!(four.disputed.exists());
        assert_eq!(four.world.branches_on_record(), ["dirty", "disp", "ref"]);
    }

    #[test]
    fn work_written_while_the_question_was_open_is_not_destroyed() {
        // The report a user answered was taken before they answered it.
        let mut four = four_clones();
        let plan = plan_for(&four.world, Insistence::NotInsisted);
        assert_eq!(removing(&plan), [four.orphan_clean.clone()]);
        std::fs::write(four.orphan_clean.join("just-typed.md"), "half a plan\n")
            .expect("work written while the question was open");
        let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
        let mut context = CommandContext::new(&four.world.devpod);

        let outcome = prune_clones(
            &mut context,
            &clones,
            &mut four.world.storage,
            &plan,
            &mut ignoring(),
        )
        .expect("the pass ran");

        let PruneOutcome::Acted(report) = &outcome else {
            panic!("expected the pass to act, got {outcome:?}");
        };
        assert!(report.removed.is_empty());
        assert_eq!(
            report
                .withheld
                .iter()
                .map(|it| it.path.clone())
                .collect::<Vec<_>>(),
            [four.orphan_clean.clone()]
        );
        assert!(four.orphan_clean.join("just-typed.md").exists());
    }

    #[test]
    fn a_clone_a_launch_registered_since_the_plan_is_not_removed_even_under_force() {
        // The clone path for `(owner, repo, branch)` is deterministic, so a concurrent
        // launch reuses the very directory in the plan. Re-asking only "has it grown
        // unsaved work" caught the other case and not this one, and the difference was
        // somebody's running workspace.
        let mut four = four_clones();
        let plan = plan_for(&four.world, Insistence::Insisted);
        assert!(removing(&plan).contains(&four.orphan_dirty));
        four.world.devpod.lists(&[
            listed("referenced", &four.referenced),
            listed("disputed", &four.world.tmp().join("somewhere").join("else")),
            listed("just-launched", &four.orphan_dirty),
        ]);
        let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
        let mut context = CommandContext::new(&four.world.devpod);

        let outcome = prune_clones(
            &mut context,
            &clones,
            &mut four.world.storage,
            &plan,
            &mut ignoring(),
        )
        .expect("the pass ran");

        let PruneOutcome::Acted(report) = &outcome else {
            panic!("expected the pass to act, got {outcome:?}");
        };
        assert!(
            report.withheld.iter().any(|it| it.path == four.orphan_dirty
                && matches!(it.because, KeptBecause::StillOpened { .. })),
            "{:?}",
            report.withheld
        );
        assert!(four.orphan_dirty.exists());
    }

    #[test]
    fn a_directory_that_refuses_is_named_and_its_siblings_still_go() {
        let mut world = World::empty();
        let stuck = world.clone_at("r-stuck-aa", "stuck");
        let goes = world.clone_at("r-goes-aaa", "goes");
        let plan = plan_for(&world, Insistence::NotInsisted);
        assert_eq!(plan.removing.len(), 2);
        let Some(_sealed) = refusing_writes(&stuck) else {
            return;
        };
        let clones = clones_for(&world.repos_dir, &world.devpod);
        let mut context = CommandContext::new(&world.devpod);

        let outcome = prune_clones(
            &mut context,
            &clones,
            &mut world.storage,
            &plan,
            &mut ignoring(),
        )
        .expect("the pass ran");

        let PruneOutcome::Acted(report) = &outcome else {
            panic!("expected the pass to act, got {outcome:?}");
        };
        assert!(!report.finished());
        assert_eq!(
            report
                .removed
                .iter()
                .map(|it| it.path.clone())
                .collect::<Vec<_>>(),
            [goes]
        );
        assert_eq!(
            report
                .refused
                .iter()
                .map(|it| it.path.clone())
                .collect::<Vec<_>>(),
            [stuck]
        );
    }

    #[test]
    fn the_acting_pass_stops_when_a_workspace_appeared_that_it_cannot_place() {
        let mut four = four_clones();
        let plan = plan_for(&four.world, Insistence::NotInsisted);
        four.world.devpod.lists(&[listed_with(
            "unreadable",
            serde_json::json!({ "localFolder": {} }),
        )]);
        let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
        let mut context = CommandContext::new(&four.world.devpod);

        let outcome = prune_clones(
            &mut context,
            &clones,
            &mut four.world.storage,
            &plan,
            &mut ignoring(),
        )
        .expect("the pass ran");

        assert!(
            matches!(outcome, PruneOutcome::Unlocatable(_)),
            "{outcome:?}"
        );
        assert!(four.orphan_clean.exists(), "nothing was removed");
    }

    #[test]
    fn a_second_run_finds_nothing_left_to_do() {
        let mut four = four_clones();
        let plan = plan_for(&four.world, Insistence::NotInsisted);
        let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
        {
            let mut context = CommandContext::new(&four.world.devpod);
            prune_clones(
                &mut context,
                &clones,
                &mut four.world.storage,
                &plan,
                &mut ignoring(),
            )
            .expect("the pass ran");
        }
        drop(clones);

        let again = plan_for(&four.world, Insistence::NotInsisted);

        assert!(again.nothing_to_do());
    }

    #[test]
    fn the_acting_pass_pays_a_second_devpod_list() {
        // It is the one question whose answer cannot be re-derived from disk, and it is
        // paid only after a user has said yes to a deletion.
        let mut four = four_clones();
        let plan = plan_for(&four.world, Insistence::NotInsisted);
        let before = four.world.devpod.devpod_argvs().len();
        let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
        let mut context = CommandContext::new(&four.world.devpod);

        prune_clones(
            &mut context,
            &clones,
            &mut four.world.storage,
            &plan,
            &mut ignoring(),
        )
        .expect("the pass ran");

        assert_eq!(
            four.world.devpod.devpod_argvs()[before..],
            [vec![
                "list".to_owned(),
                "--output".to_owned(),
                "json".to_owned()
            ]]
        );
    }

    // =======================================================================
    // reconcile (devlaunch#88)
    // =======================================================================

    /// A plain clone directory: `.git` present, nothing else.
    ///
    /// Not the corner-cutting it would be in the prune tests. `--prune` guards a
    /// deletion, so its guard is a real `git status` and a stub would answer "holds
    /// nothing" — the reply that deletes. This command asks git nothing at all: what
    /// it needs to know is whether a directory is a checkout, which is `.git`'s
    /// presence and devlaunch#88's own published diagnostic.
    fn a_bare_clone_directory(at: &Path) -> PathBuf {
        std::fs::create_dir_all(at.join(".git")).expect("a clone directory");
        at.to_path_buf()
    }

    /// devpod's own record for a workspace, in devpod's on-disk shape.
    fn devpod_record(devpod_home: &DevpodHome, workspace_id: &str, source: &Path) -> PathBuf {
        let path = devpod_home.record("default", workspace_id);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the record directory");
        std::fs::write(
            &path,
            serde_json::json!({
                "id": workspace_id,
                "provider": { "name": "docker" },
                "ide": { "name": "none" },
                "source": { "localFolder": source.display().to_string() },
                "uid": "keep-me",
                "creationTimestamp": "2026-03-01T18:39:40Z",
                "context": "default",
            })
            .to_string(),
        )
        .expect("devpod's record");
        path
    }

    fn sourced_at(record: &Path) -> String {
        let text = std::fs::read_to_string(record).expect("devpod's record");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("it is JSON");
        parsed["source"]["localFolder"]
            .as_str()
            .expect("a source folder")
            .to_owned()
    }

    /// The plan `--reconcile` would print.
    fn reconcile_for(world: &World) -> ReconcilePlan {
        let clones = clones_for(&world.repos_dir, &world.devpod);
        let mut context = CommandContext::new(&world.devpod);
        let workspaces = context.workspaces().expect("a listing");
        let placement = ClonePlacement::resolve(&clones, &workspaces);
        reconcile_plan(
            &clones,
            &world.storage,
            &workspaces,
            &placement,
            &mut ignoring(),
        )
    }

    #[test]
    fn the_legacy_leaf_is_the_branch_flattened_for_a_path_component() {
        assert_eq!(legacy_leaf("feature/auth"), "feature-auth");
        assert_eq!(legacy_leaf("feature auth"), "feature-auth");
        assert_eq!(legacy_leaf("feature:auth"), "feature-auth");
        assert_eq!(legacy_leaf("main"), "main");
        assert_eq!(legacy_leaf("v1.2-rc_3"), "v1.2-rc_3");
        assert_eq!(legacy_leaf("/lead/"), "lead");
    }

    #[test]
    fn an_orphan_whose_clone_answers_to_its_old_leaf_is_adopted() {
        // The join is by path and never by id: the id is what the scheme change moved,
        // and the source path devpod kept still names owner, repo and branch.
        let mut world = World::empty();
        let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
        let clone = a_bare_clone_directory(&world.repo_dir.join("r-feature-auth-aaa"));
        world.record("r-feature-auth-aaa", "feature/auth", &clone);
        let old = world.repo_dir.join("feature-auth");
        let record = devpod_record(&devpod_home, "ws-old", &old);
        world.devpod.lists(&[listed("ws-old", &old)]);

        let plan = reconcile_for(&world);

        assert_eq!(plan.adopting.len(), 1, "{plan:?}");
        let adoptable = &plan.adopting[0];
        assert_eq!(adoptable.workspace_id, "ws-old");
        assert_eq!(adoptable.context, "default");
        assert_eq!(
            adoptable.clone,
            canonical(&clone.to_string_lossy()).expect("the clone")
        );
        assert!(plan.reporting.is_empty());
        assert!(!plan.nothing_to_do());
        assert!(record.exists());
    }

    #[test]
    fn applying_a_plan_repoints_devpods_own_record_and_keeps_its_other_keys() {
        let mut world = World::empty();
        let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
        let clone = a_bare_clone_directory(&world.repo_dir.join("r-feature-auth-aaa"));
        world.record("r-feature-auth-aaa", "feature/auth", &clone);
        let old = world.repo_dir.join("feature-auth");
        let record = devpod_record(&devpod_home, "ws-old", &old);
        world.devpod.lists(&[listed("ws-old", &old)]);
        let plan = reconcile_for(&world);
        let updater = SelfInvocation::new("dl");
        let cache_path = fresh_cache(world.tmp());
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&updater, &cache_path);

        let report = apply_reconciliation(
            &mut context,
            &mut refresh,
            &mut world.storage,
            &devpod_home,
            &plan,
            &mut ignoring(),
        );

        assert!(report.finished());
        assert_eq!(report.repointed().count(), 1);
        assert_eq!(
            sourced_at(&record),
            canonical(&clone.to_string_lossy())
                .expect("the clone")
                .display()
                .to_string()
        );
        let text = std::fs::read_to_string(&record).expect("devpod's record");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("it is JSON");
        assert_eq!(
            parsed["uid"], "keep-me",
            "every key devpod knows about and dl does not survives"
        );
        assert_eq!(
            recorded_devpod_workspace_id(&world.storage, OWNER, REPO, "feature/auth"),
            Some("ws-old".to_owned()),
            "the second copy of the id, which stops this happening again"
        );
        assert!(
            !record.with_extension("dl-tmp").exists(),
            "the temp file is renamed, not left behind"
        );
    }

    #[test]
    fn a_record_removed_while_the_plan_sat_there_is_not_reported_as_re_pointed() {
        // The confirmation prompt is an unbounded wait, and `dl <ws> rm` in
        // another terminal is what walks through it. devpod's record is
        // re-pointed either way — that write is done before metadata is
        // reloaded — but the id is not written, because writing it would put
        // back a row the other run deleted. What must not happen is the run
        // reporting an adoption that landed anyway.
        let mut world = World::empty();
        let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
        let clone = a_bare_clone_directory(&world.repo_dir.join("r-feature-auth-aaa"));
        world.record("r-feature-auth-aaa", "feature/auth", &clone);
        let old = world.repo_dir.join("feature-auth");
        let record = devpod_record(&devpod_home, "ws-old", &old);
        world.devpod.lists(&[listed("ws-old", &old)]);
        let plan = reconcile_for(&world);
        world
            .storage
            .remove_worktree(OWNER, REPO, "feature/auth")
            .expect("the other run's delete");
        let updater = SelfInvocation::new("dl");
        let cache_path = fresh_cache(world.tmp());
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&updater, &cache_path);

        let report = apply_reconciliation(
            &mut context,
            &mut refresh,
            &mut world.storage,
            &devpod_home,
            &plan,
            &mut ignoring(),
        );

        assert_eq!(
            report.adoptions(),
            [Adoption::Unrecorded {
                workspace_id: "ws-old".to_owned()
            }],
            "the ending the report carries is the one that happened"
        );
        assert_eq!(report.repointed().count(), 0, "nothing was recorded");
        assert!(
            !report.finished(),
            "an adoption that wrote nothing is not an adoption that landed"
        );
        assert!(
            world
                .storage
                .get_worktree(OWNER, REPO, "feature/auth")
                .is_none(),
            "the other run's delete stands"
        );
        assert_eq!(
            sourced_at(&record),
            canonical(&clone.to_string_lossy())
                .expect("the clone")
                .display()
                .to_string(),
            "devpod's record was re-pointed before the reload found the row gone"
        );
    }

    #[test]
    fn reconciling_never_reaches_devpod_delete() {
        // A wrongly-adopted workspace costs a rebuild; a wrongly-deleted one costs
        // whatever was in it.
        let mut world = World::empty();
        let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
        let old = world.repo_dir.join("no-such-clone");
        devpod_record(&devpod_home, "ws-old", &old);
        world.devpod.lists(&[listed("ws-old", &old)]);
        let plan = reconcile_for(&world);
        let updater = SelfInvocation::new("dl");
        let cache_path = fresh_cache(world.tmp());
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&updater, &cache_path);

        apply_reconciliation(
            &mut context,
            &mut refresh,
            &mut world.storage,
            &devpod_home,
            &plan,
            &mut ignoring(),
        );

        assert_eq!(world.devpod.deleted(), Vec::<String>::new());
        assert_eq!(
            plan.reporting
                .iter()
                .map(|it| it.because.clone())
                .collect::<Vec<_>>(),
            [NotAdopted::NoCloneAnswers]
        );
    }

    #[test]
    fn a_name_two_clones_answer_to_is_claimed_by_neither() {
        // The legacy spelling is not injective: `feature/auth` and `feature:auth` were
        // both the directory `feature-auth`, so one devpod record can name two
        // branches' clones and a map would hand it whichever was written last.
        let mut world = World::empty();
        let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
        let slash = a_bare_clone_directory(&world.repo_dir.join("r-feature-auth-aaa"));
        let colon = a_bare_clone_directory(&world.repo_dir.join("r-feature-auth-bbb"));
        world.record("r-feature-auth-aaa", "feature/auth", &slash);
        world.record("r-feature-auth-bbb", "feature:auth", &colon);
        let old = world.repo_dir.join("feature-auth");
        devpod_record(&devpod_home, "ws-old", &old);
        world.devpod.lists(&[listed("ws-old", &old)]);

        let plan = reconcile_for(&world);

        assert!(plan.adopting.is_empty(), "{plan:?}");
        let NotAdopted::NameAnsweredByManyClones(answers) = &plan.reporting[0].because else {
            panic!("expected a contested name, got {:?}", plan.reporting);
        };
        assert_eq!(answers.len(), 2);
    }

    #[test]
    fn a_clone_two_orphans_both_match_is_claimed_by_neither() {
        // Picking one would be a coin flip decided by listing order, and the loser
        // would still be broken with nothing said about why.
        let mut world = World::empty();
        let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
        let clone = a_bare_clone_directory(&world.repo_dir.join("r-main-aaa"));
        world.record("r-main-aaa", "main", &clone);
        let old = world.repo_dir.join("main");
        devpod_record(&devpod_home, "ws-one", &old);
        devpod_record(&devpod_home, "ws-two", &old);
        world
            .devpod
            .lists(&[listed("ws-one", &old), listed("ws-two", &old)]);

        let plan = reconcile_for(&world);

        assert!(plan.adopting.is_empty(), "{plan:?}");
        assert_eq!(plan.reporting.len(), 2);
        for unadoptable in &plan.reporting {
            assert!(
                matches!(
                    unadoptable.because,
                    NotAdopted::CloneWantedByManyWorkspaces { workspaces: 2, .. }
                ),
                "{unadoptable:?}"
            );
        }
    }

    #[test]
    fn a_clone_a_live_workspace_already_opens_is_not_a_candidate() {
        // Adopting it would point two workspaces at one directory and leave the working
        // one sharing its checkout with a dead one.
        let mut world = World::empty();
        let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
        let clone = a_bare_clone_directory(&world.repo_dir.join("r-main-aaa"));
        world.record("r-main-aaa", "main", &clone);
        let old = world.repo_dir.join("main");
        devpod_record(&devpod_home, "ws-old", &old);
        world
            .devpod
            .lists(&[listed("ws-old", &old), listed("ws-live", &clone)]);

        let plan = reconcile_for(&world);

        assert!(plan.adopting.is_empty(), "{plan:?}");
        assert_eq!(
            plan.reporting
                .iter()
                .map(|it| it.because.clone())
                .collect::<Vec<_>>(),
            [NotAdopted::NoCloneAnswers]
        );
    }

    #[test]
    fn running_it_twice_changes_nothing() {
        let mut world = World::empty();
        let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
        let clone = a_bare_clone_directory(&world.repo_dir.join("r-main-aaa"));
        world.record("r-main-aaa", "main", &clone);
        let old = world.repo_dir.join("main");
        let record = devpod_record(&devpod_home, "ws-old", &old);
        world.devpod.lists(&[listed("ws-old", &old)]);
        let updater = SelfInvocation::new("dl");
        let cache_path = fresh_cache(world.tmp());
        {
            let plan = reconcile_for(&world);
            let mut context = CommandContext::new(&world.devpod);
            let mut refresh = Refresh::new(&updater, &cache_path);
            apply_reconciliation(
                &mut context,
                &mut refresh,
                &mut world.storage,
                &devpod_home,
                &plan,
                &mut ignoring(),
            );
        }
        // devpod now sources the workspace at the clone, which is what a second run
        // reads.
        world.devpod.lists(&[listed("ws-old", &clone)]);

        let again = reconcile_for(&world);

        assert!(again.nothing_to_do(), "{again:?}");
        assert_eq!(
            sourced_at(&record),
            canonical(&clone.to_string_lossy())
                .expect("the clone")
                .display()
                .to_string()
        );
    }

    #[test]
    fn a_repair_that_cannot_be_made_is_not_half_made() {
        // devpod's record is re-pointed first and metadata's id written second, and the
        // failure of the first must leave the second alone.
        let mut world = World::empty();
        let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
        let clone = a_bare_clone_directory(&world.repo_dir.join("r-main-aaa"));
        world.record("r-main-aaa", "main", &clone);
        let old = world.repo_dir.join("main");
        // Listed, with no workspace.json written for it: a run that decided to adopt
        // this workspace has nothing to rewrite and must say so.
        world.devpod.lists(&[listed("ws-old", &old)]);
        let plan = reconcile_for(&world);
        assert_eq!(plan.adopting.len(), 1, "{plan:?}");
        let updater = SelfInvocation::new("dl");
        let cache_path = fresh_cache(world.tmp());
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&updater, &cache_path);

        let report = apply_reconciliation(
            &mut context,
            &mut refresh,
            &mut world.storage,
            &devpod_home,
            &plan,
            &mut ignoring(),
        );

        assert!(!report.finished());
        assert!(matches!(
            report.adoptions(),
            [Adoption::Refused {
                failure: RepointFailure::Unreadable { .. },
                ..
            }]
        ));
        assert_eq!(
            recorded_devpod_workspace_id(&world.storage, OWNER, REPO, "main"),
            None,
            "the id is written only once devpod's record says the same thing"
        );
    }
}
