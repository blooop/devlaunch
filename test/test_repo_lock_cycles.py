# pylint: disable=redefined-outer-name
"""Pin how many times each launch shape takes the per-repo lock.

The third structural pin of its kind, after `test_devpod_spawn_counts.py` (devpod
argv) and `test_cold_launch_fetches.py` (fetch counts), and it exists for the same
reason: the property is a *shape*, and a shape is invisible to any assertion about
what one function did.

The cold path used to take the per-repo lock four separate times -- clone-if-
missing, the targeted fetch, the branch creation's neighbour, the workspace clone
-- and between any two of them the sequence was interruptible. `--prune` could weigh
a half-filled clone, and two launches of different branches of one repo genuinely
interleaved. Collapsing them into one scope is devlaunch#200; the token that scope
mints is what stops them drifting apart again, and these counts are what stop the
scope itself quietly splitting back into four.

The token forbids an *unlocked* call. It permits any number of locked scopes, so it
cannot pin this on its own -- which is exactly why the count is measured too.

Counted at the locks seam: `devlaunch.worktree.locks.hold_lock` is the primitive
every repo-lock acquisition goes through, and the per-repo lock is told apart from
the metadata lock and the per-workspace launch lock by its path.
"""

import contextlib
import json
import subprocess
import sys
from pathlib import Path
from typing import List, Optional
from unittest.mock import patch

import pytest

from devlaunch.dl import main
from devlaunch.worktree import locks
from devlaunch.worktree.config import WorktreeConfig
from devlaunch.worktree.repo_manager import RepoLock, RepositoryManager
from devlaunch.worktree.storage import MetadataStorage
from devlaunch.worktree.workspace_clone import WorkspaceCloneManager
from devlaunch.workspace_id import WorkspaceId

OWNER, REPO = "owner", "repo"
REMOTE_URL = f"git@github.com:{OWNER}/{REPO}.git"


class RepoLockCycles:
    """Counts acquisitions of the per-repo lock, at the locks module.

    The real primitive still runs: this observes the lock the production code
    takes rather than standing in for it, so a test that miscounts is a test
    that was watching a launch which really did serialize that way.

    Only the per-repo lock is counted. dl takes two others -- the single
    metadata lock (`metadata.json.lock`) and the per-workspace launch lock
    (`<id>.lock`) -- and both are legitimately taken on a launch that touches no
    repo lock at all, so counting by path is what keeps "zero repo-lock cycles"
    a statement about this repo's lock.
    """

    def __init__(self):
        self.repo_locks: List[Path] = []
        self._real = locks.hold_lock

    @contextlib.contextmanager
    def hold_lock(self, lock_path, waiting_note=None):
        # `<repos_dir>/<owner>/<repo>/.lock` -- see RepositoryManager.lock_path.
        if Path(lock_path).name == ".lock":
            self.repo_locks.append(Path(lock_path))
        with self._real(lock_path, waiting_note) as contended:
            yield contended

    @property
    def cycles(self) -> int:
        return len(self.repo_locks)


class FakeGit:
    """Answers every git command a cold launch makes, with its side effects.

    Success for all of them, plus the two things code downstream actually reads
    off the filesystem: a bare clone has to leave a `HEAD` behind or
    `repo_exists` reports the clone that was just made as absent, and a
    workspace clone has to leave a `.git` or `workspace_exists` does the same.
    """

    def __init__(self):
        self.calls: List[List[str]] = []

    def __call__(self, argv, **kwargs) -> subprocess.CompletedProcess:
        argv = list(argv)
        self.calls.append(argv)
        stdout = ""
        if argv[:3] == ["git", "clone", "--bare"]:
            bare = Path(argv[-1])
            bare.mkdir(parents=True, exist_ok=True)
            (bare / "HEAD").write_text("ref: refs/heads/main\n")
        elif argv[:2] == ["git", "clone"]:
            (Path(argv[-1]) / ".git").mkdir(parents=True, exist_ok=True)
        elif argv[:2] == ["git", "symbolic-ref"]:
            stdout = "refs/heads/main\n"
        return subprocess.CompletedProcess(args=argv, returncode=0, stdout=stdout, stderr="")


class FakeWorld(FakeGit):
    """Every subprocess a launch makes: devpod on top of :class:`FakeGit`.

    One object for both, because there is only one thing to patch.
    ``devlaunch.dl.subprocess`` and ``devlaunch.worktree.repo_manager.subprocess``
    are the same module object, so patching `subprocess.run` "in dl" and again
    "in the worktree modules" is two patches of one attribute, and the second
    silently replaces the first.

    `devpod status` exits non-zero for an id devpod does not have, and dl reads
    that as "cold" -- so which ids are listed in *known* is what selects the warm
    and cold shapes below.
    """

    def __init__(self, known: Optional[List[str]] = None):
        super().__init__()
        self.known = list(known or [])
        self.commands: List[List[str]] = []

    def __call__(self, argv, **kwargs) -> subprocess.CompletedProcess:
        argv = list(argv)
        if argv[:1] != ["devpod"]:
            return super().__call__(argv, **kwargs)
        self.commands.append(argv)
        if argv[:2] == ["devpod", "status"]:
            if argv[2] not in self.known:
                return subprocess.CompletedProcess(args=argv, returncode=1, stdout="", stderr="")
            return subprocess.CompletedProcess(
                args=argv,
                returncode=0,
                stdout=json.dumps({"id": argv[2], "state": "Running"}),
                stderr="",
            )
        if argv[:3] == ["devpod", "context", "options"]:
            return subprocess.CompletedProcess(args=argv, returncode=0, stdout="{}", stderr="")
        if argv[:2] == ["devpod", "list"]:
            return subprocess.CompletedProcess(args=argv, returncode=0, stdout="[]", stderr="")
        return subprocess.CompletedProcess(args=argv, returncode=0, stdout="", stderr="")

    def popen(self, cmd, *_args, **_kwargs):
        self.commands.append(list(cmd))
        return FinishedSession(list(cmd))


class FinishedSession:
    """A `devpod ssh` that exited 0 with nothing on stderr."""

    def __init__(self, argv: List[str]):
        self.args = argv
        self.stderr = __import__("io").StringIO("")
        self.returncode = 0

    def __enter__(self) -> "FinishedSession":
        return self

    def __exit__(self, *_exc) -> bool:
        return False


@pytest.fixture
def cycles():
    """Count repo-lock cycles for whatever the test then runs."""
    counter = RepoLockCycles()
    with patch.object(locks, "hold_lock", counter.hold_lock):
        yield counter


@contextlib.contextmanager
def launching(known: Optional[List[str]] = None):
    """Everything a `dl <spec> -- cmd` needs that is not dl's own code."""
    world = FakeWorld(known)
    with patch("devlaunch.dl.subprocess.run", side_effect=world):
        with patch("devlaunch.dl.subprocess.Popen", side_effect=world.popen):
            with patch("devlaunch.dl.update_cache_background"):
                # git-lfs absent: the LFS fork is a separate concern, pinned
                # elsewhere.
                with patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None):
                    yield world


def run_dl(*argv: str) -> int:
    with patch.object(sys, "argv", ["dl", *argv]):
        return main()


class TestLockCyclesPerLaunchShape:
    """One number per shape, and the ticket's whole invariant is these four."""

    def test_a_cold_named_branch_launch_takes_the_lock_once(self, cycles):
        """The headline: clone-if-missing, the fetch, the branch and the
        workspace clone all happen inside one scope that owns the lock for the
        whole of them, so nothing can interleave partway through."""
        with launching(known=[]):
            assert run_dl(f"{OWNER}/{REPO}@feature/x", "--", "echo", "hi") == 0
        assert cycles.cycles == 1

    def test_a_cold_bare_spec_launch_takes_the_lock_twice(self, cycles):
        """A bare `owner/repo` pays one extra cycle to learn the default branch.

        Deliberate, and cheaper than the alternatives: folding it in means
        holding the repo lock across the fast-attach `devpod status` -- a
        subprocess every sibling launch of this repo would then queue behind --
        to save one uncontended flock. Only the branch *name* crosses the gap,
        and the collapsed scope's first act re-verifies clone-if-missing under
        its own lock.
        """
        with launching(known=[]):
            assert run_dl(f"{OWNER}/{REPO}", "--", "echo", "hi") == 0
        assert cycles.cycles == 2

    def test_a_warm_named_branch_launch_takes_the_lock_not_at_all(self, cycles):
        """The fast path (#145) pinned from the lock side: a workspace devpod
        already knows needs no clone, so it must not touch the repo lock --
        which is also what keeps it from queueing behind a sibling's cold
        launch of the same repo."""
        warm = WorkspaceId(OWNER, REPO, "feature/x").value
        with launching(known=[warm]):
            assert run_dl(f"{OWNER}/{REPO}@feature/x", "--", "echo", "hi") == 0
        assert cycles.cycles == 0

    def test_a_warm_bare_spec_launch_takes_the_lock_once(self, cycles):
        """Only to name the default branch; the launch itself is still warm."""
        warm = WorkspaceId(OWNER, REPO, "main").value
        with launching(known=[warm]):
            assert run_dl(f"{OWNER}/{REPO}", "--", "echo", "hi") == 0
        assert cycles.cycles == 1


@pytest.fixture
def clone_manager(tmp_path):
    """A clone manager over an empty cache, wired to real storage on disk."""
    repos_dir = tmp_path / "repos"
    repos_dir.mkdir()
    storage = MetadataStorage(tmp_path / "metadata.json")
    config = WorktreeConfig(repos_dir=repos_dir)
    repo_manager = RepositoryManager(repos_dir, storage, config=config)
    return WorkspaceCloneManager(config=config, repo_manager=repo_manager, storage=storage)


class TestTheTokenIsProofOfTheLock:
    """What the token replaces: three comments asking callers not to re-lock.

    A signature that cannot be satisfied without the lock states the rule; a
    comment that says "hold_lock is not reentrant, so..." only warns about it,
    and the warning is not read by the caller who deadlocks.
    """

    def test_only_the_lock_scope_mints_a_token(self):
        """Constructing one by hand is refused, so a token in a signature means
        the lock was really held rather than that somebody typed the type."""
        with pytest.raises(TypeError):
            RepoLock(OWNER, REPO)

    def test_the_lock_scope_hands_out_a_token_for_the_repo_it_locked(self, clone_manager):
        with clone_manager.repo_manager.hold_repo_lock(OWNER, REPO) as lock:
            assert (lock.owner, lock.repo) == (OWNER, REPO)

    def test_a_token_for_one_repo_cannot_vouch_for_another(self, clone_manager):
        """The reason the token carries the pair rather than being a bare marker.

        A lock on `owner/repo` says nothing about `owner/other`, and a marker
        type with no identity would have let one stand in for the other -- the
        lock still held, the wrong repository still unserialized.
        """
        with clone_manager.repo_manager.hold_repo_lock(OWNER, REPO) as lock:
            with pytest.raises(ValueError):
                clone_manager.repo_manager.clone_if_missing(lock, OWNER, "other", REMOTE_URL)
            with pytest.raises(ValueError):
                clone_manager.ensure_branch(lock, OWNER, "other", "feature/x")


class TestTheColdEntrypointHoldsTheLockThroughout:
    """The scope's own guarantee, at the manager rather than at the CLI."""

    def test_prepare_cold_holds_the_lock_for_every_step_it_runs(self, clone_manager, cycles):
        """One cycle, and the git work happens inside it.

        Measured rather than asserted about the source: each recorded git call
        asks the real lock file whether it is held right now, the same probe
        test_cold_launch_fetches.py uses.
        """
        git = FakeGit()
        held: List[bool] = []
        lock_path = clone_manager.repo_manager.lock_path(OWNER, REPO)

        def recording(argv, **kwargs):
            held.append(_lock_is_held(lock_path))
            return git(argv, **kwargs)

        with patch("devlaunch.worktree.repo_manager.subprocess.run", recording):
            with patch("devlaunch.worktree.workspace_clone.subprocess.run", recording):
                with patch("devlaunch.worktree.branch_manager.subprocess.run", recording):
                    with patch(
                        "devlaunch.worktree.workspace_clone.shutil.which", return_value=None
                    ):
                        ws_path = clone_manager.prepare_cold(OWNER, REPO, "feature/x", REMOTE_URL)

        assert cycles.cycles == 1
        assert held and all(held), "a git call ran outside the lock the entrypoint took"
        assert ws_path == clone_manager.get_workspace_path(OWNER, REPO, "feature/x")


def _lock_is_held(lock_path: Path) -> bool:
    """Whether some open file description holds *lock_path* right now.

    flock conflicts between separate open file descriptions even within one
    process, so this reads the truth about the real lock the real code took.
    """
    import fcntl
    import os

    if not lock_path.exists():
        return False
    fd = os.open(lock_path, os.O_RDWR)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    except BlockingIOError:
        return True
    finally:
        os.close(fd)
