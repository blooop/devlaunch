"""Tests for worktree repository manager."""
# pylint: disable=redefined-outer-name,unused-argument,protected-access,unused-variable

import inspect
import subprocess
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from devlaunch.worktree.models import BaseRepository
from devlaunch.worktree.repo_manager import (
    FetchFailed,
    RefMissingOnRemote,
    RepositoryManager,
    Updated,
)
from devlaunch.worktree.storage import MetadataStorage


@pytest.fixture
def temp_dirs():
    """Create temporary directories for testing."""
    with tempfile.TemporaryDirectory() as tmpdir:
        repos_dir = Path(tmpdir) / "repos"
        metadata_path = Path(tmpdir) / "metadata.json"
        repos_dir.mkdir()
        yield repos_dir, metadata_path


@pytest.fixture
def repo_manager(temp_dirs):
    """Create a repository manager with temporary storage."""
    repos_dir, metadata_path = temp_dirs
    storage = MetadataStorage(metadata_path)
    return RepositoryManager(repos_dir, storage)


class TestRepositoryManager:
    """Tests for RepositoryManager class."""

    def test_init_creates_repos_dir(self, temp_dirs):
        """Test that initialization creates repos directory."""
        repos_dir, metadata_path = temp_dirs
        storage = MetadataStorage(metadata_path)
        new_repos_dir = repos_dir / "new_subdir"
        manager = RepositoryManager(new_repos_dir, storage)
        assert new_repos_dir.exists()

    def test_get_repo_path(self, repo_manager, temp_dirs):
        """Test getting repository path."""
        repos_dir, _ = temp_dirs
        path = repo_manager.get_repo_path("owner", "repo")
        assert path == repos_dir / "owner" / "repo"

    def test_repo_exists_false(self, repo_manager):
        """Test repo_exists returns False for non-existent repo."""
        assert repo_manager.repo_exists("nonexistent", "repo") is False

    def test_repo_exists_true(self, repo_manager):
        """Test repo_exists returns True for existing repo."""
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")
        assert repo_manager.repo_exists("owner", "repo") is True

    def test_repo_exists_no_bare_dir(self, repo_manager):
        """Test repo_exists returns False for directory without .bare."""
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        assert repo_manager.repo_exists("owner", "repo") is False

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_clone_repo_success(self, mock_run, repo_manager):
        """Test successful repository clone."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        # Create .bare directory to simulate clone
        def create_bare_dir(*args, **kwargs):
            bare_path = repo_manager.get_bare_path("owner", "repo")
            bare_path.mkdir(parents=True, exist_ok=True)
            (bare_path / "HEAD").write_text("ref: refs/heads/main\n")
            return MagicMock(stdout="main", stderr="", returncode=0)

        mock_run.side_effect = create_bare_dir

        result = repo_manager.clone_repo("owner", "repo", "https://github.com/owner/repo.git")

        assert result is not None
        assert result.owner == "owner"
        assert result.repo == "repo"
        assert mock_run.called

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_clone_repo_already_exists(self, mock_run, repo_manager):
        """Test clone returns existing repo if already exists."""
        # Create existing repo
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=bare_path,
        )
        repo_manager.storage.add_repository(repo)

        result = repo_manager.clone_repo("owner", "repo", "https://github.com/owner/repo.git")

        assert result is not None
        assert result.owner == "owner"
        # Clone should not be called
        assert not any("clone" in str(call) for call in mock_run.call_args_list)

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_clone_repo_failure(self, mock_run, repo_manager):
        """Test clone failure raises error."""
        mock_run.side_effect = subprocess.CalledProcessError(1, "git clone", stderr="Clone failed")

        with pytest.raises(RuntimeError, match="Failed to clone"):
            repo_manager.clone_repo("owner", "repo", "https://github.com/owner/repo.git")

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_fetch_repo_success(self, mock_run, repo_manager):
        """Test successful repository fetch."""
        # Create repo directory
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=bare_path,
        )
        repo_manager.storage.add_repository(repo)

        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        repo_manager.fetch_repo("owner", "repo")

        assert mock_run.called
        call_args = mock_run.call_args[0][0]
        assert "fetch" in call_args
        assert "origin" in call_args
        assert "+refs/heads/*:refs/heads/*" in call_args

    def test_fetch_repo_not_exists(self, repo_manager):
        """Test fetch raises error for non-existent repo."""
        with pytest.raises(ValueError, match="does not exist"):
            repo_manager.fetch_repo("nonexistent", "repo")

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_fetch_repo_failure(self, mock_run, repo_manager):
        """Test fetch failure raises error."""
        # Create repo directory
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")

        mock_run.side_effect = subprocess.CalledProcessError(1, "git fetch", stderr="Fetch failed")

        with pytest.raises(RuntimeError, match="Failed to fetch"):
            repo_manager.fetch_repo("owner", "repo")

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_ensure_repo_clones_if_not_exists(self, mock_run, repo_manager):
        """Test ensure_repo clones if repo doesn't exist."""

        def create_bare_dir(*args, **kwargs):
            if "clone" in args[0]:
                bare_path = repo_manager.get_bare_path("owner", "repo")
                bare_path.mkdir(parents=True, exist_ok=True)
                (bare_path / "HEAD").write_text("ref: refs/heads/main\n")
            return MagicMock(stdout="main", stderr="", returncode=0)

        mock_run.side_effect = create_bare_dir

        result = repo_manager.ensure_repo("owner", "repo", "https://github.com/owner/repo.git")

        assert result is not None
        assert result.owner == "owner"

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_ensure_repo_never_fetches_an_existing_clone(self, mock_run, repo_manager):
        """A cache that is already there is returned as-is, however stale.

        ensure_repo is the clone-if-missing primitive and nothing else: freshness
        is the background sweep's job, and the launch path's one network call is
        the targeted ref fetch in ensure_branch. A fetch here would put an
        unbounded network round-trip back under the repo lock, which is the whole
        defect devlaunch#144 resolved.
        """
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")

        # last_fetched=None is the strongest form of "the interval has elapsed":
        # the old lazy-fetch gate fetched unconditionally in this state.
        repo_manager.storage.add_repository(
            BaseRepository(
                owner="owner",
                repo="repo",
                remote_url="https://github.com/owner/repo.git",
                local_path=bare_path,
                last_fetched=None,
            )
        )

        result = repo_manager.ensure_repo("owner", "repo", "https://github.com/owner/repo.git")

        assert result is not None
        assert not mock_run.called

    def test_ensure_repo_takes_no_fetch_flag(self, repo_manager):
        """The auto_fetch knob is gone rather than defaulted.

        Pinned as a signature, because a parameter left in place accepting True
        would let a caller ask for the foreground fetch that no longer exists.
        """
        assert "auto_fetch" not in inspect.signature(repo_manager.ensure_repo).parameters

    def test_get_repo_returns_none_if_dir_missing(self, repo_manager):
        """Test get_repo returns None if directory is missing."""
        # Add to storage without creating directory
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=repo_manager.get_repo_path("owner", "repo"),
        )
        repo_manager.storage.add_repository(repo)

        result = repo_manager.get_repo("owner", "repo")
        assert result is None

    def test_get_repo_returns_repo_if_exists(self, repo_manager):
        """Test get_repo returns repo if exists."""
        # Create repo directory
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=bare_path,
        )
        repo_manager.storage.add_repository(repo)

        result = repo_manager.get_repo("owner", "repo")
        assert result is not None
        assert result.owner == "owner"

    def test_list_repositories(self, repo_manager):
        """Test listing repositories."""
        # Create repo directories
        bare_path1 = repo_manager.get_bare_path("owner1", "repo1")
        bare_path1.mkdir(parents=True)
        (bare_path1 / "HEAD").write_text("ref: refs/heads/main\n")

        bare_path2 = repo_manager.get_bare_path("owner2", "repo2")
        bare_path2.mkdir(parents=True)
        (bare_path2 / "HEAD").write_text("ref: refs/heads/main\n")

        # Add to storage
        repo1 = BaseRepository(
            owner="owner1",
            repo="repo1",
            remote_url="https://github.com/owner1/repo1.git",
            local_path=bare_path1,
        )
        repo2 = BaseRepository(
            owner="owner2",
            repo="repo2",
            remote_url="https://github.com/owner2/repo2.git",
            local_path=bare_path2,
        )
        repo_manager.storage.add_repository(repo1)
        repo_manager.storage.add_repository(repo2)

        repos = repo_manager.list_repositories()
        assert len(repos) == 2

    def test_remove_repository(self, repo_manager):
        """Test removing a repository."""
        # Create repo directory
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")
        repo_path = repo_manager.get_repo_path("owner", "repo")

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=bare_path,
        )
        repo_manager.storage.add_repository(repo)

        repo_manager.remove_repository("owner", "repo")

        assert repo_manager.storage.get_repository("owner", "repo") is None
        assert not repo_path.exists()  # Entire repo dir should be removed

    def test_remove_repository_keep_directory(self, repo_manager):
        """Test removing a repository without deleting directory."""
        # Create repo directory
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")
        repo_path = repo_manager.get_repo_path("owner", "repo")

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=bare_path,
        )
        repo_manager.storage.add_repository(repo)

        repo_manager.remove_repository("owner", "repo", remove_directory=False)

        assert repo_manager.storage.get_repository("owner", "repo") is None
        assert repo_path.exists()  # Directory should still exist


class TestFetchRef:
    """Tests for fetch_ref — the launch path's one network call."""

    @pytest.fixture
    def cached_repo(self, repo_manager):
        """A bare cache on disk with a metadata record, never fetched."""
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")
        repo_manager.storage.add_repository(
            BaseRepository(
                owner="owner",
                repo="repo",
                remote_url="https://github.com/owner/repo.git",
                local_path=bare_path,
                last_fetched=None,
            )
        )
        return bare_path

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_fetches_only_the_one_requested_ref(self, mock_run, repo_manager, cached_repo):
        """The refspec names the branch and nothing else.

        The point of the whole change: a wildcard here is an unbounded fetch of
        every head and tag on the launch path.
        """
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        outcome = repo_manager.fetch_ref("owner", "repo", "feature/x")

        assert isinstance(outcome, Updated)
        argv = mock_run.call_args[0][0]
        assert argv == [
            "git",
            "fetch",
            "origin",
            "+refs/heads/feature/x:refs/heads/feature/x",
        ]
        assert mock_run.call_args[1]["cwd"] == cached_repo

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_ref_absent_from_the_remote_is_its_own_answer(
        self, mock_run, repo_manager, cached_repo
    ):
        """A branch nobody has pushed is not a failure.

        Distinct from FetchFailed because the caller does something different with
        it — bases a new branch on the default branch — and reporting it as a
        failure would send an ordinary "start a new branch" launch down the
        offline path.
        """
        mock_run.side_effect = subprocess.CalledProcessError(
            128, "git fetch", stderr="fatal: couldn't find remote ref refs/heads/nosuch\n"
        )

        outcome = repo_manager.fetch_ref("owner", "repo", "nosuch")

        assert isinstance(outcome, RefMissingOnRemote)

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_unreachable_remote_carries_its_reason(self, mock_run, repo_manager, cached_repo):
        """Offline is a third answer, and it keeps what git said.

        The reason is carried rather than reconstructed at the print site: "no
        such host", an expired credential and a refused connection all arrive
        here and read differently to whoever has to fix it.
        """
        mock_run.side_effect = subprocess.CalledProcessError(
            128, "git fetch", stderr="fatal: Could not read from remote repository\n"
        )

        outcome = repo_manager.fetch_ref("owner", "repo", "main")

        assert isinstance(outcome, FetchFailed)
        assert "Could not read from remote repository" in outcome.reason

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_does_not_advance_last_fetched(self, mock_run, repo_manager, cached_repo):
        """One ref is not the sweep, so it must not claim the sweep's bookkeeping.

        Advancing last_fetched here would suppress the background sweep's broad
        fetch for a whole interval on the strength of having fetched a single
        branch — every other ref silently starved by the thing meant to keep the
        launch path cheap. It also keeps the repo→metadata lock nesting off this
        path entirely.
        """
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        repo_manager.fetch_ref("owner", "repo", "main")

        assert repo_manager.storage.get_repository("owner", "repo").last_fetched is None

    def test_rejects_a_ref_that_would_reach_git_as_an_option(self, repo_manager, cached_repo):
        """The branch is interpolated into a refspec, so it is checked first."""
        with pytest.raises(ValueError, match="Invalid git ref"):
            repo_manager.fetch_ref("owner", "repo", "--upload-pack=evil")

    def test_missing_cache_is_a_failure_not_a_missing_ref(self, repo_manager):
        """No clone to fetch into is a FetchFailed, not a claim about the remote.

        Reading it as RefMissingOnRemote would send the caller off to create the
        branch from a default branch that is equally not there.
        """
        repo_manager.storage.add_repository(
            BaseRepository(
                owner="owner",
                repo="repo",
                remote_url="https://github.com/owner/repo.git",
                local_path=repo_manager.get_bare_path("owner", "repo"),
            )
        )

        outcome = repo_manager.fetch_ref("owner", "repo", "main")

        assert isinstance(outcome, FetchFailed)


class TestLazyFetch:
    """Tests for lazy_fetch method."""

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_lazy_fetch_performs_fetch_when_interval_elapsed(self, mock_run, repo_manager):
        """Test lazy_fetch fetches when interval has elapsed."""
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")

        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=bare_path,
            last_fetched=None,  # Never fetched → should fetch
        )
        repo_manager.storage.add_repository(repo)
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        result = repo_manager.lazy_fetch("owner", "repo")

        assert result is True
        assert mock_run.called

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_lazy_fetch_skips_when_recent(self, mock_run, repo_manager):
        """Test lazy_fetch skips fetch when recently fetched."""
        from datetime import datetime

        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")

        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=bare_path,
            last_fetched=datetime.now(),  # Just fetched
        )
        repo_manager.storage.add_repository(repo)

        result = repo_manager.lazy_fetch("owner", "repo")

        assert result is False
        assert not mock_run.called

    def test_lazy_fetch_raises_when_repo_missing(self, repo_manager):
        """Test lazy_fetch raises ValueError when repo not in metadata."""
        with pytest.raises(ValueError, match="not found in metadata"):
            repo_manager.lazy_fetch("nonexistent", "repo")

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_lazy_fetch_propagates_fetch_error(self, mock_run, repo_manager):
        """Test lazy_fetch propagates errors from fetch_repo."""
        bare_path = repo_manager.get_bare_path("owner", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "HEAD").write_text("ref: refs/heads/main\n")

        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=bare_path,
            last_fetched=None,
        )
        repo_manager.storage.add_repository(repo)
        mock_run.side_effect = subprocess.CalledProcessError(1, "git fetch", stderr="fail")

        with pytest.raises(RuntimeError, match="Failed to fetch"):
            repo_manager.lazy_fetch("owner", "repo")


class TestGetDefaultBranch:
    """Tests for _get_default_branch method."""

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_get_default_branch_from_head(self, mock_run, repo_manager):
        """Test getting default branch from symbolic ref."""
        mock_run.return_value = MagicMock(
            stdout="refs/remotes/origin/main\n",
            stderr="",
            returncode=0,
        )

        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)

        result = repo_manager._get_default_branch(repo_path)
        assert result == "main"

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_get_default_branch_fallback_main(self, mock_run, repo_manager):
        """Test fallback to main branch."""
        # First call fails, second returns branches
        mock_run.side_effect = [
            subprocess.CalledProcessError(1, "git symbolic-ref"),
            MagicMock(stdout="origin/main\n", stderr="", returncode=0),
        ]

        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)

        result = repo_manager._get_default_branch(repo_path)
        assert result == "main"

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_get_default_branch_fallback_master(self, mock_run, repo_manager):
        """Test fallback to master branch."""
        # First call fails, second returns branches
        mock_run.side_effect = [
            subprocess.CalledProcessError(1, "git symbolic-ref"),
            MagicMock(stdout="origin/master\n", stderr="", returncode=0),
        ]

        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)

        result = repo_manager._get_default_branch(repo_path)
        assert result == "master"

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_get_default_branch_ultimate_fallback(self, mock_run, repo_manager):
        """Test ultimate fallback to main."""
        mock_run.side_effect = subprocess.CalledProcessError(1, "git")

        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)

        result = repo_manager._get_default_branch(repo_path)
        assert result == "main"
