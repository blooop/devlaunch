"""Tests for worktree symlink functionality.

The worktree backend creates a ~/work symlink inside containers to provide
shorter terminal prompts instead of the long worktree path.
"""

from unittest.mock import patch, MagicMock

from devlaunch.dl import (
    get_worktree_symlink_path,
    setup_worktree_symlink,
    get_worktree_container_path,
    workspace_ssh,
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


class TestWorkspaceSshPreserveSymlink:
    """Tests for workspace_ssh preserve_symlink behavior.

    When preserve_symlink=True, workspace_ssh should use 'cd' instead of --workdir
    because DevPod's --workdir resolves symlinks, but 'cd' preserves them in $PWD.
    This ensures users see ~/work in their terminal prompt, not the resolved path.
    """

    @patch("devlaunch.dl.run_devpod")
    def test_preserve_symlink_uses_cd_for_interactive_shell(self, mock_run_devpod):
        """With preserve_symlink=True and no command, should use 'cd && exec $SHELL'."""
        mock_run_devpod.return_value = MagicMock(returncode=0)

        workspace_ssh("test-ws", workdir="/home/vscode/work", preserve_symlink=True)

        call_args = mock_run_devpod.call_args[0][0]
        assert "ssh" in call_args
        assert "--command" in call_args
        # Should use cd to preserve symlink path in $PWD
        cmd_idx = call_args.index("--command") + 1
        assert "cd /home/vscode/work" in call_args[cmd_idx]
        assert "exec $SHELL" in call_args[cmd_idx]
        # Should NOT use --workdir
        assert "--workdir" not in call_args

    @patch("devlaunch.dl.run_devpod")
    def test_preserve_symlink_uses_cd_for_command(self, mock_run_devpod):
        """With preserve_symlink=True and a command, should wrap with 'cd &&'."""
        mock_run_devpod.return_value = MagicMock(returncode=0)

        workspace_ssh(
            "test-ws",
            command="git status",
            workdir="/home/vscode/work",
            preserve_symlink=True,
        )

        call_args = mock_run_devpod.call_args[0][0]
        assert "--command" in call_args
        cmd_idx = call_args.index("--command") + 1
        assert "cd /home/vscode/work" in call_args[cmd_idx]
        assert "git status" in call_args[cmd_idx]
        assert "--workdir" not in call_args

    @patch("devlaunch.dl.run_devpod")
    def test_without_preserve_symlink_uses_workdir(self, mock_run_devpod):
        """Without preserve_symlink, should use --workdir (default behavior)."""
        mock_run_devpod.return_value = MagicMock(returncode=0)

        workspace_ssh("test-ws", workdir="/home/vscode/work", preserve_symlink=False)

        call_args = mock_run_devpod.call_args[0][0]
        assert "--workdir" in call_args
        workdir_idx = call_args.index("--workdir") + 1
        assert call_args[workdir_idx] == "/home/vscode/work"

    @patch("devlaunch.dl.run_devpod")
    def test_default_preserve_symlink_is_false(self, mock_run_devpod):
        """Default preserve_symlink should be False (backward compatible)."""
        mock_run_devpod.return_value = MagicMock(returncode=0)

        workspace_ssh("test-ws", workdir="/some/path")

        call_args = mock_run_devpod.call_args[0][0]
        # Default behavior uses --workdir
        assert "--workdir" in call_args


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
    """Integration tests for symlink setup in the main workspace flows.

    Note: Worktree backend is opt-in (--backend worktree), so these tests
    explicitly request it.
    """

    @patch("devlaunch.dl.workspace_up_worktree")
    @patch("devlaunch.dl.setup_worktree_symlink")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.get_default_branch_for_repo")
    def test_worktree_flow_creates_symlink_before_ssh(
        self,
        mock_get_default_branch,
        mock_get_ids,
        mock_ssh,
        mock_setup_symlink,
        mock_up_worktree,
    ):
        """Worktree workspace flow should create symlink before SSH."""
        from devlaunch.dl import main
        import sys

        mock_get_ids.return_value = []
        mock_get_default_branch.return_value = "main"
        mock_up_worktree.return_value = MagicMock(returncode=0)
        mock_setup_symlink.return_value = True
        mock_ssh.return_value = 0

        # Must use --backend worktree since devpod is now the default
        with patch.object(sys, "argv", ["dl", "--backend", "worktree", "owner/repo@main"]):
            main()

        # Verify symlink was set up
        mock_setup_symlink.assert_called_once()
        # Verify SSH uses symlink path with preserve_symlink=True
        # (preserve_symlink=True uses 'cd' instead of --workdir to keep symlink in $PWD)
        ssh_call = mock_ssh.call_args
        assert ssh_call[1]["workdir"] == "/home/vscode/work"
        assert ssh_call[1]["preserve_symlink"] is True

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
        """Restart subcommand should create symlink before SSH (worktree backend)."""
        from devlaunch.dl import main
        import sys

        mock_get_ids.return_value = []
        mock_get_default_branch.return_value = "main"
        mock_stop.return_value = 0
        mock_up_worktree.return_value = MagicMock(returncode=0)
        mock_setup_symlink.return_value = True
        mock_ssh.return_value = 0

        # Must use --backend worktree since devpod is now the default
        with patch.object(
            sys, "argv", ["dl", "--backend", "worktree", "owner/repo@main", "restart"]
        ):
            main()

        # Verify symlink was set up
        mock_setup_symlink.assert_called_once()
        # Verify SSH uses symlink path with preserve_symlink=True
        ssh_call = mock_ssh.call_args
        assert ssh_call[1]["workdir"] == "/home/vscode/work"
        assert ssh_call[1]["preserve_symlink"] is True

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
        """Code subcommand should create symlink after workspace_up_worktree (worktree backend)."""
        from devlaunch.dl import main
        import sys

        mock_get_ids.return_value = []
        mock_get_default_branch.return_value = "main"
        mock_up_worktree.return_value = MagicMock(returncode=0)
        mock_setup_symlink.return_value = True

        # Must use --backend worktree since devpod is now the default
        with patch.object(
            sys, "argv", ["dl", "--backend", "worktree", "owner/repo@main", "code"]
        ):
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
        """Code subcommand should skip symlink if workspace_up fails (worktree backend)."""
        from devlaunch.dl import main
        import sys

        mock_get_ids.return_value = []
        mock_get_default_branch.return_value = "main"
        mock_up_worktree.return_value = MagicMock(returncode=1)

        # Must use --backend worktree since devpod is now the default
        with patch.object(
            sys, "argv", ["dl", "--backend", "worktree", "owner/repo@main", "code"]
        ):
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
