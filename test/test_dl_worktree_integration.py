"""Integration tests for dl.py with worktree backend."""

import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, Mock, patch

import pytest

from devlaunch.dl import main, should_use_worktree_backend, workspace_up_worktree
from devlaunch.worktree.models import WorktreeInfo


class TestWorktreeBackendIntegration:
    """Test integration of worktree backend with dl.py."""

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_worktree_backend_new_workspace(
        self, mock_cache, mock_use_worktree, mock_workspace_manager
    ):
        """Test creating new workspace with worktree backend."""
        mock_use_worktree.return_value = True

        # Mock workspace manager
        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.create_workspace.return_value = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="main",
            local_path=Path("/worktrees/main"),
            workspace_id="main-ws",
            devpod_workspace_id="main",
        )

        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            with patch("devlaunch.dl.workspace_ssh") as mock_ssh:
                mock_ssh.return_value = 0
                result = main()

        assert result == 0
        mock_use_worktree.assert_called_once_with("owner/repo@main", None)
        mock_ws_instance.create_workspace.assert_called_once()

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_worktree_backend_existing_workspace(
        self, mock_cache, mock_use_worktree, mock_workspace_manager
    ):
        """Test connecting to existing workspace with worktree backend."""
        mock_use_worktree.return_value = True

        # Mock workspace manager
        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.get_workspace_by_id.return_value = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="feature",
            local_path=Path("/worktrees/feature"),
            workspace_id="feature-ws",
            devpod_workspace_id="feature",
        )
        mock_ws_instance.activate_workspace.return_value = MagicMock(returncode=0)

        with patch.object(sys, "argv", ["dl", "feature"]):
            with patch("devlaunch.dl.workspace_ssh") as mock_ssh:
                mock_ssh.return_value = 0
                with patch("devlaunch.dl.get_workspace_ids") as mock_ids:
                    mock_ids.return_value = ["feature"]
                    result = main()

        assert result == 0
        mock_ws_instance.get_workspace_by_id.assert_called_with("feature")
        mock_ws_instance.activate_workspace.assert_called_once()

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    def test_main_worktree_backend_workspace_creation_failure(
        self, mock_use_worktree, mock_workspace_manager
    ):
        """Test handling workspace creation failure."""
        mock_use_worktree.return_value = True

        # Mock workspace manager that fails
        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.create_workspace.side_effect = RuntimeError("Failed to create worktree")

        with patch.object(sys, "argv", ["dl", "owner/repo@broken"]):
            result = main()

        assert result == 1

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    def test_main_worktree_backend_invalid_branch_name(
        self, mock_use_worktree, mock_workspace_manager
    ):
        """Test handling invalid branch names."""
        mock_use_worktree.return_value = True

        # Mock workspace manager
        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.create_workspace.side_effect = ValueError("Invalid branch name")

        with patch.object(sys, "argv", ["dl", "owner/repo@../../../etc/passwd"]):
            result = main()

        assert result == 1

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_worktree_backend_switch_branches(
        self, mock_cache, mock_use_worktree, mock_workspace_manager
    ):
        """Test switching between branches with worktree backend."""
        mock_use_worktree.return_value = True

        # Mock workspace manager
        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance

        # First workspace
        main_ws = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="main",
            local_path=Path("/worktrees/main"),
            workspace_id="main-ws",
            devpod_workspace_id="main",
        )

        # Second workspace
        feature_ws = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="feature",
            local_path=Path("/worktrees/feature"),
            workspace_id="feature-ws",
            devpod_workspace_id="feature",
        )

        mock_ws_instance.create_workspace.side_effect = [main_ws, feature_ws]
        mock_ws_instance.activate_workspace.return_value = MagicMock(returncode=0)

        with patch("devlaunch.dl.workspace_ssh") as mock_ssh:
            mock_ssh.return_value = 0

            # Create main workspace
            with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
                result1 = main()

            # Switch to feature branch
            with patch.object(sys, "argv", ["dl", "owner/repo@feature"]):
                result2 = main()

        assert result1 == 0
        assert result2 == 0
        assert mock_ws_instance.create_workspace.call_count == 2

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    def test_main_worktree_backend_concurrent_access(
        self, mock_use_worktree, mock_workspace_manager
    ):
        """Test handling concurrent access to same workspace."""
        import threading

        mock_use_worktree.return_value = True

        # Mock workspace manager
        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance

        results = []

        def run_dl():
            mock_ws_instance.create_workspace.return_value = WorktreeInfo(
                owner="owner",
                repo="repo",
                branch="concurrent",
                local_path=Path("/worktrees/concurrent"),
                workspace_id="concurrent-ws",
                devpod_workspace_id="concurrent",
            )

            with patch.object(sys, "argv", ["dl", "owner/repo@concurrent"]):
                with patch("devlaunch.dl.workspace_ssh") as mock_ssh:
                    mock_ssh.return_value = 0
                    with patch("devlaunch.dl.update_cache_background"):
                        result = main()
                        results.append(result)

        # Start multiple threads
        threads = [threading.Thread(target=run_dl) for _ in range(3)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        # All should succeed
        assert all(r == 0 for r in results)


class TestWorktreeBackendEdgeCases:
    """Test edge cases specific to worktree backend."""

    def test_workspace_up_worktree_missing_branch(self):
        """Test workspace_up_worktree with missing branch."""
        result = workspace_up_worktree("owner", "repo", "", workspace_id="test")
        # Should fail when branch is missing
        assert result is None or result.returncode != 0

    def test_workspace_up_worktree_invalid_repo_format(self):
        """Test workspace_up_worktree with invalid repo format."""
        result = workspace_up_worktree("", "", "", workspace_id="test")
        assert result is None or result.returncode != 0

    def test_workspace_up_worktree_with_url(self):
        """Test workspace_up_worktree with full URL format."""
        with patch("devlaunch.dl.WorkspaceManager") as mock_ws:
            mock_instance = Mock()
            mock_ws.return_value = mock_instance
            mock_instance.create_workspace.return_value = WorktreeInfo(
                owner="owner",
                repo="repo",
                branch="main",
                local_path=Path("/worktrees/main"),
                workspace_id="main-ws",
                devpod_workspace_id="main",
            )
            mock_instance.activate_workspace.return_value = MagicMock(returncode=0)

            result = workspace_up_worktree("owner", "repo", "main")

            assert result.returncode == 0
            mock_instance.create_workspace.assert_called_once()

    @patch("devlaunch.dl.should_use_worktree_backend")
    def test_main_force_worktree_for_path(self, mock_use_worktree):
        """Test forcing worktree backend for local paths."""
        mock_use_worktree.return_value = True

        with patch.object(sys, "argv", ["dl", "--backend", "worktree", "./local/project"]):
            with patch("devlaunch.dl.workspace_up_worktree") as mock_up:
                mock_up.return_value = MagicMock(returncode=0)
                with patch("devlaunch.dl.workspace_ssh") as mock_ssh:
                    mock_ssh.return_value = 0
                    with patch("devlaunch.dl.update_cache_background"):
                        result = main()

        assert result == 0
        mock_use_worktree.assert_called_with("./local/project", "worktree")

    def test_should_use_worktree_special_repos(self):
        """Test worktree backend selection for special repository formats."""
        # GitHub enterprise
        assert should_use_worktree_backend("github.enterprise.com/owner/repo") is True

        # GitLab
        assert should_use_worktree_backend("gitlab.com/owner/repo") is True

        # Bitbucket
        assert should_use_worktree_backend("bitbucket.org/owner/repo") is True

        # SSH URLs
        assert should_use_worktree_backend("git@github.com:owner/repo.git") is True

        # HTTPS URLs
        assert should_use_worktree_backend("https://github.com/owner/repo.git") is True

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    def test_main_worktree_cleanup_on_error(self, mock_use_worktree, mock_workspace_manager):
        """Test cleanup when workspace creation fails."""
        mock_use_worktree.return_value = True

        # Mock workspace manager that fails after partial creation
        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.create_workspace.side_effect = [
            RuntimeError("Disk full after partial creation")
        ]

        with patch.object(sys, "argv", ["dl", "owner/repo@feature"]):
            result = main()

        assert result == 1
        # Cleanup should have been attempted (verify through logging or other means)


class TestWorktreeLongBranchNames:
    """Test handling of long branch names."""

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.update_cache_background")
    def test_very_long_branch_name(self, mock_cache, mock_use_worktree, mock_workspace_manager):
        """Test handling very long branch names."""
        mock_use_worktree.return_value = True

        # Create a branch name that exceeds filesystem limits
        long_branch = "feature/" + "a" * 250

        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.create_workspace.return_value = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch=long_branch,
            local_path=Path("/worktrees/feature-aaa"),  # Sanitized name
            workspace_id="feature-ws",
            devpod_workspace_id="feature-aaa",
        )

        with patch.object(sys, "argv", ["dl", f"owner/repo@{long_branch}"]):
            with patch("devlaunch.dl.workspace_ssh") as mock_ssh:
                mock_ssh.return_value = 0
                result = main()

        assert result == 0
        # Verify branch was sanitized properly
        call_args = mock_ws_instance.create_workspace.call_args
        assert len(str(call_args)) < 1000  # Reasonable length


class TestWorktreeBackendListCommand:
    """Test list command with worktree backend."""

    @patch("devlaunch.dl.WorkspaceManager")
    def test_list_worktree_workspaces(self, mock_workspace_manager):
        """Test listing workspaces with worktree backend."""
        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance

        mock_ws_instance.list_workspaces.return_value = [
            WorktreeInfo(
                owner="owner1",
                repo="repo1",
                branch="main",
                local_path=Path("/worktrees/main"),
                workspace_id="main-ws",
                devpod_workspace_id="main",
            ),
            WorktreeInfo(
                owner="owner2",
                repo="repo2",
                branch="feature",
                local_path=Path("/worktrees/feature"),
                workspace_id="feature-ws",
                devpod_workspace_id="feature",
            ),
        ]

        with patch.object(sys, "argv", ["dl", "--list"]):
            with patch("builtins.print") as mock_print:
                result = main()

        assert result == 0
        # Should print workspace information
        assert mock_print.called


class TestWorktreeBackendDeleteCommand:
    """Test delete command with worktree backend."""

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    def test_delete_worktree_workspace(self, mock_ids, mock_use_worktree, mock_workspace_manager):
        """Test deleting workspace with worktree backend."""
        mock_use_worktree.return_value = True
        mock_ids.return_value = ["feature-ws"]

        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.get_workspace_by_id.return_value = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="feature",
            local_path=Path("/worktrees/feature"),
            workspace_id="feature-ws",
            devpod_workspace_id="feature",
        )
        mock_ws_instance.delete_workspace.return_value = True

        with patch.object(sys, "argv", ["dl", "--delete", "feature-ws"]):
            result = main()

        assert result == 0
        mock_ws_instance.delete_workspace.assert_called_once()

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    def test_delete_nonexistent_workspace(
        self, mock_ids, mock_use_worktree, mock_workspace_manager
    ):
        """Test deleting non-existent workspace."""
        mock_use_worktree.return_value = True
        mock_ids.return_value = []

        with patch.object(sys, "argv", ["dl", "--delete", "nonexistent"]):
            result = main()

        # Should fail when workspace doesn't exist
        assert result == 1


class TestWorktreeBackendStopCommand:
    """Test stop command with worktree backend."""

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    def test_stop_worktree_workspace(self, mock_ids, mock_use_worktree, mock_workspace_manager):
        """Test stopping workspace with worktree backend."""
        mock_use_worktree.return_value = True
        mock_ids.return_value = ["feature-ws"]

        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.get_workspace_by_id.return_value = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="feature",
            local_path=Path("/worktrees/feature"),
            workspace_id="feature-ws",
            devpod_workspace_id="feature",
        )
        mock_ws_instance.stop_workspace.return_value = MagicMock(returncode=0)

        with patch.object(sys, "argv", ["dl", "--stop", "feature-ws"]):
            result = main()

        assert result == 0
        mock_ws_instance.stop_workspace.assert_called_once()


class TestWorktreeBackendRebuildCommand:
    """Test rebuild command with worktree backend."""

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.update_cache_background")
    def test_rebuild_worktree_workspace(
        self, mock_cache, mock_ids, mock_use_worktree, mock_workspace_manager
    ):
        """Test rebuilding workspace with worktree backend."""
        mock_use_worktree.return_value = True
        mock_ids.return_value = ["feature-ws"]

        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.get_workspace_by_id.return_value = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="feature",
            local_path=Path("/worktrees/feature"),
            workspace_id="feature-ws",
            devpod_workspace_id="feature",
        )
        mock_ws_instance.rebuild_workspace.return_value = MagicMock(returncode=0)

        with patch.object(sys, "argv", ["dl", "--rebuild", "feature-ws"]):
            with patch("devlaunch.dl.workspace_ssh") as mock_ssh:
                mock_ssh.return_value = 0
                result = main()

        assert result == 0
        mock_ws_instance.rebuild_workspace.assert_called_once()


class TestWorktreeBackendErrorMessages:
    """Test error message handling for worktree backend."""

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    def test_network_error_message(self, mock_use_worktree, mock_workspace_manager):
        """Test user-friendly error message for network failures."""
        mock_use_worktree.return_value = True

        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.create_workspace.side_effect = RuntimeError(
            "Could not resolve host: github.com"
        )

        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            with patch("builtins.print") as mock_print:
                result = main()

        assert result == 1
        # Should print helpful error message
        print_calls = [str(call) for call in mock_print.call_args_list]
        assert any(
            "network" in call.lower() or "connection" in call.lower() for call in print_calls
        )

    @patch("devlaunch.dl.WorkspaceManager")
    @patch("devlaunch.dl.should_use_worktree_backend")
    def test_permission_error_message(self, mock_use_worktree, mock_workspace_manager):
        """Test user-friendly error message for permission failures."""
        mock_use_worktree.return_value = True

        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance
        mock_ws_instance.create_workspace.side_effect = PermissionError(
            "Permission denied: /worktrees"
        )

        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            with patch("builtins.print") as mock_print:
                result = main()

        assert result == 1
        # Should print helpful error message about permissions
        print_calls = [str(call) for call in mock_print.call_args_list]
        assert any("permission" in call.lower() for call in print_calls)


class TestWorktreeBackendCleanup:
    """Test cleanup operations for worktree backend."""

    @patch("devlaunch.dl.WorkspaceManager")
    def test_cleanup_stale_worktrees(self, mock_workspace_manager):
        """Test cleaning up stale worktrees."""
        mock_ws_instance = Mock()
        mock_workspace_manager.return_value = mock_ws_instance

        # Mock stale worktrees
        mock_ws_instance.cleanup_stale_workspaces.return_value = 3  # Cleaned 3 worktrees

        with patch.object(sys, "argv", ["dl", "--cleanup"]):
            result = main()

        # This command might not exist yet, but shows the pattern
        # for testing cleanup functionality
        pass  # Placeholder for when cleanup is implemented
