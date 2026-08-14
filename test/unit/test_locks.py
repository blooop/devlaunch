"""What the non-blocking lock promises a background sweep.

`hold_lock` is for work someone is waiting on: it waits its turn because the
launch behind it has nothing useful to do until the cache is consistent. The
detached cache updater is the opposite case — nobody is waiting on it, and the
one thing it must never do is make a foreground launch wait.

`run_if_lock_free` is that, and it takes the work rather than yielding a flag on
purpose: a contended sweep has no block to run, so there is no guard to forget
and no way to do the protected work unlocked. The tests below are the pins for
that shape, not just for the flock call underneath it.
"""

import fcntl
import os
import threading

from devlaunch.worktree import locks
from devlaunch.worktree.locks import hold_lock, run_if_lock_free


def _held_elsewhere(lock_path):
    """Take the lock through a separate open file description.

    flock conflicts between distinct open file descriptions even inside one
    process, so this is a faithful stand-in for another dl process holding the
    repo lock — no second process needed to prove the sweep steps aside.
    """
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    return fd


class TestWorkOnlyRunsUnderTheLock:
    """The contended case has no body, so nothing can run unprotected."""

    def test_work_on_a_free_lock_runs(self, tmp_path):
        """Nothing else holds it, so the work runs as the holder."""
        ran = []

        acquired = run_if_lock_free(tmp_path / "repo" / ".lock", lambda: ran.append("work"))

        assert acquired is True
        assert ran == ["work"]

    def test_work_is_held_off_entirely_when_another_run_holds_the_lock(self, tmp_path):
        """The pin for the whole shape: a caller cannot forget a guard it does
        not have, so a contended sweep does not fetch behind a launch's back."""
        lock_path = tmp_path / ".lock"
        fd = _held_elsewhere(lock_path)
        ran = []
        try:
            acquired = run_if_lock_free(lock_path, lambda: ran.append("work"))
        finally:
            os.close(fd)

        assert acquired is False
        assert ran == []

    def test_the_lock_really_is_held_while_the_work_runs(self, tmp_path):
        """Otherwise the work is unlocked and only looks protected."""
        lock_path = tmp_path / ".lock"
        free_to_others = []

        def work():
            fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                free_to_others.append(True)
            except BlockingIOError:
                free_to_others.append(False)
            finally:
                os.close(fd)

        run_if_lock_free(lock_path, work)

        assert free_to_others == [False]

    def test_a_raising_work_still_releases_the_lock(self, tmp_path):
        """A failed fetch must not wedge the repo for every later run."""
        lock_path = tmp_path / ".lock"

        def boom():
            raise RuntimeError("fetch failed")

        try:
            run_if_lock_free(lock_path, boom)
        except RuntimeError:
            pass

        with hold_lock(lock_path) as contended:
            assert contended is False


class TestItNeverWaits:
    """Background defers to foreground; never the other way round."""

    def test_a_lock_held_elsewhere_is_stepped_over_not_queued_for(self, tmp_path):
        """The whole point: a contended sweep skips instead of queueing."""
        lock_path = tmp_path / ".lock"
        fd = _held_elsewhere(lock_path)
        try:
            finished = threading.Event()
            outcome = []

            def sweep():
                outcome.append(run_if_lock_free(lock_path, lambda: None))
                finished.set()

            worker = threading.Thread(target=sweep, daemon=True)
            worker.start()
            assert finished.wait(timeout=5), "run_if_lock_free waited on a held lock"
            assert outcome == [False]
        finally:
            os.close(fd)


class TestTheLockFileItself:
    """Same two deliberate limits `hold_lock` has, for the same reasons."""

    def test_the_lock_is_released_when_the_work_ends(self, tmp_path):
        """A sweep that took the lock must not keep it from the next launch."""
        lock_path = tmp_path / ".lock"
        run_if_lock_free(lock_path, lambda: None)
        with hold_lock(lock_path) as contended:
            assert contended is False

    def test_the_lock_file_outlives_the_work(self, tmp_path):
        """Unlinking an flock'd file is the classic self-defeating move, so this
        one does not do it either — see the module docstring."""
        lock_path = tmp_path / ".lock"
        run_if_lock_free(lock_path, lambda: None)
        assert lock_path.exists()

    def test_a_lock_not_acquired_is_not_released_out_from_under_its_holder(self, tmp_path):
        """Leaving must not drop the other holder's lock."""
        lock_path = tmp_path / ".lock"
        fd = _held_elsewhere(lock_path)
        try:
            assert run_if_lock_free(lock_path, lambda: None) is False
            assert run_if_lock_free(lock_path, lambda: None) is False
        finally:
            os.close(fd)


class TestNoAcquireOrNotContextManager:
    """No helper hands out a lock it may not have taken.

    A context manager that yields "did I get it" and runs its block either way
    is the trap this module refuses to offer: `hold_lock` yields whether the
    caller *waited* and `try_hold_lock` used to yield whether it *acquired* —
    near-identical names, opposite polarity, and the second one's block ran
    unlocked if a caller dropped the flag. Only `hold_lock` yields now, so there
    is one such value in the module and it guards nothing.
    """

    def test_the_acquire_or_not_context_manager_is_gone(self):
        assert not hasattr(locks, "try_hold_lock")

    def test_the_non_blocking_helper_is_not_a_context_manager(self, tmp_path):
        """It returns an answer about the past, not a scope to run code in."""
        result = run_if_lock_free(tmp_path / ".lock", lambda: None)
        assert not hasattr(result, "__enter__")
        assert result is True
