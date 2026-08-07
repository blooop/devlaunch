"""Tests for WorkspaceCloneManager."""
# pylint: disable=redefined-outer-name,protected-access,unused-argument

from pathlib import Path
from unittest.mock import patch, MagicMock

import pytest

from devlaunch.workspace_id import WorkspaceId
from devlaunch.worktree.workspace_clone import WorkspaceCloneManager
from devlaunch.worktree.config import WorktreeConfig


def leaf(branch="nb4", owner="owner", repo="repo"):
    """The clone-directory leaf name for a triple.

    Derived rather than hardcoded on purpose: the leaf and the devpod workspace id
    are the same string by construction, and what that string *is* for a given
    triple is pinned in test_workspace_id.py. Restating it here would only pin the
    same fact twice, and would make these tests fail for the wrong reason.
    """
    return WorkspaceId(owner, repo, branch).value


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
        assert path == tmp_repos_dir / "owner" / "repo" / leaf("nb4")

    def test_leaf_is_the_workspace_id(self, clone_manager):
        """The clone directory and the devpod workspace share one name.

        A bare branch name was unique only within its parent directory, so any
        consumer keying on a single path component saw every branch of a repo as
        one workspace (kinisi-robotics/kinisi_ros#9766).
        """
        path = clone_manager.get_workspace_path("owner", "repo", "feature/my-branch")
        assert path.name == WorkspaceId("owner", "repo", "feature/my-branch").value

    def test_leaf_is_self_identifying_across_repos(self, clone_manager, mock_repo_manager):
        """Same branch, different repo: the leaf names must differ, not just the parent."""
        mock_repo_manager.get_repo_path.side_effect = lambda o, r: Path("/cache") / o / r
        first = clone_manager.get_workspace_path("owner", "repo-one", "main")
        second = clone_manager.get_workspace_path("owner", "repo-two", "main")
        assert first.name != second.name

    def test_rejects_unvalidated_ref(self, clone_manager):
        """The path that used to be the unguarded one of three.

        `_validate_ref` returned a naked str, so nothing forced this call site to
        use it; now the leaf name cannot be produced without constructing a
        WorkspaceId, and that constructor validates.
        """
        with pytest.raises(ValueError, match="Invalid git ref"):
            clone_manager.get_workspace_path("owner", "repo", "--evil")

    def test_rejects_unvalidated_ref_via_workspace_exists(self, clone_manager):
        with pytest.raises(ValueError, match="Invalid git ref"):
            clone_manager.workspace_exists("owner", "repo", "branch name")

    def test_rejects_unvalidated_ref_via_ensure_workspace(self, clone_manager):
        with pytest.raises(ValueError, match="Invalid git ref"):
            clone_manager.ensure_workspace(
                "owner", "repo", "--evil", "git@github.com:owner/repo.git"
            )

    def test_rejects_unvalidated_ref_via_remove_workspace(self, clone_manager):
        with pytest.raises(ValueError, match="Invalid git ref"):
            clone_manager.remove_workspace("owner", "repo", "--evil")


class TestWorkspaceExists:
    """Tests for workspace_exists."""

    def test_returns_false_when_no_dir(self, clone_manager):
        """Test returns False when workspace directory doesn't exist."""
        assert clone_manager.workspace_exists("owner", "repo", "main") is False

    def test_returns_false_when_no_git(self, clone_manager, tmp_repos_dir):
        """Test returns False when directory exists but has no .git."""
        ws_dir = tmp_repos_dir / "owner" / "repo" / leaf("main")
        ws_dir.mkdir(parents=True)
        assert clone_manager.workspace_exists("owner", "repo", "main") is False

    def test_returns_true_when_valid(self, clone_manager, tmp_repos_dir):
        """Test returns True when directory has .git."""
        ws_dir = tmp_repos_dir / "owner" / "repo" / leaf("main")
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

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_clones_from_bare_repo(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, mock_storage, tmp_repos_dir
    ):
        """Test that a new workspace is cloned from the bare repo.

        Newly-created workspaces skip ``git fetch origin`` because they were
        just cloned from a freshly-fetched bare repo. git-lfs is absent here,
        so no LFS materialization calls are made.
        """
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        ws_path = repo_root / leaf()

        clone_manager.ensure_workspace("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        # Should have called: git clone, git remote set-url,
        # git show-ref (remote ref check), git checkout -B (no fetch)
        assert mock_run.call_count == 4

        # First call: git clone from bare repo, with LFS smudge disabled
        # (the bare cache has no LFS objects to smudge from)
        clone_call = mock_run.call_args_list[0]
        assert clone_call[0][0] == ["git", "clone", str(bare_path), str(ws_path)]
        assert clone_call[1]["env"]["GIT_LFS_SKIP_SMUDGE"] == "1"

        # Second call: fix remote URL
        remote_call = mock_run.call_args_list[1]
        assert remote_call[0][0] == [
            "git",
            "remote",
            "set-url",
            "origin",
            "git@github.com:owner/repo.git",
        ]

        # Third call: show-ref to check remote branch (returns 0 = exists)
        showref_call = mock_run.call_args_list[2]
        assert showref_call[0][0] == [
            "git",
            "show-ref",
            "--verify",
            "refs/remotes/origin/nb4",
        ]

        # Fourth call: checkout -B from remote ref (no fetch for new workspaces)
        checkout_call = mock_run.call_args_list[3]
        assert checkout_call[0][0] == ["git", "checkout", "-B", "nb4", "origin/nb4"]

        # Should track in metadata
        mock_storage.add_worktree.assert_called_once()

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_existing_workspace_retries_unmaterialized_lfs(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """A workspace left holding pointer files must retry on the next run.

        Materialization used to be gated on "did we just clone this", so one
        failed `git lfs pull` left the workspace existing-but-incomplete and every
        later run took the existing-workspace path — building against pointers
        forever, silently.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        # An existing workspace whose LFS file is still a pointer.
        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        big = ws_path / "big.bin"
        big.write_bytes(b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n")

        def run_side_effect(cmd, *args, **kwargs):
            if cmd[:3] == ["git", "lfs", "ls-files"]:
                return MagicMock(returncode=0, stdout="big.bin\n", stderr="")
            return MagicMock(returncode=0, stdout="", stderr="")

        mock_run.side_effect = run_side_effect

        clone_manager.ensure_workspace("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        issued = [c[0][0] for c in mock_run.call_args_list]
        assert ["git", "lfs", "pull", "origin"] in issued

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_materialized_workspace_does_not_refetch_lfs(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Real content present means no pull, so attaching stays fast."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        (ws_path / "big.bin").write_bytes(b"\x00\x01real binary content")

        def run_side_effect(cmd, *args, **kwargs):
            if cmd[:3] == ["git", "lfs", "ls-files"]:
                return MagicMock(returncode=0, stdout="big.bin\n", stderr="")
            return MagicMock(returncode=0, stdout="", stderr="")

        mock_run.side_effect = run_side_effect

        clone_manager.ensure_workspace("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        issued = [c[0][0] for c in mock_run.call_args_list]
        assert ["git", "lfs", "pull", "origin"] not in issued

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_new_workspace_materializes_lfs(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """New workspaces with LFS-tracked files run `git lfs pull` after checkout.

        The clone comes from the local bare cache (no LFS objects), so content
        must be pulled from the real origin once the remote URL is fixed.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        # git clone is mocked, so stand in for what it would have left behind:
        # a tree whose LFS file is still a pointer (cloned with skip-smudge).
        pointer = repo_root / leaf() / "assets" / "big.bin"
        pointer.parent.mkdir(parents=True)
        pointer.write_bytes(b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n")

        def run_side_effect(cmd, *args, **kwargs):
            if cmd[:3] == ["git", "lfs", "ls-files"]:
                return MagicMock(returncode=0, stdout="assets/big.bin\n", stderr="")
            return MagicMock(returncode=0, stdout="", stderr="")

        mock_run.side_effect = run_side_effect

        clone_manager.ensure_workspace("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        lfs_calls = [c[0][0] for c in mock_run.call_args_list if c[0][0][:2] == ["git", "lfs"]]
        assert ["git", "lfs", "ls-files", "--name-only"] in lfs_calls
        assert ["git", "lfs", "pull", "origin"] in lfs_calls

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_new_workspace_new_branch_bases_on_default(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
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
            "owner", "repo", "new-feature", "git@github.com:owner/repo.git"
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
                "owner", "repo", "new-feature", "git@github.com:owner/repo.git"
            )

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_existing_workspace_uses_plain_checkout(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test that existing workspaces use plain checkout to preserve local work."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        # Create existing workspace
        ws_path = repo_root / leaf()
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        mock_run.return_value = MagicMock(returncode=0)

        clone_manager.ensure_workspace("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        # Should only call fetch + checkout (no clone, no remote set-url, no show-ref)
        assert mock_run.call_count == 2
        checkout_call = mock_run.call_args_list[1]
        assert checkout_call[0][0] == ["git", "checkout", "nb4"]

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_existing_workspace_skips_clone(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test that existing workspace is not re-cloned."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        # Create existing workspace
        ws_path = repo_root / leaf()
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        mock_run.return_value = MagicMock(returncode=0)

        clone_manager.ensure_workspace("owner", "repo", "nb4", "git@github.com:owner/repo.git")

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

        clone_manager.ensure_workspace("owner", "repo", "nb4", "git@github.com:owner/repo.git")

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
            clone_manager.ensure_workspace("owner", "repo", "main", "git@github.com:owner/repo.git")

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
            "owner", "repo", "main", "git@github.com:owner/repo.git"
        )

        assert result == repo_root / leaf("main")

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_newly_created_workspace_skips_fetch(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Newly-created workspaces must not run ``git fetch origin``."""
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        clone_manager.ensure_workspace("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        # No call should contain "git fetch origin"
        fetch_calls = [c for c in mock_run.call_args_list if c[0][0] == ["git", "fetch", "origin"]]
        assert fetch_calls == []

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_existing_workspace_still_fetches(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Existing (stale) workspaces should still run ``git fetch origin``."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        # Create existing workspace
        ws_path = repo_root / leaf()
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        mock_run.return_value = MagicMock(returncode=0)

        clone_manager.ensure_workspace("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        fetch_calls = [c for c in mock_run.call_args_list if c[0][0] == ["git", "fetch", "origin"]]
        assert len(fetch_calls) == 1


class TestRemoveWorkspace:
    """Tests for remove_workspace."""

    def test_removes_existing_workspace(
        self, clone_manager, mock_repo_manager, mock_storage, tmp_repos_dir
    ):
        """Test that existing workspace is removed."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        ws_path = repo_root / leaf()
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

        ws_path = repo_root / leaf()
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


class TestRefsReachingGit:
    """The refs handed to git are all either proven or checked at the boundary.

    `_validate_ref` used to be a method returning its own argument, so a caller
    could skip it and nothing said so — which is how get_workspace_path ended up
    unguarded. It is gone: refs that name a workspace arrive inside a WorkspaceId,
    and the one ref that does not (the stored default branch) is checked with the
    same predicate where it enters argv.
    """

    @pytest.mark.parametrize("bad", ["--evil", "", "branch name"])
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_remote_ref_exists_rejects_unsafe_names(self, mock_run, bad, clone_manager, tmp_path):
        with pytest.raises(ValueError, match="Invalid git ref name"):
            clone_manager._remote_ref_exists(tmp_path, bad)
        mock_run.assert_not_called()

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_remote_ref_exists_rejects_unsafe_remote(self, mock_run, clone_manager, tmp_path):
        with pytest.raises(ValueError, match="Invalid git remote name"):
            clone_manager._remote_ref_exists(tmp_path, "main", remote="--upload-pack=evil")
        mock_run.assert_not_called()


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
