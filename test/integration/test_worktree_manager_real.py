"""Integration tests for WorktreeManager with real git operations.

These tests run real git worktree commands against temporary repositories.
They verify worktree creation, removal, and git command functionality.
"""

import subprocess

import pytest


@pytest.mark.integration
class TestWorktreeCreation:
    """Tests for real git worktree creation."""

    def test_create_worktree_for_existing_branch(self, real_managers, local_git_repo):
        """Test creating a worktree for an existing branch."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone first
        repo_manager.clone_repo("test", "repo", remote_url)

        # Create worktree for main branch
        worktree = worktree_manager.create_worktree("test", "repo", "main")

        assert worktree is not None
        assert worktree.branch == "main"
        assert worktree.local_path.exists()

        # Verify it's a valid worktree (has .git file, not directory)
        git_path = worktree.local_path / ".git"
        assert git_path.exists()
        assert git_path.is_file()  # Worktrees have .git as a file

    def test_create_worktree_for_feature_branch(self, real_managers, local_git_repo):
        """Test creating a worktree for a feature branch."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone first
        repo_manager.clone_repo("test", "repo", remote_url)

        # Create worktree for feature branch
        worktree = worktree_manager.create_worktree("test", "repo", "feature/test")

        assert worktree is not None
        assert worktree.branch == "feature/test"
        assert worktree.local_path.exists()

        # Verify the feature file exists (was on feature branch)
        feature_file = worktree.local_path / "feature.txt"
        assert feature_file.exists()

    def test_create_worktree_path_structure(self, real_managers, local_git_repo):
        """Test that worktree is created in correct location."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone first
        repo_manager.clone_repo("test", "repo", remote_url)

        # Create worktree
        worktree = worktree_manager.create_worktree("test", "repo", "main")

        # Verify path structure: repos/owner/repo/.worktrees/branch
        base_repo_path = repo_manager.get_repo_path("test", "repo")
        expected_path = base_repo_path / ".worktrees" / "main"
        assert worktree.local_path == expected_path

    def test_create_worktree_idempotent(self, real_managers, local_git_repo):
        """Test that creating same worktree twice returns existing."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone first
        repo_manager.clone_repo("test", "repo", remote_url)

        # Create worktree twice
        worktree1 = worktree_manager.create_worktree("test", "repo", "main")
        worktree2 = worktree_manager.create_worktree("test", "repo", "main")

        assert worktree1.branch == worktree2.branch
        assert worktree1.local_path == worktree2.local_path


@pytest.mark.integration
class TestWorktreeGitOperations:
    """Tests for git operations within worktrees."""

    def test_git_status_works_in_worktree(self, real_managers, local_git_repo):
        """Verify git status works in created worktree."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "main")

        # Run git status
        result = subprocess.run(
            ["git", "status"],
            cwd=worktree.local_path,
            capture_output=True,
            text=True,
            check=False,
        )

        assert result.returncode == 0
        assert "On branch main" in result.stdout

    def test_git_log_works_in_worktree(self, real_managers, local_git_repo):
        """Verify git log works in created worktree."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "main")

        # Run git log
        result = subprocess.run(
            ["git", "log", "--oneline", "-5"],
            cwd=worktree.local_path,
            capture_output=True,
            text=True,
            check=False,
        )

        assert result.returncode == 0
        assert "Initial commit" in result.stdout

    def test_git_diff_works_in_worktree(self, real_managers, local_git_repo):
        """Verify git diff works in created worktree."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "main")

        # Make a change
        readme = worktree.local_path / "README.md"
        readme.write_text("Modified content\n")

        # Run git diff
        result = subprocess.run(
            ["git", "diff"],
            cwd=worktree.local_path,
            capture_output=True,
            text=True,
            check=False,
        )

        assert result.returncode == 0
        assert "Modified content" in result.stdout

    def test_git_branch_works_in_worktree(self, real_managers, local_git_repo):
        """Verify git branch works in created worktree."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "main")

        # Run git branch
        result = subprocess.run(
            ["git", "branch", "-a"],
            cwd=worktree.local_path,
            capture_output=True,
            text=True,
            check=False,
        )

        assert result.returncode == 0
        assert "main" in result.stdout


@pytest.mark.integration
class TestWorktreeRemoval:
    """Tests for worktree removal."""

    def test_remove_worktree(self, real_managers, local_git_repo):
        """Test removing a worktree."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "main")
        worktree_path = worktree.local_path

        # Remove worktree
        worktree_manager.remove_worktree("test", "repo", "main")

        # Verify worktree is gone
        assert not worktree_path.exists()
        assert not worktree_manager.worktree_exists("test", "repo", "main")

    def test_remove_nonexistent_worktree(self, real_managers, local_git_repo):
        """Test removing a worktree that doesn't exist."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone but don't create worktree
        repo_manager.clone_repo("test", "repo", remote_url)

        # Removing non-existent worktree should not raise
        worktree_manager.remove_worktree("test", "repo", "main")


@pytest.mark.integration
class TestWorktreeEnsure:
    """Tests for ensure_worktree."""

    def test_ensure_creates_if_not_exists(self, real_managers, local_git_repo):
        """Test ensure_worktree creates worktree if it doesn't exist."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone first
        repo_manager.clone_repo("test", "repo", remote_url)

        # Ensure worktree (should create)
        worktree = worktree_manager.ensure_worktree("test", "repo", "main")

        assert worktree is not None
        assert worktree.local_path.exists()

    def test_ensure_returns_existing(self, real_managers, local_git_repo):
        """Test ensure_worktree returns existing worktree."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree1 = worktree_manager.create_worktree("test", "repo", "main")

        # Add a marker file
        marker = worktree1.local_path / "marker.txt"
        marker.write_text("marker")

        # Ensure worktree (should return existing)
        worktree2 = worktree_manager.ensure_worktree("test", "repo", "main")

        assert worktree2 is not None
        assert marker.exists()  # Same directory


@pytest.mark.integration
class TestMultipleWorktrees:
    """Tests for multiple worktrees from same repo."""

    def test_create_multiple_worktrees(self, real_managers, local_git_repo):
        """Test creating worktrees for multiple branches."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone first
        repo_manager.clone_repo("test", "repo", remote_url)

        # Create worktrees for both branches
        wt_main = worktree_manager.create_worktree("test", "repo", "main")
        wt_feature = worktree_manager.create_worktree("test", "repo", "feature/test")

        # Both should exist independently
        assert wt_main.local_path.exists()
        assert wt_feature.local_path.exists()
        assert wt_main.local_path != wt_feature.local_path

        # Feature branch has feature.txt, main doesn't
        assert (wt_feature.local_path / "feature.txt").exists()
        assert not (wt_main.local_path / "feature.txt").exists()

    def test_list_worktrees(self, real_managers, local_git_repo):
        """Test listing all worktrees for a repository."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktrees
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree_manager.create_worktree("test", "repo", "main")
        worktree_manager.create_worktree("test", "repo", "feature/test")

        # List worktrees
        worktrees = worktree_manager.list_worktrees("test", "repo")

        assert len(worktrees) == 2
        branches = {wt.branch for wt in worktrees}
        assert "main" in branches
        assert "feature/test" in branches
