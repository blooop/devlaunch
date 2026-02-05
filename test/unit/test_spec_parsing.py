"""Unit tests for spec parsing and sanitization.

These are pure logic tests with no external commands - they test
data parsing, validation, and transformation functions.
"""

import pytest


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
