//! Async-signal-safe cleanup for a `dl` that is killed mid-flight.
//!
//! `dl`'s disposition for SIGINT, SIGTERM and SIGHUP alike is `_exit(128 +
//! signo)` — 130, 143, 129 — after the cleanup below (see `dl`'s
//! `install_signal_handlers`, which both binaries call from `main`). A signal
//! handler may do almost nothing — not allocate, not lock a mutex, not call a
//! libc function outside the async-signal-safe list — so it cannot run
//! destructors. Python got its cleanup for free, because a `KeyboardInterrupt`
//! *unwinds*: the `with` blocks that staged the GitHub token, wrote the metadata
//! temp and unpacked the tools bundle all ran their `finally`. `_exit` runs
//! none of that, so a Ctrl-C during the minutes-long `devpod up` used to leave
//! the plaintext `GH_TOKEN` file on disk (concurrency review F2/H4/R8) and the
//! `devpod up` child orphaned, still holding the build while `dl` — and the
//! launch lock it released on exit — were already gone (F3).
//!
//! This module is the missing `finally`, expressed as the only things a handler
//! is allowed to do about a file and a child: `unlink(2)`/`rmdir(2)` a path, and
//! `killpg(2)`/`kill(2)` a process group or a single detached child, with
//! `waitpid(2)` and `poll(2)` to give the group's leader a moment to act on its
//! SIGTERM before the SIGKILL. All of them are on POSIX's async-signal-safe list.
//!
//! # The registry is lock-free by construction
//!
//! A handler that took a lock could deadlock against the very thread it
//! interrupted (which may hold that lock), so nothing here locks. The live
//! paths are a fixed-size array of [`AtomicPtr`]; a registration claims a free
//! slot with a single compare-and-swap and a drop releases it with a single
//! store. The handler only ever *reads* the slots and calls the async-signal-safe
//! syscall on each non-null pointer. There is no dynamic structure to walk and
//! no allocator to re-enter.
//!
//! # Why a registration leaks its `CString`
//!
//! The pointer a slot holds is a `CString` handed over by [`CString::into_raw`].
//! A drop nulls the slot but **does not** reclaim it: freeing would race the
//! handler, which may read the slot on another thread and then `unlink` freed
//! memory, and a handler cannot take the lock that would make that safe. The
//! leak it trades for is bounded and cheap — a handful of short-lived paths in a
//! process that is about to exit — and a pointer the handler reads in the window
//! between a drop's slot-null and "not yet reached" names a random tempfile that
//! the ordinary drop has already removed, so the `unlink`/`rmdir` is a harmless
//! `ENOENT`. Correctness of the credential cleanup is worth a few dozen leaked
//! bytes.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicI32, AtomicPtr, Ordering};

/// How many temp *files* can be registered for unlink at once. Only a few are
/// ever live together — the staged token, the metadata save's temp, the tools
/// bundle — so this is comfortably oversized rather than tuned.
const FILE_SLOTS: usize = 16;

/// How many temp *directories* can be registered for rmdir at once. Only the
/// tools staging directory is ever a directory, so a handful is plenty.
const DIR_SLOTS: usize = 4;

/// How many *detached* children can be registered for a signal at once. One at a
/// time in practice -- the session manager's socket forward -- so this is slack.
const PID_SLOTS: usize = 4;

/// Paths the handler `unlink`s. `null` means the slot is free.
static FILES: [AtomicPtr<libc::c_char>; FILE_SLOTS] =
    [const { AtomicPtr::new(ptr::null_mut()) }; FILE_SLOTS];

/// Paths the handler `rmdir`s, after every file above is gone, so a staging
/// directory whose one file was itself registered comes away empty.
static DIRS: [AtomicPtr<libc::c_char>; DIR_SLOTS] =
    [const { AtomicPtr::new(ptr::null_mut()) }; DIR_SLOTS];

/// Pids the handler `kill`s. `0` means the slot is free.
///
/// Separate from [`FOREGROUND_PGID`] because these children are in sessions of
/// their own: `Runner::detach` `setsid`s, so a group-wide Ctrl-C never reaches
/// them and the foreground group the handler signals does not contain them.
static PIDS: [AtomicI32; PID_SLOTS] = [const { AtomicI32::new(0) }; PID_SLOTS];

/// The process group of the foreground child this process is waiting on, or `0`
/// for "none". The handler `killpg`s it so a `devpod up` cannot outlive the
/// `dl` that started it and go on holding a build after the launch lock is gone.
static FOREGROUND_PGID: AtomicI32 = AtomicI32::new(0);

/// A live registration. While it exists the handler will clean the path up; when
/// it drops, the slot is freed for reuse (but the backing `CString` is leaked on
/// purpose — see the module docs).
///
/// `#[must_use]`: dropping it immediately would unregister the path at once,
/// which is never what a caller means.
#[derive(Debug)]
#[must_use = "the path is registered only while this value is held"]
pub struct Registration {
    slot: Slot,
}

/// Which pool a [`Registration`] holds a slot in.
#[derive(Debug)]
enum Slot {
    Path(&'static AtomicPtr<libc::c_char>),
    Pid(&'static AtomicI32),
}

impl Drop for Registration {
    fn drop(&mut self) {
        // Free the slot for reuse. For a path, deliberately not
        // `CString::from_raw` — see the module docs on why the backing allocation
        // is leaked rather than freed.
        match self.slot {
            Slot::Path(slot) => slot.store(ptr::null_mut(), Ordering::SeqCst),
            Slot::Pid(slot) => slot.store(0, Ordering::SeqCst),
        }
    }
}

/// Register `path` to be `unlink`ed if this process is interrupted.
///
/// Returns `None` if every slot is full (more than [`FILE_SLOTS`] live at once,
/// which no `dl` flow reaches) or the path contains a NUL. A `None` costs that
/// one path its interrupt-time cleanup and nothing else; the ordinary drop of
/// the tempfile still removes it on a clean exit.
pub fn register_file(path: &Path) -> Option<Registration> {
    register_in(&FILES, path)
}

/// Register `path` to be `rmdir`ed if this process is interrupted. `rmdir`
/// removes only an empty directory, so register the files inside it with
/// [`register_file`] too — the handler unlinks files before it rmdirs
/// directories.
pub fn register_dir(path: &Path) -> Option<Registration> {
    register_in(&DIRS, path)
}

/// Register `pid` to be sent `SIGTERM` if this process is interrupted.
///
/// For a child started with [`crate::Runner::detach`], which `setsid`s: a
/// terminal's Ctrl-C goes to the foreground process group and a detached child is
/// not in it, and `dl`'s handler `_exit`s without unwinding, so nothing else takes
/// such a child down. Without this a `dl` that is interrupted leaves it running
/// indefinitely -- one per interrupted launch.
///
/// Returns `None` if every slot is full, which costs that one child its
/// interrupt-time signal and nothing else. Drop the registration when the child
/// has been reaped or signalled, so the handler cannot name a recycled pid.
pub fn register_pid(pid: i32) -> Option<Registration> {
    if pid <= 0 {
        return None;
    }
    for slot in &PIDS {
        if slot
            .compare_exchange(0, pid, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Some(Registration {
                slot: Slot::Pid(slot),
            });
        }
    }
    None
}

fn register_in(slots: &'static [AtomicPtr<libc::c_char>], path: &Path) -> Option<Registration> {
    // The allocation happens here, in ordinary control flow — never in the
    // handler.
    let raw = CString::new(path.as_os_str().as_bytes()).ok()?.into_raw();
    for slot in slots {
        if slot
            .compare_exchange(ptr::null_mut(), raw, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Some(Registration {
                slot: Slot::Path(slot),
            });
        }
    }
    // No free slot: reclaim the string we could not place. Safe to free because
    // it was never stored anywhere the handler can see.
    // SAFETY: `raw` came from `CString::into_raw` just above and was not shared.
    drop(unsafe { CString::from_raw(raw) });
    None
}

/// Have the child `command` spawns take `signal` when this process dies, however
/// it dies.
///
/// The handler below is what tears a `devpod up` down when `dl` is *told* to stop,
/// and it is not enough on its own. A SIGKILL runs no handler at all, and a
/// `devpod up` that takes the handler's SIGTERM and does not act on it outlives
/// the `_exit` behind it. Either way the child is reparented to init still holding
/// devpod's workspace flock, and every later `dl <ws>`, `rm` and `devpod delete`
/// waits on it. That was measured on a host: two such orphans of a `dla
/// --boot-up` sat for four hours, and the kernel log showed no OOM kill. Linux's
/// `PR_SET_PDEATHSIG` is the kernel's own answer, and it does not depend on this
/// process getting to run anything.
///
/// **The race it leaves, and how it is closed.** The parent can die after the fork
/// and before the `prctl`, and then the child is already init's and no death is
/// left to signal it. So the pid is read here, before the fork, and the child
/// compares its parent with it once the `prctl` is in: a mismatch means the parent
/// is gone, and the child `_exit`s instead of `exec`ing.
///
/// **The kernel watches the parent *thread*, not the process.** The signal fires
/// when the thread that forked exits, so a child spawned from a short-lived thread
/// would be killed when that thread ends. Every child this is set on is spawned
/// from `main` in both binaries: the launch's `devpod up` and `dla`'s boot child.
/// A caller that spawns from another thread has to keep that thread alive for as
/// long as the child should live.
///
/// A no-op off Linux, where there is no `PR_SET_PDEATHSIG`: the handler's group
/// kill is all those hosts get.
pub fn ends_with_this_process(command: &mut std::process::Command, signal: i32) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: `getpid` reads a value and touches nothing.
        let parent = unsafe { libc::getpid() };
        // SAFETY: `prctl`, `getppid` and `_exit` are bare syscalls, which is all a
        // pre-exec hook may make: no allocation, no locks. The closure captures two
        // integers by value.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, signal as libc::c_ulong, 0, 0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (command, signal);
    }
}

/// Record the process group of the foreground child now being waited on, so the
/// interrupt handler can tear it down. Paired with [`clear_foreground_child`]
/// once the child is reaped.
pub(crate) fn note_foreground_child(pgid: i32) {
    FOREGROUND_PGID.store(pgid, Ordering::SeqCst);
}

/// Forget the foreground child: it has been reaped, so the handler must not
/// signal its (possibly recycled) process group id.
pub(crate) fn clear_foreground_child() {
    FOREGROUND_PGID.store(0, Ordering::SeqCst);
}

/// Do everything the interrupt handler is allowed to do, then `_exit(code)`.
///
/// # Safety
///
/// Async-signal-safe, and only that: it calls `killpg`, `waitpid`, `poll`,
/// `unlink`, `rmdir` and `_exit`, all on POSIX's async-signal-safe list, and reads
/// only lock-free atomics. It can take up to two seconds, while a foreground
/// child that took its SIGTERM unwinds. It must be called **only** from a signal handler (it never
/// returns). Calling it from ordinary code would end the process without
/// flushing anything.
pub unsafe fn cleanup_and_exit(code: i32) -> ! {
    // SAFETY: every call below is async-signal-safe; see the function contract.
    unsafe {
        drain();
        libc::_exit(code);
    }
}

/// The cleanup half of [`cleanup_and_exit`], without the exit — so a test can
/// observe its effect. Async-signal-safe on its own.
///
/// # Safety
///
/// Same contract as [`cleanup_and_exit`]: async-signal-safe, reads only
/// lock-free atomics and calls only async-signal-safe syscalls.
unsafe fn drain() {
    // The terminal, because `dl` is about to `_exit` without unwinding and a
    // session child holding modes on it has no one else left to undo them.
    // `restore` is async-signal-safe on the same terms as everything below —
    // `isatty`, `write`, and no allocation.
    //
    // Its position among the three is arbitrary and deliberately not argued for:
    // the child below is waited for only briefly, and what is left of its group
    // can still be writing to this terminal as it dies. Repairing after the kill
    // would not close that, and nothing here can.
    crate::terminal::restore();
    // The child next: killing the `devpod up` group before unlinking means the
    // build is already on its way down by the time the token it was handed is
    // gone, closing the window in which an orphan could still read it.
    let pgid = FOREGROUND_PGID.load(Ordering::SeqCst);
    if pgid > 0 {
        // SAFETY: `killpg` is async-signal-safe. The one hazard is a stale pgid:
        // the child was reaped, and `clear_foreground_child` — which runs just
        // after the wait returns, not before it — has not stored the zero yet. In
        // that window the group is empty, so `killpg` fails ESRCH and nothing
        // happens; for it to name a *live* group instead, the kernel would have
        // had to recycle that pid as a new group leader within those few
        // instructions, which needs the whole pid space to wrap first.
        //
        // Deliberately not argued from which signal delivered this: any of the
        // three handled signals can arrive here, and a group-wide or cgroup-wide
        // SIGTERM reaches the child as well as `dl`. The bound above holds
        // whatever woke the handler, which is why it is stated in terms of the
        // reap rather than the sender.
        unsafe {
            libc::killpg(pgid, libc::SIGTERM);
        }
        // Then a short wait for the group's leader, and SIGKILL for whatever is
        // left. Two reasons, and both are about the `devpod up` that holds the
        // workspace's flock. On Linux that child also takes a SIGKILL from the
        // kernel the moment this process exits ([`ends_with_this_process`]), so
        // an `_exit` straight after the SIGTERM would give devpod no time to
        // unwind at all, and an unwind is what takes its busy marker with it. And
        // a child that ignores SIGTERM, which is how the orphans this was written
        // for came about, is stopped here rather than left for the kernel or for
        // nobody, off Linux. The rest of the group goes with it: devpod's own
        // children are in it.
        //
        // SAFETY: `waitpid`, `poll` and `killpg` are async-signal-safe. Reaping
        // the leader frees its pid, so the reap is not what keeps the SIGKILL on
        // target. The live group is: Linux does not hand out a pgid while any
        // member of that group lives, and an empty group makes `killpg` fail
        // ESRCH. The window left is the one the SIGTERM above has: the group
        // empties and a new group takes the number before the SIGKILL, which
        // needs the pid space to wrap inside the wait.
        unsafe {
            wait_for_the_leader(pgid);
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    // The detached children next, for the same reason the foreground child is
    // signalled above: `dl` is about to `_exit`, and one of these holds a remote
    // forward open inside a container.
    //
    // SAFETY: same contract as this function; see [`signal_detached`].
    unsafe {
        signal_detached();
    }
    for slot in &FILES {
        let path = slot.load(Ordering::SeqCst);
        if !path.is_null() {
            // SAFETY: `unlink` is async-signal-safe; `path` is a live (leaked,
            // never freed) NUL-terminated C string. A path already removed by an
            // ordinary drop yields ENOENT, which is ignored.
            unsafe {
                libc::unlink(path);
            }
        }
    }
    for slot in &DIRS {
        let path = slot.load(Ordering::SeqCst);
        if !path.is_null() {
            // SAFETY: as above; `rmdir` is async-signal-safe and a non-empty or
            // absent directory yields an error that is ignored.
            unsafe {
                libc::rmdir(path);
            }
        }
    }
}

/// How long the drain gives the foreground child to act on its SIGTERM before it
/// is SIGKILLed: two seconds, `kill`'s own SIGTERM grace for the same `devpod
/// up`, in steps of [`UNWIND_STEP_MS`].
const UNWIND_STEPS: u32 = 40;

/// One step of [`UNWIND_STEPS`], in milliseconds.
const UNWIND_STEP_MS: libc::c_int = 50;

/// Wait, for [`UNWIND_STEPS`] at most, for the foreground child that leads group
/// `pgid` to exit, and reap it.
///
/// The leader and not the group, because the leader is the one this process can
/// wait for: a reaped zombie is how its exit is seen here, and a group whose
/// leader is a zombie still answers `killpg(pgid, 0)`. The rest of the group is
/// SIGKILLed after this returns either way. `ECHILD` is the main thread reaping it
/// first, which is the same answer.
///
/// # Safety
///
/// Same contract as [`drain`]: `waitpid` and `poll` are async-signal-safe.
unsafe fn wait_for_the_leader(pgid: i32) {
    for _ in 0..UNWIND_STEPS {
        let mut status: libc::c_int = 0;
        // SAFETY: async-signal-safe; see the function contract.
        let reaped = unsafe { libc::waitpid(pgid, &mut status, libc::WNOHANG) };
        if reaped != 0 {
            return;
        }
        // SAFETY: a `poll` of no descriptors is a sleep, and async-signal-safe.
        unsafe {
            libc::poll(ptr::null_mut(), 0, UNWIND_STEP_MS);
        }
    }
}

/// `SIGTERM` every registered detached child.
///
/// Its own function so a test can call it without the rest of [`drain`], which
/// signals the foreground process group and restores the terminal -- neither of
/// which a test can do to a process it shares with other tests.
///
/// # Safety
///
/// Same contract as [`drain`]: async-signal-safe, reads only lock-free atomics
/// and calls only async-signal-safe syscalls.
unsafe fn signal_detached() {
    for slot in &PIDS {
        let pid = slot.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: `kill` is async-signal-safe. The stale-pid hazard is the one
            // `killpg` in [`drain`] has, bounded the same way: a registration is
            // dropped the moment the child is signalled or reaped, and for a slot
            // read in that window to name a *live* process the kernel would have
            // had to recycle that pid within those few instructions, which needs
            // the whole pid space to wrap first.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    /// Whether any live file slot names exactly this path — a non-destructive
    /// read, so tests running in parallel never disturb one another's paths.
    fn file_is_registered(path: &Path) -> bool {
        let wanted = CString::new(path.as_os_str().as_bytes()).expect("no NUL");
        FILES.iter().any(|slot| {
            let raw = slot.load(Ordering::SeqCst);
            // SAFETY: a non-null slot holds a leaked, still-valid C string.
            !raw.is_null() && unsafe { CStr::from_ptr(raw) } == wanted.as_c_str()
        })
    }

    #[test]
    fn a_registration_holds_the_path_and_a_drop_releases_it() {
        let path = Path::new("/tmp/devlaunch-interrupt-test-file-unique-1");
        assert!(!file_is_registered(path), "starts unregistered");
        let registration = register_file(path).expect("a free slot");
        assert!(file_is_registered(path), "held while the guard lives");
        drop(registration);
        assert!(!file_is_registered(path), "gone once the guard drops");
    }

    /// The registered child is signalled, which is the whole point of the pool.
    ///
    /// A detached child is `setsid`'d, so a terminal's Ctrl-C to the foreground
    /// process group never reaches it, and `dl`'s handler `_exit`s without
    /// unwinding, so nothing on the ordinary return path runs either. Before this
    /// pool existed, an interrupted launch left the session manager's `ssh -N -R`
    /// holding a listen path inside a container indefinitely -- one per launch.
    #[test]
    fn a_registered_child_is_signalled_and_a_dropped_registration_is_not() {
        let child = std::process::Command::new("sh")
            .arg("-c")
            .arg("exec sleep 30")
            .spawn()
            .expect("a child to signal");
        let pid = i32::try_from(child.id()).expect("a pid fits");
        let registration = register_pid(pid).expect("a free slot");

        // SAFETY: the test contract for this function -- it reads atomics and
        // calls `kill`, and touches neither the terminal nor the foreground group.
        unsafe { signal_detached() };

        let mut child = child;
        let status = child.wait().expect("the child is ours to reap");
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(libc::SIGTERM),
            "the registered child outlived the handler"
        );
        drop(registration);
        assert!(
            PIDS.iter().all(|slot| slot.load(Ordering::SeqCst) != pid),
            "a dropped registration left a pid the handler would signal again"
        );
    }

    #[test]
    fn a_pid_that_names_no_process_is_not_registered() {
        // `0` is "every process in my group" and a negative pid is a group, both
        // of which `kill` would read as something far larger than one child.
        assert!(register_pid(0).is_none());
        assert!(register_pid(-1).is_none());
    }

    #[test]
    fn a_path_with_a_nul_byte_cannot_be_registered() {
        let path = Path::new("/tmp/de\0vlaunch");
        assert!(register_file(path).is_none());
    }

    #[test]
    fn a_directory_registers_in_its_own_pool() {
        let path = Path::new("/tmp/devlaunch-interrupt-test-dir-unique-1");
        let wanted = CString::new(path.as_os_str().as_bytes()).expect("no NUL");
        let held = |want: bool| {
            let found = DIRS.iter().any(|slot| {
                let raw = slot.load(Ordering::SeqCst);
                // SAFETY: a non-null slot holds a leaked, still-valid C string.
                !raw.is_null() && unsafe { CStr::from_ptr(raw) } == wanted.as_c_str()
            });
            assert_eq!(found, want);
        };
        held(false);
        let registration = register_dir(path).expect("a free slot");
        held(true);
        drop(registration);
        held(false);
    }
}
