//! The fetch sweep: keeping the bare caches current in the background.

use super::notices::{LifecycleNotice, extend_with_cache, extend_with_store};
use crate::domain::locks::{self, LockError};
use crate::domain::metadata::{MetadataStorage, RecordUpdate};
use crate::domain::model::{SweepNote, SweepTrouble};
use crate::flows::repo_manager::{
    BACKGROUND_FETCH_TIMEOUT, CacheNotice, FetchRepoError, Fetched, LazyFetchError,
    RepositoryManager,
};
use crate::notices::Notices;

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
        // Read off the same two values the arm below is built from, before either
        // is moved: the note and the counted arm have to be two readings of one
        // pass, which is the mistake `disk` and `unsaved` each made in turn in the
        // listing.
        let left_behind = last_sweep_note(&swept, &cache_notices);
        extend_with_cache(&mut report.notices, cache_notices);
        if let LastSweep::Wrote(note) = left_behind {
            record_sweep_note(storage, &owner, &repo, note, &mut report.notices);
        }
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

/// What one pass of the sweep leaves in one repository's record.
///
/// Two arms rather than a bare `Option<SweepNote>`, because there are three
/// outcomes and only two of them are notes: a pass can leave a note, clear the
/// last one, or have no standing to touch the field at all. Collapsing the last
/// two would have a contended repository report a clean sweep that never ran.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LastSweep {
    /// The pass acted on the repository. `None` clears whatever the pass before
    /// it left, which is what makes a cache that has been fixed stop complaining.
    Wrote(Option<SweepNote>),
    /// Nothing was attempted — the interval had not elapsed, another dl run held
    /// the lock, or the lock would not open — so the last pass's note stands.
    Untouched,
}

/// What the record should say about one repository after this pass.
///
/// The pack refusal is read out of the notices the fetch raised rather than out of
/// its return value, because a refused pack is deliberately *not* a failed fetch
/// (docs/cleanup.md): `fetch_repo` returns `Ok` and says so in a
/// [`CacheNotice::RefsNotPacked`], which until now went to the detached child's
/// null stderr and nowhere else.
fn last_sweep_note(
    swept: &Result<Option<Result<Fetched, LazyFetchError>>, LockError>,
    raised: &[CacheNotice],
) -> LastSweep {
    match swept {
        Ok(Some(Ok(Fetched::Fetched))) => LastSweep::Wrote(refs_not_packed(raised)),
        // The repository left the store between the listing above and the fetch,
        // so there is no record to annotate. `update_repository` would answer
        // `Absent`; saying so here keeps the write off a path with nothing to
        // write to.
        Ok(Some(Err(LazyFetchError::NotInMetadata { .. }))) => LastSweep::Untouched,
        Ok(Some(Err(LazyFetchError::Fetch(refused)))) => {
            LastSweep::Wrote(Some(fetch_trouble(refused)))
        }
        Ok(Some(Ok(Fetched::Skipped))) | Ok(None) | Err(_) => LastSweep::Untouched,
    }
}

/// The pack refusal this pass raised, if it raised one.
fn refs_not_packed(raised: &[CacheNotice]) -> Option<SweepNote> {
    raised.iter().find_map(|notice| match notice {
        CacheNotice::RefsNotPacked { reason, .. } => Some(SweepNote {
            trouble: SweepTrouble::RefsNotPacked,
            said: Some(reason.clone()),
        }),
        _ => None,
    })
}

/// A failed fetch as the record carries it.
///
/// Every arm keeps git's words where git is what refused, and `None` where nothing
/// spoke — a child killed at the bound and a directory that is not there are
/// conditions dl observed, and inventing a sentence for them here would be core
/// writing English (#251 §5). Which condition it was is the arm, and the `dl`
/// binary spells it.
fn fetch_trouble(refused: &FetchRepoError) -> SweepNote {
    match refused {
        FetchRepoError::NoLocalClone { .. } => SweepNote {
            trouble: SweepTrouble::CloneMissing,
            said: None,
        },
        FetchRepoError::TimedOut { .. } => SweepNote {
            trouble: SweepTrouble::FetchTimedOut,
            said: None,
        },
        FetchRepoError::Refused { reason } => SweepNote {
            trouble: SweepTrouble::FetchRefused,
            said: Some(reason.clone()),
        },
        FetchRepoError::NotRecorded(_) => SweepNote {
            trouble: SweepTrouble::NotRecorded,
            said: None,
        },
    }
}

/// Put the note in the record, outside the repo lock the fetch held.
///
/// Outside on purpose: the metadata lock is its own, `update_repository` reloads
/// the record under it and moves one field, and holding the repository's lock
/// across that write would put every sibling launch behind a second file lock for
/// a field no launch reads.
fn record_sweep_note(
    storage: &mut MetadataStorage,
    owner: &str,
    repo: &str,
    note: Option<SweepNote>,
    notices: &mut dyn Notices<LifecycleNotice>,
) {
    match storage.update_repository(owner, repo, |recorded| recorded.last_sweep = note) {
        // `Absent` is the record removed by another run while this pass was in it.
        // Nothing was written, which is right — a store that inserted would undo
        // that delete — and it is not news about the sweep.
        Ok((RecordUpdate::Applied | RecordUpdate::Absent, store_notices)) => {
            extend_with_store(notices, store_notices);
        }
        // The sweep never complains, and here it could not if it wanted to: a note
        // that cannot be written is the very condition it would have reported, and
        // there is nowhere left to put it. The next pass writes the same note.
        Err(_nowhere_to_put_it) => {}
    }
}
