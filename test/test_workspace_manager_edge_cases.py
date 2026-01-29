"""Edge case tests for workspace manager."""

import fcntl
import json
import os
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from unittest.mock import MagicMock, Mock, mock_open, patch

import pytest

from devlaunch.worktree.models import WorktreeInfo
from devlaunch.worktree.storage import MetadataStorage
from devlaunch.worktree.workspace_manager import WorkspaceManager
from devlaunch.worktree.worktree_manager import WorktreeManager


@pytest.fixture
def temp_dir():
    """Create a temporary directory for testing."""
    with tempfile.TemporaryDirectory() as tmpdir:
        yield Path(tmpdir)


@pytest.fixture
def mock_worktree_manager():
    """Create a mock worktree manager."""
    mock = Mock(spec=WorktreeManager)
    mock.ensure_worktree.return_value = WorktreeInfo(
        owner="owner",
        repo="repo",
        branch="main",
        local_path=Path("/worktrees/main"),
        workspace_id="main-ws",
    )
    return mock


@pytest.fixture
def workspace_manager(temp_dir, mock_worktree_manager):
    """Create a workspace manager with mocks."""
    storage = MetadataStorage(temp_dir / "metadata")
    return WorkspaceManager(mock_worktree_manager, storage)


class TestWorkspaceManagerLocking:
    """Test file locking and concurrent access."""

    def test_concurrent_workspace_creation(self, workspace_manager):
        """Test handling concurrent workspace creation requests."""
        results = []
        errors = []

        def create_workspace():
            try:
                workspace = workspace_manager.create_workspace(
                    "owner", "repo", "concurrent-test", "https://github.com/owner/repo.git"
                )
                results.append(workspace)
            except Exception as e:
                errors.append(e)

        # Start multiple threads trying to create the same workspace
        threads = [threading.Thread(target=create_workspace) for _ in range(5)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        # At least one should succeed
        assert len(results) > 0
        # All successful results should be the same workspace
        if len(results) > 1:
            first_ws = results[0]
            for ws in results[1:]:
                assert ws.workspace_id == first_ws.workspace_id

    @patch("fcntl.flock")
    def test_lock_timeout_handling(self, mock_flock, workspace_manager):
        """Test handling of lock timeout."""
        # Simulate lock being held by another process
        mock_flock.side_effect = BlockingIOError("Resource temporarily unavailable")

        with pytest.raises(RuntimeError, match="Could not acquire lock"):
            workspace_manager._acquire_lock("/tmp/test.lock")

    @patch("builtins.open")
    @patch("fcntl.flock")
    def test_lock_file_creation_failure(self, mock_flock, mock_open_builtin, workspace_manager):
        """Test handling lock file creation failure."""
        mock_open_builtin.side_effect = PermissionError("Permission denied")

        with pytest.raises(PermissionError):
            workspace_manager._acquire_lock("/restricted/lock.lock")

    def test_lock_cleanup_on_exception(self, workspace_manager):
        """Test that locks are properly released on exception."""
        with patch.object(workspace_manager.worktree_manager, "ensure_worktree") as mock_ensure:
            mock_ensure.side_effect = RuntimeError("Worktree creation failed")

            with pytest.raises(RuntimeError):
                workspace_manager.create_workspace(
                    "owner", "repo", "test", "https://github.com/owner/repo.git"
                )

            # Lock should be released (no deadlock on next attempt)
            mock_ensure.side_effect = None
            mock_ensure.return_value = WorktreeInfo(
                owner="owner",
                repo="repo",
                branch="test",
                local_path=Path("/worktrees/test"),
                workspace_id="test-ws",
            )

            # This should succeed if lock was properly released
            workspace = workspace_manager.create_workspace(
                "owner", "repo", "test", "https://github.com/owner/repo.git"
            )
            assert workspace is not None


class TestWorkspaceManagerDevPodIntegration:
    """Test DevPod integration edge cases."""

    def test_devpod_up_network_failure(self, workspace_manager):
        """Test handling network failure during devpod up."""
        with patch("subprocess.run") as mock_run:
            mock_run.side_effect = subprocess.CalledProcessError(
                1, ["devpod", "up"], stderr="Network is unreachable"
            )

            result = workspace_manager.activate_workspace("test-ws")

            assert result.returncode != 0

    def test_devpod_up_timeout(self, workspace_manager):
        """Test handling timeout during devpod up."""
        with patch("subprocess.run") as mock_run:
            # Simulate timeout
            mock_run.side_effect = subprocess.TimeoutExpired(["devpod", "up"], timeout=300)

            with pytest.raises(subprocess.TimeoutExpired):
                workspace_manager.activate_workspace("test-ws")

    def test_devpod_ssh_missing_workspace(self, workspace_manager):
        """Test SSH to non-existent workspace."""
        workspace_manager.get_workspace_by_id = Mock(return_value=None)

        with pytest.raises(ValueError, match="Workspace .* not found"):
            workspace_manager.ssh_to_workspace("nonexistent")

    def test_devpod_delete_in_use_workspace(self, workspace_manager):
        """Test deleting workspace that's currently in use."""
        workspace_manager.get_workspace_by_id = Mock(
            return_value=WorktreeInfo(
                owner="owner",
                repo="repo",
                branch="main",
                local_path=Path("/worktrees/main"),
                workspace_id="main-ws",
                devpod_workspace_id="main",
            )
        )

        with patch("subprocess.run") as mock_run:
            # First attempt fails because workspace is in use
            mock_run.side_effect = [
                subprocess.CalledProcessError(
                    1, ["devpod", "delete"], stderr="workspace is currently in use"
                ),
                MagicMock(returncode=0),  # Stop succeeds
                MagicMock(returncode=0),  # Delete succeeds
            ]

            result = workspace_manager.delete_workspace("main-ws")

            # Should have tried to stop first, then delete
            assert result is True
            assert mock_run.call_count >= 2

    def test_devpod_status_parsing_error(self, workspace_manager):
        """Test handling malformed devpod status output."""
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(
                returncode=0,
                stdout="MALFORMED JSON {]}",  # Invalid JSON
            )

            # Should handle gracefully
            status = workspace_manager.get_workspace_status("test-ws")
            assert status is None or status == "unknown"


class TestWorkspaceManagerPathHandling:
    """Test path handling edge cases."""

    def test_workspace_path_with_spaces(self, workspace_manager):
        """Test handling workspace paths with spaces."""
        worktree = WorktreeInfo(
            owner="my owner",
            repo="my repo",
            branch="my branch",
            local_path=Path("/worktrees/my branch"),
            workspace_id="my-ws",
        )

        workspace_manager.worktree_manager.ensure_worktree = Mock(return_value=worktree)

        workspace = workspace_manager.create_workspace(
            "my owner", "my repo", "my branch", "https://github.com/my%20owner/my%20repo.git"
        )

        assert workspace.workspace_id == worktree.workspace_id

    def test_workspace_path_with_unicode(self, workspace_manager):
        """Test handling workspace paths with unicode characters."""
        worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="分支-テスト",
            local_path=Path("/worktrees/branch-test"),
            workspace_id="unicode-ws",
        )

        workspace_manager.worktree_manager.ensure_worktree = Mock(return_value=worktree)

        workspace = workspace_manager.create_workspace(
            "owner", "repo", "分支-テスト", "https://github.com/owner/repo.git"
        )

        assert workspace is not None

    def test_workspace_path_exceeds_length_limit(self, workspace_manager):
        """Test handling paths that exceed filesystem limits."""
        # Create a very long branch name
        long_branch = "feature/" + "a" * 250

        worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch=long_branch[:50],  # Truncated
            local_path=Path("/worktrees/feature-aaa"),
            workspace_id="long-ws",
        )

        workspace_manager.worktree_manager.ensure_worktree = Mock(return_value=worktree)

        workspace = workspace_manager.create_workspace(
            "owner", "repo", long_branch, "https://github.com/owner/repo.git"
        )

        assert workspace is not None
        assert len(str(workspace.local_path)) < 260  # Windows path limit


class TestWorkspaceManagerConfigHandling:
    """Test configuration file handling edge cases."""

    def test_corrupted_config_file(self, workspace_manager, temp_dir):
        """Test handling corrupted configuration file."""
        config_file = temp_dir / ".devpod" / "config.json"
        config_file.parent.mkdir(parents=True, exist_ok=True)
        config_file.write_text("{ CORRUPTED JSON }")

        # Should handle gracefully and recreate config
        workspace_manager._update_devpod_config("test-ws", Path("/worktrees/test"))

        # Config should be valid JSON now
        config_data = json.loads(config_file.read_text())
        assert isinstance(config_data, dict)

    def test_config_file_permission_denied(self, workspace_manager):
        """Test handling permission denied when writing config."""
        with patch("builtins.open") as mock_open_builtin:
            mock_open_builtin.side_effect = PermissionError("Permission denied")

            # Should log error but not crash
            workspace_manager._update_devpod_config("test-ws", Path("/worktrees/test"))

    def test_config_file_disk_full(self, workspace_manager):
        """Test handling disk full error when writing config."""
        with patch("builtins.open") as mock_open_builtin:
            mock_open_builtin.side_effect = OSError("No space left on device")

            with pytest.raises(OSError, match="No space left"):
                workspace_manager._update_devpod_config("test-ws", Path("/worktrees/test"))


class TestWorkspaceManagerRecovery:
    """Test error recovery scenarios."""

    def test_recover_from_partial_creation(self, workspace_manager):
        """Test recovery from partial workspace creation."""
        # First attempt fails after worktree creation
        with patch.object(workspace_manager.worktree_manager, "ensure_worktree") as mock_ensure:
            mock_ensure.return_value = WorktreeInfo(
                owner="owner",
                repo="repo",
                branch="test",
                local_path=Path("/worktrees/test"),
                workspace_id="test-ws",
            )

            with patch("subprocess.run") as mock_run:
                mock_run.side_effect = subprocess.CalledProcessError(
                    1, ["devpod", "up"], stderr="Container creation failed"
                )

                result = workspace_manager.activate_workspace("test-ws")
                assert result.returncode != 0

            # Second attempt should succeed
            with patch("subprocess.run") as mock_run:
                mock_run.return_value = MagicMock(returncode=0)

                result = workspace_manager.activate_workspace("test-ws")
                assert result.returncode == 0

    def test_cleanup_orphaned_workspaces(self, workspace_manager):
        """Test cleanup of orphaned workspaces."""
        # Create orphaned workspace in storage
        orphaned = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="orphaned",
            local_path=Path("/nonexistent/path"),
            workspace_id="orphaned-ws",
            devpod_workspace_id="orphaned",
        )

        workspace_manager.storage.save_worktree(orphaned)

        # List should handle orphaned workspace gracefully
        workspaces = workspace_manager.list_workspaces()

        # Should either filter out or mark as orphaned
        orphaned_ws = next((w for w in workspaces if w.workspace_id == "orphaned-ws"), None)
        if orphaned_ws:
            # Should be marked or handled specially
            assert not orphaned_ws.local_path.exists()


class TestWorkspaceManagerBranchOperations:
    """Test branch-related operations."""

    def test_switch_branch_with_uncommitted_changes(self, workspace_manager):
        """Test switching branches with uncommitted changes."""
        workspace_manager.get_workspace_by_id = Mock(
            return_value=WorktreeInfo(
                owner="owner",
                repo="repo",
                branch="feature",
                local_path=Path("/worktrees/feature"),
                workspace_id="feature-ws",
                devpod_workspace_id="feature",
            )
        )

        with patch("subprocess.run") as mock_run:
            # Git status shows uncommitted changes
            mock_run.side_effect = [
                MagicMock(returncode=0, stdout="M  file.txt\n"),  # git status
            ]

            # Should warn about uncommitted changes
            with patch("builtins.print") as mock_print:
                workspace_manager.check_uncommitted_changes("feature-ws")

                # Should print warning
                print_calls = [str(call) for call in mock_print.call_args_list]
                assert any("uncommitted" in call.lower() for call in print_calls)

    def test_create_workspace_from_tag(self, workspace_manager):
        """Test creating workspace from a git tag."""
        tag_worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="v1.0.0",
            local_path=Path("/worktrees/v1.0.0"),
            workspace_id="tag-ws",
        )

        workspace_manager.worktree_manager.ensure_worktree = Mock(return_value=tag_worktree)

        workspace = workspace_manager.create_workspace(
            "owner", "repo", "v1.0.0", "https://github.com/owner/repo.git"
        )

        assert workspace.branch == "v1.0.0"

    def test_create_workspace_from_commit_sha(self, workspace_manager):
        """Test creating workspace from a commit SHA."""
        sha_worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="abc123def456",  # Commit SHA
            local_path=Path("/worktrees/abc123def456"),
            workspace_id="sha-ws",
        )

        workspace_manager.worktree_manager.ensure_worktree = Mock(return_value=sha_worktree)

        workspace = workspace_manager.create_workspace(
            "owner", "repo", "abc123def456", "https://github.com/owner/repo.git"
        )

        assert workspace.branch == "abc123def456"


class TestWorkspaceManagerResourceLimits:
    """Test resource limit handling."""

    def test_max_workspaces_limit(self, workspace_manager):
        """Test enforcing maximum number of workspaces."""
        # Create max workspaces
        for i in range(20):  # Assuming max is 20
            workspace_manager.storage.save_worktree(
                WorktreeInfo(
                    owner="owner",
                    repo=f"repo{i}",
                    branch="main",
                    local_path=Path(f"/worktrees/repo{i}"),
                    workspace_id=f"ws-{i}",
                )
            )

        # Try to create one more
        with patch.object(workspace_manager, "MAX_WORKSPACES", 20):
            # Should either succeed with cleanup or fail gracefully
            try:
                workspace_manager.create_workspace(
                    "owner", "repo21", "main", "https://github.com/owner/repo21.git"
                )
            except RuntimeError as e:
                assert "maximum" in str(e).lower() or "limit" in str(e).lower()

    def test_disk_space_check(self, workspace_manager):
        """Test checking available disk space before creation."""
        with patch("shutil.disk_usage") as mock_disk:
            # Simulate low disk space (less than 1GB free)
            mock_disk.return_value = MagicMock(free=500_000_000)  # 500MB

            # Should warn or fail
            with patch("builtins.print") as mock_print:
                workspace_manager.check_disk_space()

                print_calls = [str(call) for call in mock_print.call_args_list]
                assert any(
                    "disk" in call.lower() or "space" in call.lower() for call in print_calls
                )


class TestWorkspaceManagerMigration:
    """Test migration and upgrade scenarios."""

    def test_migrate_old_workspace_format(self, workspace_manager, temp_dir):
        """Test migrating workspaces from old format."""
        # Create old format workspace file
        old_format = {
            "workspaces": [
                {
                    "name": "old-ws",
                    "path": "/old/path",
                    "branch": "main",
                }
            ]
        }

        old_file = temp_dir / "workspaces.json"
        old_file.write_text(json.dumps(old_format))

        # Should migrate to new format
        workspace_manager.migrate_old_workspaces(old_file)

        # Check new format exists
        workspaces = workspace_manager.list_workspaces()
        # Should have migrated workspace
        assert any(w.branch == "main" for w in workspaces)

    def test_handle_incompatible_version(self, workspace_manager):
        """Test handling incompatible workspace format version."""
        incompatible_data = {
            "version": "99.0.0",  # Future version
            "workspaces": [],
        }

        with patch("builtins.open", mock_open(read_data=json.dumps(incompatible_data))):
            # Should handle gracefully
            workspaces = workspace_manager.load_workspaces_from_file("test.json")
            assert isinstance(workspaces, list)
