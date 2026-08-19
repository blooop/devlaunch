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
use devlaunch_core::flows::migration::{self, MigrationReport};
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
pub(crate) fn self_invocation() -> SelfInvocation {
    SelfInvocation::new(refresh_program(std::env::current_exe().ok()))
}

/// The program the refresh child is spawned as, from what `current_exe()` said.
///
/// The answer has to still be spawnable, which the running binary's path is not
/// guaranteed to be: after `pixi global update` swaps the binary mid-run, Linux
/// reports the unlinked inode as `/path/dl (deleted)` — a path that exists for no
/// one — so spawning it fails with `ProgramNotFound` and completions silently
/// lose their freshness. Python's `sys.executable` survives the same swap, so
/// this is where the gap is closed: a path that no longer exists, or that carries
/// the kernel's ` (deleted)` mark (checked on its own too, against a file that
/// happens to sit at the marked name), falls back to the bare program name and
/// lets the spawn's PATH search find the replacement. That name is the only
/// other honest guess; a spawn that then finds nothing is
/// [`lifecycle::SpawnRefused::ProgramNotFound`](devlaunch_core::flows::lifecycle::SpawnRefused::ProgramNotFound),
/// and a refresh that could not be spawned costs completions their freshness and
/// nothing else.
fn refresh_program(current_exe: Option<PathBuf>) -> String {
    match current_exe {
        Some(path) if path.exists() && !path.to_string_lossy().ends_with(" (deleted)") => {
            path.to_string_lossy().into_owned()
        }
        _ => "dl".to_owned(),
    }
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
/// is what answers for that (see [`lifecycle::ClonePlacement`]), so a command
/// cannot scan one tree while locking against another.
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
    /// What the cache migration did, when it ran and produced a report. `None`
    /// covers both the common already-current case (a single integer comparison,
    /// no scan) and a migration that a concurrent process had already finished.
    /// Rendered by the caller: the report carries the facts, and the sentences —
    /// Python's `_announce`, up to nine notice classes and the only pointer to
    /// `dl --reconcile` for orphaned containers — are the binary's (#251).
    pub(crate) migration: Option<MigrationReport>,
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
    // The report is kept and rendered by the caller. Python's `migrate_cache`
    // announces inside itself (migration.py `_announce`); core renders no English
    // (#251), so the report travels up and the binary writes the sentences — the
    // migration's orphan/unmigrated notices, including the only pointer a user
    // gets to `dl --reconcile`/`recreate` for the containers it orphaned.
    let (migration, migration_refused) =
        match migration::migrate_cache(&mut storage, &config.repos_dir) {
            Ok(report) => (report, None),
            Err(refused) => (None, Some(refused)),
        };
    let clones = WorkspaceCloneManager::from_config(&config, Git::new(runner));
    Ok(Records {
        storage,
        clones,
        notices,
        migration,
        migration_refused,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_that_still_exists_is_respawned_by_its_own_path() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let binary = dir.path().join("dl");
        std::fs::write(&binary, "").expect("a file standing in for the binary");

        assert_eq!(
            refresh_program(Some(binary.clone())),
            binary.to_string_lossy()
        );
    }

    #[test]
    fn a_binary_swapped_out_from_under_the_run_falls_back_to_the_bare_name() {
        // What `current_exe()` answers after `pixi global update` replaces the
        // binary mid-run: the unlinked inode's path, which exists for no one. The
        // swap is simulated by the path-exists check — the path simply is not
        // there — which is exactly the fact the decision turns on.
        let dir = tempfile::tempdir().expect("a temporary directory");

        assert_eq!(refresh_program(Some(dir.path().join("dl (deleted)"))), "dl");
        assert_eq!(refresh_program(Some(dir.path().join("dl"))), "dl");
    }

    #[test]
    fn the_kernels_deleted_mark_is_refused_even_where_a_file_wears_it() {
        // ` (deleted)` is the kernel's annotation, not part of any name dl was
        // started by — so a file that happens to sit at the marked path must not
        // launder the mark into an answer.
        let dir = tempfile::tempdir().expect("a temporary directory");
        let marked = dir.path().join("dl (deleted)");
        std::fs::write(&marked, "").expect("a file at the marked name");

        assert_eq!(refresh_program(Some(marked)), "dl");
    }

    #[test]
    fn a_path_that_could_not_be_read_at_all_falls_back_to_the_bare_name() {
        assert_eq!(refresh_program(None), "dl");
    }
}
