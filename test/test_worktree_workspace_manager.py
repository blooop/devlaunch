"""Tests for worktree workspace manager."""
# pylint: disable=redefined-outer-name,unused-argument,unused-variable

import tempfile
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from devlaunch.worktree.models import WorktreeInfo
from devlaunch.worktree.workspace_manager import WorkspaceManager, run_devpod


@pytest.fixture
def mock_worktree_manager():
    """Create a mock worktree manager."""
    manager = MagicMock()
    manager.repo_manager.get_repo_path.return_value = Path("/tmp/repos/owner/repo")
    return manager


@pytest.fixture
def mock_storage():
    """Create a mock storage."""
    return MagicMock()


@pytest.fixture
def workspace_manager(mock_worktree_manager, mock_storage):
    """Create a workspace manager with mocks."""
    return WorkspaceManager(mock_worktree_manager, mock_storage)


class TestRunDevpod:
    """Tests for run_devpod function."""

    @patch("devlaunch.worktree.workspace_manager.subprocess.run")
    def test_run_devpod_without_capture(self, mock_run):
        """Test running devpod without capturing output."""
        mock_run.return_value = MagicMock(returncode=0)

        result = run_devpod(["up", "workspace"], capture=False)

        mock_run.assert_called_once_with(["devpod", "up", "workspace"], check=False)
        assert result.returncode == 0

    @patch("devlaunch.worktree.workspace_manager.subprocess.run")
    def test_run_devpod_with_capture(self, mock_run):
        """Test running devpod with captured output."""
        mock_run.return_value = MagicMock(returncode=0, stdout="output", stderr="")

        result = run_devpod(["list", "--output", "json"], capture=True)

        mock_run.assert_called_once_with(
            ["devpod", "list", "--output", "json"],
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0


class TestWorkspaceManagerCreate:
    """Tests for workspace creation."""

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    @patch("devlaunch.worktree.workspace_manager.fcntl.flock")
    def test_create_workspace_uses_lock(
        self, mock_flock, mock_run_devpod, workspace_manager, mock_worktree_manager
    ):
        """Test that create_workspace acquires a lock."""
        worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="main",
            local_path=Path("/tmp/repos/owner/repo/.worktrees/main"),
            workspace_id="main",
        )
        mock_worktree_manager.ensure_worktree.return_value = worktree
        mock_run_devpod.return_value = MagicMock(returncode=0)

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch.object(Path, "home", return_value=Path(tmpdir)):
                workspace_manager.create_workspace("owner", "repo", "main")

        # Check that flock was called (lock acquired and released)
        assert mock_flock.call_count >= 2  # LOCK_EX and LOCK_UN

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    @patch("devlaunch.worktree.workspace_manager.fcntl.flock")
    def test_create_workspace_mounts_base_repo(
        self, mock_flock, mock_run_devpod, workspace_manager, mock_worktree_manager
    ):
        """Test that create_workspace mounts the base repo directory."""
        worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="feature",
            local_path=Path("/tmp/repos/owner/repo/.worktrees/feature"),
            workspace_id="feature",
        )
        mock_worktree_manager.ensure_worktree.return_value = worktree
        mock_run_devpod.return_value = MagicMock(returncode=0)

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch.object(Path, "home", return_value=Path(tmpdir)):
                workspace_manager.create_workspace("owner", "repo", "feature")

        # Check that devpod was called with the base repo path
        call_args = mock_run_devpod.call_args[0][0]
        assert "/tmp/repos/owner/repo" in call_args
        assert "--id" in call_args
        assert "feature" in call_args
        assert "--workdir" in call_args

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    @patch("devlaunch.worktree.workspace_manager.fcntl.flock")
    def test_create_workspace_uses_branch_as_id(
        self, mock_flock, mock_run_devpod, workspace_manager, mock_worktree_manager
    ):
        """Test that workspace ID is derived from branch name."""
        worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="feature/my-feature",
            local_path=Path("/tmp/repos/owner/repo/.worktrees/feature-my-feature"),
            workspace_id="feature-my-feature",
        )
        mock_worktree_manager.ensure_worktree.return_value = worktree
        mock_run_devpod.return_value = MagicMock(returncode=0)

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch.object(Path, "home", return_value=Path(tmpdir)):
                result, _ = workspace_manager.create_workspace(
                    "owner", "repo", "feature/my-feature"
                )

        # Check workspace ID is sanitized branch name
        call_args = mock_run_devpod.call_args[0][0]
        assert "--id" in call_args
        id_index = call_args.index("--id")
        assert call_args[id_index + 1] == "feature-my-feature"

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    @patch("devlaunch.worktree.workspace_manager.fcntl.flock")
    def test_create_workspace_with_ide(
        self, mock_flock, mock_run_devpod, workspace_manager, mock_worktree_manager
    ):
        """Test creating workspace with IDE."""
        worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="main",
            local_path=Path("/tmp/repos/owner/repo/.worktrees/main"),
            workspace_id="main",
        )
        mock_worktree_manager.ensure_worktree.return_value = worktree
        mock_run_devpod.return_value = MagicMock(returncode=0)

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch.object(Path, "home", return_value=Path(tmpdir)):
                workspace_manager.create_workspace("owner", "repo", "main", ide="vscode")

        call_args = mock_run_devpod.call_args[0][0]
        assert "--ide" in call_args
        assert "vscode" in call_args

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    @patch("devlaunch.worktree.workspace_manager.fcntl.flock")
    def test_create_workspace_failure(
        self, mock_flock, mock_run_devpod, workspace_manager, mock_worktree_manager
    ):
        """Test that creation failure raises error."""
        worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="main",
            local_path=Path("/tmp/repos/owner/repo/.worktrees/main"),
            workspace_id="main",
        )
        mock_worktree_manager.ensure_worktree.return_value = worktree
        mock_run_devpod.return_value = MagicMock(returncode=1)

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch.object(Path, "home", return_value=Path(tmpdir)):
                with pytest.raises(RuntimeError, match="DevPod failed"):
                    workspace_manager.create_workspace("owner", "repo", "main")


class TestWorkspaceManagerOperations:
    """Tests for workspace operations."""

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    def test_start_workspace(self, mock_run_devpod, workspace_manager):
        """Test starting a workspace."""
        mock_run_devpod.return_value = MagicMock(returncode=0)

        workspace_manager.start_workspace("my-workspace")

        mock_run_devpod.assert_called_once_with(["up", "my-workspace"], capture=False)

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    def test_stop_workspace(self, mock_run_devpod, workspace_manager):
        """Test stopping a workspace."""
        mock_run_devpod.return_value = MagicMock(returncode=0, stdout="stopped")

        result = workspace_manager.stop_workspace("my-workspace")

        mock_run_devpod.assert_called_once_with(["stop", "my-workspace"], capture=True)
        assert result == "stopped"

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    def test_delete_workspace(self, mock_run_devpod, workspace_manager):
        """Test deleting a workspace."""
        mock_run_devpod.return_value = MagicMock(returncode=0, stdout="deleted")

        result = workspace_manager.delete_workspace("my-workspace")

        mock_run_devpod.assert_called_once_with(["delete", "my-workspace"], capture=True)
        assert result == "deleted"

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    def test_delete_workspace_with_worktree_removal(
        self, mock_run_devpod, workspace_manager, mock_storage, mock_worktree_manager
    ):
        """Test deleting a workspace and its worktree."""
        mock_run_devpod.return_value = MagicMock(returncode=0, stdout="deleted")

        worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="feature",
            local_path=Path("/tmp/worktrees/feature"),
            workspace_id="feature",
            devpod_workspace_id="feature",
        )
        mock_storage.list_worktrees.return_value = [worktree]

        workspace_manager.delete_workspace("feature", remove_worktree=True)

        mock_worktree_manager.remove_worktree.assert_called_once_with("owner", "repo", "feature")


class TestWorkspaceManagerList:
    """Tests for workspace listing."""

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    def test_list_workspaces_empty(self, mock_run_devpod, workspace_manager, mock_storage):
        """Test listing workspaces when none exist."""
        mock_run_devpod.return_value = MagicMock(returncode=0, stdout="[]")
        mock_storage.list_worktrees.return_value = []

        result = workspace_manager.list_workspaces()

        assert result == []

    @patch("devlaunch.worktree.workspace_manager.run_devpod")
    def test_list_workspaces_with_worktree_info(
        self, mock_run_devpod, workspace_manager, mock_storage
    ):
        """Test listing workspaces enhances with worktree info."""
        import json
        from datetime import datetime

        mock_run_devpod.return_value = MagicMock(
            returncode=0,
            stdout=json.dumps([{"id": "main", "status": "Running"}]),
        )

        worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="main",
            local_path=Path("/tmp/worktrees/main"),
            workspace_id="main",
            created_at=datetime(2024, 1, 1),
            last_used=datetime(2024, 1, 2),
        )
        mock_storage.list_worktrees.return_value = [worktree]

        result = workspace_manager.list_workspaces()

        assert len(result) == 1
        assert result[0]["backend"] == "worktree"
        assert result[0]["worktree"]["branch"] == "main"
        assert result[0]["worktree"]["owner"] == "owner"
