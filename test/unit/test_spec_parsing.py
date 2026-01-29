"""Unit tests for spec parsing and sanitization.

These are pure logic tests with no external commands - they test
data parsing, validation, and transformation functions.
"""

import pytest

from devlaunch.worktree.worktree_manager import sanitize_branch_name


@pytest.mark.unit
class TestSanitizeBranchName:
    """Tests for branch name sanitization."""

    def test_simple_branch_unchanged(self):
        """Test that simple branch names pass through."""
        assert sanitize_branch_name("main") == "main"
        assert sanitize_branch_name("develop") == "develop"
        assert sanitize_branch_name("feature") == "feature"

    def test_slash_replaced_with_hyphen(self):
        """Test that slashes are replaced with hyphens."""
        assert sanitize_branch_name("feature/test") == "feature-test"
        assert sanitize_branch_name("fix/bug/critical") == "fix-bug-critical"

    def test_special_chars_replaced(self):
        """Test that special characters are replaced with underscores."""
        assert sanitize_branch_name("feature@test") == "feature_test"
        assert sanitize_branch_name("fix#123") == "fix_123"

    def test_alphanumeric_preserved(self):
        """Test that alphanumeric characters are preserved."""
        assert sanitize_branch_name("v1.2.3") == "v1.2.3"
        assert sanitize_branch_name("release-2024") == "release-2024"

    def test_leading_trailing_dots_stripped(self):
        """Test that leading/trailing dots and hyphens are stripped."""
        assert sanitize_branch_name(".hidden") == "hidden"
        assert sanitize_branch_name("branch.") == "branch"
        assert sanitize_branch_name("-dashed-") == "dashed"

    def test_hyphen_underscore_dot_preserved(self):
        """Test that hyphens, underscores, and dots in middle are preserved."""
        assert sanitize_branch_name("feature-test") == "feature-test"
        assert sanitize_branch_name("feature_test") == "feature_test"
        assert sanitize_branch_name("v1.2.3-beta") == "v1.2.3-beta"


@pytest.mark.unit
class TestWorkspaceIdGeneration:
    """Tests for workspace ID generation."""

    def test_workspace_id_format(self):
        """Test workspace ID format is owner-repo-branch."""
        from devlaunch.worktree.worktree_manager import WorktreeManager

        # Create a mock manager
        manager = WorktreeManager.__new__(WorktreeManager)

        workspace_id = manager._generate_workspace_id("owner", "repo", "main")  # pylint: disable=protected-access
        assert workspace_id == "owner-repo-main"

    def test_workspace_id_with_slash_branch(self):
        """Test workspace ID with branch containing slash."""
        from devlaunch.worktree.worktree_manager import WorktreeManager

        manager = WorktreeManager.__new__(WorktreeManager)

        workspace_id = manager._generate_workspace_id("owner", "repo", "feature/test")  # pylint: disable=protected-access
        assert workspace_id == "owner-repo-feature-test"

    def test_workspace_id_truncation(self):
        """Test workspace ID is truncated if too long."""
        from devlaunch.worktree.worktree_manager import WorktreeManager

        manager = WorktreeManager.__new__(WorktreeManager)

        long_branch = "feature/" + "x" * 100
        workspace_id = manager._generate_workspace_id("owner", "repo", long_branch)  # pylint: disable=protected-access

        # Should be truncated to 50 chars max
        assert len(workspace_id) <= 50


@pytest.mark.unit
class TestDataModels:
    """Tests for data model serialization."""

    def test_base_repository_to_dict(self):
        """Test BaseRepository serialization."""
        from devlaunch.worktree.models import BaseRepository
        from pathlib import Path
        from datetime import datetime

        repo = BaseRepository(
            owner="owner",
            repo="repo",
            remote_url="https://github.com/owner/repo.git",
            local_path=Path("/tmp/repos/owner/repo"),
            default_branch="main",
            last_fetched=datetime(2024, 1, 1, 12, 0, 0),
            worktrees=["main", "develop"],
        )

        data = repo.to_dict()

        assert data["owner"] == "owner"
        assert data["repo"] == "repo"
        assert data["remote_url"] == "https://github.com/owner/repo.git"
        assert data["local_path"] == "/tmp/repos/owner/repo"
        assert data["default_branch"] == "main"
        assert data["last_fetched"] == "2024-01-01T12:00:00"
        assert data["worktrees"] == ["main", "develop"]

    def test_base_repository_from_dict(self):
        """Test BaseRepository deserialization."""
        from devlaunch.worktree.models import BaseRepository
        from pathlib import Path

        data = {
            "owner": "owner",
            "repo": "repo",
            "remote_url": "https://github.com/owner/repo.git",
            "local_path": "/tmp/repos/owner/repo",
            "default_branch": "main",
            "last_fetched": "2024-01-01T12:00:00",
            "worktrees": ["main"],
        }

        repo = BaseRepository.from_dict(data)

        assert repo.owner == "owner"
        assert repo.repo == "repo"
        assert repo.local_path == Path("/tmp/repos/owner/repo")
        assert repo.last_fetched is not None
        assert repo.last_fetched.year == 2024

    def test_worktree_info_to_dict(self):
        """Test WorktreeInfo serialization."""
        from devlaunch.worktree.models import WorktreeInfo
        from pathlib import Path
        from datetime import datetime

        worktree = WorktreeInfo(
            owner="owner",
            repo="repo",
            branch="main",
            local_path=Path("/tmp/worktrees/main"),
            workspace_id="owner-repo-main",
            created_at=datetime(2024, 1, 1, 12, 0, 0),
            last_used=datetime(2024, 1, 2, 12, 0, 0),
            devpod_workspace_id="my-workspace",
        )

        data = worktree.to_dict()

        assert data["owner"] == "owner"
        assert data["branch"] == "main"
        assert data["local_path"] == "/tmp/worktrees/main"
        assert data["workspace_id"] == "owner-repo-main"
        assert data["devpod_workspace_id"] == "my-workspace"

    def test_worktree_info_from_dict(self):
        """Test WorktreeInfo deserialization."""
        from devlaunch.worktree.models import WorktreeInfo
        from pathlib import Path

        data = {
            "owner": "owner",
            "repo": "repo",
            "branch": "feature/test",
            "local_path": "/tmp/worktrees/feature-test",
            "workspace_id": "owner-repo-feature-test",
            "created_at": "2024-01-01T12:00:00",
            "last_used": "2024-01-02T12:00:00",
            "devpod_workspace_id": None,
        }

        worktree = WorktreeInfo.from_dict(data)

        assert worktree.branch == "feature/test"
        assert worktree.local_path == Path("/tmp/worktrees/feature-test")
        assert worktree.devpod_workspace_id is None


@pytest.mark.unit
class TestWorktreeConfig:
    """Tests for WorktreeConfig."""

    def test_config_defaults(self):
        """Test default configuration values."""
        from devlaunch.worktree.config import WorktreeConfig

        config = WorktreeConfig()

        assert config.enabled is True
        assert config.auto_fetch is True
        assert config.fetch_interval == 3600
        assert config.auto_prune is True
        assert config.prune_after_days == 30

    def test_config_to_dict(self):
        """Test config serialization."""
        from devlaunch.worktree.config import WorktreeConfig
        from pathlib import Path

        config = WorktreeConfig(
            repos_dir=Path("/tmp/repos"),
            auto_fetch=False,
            fallback_image="ubuntu:latest",
        )

        data = config.to_dict()

        assert data["worktree"]["enabled"] is True
        assert data["worktree"]["auto_fetch"] is False
        assert data["worktree"]["repos_dir"] == "/tmp/repos"
        assert data["worktree"]["fallback_image"] == "ubuntu:latest"

    def test_config_from_dict(self):
        """Test config deserialization."""
        from devlaunch.worktree.config import WorktreeConfig

        data = {
            "worktree": {
                "enabled": False,
                "repos_dir": "/custom/path",
                "auto_fetch": False,
                "fetch_interval": 7200,
                "cleanup": {
                    "auto_prune": False,
                    "prune_after_days": 60,
                },
            }
        }

        config = WorktreeConfig.from_dict(data)

        assert config.enabled is False
        assert config.auto_fetch is False
        assert config.fetch_interval == 7200
        assert config.auto_prune is False
        assert config.prune_after_days == 60
