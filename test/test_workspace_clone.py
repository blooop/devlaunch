"""Tests for WorkspaceCloneManager."""
# pylint: disable=redefined-outer-name,protected-access,unused-argument

from unittest.mock import patch, MagicMock

import pytest

from devlaunch.worktree.workspace_clone import WorkspaceCloneManager, _sanitize_branch_dir
from devlaunch.worktree.config import WorktreeConfig


@pytest.fixture
def tmp_repos_dir(tmp_path):
    """Create temporary repos directory."""
    repos_dir = tmp_path / "repos"
    repos_dir.mkdir()
    return repos_dir


@pytest.fixture
def config(tmp_repos_dir):
    """Create a WorktreeConfig with temporary directory."""
    return WorktreeConfig(repos_dir=tmp_repos_dir)


@pytest.fixture
def mock_repo_manager(tmp_repos_dir):
    """Create a mock RepositoryManager."""
    mgr = MagicMock()
    repo_root = tmp_repos_dir / "owner" / "repo"
    mgr.get_repo_path.return_value = repo_root
    mgr.get_bare_path.return_value = repo_root / ".bare"
    mgr.ensure_repo.return_value = MagicMock()
    return mgr


@pytest.fixture
def mock_storage():
    """Create a mock MetadataStorage."""
    return MagicMock()


@pytest.fixture
def mock_branch_manager():
    """Create a mock BranchManager."""
    return MagicMock()


@pytest.fixture
def clone_manager(config, mock_repo_manager, mock_storage, mock_branch_manager):
    """Create a WorkspaceCloneManager with mocked dependencies."""
    return WorkspaceCloneManager(
        config=config,
        repo_manager=mock_repo_manager,
        storage=mock_storage,
        branch_manager=mock_branch_manager,
    )


class TestSanitizeBranchDir:
    """Tests for _sanitize_branch_dir."""

    def test_simple_branch(self):
        assert _sanitize_branch_dir("main") == "main"

    def test_slash_branch(self):
        assert _sanitize_branch_dir("feature/my-branch") == "feature-my-branch"

    def test_dots_preserved(self):
        assert _sanitize_branch_dir("v1.2.3") == "v1.2.3"


class TestWorkspaceCloneManagerInit:
    """Tests for WorkspaceCloneManager initialization."""

    def test_uses_config_repos_dir(self, config, mock_repo_manager, mock_storage):
        mgr = WorkspaceCloneManager(
            config=config, repo_manager=mock_repo_manager, storage=mock_storage
        )
        assert mgr.config.repos_dir == config.repos_dir


class TestGetWorkspacePath:
    """Tests for get_workspace_path."""

    def test_returns_correct_path(self, clone_manager, tmp_repos_dir):
        """Test workspace path is a sibling of .bare."""
        path = clone_manager.get_workspace_path("owner", "repo", "nb4")
        assert path == tmp_repos_dir / "owner" / "repo" / "nb4"

    def test_sanitizes_branch(self, clone_manager, tmp_repos_dir):
        """Test workspace path sanitizes branch with slashes."""
        path = clone_manager.get_workspace_path("owner", "repo", "feature/my-branch")
        assert path == tmp_repos_dir / "owner" / "repo" / "feature-my-branch"


class TestWorkspaceExists:
    """Tests for workspace_exists."""

    def test_returns_false_when_no_dir(self, clone_manager):
        """Test returns False when workspace directory doesn't exist."""
        assert clone_manager.workspace_exists("owner", "repo", "main") is False

    def test_returns_false_when_no_git(self, clone_manager, tmp_repos_dir):
        """Test returns False when directory exists but has no .git."""
        ws_dir = tmp_repos_dir / "owner" / "repo" / "main"
        ws_dir.mkdir(parents=True)
        assert clone_manager.workspace_exists("owner", "repo", "main") is False

    def test_returns_true_when_valid(self, clone_manager, tmp_repos_dir):
        """Test returns True when directory has .git."""
        ws_dir = tmp_repos_dir / "owner" / "repo" / "main"
        ws_dir.mkdir(parents=True)
        (ws_dir / ".git").mkdir()
        assert clone_manager.workspace_exists("owner", "repo", "main") is True


class TestEnsureBranch:
    """Tests for ensure_branch."""

    def test_fetches_then_ensures(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir
    ):
        """Test that ensure_branch fetches then delegates to BranchManager."""
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path

        clone_manager.ensure_branch("owner", "repo", "newbranch")

        mock_repo_manager.fetch_repo.assert_called_once_with("owner", "repo")
        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path, "newbranch", create_remote=False
        )

    def test_continues_if_fetch_fails(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir
    ):
        """Test that ensure_branch continues even if fetch fails."""
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.fetch_repo.side_effect = RuntimeError("network error")

        clone_manager.ensure_branch("owner", "repo", "newbranch")

        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path, "newbranch", create_remote=False
        )


class TestEnsureWorkspace:
    """Tests for ensure_workspace."""

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_clones_from_bare_repo(
        self, mock_run, clone_manager, mock_repo_manager, mock_storage, tmp_repos_dir
    ):
        """Test that a new workspace is cloned from the bare repo."""
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        ws_path = repo_root / "nb4"

        clone_manager.ensure_workspace(
            "owner", "repo", "nb4", "git@github.com:owner/repo.git", "repo-nb4"
        )

        # Should have called: git clone, git remote set-url, git fetch,
        # git show-ref (remote ref check), git checkout -B
        assert mock_run.call_count == 5

        # First call: git clone from bare repo
        clone_call = mock_run.call_args_list[0]
        assert clone_call[0][0] == ["git", "clone", str(bare_path), str(ws_path)]

        # Second call: fix remote URL
        remote_call = mock_run.call_args_list[1]
        assert remote_call[0][0] == [
            "git",
            "remote",
            "set-url",
            "origin",
            "git@github.com:owner/repo.git",
        ]

        # Third call: fetch origin
        fetch_call = mock_run.call_args_list[2]
        assert fetch_call[0][0] == ["git", "fetch", "origin"]

        # Fourth call: show-ref to check remote branch (returns 0 = exists)
        showref_call = mock_run.call_args_list[3]
        assert showref_call[0][0] == [
            "git",
            "show-ref",
            "--verify",
            "refs/remotes/origin/nb4",
        ]

        # Fifth call: checkout -B from remote ref
        checkout_call = mock_run.call_args_list[4]
        assert checkout_call[0][0] == ["git", "checkout", "-B", "nb4", "origin/nb4"]

        # Should track in metadata
        mock_storage.add_worktree.assert_called_once()

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_new_workspace_new_branch_bases_on_default(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test that a new workspace for a new branch checks out from origin/<default>."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        # show-ref returns non-zero for the requested branch, zero for the default
        def run_side_effect(cmd, *args, **kwargs):
            if cmd[0:3] == ["git", "show-ref", "--verify"]:
                if "new-feature" in cmd[3]:
                    return MagicMock(returncode=1)
                # default branch (main) exists on remote
                return MagicMock(returncode=0)
            return MagicMock(returncode=0)

        mock_run.side_effect = run_side_effect

        base_repo = MagicMock()
        base_repo.default_branch = "main"
        mock_repo_manager.get_repo.return_value = base_repo

        clone_manager.ensure_workspace(
            "owner", "repo", "new-feature", "git@github.com:owner/repo.git", "repo-new-feature"
        )

        # Last call should be checkout -B <branch> origin/main
        checkout_call = mock_run.call_args_list[-1]
        assert checkout_call[0][0] == [
            "git",
            "checkout",
            "-B",
            "new-feature",
            "origin/main",
        ]

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_new_workspace_raises_when_no_remote_refs(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test error when neither branch nor default branch exist on remote."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        # show-ref always returns non-zero (no remote refs exist)
        def run_side_effect(cmd, *args, **kwargs):
            if cmd[0:3] == ["git", "show-ref", "--verify"]:
                return MagicMock(returncode=1)
            return MagicMock(returncode=0)

        mock_run.side_effect = run_side_effect

        base_repo = MagicMock()
        base_repo.default_branch = "main"
        mock_repo_manager.get_repo.return_value = base_repo

        with pytest.raises(RuntimeError, match="neither 'origin/new-feature' nor 'origin/main'"):
            clone_manager.ensure_workspace(
                "owner", "repo", "new-feature", "git@github.com:owner/repo.git", "repo-nf"
            )

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_existing_workspace_uses_plain_checkout(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test that existing workspaces use plain checkout to preserve local work."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        # Create existing workspace
        ws_path = repo_root / "nb4"
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        mock_run.return_value = MagicMock(returncode=0)

        clone_manager.ensure_workspace(
            "owner", "repo", "nb4", "git@github.com:owner/repo.git", "repo-nb4"
        )

        # Should only call fetch + checkout (no clone, no remote set-url, no show-ref)
        assert mock_run.call_count == 2
        checkout_call = mock_run.call_args_list[1]
        assert checkout_call[0][0] == ["git", "checkout", "nb4"]

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_existing_workspace_skips_clone(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test that existing workspace is not re-cloned."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        # Create existing workspace
        ws_path = repo_root / "nb4"
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        mock_run.return_value = MagicMock(returncode=0)

        clone_manager.ensure_workspace(
            "owner", "repo", "nb4", "git@github.com:owner/repo.git", "repo-nb4"
        )

        # Should only call fetch + checkout (no clone, no remote set-url)
        assert mock_run.call_count == 2
        fetch_call = mock_run.call_args_list[0]
        assert fetch_call[0][0] == ["git", "fetch", "origin"]
        checkout_call = mock_run.call_args_list[1]
        assert checkout_call[0][0] == ["git", "checkout", "nb4"]

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_ensures_bare_repo_first(self, mock_run, clone_manager, mock_repo_manager):
        """Test that ensure_repo is called before cloning."""
        mock_run.return_value = MagicMock(returncode=0)

        clone_manager.ensure_workspace(
            "owner", "repo", "nb4", "git@github.com:owner/repo.git", "repo-nb4"
        )

        mock_repo_manager.ensure_repo.assert_called_once_with(
            "owner", "repo", "git@github.com:owner/repo.git"
        )

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_clone_failure_raises(self, mock_run, clone_manager):
        """Test that clone failure raises RuntimeError."""
        mock_run.side_effect = __import__("subprocess").CalledProcessError(
            1, "git clone", stderr="fatal: error"
        )

        with pytest.raises(RuntimeError, match="Failed to clone workspace"):
            clone_manager.ensure_workspace(
                "owner", "repo", "main", "git@github.com:owner/repo.git", "repo-main"
            )

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_returns_workspace_path(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test that ensure_workspace returns the workspace path."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"
        mock_run.return_value = MagicMock(returncode=0)

        result = clone_manager.ensure_workspace(
            "owner", "repo", "main", "git@github.com:owner/repo.git", "repo-main"
        )

        assert result == repo_root / "main"


class TestRemoveWorkspace:
    """Tests for remove_workspace."""

    def test_removes_existing_workspace(
        self, clone_manager, mock_repo_manager, mock_storage, tmp_repos_dir
    ):
        """Test that existing workspace is removed."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        ws_path = repo_root / "nb4"
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()
        (ws_path / "file.txt").write_text("content")

        result = clone_manager.remove_workspace("owner", "repo", "nb4")

        assert result is True
        assert not ws_path.exists()
        mock_storage.remove_worktree.assert_called_once_with("owner", "repo", "nb4")

    def test_returns_false_for_nonexistent(self, clone_manager):
        """Test that removing nonexistent workspace returns False."""
        result = clone_manager.remove_workspace("owner", "repo", "nonexistent")
        assert result is False


class TestRemoveWorkspaceById:
    """Tests for remove_workspace_by_id."""

    def test_finds_and_removes(self, clone_manager, mock_storage, mock_repo_manager, tmp_repos_dir):
        """Test that workspace is found by ID and removed."""

        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        ws_path = repo_root / "nb4"
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        wt_info = MagicMock()
        wt_info.owner = "owner"
        wt_info.repo = "repo"
        wt_info.branch = "nb4"
        mock_storage.get_worktree_by_workspace_id.return_value = wt_info

        result = clone_manager.remove_workspace_by_id("repo-nb4")

        assert result is True
        mock_storage.get_worktree_by_workspace_id.assert_called_once_with("repo-nb4")

    def test_returns_false_when_not_found(self, clone_manager, mock_storage):
        """Test that returns False when workspace ID is not in metadata."""
        mock_storage.get_worktree_by_workspace_id.return_value = None

        result = clone_manager.remove_workspace_by_id("nonexistent")

        assert result is False


class TestValidateRef:
    """Tests for _validate_ref."""

    def test_accepts_simple_branch(self, clone_manager):
        assert clone_manager._validate_ref("main") == "main"

    def test_accepts_slashes(self, clone_manager):
        assert clone_manager._validate_ref("feature/my-branch") == "feature/my-branch"

    def test_rejects_leading_dash(self, clone_manager):
        with pytest.raises(ValueError, match="Invalid git ref name"):
            clone_manager._validate_ref("--evil")

    def test_rejects_empty(self, clone_manager):
        with pytest.raises(ValueError, match="Invalid git ref name"):
            clone_manager._validate_ref("")

    def test_rejects_spaces(self, clone_manager):
        with pytest.raises(ValueError, match="Invalid git ref name"):
            clone_manager._validate_ref("branch name")


class TestRemoteRefExists:
    """Tests for _remote_ref_exists."""

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_returns_true_when_ref_exists(self, mock_run, clone_manager, tmp_repos_dir):
        """Test returns True when remote ref exists."""
        mock_run.return_value = MagicMock(returncode=0)
        ws_path = tmp_repos_dir / "ws"

        result = clone_manager._remote_ref_exists(ws_path, "main")

        assert result is True
        mock_run.assert_called_once_with(
            ["git", "show-ref", "--verify", "refs/remotes/origin/main"],
            cwd=ws_path,
            capture_output=True,
            text=True,
            check=False,
        )

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_returns_false_when_ref_missing(self, mock_run, clone_manager, tmp_repos_dir):
        """Test returns False when remote ref does not exist."""
        mock_run.return_value = MagicMock(returncode=1)
        ws_path = tmp_repos_dir / "ws"

        result = clone_manager._remote_ref_exists(ws_path, "no-such-branch")

        assert result is False

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_custom_remote(self, mock_run, clone_manager, tmp_repos_dir):
        """Test with a custom remote name."""
        mock_run.return_value = MagicMock(returncode=0)
        ws_path = tmp_repos_dir / "ws"

        result = clone_manager._remote_ref_exists(ws_path, "main", remote="upstream")

        assert result is True
        mock_run.assert_called_once_with(
            ["git", "show-ref", "--verify", "refs/remotes/upstream/main"],
            cwd=ws_path,
            capture_output=True,
            text=True,
            check=False,
        )
