"""Tests for worktree backend selection logic."""

import os
import sys
from unittest.mock import MagicMock, patch

from devlaunch.dl import main, should_use_worktree_backend


class TestWorktreeBackendSelection:
    """Tests for backend selection logic."""

    def test_git_repos_use_worktree_by_default(self):
        """Test that git repos use worktree backend by default."""
        # Clear any environment variable
        with patch.dict(os.environ, {}, clear=True):
            # GitHub repos should use worktree
            assert should_use_worktree_backend("owner/repo") is True
            assert should_use_worktree_backend("owner/repo@main") is True
            assert should_use_worktree_backend("github.com/owner/repo") is True
            assert should_use_worktree_backend("github.com/owner/repo@branch") is True

    def test_paths_dont_use_worktree(self):
        """Test that paths don't use worktree backend."""
        with patch.dict(os.environ, {}, clear=True):
            # Paths should not use worktree
            assert should_use_worktree_backend("./project") is False
            assert should_use_worktree_backend("/absolute/path") is False
            assert should_use_worktree_backend("~/home/path") is False

    def test_explicit_backend_flag_overrides(self):
        """Test that explicit backend flag overrides defaults."""
        # Force worktree even for paths
        assert should_use_worktree_backend("./project", backend="worktree") is True

        # Force devpod even for git repos
        assert should_use_worktree_backend("owner/repo", backend="devpod") is False

    def test_environment_variable_overrides_default(self):
        """Test that DEVLAUNCH_BACKEND env var overrides defaults."""
        # Force devpod via env var
        with patch.dict(os.environ, {"DEVLAUNCH_BACKEND": "devpod"}):
            assert should_use_worktree_backend("owner/repo") is False

        # Force worktree via env var
        with patch.dict(os.environ, {"DEVLAUNCH_BACKEND": "worktree"}):
            assert should_use_worktree_backend("./project") is True

    def test_backend_flag_overrides_env_var(self):
        """Test that backend flag overrides environment variable."""
        # Backend flag should override env var
        with patch.dict(os.environ, {"DEVLAUNCH_BACKEND": "devpod"}):
            assert should_use_worktree_backend("owner/repo", backend="worktree") is True

        with patch.dict(os.environ, {"DEVLAUNCH_BACKEND": "worktree"}):
            assert should_use_worktree_backend("owner/repo", backend="devpod") is False


class TestBackendFlagCLI:
    """Tests for --backend CLI flag."""

    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_backend_flag_devpod(self, _cache, mock_ssh, mock_up, mock_ids, mock_use_worktree):
        """Test --backend devpod flag forces DevPod backend."""
        mock_use_worktree.return_value = False
        mock_ids.return_value = []
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0

        with patch.object(sys, "argv", ["dl", "--backend", "devpod", "owner/repo"]):
            result = main()

        assert result == 0
        mock_use_worktree.assert_called_once_with("owner/repo", "devpod")

    @patch("devlaunch.dl.workspace_up_worktree")
    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_backend_flag_worktree(
        self, _cache, mock_ssh, mock_ids, mock_use_worktree, mock_up_worktree
    ):
        """Test --backend worktree flag forces worktree backend."""
        mock_use_worktree.return_value = True
        mock_ids.return_value = []
        mock_up_worktree.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0

        with patch.object(sys, "argv", ["dl", "--backend", "worktree", "owner/repo@main"]):
            result = main()

        assert result == 0
        mock_use_worktree.assert_called_once_with("owner/repo@main", "worktree")
        mock_up_worktree.assert_called_once()

    def test_backend_flag_invalid(self):
        """Test invalid --backend value."""
        with patch.object(sys, "argv", ["dl", "--backend", "invalid", "owner/repo"]):
            result = main()

        assert result == 1

    def test_backend_flag_missing_value(self):
        """Test --backend without value."""
        with patch.object(sys, "argv", ["dl", "--backend"]):
            result = main()

        assert result == 1

    def test_backend_flag_missing_workspace(self):
        """Test --backend with value but no workspace."""
        with patch.object(sys, "argv", ["dl", "--backend", "devpod"]):
            result = main()

        assert result == 1
