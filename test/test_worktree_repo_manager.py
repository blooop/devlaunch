"""Tests for worktree repository manager."""
# pylint: disable=redefined-outer-name,unused-argument,protected-access,unused-variable

import subprocess
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from devlaunch.worktree.models import BaseRepository
from devlaunch.worktree.repo_manager import RepositoryManager
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
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        (repo_path / ".git").mkdir()
        assert repo_manager.repo_exists("owner", "repo") is True

    def test_repo_exists_no_git_dir(self, repo_manager):
        """Test repo_exists returns False for directory without .git."""
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        assert repo_manager.repo_exists("owner", "repo") is False

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_clone_repo_success(self, mock_run, repo_manager):
        """Test successful repository clone."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        # Create .git directory to simulate clone
        def create_git_dir(*args, **kwargs):
            repo_path = repo_manager.get_repo_path("owner", "repo")
            repo_path.mkdir(parents=True, exist_ok=True)
            (repo_path / ".git").mkdir(exist_ok=True)
            return MagicMock(stdout="main", stderr="", returncode=0)

        mock_run.side_effect = create_git_dir

        result = repo_manager.clone_repo("owner", "repo", "https://github.com/owner/repo.git")

        assert result is not None
        assert result.owner == "owner"
        assert result.repo == "repo"
        assert mock_run.called

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_clone_repo_already_exists(self, mock_run, repo_manager):
        """Test clone returns existing repo if already exists."""
        # Create existing repo
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        (repo_path / ".git").mkdir()

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=repo_path,
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
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        (repo_path / ".git").mkdir()

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=repo_path,
        )
        repo_manager.storage.add_repository(repo)

        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        repo_manager.fetch_repo("owner", "repo")

        assert mock_run.called
        call_args = mock_run.call_args[0][0]
        assert "fetch" in call_args
        assert "--all" in call_args

    def test_fetch_repo_not_exists(self, repo_manager):
        """Test fetch raises error for non-existent repo."""
        with pytest.raises(ValueError, match="does not exist"):
            repo_manager.fetch_repo("nonexistent", "repo")

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_fetch_repo_failure(self, mock_run, repo_manager):
        """Test fetch failure raises error."""
        # Create repo directory
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        (repo_path / ".git").mkdir()

        mock_run.side_effect = subprocess.CalledProcessError(1, "git fetch", stderr="Fetch failed")

        with pytest.raises(RuntimeError, match="Failed to fetch"):
            repo_manager.fetch_repo("owner", "repo")

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_ensure_repo_clones_if_not_exists(self, mock_run, repo_manager):
        """Test ensure_repo clones if repo doesn't exist."""

        def create_git_dir(*args, **kwargs):
            if "clone" in args[0]:
                repo_path = repo_manager.get_repo_path("owner", "repo")
                repo_path.mkdir(parents=True, exist_ok=True)
                (repo_path / ".git").mkdir(exist_ok=True)
            return MagicMock(stdout="main", stderr="", returncode=0)

        mock_run.side_effect = create_git_dir

        result = repo_manager.ensure_repo("owner", "repo", "https://github.com/owner/repo.git")

        assert result is not None
        assert result.owner == "owner"

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_ensure_repo_fetches_if_exists(self, mock_run, repo_manager):
        """Test ensure_repo fetches if repo exists."""
        # Create repo directory
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        (repo_path / ".git").mkdir()

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=repo_path,
        )
        repo_manager.storage.add_repository(repo)

        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        result = repo_manager.ensure_repo("owner", "repo", "https://github.com/owner/repo.git")

        assert result is not None
        assert mock_run.called
        call_args = mock_run.call_args[0][0]
        assert "fetch" in call_args

    @patch("devlaunch.worktree.repo_manager.subprocess.run")
    def test_ensure_repo_no_auto_fetch(self, mock_run, repo_manager):
        """Test ensure_repo with auto_fetch=False skips fetch."""
        # Create repo directory
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        (repo_path / ".git").mkdir()

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=repo_path,
        )
        repo_manager.storage.add_repository(repo)

        result = repo_manager.ensure_repo(
            "owner", "repo", "https://github.com/owner/repo.git", auto_fetch=False
        )

        assert result is not None
        assert not mock_run.called

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
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        (repo_path / ".git").mkdir()

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=repo_path,
        )
        repo_manager.storage.add_repository(repo)

        result = repo_manager.get_repo("owner", "repo")
        assert result is not None
        assert result.owner == "owner"

    def test_list_repositories(self, repo_manager):
        """Test listing repositories."""
        # Create repo directories
        repo_path1 = repo_manager.get_repo_path("owner1", "repo1")
        repo_path1.mkdir(parents=True)
        (repo_path1 / ".git").mkdir()

        repo_path2 = repo_manager.get_repo_path("owner2", "repo2")
        repo_path2.mkdir(parents=True)
        (repo_path2 / ".git").mkdir()

        # Add to storage
        repo1 = BaseRepository(
            owner="owner1",
            repo="repo1",
            remote_url="https://github.com/owner1/repo1.git",
            local_path=repo_path1,
        )
        repo2 = BaseRepository(
            owner="owner2",
            repo="repo2",
            remote_url="https://github.com/owner2/repo2.git",
            local_path=repo_path2,
        )
        repo_manager.storage.add_repository(repo1)
        repo_manager.storage.add_repository(repo2)

        repos = repo_manager.list_repositories()
        assert len(repos) == 2

    def test_remove_repository(self, repo_manager):
        """Test removing a repository."""
        # Create repo directory
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        (repo_path / ".git").mkdir()

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=repo_path,
        )
        repo_manager.storage.add_repository(repo)

        repo_manager.remove_repository("owner", "repo")

        assert repo_manager.storage.get_repository("owner", "repo") is None
        assert not repo_path.exists()  # Directory should be removed

    def test_remove_repository_keep_directory(self, repo_manager):
        """Test removing a repository without deleting directory."""
        # Create repo directory
        repo_path = repo_manager.get_repo_path("owner", "repo")
        repo_path.mkdir(parents=True)
        (repo_path / ".git").mkdir()

        # Add to storage
        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=repo_path,
        )
        repo_manager.storage.add_repository(repo)

        repo_manager.remove_repository("owner", "repo", remove_directory=False)

        assert repo_manager.storage.get_repository("owner", "repo") is None
        assert repo_path.exists()  # Directory should still exist


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
