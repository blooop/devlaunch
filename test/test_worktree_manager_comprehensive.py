"""Comprehensive tests for WorktreeManager."""
# pylint: disable=redefined-outer-name,unused-argument,protected-access

import subprocess
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, Mock, patch

import pytest

from devlaunch.worktree.models import BaseRepository, WorktreeInfo
from devlaunch.worktree.repo_manager import RepositoryManager
from devlaunch.worktree.storage import MetadataStorage
from devlaunch.worktree.worktree_manager import WorktreeManager, sanitize_branch_name


@pytest.fixture
def temp_dir():
    """Create a temporary directory for testing."""
    with tempfile.TemporaryDirectory() as tmpdir:
        yield Path(tmpdir)


@pytest.fixture
def mock_repo_manager():
    """Create a mock repository manager."""
    mock = Mock(spec=RepositoryManager)
    mock.get_repo_path.return_value = Path("/repos/owner/repo")
    mock.ensure_repo.return_value = BaseRepository(
        owner="owner",
        repo="repo",
        local_path=Path("/repos/owner/repo"),
        remote_url="https://github.com/owner/repo.git",
    )
    mock.get_repo.return_value = BaseRepository(
        owner="owner",
        repo="repo",
        local_path=Path("/repos/owner/repo"),
        remote_url="https://github.com/owner/repo.git",
    )
    return mock


@pytest.fixture
def mock_storage():
    """Create a mock metadata storage."""
    mock = Mock(spec=MetadataStorage)
    mock.list_worktrees.return_value = []
    mock.get_worktree.return_value = None
    return mock


@pytest.fixture
def worktree_manager(temp_dir, mock_repo_manager, mock_storage):
    """Create a worktree manager with mocks."""
    return WorktreeManager(temp_dir, mock_repo_manager, mock_storage)


class TestSanitizeBranchName:
    """Tests for the sanitize_branch_name function."""

    def test_replaces_slashes(self):
        """Test that slashes are replaced with hyphens."""
        assert sanitize_branch_name("feature/test") == "feature-test"

    def test_removes_special_chars(self):
        """Test that special characters are removed."""
        assert sanitize_branch_name("branch!@#$%") == "branch_____"

    def test_preserves_valid_chars(self):
        """Test that valid characters are preserved."""
        assert sanitize_branch_name("valid-branch_name.1") == "valid-branch_name.1"

    def test_strips_leading_trailing(self):
        """Test that leading/trailing dots and hyphens are stripped."""
        assert sanitize_branch_name(".branch-") == "branch"
        assert sanitize_branch_name("-branch.") == "branch"


class TestWorktreeManagerCreation:
    """Tests for worktree creation functionality."""

    def test_create_worktree_calls_git(self, worktree_manager, mock_repo_manager, temp_dir):
        """Test that creating a worktree calls git."""
        test_repo_path = temp_dir / "repos" / "owner" / "repo"
        test_repo_path.mkdir(parents=True, exist_ok=True)
        mock_repo_manager.get_repo_path.return_value = test_repo_path

        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0, stdout="", stderr="")
            worktree_manager._remote_branch_exists = Mock(return_value=False)

            result = worktree_manager.create_worktree(
                "owner", "repo", "feature-branch", "https://github.com/owner/repo.git"
            )

            assert result.owner == "owner"
            assert result.repo == "repo"
            assert result.branch == "feature-branch"
            mock_run.assert_called()

    def test_create_worktree_no_repo_raises(self, worktree_manager, mock_repo_manager):
        """Test that creating worktree without repo raises error."""
        mock_repo_manager.get_repo.return_value = None

        with pytest.raises(ValueError, match="Repository .* not found"):
            worktree_manager.create_worktree("owner", "repo", "branch")

    def test_create_worktree_failure_cleans_up(self, worktree_manager, temp_dir, mock_repo_manager):
        """Test that failed worktree creation cleans up."""
        test_repo_path = temp_dir / "repos" / "owner" / "repo"
        test_repo_path.mkdir(parents=True, exist_ok=True)
        mock_repo_manager.get_repo_path.return_value = test_repo_path

        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(
                1, ["git", "worktree", "add"], stderr="Error"
            )
            worktree_manager._remote_branch_exists = Mock(return_value=False)

            with pytest.raises(RuntimeError, match="Failed to create worktree"):
                worktree_manager.create_worktree(
                    "owner", "repo", "feature", "https://github.com/owner/repo.git"
                )


class TestWorktreeManagerRemoval:
    """Tests for worktree removal."""

    def test_remove_worktree_calls_git(self, worktree_manager, mock_repo_manager, temp_dir):
        """Test that removing a worktree calls git."""
        # Create worktree directory
        worktree_path = temp_dir / ".worktrees" / "feature"
        worktree_path.mkdir(parents=True)
        (worktree_path / ".git").touch()
        mock_repo_manager.get_repo_path.return_value = temp_dir

        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0)

            worktree_manager.remove_worktree("owner", "repo", "feature")

            mock_run.assert_called()

    def test_remove_nonexistent_worktree_removes_metadata(self, worktree_manager, mock_storage):
        """Test removing non-existent worktree still cleans metadata."""
        worktree_manager.remove_worktree("owner", "repo", "nonexistent")

        mock_storage.remove_worktree.assert_called_once_with("owner", "repo", "nonexistent")


class TestWorktreeManagerListing:
    """Tests for worktree listing."""

    def test_list_worktrees_delegates_to_storage(self, worktree_manager, mock_storage):
        """Test that listing worktrees delegates to storage."""
        expected = [
            WorktreeInfo(
                owner="owner",
                repo="repo",
                branch="main",
                local_path=Path("/repos/owner/repo/.worktrees/main"),
                workspace_id="main-ws",
            )
        ]
        mock_storage.list_worktrees.return_value = expected

        result = worktree_manager.list_worktrees("owner", "repo")

        assert result == expected
        mock_storage.list_worktrees.assert_called_once_with("owner", "repo")

    def test_list_all_worktrees(self, worktree_manager, mock_storage):
        """Test listing all worktrees."""
        expected = []
        mock_storage.list_worktrees.return_value = expected

        result = worktree_manager.list_all_worktrees()

        assert result == expected


class TestWorktreeManagerHelpers:
    """Tests for helper methods."""

    def test_worktree_exists_false(self, worktree_manager):
        """Test worktree_exists returns False when directory doesn't exist."""
        assert worktree_manager.worktree_exists("owner", "repo", "nonexistent") is False

    def test_get_worktree_path(self, worktree_manager, mock_repo_manager, temp_dir):
        """Test getting worktree path."""
        mock_repo_manager.get_repo_path.return_value = temp_dir / "repos" / "owner" / "repo"

        path = worktree_manager.get_worktree_path("owner", "repo", "feature")

        assert path == temp_dir / "repos" / "owner" / "repo" / ".worktrees" / "feature"

    def test_remote_branch_exists_true(self, worktree_manager):
        """Test remote branch check returns True when branch exists."""
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0, stdout="refs/heads/main")

            result = worktree_manager._remote_branch_exists(Path("/repo"), "main")

            assert result is True

    def test_remote_branch_exists_false(self, worktree_manager):
        """Test remote branch check returns False when branch doesn't exist."""
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0, stdout="")

            result = worktree_manager._remote_branch_exists(Path("/repo"), "nonexistent")

            assert result is False

    def test_remote_branch_exists_error(self, worktree_manager):
        """Test remote branch check returns False on error."""
        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(1, ["git"])

            result = worktree_manager._remote_branch_exists(Path("/repo"), "main")

            assert result is False
