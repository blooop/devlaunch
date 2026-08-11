"""``hold_lock`` itself, against the four promises its docstring makes.

Every other test that touches a lock patches this function out — reasonably,
because they are about what a caller does with the answer, not about how the
answer is obtained. What that left is a primitive the whole shared cache rests
on and that nothing exercises: the contention branch (the ``except
BlockingIOError`` arm, which is the only reason ``hold_lock`` yields anything at
all) had no test, and neither did the reason ``flock`` was chosen over a pid
file. Both are the kind of thing that keeps working right up until two agents
launch at the same moment on a machine nobody is watching.

The claims, in the order ``locks.py`` makes them:

1. **Mutual exclusion across processes**, and the lock freed at block exit —
   including when the block exits by raising, which is what the ``finally`` in
   ``hold_lock`` is for.
2. **Contention is reported.** ``waited`` is what tells a launch the world may
   have moved under it while it queued, and a launch that queued behind a
   sibling bringing up the very workspace it wants is the case the flag was
   added for.
3. **A dead holder releases it.** The whole argument for ``flock``. A pid file
   left by a killed process wedges the cache until a human deletes it; a lock
   the kernel owns does not.
4. **The lock file is never unlinked.** Removing it is the classic
   self-defeating move — a later arrival locks a fresh inode and walks straight
   past the holder — so its survival is a guarantee rather than litter.

At ``test/`` root rather than under ``test/unit/``, deliberately.
``conftest.pytest_collection_modifyitems`` marks by directory with no opt-out,
and ``unit`` is defined as "Pure logic tests with no external commands. Fast,
runs everywhere" — which a file that starts eight interpreters is not.
``test_concurrent_launches.py`` sits here for the same reason.

**Every acquisition in this file is bounded**, in-process ones included. The
regression these tests exist to catch — a leaked descriptor, a lock not
released — makes ``flock`` block forever rather than return something wrong, so
an unbounded ``with hold_lock(...)`` would hang the suite instead of failing it.
There is no ``timeout-minutes`` on the ``ci`` job to catch that; a hung matrix
leg would sit until GitHub's own six-hour default killed it with no output. See
``bounded``.
"""

import contextlib
import os
import signal
from pathlib import Path

from devlaunch.worktree.locks import hold_lock
from fixtures.subprocess_drivers import (
    DRIVER_TIMEOUT,
    await_blocked_on_lock,
    await_flags,
    spawn_driver,
)

# What an in-process acquisition gets before it is called wedged. Nothing here
# contends in-process, so a correct lock returns in microseconds and this is
# only ever reached by a broken one.
ACQUIRE_TIMEOUT = 10


@contextlib.contextmanager
def bounded(what: str, seconds: int = ACQUIRE_TIMEOUT):
    """Fail rather than hang if the block does not finish in *seconds*.

    ``SIGALRM`` because the thing being bounded is a blocking syscall:
    ``fcntl.flock`` with ``LOCK_EX`` and no timeout, which no amount of Python
    control flow can interrupt from the outside. The signal makes the call
    return ``EINTR`` and the handler turns that into an assertion, so a lock
    that was never released reads as a red test rather than a silent job.

    Main-thread only, which is where pytest runs its tests.
    """

    def wedged(_signum, _frame):
        raise AssertionError(
            f"{what} did not complete within {seconds}s — the lock was never released"
        )

    previous = signal.signal(signal.SIGALRM, wedged)
    signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


# Holds the lock through dl's own contextmanager -- not a raw flock -- so what
# the contender is queued behind is the code under test rather than the test's
# idea of it. Prints the contention it saw, then waits to be let go.
_HOLDER = """
import sys, time
from pathlib import Path
from devlaunch.worktree.locks import hold_lock

lock_path, held_flag, release_flag = sys.argv[1:4]
with hold_lock(Path(lock_path)) as contended:
    print(f"contended={contended}", flush=True)
    Path(held_flag).touch()
    while not Path(release_flag).exists():
        time.sleep(0.01)
"""

# Reports the contention it saw and marks the moment it got in, so the parent
# can check the two processes' critical sections never overlap.
_CONTENDER = """
import sys
from pathlib import Path
from devlaunch.worktree.locks import hold_lock

lock_path, entered_flag, note = sys.argv[1:4]
with hold_lock(Path(lock_path), note if note else None) as contended:
    Path(entered_flag).touch()
    print(f"contended={contended}", flush=True)
"""


@contextlib.contextmanager
def a_holder(tmp_path: Path, lock: Path):
    """A second process sitting inside ``hold_lock(lock)`` for the block.

    Yields the process and the flag that lets it go. The teardown is the point
    of the contextmanager: every test here can fail while a child is parked on
    a lock or spinning on a flag file, and a child that outlives its test does
    not stop — ``_HOLDER`` polls at 100 Hz for a release flag that pytest's
    tmp_path cleanup is about to delete. Killing on the way out, however the
    way out is taken, is what keeps a failing run from leaving one behind.
    """
    held, release = tmp_path / "held", tmp_path / "release"
    holder = spawn_driver(_HOLDER, [lock, held, release], tmp_path, "holder")
    try:
        await_flags(held)
        yield holder, release
    finally:
        release.touch()
        if holder.poll() is None:
            holder.kill()
        holder.communicate(timeout=DRIVER_TIMEOUT)


@contextlib.contextmanager
def a_contender(tmp_path: Path, lock: Path, entered: Path, note: str = ""):
    """A second process queuing for *lock*, killed on the way out if it is still there."""
    contender = spawn_driver(_CONTENDER, [lock, entered, note], tmp_path, "contender")
    try:
        yield contender
    finally:
        if contender.poll() is None:
            contender.kill()
            contender.communicate(timeout=DRIVER_TIMEOUT)


class TestMutualExclusion:
    def test_a_second_process_waits_for_the_first_to_let_go(self, tmp_path):
        """Claim 1. The contender must not enter while the holder is inside.

        The ordering is what is asserted, not a duration: the contender is
        observed *parked on the lock* while the holder is provably still inside
        its block, which is a state a lock that did not exclude cannot produce.

        An earlier version of this test slept a second and checked the flag had
        not appeared, which is weaker than it looks — a child still starting its
        interpreter has not entered either, so a completely broken lock passed
        whenever process startup ran long.
        """
        lock = tmp_path / "cache" / "repo.lock"
        entered = tmp_path / "entered"

        with a_holder(tmp_path, lock) as (_holder, release):
            with a_contender(tmp_path, lock, entered) as contender:
                await_blocked_on_lock(contender)
                assert not entered.exists(), (
                    "the contender entered the critical section while the holder was inside it"
                )
                release.touch()
                out, err = contender.communicate(timeout=DRIVER_TIMEOUT)
                assert contender.returncode == 0, err
                assert entered.exists(), "the contender never got the lock after it was released"
                assert "contended=True" in out, f"the contender queued and must say so: {out!r}"

    def test_the_lock_is_free_again_once_the_block_ends(self, tmp_path):
        """The other half of claim 1: it is a lock, not a latch.

        Same process, twice in sequence. Uncontended both times — if the first
        ``with`` leaked its descriptor the second would block on it, which is
        the non-reentrancy the module documents, pointed at the wrong target.
        """
        lock = tmp_path / "cache" / "repo.lock"
        with bounded("the first acquisition"), hold_lock(lock) as contended:
            assert contended is False
        with bounded("the second acquisition"), hold_lock(lock) as contended:
            assert contended is False

    def test_the_lock_is_released_when_the_block_raises(self, tmp_path):
        """The `finally` in ``hold_lock``, which nothing else here reaches.

        Every other test leaves the block normally, so ``finally: os.close(fd)``
        could be a plain trailing statement and the suite would not notice. It
        is not a hypothetical distinction: the critical sections this guards are
        clone and metadata writes, the operations most likely to raise, and a
        `dl` that kept the repo lock after one did would wedge every sibling
        waiting on that repo for the life of the process.
        """
        lock = tmp_path / "cache" / "repo.lock"
        with contextlib.suppress(RuntimeError):
            with bounded("the acquisition that raises"), hold_lock(lock):
                raise RuntimeError("the critical section failed")
        with bounded("the acquisition after the raise"), hold_lock(lock) as contended:
            assert contended is False, "a block that raised still let go of the lock"


class TestContentionIsReported:
    def test_a_contended_acquisition_says_so(self, tmp_path):
        """Claim 2. The flag exists to tell a launch its reads may be stale.

        Read off a process that genuinely queued: the holder reports False
        because it walked in, the contender True because it did not. Both ends
        asserted, because a ``hold_lock`` hard-wired to either answer would
        satisfy one of them.
        """
        lock = tmp_path / "cache" / "repo.lock"
        entered = tmp_path / "entered"

        with a_holder(tmp_path, lock) as (holder, release):
            with a_contender(tmp_path, lock, entered) as contender:
                await_blocked_on_lock(contender)
                release.touch()
                out, err = contender.communicate(timeout=DRIVER_TIMEOUT)
                assert contender.returncode == 0, err
                assert "contended=True" in out, f"the contender queued and must say so: {out!r}"
            holder_out, _ = holder.communicate(timeout=DRIVER_TIMEOUT)
        assert "contended=False" in holder_out, (
            f"the holder walked straight in and must say so: {holder_out!r}"
        )

    def test_a_waiting_note_reaches_stderr_only_when_there_was_a_wait(self, tmp_path):
        """The note is for a human watching a run that has gone quiet.

        Both directions, because the failure that matters is the silent one: a
        ``dl`` that sits for ninety seconds behind a sibling's clone and prints
        nothing looks exactly like a ``dl`` that has hung.
        """
        lock = tmp_path / "cache" / "repo.lock"
        entered = tmp_path / "entered"
        note = "a sibling's clone"

        with a_holder(tmp_path, lock) as (_holder, release):
            with a_contender(tmp_path, lock, entered, note) as contender:
                await_blocked_on_lock(contender)
                release.touch()
                _, err = contender.communicate(timeout=DRIVER_TIMEOUT)
                assert contender.returncode == 0, err
                assert f"dl: waiting for {note}" in err, (
                    f"a run that waited must say what it waited for: {err!r}"
                )

        # And the same note, uncontended: nothing is printed, because there was
        # nothing to explain.
        quiet = tmp_path / "second"
        with a_contender(tmp_path, lock, quiet, note) as uncontended:
            _, quiet_err = uncontended.communicate(timeout=DRIVER_TIMEOUT)
        assert uncontended.returncode == 0, quiet_err
        assert "waiting for" not in quiet_err, (
            f"a run that walked straight in has nothing to report: {quiet_err!r}"
        )


class TestADeadHolderReleasesTheLock:
    def test_sigkill_does_not_wedge_the_cache(self, tmp_path):
        """Claim 3 — the entire argument for ``flock`` over a pid file.

        SIGKILL and not SIGTERM: the point is a holder that runs no cleanup at
        all, which is what an OOM kill or a pulled power cord looks like. A pid
        file survives that and locks the cache out until a human deletes it; the
        kernel drops an flock when the fd closes, and every fd closes when the
        process dies.

        The acquisition afterwards is the assertion, and it is bounded — without
        the guarantee this hangs rather than fails, so the timeout is what turns
        a wedged cache into a red tick.
        """
        lock = tmp_path / "cache" / "repo.lock"
        entered = tmp_path / "entered"

        with a_holder(tmp_path, lock) as (holder, _release):
            holder.kill()  # SIGKILL: no finally, no close, no cleanup.
            holder.wait(timeout=DRIVER_TIMEOUT)
            assert holder.returncode == -signal.SIGKILL

            with a_contender(tmp_path, lock, entered) as survivor:
                out, err = survivor.communicate(timeout=DRIVER_TIMEOUT)
                assert survivor.returncode == 0, f"the lock outlived the process holding it:\n{err}"
                assert entered.exists()
                # It was free, not merely eventually free: the dead holder's
                # lock was already gone by the time the survivor asked.
                assert "contended=False" in out, f"a dead holder leaves no queue behind: {out!r}"


class TestTheLockFileIsNeverUnlinked:
    def test_the_file_survives_the_block(self, tmp_path):
        """Claim 4. Unlinking is what makes a lock stop working silently.

        Two arrivals that lock different inodes both "hold" the lock and neither
        can see the other, so the file's survival is load-bearing rather than
        housekeeping — and it is asserted by inode, because a delete-and-recreate
        leaves a path that exists and a guarantee that does not.
        """
        lock = tmp_path / "cache" / "repo.lock"
        with bounded("the acquisition"), hold_lock(lock):
            assert lock.exists(), "the lock file exists while it is held"
            inode = lock.stat().st_ino
        assert lock.exists(), "the lock file outlives the block that held it"
        assert lock.stat().st_ino == inode, "the same inode, so a later arrival queues behind it"

    def test_the_parent_directory_is_created_on_demand(self, tmp_path):
        """A first launch locks a repo directory that does not exist yet.

        The lock has to be takeable *before* the thing it protects is built —
        that is the race it exists for, two processes both about to run
        ``git clone --bare`` into a path neither has created.
        """
        lock = tmp_path / "not" / "yet" / "there" / "repo.lock"
        assert not lock.parent.exists()
        with bounded("the acquisition"), hold_lock(lock):
            pass
        assert lock.exists()

    def test_a_lock_file_this_created_is_not_world_readable(self, tmp_path):
        """0o600 on the file ``hold_lock`` creates, whatever the ambient umask.

        The shared cache is per-user state under ``$XDG_CACHE_HOME``; a lock
        file another user can open is a lock another user can hold, which on a
        shared box is a way to stop somebody else's ``dl`` for free.

        Scoped to creation in the name, because that is all ``os.open`` can
        promise: its mode argument is ignored for a file that already exists,
        and ``hold_lock`` never deletes one. A lock file that arrived with
        looser bits — from an older release, or a restored backup — keeps them,
        and no assertion here would see it. ``dl --purge`` sweeping the cache is
        what clears that, not this call.

        The umask is dropped to 0 for the duration so the test reads the mode
        ``hold_lock`` asked for rather than one the environment happened to mask
        down to something acceptable.
        """
        lock = tmp_path / "repo.lock"
        previous = os.umask(0)
        try:
            with bounded("the acquisition"), hold_lock(lock):
                pass
        finally:
            os.umask(previous)
        assert lock.stat().st_mode & 0o077 == 0, "no group or other bits on the lock file"
