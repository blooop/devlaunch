"""Tests for worktree metadata storage."""
# pylint: disable=redefined-outer-name

import json
import tempfile
from datetime import datetime
from pathlib import Path

import pytest

from devlaunch.worktree.models import BaseRepository, WorktreeInfo
from devlaunch.worktree.storage import MetadataStorage


@pytest.fixture
def temp_storage():
    """Create a temporary storage instance."""
    with tempfile.TemporaryDirectory() as tmpdir:
        metadata_path = Path(tmpdir) / "metadata.json"
        storage = MetadataStorage(metadata_path)
        yield storage


class TestMetadataStorage:
    """Tests for MetadataStorage class."""

    def test_init_creates_parent_dir(self):
        """Test that initialization creates parent directory."""
        with tempfile.TemporaryDirectory() as tmpdir:
            metadata_path = Path(tmpdir) / "subdir" / "metadata.json"
            storage = MetadataStorage(metadata_path)
            assert metadata_path.parent.exists()
            assert storage.metadata_path == metadata_path

    def test_init_loads_empty_state(self, temp_storage):
        """Test that initialization creates empty repositories and worktrees."""
        assert temp_storage.repositories == {}
        assert temp_storage.worktrees == {}

    def test_add_repository(self, temp_storage):
        """Test adding a repository."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
            default_branch="main",
            last_fetched=datetime(2024, 1, 1, 12, 0),
            worktrees=[],
        )

        temp_storage.add_repository(repo)

        assert "test-owner/test-repo" in temp_storage.repositories
        assert temp_storage.repositories["test-owner/test-repo"] == repo

    def test_get_repository(self, temp_storage):
        """Test getting a repository."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
            default_branch="main",
        )

        temp_storage.add_repository(repo)
        retrieved = temp_storage.get_repository("test-owner", "test-repo")

        assert retrieved is not None
        assert retrieved.owner == "test-owner"
        assert retrieved.repo == "test-repo"

    def test_get_repository_not_found(self, temp_storage):
        """Test getting a non-existent repository."""
        retrieved = temp_storage.get_repository("nonexistent", "repo")
        assert retrieved is None

    def test_list_repositories(self, temp_storage):
        """Test listing repositories."""
        repo1 = BaseRepository(
            owner="owner1",
            repo="repo1",
            remote_url="https://github.com/owner1/repo1.git",
            local_path=Path("/tmp/repos/owner1/repo1"),
        )
        repo2 = BaseRepository(
            owner="owner2",
            repo="repo2",
            remote_url="https://github.com/owner2/repo2.git",
            local_path=Path("/tmp/repos/owner2/repo2"),
        )

        temp_storage.add_repository(repo1)
        temp_storage.add_repository(repo2)

        repos = temp_storage.list_repositories()
        assert len(repos) == 2
        assert repo1 in repos
        assert repo2 in repos

    def test_remove_repository(self, temp_storage):
        """Test removing a repository."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
        )

        temp_storage.add_repository(repo)
        temp_storage.remove_repository("test-owner", "test-repo")

        assert temp_storage.get_repository("test-owner", "test-repo") is None

    def test_remove_nonexistent_repository(self, temp_storage):
        """Test removing a non-existent repository doesn't raise."""
        temp_storage.remove_repository("nonexistent", "repo")

    def test_add_worktree(self, temp_storage):
        """Test adding a worktree."""
        # First add a repository
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
            worktrees=[],
        )
        temp_storage.add_repository(repo)

        worktree = WorktreeInfo(
            owner="test-owner",
            repo="test-repo",
            branch="feature-branch",
            local_path=Path("/tmp/worktrees/test-owner/test-repo/feature-branch"),
            workspace_id="feature-branch",
            created_at=datetime(2024, 1, 1, 10, 0),
            last_used=datetime(2024, 1, 1, 12, 0),
        )

        temp_storage.add_worktree(worktree)

        assert "test-owner/test-repo/feature-branch" in temp_storage.worktrees
        # Check that the repository's worktrees list was updated
        updated_repo = temp_storage.get_repository("test-owner", "test-repo")
        assert "feature-branch" in updated_repo.worktrees

    def test_get_worktree(self, temp_storage):
        """Test getting a worktree."""
        worktree = WorktreeInfo(
            owner="test-owner",
            repo="test-repo",
            branch="feature-branch",
            local_path=Path("/tmp/worktrees/test-owner/test-repo/feature-branch"),
            workspace_id="feature-branch",
        )

        temp_storage.add_worktree(worktree)
        retrieved = temp_storage.get_worktree("test-owner", "test-repo", "feature-branch")

        assert retrieved is not None
        assert retrieved.branch == "feature-branch"

    def test_get_worktree_not_found(self, temp_storage):
        """Test getting a non-existent worktree."""
        retrieved = temp_storage.get_worktree("nonexistent", "repo", "branch")
        assert retrieved is None

    def test_list_worktrees_all(self, temp_storage):
        """Test listing all worktrees."""
        wt1 = WorktreeInfo(
            owner="owner1",
            repo="repo1",
            branch="branch1",
            local_path=Path("/tmp/worktrees/owner1/repo1/branch1"),
            workspace_id="branch1",
        )
        wt2 = WorktreeInfo(
            owner="owner2",
            repo="repo2",
            branch="branch2",
            local_path=Path("/tmp/worktrees/owner2/repo2/branch2"),
            workspace_id="branch2",
        )

        temp_storage.add_worktree(wt1)
        temp_storage.add_worktree(wt2)

        worktrees = temp_storage.list_worktrees()
        assert len(worktrees) == 2

    def test_list_worktrees_filtered_by_owner_and_repo(self, temp_storage):
        """Test listing worktrees filtered by owner and repo."""
        wt1 = WorktreeInfo(
            owner="owner1",
            repo="repo1",
            branch="branch1",
            local_path=Path("/tmp/worktrees/owner1/repo1/branch1"),
            workspace_id="branch1",
        )
        wt2 = WorktreeInfo(
            owner="owner1",
            repo="repo1",
            branch="branch2",
            local_path=Path("/tmp/worktrees/owner1/repo1/branch2"),
            workspace_id="branch2",
        )
        wt3 = WorktreeInfo(
            owner="owner2",
            repo="repo2",
            branch="branch3",
            local_path=Path("/tmp/worktrees/owner2/repo2/branch3"),
            workspace_id="branch3",
        )

        temp_storage.add_worktree(wt1)
        temp_storage.add_worktree(wt2)
        temp_storage.add_worktree(wt3)

        worktrees = temp_storage.list_worktrees(owner="owner1", repo="repo1")
        assert len(worktrees) == 2
        branches = [wt.branch for wt in worktrees]
        assert "branch1" in branches
        assert "branch2" in branches

    def test_list_worktrees_filtered_by_owner_only(self, temp_storage):
        """Test listing worktrees filtered by owner only."""
        wt1 = WorktreeInfo(
            owner="owner1",
            repo="repo1",
            branch="branch1",
            local_path=Path("/tmp/worktrees/owner1/repo1/branch1"),
            workspace_id="branch1",
        )
        wt2 = WorktreeInfo(
            owner="owner2",
            repo="repo2",
            branch="branch2",
            local_path=Path("/tmp/worktrees/owner2/repo2/branch2"),
            workspace_id="branch2",
        )

        temp_storage.add_worktree(wt1)
        temp_storage.add_worktree(wt2)

        worktrees = temp_storage.list_worktrees(owner="owner1")
        assert len(worktrees) == 1
        assert worktrees[0].owner == "owner1"

    def test_remove_worktree(self, temp_storage):
        """Test removing a worktree."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
            worktrees=["feature-branch"],
        )
        temp_storage.add_repository(repo)

        worktree = WorktreeInfo(
            owner="test-owner",
            repo="test-repo",
            branch="feature-branch",
            local_path=Path("/tmp/worktrees/test-owner/test-repo/feature-branch"),
            workspace_id="feature-branch",
        )
        temp_storage.add_worktree(worktree)

        temp_storage.remove_worktree("test-owner", "test-repo", "feature-branch")

        assert temp_storage.get_worktree("test-owner", "test-repo", "feature-branch") is None
        # Check that the repository's worktrees list was updated
        updated_repo = temp_storage.get_repository("test-owner", "test-repo")
        assert "feature-branch" not in updated_repo.worktrees

    def test_remove_nonexistent_worktree(self, temp_storage):
        """Test removing a non-existent worktree doesn't raise."""
        temp_storage.remove_worktree("nonexistent", "repo", "branch")

    def test_persistence(self):
        """Test that data persists across storage instances."""
        with tempfile.TemporaryDirectory() as tmpdir:
            metadata_path = Path(tmpdir) / "metadata.json"

            # Create and populate first storage instance
            storage1 = MetadataStorage(metadata_path)
            repo = BaseRepository(
                owner="test-owner",
                repo="test-repo",
                remote_url="https://github.com/test-owner/test-repo.git",
                local_path=Path("/tmp/repos/test-owner/test-repo"),
            )
            storage1.add_repository(repo)

            worktree = WorktreeInfo(
                owner="test-owner",
                repo="test-repo",
                branch="feature-branch",
                local_path=Path("/tmp/worktrees/test-owner/test-repo/feature-branch"),
                workspace_id="feature-branch",
            )
            storage1.add_worktree(worktree)

            # Create second storage instance and verify data persists
            storage2 = MetadataStorage(metadata_path)
            assert storage2.get_repository("test-owner", "test-repo") is not None
            assert storage2.get_worktree("test-owner", "test-repo", "feature-branch") is not None

    def test_save_creates_valid_json(self, temp_storage):
        """Test that save creates valid JSON file."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
        )
        temp_storage.add_repository(repo)

        # Read the file directly and verify it's valid JSON
        with open(temp_storage.metadata_path, "r", encoding="utf-8") as f:
            data = json.load(f)

        assert "repositories" in data
        assert "worktrees" in data
        assert "test-owner/test-repo" in data["repositories"]
