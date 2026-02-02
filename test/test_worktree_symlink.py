"""Tests for worktree symlink functionality.

The worktree backend creates a ~/work symlink inside containers to provide
shorter terminal prompts instead of the long worktree path.
"""

from unittest.mock import patch, MagicMock

from devlaunch.dl import (
    get_worktree_symlink_path,
    setup_worktree_symlink,
    get_worktree_container_path,
)


class TestGetWorktreeSymlinkPath:
    """Tests for get_worktree_symlink_path()."""

    def test_returns_home_vscode_work(self):
        """Symlink path should always be /home/vscode/work."""
        assert get_worktree_symlink_path() == "/home/vscode/work"

    def test_returns_consistent_value(self):
        """Multiple calls should return the same path."""
        path1 = get_worktree_symlink_path()
        path2 = get_worktree_symlink_path()
        assert path1 == path2

    def test_returns_absolute_path(self):
        """Symlink path should be absolute."""
        path = get_worktree_symlink_path()
        assert path.startswith("/")


class TestSetupWorktreeSymlink:
    """Tests for setup_worktree_symlink()."""

    @patch("devlaunch.dl.workspace_ssh")
    def test_creates_symlink_with_correct_command(self, mock_ssh):
        """Should create symlink using ln -sfn."""
        mock_ssh.return_value = 0
        workspace_id = "blooop-bencher-main"
        worktree_path = "/workspaces/blooop-bencher-main/.worktrees/main"

        result = setup_worktree_symlink(workspace_id, worktree_path)

        assert result is True
        mock_ssh.assert_called_once()
        call_args = mock_ssh.call_args
        assert call_args[0][0] == workspace_id
        command = call_args[1]["command"]
        assert "ln -sfn" in command
        assert worktree_path in command
        assert "/home/vscode/work" in command

    @patch("devlaunch.dl.workspace_ssh")
    def test_returns_true_on_success(self, mock_ssh):
        """Should return True when symlink creation succeeds."""
        mock_ssh.return_value = 0

        result = setup_worktree_symlink(
            "test-workspace", "/workspaces/test-workspace/.worktrees/main"
        )

        assert result is True

    @patch("devlaunch.dl.workspace_ssh")
    def test_returns_false_on_failure(self, mock_ssh):
        """Should return False when symlink creation fails."""
        mock_ssh.return_value = 1

        result = setup_worktree_symlink(
            "test-workspace", "/workspaces/test-workspace/.worktrees/main"
        )

        assert result is False

    @patch("devlaunch.dl.workspace_ssh")
    def test_returns_false_on_non_zero_exit(self, mock_ssh):
        """Should return False for any non-zero exit code."""
        for exit_code in [1, 2, 127, 255]:
            mock_ssh.return_value = exit_code

            result = setup_worktree_symlink(
                "test-workspace", "/workspaces/test-workspace/.worktrees/main"
            )

            assert result is False

    @patch("devlaunch.dl.workspace_ssh")
    def test_uses_force_flag_to_overwrite_existing(self, mock_ssh):
        """The -f flag should allow overwriting existing symlinks."""
        mock_ssh.return_value = 0

        setup_worktree_symlink("test-workspace", "/workspaces/test-workspace/.worktrees/main")

        command = mock_ssh.call_args[1]["command"]
        assert "-f" in command or "-sfn" in command

    @patch("devlaunch.dl.workspace_ssh")
    def test_uses_no_dereference_flag(self, mock_ssh):
        """The -n flag prevents dereferencing existing symlinks."""
        mock_ssh.return_value = 0

        setup_worktree_symlink("test-workspace", "/workspaces/test-workspace/.worktrees/main")

        command = mock_ssh.call_args[1]["command"]
        assert "-n" in command or "-sfn" in command

    @patch("devlaunch.dl.workspace_ssh")
    def test_handles_branch_with_slashes(self, mock_ssh):
        """Should handle branches like feature/my-feature."""
        mock_ssh.return_value = 0
        # Note: branch names with slashes are sanitized to dashes
        worktree_path = "/workspaces/owner-repo-feature-branch/.worktrees/feature-branch"

        result = setup_worktree_symlink("owner-repo-feature-branch", worktree_path)

        assert result is True
        command = mock_ssh.call_args[1]["command"]
        assert worktree_path in command

    @patch("devlaunch.dl.workspace_ssh")
    def test_handles_special_characters_in_path(self, mock_ssh):
        """Should handle workspace IDs with various characters."""
        mock_ssh.return_value = 0
        workspace_id = "my-org-repo-name-v1-2-3"
        worktree_path = f"/workspaces/{workspace_id}/.worktrees/release-v1.2.3"

        result = setup_worktree_symlink(workspace_id, worktree_path)

        assert result is True


class TestWorktreeContainerPath:
    """Tests for get_worktree_container_path() to ensure compatibility."""

    def test_returns_worktree_path(self):
        """Should return the full worktree container path."""
        path = get_worktree_container_path("blooop-bencher-main", "main")
        assert path == "/workspaces/blooop-bencher-main/.worktrees/main"

    def test_sanitizes_branch_name(self):
        """Should sanitize branch names with slashes."""
        path = get_worktree_container_path("owner-repo-feature-x", "feature/x")
        assert path == "/workspaces/owner-repo-feature-x/.worktrees/feature-x"

    def test_handles_complex_branch_names(self):
        """Should handle complex branch names."""
        path = get_worktree_container_path("test-ws", "feature/ABC-123/sub-task")
        # Slashes become dashes
        assert ".worktrees/feature-abc-123-sub-task" in path


class TestSymlinkIntegrationWithMainFlow:
    """Integration tests for symlink setup in the main workspace flows."""

    @patch("devlaunch.dl.workspace_up_worktree")
    @patch("devlaunch.dl.setup_worktree_symlink")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.get_default_branch_for_repo")
    def test_default_flow_creates_symlink_before_ssh(
        self,
        mock_get_default_branch,
        mock_get_ids,
        mock_ssh,
        mock_setup_symlink,
        mock_up_worktree,
    ):
        """Default workspace flow should create symlink before SSH."""
        from devlaunch.dl import main
        import sys

        mock_get_ids.return_value = []
        mock_get_default_branch.return_value = "main"
        mock_up_worktree.return_value = MagicMock(returncode=0)
        mock_setup_symlink.return_value = True
        mock_ssh.return_value = 0

        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            main()

        # Verify symlink was set up
        mock_setup_symlink.assert_called_once()
        # Verify SSH uses symlink path
        ssh_call = mock_ssh.call_args
        assert ssh_call[1]["workdir"] == "/home/vscode/work"

    @patch("devlaunch.dl.workspace_up_worktree")
    @patch("devlaunch.dl.setup_worktree_symlink")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.workspace_stop")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.get_default_branch_for_repo")
    def test_restart_flow_creates_symlink_before_ssh(
        self,
        mock_get_default_branch,
        mock_get_ids,
        mock_stop,
        mock_ssh,
        mock_setup_symlink,
        mock_up_worktree,
    ):
        """Restart subcommand should create symlink before SSH."""
        from devlaunch.dl import main
        import sys

        mock_get_ids.return_value = []
        mock_get_default_branch.return_value = "main"
        mock_stop.return_value = 0
        mock_up_worktree.return_value = MagicMock(returncode=0)
        mock_setup_symlink.return_value = True
        mock_ssh.return_value = 0

        with patch.object(sys, "argv", ["dl", "owner/repo@main", "restart"]):
            main()

        # Verify symlink was set up
        mock_setup_symlink.assert_called_once()
        # Verify SSH uses symlink path
        ssh_call = mock_ssh.call_args
        assert ssh_call[1]["workdir"] == "/home/vscode/work"

    @patch("devlaunch.dl.workspace_up_worktree")
    @patch("devlaunch.dl.setup_worktree_symlink")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.get_default_branch_for_repo")
    def test_code_flow_creates_symlink_after_up(
        self,
        mock_get_default_branch,
        mock_get_ids,
        mock_setup_symlink,
        mock_up_worktree,
    ):
        """Code subcommand should create symlink after workspace_up_worktree."""
        from devlaunch.dl import main
        import sys

        mock_get_ids.return_value = []
        mock_get_default_branch.return_value = "main"
        mock_up_worktree.return_value = MagicMock(returncode=0)
        mock_setup_symlink.return_value = True

        with patch.object(sys, "argv", ["dl", "owner/repo@main", "code"]):
            result = main()

        # Verify symlink was set up
        mock_setup_symlink.assert_called_once()
        assert result == 0

    @patch("devlaunch.dl.workspace_up_worktree")
    @patch("devlaunch.dl.setup_worktree_symlink")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.get_default_branch_for_repo")
    def test_code_flow_skips_symlink_on_failure(
        self,
        mock_get_default_branch,
        mock_get_ids,
        mock_setup_symlink,
        mock_up_worktree,
    ):
        """Code subcommand should skip symlink if workspace_up fails."""
        from devlaunch.dl import main
        import sys

        mock_get_ids.return_value = []
        mock_get_default_branch.return_value = "main"
        mock_up_worktree.return_value = MagicMock(returncode=1)

        with patch.object(sys, "argv", ["dl", "owner/repo@main", "code"]):
            result = main()

        # Verify symlink was NOT set up because workspace_up failed
        mock_setup_symlink.assert_not_called()
        assert result == 1


class TestSymlinkEdgeCases:
    """Edge case tests for symlink functionality."""

    @patch("devlaunch.dl.workspace_ssh")
    def test_symlink_survives_container_restart(self, mock_ssh):
        """Symlink should be recreated on each attach (handles restarts)."""
        mock_ssh.return_value = 0

        # First attach
        result1 = setup_worktree_symlink("test-ws", "/workspaces/test-ws/.worktrees/main")
        # Second attach (after container restart)
        result2 = setup_worktree_symlink("test-ws", "/workspaces/test-ws/.worktrees/main")

        assert result1 is True
        assert result2 is True
        # Should be called twice (once per attach)
        assert mock_ssh.call_count == 2

    @patch("devlaunch.dl.workspace_ssh")
    def test_symlink_updates_for_different_branch(self, mock_ssh):
        """Symlink should be updated when switching branches in shared mode."""
        mock_ssh.return_value = 0

        # First branch
        setup_worktree_symlink("owner-repo", "/workspaces/owner-repo/.worktrees/main")
        # Different branch in same shared container
        setup_worktree_symlink("owner-repo", "/workspaces/owner-repo/.worktrees/feature")

        # Both calls should succeed - ln -sfn handles the update
        assert mock_ssh.call_count == 2
        # Verify different paths were used
        calls = mock_ssh.call_args_list
        assert ".worktrees/main" in calls[0][1]["command"]
        assert ".worktrees/feature" in calls[1][1]["command"]

    @patch("devlaunch.dl.workspace_ssh")
    def test_handles_long_workspace_ids(self, mock_ssh):
        """Should handle workspace IDs up to the max length."""
        mock_ssh.return_value = 0
        long_workspace_id = "very-long-organization-name-with-very-long-rep"
        worktree_path = f"/workspaces/{long_workspace_id}/.worktrees/main"

        result = setup_worktree_symlink(long_workspace_id, worktree_path)

        assert result is True

    @patch("devlaunch.dl.workspace_ssh")
    def test_handles_empty_workspace_id(self, mock_ssh):
        """Should handle edge case of empty workspace ID."""
        mock_ssh.return_value = 0

        # This is an edge case that shouldn't happen in practice
        result = setup_worktree_symlink("", "/workspaces//.worktrees/main")

        # Should still attempt the command
        mock_ssh.assert_called_once()


class TestSymlinkWithNonWorktreeBackend:
    """Tests to ensure symlink is not used with non-worktree backend."""

    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.setup_worktree_symlink")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_workspace_ids")
    def test_no_symlink_for_local_path(self, mock_get_ids, mock_ssh, mock_setup_symlink, mock_up):
        """Local paths should not use symlink (devpod backend)."""
        from devlaunch.dl import main
        import sys
        import tempfile
        import os

        # Create a temporary directory for the test
        with tempfile.TemporaryDirectory() as tmpdir:
            mock_get_ids.return_value = []
            mock_up.return_value = MagicMock(returncode=0)
            mock_ssh.return_value = 0

            with patch.object(sys, "argv", ["dl", f"./{os.path.basename(tmpdir)}"]):
                with patch("os.path.isdir", return_value=True):
                    main()

            # Verify symlink was NOT set up for local path
            mock_setup_symlink.assert_not_called()

    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.setup_worktree_symlink")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_workspace_ids")
    def test_no_symlink_for_existing_workspace(
        self, mock_get_ids, mock_ssh, mock_setup_symlink, mock_up
    ):
        """Existing workspaces should not use symlink (devpod backend)."""
        from devlaunch.dl import main
        import sys

        mock_get_ids.return_value = ["my-existing-workspace"]
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0

        with patch.object(sys, "argv", ["dl", "my-existing-workspace"]):
            main()

        # Verify symlink was NOT set up for existing workspace
        mock_setup_symlink.assert_not_called()

    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.setup_worktree_symlink")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_workspace_ids")
    def test_no_symlink_with_devpod_backend_flag(
        self, mock_get_ids, mock_ssh, mock_setup_symlink, mock_up
    ):
        """--backend devpod should not use symlink."""
        from devlaunch.dl import main
        import sys

        mock_get_ids.return_value = []
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0

        with patch.object(sys, "argv", ["dl", "--backend", "devpod", "owner/repo"]):
            main()

        # Verify symlink was NOT set up with devpod backend
        mock_setup_symlink.assert_not_called()
