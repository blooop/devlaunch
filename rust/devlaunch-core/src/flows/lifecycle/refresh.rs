//! The detached refresh child: `dl` re-invoking itself to warm the listing.

use std::path::Path;

use crate::flows::completion_cache;
use crate::runner::{DetachOutcome, Invocation, Runner};

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
