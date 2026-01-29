"""Tests for worktree data models."""

from datetime import datetime
from pathlib import Path


from devlaunch.worktree.models import BaseRepository, WorktreeInfo


class TestBaseRepository:
    """Tests for BaseRepository model."""

    def test_creation(self):
        """Test creating a BaseRepository."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
            default_branch="main",
            last_fetched=datetime(2024, 1, 1, 12, 0),
            worktrees=["feature-1", "feature-2"],
        )

        assert repo.owner == "test-owner"
        assert repo.repo == "test-repo"
        assert repo.remote_url == "https://github.com/test-owner/test-repo.git"
        assert repo.local_path == Path("/tmp/repos/test-owner/test-repo")
        assert repo.default_branch == "main"
        assert repo.last_fetched == datetime(2024, 1, 1, 12, 0)
        assert repo.worktrees == ["feature-1", "feature-2"]

    def test_to_dict(self):
        """Test converting BaseRepository to dict."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
            default_branch="main",
            last_fetched=datetime(2024, 1, 1, 12, 0),
            worktrees=["feature-1"],
        )

        data = repo.to_dict()
        assert data["owner"] == "test-owner"
        assert data["repo"] == "test-repo"
        assert data["remote_url"] == "https://github.com/test-owner/test-repo.git"
        assert data["local_path"] == "/tmp/repos/test-owner/test-repo"
        assert data["default_branch"] == "main"
        assert data["last_fetched"] == "2024-01-01T12:00:00"
        assert data["worktrees"] == ["feature-1"]

    def test_from_dict(self):
        """Test creating BaseRepository from dict."""
        data = {
            "owner": "test-owner",
            "repo": "test-repo",
            "remote_url": "https://github.com/test-owner/test-repo.git",
            "local_path": "/tmp/repos/test-owner/test-repo",
            "default_branch": "main",
            "last_fetched": "2024-01-01T12:00:00",
            "worktrees": ["feature-1", "feature-2"],
        }

        repo = BaseRepository.from_dict(data)
        assert repo.owner == "test-owner"
        assert repo.repo == "test-repo"
        assert repo.remote_url == "https://github.com/test-owner/test-repo.git"
        assert repo.local_path == Path("/tmp/repos/test-owner/test-repo")
        assert repo.default_branch == "main"
        assert repo.last_fetched == datetime(2024, 1, 1, 12, 0)
        assert repo.worktrees == ["feature-1", "feature-2"]

    def test_from_dict_no_last_fetched(self):
        """Test creating BaseRepository from dict without last_fetched."""
        data = {
            "owner": "test-owner",
            "repo": "test-repo",
            "remote_url": "https://github.com/test-owner/test-repo.git",
            "local_path": "/tmp/repos/test-owner/test-repo",
            "default_branch": "main",
            "last_fetched": None,
            "worktrees": [],
        }

        repo = BaseRepository.from_dict(data)
        assert repo.last_fetched is None


class TestWorktreeInfo:
    """Tests for WorktreeInfo model."""

    def test_creation(self):
        """Test creating a WorktreeInfo."""
        created_at = datetime(2024, 1, 1, 10, 0)
        last_used = datetime(2024, 1, 1, 12, 0)

        worktree = WorktreeInfo(
            owner="test-owner",
            repo="test-repo",
            branch="feature-branch",
            local_path=Path("/tmp/worktrees/test-owner/test-repo/feature-branch"),
            workspace_id="feature-branch",
            created_at=created_at,
            last_used=last_used,
            devpod_workspace_id="feature-branch-ws",
        )

        assert worktree.owner == "test-owner"
        assert worktree.repo == "test-repo"
        assert worktree.branch == "feature-branch"
        assert worktree.local_path == Path("/tmp/worktrees/test-owner/test-repo/feature-branch")
        assert worktree.workspace_id == "feature-branch"
        assert worktree.created_at == created_at
        assert worktree.last_used == last_used
        assert worktree.devpod_workspace_id == "feature-branch-ws"

    def test_to_dict(self):
        """Test converting WorktreeInfo to dict."""
        worktree = WorktreeInfo(
            owner="test-owner",
            repo="test-repo",
            branch="feature-branch",
            local_path=Path("/tmp/worktrees/test-owner/test-repo/feature-branch"),
            workspace_id="feature-branch",
            created_at=datetime(2024, 1, 1, 10, 0),
            last_used=datetime(2024, 1, 1, 12, 0),
            devpod_workspace_id="feature-branch-ws",
        )

        data = worktree.to_dict()
        assert data["owner"] == "test-owner"
        assert data["repo"] == "test-repo"
        assert data["branch"] == "feature-branch"
        assert data["local_path"] == "/tmp/worktrees/test-owner/test-repo/feature-branch"
        assert data["workspace_id"] == "feature-branch"
        assert data["created_at"] == "2024-01-01T10:00:00"
        assert data["last_used"] == "2024-01-01T12:00:00"
        assert data["devpod_workspace_id"] == "feature-branch-ws"

    def test_from_dict(self):
        """Test creating WorktreeInfo from dict."""
        data = {
            "owner": "test-owner",
            "repo": "test-repo",
            "branch": "feature-branch",
            "local_path": "/tmp/worktrees/test-owner/test-repo/feature-branch",
            "workspace_id": "feature-branch",
            "created_at": "2024-01-01T10:00:00",
            "last_used": "2024-01-01T12:00:00",
            "devpod_workspace_id": "feature-branch-ws",
        }

        worktree = WorktreeInfo.from_dict(data)
        assert worktree.owner == "test-owner"
        assert worktree.repo == "test-repo"
        assert worktree.branch == "feature-branch"
        assert worktree.local_path == Path("/tmp/worktrees/test-owner/test-repo/feature-branch")
        assert worktree.workspace_id == "feature-branch"
        assert worktree.created_at == datetime(2024, 1, 1, 10, 0)
        assert worktree.last_used == datetime(2024, 1, 1, 12, 0)
        assert worktree.devpod_workspace_id == "feature-branch-ws"
