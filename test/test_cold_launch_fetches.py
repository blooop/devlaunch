# pylint: disable=redefined-outer-name
"""Pin what the cold launch path fetches, and what it holds the repo lock for.

The headline test of devlaunch#144. Two properties, and neither is visible from
a call-count:

1. **No broad fetch in the foreground at all.** The interval sweep of every head
   and tag belongs to the detached updater (devlaunch#149). A single
   ``+refs/heads/*`` refspec on this path is the whole regression, whatever else
   is true of the launch.
2. **Nothing unbounded runs under the repo lock.** This is the property that
   actually hurt: the launch that drew the short straw paid for everyone's
   freshness, and every concurrent launch of the same repo queued behind it. So
   the assertion is not just "which fetches ran" but "which fetches ran *while
   holding the lock other dl runs block on*".

Whether the lock was held is measured rather than inferred: each recorded git
call opens the lock file in a second file description and tries a non-blocking
``flock``. flock conflicts between separate open file descriptions even within
one process, so the probe reads the truth about the real lock the real code
took, with no bookkeeping for the production path to get wrong.

Counted at the ``subprocess`` boundary, following test/test_devpod_spawn_counts.py.
"""

import fcntl
import os
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional
from unittest.mock import MagicMock, patch

import pytest

from devlaunch.worktree.models import BaseRepository
from devlaunch.worktree.repo_manager import RepositoryManager
from devlaunch.worktree.storage import MetadataStorage
from devlaunch.worktree.workspace_clone import WorkspaceCloneManager
from devlaunch.worktree.config import WorktreeConfig


@dataclass(frozen=True)
class GitCall:
    """One git command, where it ran, and whether the repo lock was held."""

    argv: List[str]
    lock_held: bool
    cwd: Optional[Path]

    @property
    def is_fetch(self) -> bool:
        return self.argv[:2] == ["git", "fetch"]

    @property
    def refspecs(self) -> List[str]:
        """The refspec-shaped arguments, which is where a wildcard would hide."""
        return [a for a in self.argv if ":" in a and a.startswith(("+refs/", "refs/"))]


class GitCalls:
    """Stands in for subprocess.run, recording argv and lock state per call.

    Answers every git command the cold path makes with success, and materializes
    the one side effect the code downstream of it depends on: ``git clone`` has
    to leave a directory with a ``.git`` in it, or ``workspace_exists`` reports
    the clone that was just made as absent.
    """

    def __init__(self, lock_path: Path):
        self.lock_path = lock_path
        self.calls: List[GitCall] = []

    def _lock_is_held(self) -> bool:
        """Whether *some* open file description holds the repo lock right now.

        A second descriptor on the same path: flock is per-open-file-description,
        so this conflicts with the production code's lock even though both live in
        this one process. The probe releases immediately on close, so it can never
        be the thing a later call sees.
        """
        if not self.lock_path.exists():
            return False
        fd = os.open(self.lock_path, os.O_RDWR)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(fd, fcntl.LOCK_UN)
            return False
        except BlockingIOError:
            return True
        finally:
            os.close(fd)

    def __call__(self, argv, **kwargs):
        cwd = kwargs.get("cwd")
        self.calls.append(
            GitCall(
                argv=list(argv),
                lock_held=self._lock_is_held(),
                cwd=None if cwd is None else Path(cwd),
            )
        )

        if argv[:2] == ["git", "clone"]:
            # argv is [git, clone, <src>, <dest>] on the workspace path
            dest = Path(argv[-1])
            (dest / ".git").mkdir(parents=True, exist_ok=True)

        return MagicMock(stdout="", stderr="", returncode=0)

    @property
    def fetches(self) -> List[GitCall]:
        return [c for c in self.calls if c.is_fetch]


@pytest.fixture
def warm_cache(tmp_path):
    """A bare cache already on disk, with a metadata record that is overdue a sweep.

    ``last_fetched=None`` is deliberately the state the old interval gate treated
    as "fetch now, unconditionally": if any foreground broad fetch survives, this
    is the fixture that provokes it.
    """
    repos_dir = tmp_path / "repos"
    repos_dir.mkdir()
    storage = MetadataStorage(tmp_path / "metadata.json")
    repo_manager = RepositoryManager(
        repos_dir,
        storage,
        config=WorktreeConfig(repos_dir=repos_dir, fetch_interval=0),
    )

    bare_path = repo_manager.get_bare_path("owner", "repo")
    bare_path.mkdir(parents=True)
    (bare_path / "HEAD").write_text("ref: refs/heads/main\n")
    storage.add_repository(
        BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="git@github.com:owner/repo.git",
            local_path=bare_path,
            default_branch="main",
            last_fetched=None,
        )
    )

    clone_manager = WorkspaceCloneManager(
        config=WorktreeConfig(repos_dir=repos_dir, fetch_interval=0),
        repo_manager=repo_manager,
        storage=storage,
    )
    return clone_manager, repo_manager


@pytest.fixture
def git_calls(warm_cache):
    """Record every git call made by the three modules on the cold path."""
    clone_manager, repo_manager = warm_cache
    recorder = GitCalls(repo_manager.lock_path("owner", "repo"))
    targets = (
        "devlaunch.worktree.repo_manager.subprocess.run",
        "devlaunch.worktree.workspace_clone.subprocess.run",
        "devlaunch.worktree.branch_manager.subprocess.run",
    )
    with patch(targets[0], recorder), patch(targets[1], recorder), patch(targets[2], recorder):
        # git-lfs absent: the LFS fork is a separate concern, pinned elsewhere.
        with patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None):
            yield recorder, clone_manager


def cold_launch(clone_manager, branch="feature/x"):
    """The cold path dl.py takes when devpod does not know the workspace."""
    clone_manager.repo_manager.ensure_repo("owner", "repo", "git@github.com:owner/repo.git")
    clone_manager.ensure_branch("owner", "repo", branch)
    clone_manager.ensure_workspace("owner", "repo", branch, "git@github.com:owner/repo.git")


class TestColdLaunchFetches:
    """What a first launch of a branch on this machine costs in network calls."""

    def test_no_broad_fetch_anywhere_in_the_foreground(self, git_calls):
        """Not one wildcard refspec, however overdue the cache's sweep is.

        The fixture's ``last_fetched=None`` is the exact state that used to fetch
        every head and tag before the launch could proceed.
        """
        recorder, clone_manager = git_calls

        cold_launch(clone_manager)

        wildcards = [c for c in recorder.fetches if any("*" in r for r in c.refspecs)]
        assert wildcards == []
        assert not any("--tags" in c.argv or "--prune" in c.argv for c in recorder.fetches)

    def test_only_the_targeted_refspec_ever_runs_under_the_repo_lock(self, git_calls):
        """Whatever is fetched while others are blocked is one named branch.

        The lock is what makes a fetch here everyone's problem, so this is the
        assertion that pins the fix rather than merely its symptom.
        """
        recorder, clone_manager = git_calls

        cold_launch(clone_manager, branch="feature/x")

        locked_fetches = [c for c in recorder.fetches if c.lock_held]
        assert locked_fetches, "expected the targeted fetch to run under the repo lock"
        for fetch in locked_fetches:
            assert fetch.refspecs == ["+refs/heads/feature/x:refs/heads/feature/x"]

    def test_the_whole_launch_costs_one_fetch(self, git_calls):
        """One network call for a branch that exists upstream, and one only.

        The bound the staleness contract promises. A second fetch would mean
        either the deleted workspace-clone fetch or the broad sweep had come back.
        """
        recorder, clone_manager = git_calls

        cold_launch(clone_manager)

        assert [c.argv for c in recorder.fetches] == [
            ["git", "fetch", "origin", "+refs/heads/feature/x:refs/heads/feature/x"],
        ]

    def test_the_fetch_runs_in_the_bare_cache_not_the_workspace(self, git_calls):
        """The ref lands in the shared cache, which is what the clone is cut from.

        Fetching into the workspace instead would leave the bare cache stale for
        the next branch and put the round-trip after the clone, where it cannot
        affect what the clone contains.
        """
        recorder, clone_manager = git_calls
        bare_path = clone_manager.repo_manager.get_bare_path("owner", "repo")
        ws_path = clone_manager.get_workspace_path("owner", "repo", "feature/x")

        cold_launch(clone_manager)

        # The cwd git was actually given, read off the recorded call's kwargs.
        assert [c.cwd for c in recorder.fetches] == [bare_path]
        assert all(c.cwd != ws_path for c in recorder.fetches)


class TestReRegistration:
    """A clone already on disk that devpod has forgotten."""

    def test_re_registration_runs_no_fetch_in_the_workspace(self, git_calls):
        """Re-registering an existing clone makes no network call of its own.

        The workspace-clone fetch this pins the deletion of was unconditional for
        existing clones and its output was never read — the checkout is a plain
        ``git checkout <branch>`` against the local branch.
        """
        recorder, clone_manager = git_calls
        ws_path = clone_manager.get_workspace_path("owner", "repo", "feature/x")
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        clone_manager.ensure_workspace(
            "owner", "repo", "feature/x", "git@github.com:owner/repo.git"
        )

        assert recorder.fetches == []
        assert [c.argv for c in recorder.calls] == [["git", "checkout", "feature/x"]]


class TestFetchFailureStillLaunches:
    """Offline is a warning on this path, not an error."""

    def test_unreachable_remote_does_not_stop_ensure_branch(self, git_calls):
        """A cached branch still launches when the fetch cannot be made.

        ensure_branch must not raise: the cache may well hold everything this
        launch needs, and the error for "there is nothing to launch from" belongs
        to the checkout, which is where the cache is actually consulted.
        """
        recorder, clone_manager = git_calls

        def unreachable(argv, **kwargs):
            if argv[:2] == ["git", "fetch"]:
                raise subprocess.CalledProcessError(
                    128, argv, stderr="fatal: Could not read from remote repository\n"
                )
            return recorder(argv, **kwargs)

        with patch("devlaunch.worktree.repo_manager.subprocess.run", unreachable):
            clone_manager.ensure_branch("owner", "repo", "feature/x")
