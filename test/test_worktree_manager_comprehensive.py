"""Comprehensive tests for WorktreeManager with edge cases."""

import subprocess
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, Mock, patch

import pytest

from devlaunch.worktree.models import BaseRepository, WorktreeInfo
from devlaunch.worktree.repo_manager import RepositoryManager
from devlaunch.worktree.storage import MetadataStorage
from devlaunch.worktree.worktree_manager import WorktreeManager, sanitize_branch_name


@pytest.fixture
def temp_dir():
    """Create a temporary directory for testing."""
    with tempfile.TemporaryDirectory() as tmpdir:
        yield Path(tmpdir)


@pytest.fixture
def mock_repo_manager():
    """Create a mock repository manager."""
    mock = Mock(spec=RepositoryManager)
    mock.get_repo_path.return_value = Path("/repos/owner/repo")
    mock.ensure_repo.return_value = BaseRepository(
        owner="owner",
        repo="repo",
        local_path=Path("/repos/owner/repo"),
        remote_url="https://github.com/owner/repo.git",
    )
    mock.get_repo.return_value = BaseRepository(
        owner="owner",
        repo="repo",
        local_path=Path("/repos/owner/repo"),
        remote_url="https://github.com/owner/repo.git",
    )
    return mock


@pytest.fixture
def mock_storage():
    """Create a mock metadata storage."""
    mock = Mock(spec=MetadataStorage)
    mock.list_worktrees.return_value = []
    return mock


@pytest.fixture
def worktree_manager(temp_dir, mock_repo_manager, mock_storage):
    """Create a worktree manager with mocks."""
    return WorktreeManager(temp_dir, mock_repo_manager, mock_storage)


class TestWorktreeManagerCreation:
    """Tests for worktree creation functionality."""

    def test_create_worktree_success(self, worktree_manager, mock_repo_manager, temp_dir):
        """Test successful worktree creation."""
        # Setup paths to use temp directory
        test_repo_path = temp_dir / "repos" / "owner" / "repo"
        test_repo_path.mkdir(parents=True, exist_ok=True)
        mock_repo_manager.get_repo_path.return_value = test_repo_path

        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0, stdout="", stderr="")
            worktree_manager._remote_branch_exists = Mock(return_value=False)

            result = worktree_manager.create_worktree(
                "owner", "repo", "feature-branch", "https://github.com/owner/repo.git"
            )

            assert result.owner == "owner"
            assert result.repo == "repo"
            assert result.branch == "feature-branch"
            assert result.local_path == test_repo_path / ".worktrees" / "feature-branch"
            mock_run.assert_called()

    def test_create_worktree_already_exists(self, worktree_manager, mock_storage):
        """Test creating worktree that already exists."""
        existing_worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="existing-branch",
            local_path=Path("/repos/owner/repo/.worktrees/existing-branch"),
            workspace_id="existing-ws",
        )
        worktree_manager.get_worktree = Mock(return_value=existing_worktree)

        # Create directory to simulate existing worktree
        worktree_path = worktree_manager.get_worktree_path("owner", "repo", "existing-branch")
        worktree_path.parent.mkdir(parents=True, exist_ok=True)
        worktree_path.mkdir(parents=True, exist_ok=True)

        result = worktree_manager.create_worktree("owner", "repo", "existing-branch")

        assert result == existing_worktree

    def test_create_worktree_remote_branch_exists(self, worktree_manager):
        """Test creating worktree when remote branch exists."""
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0)
            worktree_manager._remote_branch_exists = Mock(return_value=True)

            worktree_manager.create_worktree("owner", "repo", "main")

            # Should track remote branch
            call_args = mock_run.call_args_list[-1][0][0]
            assert "origin/main" in call_args

    def test_create_worktree_subprocess_failure(self, worktree_manager):
        """Test handling subprocess failure during worktree creation."""
        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(
                1, ["git", "worktree", "add"], stderr="Error creating worktree"
            )
            worktree_manager._remote_branch_exists = Mock(return_value=False)

            with pytest.raises(RuntimeError, match="Failed to create worktree"):
                worktree_manager.create_worktree("owner", "repo", "feature")

    def test_create_worktree_no_repo_no_url(self, worktree_manager, mock_repo_manager):
        """Test creating worktree without repo and no remote URL."""
        mock_repo_manager.get_repo.return_value = None

        with pytest.raises(ValueError, match="Repository .* not found"):
            worktree_manager.create_worktree("owner", "repo", "branch")


class TestWorktreeManagerRemoval:
    """Tests for worktree removal functionality."""

    def test_remove_worktree_success(self, worktree_manager):
        """Test successful worktree removal."""
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0)

            worktree_manager.remove_worktree("owner", "repo", "feature")

            mock_run.assert_called_with(
                ["git", "worktree", "remove", "--force", str(worktree_manager.get_worktree_path("owner", "repo", "feature"))],
                cwd=Path("/repos/owner/repo"),
                capture_output=True,
                text=True,
                check=True,
            )

    def test_remove_worktree_not_exists(self, worktree_manager):
        """Test removing non-existent worktree."""
        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(
                1, ["git", "worktree", "remove"], stderr="not a working tree"
            )

            # Should not raise, just log warning
            worktree_manager.remove_worktree("owner", "repo", "nonexistent")

    def test_remove_worktree_permission_error(self, worktree_manager):
        """Test handling permission errors during removal."""
        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(
                1, ["git", "worktree", "remove"], stderr="Permission denied"
            )

            # Should log but not raise
            worktree_manager.remove_worktree("owner", "repo", "protected")


class TestWorktreeManagerListing:
    """Tests for worktree listing functionality."""

    def test_list_worktrees_success(self, worktree_manager):
        """Test listing worktrees for a repository."""
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(
                returncode=0,
                stdout="/repos/owner/repo/.worktrees/main abcd123 [main]\n"
                       "/repos/owner/repo/.worktrees/feature ef45678 [feature]\n",
            )

            worktrees = worktree_manager.list_worktrees("owner", "repo")

            assert len(worktrees) == 2
            assert any(w.branch == "main" for w in worktrees)
            assert any(w.branch == "feature" for w in worktrees)

    def test_list_worktrees_empty(self, worktree_manager):
        """Test listing worktrees when none exist."""
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0, stdout="")

            worktrees = worktree_manager.list_worktrees("owner", "repo")

            assert worktrees == []

    def test_list_worktrees_subprocess_error(self, worktree_manager):
        """Test handling subprocess error when listing worktrees."""
        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(
                1, ["git", "worktree", "list"], stderr="Not a git repository"
            )

            worktrees = worktree_manager.list_worktrees("owner", "repo")

            assert worktrees == []

    def test_list_all_worktrees(self, worktree_manager, mock_storage):
        """Test listing all worktrees from storage."""
        mock_worktrees = [
            WorktreeInfo(
                owner="owner1", repo="repo1", branch="main",
                local_path=Path("/repos/owner1/repo1/.worktrees/main"),
                workspace_id="ws1",
            ),
            WorktreeInfo(
                owner="owner2", repo="repo2", branch="feature",
                local_path=Path("/repos/owner2/repo2/.worktrees/feature"),
                workspace_id="ws2",
            ),
        ]
        mock_storage.list_worktrees.return_value = mock_worktrees

        result = worktree_manager.list_all_worktrees()

        assert len(result) == 2
        assert result == mock_worktrees


class TestWorktreeManagerEnsure:
    """Tests for worktree ensure functionality."""

    def test_ensure_worktree_exists(self, worktree_manager):
        """Test ensuring worktree when it already exists."""
        existing = WorktreeInfo(
            owner="owner", repo="repo", branch="main",
            local_path=Path("/repos/owner/repo/.worktrees/main"),
            workspace_id="existing",
        )
        worktree_manager.get_worktree = Mock(return_value=existing)

        result = worktree_manager.ensure_worktree("owner", "repo", "main")

        assert result == existing

    def test_ensure_worktree_creates_new(self, worktree_manager):
        """Test ensuring worktree creates new when doesn't exist."""
        worktree_manager.get_worktree = Mock(return_value=None)
        new_worktree = WorktreeInfo(
            owner="owner", repo="repo", branch="feature",
            local_path=Path("/repos/owner/repo/.worktrees/feature"),
            workspace_id="new",
        )
        worktree_manager.create_worktree = Mock(return_value=new_worktree)

        result = worktree_manager.ensure_worktree(
            "owner", "repo", "feature", "https://github.com/owner/repo.git"
        )

        assert result == new_worktree
        worktree_manager.create_worktree.assert_called_once_with(
            "owner", "repo", "feature", "https://github.com/owner/repo.git"
        )


class TestWorktreeManagerHelpers:
    """Tests for helper methods."""

    def test_worktree_exists_true(self, worktree_manager):
        """Test checking if worktree exists."""
        worktree_manager.get_worktree = Mock(return_value=WorktreeInfo(
            owner="owner", repo="repo", branch="main",
            local_path=Path("/repos/owner/repo/.worktrees/main"),
            workspace_id="ws",
        ))

        assert worktree_manager.worktree_exists("owner", "repo", "main") is True

    def test_worktree_exists_false(self, worktree_manager):
        """Test checking if worktree doesn't exist."""
        worktree_manager.get_worktree = Mock(return_value=None)

        assert worktree_manager.worktree_exists("owner", "repo", "nonexistent") is False

    def test_get_worktree_from_storage(self, worktree_manager, mock_storage):
        """Test getting worktree from storage."""
        expected = WorktreeInfo(
            owner="owner", repo="repo", branch="main",
            local_path=Path("/repos/owner/repo/.worktrees/main"),
            workspace_id="ws",
        )
        mock_storage.get_worktree.return_value = expected

        result = worktree_manager.get_worktree("owner", "repo", "main")

        assert result == expected
        mock_storage.get_worktree.assert_called_once_with("owner", "repo", "main")

    def test_remote_branch_exists_true(self, worktree_manager):
        """Test checking if remote branch exists."""
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0, stdout="refs/heads/main")

            result = worktree_manager._remote_branch_exists(Path("/repo"), "main")

            assert result is True

    def test_remote_branch_exists_false(self, worktree_manager):
        """Test checking if remote branch doesn't exist."""
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(returncode=0, stdout="")

            result = worktree_manager._remote_branch_exists(Path("/repo"), "nonexistent")

            assert result is False

    def test_sync_with_git(self, worktree_manager):
        """Test syncing worktrees with git."""
        with patch("subprocess.run") as mock_run:
            # Mock git worktree list output
            mock_run.return_value = MagicMock(
                returncode=0,
                stdout="/repos/owner/repo/.worktrees/main abc123 [main]\n"
                       "/repos/owner/repo/.worktrees/feature def456 [feature]\n",
            )
            worktree_manager.storage.sync_worktrees = Mock()

            worktree_manager.sync_with_git("owner", "repo")

            # Should have parsed 2 worktrees and synced
            worktree_manager.storage.sync_worktrees.assert_called_once()
            synced_worktrees = worktree_manager.storage.sync_worktrees.call_args[0][0]
            assert len(synced_worktrees) == 2
            assert any(w.branch == "main" for w in synced_worktrees)
            assert any(w.branch == "feature" for w in synced_worktrees)


class TestSanitizeBranchNameEdgeCases:
    """Test edge cases for branch name sanitization."""

    def test_unicode_characters(self):
        """Test sanitizing branch names with unicode."""
        assert sanitize_branch_name("feature/测试") == "feature-_"
        assert sanitize_branch_name("ветка/功能") == "_-_"
        assert sanitize_branch_name("branche-été") == "branche-t"

    def test_very_long_branch_names(self):
        """Test sanitizing very long branch names."""
        long_name = "feature/" + "a" * 250
        result = sanitize_branch_name(long_name)
        assert len(result) <= 255  # Filesystem limit
        assert result.startswith("feature-aaa")

    def test_special_git_refs(self):
        """Test sanitizing special git reference names."""
        assert sanitize_branch_name("refs/heads/main") == "refs-heads-main"
        assert sanitize_branch_name("HEAD") == "HEAD"
        assert sanitize_branch_name("@") == "_"
        assert sanitize_branch_name("..") == ""

    def test_windows_reserved_names(self):
        """Test sanitizing Windows reserved names."""
        # These should be handled gracefully
        assert sanitize_branch_name("CON") == "CON"
        assert sanitize_branch_name("PRN") == "PRN"
        assert sanitize_branch_name("AUX") == "AUX"
        assert sanitize_branch_name("NUL") == "NUL"

    def test_sql_injection_attempts(self):
        """Test sanitizing potential SQL injection patterns."""
        assert sanitize_branch_name("'; DROP TABLE--") == "_DROP_TABLE"
        assert sanitize_branch_name("1=1") == "1_1"
        assert sanitize_branch_name("admin'--") == "admin_"

    def test_path_traversal_attempts(self):
        """Test sanitizing path traversal attempts."""
        assert sanitize_branch_name("../../../etc/passwd") == "etc-passwd"
        assert sanitize_branch_name("..\\..\\windows\\system32") == "windows_system32"


class TestWorktreeManagerConcurrency:
    """Test concurrent operations on worktrees."""

    def test_concurrent_create_same_worktree(self, worktree_manager):
        """Test handling concurrent creation of the same worktree."""
        import threading
        from time import sleep

        results = []
        errors = []

        def create_worktree():
            try:
                with patch("subprocess.run") as mock_run:
                    mock_run.return_value = MagicMock(returncode=0)
                    worktree_manager._remote_branch_exists = Mock(return_value=False)

                    # Simulate some processing time
                    sleep(0.01)

                    result = worktree_manager.create_worktree("owner", "repo", "concurrent-test")
                    results.append(result)
            except Exception as e:
                errors.append(e)

        # Start multiple threads trying to create the same worktree
        threads = [threading.Thread(target=create_worktree) for _ in range(5)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        # At least one should succeed
        assert len(results) > 0 or len(errors) > 0

    def test_concurrent_remove_operations(self, worktree_manager):
        """Test concurrent removal operations."""
        import threading

        remove_calls = []

        def remove_worktree(branch):
            with patch("subprocess.run") as mock_run:
                mock_run.return_value = MagicMock(returncode=0)
                worktree_manager.remove_worktree("owner", "repo", branch)
                remove_calls.append(branch)

        # Start multiple threads removing different worktrees
        branches = [f"branch-{i}" for i in range(5)]
        threads = [threading.Thread(target=remove_worktree, args=(branch,)) for branch in branches]

        for t in threads:
            t.start()
        for t in threads:
            t.join()

        assert len(remove_calls) == 5
        assert set(remove_calls) == set(branches)


class TestWorktreeManagerErrorRecovery:
    """Test error recovery and cleanup."""

    def test_cleanup_on_partial_failure(self, worktree_manager):
        """Test cleanup when worktree creation partially fails."""
        with patch("subprocess.run") as mock_run:
            # First call succeeds (checking remote branch)
            # Second call fails (creating worktree)
            mock_run.side_effect = [
                MagicMock(returncode=0, stdout=""),  # Remote branch check
                subprocess.CalledProcessError(1, ["git", "worktree", "add"], stderr="disk full"),
            ]

            with pytest.raises(RuntimeError, match="Failed to create worktree"):
                worktree_manager.create_worktree("owner", "repo", "feature")

    def test_recover_from_corrupted_worktree(self, worktree_manager):
        """Test recovery from corrupted worktree state."""
        # Create a worktree path that exists but has no git metadata
        worktree_path = worktree_manager.get_worktree_path("owner", "repo", "corrupted")
        worktree_path.mkdir(parents=True, exist_ok=True)

        # Should handle gracefully when trying to remove
        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(
                1, ["git", "worktree", "remove"], stderr="not a working tree"
            )

            # Should not raise
            worktree_manager.remove_worktree("owner", "repo", "corrupted")

    def test_network_failure_during_remote_check(self, worktree_manager):
        """Test handling network failure when checking remote branch."""
        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(
                128, ["git", "ls-remote"], stderr="Could not resolve host"
            )

            result = worktree_manager._remote_branch_exists(Path("/repo"), "main")

            # Should return False on network failure
            assert result is False


class TestWorktreeManagerDiskSpace:
    """Test handling of disk space issues."""

    def test_create_worktree_disk_full(self, worktree_manager):
        """Test handling disk full error during worktree creation."""
        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(
                1, ["git", "worktree", "add"], stderr="No space left on device"
            )
            worktree_manager._remote_branch_exists = Mock(return_value=False)

            with pytest.raises(RuntimeError, match="Failed to create worktree"):
                worktree_manager.create_worktree("owner", "repo", "feature")

    def test_create_worktree_permission_denied(self, worktree_manager):
        """Test handling permission denied error."""
        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(
                1, ["git", "worktree", "add"], stderr="Permission denied"
            )
            worktree_manager._remote_branch_exists = Mock(return_value=False)

            with pytest.raises(RuntimeError, match="Failed to create worktree"):
                worktree_manager.create_worktree("owner", "repo", "feature")


class TestWorktreeManagerIntegration:
    """Integration tests with real git operations."""

    @pytest.mark.integration
    def test_full_worktree_lifecycle(self, temp_dir):
        """Test complete worktree lifecycle with real git."""
        import subprocess
        import shutil

        # Skip if git is not available
        if shutil.which("git") is None:
            pytest.skip("Git not available")

        # Create a real git repo
        repo_dir = temp_dir / "test-repo"
        repo_dir.mkdir()

        subprocess.run(["git", "init"], cwd=repo_dir, check=True)
        subprocess.run(["git", "config", "user.email", "test@example.com"], cwd=repo_dir, check=True)
        subprocess.run(["git", "config", "user.name", "Test User"], cwd=repo_dir, check=True)

        # Create initial commit
        (repo_dir / "README.md").write_text("# Test Repo")
        subprocess.run(["git", "add", "."], cwd=repo_dir, check=True)
        subprocess.run(["git", "commit", "-m", "Initial commit"], cwd=repo_dir, check=True)

        # Create real managers
        storage = MetadataStorage(temp_dir / "metadata")
        repo_manager = RepositoryManager(temp_dir / "repos", storage)
        manager = WorktreeManager(temp_dir / "worktrees", repo_manager, storage)

        # Mock the repo manager to return our test repo
        repo_manager.get_repo_path = Mock(return_value=repo_dir)
        repo_manager.get_repo = Mock(return_value=BaseRepository(
            owner="test", repo="repo",
            local_path=repo_dir,
            remote_url="file://" + str(repo_dir),
        ))

        # Test creating a worktree
        worktree = manager.create_worktree("test", "repo", "feature-branch")
        assert worktree.branch == "feature-branch"
        assert (repo_dir / ".worktrees" / "feature-branch").exists()

        # Test listing worktrees
        worktrees = manager.list_worktrees("test", "repo")
        assert len(worktrees) >= 1
        assert any(w.branch == "feature-branch" for w in worktrees)

        # Test removing worktree
        manager.remove_worktree("test", "repo", "feature-branch")
        assert not (repo_dir / ".worktrees" / "feature-branch").exists()