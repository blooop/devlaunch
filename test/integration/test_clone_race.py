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

So both processes here are real, started with ``sys.executable``, and the
"remote" they clone is a bare repository in the same ``tmp_path`` — a real
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
    config=WorktreeConfig(repos_dir=repos_dir, auto_fetch=False, fetch_interval=0),
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
    ready, = flags
    ready.write_text("ready", encoding="utf-8")
    if role == "racer":
        wait_for(ready.parent / "go")
    got = manager.ensure_repo("test", "repo", remote, auto_fetch=False)
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
    def test_a_simultaneous_start_produces_one_clone_and_two_winners(
        self, isolated_devlaunch_env, local_git_repo, tmp_path
    ):
        # Both processes released at once, which is the shape the bug arrived
        # in. Which of them clones is genuinely undecided and the assertions do
        # not care: what has to hold either way is that one clone exists, both
        # callers were handed it, and neither was handed an error.
        env = isolated_devlaunch_env
        args = [env["repos_dir"], env["metadata_path"], local_git_repo["remote_url"]]
        first_ready = tmp_path / "first-ready"
        second_ready = tmp_path / "second-ready"

        first = spawn_driver(DRIVER, ["racer", *args, first_ready], tmp_path, "racer_one")
        second = spawn_driver(DRIVER, ["racer", *args, second_ready], tmp_path, "racer_two")
        try:
            await_flags(first_ready, second_ready)
            (tmp_path / "go").write_text("go", encoding="utf-8")
            said_first = finish(first, "the first racer", timeout=CLONE_TIMEOUT).strip()
            said_second = finish(second, "the second racer", timeout=CLONE_TIMEOUT).strip()
        finally:
            for proc in (first, second):
                if proc.poll() is None:
                    proc.kill()

        bare_path = env["repos_dir"] / "test" / "repo" / ".bare"
        assert said_first == said_second == str(bare_path), "the two runs disagree on the cache"
        assert head_of(bare_path), "the surviving clone is not a working repository"

        repo_dir = env["repos_dir"] / "test" / "repo"
        assert sorted(p.name for p in repo_dir.iterdir()) == [".bare", ".lock"]
        assert list(cache_of(env["metadata_path"])["repositories"]) == ["test/repo"]

    def test_the_waiting_process_adopts_the_clone_it_waited_for(
        self, isolated_devlaunch_env, local_git_repo, tmp_path
    ):
        # The same race, staged so the interesting ordering happens every time
        # rather than sometimes. The loser loads its metadata *before* the
        # winner writes any, so when it finally gets the lock it is looking at a
        # `.bare` on disk that its own records have never heard of — which is
        # exactly the "another process just made it" case `clone_repo` adopts.
        # Reached here across two interpreters instead of by deleting a record.
        env = isolated_devlaunch_env
        args = [env["repos_dir"], env["metadata_path"], local_git_repo["remote_url"]]
        held = tmp_path / "held"
        clone_now = tmp_path / "clone-now"
        loaded = tmp_path / "loaded"

        winner = spawn_driver(DRIVER, ["winner", *args, held, clone_now], tmp_path, "winner")
        loser = None
        try:
            await_flags(held)

            loser = spawn_driver(DRIVER, ["loser", *args, loaded], tmp_path, "loser")
            await_flags(loaded)
            await_blocked_on_lock(loser)
            assert loser.poll() is None, "the loser walked straight past a held lock"

            clone_now.write_text("go", encoding="utf-8")
            said_winner = finish(winner, "the winner", timeout=CLONE_TIMEOUT).strip()
            said_loser = finish(loser, "the loser", timeout=CLONE_TIMEOUT).strip()
        finally:
            for proc in (winner, loser):
                if proc is not None and proc.poll() is None:
                    proc.kill()

        bare_path = env["repos_dir"] / "test" / "repo" / ".bare"
        assert said_winner == said_loser == str(bare_path)
        # The whole point. The loser found a clone it had no record of and took
        # it as the authority: it did not clone over it, and its own failure
        # cleanup did not delete it.
        assert (bare_path / "winner-was-here").exists(), "the loser destroyed the winner's clone"
        assert head_of(bare_path)
        assert list(cache_of(env["metadata_path"])["repositories"]) == ["test/repo"]
