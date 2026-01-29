"""Integration tests for RepositoryManager with real git operations.

These tests run real git commands against temporary local repositories.
They verify that git command construction, cloning, and fetching work correctly.
"""

import subprocess

import pytest


@pytest.mark.integration
class TestRepoManagerRealClone:
    """Tests for real git clone operations."""

    def test_clone_from_local_remote(self, real_managers, local_git_repo):
        """Test cloning a repository from a local 'remote'."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone the repository
        result = repo_manager.clone_repo("test", "repo", remote_url)

        assert result is not None
        assert result.owner == "test"
        assert result.repo == "repo"
        assert result.remote_url == remote_url

        # Verify the clone is a bare repo
        repo_path = repo_manager.get_repo_path("test", "repo")
        assert repo_path.exists()
        # Bare repos have HEAD directly in the repo directory
        assert (repo_path / "HEAD").exists()
        # Bare repos don't have a .git subdirectory
        assert not (repo_path / ".git").exists()

    def test_clone_preserves_branches(self, real_managers, local_git_repo):
        """Test that cloning preserves all branches from remote."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone the repository
        repo_manager.clone_repo("test", "repo", remote_url)
        repo_path = repo_manager.get_repo_path("test", "repo")

        # List branches in the bare repo
        result = subprocess.run(
            ["git", "branch", "-a"],
            cwd=repo_path,
            capture_output=True,
            text=True,
            check=True,
        )

        # Should have main and feature/test
        assert "main" in result.stdout
        assert "feature/test" in result.stdout or "feature-test" in result.stdout

    def test_clone_idempotent(self, real_managers, local_git_repo):
        """Test that cloning the same repo twice returns existing repo."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone twice
        result1 = repo_manager.clone_repo("test", "repo", remote_url)
        result2 = repo_manager.clone_repo("test", "repo", remote_url)

        assert result1 is not None
        assert result2 is not None
        assert result1.owner == result2.owner
        assert result1.repo == result2.repo

    def test_clone_invalid_url_fails(self, real_managers):
        """Test that cloning from invalid URL raises error."""
        repo_manager = real_managers["repo_manager"]

        with pytest.raises(RuntimeError, match="Failed to clone"):
            repo_manager.clone_repo("test", "repo", "/nonexistent/path.git")


@pytest.mark.integration
class TestRepoManagerRealFetch:
    """Tests for real git fetch operations."""

    def test_fetch_after_clone(self, real_managers, local_git_repo):
        """Test fetching updates after initial clone."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]
        work_dir = local_git_repo["work_dir"]

        # Clone the repository
        repo_manager.clone_repo("test", "repo", remote_url)
        repo_path = repo_manager.get_repo_path("test", "repo")

        # Get initial commit count
        before_result = subprocess.run(
            ["git", "rev-list", "--count", "HEAD"],
            cwd=repo_path,
            capture_output=True,
            text=True,
            check=True,
        )
        count_before = int(before_result.stdout.strip())

        # Make a new commit in the remote working copy
        new_file = work_dir / "new_file.txt"
        new_file.write_text("new content")
        subprocess.run(["git", "add", "new_file.txt"], cwd=work_dir, check=True)
        subprocess.run(
            ["git", "commit", "-m", "Add new file"],
            cwd=work_dir,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "push", "origin", "main"],
            cwd=work_dir,
            check=True,
            capture_output=True,
        )

        # For bare repos cloned with --bare, we need to fetch and update the local branch
        # The remote is configured as origin, so fetch will update origin/* refs
        # But the local heads need to be updated too

        # First, verify fetch completes without error
        repo_manager.fetch_repo("test", "repo")

        # After fetch, the new commit should be reachable
        # Check that we can see the new commit via rev-list
        after_result = subprocess.run(
            ["git", "rev-list", "--count", "--all"],
            cwd=repo_path,
            capture_output=True,
            text=True,
            check=True,
        )
        count_after = int(after_result.stdout.strip())

        # Should have at least one more commit after fetch
        assert count_after > count_before, (
            f"Expected more commits after fetch. Before: {count_before}, After: {count_after}"
        )

    def test_fetch_nonexistent_repo_fails(self, real_managers):
        """Test that fetching non-existent repo raises error."""
        repo_manager = real_managers["repo_manager"]

        with pytest.raises(ValueError, match="does not exist"):
            repo_manager.fetch_repo("nonexistent", "repo")


@pytest.mark.integration
class TestRepoManagerEnsure:
    """Tests for ensure_repo which combines clone and fetch."""

    def test_ensure_clones_if_not_exists(self, real_managers, local_git_repo):
        """Test ensure_repo clones if repo doesn't exist."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Ensure repo (should clone)
        result = repo_manager.ensure_repo("test", "repo", remote_url)

        assert result is not None
        assert result.owner == "test"
        assert repo_manager.repo_exists("test", "repo")

    def test_ensure_returns_existing(self, real_managers, local_git_repo):
        """Test ensure_repo returns existing repo without re-cloning."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone first
        repo_manager.clone_repo("test", "repo", remote_url)
        repo_path = repo_manager.get_repo_path("test", "repo")

        # Create a marker file to verify it's the same directory
        marker = repo_path / "marker.txt"
        marker.write_text("marker")

        # Ensure repo (should return existing, not re-clone)
        result = repo_manager.ensure_repo("test", "repo", remote_url, auto_fetch=False)

        assert result is not None
        assert marker.exists()  # Directory wasn't replaced


@pytest.mark.integration
class TestRepoManagerDefaultBranch:
    """Tests for default branch detection."""

    def test_detects_main_as_default(self, real_managers, local_git_repo):
        """Test that main is detected as default branch."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        result = repo_manager.clone_repo("test", "repo", remote_url)

        assert result.default_branch == "main"

    def test_detects_default_from_bare_repo(self, real_managers, local_git_repo):
        """Test default branch detection works for bare repos."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone
        repo_manager.clone_repo("test", "repo", remote_url)
        repo_path = repo_manager.get_repo_path("test", "repo")

        # Verify HEAD points to main
        result = subprocess.run(
            ["git", "symbolic-ref", "HEAD"],
            cwd=repo_path,
            capture_output=True,
            text=True,
            check=True,
        )
        assert "main" in result.stdout
