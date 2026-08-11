"""``hold_lock`` itself, against the four promises its docstring makes.

Every other test that touches a lock patches this function out — reasonably,
because they are about what a caller does with the answer, not about how the
answer is obtained. What that left is a primitive that the whole shared cache
rests on and that nothing exercises: the contention branch (the ``except
BlockingIOError`` arm, which is the only reason ``hold_lock`` yields anything
at all) had no test, and neither did the reason ``flock`` was chosen over a pid
file. Both are the kind of thing that keeps working right up until two agents
launch at the same moment on a machine nobody is watching.

The claims, in the order ``locks.py`` makes them:

1. **Mutual exclusion across processes.** Two real ``dl``s, not two threads —
   ``flock`` is per open file description, so a threaded test would prove
   something about a lock this one does not have.
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

Subprocesses are written to disk and run with ``sys.executable``, the way
``test_concurrent_launches.py`` does it, because that is what a second ``dl``
actually is.
"""

import os
import signal
import subprocess
import sys
import time
from pathlib import Path

from devlaunch.worktree.locks import hold_lock

# How long a driver may take to reach a flag before the test calls it hung.
# Generous: this bounds a failure, it does not time anything.
READY_TIMEOUT = 60


def _spawn(driver: str, args: list, tmp_path: Path, name: str) -> subprocess.Popen:
    script = tmp_path / f"{name}.py"
    script.write_text(driver)
    return subprocess.Popen(
        [sys.executable, str(script), *[str(a) for a in args]],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=os.environ.copy(),
    )


def _await(*flags: Path) -> None:
    deadline = time.monotonic() + READY_TIMEOUT
    while not all(flag.exists() for flag in flags):
        assert time.monotonic() < deadline, f"driver never became ready: {flags}"
        time.sleep(0.01)


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

# Declares that it is about to ask, then marks the moment it gets in. The
# `trying` flag is what lets a test releasing the holder know the contender is
# genuinely queued rather than still starting a Python interpreter -- without
# it, "release once the contender exists" is a race the contender usually
# loses, and a contender that never contended reports False.
_CONTENDER = """
import sys, time
from pathlib import Path
from devlaunch.worktree.locks import hold_lock

lock_path, entered_flag, note = sys.argv[1:4]
Path(entered_flag).with_name(Path(entered_flag).name + "-trying").touch()
with hold_lock(Path(lock_path), note if note else None) as contended:
    Path(entered_flag).touch()
    print(f"contended={contended}", flush=True)
"""


def _await_queued(entered: Path) -> None:
    """Block until the contender for *entered* is actually waiting on the lock.

    The flag says it reached the call; the settle is for the microseconds
    between that and the kernel putting it to sleep. Both halves are needed and
    neither can be tightened into a synchronisation -- there is no event for
    "a process is now blocked in flock" that a parent can wait on. The failure
    direction is safe: too short and the test reports a contention that did not
    happen, which is a red tick, not a green one.
    """
    _await(entered.with_name(entered.name + "-trying"))
    time.sleep(0.5)


class TestMutualExclusion:
    def test_a_second_process_waits_for_the_first_to_let_go(self, tmp_path):
        """Claim 1. The contender must not enter while the holder is inside.

        Asserted as an ordering between two observable events rather than as a
        duration: the holder is still in its block when the contender is
        checked, so a lock that did not exclude shows up as the contender's
        flag existing at a moment it provably must not.
        """
        lock = tmp_path / "cache" / "repo.lock"
        held, release, entered = tmp_path / "held", tmp_path / "release", tmp_path / "entered"

        holder = _spawn(_HOLDER, [lock, held, release], tmp_path, "holder")
        try:
            _await(held)
            contender = _spawn(_CONTENDER, [lock, entered, ""], tmp_path, "contender")
            try:
                # Long enough that a contender which was never blocked has
                # finished several times over. It cannot make a passing test
                # out of a broken lock -- it can only make the failure louder.
                time.sleep(1.0)
                assert not entered.exists(), (
                    "the contender entered the critical section while the holder was inside it"
                )
                release.touch()
                _, err = contender.communicate(timeout=READY_TIMEOUT)
                assert contender.returncode == 0, err
                assert entered.exists(), "the contender never got the lock after it was released"
            finally:
                if contender.poll() is None:
                    contender.kill()
        finally:
            release.touch()
            holder.communicate(timeout=READY_TIMEOUT)

    def test_the_lock_is_free_again_once_the_block_ends(self, tmp_path):
        """The other half of claim 1: it is a lock, not a latch.

        Same process, twice in sequence. Uncontended both times -- if the first
        `with` leaked its descriptor the second would block forever, which is
        the non-reentrancy the module documents pointed at the wrong target.
        """
        lock = tmp_path / "cache" / "repo.lock"
        with hold_lock(lock) as contended:
            assert contended is False
        with hold_lock(lock) as contended:
            assert contended is False


class TestContentionIsReported:
    def test_an_uncontended_acquisition_says_so(self, tmp_path):
        """Claim 2, the quiet half. Nobody holds it, so nobody waited."""
        with hold_lock(tmp_path / "repo.lock") as contended:
            assert contended is False

    def test_a_contended_acquisition_says_so(self, tmp_path):
        """Claim 2. The flag exists to tell a launch its reads may be stale.

        Read off a process that genuinely queued: the holder reports False
        because it walked in, the contender True because it did not. Both ends
        asserted, because a `hold_lock` hard-wired to either answer would
        satisfy one of them.
        """
        lock = tmp_path / "cache" / "repo.lock"
        held, release, entered = tmp_path / "held", tmp_path / "release", tmp_path / "entered"

        holder = _spawn(_HOLDER, [lock, held, release], tmp_path, "holder")
        try:
            _await(held)
            contender = _spawn(_CONTENDER, [lock, entered, ""], tmp_path, "contender")
            _await_queued(entered)
            release.touch()
            contender_out, contender_err = contender.communicate(timeout=READY_TIMEOUT)
            assert contender.returncode == 0, contender_err
            assert "contended=True" in contender_out, (
                f"the contender queued and must say so: {contender_out!r}"
            )
        finally:
            release.touch()
            holder_out, _ = holder.communicate(timeout=READY_TIMEOUT)
        assert "contended=False" in holder_out, (
            f"the holder walked straight in and must say so: {holder_out!r}"
        )

    def test_a_waiting_note_reaches_stderr_only_when_there_was_a_wait(self, tmp_path):
        """The note is for a human watching a run that has gone quiet.

        Both directions, because the failure that matters is the silent one: a
        `dl` that sits for ninety seconds behind a sibling's clone and prints
        nothing looks exactly like a `dl` that has hung.
        """
        lock = tmp_path / "cache" / "repo.lock"
        held, release, entered = tmp_path / "held", tmp_path / "release", tmp_path / "entered"

        holder = _spawn(_HOLDER, [lock, held, release], tmp_path, "holder")
        try:
            _await(held)
            contender = _spawn(
                _CONTENDER, [lock, entered, "a sibling's clone"], tmp_path, "contender"
            )
            _await_queued(entered)
            release.touch()
            _, contender_err = contender.communicate(timeout=READY_TIMEOUT)
            assert contender.returncode == 0, contender_err
            assert "dl: waiting for a sibling's clone" in contender_err, (
                f"a run that waited must say what it waited for: {contender_err!r}"
            )
        finally:
            release.touch()
            holder.communicate(timeout=READY_TIMEOUT)

        # And the same note, uncontended: nothing is printed, because there was
        # nothing to explain.
        uncontended = _spawn(
            _CONTENDER, [lock, tmp_path / "second", "a sibling's clone"], tmp_path, "uncontended"
        )
        _, quiet_err = uncontended.communicate(timeout=READY_TIMEOUT)
        assert uncontended.returncode == 0, quiet_err
        assert "waiting for" not in quiet_err, (
            f"a run that walked straight in has nothing to report: {quiet_err!r}"
        )


class TestADeadHolderReleasesTheLock:
    def test_sigkill_does_not_wedge_the_cache(self, tmp_path):
        """Claim 3 -- the entire argument for ``flock`` over a pid file.

        SIGKILL and not SIGTERM: the point is a holder that runs no cleanup at
        all, which is what an OOM kill or a pulled power cord looks like. A pid
        file survives that and locks the cache out until a human deletes it;
        the kernel drops an flock when the fd closes, and every fd closes when
        the process dies.

        The acquisition afterwards is the assertion, and it is bounded --
        without the guarantee this test hangs rather than fails, so the timeout
        is what turns a wedged cache into a red tick.
        """
        lock = tmp_path / "cache" / "repo.lock"
        held, release = tmp_path / "held", tmp_path / "release"

        holder = _spawn(_HOLDER, [lock, held, release], tmp_path, "holder")
        _await(held)
        holder.kill()  # SIGKILL: no finally, no close, no cleanup.
        holder.wait(timeout=READY_TIMEOUT)
        assert holder.returncode == -signal.SIGKILL

        entered = tmp_path / "entered"
        survivor = _spawn(_CONTENDER, [lock, entered, ""], tmp_path, "survivor")
        out, err = survivor.communicate(timeout=READY_TIMEOUT)
        assert survivor.returncode == 0, f"the lock outlived the process holding it:\n{err}"
        assert entered.exists()
        # It was free, not merely eventually free: the dead holder's lock was
        # already gone by the time the survivor asked, so nothing queued.
        assert "contended=False" in out, f"a dead holder leaves no queue behind: {out!r}"


class TestTheLockFileIsNeverUnlinked:
    def test_the_file_survives_the_block(self, tmp_path):
        """Claim 4. Unlinking is what makes a lock stop working silently.

        Two arrivals that lock different inodes both "hold" the lock and
        neither can see the other, so the file's survival is load-bearing
        rather than housekeeping -- and it is asserted by inode, because a
        delete-and-recreate leaves a path that exists and a guarantee that does
        not.
        """
        lock = tmp_path / "cache" / "repo.lock"
        with hold_lock(lock):
            assert lock.exists(), "the lock file exists while it is held"
            inode = lock.stat().st_ino
        assert lock.exists(), "the lock file outlives the block that held it"
        assert lock.stat().st_ino == inode, "the same inode, so a later arrival queues behind it"

    def test_the_parent_directory_is_created_on_demand(self, tmp_path):
        """A first launch locks a repo directory that does not exist yet.

        The lock has to be takeable *before* the thing it protects is built --
        that is the race it exists for, two processes both about to run
        ``git clone --bare`` into a path neither has created.
        """
        lock = tmp_path / "not" / "yet" / "there" / "repo.lock"
        assert not lock.parent.exists()
        with hold_lock(lock):
            pass
        assert lock.exists()

    def test_the_lock_file_is_not_world_readable(self, tmp_path):
        """0o600, as ``os.open`` is asked for.

        The shared cache is per-user state under ``$XDG_CACHE_HOME``; a lock
        file another user can open is a lock another user can hold, which on a
        shared box is a way to stop somebody else's ``dl`` for free.
        """
        lock = tmp_path / "repo.lock"
        with hold_lock(lock):
            pass
        assert lock.stat().st_mode & 0o077 == 0, "no group or other bits on the lock file"
