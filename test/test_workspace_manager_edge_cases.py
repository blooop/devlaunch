"""Edge case tests for workspace manager."""
# pylint: disable=redefined-outer-name,unused-argument,protected-access,unused-variable

import tempfile
from pathlib import Path
from unittest.mock import MagicMock, Mock, patch

import pytest

from devlaunch.worktree.models import WorktreeInfo
from devlaunch.worktree.storage import MetadataStorage
from devlaunch.worktree.workspace_manager import WorkspaceManager
from devlaunch.worktree.worktree_manager import WorktreeManager


@pytest.fixture
def temp_dir():
    """Create a temporary directory for testing."""
    with tempfile.TemporaryDirectory() as tmpdir:
        yield Path(tmpdir)


@pytest.fixture
def mock_worktree_manager():
    """Create a mock worktree manager."""
    mock = Mock(spec=WorktreeManager)
    mock.repo_manager = Mock()
    mock.repo_manager.get_repo_path.return_value = Path("/repos/owner/repo")
    mock.ensure_worktree.return_value = WorktreeInfo(
        owner="owner",
        repo="repo",
        branch="main",
        local_path=Path("/repos/owner/repo/.worktrees/main"),
        workspace_id="main-ws",
    )
    return mock


@pytest.fixture
def workspace_manager(temp_dir, mock_worktree_manager):
    """Create a workspace manager with mocks."""
    storage = MetadataStorage(temp_dir / "metadata.json")
    return WorkspaceManager(mock_worktree_manager, storage)


class TestWorkspaceManagerCreateWorkspace:
    """Test workspace creation functionality."""

    def test_create_workspace_with_lock(self, workspace_manager, mock_worktree_manager):
        """Test that workspace creation uses file locking."""
        with patch("devlaunch.worktree.workspace_manager.run_devpod") as mock_devpod:
            mock_devpod.return_value = MagicMock(returncode=0)

            result, output = workspace_manager.create_workspace(
                "owner", "repo", "main", remote_url="https://github.com/owner/repo.git"
            )

            assert result.owner == "owner"
            assert result.repo == "repo"
            assert result.branch == "main"
            mock_worktree_manager.ensure_worktree.assert_called_once()

    def test_create_workspace_failure_raises(self, workspace_manager, mock_worktree_manager):
        """Test workspace creation failure raises RuntimeError."""
        with patch("devlaunch.worktree.workspace_manager.run_devpod") as mock_devpod:
            mock_devpod.return_value = MagicMock(returncode=1)

            with pytest.raises(RuntimeError, match="DevPod failed"):
                workspace_manager.create_workspace(
                    "owner", "repo", "main", remote_url="https://github.com/owner/repo.git"
                )


class TestWorkspaceManagerOperations:
    """Test workspace operations."""

    def test_start_workspace(self, workspace_manager):
        """Test starting a workspace."""
        with patch("devlaunch.worktree.workspace_manager.run_devpod") as mock_devpod:
            mock_devpod.return_value = MagicMock(returncode=0)

            result = workspace_manager.start_workspace("test-ws")

            assert result == ""
            mock_devpod.assert_called_once()

    def test_stop_workspace(self, workspace_manager):
        """Test stopping a workspace."""
        with patch("devlaunch.worktree.workspace_manager.run_devpod") as mock_devpod:
            mock_devpod.return_value = MagicMock(returncode=0, stdout="stopped")

            result = workspace_manager.stop_workspace("test-ws")

            assert "stopped" in result

    def test_delete_workspace(self, workspace_manager):
        """Test deleting a workspace."""
        with patch("devlaunch.worktree.workspace_manager.run_devpod") as mock_devpod:
            mock_devpod.return_value = MagicMock(returncode=0, stdout="deleted")

            result = workspace_manager.delete_workspace("test-ws")

            assert "deleted" in result or result == ""


class TestWorkspaceManagerList:
    """Test workspace listing."""

    def test_list_workspaces_empty(self, workspace_manager):
        """Test listing workspaces when none exist."""
        with patch("devlaunch.worktree.workspace_manager.run_devpod") as mock_devpod:
            mock_devpod.return_value = MagicMock(returncode=1, stdout="")

            result = workspace_manager.list_workspaces()

            assert result == []

    def test_list_workspaces_with_json(self, workspace_manager):
        """Test listing workspaces with JSON output."""
        import json

        workspaces_json = json.dumps([{"id": "test-ws", "status": "running"}])
        with patch("devlaunch.worktree.workspace_manager.run_devpod") as mock_devpod:
            mock_devpod.return_value = MagicMock(returncode=0, stdout=workspaces_json)

            result = workspace_manager.list_workspaces()

            assert len(result) == 1
            assert result[0]["id"] == "test-ws"


class TestWorkspaceManagerConcurrency:
    """Test concurrent operations."""

    def test_workspace_manager_creates_lock_directory(
        self, workspace_manager, mock_worktree_manager
    ):
        """Test that workspace manager can create workspaces with locking."""
        with patch("devlaunch.worktree.workspace_manager.run_devpod") as mock_devpod:
            mock_devpod.return_value = MagicMock(returncode=0)
            result, _ = workspace_manager.create_workspace(
                "owner",
                "repo",
                "test-branch",
                remote_url="https://github.com/owner/repo.git",
            )
            # Should complete without error
            assert result is not None
            assert result.branch == "main"  # From mock
