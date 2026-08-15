"""Tests for worktree branch manager."""
# pylint: disable=redefined-outer-name,unused-argument

import os
import shlex
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
    def test_already_exists_answer_survives_a_translated_git(
        self, mock_run, branch_manager, temp_repo, monkeypatch
    ):
        """The benign/fatal split must hold on a host whose git speaks German.

        "The branch is already there" is told apart from a real failure by
        reading git's stderr text, and git marks that message for translation.
        So the split is only sound if git is always addressed in the C locale,
        whatever the host env says: on a host with git translations installed, a
        wrong env turns the ordinary re-launch of an existing branch into a
        raised error. Pinned at the env handed to the subprocess, which is the
        only place the guarantee can be made.
        """
        monkeypatch.setenv("LANG", "de_DE.UTF-8")
        monkeypatch.setenv("LC_ALL", "de_DE.UTF-8")
        monkeypatch.setenv("LANGUAGE", "de_DE:de")
        monkeypatch.setenv("SSH_AUTH_SOCK", "/tmp/sentinel-agent.sock")
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        branch_manager.create_local_branch(temp_repo, "new-branch")

        env = mock_run.call_args[1].get("env")
        assert env is not None, "git must get an explicit env pinning its message locale"
        assert env["LC_ALL"] == "C"
        # LANGUAGE outranks LC_ALL under gettext unless LC_ALL is C; pinned
        # anyway so the guarantee does not hang on that one glibc rule.
        assert env["LANGUAGE"] == "C"
        # The rest of the environment must survive — losing it breaks ssh auth.
        assert env["SSH_AUTH_SOCK"] == "/tmp/sentinel-agent.sock"

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
    def test_ssh_key_push_keeps_the_inherited_environment(
        self, mock_run, branch_manager, temp_repo, monkeypatch
    ):
        """Naming an ssh key must add to git's environment, not replace it.

        A push handed a bare ``{"GIT_SSH_COMMAND": ...}`` runs with no ``PATH``,
        ``HOME`` or ``SSH_AUTH_SOCK`` -- so git cannot find the ssh binary it was
        just told to use, cannot read ``~/.ssh/known_hosts``, and cannot reach the
        agent. Pinned at the env handed to the subprocess, the only place the
        guarantee can be made.
        """
        monkeypatch.setenv("SSH_AUTH_SOCK", "/tmp/sentinel-agent.sock")
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        branch_manager.push_branch_to_remote(temp_repo, "new-branch", ssh_key_path="/path/to/key")

        env = mock_run.call_args[1].get("env")
        assert env is not None, "an ssh key must reach git through an explicit env"
        assert "/path/to/key" in env["GIT_SSH_COMMAND"]
        # The rest of the environment must survive -- losing it breaks ssh auth.
        assert env["SSH_AUTH_SOCK"] == "/tmp/sentinel-agent.sock"
        assert env["PATH"] == os.environ["PATH"]

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_ssh_key_path_reaches_ssh_whole_when_it_contains_a_space(
        self, mock_run, branch_manager, temp_repo
    ):
        """A key path with a space must arrive at ssh as one argument.

        ``GIT_SSH_COMMAND`` is a shell string, not argv, so an unquoted path
        containing a space is split by the shell -- ssh is handed a truncated
        ``-i`` and the rest of the path as a hostname, and the push fails on the
        one setup naming a key was supposed to guarantee.
        """
        key = "/tmp/dl keys/id ed25519"
        mock_run.return_value = MagicMock(stdout="", stderr="", returncode=0)

        branch_manager.push_branch_to_remote(temp_repo, "new-branch", ssh_key_path=key)

        ssh_command = mock_run.call_args[1]["env"]["GIT_SSH_COMMAND"]
        assert shlex.split(ssh_command) == [
            "ssh",
            "-i",
            "/tmp/dl keys/id ed25519",
            "-o",
            "IdentitiesOnly=yes",
        ]

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_push_failure_reads_as_a_failure_when_git_said_nothing(
        self, mock_run, branch_manager, temp_repo
    ):
        """A push that failed silently must still name what happened.

        ``CalledProcessError.stderr`` is ``None`` when the output was never
        captured, and formatting it raw reports "Failed to push branch to
        remote: None" -- a message that tells whoever reads it nothing. The exit
        code is what is left to say.
        """
        mock_run.side_effect = subprocess.CalledProcessError(128, "git push", stderr=None)

        with pytest.raises(RuntimeError) as excinfo:
            branch_manager.push_branch_to_remote(temp_repo, "new-branch")

        assert "None" not in str(excinfo.value)
        assert "128" in str(excinfo.value)

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_create_local_branch_survives_stderr_less_failure(
        self, mock_run, branch_manager, temp_repo
    ):
        """A failure git wrote nothing to stderr for is still a failure.

        ``CalledProcessError.stderr`` is ``None`` when the output was never
        captured, so the already-exists arm must not read it unguarded -- an
        unguarded membership test turns that failure into a ``TypeError`` that
        names neither the branch nor the cause.
        """
        mock_run.side_effect = subprocess.CalledProcessError(1, "git branch", stderr=None)

        with pytest.raises(RuntimeError, match="Failed to create branch"):
            branch_manager.create_local_branch(temp_repo, "new-branch")

    @patch("devlaunch.worktree.branch_manager.subprocess.run")
    def test_push_branch_to_remote_failure(self, mock_run, branch_manager, temp_repo):
        """Test branch push failure."""
        mock_run.side_effect = subprocess.CalledProcessError(1, "git push", stderr="Push failed")

        with pytest.raises(RuntimeError, match="Failed to push branch"):
            branch_manager.push_branch_to_remote(temp_repo, "new-branch")


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

        mock_create.assert_called_once_with(temp_repo, "new-branch", "HEAD")
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

        mock_create.assert_called_once_with(temp_repo, "new-branch", "HEAD")

    @patch.object(BranchManager, "local_branch_exists")
    @patch.object(BranchManager, "remote_branch_exists")
    @patch.object(BranchManager, "create_local_branch")
    @patch.object(BranchManager, "push_branch_to_remote")
    @patch.object(BranchManager, "track_remote_branch")
    def test_branch_custom_start_point(
        self,
        mock_track,
        mock_push,
        mock_create,
        mock_remote_exists,
        mock_local_exists,
        branch_manager,
        temp_repo,
    ):
        """Test ensure_branch_exists passes custom start_point to create_local_branch."""
        mock_local_exists.return_value = False
        mock_remote_exists.return_value = False

        branch_manager.ensure_branch_exists(
            temp_repo, "new-branch", create_remote=False, start_point="origin/main"
        )

        mock_create.assert_called_once_with(temp_repo, "new-branch", "origin/main")
        mock_push.assert_not_called()


class TestEnsureBranchExistsUseLocalRefs:
    """Tests for ensure_branch_exists with use_local_refs=True."""

    @patch.object(BranchManager, "local_branch_exists")
    @patch.object(BranchManager, "remote_branch_exists")
    def test_use_local_refs_skips_ls_remote_when_branch_exists(
        self, mock_remote_exists, mock_local_exists, branch_manager, temp_repo
    ):
        """When use_local_refs=True and branch exists locally, remote_branch_exists is not called."""
        mock_local_exists.return_value = True

        branch_manager.ensure_branch_exists(temp_repo, "main", use_local_refs=True)

        mock_local_exists.assert_called_once()
        mock_remote_exists.assert_not_called()

    @patch.object(BranchManager, "local_branch_exists")
    @patch.object(BranchManager, "remote_branch_exists")
    @patch.object(BranchManager, "create_local_branch")
    @patch.object(BranchManager, "track_remote_branch")
    def test_use_local_refs_creates_branch_when_missing(
        self,
        mock_track,
        mock_create,
        mock_remote_exists,
        mock_local_exists,
        branch_manager,
        temp_repo,
    ):
        """When use_local_refs=True and branch missing locally, it is created without ls-remote."""
        mock_local_exists.return_value = False

        branch_manager.ensure_branch_exists(
            temp_repo, "new-branch", create_remote=False, use_local_refs=True
        )

        mock_remote_exists.assert_not_called()
        mock_create.assert_called_once_with(temp_repo, "new-branch", "HEAD")

    @patch.object(BranchManager, "local_branch_exists")
    @patch.object(BranchManager, "remote_branch_exists")
    @patch.object(BranchManager, "create_local_branch")
    @patch.object(BranchManager, "push_branch_to_remote")
    @patch.object(BranchManager, "track_remote_branch")
    def test_use_local_refs_false_still_calls_ls_remote(
        self,
        mock_track,
        mock_push,
        mock_create,
        mock_remote_exists,
        mock_local_exists,
        branch_manager,
        temp_repo,
    ):
        """Default use_local_refs=False still calls remote_branch_exists."""
        mock_local_exists.return_value = False
        mock_remote_exists.return_value = False

        branch_manager.ensure_branch_exists(temp_repo, "new-branch")

        mock_remote_exists.assert_called_once()
