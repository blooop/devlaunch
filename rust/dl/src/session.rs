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
use devlaunch_core::domain::model::WorktreeInfo;
use devlaunch_core::domain::xdg::{self, NoHomeDirectory};
use devlaunch_core::flows::listing::ClonePathResolver;
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
pub(crate) fn cache_dir() -> Result<PathBuf, NoHomeDirectory> {
    xdg::devlaunch_cache()
}

/// The worktree config, for the commands that need only `repos_dir`.
pub(crate) fn worktree_config() -> Result<WorktreeConfig, ConfigError> {
    config::worktree_config()
}

/// dl's own records and clones, with the cache migration already run.
///
/// Holds the manager and the store together because the listing reads both and
/// they have to describe the same cache.
pub(crate) struct Records<'r> {
    /// The config the manager was built from — `repos_dir` is what `--prune` and
    /// the launch flows need, and they are the ones that will read it.
    #[allow(dead_code)] // consumed from M6 on
    pub(crate) config: WorktreeConfig,
    pub(crate) storage: MetadataStorage,
    pub(crate) clones: Clones<'r>,
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
    let clones = Clones {
        manager: WorkspaceCloneManager::from_config(&config, Git::new(runner)),
    };
    Ok(Records {
        config,
        storage,
        clones,
        notices,
        migration_refused,
    })
}

/// The clone manager, as the listing's [`ClonePathResolver`].
///
/// The listing, the `dl <ws> rm` guard and the delete itself all have to name the
/// *same* directory, and they used to name it separately and could disagree
/// (devlaunch#174). This is the seam that keeps the listing reading the one
/// function that names it rather than a second copy of the rule.
pub(crate) struct Clones<'r> {
    manager: WorkspaceCloneManager<'r>,
}

impl ClonePathResolver for Clones<'_> {
    fn clone_path(&self, record: &WorktreeInfo) -> Option<PathBuf> {
        self.manager.clone_path(record)
    }
}
