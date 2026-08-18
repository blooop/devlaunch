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
//! - **The lock file is never deleted.** Unlinking an flock'd file is the
//!   classic self-defeating move: a process that opened the old inode still
//!   "holds" a lock nobody else can see, while new arrivals lock a fresh file
//!   and walk straight past it. A few empty `.lock` files in the cache are the
//!   price of the guarantee; `dl --purge` sweeps them away with everything else.
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

// The consumers of these locks are the storage flows (M4) and the launch path
// (M7), which are not ported yet: until then the typed failure data has no
// reader outside the tests. Remove this when they land.
#![allow(dead_code)]

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::fs::{FlockOperation, flock};

/// The permission bits a lock file this module creates is born with.
///
/// The shared cache is per-user state under `$XDG_CACHE_HOME`; a lock file
/// another user can open is a lock another user can hold, which on a shared box
/// is a way to stop somebody else's dl for free. Creation is all this can
/// promise: the mode is ignored for a file that already exists, and no lock
/// file is ever deleted, so one that arrived with looser bits keeps them.
const LOCK_FILE_MODE: u32 = 0o600;

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
#[derive(Debug)]
pub enum LockError {
    /// The lock file's directory could not be created. A first launch locks a
    /// repo directory that does not exist yet, so this is a real step.
    CreateParent { path: PathBuf, source: io::Error },
    /// The lock file could not be opened or created.
    Open { path: PathBuf, source: io::Error },
    /// `flock` failed for a reason that is not "somebody else holds it".
    Acquire { path: PathBuf, source: io::Error },
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
    let file = open_lock_file(lock_path)?;

    let contention = match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Contention::WalkedIn,
        Err(errno) if would_block(errno) => {
            about_to_wait(WaitStarted {
                lock_path: lock_path.to_path_buf(),
            });
            let queued_at = Instant::now();
            flock(&file, FlockOperation::LockExclusive).map_err(|errno| LockError::Acquire {
                path: lock_path.to_path_buf(),
                source: io::Error::from(errno),
            })?;
            Contention::Queued {
                waited: queued_at.elapsed(),
            }
        }
        Err(errno) => {
            return Err(LockError::Acquire {
                path: lock_path.to_path_buf(),
                source: io::Error::from(errno),
            });
        }
    };

    Ok(LockGuard { file, contention })
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
/// Like [`hold_lock`] it is not reentrant and never unlinks the lock file — and
/// a miss releases nothing, because there was nothing here to release.
pub(crate) fn run_if_lock_free<T>(
    lock_path: &Path,
    work: impl FnOnce() -> T,
) -> Result<Option<T>, LockError> {
    let file = open_lock_file(lock_path)?;

    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {}
        Err(errno) if would_block(errno) => return Ok(None),
        Err(errno) => {
            return Err(LockError::Acquire {
                path: lock_path.to_path_buf(),
                source: io::Error::from(errno),
            });
        }
    }
    let _guard = LockGuard {
        file,
        contention: Contention::WalkedIn,
    };
    Ok(Some(work()))
}

/// Open (creating if needed) the lock file, and its directory with it.
fn open_lock_file(lock_path: &Path) -> Result<File, LockError> {
    if let Some(parent) = lock_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| LockError::CreateParent {
            path: parent.to_path_buf(),
            source,
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
            source,
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
    //! 4. **The lock file is never unlinked**, asserted by inode: a
    //!    delete-and-recreate leaves a path that exists and a guarantee that
    //!    does not.
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

    // --- claim 4: the lock file is never unlinked ------------------------

    #[test]
    fn the_lock_file_outlives_the_guard_with_the_same_inode() {
        // Two arrivals that lock different inodes both "hold" the lock and
        // neither can see the other, so survival is load-bearing rather than
        // housekeeping — and it is asserted by inode, because a
        // delete-and-recreate leaves a path that exists and a guarantee that
        // does not.
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
        // is ignored for a file that already exists, and no lock file is ever
        // deleted. Asserted as "no group or other bits" rather than an exact
        // mode, since the ambient umask can only take bits away.
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
