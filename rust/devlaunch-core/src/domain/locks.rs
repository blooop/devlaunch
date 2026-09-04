//! Inter-process locks for the shared cache.
//!
//! Several dl processes can run at once — two agents launched on their own
//! branches, a completion refresh in the background — and they share one
//! bare-clone cache and one metadata.json. These locks are what keeps
//! simultaneous runs from racing each other over that state: without them, two
//! first launches of a repo both ran `git clone --bare` into the same path (and
//! the loser's cleanup deleted the winner's half-written clone), and metadata
//! writers rewrote the file from stale in-memory copies, dropping each other's
//! records.
//!
//! `flock(2)` rather than a pid file: the kernel releases it when the process
//! dies, however it dies, so a crashed dl never leaves the cache wedged. It is
//! the same syscall Python's `fcntl.flock` makes — advisory, **per open file
//! description** — so two `File` handles on one path conflict even inside one
//! process, which is what lets the tests below pin the semantics without
//! subprocesses.
//!
//! Two deliberate limits, both load-bearing:
//!
//! - **Not reentrant.** Acquiring a path twice in one process deadlocks (the
//!   second open file description blocks on the first). Call sites are
//!   structured so no lock is ever taken while the same lock is held. For the
//!   per-repo lock, one scope takes it and the steps running under it require
//!   the token that scope mints. What the token buys is narrower than it looks:
//!   it is proof the lock **is** held, so a step that has one has no reason
//!   left to acquire the lock itself, and a step written without one cannot be
//!   called from inside the scope at all. It is not a guard against re-locking
//!   — structuring the call sites remains what prevents that; the token is what
//!   makes the structure visible in the types instead of remembered.
//! - **A lock file can be unlinked under a holder, and every path that takes one
//!   revalidates.** Unlinking an flock'd file is the classic self-defeating
//!   move: a process that opened the old inode still "holds" a lock nobody else
//!   can see, while new arrivals lock a fresh file and walk straight past it.
//!   The answer used to be never to unlink at all, which made a per-workspace
//!   lock file a permanent straggler (devlaunch#575). It is closed here instead,
//!   by asking after every flock whether the path still names the inode just
//!   locked: an acquisition that has lost its file queues again against the live
//!   one rather than believing itself alone, and [`reclaim`] declines to unlink a
//!   file that is not the one it locked.
//!
//!   **Not because [`reclaim`] is the only unlinker.** It is the only one that
//!   takes a lock first, which is a different claim: `dl --purge` removes the
//!   cache directory entire ([`crate::flows::lifecycle::purge`]) and the launch
//!   locks go with it, holding nothing. So "somebody unlinked this path" is an
//!   ordinary event here, not a devlaunch bug, and the revalidation is what every
//!   path relies on rather than a belt over a brace.
//!
//! **Lock ordering is an invariant, not a habit.** Only one order between the
//! per-repo lock and the single metadata lock ([`crate::domain::metadata`]) is
//! legal:
//!
//! > the metadata lock may be taken while a repo lock is held; never the
//! > reverse.
//!
//! Every site that writes metadata while holding a repo lock takes them in that
//! order, and a single site taking them the other way round would be enough to
//! deadlock two dl runs against each other with nothing looking wrong at either
//! site. There is a third lock — the per-workspace launch lock — but it is only
//! ever the outermost one, so it does not participate in the ordering above.
//!
//! The enumeration of the sites deliberately lives in the code rather than
//! here: a list in a doc comment goes stale the first time someone adds a
//! writer, and a stale list is worse than none because it is what the next
//! reader trusts. Together with the non-reentrancy above, the rule in full: no
//! lock is taken while the same lock is held, and repo always precedes
//! metadata.
//!
//! **Nothing here prints.** Python announced a contended wait on stderr from
//! inside the blocking acquisition; core renders no English (#251), so the
//! about-to-block moment is handed to the caller as [`WaitStarted`] and the
//! wait it cost comes back on the guard as [`Contention`]. Both are ignorable:
//! [`hold_lock`] is the same call without either.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::fs::{FlockOperation, flock};

use super::metadata::OsFailure;

/// The permission bits a lock file this module creates is born with.
///
/// The shared cache is per-user state under `$XDG_CACHE_HOME`; a lock file
/// another user can open is a lock another user can hold, which on a shared box
/// is a way to stop somebody else's dl for free. Creation is all this can
/// promise: the mode is ignored for a file that already exists, so one that
/// arrived with looser bits keeps them until something removes it: [`reclaim`],
/// for a workspace that is gone, or `dl --purge` taking the cache with it.
const LOCK_FILE_MODE: u32 = 0o600;

/// How many times an acquisition will re-open a path that was replaced under it.
///
/// One retry is all a real race needs: a sweep unlinks at most once per path per
/// run, and `dl --purge` removes the cache directory once and does not put it
/// back, so the second attempt meets a file nobody is about to take away. The
/// bound is generous against that and still finite, because the alternative to a
/// bound here is a loop whose exit depends on another process losing interest.
const ACQUIRE_ATTEMPTS: usize = 8;

/// Whether an acquisition had to queue behind another holder.
///
/// Contention is information: a launch that waited knows the world may have
/// changed while it did (a sibling may have brought the very workspace it wants
/// up) and can re-check cheaply, where a launch that walked straight in knows
/// its earlier reads still stand. Callers that only want the mutual exclusion
/// never look.
///
/// `Queued` carries how long the wait took because that is the one measurement
/// only the blocking call can make — [`crate::timing`] spans the queueing, not
/// the holding: an uncontended lock costs nothing and records nothing, and what
/// a summary should show is the time this process spent behind a sibling rather
/// than the time its own work then took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Contention {
    /// The lock was free; this process walked straight in.
    WalkedIn,
    /// Another holder had it, and this process waited this long for it.
    Queued { waited: Duration },
}

impl Contention {
    /// Whether this process had to wait — Python's `waited` flag.
    ///
    /// Only this module's tests ask; the flows above match on the arm itself.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn waited(self) -> bool {
        matches!(self, Contention::Queued { .. })
    }
}

/// The moment an acquisition found the lock held and is about to block.
///
/// Handed to [`hold_lock_watching`]'s callback before the blocking call, so a
/// dl run that sits waiting on a sibling's long clone can say why it is
/// sitting. The rendering — and the words — belong to the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WaitStarted {
    pub(crate) lock_path: PathBuf,
}

/// Which step of taking a lock failed, and what the OS said about it.
///
/// The OS's side is an [`OsFailure`] rather than the `io::Error` it came from, and
/// that is what makes this type `Clone` and comparable — which the types above it
/// need, because a lock refusal travels inside a notice
/// ([`crate::flows::lifecycle::LifecycleNotice`]) and a notice is both. The message
/// is the `io::Error`'s own `to_string()`, so nothing a reader sees changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// The lock file's directory could not be created. A first launch locks a
    /// repo directory that does not exist yet, so this is a real step.
    CreateParent { path: PathBuf, failure: OsFailure },
    /// The lock file could not be opened or created.
    Open { path: PathBuf, failure: OsFailure },
    /// `flock` failed for a reason that is not "somebody else holds it".
    Acquire { path: PathBuf, failure: OsFailure },
    /// The file this locked stopped being the file the path names, [`ACQUIRE_ATTEMPTS`]
    /// times over. Reachable only if something is unlinking the lock in a loop,
    /// which nothing in devlaunch does; it is here so that the revalidation has a
    /// bounded exit rather than one that depends on another process stopping.
    Superseded { path: PathBuf, attempts: usize },
}

/// An exclusive inter-process lock, held until this value is dropped.
///
/// Releasing is the drop, and the drop is a close: the kernel owns the lock, so
/// a panic, an early return or a `SIGKILL` all release it. The lock file itself
/// is left where it is.
#[derive(Debug)]
pub(crate) struct LockGuard {
    file: File,
    contention: Contention,
}

impl LockGuard {
    /// Whether this acquisition had to queue, and for how long.
    pub(crate) fn contention(&self) -> Contention {
        self.contention
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Closing the descriptor a moment from now would release it anyway;
        // unlocking here says so in the code instead of relying on the reader
        // knowing. Nothing is unlinked — see the module docs.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

/// Hold an exclusive inter-process lock on `lock_path` until the guard drops.
///
/// Blocks until the lock is free. The guard reports whether the wait happened;
/// nothing is printed and nothing is announced.
/// Only tests take an unwatched lock -- this module's, and the prune tests that
/// need a launch to be holding one; every flow above wants the callback, so they
/// call [`hold_lock_watching`] directly.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn hold_lock(lock_path: &Path) -> Result<LockGuard, LockError> {
    hold_lock_watching(lock_path, |_| {})
}

/// [`hold_lock`], with `about_to_wait` called once if the lock is already held.
///
/// The callback runs *before* the blocking acquisition, which is the only
/// moment at which "this run is now waiting" can be reported at all — after it
/// returns, the wait is over. It is not called when the lock was free.
pub(crate) fn hold_lock_watching(
    lock_path: &Path,
    about_to_wait: impl FnOnce(WaitStarted),
) -> Result<LockGuard, LockError> {
    // `FnOnce` is the contract the callers were written against -- "this run is
    // now waiting" is said once or not at all -- and the retry below is the one
    // thing that could say it twice. Taking it out of the option is what keeps
    // the second attempt from announcing a wait the first already announced.
    let mut about_to_wait = Some(about_to_wait);
    let mut waited = Duration::ZERO;
    let mut queued = false;

    for _ in 0..ACQUIRE_ATTEMPTS {
        let file = open_lock_file(lock_path)?;

        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(errno) if would_block(errno) => {
                if let Some(announce) = about_to_wait.take() {
                    announce(WaitStarted {
                        lock_path: lock_path.to_path_buf(),
                    });
                }
                queued = true;
                let queued_at = Instant::now();
                flock(&file, FlockOperation::LockExclusive).map_err(|errno| {
                    LockError::Acquire {
                        path: lock_path.to_path_buf(),
                        failure: io::Error::from(errno).into(),
                    }
                })?;
                waited += queued_at.elapsed();
            }
            Err(errno) => {
                return Err(LockError::Acquire {
                    path: lock_path.to_path_buf(),
                    failure: io::Error::from(errno).into(),
                });
            }
        }

        if still_named_by(&file, lock_path) {
            let contention = if queued {
                Contention::Queued { waited }
            } else {
                Contention::WalkedIn
            };
            return Ok(LockGuard { file, contention });
        }
        // A [`reclaim`] took this inode out of the tree while this was queued for
        // it, so holding it excludes nobody: the next arrival creates a fresh file
        // and walks past. Closing the descriptor releases it, and the loop queues
        // against whatever the path names now.
        drop(file);
    }

    Err(LockError::Superseded {
        path: lock_path.to_path_buf(),
        attempts: ACQUIRE_ATTEMPTS,
    })
}

/// Run `work` holding `lock_path`, but only if the lock is free right now.
///
/// A function taking the work rather than a guard that answers "did I get it",
/// because those are not the same guarantee. A block that runs either way needs
/// a check the caller can forget, and forgetting it does the protected work
/// *unlocked* while reading exactly like the correct code. Here the
/// not-acquired case has no body to run: the lock is either held for the whole
/// of `work` or `work` never happens. `None` is that miss, and ignoring it is
/// safe — it reports what happened, it does not protect anything.
///
/// This is what background work uses and [`hold_lock`] is what foreground work
/// uses, and the difference is who is waiting on whom. A launch that waits for
/// a sibling's clone gets the clone; a sweep that waited for a launch would be
/// taxing the very path it exists to keep clear.
///
/// Note what this does **not** buy, because the asymmetry is easy to overstate:
/// it makes the caller never queue, not the lock cheap to hold. Once `work` has
/// started this holds an ordinary exclusive lock, and anything taking the same
/// path with [`hold_lock`] blocks for the whole of `work` — so background work
/// still owes the foreground a bound on how long `work` can run. In one line:
/// **the caller never queues for anyone, and anyone may still queue for the
/// caller.**
///
/// Like [`hold_lock`] it is not reentrant and unlinks nothing — and a miss
/// releases nothing, because there was nothing here to release. It revalidates
/// for [`hold_lock_watching`]'s reason: an inode a [`reclaim`] has taken out of
/// the tree excludes nobody, so work run under one would be running unlocked.
pub(crate) fn run_if_lock_free<T>(
    lock_path: &Path,
    work: impl FnOnce() -> T,
) -> Result<Option<T>, LockError> {
    for _ in 0..ACQUIRE_ATTEMPTS {
        let file = open_lock_file(lock_path)?;

        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(errno) if would_block(errno) => return Ok(None),
            Err(errno) => {
                return Err(LockError::Acquire {
                    path: lock_path.to_path_buf(),
                    failure: io::Error::from(errno).into(),
                });
            }
        }
        if !still_named_by(&file, lock_path) {
            drop(file);
            continue;
        }
        let _guard = LockGuard {
            file,
            contention: Contention::WalkedIn,
        };
        return Ok(Some(work()));
    }

    Err(LockError::Superseded {
        path: lock_path.to_path_buf(),
        attempts: ACQUIRE_ATTEMPTS,
    })
}

/// What became of a lock file a sweep asked to reclaim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reclaimed {
    /// Nothing held it, and it is gone.
    Removed,
    /// Something holds it: a launch keyed by this lock is running right now. The
    /// file stays, which is the whole of what "fail towards keeping" costs here.
    Held,
    /// There was no such file. What a sweep run twice says the second time.
    AlreadyGone,
    /// The OS refused the open or the unlink. Reported rather than raised: a lock
    /// file that would not come away is the leak this closes, not a failed run.
    Refused(OsFailure),
}

/// Unlink `lock_path`, but only while holding the lock it is.
///
/// The reason [`hold_lock_watching`] revalidates, and it revalidates for the same
/// reason. Three properties make it safe, and none is sufficient alone:
///
/// - **It never creates.** A sweep that opened with `O_CREAT` would manufacture
///   the file it then reports removing, so a path already reclaimed would come
///   back as [`Reclaimed::Removed`] every run rather than [`Reclaimed::AlreadyGone`]
///   once.
/// - **It holds the lock across the unlink**, non-blocking, so a launch that is
///   inside its critical section keeps its file — and a launch that is *queued*
///   for it acquires a doomed inode, sees the path no longer names it, and queues
///   again against the live one. That second half is the acquisition's, not this
///   function's, which is why the two were written together.
/// - **It unlinks only the inode it locked.** The flock proves nothing about the
///   *path*: an inode already unlinked has no holders, so locking one always
///   succeeds, and the unlink that followed would take away whatever the path
///   names by then. `a_sweep_does_not_unlink_a_file_that_replaced_the_one_it_locked`
///   is the case, and devlaunch is not the only unlinker — `dl --purge` removes
///   the cache directory entire (`lifecycle::purge`, and docs/cleanup.md says so),
///   holding no lock at all.
///
/// A sweep never queues ([`run_if_lock_free`]'s argument, verbatim): waiting for a
/// launch would be housekeeping taxing the path it exists to keep clear.
pub fn reclaim(lock_path: &Path) -> Reclaimed {
    reclaim_between(lock_path, || {})
}

/// [`reclaim`], with a seam at the one instant that decides whether it is correct.
///
/// `between` runs after the open and before the flock, which is the window the
/// third property above is about. A race is not a thing a test can win by running
/// it often, so the test drives the interleaving instead of hoping for it.
fn reclaim_between(lock_path: &Path, between: impl FnOnce()) -> Reclaimed {
    let file = match OpenOptions::new().read(true).write(true).open(lock_path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Reclaimed::AlreadyGone,
        Err(error) => return Reclaimed::Refused(error.into()),
    };
    between();
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {}
        Err(errno) if would_block(errno) => return Reclaimed::Held,
        Err(errno) => return Reclaimed::Refused(io::Error::from(errno).into()),
    }
    // The lock is on an inode; the unlink below is on a path. Between the open and
    // here the path can have been unlinked and re-created, and the flock would have
    // succeeded anyway, because the inode this holds is detached and detached inodes
    // have no other holders. Unlinking then takes away a file somebody else created,
    // locked and revalidated -- and two launches of one workspace get in.
    if !still_named_by(&file, lock_path) {
        return Reclaimed::AlreadyGone;
    }
    let _guard = LockGuard {
        file,
        contention: Contention::WalkedIn,
    };
    match std::fs::remove_file(lock_path) {
        Ok(()) => Reclaimed::Removed,
        // Somebody else's sweep got there between the open and the unlink. The
        // file is gone, which is what was asked for; whose unlink it was is not a
        // distinction any caller acts on.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Reclaimed::AlreadyGone,
        Err(error) => Reclaimed::Refused(error.into()),
    }
}

/// Whether `file` is still the file `lock_path` names.
///
/// Device and inode together, because an inode number is only unique within a
/// filesystem and the cache directory is one a user can mount anything under.
/// A path that resolves to nothing is a mismatch: there is no file there to be
/// the one this holds.
fn still_named_by(file: &File, lock_path: &Path) -> bool {
    let (Ok(held), Ok(named)) = (file.metadata(), std::fs::metadata(lock_path)) else {
        return false;
    };
    held.dev() == named.dev() && held.ino() == named.ino()
}

/// Open (creating if needed) the lock file, and its directory with it.
fn open_lock_file(lock_path: &Path) -> Result<File, LockError> {
    if let Some(parent) = lock_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| LockError::CreateParent {
            path: parent.to_path_buf(),
            failure: source.into(),
        })?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(LOCK_FILE_MODE)
        .open(lock_path)
        .map_err(|source| LockError::Open {
            path: lock_path.to_path_buf(),
            failure: source.into(),
        })
}

/// Whether an `flock` errno means "another open file description holds it".
///
/// `EWOULDBLOCK` and `EAGAIN` are the same value on Linux; both are named
/// because the contract is the constant, not the number.
fn would_block(errno: rustix::io::Errno) -> bool {
    errno == rustix::io::Errno::WOULDBLOCK || errno == rustix::io::Errno::AGAIN
}

/// The permission bits of `path`, for tests and callers that check privacy.
#[cfg(test)]
fn file_mode(path: &Path) -> io::Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o7777)
}

#[cfg(test)]
mod tests {
    //! The four promises `locks.py` makes, in the order it makes them:
    //!
    //! 1. **Mutual exclusion**, in one process and across two, with the lock
    //!    freed however the critical section ends — including by a panic.
    //! 2. **Contention is reported**, which is what tells a launch its earlier
    //!    reads may be stale.
    //! 3. **A dead holder releases it** — the whole argument for `flock` over a
    //!    pid file.
    //! 4. **An acquisition unlinks nothing, and holds only the inode the path
    //!    still names**, both asserted by inode: a delete-and-recreate leaves a
    //!    path that exists and a guarantee that does not, and the guarantee is
    //!    what [`reclaim`] is allowed to unlink underneath.
    //!
    //! **Every wait here is bounded.** The regressions these catch — a leaked
    //! descriptor, a lock not released — make `flock` block forever rather than
    //! return something wrong, so an unbounded wait would hang the suite
    //! instead of failing it.

    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;

    /// What any bounded step gets before it is called wedged. Nothing here
    /// contends for longer than a test takes to notice, so a correct lock
    /// returns in microseconds and this is only ever reached by a broken one.
    const BOUND: Duration = Duration::from_secs(10);

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temp dir")
    }

    // --- claim 1: mutual exclusion ---------------------------------------

    #[test]
    fn an_uncontended_acquisition_walks_straight_in() {
        let dir = temp_dir();
        let lock = dir.path().join("repo.lock");
        let mut announced = 0;

        let guard = hold_lock_watching(&lock, |_| announced += 1).expect("the lock");

        assert_eq!(guard.contention(), Contention::WalkedIn);
        assert!(!guard.contention().waited());
        drop(guard);
        assert_eq!(announced, 0, "a run that walked in has nothing to report");
    }

    #[test]
    fn a_second_open_of_the_same_path_cannot_take_it_while_held() {
        // The per-OFD pin. Two `File`s on one path are two open file
        // descriptions, so this conflicts inside one process exactly as it
        // would across two — which is why the rest of these tests need no
        // subprocess, and why re-locking one path in one process deadlocks.
        let dir = temp_dir();
        let lock = dir.path().join("repo.lock");
        let held = hold_lock(&lock).expect("the lock");

        let mut ran = 0;
        let outcome = run_if_lock_free(&lock, || ran += 1).expect("no error");

        assert!(outcome.is_none(), "the second description must not get in");
        assert_eq!(ran, 0, "the work of a missed acquisition never runs");
        drop(held);
    }

    #[test]
    fn the_lock_is_free_again_once_the_guard_is_dropped() {
        // It is a lock, not a latch. If the first acquisition leaked its
        // descriptor the second would block on it — the documented
        // non-reentrancy, pointed at the wrong target.
        let dir = temp_dir();
        let lock = dir.path().join("cache").join("repo.lock");

        let first = hold_lock(&lock).expect("the first acquisition");
        assert_eq!(first.contention(), Contention::WalkedIn);
        drop(first);

        let second = hold_lock(&lock).expect("the second acquisition");
        assert_eq!(
            second.contention(),
            Contention::WalkedIn,
            "the first acquisition let go"
        );
    }

    #[test]
    fn a_panicking_critical_section_still_releases_the_lock() {
        // The critical sections this guards are clone and metadata writes, the
        // operations most likely to fail, and a dl that kept the repo lock
        // after one did would wedge every sibling waiting on that repo.
        let dir = temp_dir();
        let lock = dir.path().join("repo.lock");

        let panicked = std::panic::catch_unwind(|| {
            let _held = hold_lock(&lock).expect("the lock");
            panic!("the critical section failed");
        });
        assert!(panicked.is_err(), "the block really did panic");

        let after = hold_lock(&lock).expect("the acquisition after the panic");
        assert_eq!(after.contention(), Contention::WalkedIn);
    }

    #[test]
    fn a_second_process_is_held_off_and_a_dead_one_leaves_nothing_behind() {
        // Claims 1 and 3 across a real process boundary. `flock(1)` from
        // util-linux is the second holder: it takes the same advisory lock on
        // the same file, so what it queues behind is the kernel's lock rather
        // than this test's idea of one.
        let dir = temp_dir();
        let lock = dir.path().join("repo.lock");
        let mut holder = spawn_holder(&lock);

        wait_until(|| run_if_lock_free(&lock, || ()).expect("no error").is_none());

        assert!(
            run_if_lock_free(&lock, || ()).expect("no error").is_none(),
            "another process holds it"
        );

        // SIGKILL: no cleanup, no close, nothing run on the way out — what an
        // OOM kill or a pulled power cord looks like. A pid file survives that
        // and wedges the cache until a human deletes it.
        holder.kill().expect("the holder is killable");
        holder.wait().expect("reaping the holder");

        wait_until(|| run_if_lock_free(&lock, || ()).expect("no error").is_some());
    }

    // --- claim 2: contention is reported ---------------------------------

    #[test]
    fn a_contended_acquisition_announces_the_wait_and_then_reports_it() {
        // The contender is a thread with its own open file description, so it
        // queues behind this one for real. What makes the test deterministic
        // rather than a sleep is the announcement: the callback fires exactly
        // when the lock was found held and before the blocking call, so
        // receiving it proves the contender is queued — no `/proc` peeking, no
        // settle time.
        let dir = temp_dir();
        let lock = dir.path().join("cache").join("repo.lock");
        let held = hold_lock(&lock).expect("the lock");

        let (announced_tx, announced_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let contender_lock = lock.clone();
        let contender = std::thread::spawn(move || {
            let guard = hold_lock_watching(&contender_lock, |wait| {
                announced_tx.send(wait).expect("the parent is listening");
            })
            .expect("the lock, eventually");
            done_tx
                .send(guard.contention())
                .expect("the parent listens");
        });

        let announced = announced_rx
            .recv_timeout(BOUND)
            .expect("the contender must announce that it is about to wait");
        assert_eq!(announced, WaitStarted { lock_path: lock });
        assert!(
            done_rx.try_recv().is_err(),
            "the contender entered the critical section while the holder was inside it"
        );

        drop(held);
        let contention = done_rx
            .recv_timeout(BOUND)
            .expect("the contender never got the lock after it was released");
        match contention {
            Contention::Queued { .. } => {}
            Contention::WalkedIn => panic!("a run that queued must say so"),
        }
        assert!(contention.waited());
        contender.join().expect("the contender finished");
    }

    // --- claim 4, first half: an acquisition unlinks nothing --------------

    #[test]
    fn the_lock_file_outlives_the_guard_with_the_same_inode() {
        // Two arrivals that lock different inodes both "hold" the lock and
        // neither can see the other, so an *acquisition* replacing the file
        // would be the self-defeating move — and it is asserted by inode,
        // because a delete-and-recreate leaves a path that exists and a
        // guarantee that does not. `reclaim` is the one unlink, and it takes the
        // lock before it performs one.
        let dir = temp_dir();
        let lock = dir.path().join("repo.lock");

        let guard = hold_lock(&lock).expect("the lock");
        assert!(lock.exists(), "the lock file exists while it is held");
        let inode = lock.metadata().expect("a stat").ino();
        drop(guard);

        assert!(
            lock.exists(),
            "the lock file outlives the guard that held it"
        );
        assert_eq!(
            lock.metadata().expect("a stat").ino(),
            inode,
            "the same inode, so a later arrival queues behind it"
        );
    }

    #[test]
    fn the_parent_directory_is_created_on_demand() {
        // A first launch locks a repo directory that does not exist yet: the
        // lock has to be takeable *before* the thing it protects is built —
        // that is the race it exists for.
        let dir = temp_dir();
        let lock = dir
            .path()
            .join("not")
            .join("yet")
            .join("there")
            .join("repo.lock");
        assert!(!lock.parent().expect("a parent").exists());

        let guard = hold_lock(&lock).expect("the lock");
        drop(guard);

        assert!(lock.exists());
    }

    #[test]
    fn a_lock_file_this_created_is_not_world_readable() {
        // Scoped to creation, because that is all `open` can promise: the mode
        // is ignored for a file that already exists. Asserted as "no group or
        // other bits" rather than an exact mode, since the ambient umask can
        // only take bits away.
        let dir = temp_dir();
        let lock = dir.path().join("repo.lock");

        drop(hold_lock(&lock).expect("the lock"));

        let mode = file_mode(&lock).expect("a stat");
        assert_eq!(mode & 0o077, 0, "no group or other bits on the lock file");
    }

    // --- run_if_lock_free ------------------------------------------------

    #[test]
    fn work_on_a_free_lock_runs_and_comes_back() {
        let dir = temp_dir();

        let outcome =
            run_if_lock_free(&dir.path().join("repo").join(".lock"), || "swept").expect("no error");

        assert_eq!(outcome, Some("swept"));
    }

    #[test]
    fn the_lock_really_is_held_while_the_work_runs() {
        // Otherwise the work is unlocked and only looks protected.
        let dir = temp_dir();
        let lock = dir.path().join(".lock");

        let inner = run_if_lock_free(&lock, || run_if_lock_free(&lock, || ()).expect("no error"))
            .expect("no error");

        assert_eq!(
            inner,
            Some(None),
            "the work ran, and while it ran nobody else could take the lock"
        );
    }

    #[test]
    fn a_panicking_work_still_releases_the_lock() {
        // A failed background fetch must not wedge the repo for every later run.
        let dir = temp_dir();
        let lock = dir.path().join(".lock");

        let panicked = std::panic::catch_unwind(|| {
            let _ = run_if_lock_free(&lock, || panic!("fetch failed"));
        });
        assert!(panicked.is_err());

        let after = hold_lock(&lock).expect("the lock");
        assert_eq!(after.contention(), Contention::WalkedIn);
    }

    #[test]
    fn a_sweep_that_took_the_lock_hands_it_back() {
        let dir = temp_dir();
        let lock = dir.path().join(".lock");

        run_if_lock_free(&lock, || ()).expect("no error");

        let after = hold_lock(&lock).expect("the lock");
        assert_eq!(after.contention(), Contention::WalkedIn);
    }

    #[test]
    fn the_lock_file_outlives_the_work() {
        let dir = temp_dir();
        let lock = dir.path().join(".lock");

        run_if_lock_free(&lock, || ()).expect("no error");

        assert!(lock.exists(), "unlinking it is the self-defeating move");
    }

    #[test]
    fn a_lock_not_acquired_is_not_released_out_from_under_its_holder() {
        // Leaving must not drop the other holder's lock, however many times a
        // sweep steps over it.
        let dir = temp_dir();
        let lock = dir.path().join(".lock");
        let held = hold_lock(&lock).expect("the lock");

        assert!(run_if_lock_free(&lock, || ()).expect("no error").is_none());
        assert!(run_if_lock_free(&lock, || ()).expect("no error").is_none());

        drop(held);
    }

    #[test]
    fn a_missed_acquisition_never_queues() {
        // The whole point of the non-blocking helper: a contended sweep skips
        // instead of queueing. Bounded by the channel, so a helper that started
        // waiting fails the test instead of hanging it.
        let dir = temp_dir();
        let lock = dir.path().join(".lock");
        let held = hold_lock(&lock).expect("the lock");

        let (tx, rx) = mpsc::channel();
        let sweep_lock = lock.clone();
        let sweep = std::thread::spawn(move || {
            let outcome = run_if_lock_free(&sweep_lock, || ()).expect("no error");
            tx.send(outcome).expect("the parent listens");
        });

        let outcome = rx
            .recv_timeout(BOUND)
            .expect("run_if_lock_free waited on a held lock");
        assert_eq!(outcome, None);
        sweep.join().expect("the sweep finished");
        drop(held);
    }

    // --- claim 4, second half: reclaim, and the revalidation it needs -----

    #[test]
    fn a_free_lock_file_is_reclaimed() {
        // The leak devlaunch#575 measured: 18 of 26 launch locks named no live
        // workspace, and nothing short of `--purge` reached them.
        let dir = temp_dir();
        let lock = dir.path().join("gone-workspace.lock");
        drop(hold_lock(&lock).expect("the lock"));

        assert_eq!(reclaim(&lock), Reclaimed::Removed);
        assert!(!lock.exists());
    }

    #[test]
    fn a_lock_somebody_holds_is_left_exactly_where_it_is() {
        // Principle 1 of devlaunch#444 at its narrowest: where the check cannot
        // prove the file abandoned, fail towards keeping it. A launch is inside
        // its critical section and this is housekeeping.
        let dir = temp_dir();
        let lock = dir.path().join("live-workspace.lock");
        let held = hold_lock(&lock).expect("the lock");

        assert_eq!(reclaim(&lock), Reclaimed::Held);
        assert!(lock.exists(), "a held lock file stays");
        drop(held);
    }

    #[test]
    fn reclaiming_a_lock_that_is_not_there_creates_nothing() {
        // A sweep that opened with O_CREAT would report removing a file it had
        // just made, every run, for every path already reclaimed.
        let dir = temp_dir();
        let lock = dir.path().join("never-existed.lock");

        assert_eq!(reclaim(&lock), Reclaimed::AlreadyGone);
        assert!(
            !lock.exists(),
            "the sweep did not manufacture its own subject"
        );
    }

    #[test]
    fn a_reclaimed_lock_is_taken_again_by_the_next_launch() {
        // Reclaiming is not retiring: the workspace id can come back, and the
        // lock has to be there for it when it does.
        let dir = temp_dir();
        let lock = dir.path().join("relaunched.lock");
        drop(hold_lock(&lock).expect("the first launch"));
        assert_eq!(reclaim(&lock), Reclaimed::Removed);

        let after = hold_lock(&lock).expect("the launch after the sweep");

        assert_eq!(after.contention(), Contention::WalkedIn);
        assert!(lock.exists());
    }

    #[test]
    fn a_waiter_whose_file_was_unlinked_under_it_still_excludes_a_newcomer() {
        // The hazard the whole revalidation exists for, and the one that used to
        // be closed by never unlinking at all. A run queued on the lock acquires
        // an inode that is no longer in the tree; without the re-check it returns
        // holding a lock nobody else can see, and the next arrival creates a
        // fresh file and walks straight past it. Two runs then both believe they
        // hold the workspace, which is the pair of `devpod up`s the lock exists
        // to prevent.
        //
        // Deterministic without a sleep: the contender's announcement fires
        // exactly when it has found the lock held and is about to block, so
        // receiving it proves the contender is queued on *this* inode.
        let dir = temp_dir();
        let lock = dir.path().join("raced.lock");
        let held = hold_lock(&lock).expect("the lock");

        let (announced_tx, announced_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let contender_lock = lock.clone();
        let contender = std::thread::spawn(move || {
            let guard = hold_lock_watching(&contender_lock, |wait| {
                announced_tx.send(wait).expect("the parent is listening");
            })
            .expect("the lock, eventually");
            done_tx.send(()).expect("the parent listens");
            // Held until the parent has had its say, which is what the parent is
            // asserting about.
            std::thread::sleep(Duration::from_millis(200));
            drop(guard);
        });

        announced_rx
            .recv_timeout(BOUND)
            .expect("the contender must announce that it is about to wait");
        std::fs::remove_file(&lock).expect("unlinking the inode the contender is queued on");
        drop(held);
        done_rx
            .recv_timeout(BOUND)
            .expect("the contender never got the lock");

        assert!(
            run_if_lock_free(&lock, || ()).expect("no error").is_none(),
            "a newcomer walked in beside a run that believes it holds the lock"
        );
        contender.join().expect("the contender finished");
    }

    #[test]
    fn an_unlinked_or_replaced_inode_is_not_the_one_the_path_names() {
        // The predicate both acquisitions branch on, pinned directly: the
        // end-to-end race above proves it is consulted, and this proves it
        // answers. Both mismatch shapes are here because they arrive from
        // different sides -- an unlink that nothing has replaced yet, and a
        // replacement that arrived first -- and a check that caught only the
        // second would pass a waiter straight through the window the first opens.
        let dir = temp_dir();
        let lock = dir.path().join("replaced.lock");

        let held = open_lock_file(&lock).expect("the file");
        assert!(still_named_by(&held, &lock), "nothing has moved yet");

        std::fs::remove_file(&lock).expect("unlinking it");
        assert!(
            !still_named_by(&held, &lock),
            "an unlinked inode is named by no path"
        );

        let _fresh = open_lock_file(&lock).expect("the replacement");
        assert!(
            !still_named_by(&held, &lock),
            "the path names the replacement, not the inode this holds"
        );
    }

    #[test]
    fn work_runs_under_the_lock_file_that_was_there_all_along() {
        // The other side of the predicate, at the call site: an untouched lock
        // file must not be replaced or re-taken by the revalidation itself.
        let dir = temp_dir();
        let lock = dir.path().join("swept.lock");
        drop(hold_lock(&lock).expect("creating the file"));
        let before = lock.metadata().expect("a stat").ino();

        let mut inode_work_ran_under = None;
        let outcome = run_if_lock_free(&lock, || {
            inode_work_ran_under = Some(lock.metadata().expect("a stat").ino());
        });

        assert!(outcome.expect("no error").is_some(), "the work ran");
        assert_eq!(inode_work_ran_under, Some(before));
    }

    #[test]
    fn a_reclaim_and_an_acquisition_do_not_both_get_in() {
        // The two halves against each other, in the order that would break:
        // reclaim takes the lock before it unlinks, so a launch that is inside
        // its critical section is never swept out from under.
        let dir = temp_dir();
        let lock = dir.path().join("contested.lock");

        let seen = run_if_lock_free(&lock, || reclaim(&lock)).expect("no error");

        assert_eq!(seen, Some(Reclaimed::Held));
        assert!(lock.exists());
    }

    /// The case `a_reclaim_and_an_acquisition_do_not_both_get_in` cannot reach.
    ///
    /// That one starts the acquisition first, so reclaim's flock is refused and the
    /// sweep never gets as far as the unlink. Here the sweep opens *first* and the
    /// path is replaced under it, which is what `--purge` unlinking the cache
    /// directory does, and what any second sweep does. The flock then succeeds --
    /// nothing holds a detached inode -- and an unlink by path would take away the
    /// live holder's file, leaving two launches of one workspace holding the lock.
    #[test]
    fn a_sweep_does_not_unlink_a_file_that_replaced_the_one_it_locked() {
        let dir = temp_dir();
        let lock = dir.path().join("swapped.lock");
        drop(hold_lock(&lock).expect("creating the file"));
        let doomed = lock.metadata().expect("a stat").ino();

        let mut holder = None;
        let outcome = reclaim_between(&lock, || {
            // The swap, in the window between the sweep's open and its flock.
            std::fs::remove_file(&lock).expect("unlinking the file the sweep opened");
            let live = spawn_holder(&lock);
            wait_until(|| lock.exists() && lock.metadata().expect("a stat").ino() != doomed);
            wait_until(|| lock_is_held(&lock));
            holder = Some(live);
        });

        let mut holder = holder.expect("the replacement's holder");
        assert_eq!(outcome, Reclaimed::AlreadyGone);
        assert!(
            lock.exists(),
            "the sweep unlinked a file it never held the lock on"
        );
        assert!(
            lock_is_held(&lock),
            "the replacement's holder still holds it"
        );
        holder.kill().expect("killing the holder");
        holder.wait().expect("reaping the holder");
    }

    // --- helpers ---------------------------------------------------------

    /// A second *process* holding an exclusive lock on `lock` until killed.
    ///
    /// One process holding one descriptor, which takes some care: `flock(1)`
    /// given a command forks, and the fork inherits the locked descriptor, so
    /// killing what was spawned would leave the lock held by a survivor — the
    /// opposite of what claim 3 is about. The shell takes the lock on a
    /// descriptor of its own and then `exec`s, so the process that ends up
    /// holding it is the one this test can kill.
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
            .expect("a shell and flock(1) from util-linux, for the cross-process claims")
    }

    /// Whether somebody else holds `lock` right now.
    ///
    /// `run_if_lock_free` answers without queuing, which is what makes it usable as
    /// a probe: a held lock comes back `Ok(None)` on the first refused flock.
    fn lock_is_held(lock: &Path) -> bool {
        run_if_lock_free(lock, || ()).expect("no error").is_none()
    }

    /// Poll `condition` until it holds, or fail rather than hang.
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
}
