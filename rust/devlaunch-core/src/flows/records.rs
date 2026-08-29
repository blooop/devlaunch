//! devlaunch's own records: the config, the metadata store and the clone manager,
//! opened together and once.
//!
//! # Built when they are needed, and once
//!
//! Python memoized the clone manager in a module-level dict and ran the one-shot
//! id-scheme migration on the way through the factory, so that `--help`,
//! `--version`, `--ls`, the completion commands and a warm launch never paid for
//! any of it (#58, then #145). That laziness is behaviour and not an
//! optimisation: it decides which commands run the migration at all.
//!
//! Here it is a value a caller builds when it needs one, rather than a memo to
//! reset: [`open_records`] is the single construction point, it runs the migration
//! exactly once because it is called at most once per command, and a caller that
//! never calls it has provably not touched `metadata.json`. The type that holds
//! the other end of that promise is
//! [`flows::launch::ColdPath`](crate::flows::launch::ColdPath).
//!
//! # No sentences here
//!
//! The load, the retired keys and the migration all have things to report, and
//! none of them is a sentence: they travel as [`RecordsNotice`] and whoever holds
//! the sink writes the words (#251 §5). This module was the `dl` binary's
//! `session.rs` until #340, and the reason it moved is that the binary was the
//! only program that could open devlaunch's records at all.

use crate::clients::git::Git;
use crate::domain::config::{self, ConfigError, RetiredKey};
use crate::domain::metadata::{self, MetadataError, MetadataStorage};
use crate::domain::xdg::{self, NoHomeDirectory};
use crate::flows::migration::{self, MigrationReport};
use crate::flows::workspace_clone::WorkspaceCloneManager;
use crate::runner::Runner;

/// Why a command could not get as far as running.
///
/// Three separate reasons because they are fixed in three different places: an
/// environment with no home directory, a `config.toml` that cannot be read, and a
/// `metadata.json` that cannot be opened.
///
/// **Part of the frozen wf API (#251 §7)**, re-exported from
/// [`api`](crate::api) since #340: it is the payload of
/// [`ColdRefused::Startup`](crate::flows::launch::ColdRefused::Startup), and a
/// consumer that cannot match on it is holding a refusal it cannot read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupError {
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

/// Something opening the records had to say, in the order it happened.
///
/// One vocabulary over what used to be four fields a caller drained in a fixed
/// order: the config's retired keys, the load's notices, the migration's report and
/// the migration's refusal. It is one ordered sequence rather than a struct of four
/// lists because the order *is* the report — Python's factory read the config,
/// opened the store and then announced the migration from inside it — and a caller
/// holding four lists has to know that order to reproduce it.
///
/// Note what this vocabulary does *not* buy, unlike the launch's: the open is a
/// single act, so [`open_records`] finishes before anything can be said about it and
/// [`ColdPath`](crate::flows::launch::ColdPath) says the whole sequence at once. The
/// sink is what makes the saying the caller's and the ordering core's, not what
/// makes it early.
///
/// **Part of the frozen wf API (#251 §7)**, re-exported from [`api`](crate::api)
/// since #340: it is what
/// [`ColdPath::new`](crate::flows::launch::ColdPath::new) takes its sink in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordsNotice {
    /// A key `config.toml` names that this build no longer reads. Only
    /// `worktree.repos_dir` today, and it is reported rather than ignored because
    /// it used to decide where the clones went: a user who set it has a tree at
    /// that path, and this run is the only thing that will ever name it.
    RetiredKey(RetiredKey),
    /// Something the load of `metadata.json` found and the user should be told.
    Metadata(metadata::Notice),
    /// What the cache migration did, on the runs where it ran and produced a
    /// report. Absent on the common already-current case, which costs a single
    /// integer comparison and no scan.
    Migrated(MigrationReport),
    /// Why the cache could not be migrated.
    ///
    /// A failed migration must not take the command with it — the renames that did
    /// happen are still resumable, because the version header is only written by
    /// the final save — so this is reported and the command carries on, as Python's
    /// `logging.warning` did.
    MigrationRefused(MetadataError),
}

/// devlaunch's own records and clones, with the cache migration already run.
///
/// Holds the manager and the store together because the listing reads both and
/// they have to describe the same cache. There is deliberately no second copy of
/// the config here: the clone root is what the commands want, and the manager is
/// what answers for that (see
/// [`lifecycle::ClonePlacement`](crate::flows::lifecycle::ClonePlacement)), so a
/// command cannot scan one tree while locking against another.
///
/// Not re-exported from [`api`](crate::api), and reachable from it all the same:
/// it is what [`ColdPath::records`](crate::flows::launch::ColdPath::records)
/// answers with. That is the classifier gap #352 is about rather than a second
/// tier, so treat a change here as a change to the promise.
pub struct Records<'r> {
    pub storage: MetadataStorage,
    /// The clone manager, which is the one thing that names a record's clone
    /// directory: the listing, the `dl <ws> rm` guard and the delete itself all
    /// have to name the *same* directory, and they used to name it separately and
    /// could disagree (devlaunch#174).
    pub clones: WorkspaceCloneManager<'r>,
    /// Everything the load and the migration had to say, in the order it happened.
    /// Said by whoever opened the records: these are typed events, and the
    /// sentences are the caller's.
    pub reported: Vec<RecordsNotice>,
}

/// Open `metadata.json` and nothing else: no config, no clone manager, no cache
/// migration.
///
/// What a caller reaches for when it has one field of the record to read and no
/// business writing anything. [`open_records`] is still the construction point for
/// everything that mutates — this is the same load without the three things that
/// make that call expensive and consequential, and it is deliberately not a way
/// around it: a caller that wants a record's *clone* wants the migration, because
/// the directory it is about may not have been renamed yet.
///
/// The load is not a pure read. A `metadata.json` that cannot be parsed is
/// quarantined by it, which is exactly what every other command does to the same
/// file, and the notices come back so a caller can say so rather than letting a
/// record be moved aside in silence.
pub fn open_storage() -> Result<(MetadataStorage, Vec<metadata::Notice>), StartupError> {
    Ok(MetadataStorage::open(MetadataStorage::default_path()?)?)
}

/// Open devlaunch's records, migrating the cache if it has not been migrated yet.
///
/// The one construction point, so nothing can reach a stale clone path before the
/// rename. On an already-migrated cache the migration costs a single integer
/// comparison: the trigger is the version header the load already parsed.
pub fn open_records<'r>(runner: &'r dyn Runner) -> Result<Records<'r>, StartupError> {
    let (config, retired_keys) = config::worktree_config()?;
    let cache_dir = xdg::devlaunch_cache()?;
    let (mut storage, notices) = open_storage()?;
    // The report is carried out and said by the caller. Python's `migrate_cache`
    // announces inside itself (migration.py `_announce`); core renders no English
    // (#251), so the report travels up and the binary writes the sentences — the
    // migration's orphan/unmigrated notices, including the only pointer a user
    // gets to `dl --reconcile`/`recreate` for the containers it orphaned.
    let (migration, migration_refused) =
        match migration::migrate_cache(&mut storage, &xdg::clone_root_in(&cache_dir)) {
            Ok(report) => (report, None),
            Err(refused) => (None, Some(refused)),
        };
    let clones = WorkspaceCloneManager::in_cache(&cache_dir, &config, Git::new(runner));
    Ok(Records {
        storage,
        clones,
        reported: reported(retired_keys, notices, migration, migration_refused),
    })
}

/// The one order the four sources are reported in, as a function of them.
///
/// The config is read before the records are opened, so its notices come first;
/// the migration's come after the load's and before any refusal, which is the order
/// Python's factory produced them in. Separated from [`open_records`] so the order
/// is testable without a cache directory to open.
fn reported(
    retired_keys: Vec<RetiredKey>,
    notices: Vec<metadata::Notice>,
    migration: Option<MigrationReport>,
    migration_refused: Option<MetadataError>,
) -> Vec<RecordsNotice> {
    retired_keys
        .into_iter()
        .map(RecordsNotice::RetiredKey)
        .chain(notices.into_iter().map(RecordsNotice::Metadata))
        .chain(migration.into_iter().map(RecordsNotice::Migrated))
        .chain(
            migration_refused
                .into_iter()
                .map(RecordsNotice::MigrationRefused),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::metadata::OsFailure;
    use std::path::PathBuf;

    fn a_notice() -> metadata::Notice {
        metadata::Notice::VersionFromNewerBuild {
            path: PathBuf::from("/cache/metadata.json"),
            found: 9,
            understood: 2,
        }
    }

    #[test]
    fn the_four_sources_are_reported_in_the_order_python_produced_them() {
        let refused = MetadataError::CreateDir {
            path: PathBuf::from("/nowhere"),
            failure: OsFailure {
                kind: std::io::ErrorKind::PermissionDenied,
                message: "Permission denied (os error 13)".to_owned(),
            },
        };
        let key = RetiredKey::ReposDir {
            named: "/old".to_owned(),
        };
        let said = reported(
            vec![key.clone()],
            vec![a_notice()],
            Some(MigrationReport::default()),
            Some(refused.clone()),
        );

        assert_eq!(
            said,
            [
                RecordsNotice::RetiredKey(key),
                RecordsNotice::Metadata(a_notice()),
                RecordsNotice::Migrated(MigrationReport::default()),
                RecordsNotice::MigrationRefused(refused),
            ]
        );
    }

    #[test]
    fn a_clean_open_has_nothing_to_say() {
        assert_eq!(reported(Vec::new(), Vec::new(), None, None), Vec::new());
    }
}
