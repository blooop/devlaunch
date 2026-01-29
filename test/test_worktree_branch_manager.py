"""Tests for worktree branch manager."""
# pylint: disable=redefined-outer-name

import subprocess
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from devlaunch.worktree.branch_manager import BranchManager


@pytest.fixture
def branch_manager():
    """Create a branch manager for testing."""
    return BranchManager()


@pytest.fixture
def temp_repo():
    """Create a temporary git repository for testing."""
    with tempfile.TemporaryDirectory() as tmpdir:
        repo_path = Path(tmpdir) / "repo"
        repo_path.mkdir()
        (repo_path / ".git").mkdir()
        yield repo_path


class TestBranchManager:
    """Tests for BranchManager class."""

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_local_branch_exists_true(self, mock_run, branch_manager, temp_repo):
        """Test local_branch_exists returns True when branch exists."""
        mock_run.return_value = MagicMock(returncode=0)

        result = branch_manager.local_branch_exists(temp_repo, "main")

        assert result is True
        mock_run.assert_called_once()
        call_args = mock_run.call_args[0][0]
        assert "show-ref" in call_args
        assert "refs/heads/main" in call_args

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_local_branch_exists_false(self, mock_run, branch_manager, temp_repo):
        """Test local_branch_exists returns False when branch doesn't exist."""
        mock_run.return_value = MagicMock(returncode=1)

        result = branch_manager.local_branch_exists(temp_repo, "nonexistent")

        assert result is False

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_local_branch_exists_exception(self, mock_run, branch_manager, temp_repo):
        """Test local_branch_exists returns False on exception."""
        mock_run.side_effect = Exception("Git error")

        result = branch_manager.local_branch_exists(temp_repo, "main")

        assert result is False

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_remote_branch_exists_true(self, mock_run, branch_manager, temp_repo):
        """Test remote_branch_exists returns True when branch exists."""
        mock_run.return_value = MagicMock(
            stdout="abc123\trefs/heads/main\n",
            returncode=0,
        )

        result = branch_manager.remote_branch_exists(temp_repo, "main")

        assert result is True

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_remote_branch_exists_false(self, mock_run, branch_manager, temp_repo):
        """Test remote_branch_exists returns False when branch doesn't exist."""
        mock_run.return_value = MagicMock(stdout="", returncode=0)

        result = branch_manager.remote_branch_exists(temp_repo, "nonexistent")

        assert result is False

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_remote_branch_exists_error(self, mock_run, branch_manager, temp_repo):
        """Test remote_branch_exists returns False on error."""
        mock_run.side_effect = subprocess.CalledProcessError(1, "git ls-remote")

        result = branch_manager.remote_branch_exists(temp_repo, "main")

        assert result is False

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_local_branch_success(self, mock_run, branch_manager, temp_repo):
        """Test successful local branch creation."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        branch_manager.create_local_branch(temp_repo, "new-branch")

        mock_run.assert_called_once()
        call_args = mock_run.call_args[0][0]
        assert "branch" in call_args
        assert "new-branch" in call_args
        assert "HEAD" in call_args

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_local_branch_with_start_point(self, mock_run, branch_manager, temp_repo):
        """Test local branch creation from start point."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        branch_manager.create_local_branch(temp_repo, "new-branch", "origin/main")

        call_args = mock_run.call_args[0][0]
        assert "origin/main" in call_args

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_local_branch_already_exists(self, mock_run, branch_manager, temp_repo):
        """Test create_local_branch handles existing branch gracefully."""
        mock_run.side_effect = subprocess.CalledProcessError(
            1, "git branch", stderr="fatal: branch already exists"
        )

        # Should not raise
        branch_manager.create_local_branch(temp_repo, "existing-branch")

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_local_branch_failure(self, mock_run, branch_manager, temp_repo):
        """Test create_local_branch raises on other errors."""
        mock_run.side_effect = subprocess.CalledProcessError(
            1, "git branch", stderr="fatal: some other error"
        )

        with pytest.raises(RuntimeError, match="Failed to create branch"):
            branch_manager.create_local_branch(temp_repo, "new-branch")

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_track_remote_branch_success(self, mock_run, branch_manager, temp_repo):
        """Test successful remote branch tracking."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        branch_manager.track_remote_branch(temp_repo, "main")

        mock_run.assert_called_once()
        call_args = mock_run.call_args[0][0]
        assert "--set-upstream-to=origin/main" in call_args
        assert "main" in call_args

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_track_remote_branch_custom_remote(self, mock_run, branch_manager, temp_repo):
        """Test tracking with custom remote."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        branch_manager.track_remote_branch(temp_repo, "main", "upstream")

        call_args = mock_run.call_args[0][0]
        assert "--set-upstream-to=upstream/main" in call_args

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_track_remote_branch_fails_silently(self, mock_run, branch_manager, temp_repo):
        """Test track_remote_branch doesn't raise on failure."""
        mock_run.side_effect = subprocess.CalledProcessError(1, "git branch")

        # Should not raise
        branch_manager.track_remote_branch(temp_repo, "main")

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_get_remote_branches_success(self, mock_run, branch_manager, temp_repo):
        """Test getting remote branches."""
        mock_run.return_value = MagicMock(
            stdout="abc123\trefs/heads/main\ndef456\trefs/heads/develop\n",
            returncode=0,
        )

        branches = branch_manager.get_remote_branches(temp_repo)

        assert branches == ["main", "develop"]

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_get_remote_branches_empty(self, mock_run, branch_manager, temp_repo):
        """Test getting remote branches when none exist."""
        mock_run.return_value = MagicMock(stdout="", returncode=0)

        branches = branch_manager.get_remote_branches(temp_repo)

        assert branches == []

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_get_remote_branches_error(self, mock_run, branch_manager, temp_repo):
        """Test getting remote branches on error."""
        mock_run.side_effect = subprocess.CalledProcessError(1, "git ls-remote")

        branches = branch_manager.get_remote_branches(temp_repo)

        assert branches == []

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_push_branch_to_remote_success(self, mock_run, branch_manager, temp_repo):
        """Test successful branch push."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        branch_manager.push_branch_to_remote(temp_repo, "new-branch")

        mock_run.assert_called_once()
        call_args = mock_run.call_args[0][0]
        assert "push" in call_args
        assert "-u" in call_args
        assert "origin" in call_args
        assert "new-branch" in call_args

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_push_branch_to_remote_with_ssh_key(self, mock_run, branch_manager, temp_repo):
        """Test branch push with SSH key."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        branch_manager.push_branch_to_remote(temp_repo, "new-branch", ssh_key_path="/path/to/key")

        call_kwargs = mock_run.call_args[1]
        assert "env" in call_kwargs
        assert "GIT_SSH_COMMAND" in call_kwargs["env"]
        assert "/path/to/key" in call_kwargs["env"]["GIT_SSH_COMMAND"]

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_push_branch_to_remote_failure(self, mock_run, branch_manager, temp_repo):
        """Test branch push failure."""
        mock_run.side_effect = subprocess.CalledProcessError(1, "git push", stderr="Push failed")

        with pytest.raises(RuntimeError, match="Failed to push branch"):
            branch_manager.push_branch_to_remote(temp_repo, "new-branch")

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_checkout_branch_success(self, mock_run, branch_manager, temp_repo):
        """Test successful branch checkout."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        branch_manager.checkout_branch(temp_repo, "main")

        mock_run.assert_called_once()
        call_args = mock_run.call_args[0][0]
        assert "checkout" in call_args
        assert "main" in call_args

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_checkout_branch_failure(self, mock_run, branch_manager, temp_repo):
        """Test checkout failure."""
        mock_run.side_effect = subprocess.CalledProcessError(
            1, "git checkout", stderr="Checkout failed"
        )

        with pytest.raises(RuntimeError, match="Failed to checkout branch"):
            branch_manager.checkout_branch(temp_repo, "main")


class TestEnsureBranchExists:
    """Tests for ensure_branch_exists method."""

    @patch.object(BranchManager, "local_branch_exists")
    @patch.object(BranchManager, "remote_branch_exists")
    def test_branch_exists_locally_and_remotely(
        self, mock_remote_exists, mock_local_exists, branch_manager, temp_repo
    ):
        """Test when branch exists both locally and remotely."""
        mock_local_exists.return_value = True
        mock_remote_exists.return_value = True

        branch_manager.ensure_branch_exists(temp_repo, "main")

        # Should not create anything
        mock_local_exists.assert_called_once()
        mock_remote_exists.assert_called_once()

    @patch.object(BranchManager, "local_branch_exists")
    @patch.object(BranchManager, "remote_branch_exists")
    @patch.object(BranchManager, "create_local_branch")
    @patch.object(BranchManager, "track_remote_branch")
    def test_branch_exists_remotely_only(
        self,
        mock_track,
        mock_create,
        mock_remote_exists,
        mock_local_exists,
        branch_manager,
        temp_repo,
    ):
        """Test when branch exists remotely but not locally."""
        mock_local_exists.return_value = False
        mock_remote_exists.return_value = True

        branch_manager.ensure_branch_exists(temp_repo, "main")

        mock_create.assert_called_once_with(temp_repo, "main", "origin/main")
        mock_track.assert_called_once_with(temp_repo, "main", "origin")

    @patch.object(BranchManager, "local_branch_exists")
    @patch.object(BranchManager, "remote_branch_exists")
    @patch.object(BranchManager, "create_local_branch")
    @patch.object(BranchManager, "push_branch_to_remote")
    @patch.object(BranchManager, "track_remote_branch")
    def test_branch_does_not_exist(
        self,
        mock_track,
        mock_push,
        mock_create,
        mock_remote_exists,
        mock_local_exists,
        branch_manager,
        temp_repo,
    ):
        """Test when branch doesn't exist anywhere."""
        mock_local_exists.return_value = False
        mock_remote_exists.return_value = False

        branch_manager.ensure_branch_exists(temp_repo, "new-branch")

        mock_create.assert_called_once_with(temp_repo, "new-branch")
        mock_push.assert_called_once()
        mock_track.assert_called_once()

    @patch.object(BranchManager, "local_branch_exists")
    @patch.object(BranchManager, "remote_branch_exists")
    @patch.object(BranchManager, "create_local_branch")
    def test_branch_no_create_remote(
        self, mock_create, mock_remote_exists, mock_local_exists, branch_manager, temp_repo
    ):
        """Test create_remote=False skips remote creation."""
        mock_local_exists.return_value = False
        mock_remote_exists.return_value = False

        branch_manager.ensure_branch_exists(temp_repo, "new-branch", create_remote=False)

        mock_create.assert_called_once()


class TestCreateRemoteBranchViaSSH:
    """Tests for create_remote_branch_via_ssh method."""

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_success(self, mock_run, branch_manager):
        """Test successful remote branch creation via SSH."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        result = branch_manager.create_remote_branch_via_ssh("owner", "repo", "new-branch")

        assert result is True
        call_args = mock_run.call_args[0][0]
        assert "ssh" in call_args
        assert "git@github.com" in call_args
        assert "create" in call_args
        assert "owner/repo" in call_args
        assert "new-branch" in call_args

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_with_ssh_key(self, mock_run, branch_manager):
        """Test remote branch creation with SSH key."""
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        result = branch_manager.create_remote_branch_via_ssh(
            "owner", "repo", "new-branch", ssh_key_path="/path/to/key"
        )

        assert result is True
        call_args = mock_run.call_args[0][0]
        assert "-i" in call_args
        assert "/path/to/key" in call_args

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_branch_already_exists(self, mock_run, branch_manager):
        """Test when branch already exists."""
        mock_run.return_value = MagicMock(
            stdout="",
            stderr="branch already exists",
            returncode=1,
        )

        result = branch_manager.create_remote_branch_via_ssh("owner", "repo", "existing")

        assert result is True

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_fails(self, mock_run, branch_manager):
        """Test when creation fails."""
        mock_run.return_value = MagicMock(
            stdout="",
            stderr="permission denied",
            returncode=1,
        )

        result = branch_manager.create_remote_branch_via_ssh("owner", "repo", "new-branch")

        assert result is False

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_timeout(self, mock_run, branch_manager):
        """Test timeout handling."""
        mock_run.side_effect = subprocess.TimeoutExpired("ssh", 10)

        result = branch_manager.create_remote_branch_via_ssh("owner", "repo", "new-branch")

        assert result is False

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_exception(self, mock_run, branch_manager):
        """Test general exception handling."""
        mock_run.side_effect = Exception("Unknown error")

        result = branch_manager.create_remote_branch_via_ssh("owner", "repo", "new-branch")

        assert result is False
