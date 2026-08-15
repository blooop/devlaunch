"""Two dl processes preparing the same repository at the same time.

This is the race ``worktree/locks.py`` was written for, and the only test that
reproduces it rather than describing it. Two agents launched on two branches of
one repo is an ordinary Tuesday, and before the lock both of their first
launches ran ``git clone --bare`` into the same path: the loser's
``CalledProcessError`` cleanup then deleted the winner's half-written cache, so
the surviving process was left running out of a directory that had been removed
underneath it.

``test_locks.py`` covers the lock itself — that it excludes, that a dead holder
releases it, that the file is never unlinked. What it cannot cover is the thing
being excluded, because that needs a real ``git clone`` racing another real
``git clone`` in another interpreter. Threads will not do: one file description
and one GIL between them means a threaded version proves something about a lock
that is not the one shipping.

**Both tests here are staged, and that is deliberate.** The obvious version of
this file starts two processes at once and asserts they converge on one clone.
That test was written, and then measured: with the ``flock`` acquisition removed
entirely it still passed 8 runs out of 30, because when the two happen to
serialize by luck the second one's stale metadata sends it down the adoption
path and every assertion holds. A guard that greens a quarter of the time under
the defect it names is worse than no guard, and it gets *more* likely to green on
the loaded single-core runner where the lock matters most. So the schedule is
forced instead — one process is made to arrive while the other holds the lock —
and both tests below fail on every run when the lock stops excluding.

The "remote" they clone is a bare repository in the same ``tmp_path``: a real
clone, no network.
"""

import json
import subprocess
from pathlib import Path

import pytest

from fixtures.subprocess_drivers import (
    await_blocked_on_lock,
    await_flags,
    finish,
    spawn_driver,
    stop,
)

# A driver that runs `git clone --bare` is slow rather than hung. The lock-only
# drivers in test_locks.py are bounded far tighter; this one is bounded so a
# wedged clone fails with output instead of sitting until CI gives up.
CLONE_TIMEOUT = 120

DRIVER = """
import sys
import time
from pathlib import Path

from devlaunch.worktree.config import WorktreeConfig
from devlaunch.worktree.locks import hold_lock
from devlaunch.worktree.repo_manager import RepositoryManager
from devlaunch.worktree.storage import MetadataStorage

role = sys.argv[1]
repos_dir, metadata = Path(sys.argv[2]), Path(sys.argv[3])
remote = sys.argv[4]
flags = [Path(arg) for arg in sys.argv[5:]]


def wait_for(flag):
    while not flag.exists():
        time.sleep(0.01)


# Built before any flag is signalled, and that ordering is the substance of the
# race: a process's in-memory metadata is whatever the file said when it
# started, so a process that starts before a sibling saves goes on believing
# the repo is not cloned long after it is.
manager = RepositoryManager(
    repos_dir=repos_dir,
    storage=MetadataStorage(metadata),
    config=WorktreeConfig(repos_dir=repos_dir, fetch_interval=0),
)

if role == "winner":
    held, clone_now = flags
    with hold_lock(manager.lock_path("test", "repo")):
        held.write_text("held", encoding="utf-8")
        wait_for(clone_now)
        manager.clone_repo("test", "repo", remote)
        # Written inside the clone, still under the lock. Nothing the loser is
        # allowed to do removes it: adopting the clone leaves it, re-cloning
        # over it is refused by git, and clearing it first would take this with
        # it.
        marker = manager.get_bare_path("test", "repo") / "winner-was-here"
        marker.write_text("x", encoding="utf-8")
    print(manager.get_bare_path("test", "repo"))
else:
    (ready,) = flags
    ready.write_text("ready", encoding="utf-8")
    got = manager.ensure_repo("test", "repo", remote)
    print(got.local_path)
"""


def cache_of(metadata_path: Path) -> dict:
    return json.loads(metadata_path.read_text(encoding="utf-8"))


def head_of(bare_path: Path) -> str:
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=bare_path,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


@pytest.mark.integration
class TestTwoProcessesPreparingOneRepo:
    """One holds the repo lock and clones; the other arrives while it does."""

    @staticmethod
    def stage(env, local_git_repo, tmp_path):
        """Start the winner, wait until it holds the lock, then start the loser.

        Returns ``(winner, loser, clone_now)``: two running drivers with the
        loser parked on the lock, and the flag that releases the winder into its
        clone. The caller is responsible for stopping both.
        """
        args = [env["repos_dir"], env["metadata_path"], local_git_repo["remote_url"]]
        held = tmp_path / "held"
        clone_now = tmp_path / "clone-now"
        loaded = tmp_path / "loaded"

        winner = spawn_driver(DRIVER, ["winner", *args, held, clone_now], tmp_path, "winner")
        loser = None
        try:
            await_flags(held, watching=[winner])
            loser = spawn_driver(DRIVER, ["loser", *args, loaded], tmp_path, "loser")
            await_flags(loaded, watching=[winner, loser])
            await_blocked_on_lock(loser)
            assert loser.poll() is None, "the loser walked straight past a held lock"
        except BaseException:
            stop(winner)
            stop(loser)
            raise
        return winner, loser, clone_now

    def test_the_waiting_process_adopts_the_clone_it_waited_for(
        self, isolated_devlaunch_env, local_git_repo, tmp_path
    ):
        # The loser loads its metadata *before* the winner writes any, so when
        # it finally gets the lock it is looking at a `.bare` on disk that its
        # own records have never heard of — exactly the "another process just
        # made it" case `clone_repo` adopts, reached across two interpreters
        # instead of by deleting a record.
        env = isolated_devlaunch_env
        winner, loser, clone_now = self.stage(env, local_git_repo, tmp_path)
        try:
            clone_now.write_text("go", encoding="utf-8")
            said_winner = finish(winner, "the winner", timeout=CLONE_TIMEOUT).strip()
            said_loser = finish(loser, "the loser", timeout=CLONE_TIMEOUT).strip()
        finally:
            stop(winner)
            stop(loser)

        bare_path = env["repos_dir"] / "test" / "repo" / ".bare"
        assert said_winner == said_loser == str(bare_path)
        # The whole point. The loser found a clone it had no record of and took
        # it as the authority: it did not clone over it, and its own failure
        # cleanup did not delete it.
        assert (bare_path / "winner-was-here").exists(), "the loser destroyed the winner's clone"
        assert head_of(bare_path)
        assert list(cache_of(env["metadata_path"])["repositories"]) == ["test/repo"]

        repo_dir = env["repos_dir"] / "test" / "repo"
        assert sorted(p.name for p in repo_dir.iterdir()) == [".bare", ".lock"]

    def test_the_waiting_process_is_told_what_it_is_waiting_for(
        self, isolated_devlaunch_env, local_git_repo, tmp_path
    ):
        # A first launch of a large repo can sit for a minute behind a sibling's
        # clone, and the two look identical from outside: a dl that has printed
        # nothing. `hold_lock` takes a `waiting_note` so the second one says
        # why, and `ensure_repo` is the caller that passes it. Nothing checked
        # that the note names the repo, or that it reaches stderr rather than
        # the stdout the completion machinery parses.
        env = isolated_devlaunch_env
        winner, loser, clone_now = self.stage(env, local_git_repo, tmp_path)
        try:
            clone_now.write_text("go", encoding="utf-8")
            finish(winner, "the winner", timeout=CLONE_TIMEOUT)
            out, err = loser.communicate(timeout=CLONE_TIMEOUT)
        finally:
            stop(winner)
            stop(loser)

        assert loser.returncode == 0, err
        assert "dl: waiting for another dl run preparing test/repo" in err, (
            f"the waiting run said nothing about why it was waiting:\n{err}"
        )
        assert "waiting for" not in out, "the note belongs on stderr, which is not parsed"
