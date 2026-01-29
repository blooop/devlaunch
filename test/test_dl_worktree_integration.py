"""Integration tests for dl.py with worktree backend."""
# pylint: disable=redefined-outer-name,unused-argument,unused-variable

import sys
from pathlib import Path
from unittest.mock import Mock, patch

import pytest

from devlaunch.dl import main, should_use_worktree_backend, workspace_up_worktree
from devlaunch.worktree.models import WorktreeInfo


@pytest.fixture
def mock_workspace_manager():
    """Create a mock workspace manager."""
    mock = Mock()
    mock.create_workspace.return_value = (
        WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="main",
            local_path=Path("/worktrees/main"),
            workspace_id="main-ws",
            devpod_workspace_id="main",
        ),
        "",
    )
    return mock


@pytest.fixture
def mock_managers(mock_workspace_manager):
    """Create mock managers tuple."""
    mock_repo_manager = Mock()
    mock_worktree_manager = Mock()
    mock_storage = Mock()
    mock_config = Mock()
    mock_config.prune_after_days = 30
    return (
        mock_repo_manager,
        mock_worktree_manager,
        mock_workspace_manager,
        mock_storage,
        mock_config,
    )


class TestShouldUseWorktreeBackend:
    """Test the should_use_worktree_backend function."""

    def test_git_repo_format_returns_true(self):
        """Test that owner/repo format returns True."""
        assert should_use_worktree_backend("owner/repo") is True
        assert should_use_worktree_backend("owner/repo@main") is True

    def test_local_path_returns_false(self):
        """Test that local paths return False."""
        assert should_use_worktree_backend("./local/path") is False
        assert should_use_worktree_backend("/absolute/path") is False

    def test_backend_override_worktree(self):
        """Test backend override to worktree."""
        assert should_use_worktree_backend("./local/path", "worktree") is True
        assert should_use_worktree_backend("owner/repo", "worktree") is True

    def test_backend_override_devpod(self):
        """Test backend override to devpod."""
        assert should_use_worktree_backend("owner/repo", "devpod") is False
        assert should_use_worktree_backend("./local/path", "devpod") is False

    def test_url_format_returns_true(self):
        """Test URL formats return True."""
        assert should_use_worktree_backend("github.com/owner/repo") is True
        assert should_use_worktree_backend("https://github.com/owner/repo.git") is True


class TestWorkspaceUpWorktree:
    """Test the workspace_up_worktree function."""

    @patch("devlaunch.dl.get_worktree_managers")
    def test_creates_workspace(self, mock_get_managers, mock_managers):
        """Test that workspace_up_worktree creates a workspace."""
        mock_get_managers.return_value = mock_managers
        _, _, mock_ws_manager, _, _ = mock_managers

        result = workspace_up_worktree("owner", "repo", "main")

        assert result.returncode == 0
        mock_ws_manager.create_workspace.assert_called_once()
        call_kwargs = mock_ws_manager.create_workspace.call_args.kwargs
        assert call_kwargs["owner"] == "owner"
        assert call_kwargs["repo"] == "repo"
        assert call_kwargs["branch"] == "main"

    @patch("devlaunch.dl.get_worktree_managers")
    def test_with_custom_workspace_id(self, mock_get_managers, mock_managers):
        """Test workspace creation with custom ID."""
        mock_get_managers.return_value = mock_managers
        _, _, mock_ws_manager, _, _ = mock_managers

        workspace_up_worktree("owner", "repo", "main", workspace_id="custom-id")

        call_kwargs = mock_ws_manager.create_workspace.call_args.kwargs
        assert call_kwargs["workspace_id"] == "custom-id"

    @patch("devlaunch.dl.get_worktree_managers")
    def test_with_ide(self, mock_get_managers, mock_managers):
        """Test workspace creation with IDE specification."""
        mock_get_managers.return_value = mock_managers
        _, _, mock_ws_manager, _, _ = mock_managers

        workspace_up_worktree("owner", "repo", "main", ide="vscode")

        call_kwargs = mock_ws_manager.create_workspace.call_args.kwargs
        assert call_kwargs["ide"] == "vscode"


class TestMainWithWorktreeBackend:
    """Test main() function with worktree backend."""

    @patch("devlaunch.dl.get_worktree_managers")
    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_uses_worktree_backend(
        self, mock_cache, mock_ssh, mock_use_worktree, mock_get_managers, mock_managers
    ):
        """Test main uses worktree backend when should_use_worktree_backend returns True."""
        mock_use_worktree.return_value = True
        mock_get_managers.return_value = mock_managers
        mock_ssh.return_value = 0

        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()

        assert result == 0
        mock_use_worktree.assert_called()

    @patch("devlaunch.dl.get_worktree_managers")
    @patch("devlaunch.dl.should_use_worktree_backend")
    def test_main_worktree_backend_failure(
        self, mock_use_worktree, mock_get_managers, mock_managers
    ):
        """Test main handles worktree backend failures."""
        mock_use_worktree.return_value = True
        _, _, mock_ws_manager, _, _ = mock_managers
        mock_ws_manager.create_workspace.side_effect = RuntimeError("Clone failed")
        mock_get_managers.return_value = mock_managers

        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()

        assert result == 1


class TestWorktreeBackendEdgeCases:
    """Test edge cases for worktree backend."""

    def test_should_use_worktree_with_branch_at_symbol(self):
        """Test parsing spec with @ symbol for branch."""
        assert should_use_worktree_backend("owner/repo@feature/test") is True
        assert should_use_worktree_backend("owner/repo@v1.0.0") is True

    def test_should_use_worktree_with_special_characters(self):
        """Test parsing spec with special characters."""
        assert should_use_worktree_backend("owner-name/repo-name@branch-name") is True
        assert should_use_worktree_backend("owner_name/repo_name") is True
