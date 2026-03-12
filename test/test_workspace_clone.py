"""Tests for WorkspaceCloneManager."""
# pylint: disable=redefined-outer-name

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
        """Test that ensure_branch lazy-fetches then delegates to BranchManager."""
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"

        clone_manager.ensure_branch("owner", "repo", "newbranch")

        mock_repo_manager.lazy_fetch.assert_called_once_with("owner", "repo")
        mock_repo_manager.get_default_branch.assert_called_once_with("owner", "repo")
        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="main",
            use_local_refs=True,
        )

    def test_continues_if_fetch_fails(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir
    ):
        """Test that ensure_branch continues even if fetch fails."""
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.lazy_fetch.side_effect = RuntimeError("network error")
        mock_repo_manager.get_default_branch.return_value = "main"

        clone_manager.ensure_branch("owner", "repo", "newbranch")

        mock_repo_manager.get_default_branch.assert_called_once_with("owner", "repo")
        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="main",
            use_local_refs=True,
        )

    def test_falls_back_to_head_if_get_default_branch_fails(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir
    ):
        """Test that ensure_branch falls back to HEAD when get_default_branch raises."""
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.side_effect = RuntimeError("no HEAD")

        clone_manager.ensure_branch("owner", "repo", "newbranch")

        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="HEAD",
            use_local_refs=True,
        )

    def test_falls_back_to_head_if_get_default_branch_returns_empty(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir
    ):
        """Test that ensure_branch falls back to HEAD when get_default_branch returns empty."""
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = ""

        clone_manager.ensure_branch("owner", "repo", "newbranch")

        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="HEAD",
            use_local_refs=True,
        )


class TestEnsureWorkspace:
    """Tests for ensure_workspace."""

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_clones_from_bare_repo(
        self, mock_run, clone_manager, mock_repo_manager, mock_storage, tmp_repos_dir
    ):
        """Test that a new workspace is cloned from the bare repo.

        Newly-created workspaces skip ``git fetch origin`` because they were
        just cloned from a freshly-fetched bare repo.
        """
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        ws_path = repo_root / "nb4"

        clone_manager.ensure_workspace(
            "owner", "repo", "nb4", "git@github.com:owner/repo.git", "repo-nb4"
        )

        # Should have called: git clone, git remote set-url, git checkout (no fetch)
        assert mock_run.call_count == 3

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

        # Third call: checkout branch (no fetch for newly-created workspaces)
        checkout_call = mock_run.call_args_list[2]
        assert checkout_call[0][0] == ["git", "checkout", "nb4"]

        # Should track in metadata
        mock_storage.add_worktree.assert_called_once()

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


    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_newly_created_workspace_skips_fetch(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Newly-created workspaces must not run ``git fetch origin``."""
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        clone_manager.ensure_workspace(
            "owner", "repo", "nb4", "git@github.com:owner/repo.git", "repo-nb4"
        )

        # No call should contain "git fetch origin"
        fetch_calls = [
            c for c in mock_run.call_args_list if c[0][0] == ["git", "fetch", "origin"]
        ]
        assert fetch_calls == []

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_existing_workspace_still_fetches(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Existing (stale) workspaces should still run ``git fetch origin``."""
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

        fetch_calls = [
            c for c in mock_run.call_args_list if c[0][0] == ["git", "fetch", "origin"]
        ]
        assert len(fetch_calls) == 1


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
