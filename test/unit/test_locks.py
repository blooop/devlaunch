"""What the non-blocking lock promises a background sweep.

`hold_lock` is for work someone is waiting on: it waits its turn because the
launch behind it has nothing useful to do until the cache is consistent. The
detached cache updater is the opposite case — nobody is waiting on it, and the
one thing it must never do is make a foreground launch wait. `try_hold_lock` is
that: it reports whether it got the lock and carries on either way.
"""

import fcntl
import os
import threading

from devlaunch.worktree.locks import hold_lock, try_hold_lock


def _held_elsewhere(lock_path):
    """Take the lock through a separate open file description.

    flock conflicts between distinct open file descriptions even inside one
    process, so this is a faithful stand-in for another dl process holding the
    repo lock — no second process needed to prove the sweep steps aside.
    """
    fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    return fd


class TestTryHoldLock:
    """Acquire-or-report, never wait."""

    def test_an_uncontended_lock_is_acquired(self, tmp_path):
        """Nothing else holds it, so the block runs as the holder."""
        with try_hold_lock(tmp_path / "repo" / ".lock") as acquired:
            assert acquired is True

    def test_a_lock_held_elsewhere_is_reported_not_waited_for(self, tmp_path):
        """The whole point: a contended sweep skips instead of queueing."""
        lock_path = tmp_path / ".lock"
        fd = _held_elsewhere(lock_path)
        try:
            finished = threading.Event()
            seen = []

            def sweep():
                with try_hold_lock(lock_path) as acquired:
                    seen.append(acquired)
                finished.set()

            worker = threading.Thread(target=sweep, daemon=True)
            worker.start()
            assert finished.wait(timeout=5), "try_hold_lock waited on a held lock"
            assert seen == [False]
        finally:
            os.close(fd)

    def test_the_lock_is_released_when_the_block_ends(self, tmp_path):
        """A sweep that took the lock must not keep it from the next launch."""
        lock_path = tmp_path / ".lock"
        with try_hold_lock(lock_path) as acquired:
            assert acquired is True
        with hold_lock(lock_path) as contended:
            assert contended is False

    def test_the_lock_file_outlives_the_block(self, tmp_path):
        """Unlinking an flock'd file is the classic self-defeating move, so this
        one does not do it either — see the module docstring."""
        lock_path = tmp_path / ".lock"
        with try_hold_lock(lock_path):
            pass
        assert lock_path.exists()

    def test_a_lock_not_acquired_is_not_released_out_from_under_its_holder(self, tmp_path):
        """Leaving the block must not drop the other holder's lock."""
        lock_path = tmp_path / ".lock"
        fd = _held_elsewhere(lock_path)
        try:
            with try_hold_lock(lock_path) as acquired:
                assert acquired is False
            with try_hold_lock(lock_path) as still_held_elsewhere:
                assert still_held_elsewhere is False
        finally:
            os.close(fd)
