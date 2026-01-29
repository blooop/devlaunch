"""Integration tests for worktree path handling.

These tests verify that worktree paths are correctly set up for container
mounting, specifically testing the relative path fixups that make git
work correctly inside DevPod containers.
"""

import subprocess

import pytest


@pytest.mark.integration
class TestWorktreeGitFile:
    """Tests for worktree .git file content and path handling."""

    def test_worktree_git_file_exists(self, real_managers, local_git_repo):
        """Test that worktree has a .git file (not directory)."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "main")

        git_path = worktree.local_path / ".git"
        assert git_path.exists()
        assert git_path.is_file(), ".git should be a file in worktrees, not a directory"

    def test_worktree_git_file_uses_relative_path(self, real_managers, local_git_repo):
        """Verify .git file uses relative paths for container portability.

        This is critical: absolute paths break when the worktree is mounted
        in a container because the absolute path doesn't exist inside the container.
        """
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "main")

        # Read .git file content
        git_file = worktree.local_path / ".git"
        content = git_file.read_text().strip()

        assert content.startswith("gitdir:"), f"Expected 'gitdir:' prefix, got: {content}"

        # Extract the gitdir path
        gitdir_path = content.replace("gitdir:", "").strip()

        # CRITICAL: Path must be relative, not absolute
        assert not gitdir_path.startswith("/"), (
            f"gitdir path must be relative for container mounting, "
            f"but got absolute path: {gitdir_path}"
        )

        # Should point to parent directories using ../
        assert "../" in gitdir_path, f"Expected relative path with ../, got: {gitdir_path}"

    def test_worktree_git_file_relative_path_resolves(self, real_managers, local_git_repo):
        """Test that the relative path actually resolves to valid git metadata."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "main")

        # Read .git file and resolve path
        git_file = worktree.local_path / ".git"
        content = git_file.read_text().strip()
        gitdir_path = content.replace("gitdir:", "").strip()

        # Resolve the relative path
        resolved = (worktree.local_path / gitdir_path).resolve()

        # The resolved path should exist and contain git worktree metadata
        assert resolved.exists(), f"Resolved gitdir path doesn't exist: {resolved}"
        assert (resolved / "HEAD").exists(), f"Missing HEAD in gitdir: {resolved}"


@pytest.mark.integration
class TestWorktreeGitdirFile:
    """Tests for the gitdir file in base repo's worktrees/ directory."""

    def test_gitdir_file_uses_relative_path(self, real_managers, local_git_repo):
        """Verify gitdir file also uses relative paths."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        _worktree = worktree_manager.create_worktree("test", "repo", "main")  # noqa: F841

        # Find the gitdir file in base repo
        base_repo = repo_manager.get_repo_path("test", "repo")
        # For bare repos, worktree metadata is in <repo>/worktrees/<name>/
        gitdir_file = base_repo / "worktrees" / "main" / "gitdir"

        if gitdir_file.exists():
            content = gitdir_file.read_text().strip()

            # Should be relative path
            assert not content.startswith("/"), (
                f"gitdir file should use relative path, got: {content}"
            )

            # Should point to .worktrees directory
            assert ".worktrees" in content, f"gitdir should point to .worktrees, got: {content}"

    def test_gitdir_file_resolves_to_worktree(self, real_managers, local_git_repo):
        """Test that gitdir file resolves to the actual worktree."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "main")

        # Find the gitdir file
        base_repo = repo_manager.get_repo_path("test", "repo")
        worktree_meta_dir = base_repo / "worktrees" / "main"
        gitdir_file = worktree_meta_dir / "gitdir"

        if gitdir_file.exists():
            content = gitdir_file.read_text().strip()
            resolved = (worktree_meta_dir / content).resolve()

            # Should resolve to the worktree directory
            assert resolved == worktree.local_path.resolve(), (
                f"gitdir should resolve to worktree path. "
                f"Expected: {worktree.local_path.resolve()}, got: {resolved}"
            )


@pytest.mark.integration
class TestWorktreeContainerSimulation:
    """Tests simulating container mounting scenarios."""

    def test_git_works_after_path_change(self, real_managers, local_git_repo, tmp_path):
        """Simulate container mount by copying worktree to different path.

        This test verifies that git commands work even when the worktree
        is at a different absolute path (as would happen in a container).
        """
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        _worktree = worktree_manager.create_worktree("test", "repo", "main")  # noqa: F841

        # Get the base repo path (we need to copy the whole repo, not just worktree)
        base_repo = repo_manager.get_repo_path("test", "repo")

        # Copy entire repo (including .worktrees and worktrees metadata) to new location
        import shutil

        new_base = tmp_path / "mounted_repo"
        shutil.copytree(base_repo, new_base)

        # The worktree in the new location
        new_worktree_path = new_base / ".worktrees" / "main"

        # Verify git status works at the new location
        result = subprocess.run(
            ["git", "status"],
            cwd=new_worktree_path,
            capture_output=True,
            text=True,
            check=False,
        )

        assert result.returncode == 0, (
            f"git status failed at new path. This indicates relative paths aren't working. "
            f"stderr: {result.stderr}"
        )
        assert "On branch main" in result.stdout

    def test_git_log_works_after_path_change(self, real_managers, local_git_repo, tmp_path):
        """Verify git log works after simulated container mount."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree_manager.create_worktree("test", "repo", "main")

        # Copy to new location
        import shutil

        base_repo = repo_manager.get_repo_path("test", "repo")
        new_base = tmp_path / "mounted_repo"
        shutil.copytree(base_repo, new_base)
        new_worktree_path = new_base / ".worktrees" / "main"

        # Verify git log works
        result = subprocess.run(
            ["git", "log", "--oneline", "-1"],
            cwd=new_worktree_path,
            capture_output=True,
            text=True,
            check=False,
        )

        assert result.returncode == 0, f"git log failed: {result.stderr}"


@pytest.mark.integration
class TestWorktreePathSanitization:
    """Tests for branch name sanitization in paths."""

    def test_slash_in_branch_name(self, real_managers, local_git_repo):
        """Test that branches with slashes have sanitized paths."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree for feature/test branch
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "feature/test")

        # Path should not contain slashes (except directory separators)
        path_str = str(worktree.local_path)
        assert "feature/test" not in path_str, "Branch name with slash should be sanitized in path"
        # Should be sanitized to feature-test
        assert "feature-test" in path_str

    def test_git_file_correct_for_sanitized_branch(self, real_managers, local_git_repo):
        """Test .git file is correct even with sanitized branch names."""
        repo_manager = real_managers["repo_manager"]
        worktree_manager = real_managers["worktree_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone and create worktree
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree = worktree_manager.create_worktree("test", "repo", "feature/test")

        # Git commands should still work
        result = subprocess.run(
            ["git", "status"],
            cwd=worktree.local_path,
            capture_output=True,
            text=True,
            check=False,
        )

        assert result.returncode == 0
        # Should show we're on the feature/test branch (actual git branch name)
        assert "feature/test" in result.stdout
