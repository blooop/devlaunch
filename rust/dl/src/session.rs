//! What one `dl` command holds: the runner it spawns through, the cache
//! directory, and — for the commands that need them — the config, the records and
//! the clone manager.
//!
//! # Built when they are needed, and once
//!
//! Python memoized the clone manager in a module-level dict and ran the one-shot
//! id-scheme migration on the way through the factory, so that `--help`,
//! `--version`, `--ls`, the completion commands and a warm launch never paid for
//! any of it (#58, then #145). That laziness is behaviour and not an
//! optimisation: it decides which commands run the migration at all.
//!
//! Here it is a value a command builds when it needs one, rather than a memo to
//! reset: [`open_records`] is the single construction point, it runs the migration
//! exactly once because it is called at most once per command, and a command that
//! never calls it has provably not touched `metadata.json`.

use std::path::PathBuf;

use devlaunch_core::clients::git::Git;
use devlaunch_core::domain::config::{self, ConfigError, WorktreeConfig};
use devlaunch_core::domain::metadata::{MetadataError, MetadataStorage, Notice};
use devlaunch_core::domain::xdg::{self, NoHomeDirectory};
use devlaunch_core::flows::lifecycle::SelfInvocation;
use devlaunch_core::flows::migration;
use devlaunch_core::flows::workspace_clone::WorkspaceCloneManager;
use devlaunch_core::runner::Runner;

/// Why a command could not get as far as running.
///
/// Three separate reasons because they are fixed in three different places: an
/// environment with no home directory, a `config.toml` that cannot be read, and a
/// `metadata.json` that cannot be opened.
#[derive(Debug)]
pub(crate) enum StartupError {
    NoHomeDirectory,
    Config(ConfigError),
    Metadata(MetadataError),
}

impl From<NoHomeDirectory> for StartupError {
    fn from(_: NoHomeDirectory) -> Self {
        StartupError::NoHomeDirectory
    }
}

impl From<ConfigError> for StartupError {
    fn from(error: ConfigError) -> Self {
        StartupError::Config(error)
    }
}

impl From<MetadataError> for StartupError {
    fn from(error: MetadataError) -> Self {
        StartupError::Metadata(error)
    }
}

/// Where devlaunch keeps everything: the directory ownership is decided by and
/// `--purge` removes.
///
/// The answer comes from `xdg` so that this, the worktree config's default
/// `repos_dir` and `metadata.json`'s default path cannot drift apart — ownership
/// decides what `--purge` may delete by asking whether a workspace's source is
/// under this directory, and the clones it is asking about were put there by the
/// other two. (`flows::completion_cache` carried a second copy of this call until
/// the port finished and there was one caller left.)
pub(crate) fn cache_dir() -> Result<PathBuf, NoHomeDirectory> {
    xdg::devlaunch_cache()
}

/// How to re-run *this* build as a detached child.
///
/// `current_exe()` is asked here and nowhere in core: a library that asked the OS
/// who it is would answer `wf` when wf links it and `python` when the harness
/// drives it, so the one process that knows which program it is hands the answer
/// down. No leading arguments — Python's re-invocation needs `-m devlaunch.dl` and
/// a compiled binary needs nothing.
///
/// A build whose own path cannot be read falls back to the name a shell would
/// resolve, which is the only other honest guess; a spawn that then finds nothing
/// is [`lifecycle::SpawnRefused::ProgramNotFound`], and a refresh that could not be
/// spawned costs completions their freshness and nothing else.
pub(crate) fn self_invocation() -> SelfInvocation {
    let program = std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "dl".to_owned());
    SelfInvocation::new(program)
}

/// The worktree config, for the commands that need only `repos_dir`.
pub(crate) fn worktree_config() -> Result<WorktreeConfig, ConfigError> {
    config::worktree_config()
}

/// dl's own records and clones, with the cache migration already run.
///
/// Holds the manager and the store together because the listing reads both and
/// they have to describe the same cache. There is deliberately no second copy of
/// the config here: `repos_dir` is what the commands want from it, and the manager
/// is what answers for that (see [`lifecycle::clone_root`]), so a command cannot
/// scan one tree while locking against another.
pub(crate) struct Records<'r> {
    pub(crate) storage: MetadataStorage,
    /// The clone manager, which is the one thing that names a record's clone
    /// directory: the listing, the `dl <ws> rm` guard and the delete itself all
    /// have to name the *same* directory, and they used to name it separately and
    /// could disagree (devlaunch#174).
    pub(crate) clones: WorkspaceCloneManager<'r>,
    /// Everything the load and the migration had to say, in the order it happened.
    /// Rendered by the caller: these are typed events, and the sentences are the
    /// binary's.
    pub(crate) notices: Vec<Notice>,
    /// Why the cache could not be migrated, when it could not.
    ///
    /// A failed migration must not take the command with it — the renames that did
    /// happen are still resumable, because the version header is only written by
    /// the final save — so this is reported and the command carries on, as Python's
    /// `logging.warning` did.
    pub(crate) migration_refused: Option<MetadataError>,
}

/// Open dl's records, migrating the cache if it has not been migrated yet.
///
/// The one construction point, so nothing can reach a stale clone path before the
/// rename. On an already-migrated cache the migration costs a single integer
/// comparison: the trigger is the version header the load already parsed.
pub(crate) fn open_records<'r>(runner: &'r dyn Runner) -> Result<Records<'r>, StartupError> {
    let config = worktree_config()?;
    let (mut storage, notices) = MetadataStorage::open(MetadataStorage::default_path()?)?;
    // The report is not rendered: Python discards it too, and the files the
    // migration writes (the orphan and unmigrated listings) are what it leaves
    // behind for a person to read.
    let migration_refused = migration::migrate_cache(&mut storage, &config.repos_dir).err();
    let clones = WorkspaceCloneManager::from_config(&config, Git::new(runner));
    Ok(Records {
        storage,
        clones,
        notices,
        migration_refused,
    })
}
