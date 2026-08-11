"""Two dl processes launching at the same moment must not corrupt shared state.

The shared state is the bare-clone cache and metadata.json. Before the
inter-process locks existed, two simultaneous first launches raced twice over:

- both saw no ``.bare`` and both ran ``git clone --bare`` into the same path;
  the loser's error handler then ``rmtree``'d the winner's in-progress clone.
- each process rewrote metadata.json from an in-memory copy loaded at startup,
  so the last writer silently dropped the other's workspace record.

The storage tests here are deterministic reproductions of the lost update; the
subprocess tests drive the real clone paths from separate processes the way two
concurrent ``dl owner/repo@branch`` runs do.
"""

import time
import subprocess
from pathlib import Path

from devlaunch.worktree.models import BaseRepository
from devlaunch.worktree.storage import MetadataStorage
from fixtures.subprocess_drivers import await_flags, finish, spawn_driver

# The repo-preparation lock every dl process must take before mutating
# repos/<owner>/<repo>. The tests spell the path out rather than asking the
# implementation for it: the on-disk location is the contract between
# processes, and a refactor that moves it breaks that contract even if every
# in-tree caller moves with it.
REPO_LOCK_LEAF = ".lock"


def _repo(owner: str, name: str, tmp_path: Path) -> BaseRepository:
    return BaseRepository(
        owner=owner,
        repo=name,
        remote_url=f"https://example.invalid/{owner}/{name}.git",
        local_path=tmp_path / owner / name / ".bare",
    )


class TestMetadataLostUpdates:
    """Interleaved writers on one metadata.json must not drop each other's records."""

    def test_interleaved_repository_writers_both_survive(self, tmp_path):
        path = tmp_path / "metadata.json"
        # Two handles on one file, both loaded while it is empty — exactly the
        # state two dl processes are in just after startup.
        first = MetadataStorage(path)
        second = MetadataStorage(path)

        first.add_repository(_repo("owner", "one", tmp_path))
        second.add_repository(_repo("owner", "two", tmp_path))

        fresh = MetadataStorage(path)
        assert fresh.get_repository("owner", "one") is not None
        assert fresh.get_repository("owner", "two") is not None

    def test_interleaved_worktree_writers_both_survive(self, tmp_path):
        from devlaunch.worktree.models import WorktreeInfo

        path = tmp_path / "metadata.json"
        first = MetadataStorage(path)
        second = MetadataStorage(path)

        for storage, branch in ((first, "alpha"), (second, "beta")):
            storage.add_worktree(
                WorktreeInfo(
                    owner="owner",
                    repo="repo",
                    branch=branch,
                    local_path=tmp_path / branch,
                    workspace_id=f"repo-{branch}-abcdefgh",
                )
            )

        fresh = MetadataStorage(path)
        assert fresh.get_worktree("owner", "repo", "alpha") is not None
        assert fresh.get_worktree("owner", "repo", "beta") is not None


# --- subprocess drivers -----------------------------------------------------
#
# Written to disk and run with sys.executable so each side is a real process,
# the way two dl invocations are. The gate files make the start simultaneous:
# both processes construct their managers, declare ready, and block until the
# parent drops the "go" file.

_ENSURE_WORKSPACE_DRIVER = """
import sys, time
from pathlib import Path
from devlaunch.worktree.config import WorktreeConfig
from devlaunch.worktree.storage import MetadataStorage
from devlaunch.worktree.workspace_clone import WorkspaceCloneManager

repos_dir, metadata_path, remote_url, branch, gate_dir, tag = sys.argv[1:7]
gate = Path(gate_dir)
manager = WorkspaceCloneManager(
    config=WorktreeConfig(repos_dir=repos_dir),
    storage=MetadataStorage(Path(metadata_path)),
)
(gate / f"ready-{tag}").touch()
while not (gate / "go").exists():
    time.sleep(0.001)
print(manager.ensure_workspace("caseowner", "caserepo", branch, remote_url))
"""

_ENSURE_REPO_DRIVER = """
import sys
from pathlib import Path
from devlaunch.worktree.repo_manager import RepositoryManager
from devlaunch.worktree.storage import MetadataStorage

repos_dir, metadata_path, remote_url = sys.argv[1:4]
manager = RepositoryManager(
    repos_dir=Path(repos_dir), storage=MetadataStorage(Path(metadata_path))
)
manager.ensure_repo("caseowner", "caserepo", remote_url)
print("done")
"""

_LOCK_HOLDER_DRIVER = """
import fcntl, os, sys, time
from pathlib import Path

lock_path, held_flag, release_flag = sys.argv[1:4]
Path(lock_path).parent.mkdir(parents=True, exist_ok=True)
fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
fcntl.flock(fd, fcntl.LOCK_EX)
Path(held_flag).touch()
while not Path(release_flag).exists():
    time.sleep(0.01)
os.close(fd)
"""


# How long a driver that runs a real `git clone --bare` gets. Longer than the
# harness default, which sizes a driver that only takes a lock: a clone of a
# fixture repo on a cold CI runner is slow rather than hung.
CLONE_TIMEOUT = 120


def _finish(proc: subprocess.Popen, label: str) -> str:
    return finish(proc, label, timeout=CLONE_TIMEOUT)


def _git_ok(path: Path, *args: str) -> bool:
    return (
        subprocess.run(
            ["git", "-C", str(path), *args], capture_output=True, text=True, check=False
        ).returncode
        == 0
    )


def _current_branch(path: Path) -> str:
    return subprocess.run(
        ["git", "-C", str(path), "rev-parse", "--abbrev-ref", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


class TestSimultaneousProcesses:
    def test_ensure_repo_waits_for_the_holder_of_the_repo_lock(self, tmp_path, local_git_repo):
        """A process preparing owner/repo blocks while another holds its lock.

        Deterministic serialization proof: a subprocess takes the lock and sits
        on it; ensure_repo in a second subprocess must not complete until the
        holder lets go. Before the locks existed the contender sailed straight
        through in well under the grace period.
        """
        repos_dir = tmp_path / "repos"
        lock_path = repos_dir / "caseowner" / "caserepo" / REPO_LOCK_LEAF
        held, release = tmp_path / "held", tmp_path / "release"

        holder = spawn_driver(_LOCK_HOLDER_DRIVER, [lock_path, held, release], tmp_path, "holder")
        try:
            await_flags(held)
            contender = spawn_driver(
                _ENSURE_REPO_DRIVER,
                [repos_dir, tmp_path / "metadata.json", local_git_repo["remote_url"]],
                tmp_path,
                "contender",
            )
            # Generous grace period: a tiny local clone takes well under a
            # second, so an unserialized contender is long gone by now.
            time.sleep(3)
            still_blocked = contender.poll() is None
            release.touch()
            _finish(contender, "contender")
            assert still_blocked, "ensure_repo completed while another process held the repo lock"
            assert _git_ok(repos_dir / "caseowner" / "caserepo" / ".bare", "rev-parse", "HEAD")
        finally:
            release.touch()
            _finish(holder, "holder")

    def test_simultaneous_first_launches_of_two_branches(self, tmp_path, local_git_repo):
        """Both processes survive racing over the first bare clone of one repo.

        This is `dl owner/repo@a` and `dl owner/repo@b` fired at the same
        moment on a cold cache — the exact scenario of running two agents on
        their own branches at once.
        """
        repos_dir, metadata = tmp_path / "repos", tmp_path / "metadata.json"
        gate = tmp_path / "gate"
        gate.mkdir()

        procs = [
            spawn_driver(
                _ENSURE_WORKSPACE_DRIVER,
                [repos_dir, metadata, local_git_repo["remote_url"], branch, gate, branch],
                tmp_path,
                f"launch-{branch}",
            )
            for branch in ("tmp-a", "tmp-b")
        ]
        await_flags(gate / "ready-tmp-a", gate / "ready-tmp-b")
        (gate / "go").touch()
        outputs = [_finish(proc, f"launch {i}") for i, proc in enumerate(procs)]

        # The shared bare cache survived both writers.
        assert _git_ok(repos_dir / "caseowner" / "caserepo" / ".bare", "rev-parse", "HEAD")

        # Each launch got its own clone, on its own branch.
        for branch, output in zip(("tmp-a", "tmp-b"), outputs):
            ws_path = Path(output.strip().splitlines()[-1])
            assert ws_path.is_dir(), f"workspace clone for {branch} missing: {ws_path}"
            assert _current_branch(ws_path) == branch

        # And neither launch's record displaced the other's.
        fresh = MetadataStorage(metadata)
        assert fresh.get_repository("caseowner", "caserepo") is not None
        assert fresh.get_worktree("caseowner", "caserepo", "tmp-a") is not None
        assert fresh.get_worktree("caseowner", "caserepo", "tmp-b") is not None

    def test_simultaneous_launches_of_the_same_branch(self, tmp_path, local_git_repo):
        """Double-firing one workspace must converge on a single valid clone."""
        repos_dir, metadata = tmp_path / "repos", tmp_path / "metadata.json"
        gate = tmp_path / "gate"
        gate.mkdir()

        procs = [
            spawn_driver(
                _ENSURE_WORKSPACE_DRIVER,
                [repos_dir, metadata, local_git_repo["remote_url"], "main", gate, tag],
                tmp_path,
                f"launch-{tag}",
            )
            for tag in ("one", "two")
        ]
        await_flags(gate / "ready-one", gate / "ready-two")
        (gate / "go").touch()
        outputs = [_finish(proc, f"launch {i}") for i, proc in enumerate(procs)]

        paths = {output.strip().splitlines()[-1] for output in outputs}
        assert len(paths) == 1, f"one workspace, two homes: {paths}"
        ws_path = Path(paths.pop())
        assert ws_path.is_dir()
        assert _current_branch(ws_path) == "main"
        assert MetadataStorage(metadata).get_worktree("caseowner", "caserepo", "main") is not None
