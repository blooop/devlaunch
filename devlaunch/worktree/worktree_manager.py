"""Worktree manager for worktree backend.

This module manages git worktrees for the devlaunch worktree backend.

Directory Structure
-------------------
~/.cache/devlaunch/
├── repos/
│   └── owner/
│       └── repo/              # Base repository (cloned once)
│           ├── .git/          # Git directory with all objects
│           │   └── worktrees/ # Git worktree metadata
│           │       └── main/  # Metadata for 'main' worktree
│           └── .worktrees/    # Actual worktree directories
│               └── main/      # Working directory for 'main' branch
│                   └── .git   # FILE (not dir) with gitdir pointer
└── metadata.json              # Devlaunch metadata

Why Worktrees Inside .worktrees/?
---------------------------------
Worktrees are created INSIDE the base repo under .worktrees/ rather than
in a separate directory. This ensures that when we mount the base repo into
a container, both the .git directory and worktrees are accessible together.

Relative Git Paths
------------------
By default, git worktrees use absolute paths in the .git file:
    gitdir: /home/user/.cache/devlaunch/repos/owner/repo/.git/worktrees/main

This breaks when mounted in a container. We fix this by converting to relative:
    gitdir: ../../.git/worktrees/main

This allows the worktree to work regardless of where it's mounted.
"""

import logging
import re
import subprocess
from datetime import datetime, timedelta
from pathlib import Path
from typing import List, Optional

from .models import WorktreeInfo
from .repo_manager import RepositoryManager
from .storage import MetadataStorage

logger = logging.getLogger(__name__)


def sanitize_branch_name(branch: str) -> str:
    """Sanitize branch name for filesystem use."""
    # Replace slashes with hyphens
    sanitized = branch.replace("/", "-")
    # Remove other problematic characters
    sanitized = re.sub(r"[^a-zA-Z0-9\-_.]", "_", sanitized)
    # Remove leading/trailing dots and hyphens
    sanitized = sanitized.strip(".-")
    return sanitized


class WorktreeManager:
    """Manages git worktrees.

    Worktrees are created inside each repo's .worktrees/ directory:
    - repos/owner/repo/.worktrees/branch-name/
    """

    def __init__(
        self,
        repo_manager: RepositoryManager,
        storage: Optional[MetadataStorage] = None,
    ):
        """Initialize worktree manager."""
        self.repo_manager = repo_manager
        self.storage = storage or MetadataStorage()

    def get_worktree_path(self, owner: str, repo: str, branch: str) -> Path:
        """Get local path for a worktree.

        Worktrees are created INSIDE the base repo directory under .worktrees/
        This ensures git commands work inside DevPod containers since the entire
        repo (including .git) is mounted together.
        """
        sanitized_branch = sanitize_branch_name(branch)
        base_repo_path = self.repo_manager.get_repo_path(owner, repo)
        return base_repo_path / ".worktrees" / sanitized_branch

    def create_worktree(
        self, owner: str, repo: str, branch: str, remote_url: Optional[str] = None
    ) -> WorktreeInfo:
        """Create a new git worktree for a branch."""
        # Ensure base repo exists
        if not remote_url:
            base_repo = self.repo_manager.get_repo(owner, repo)
            if not base_repo:
                raise ValueError(f"Repository {owner}/{repo} not found and no remote URL provided")
            remote_url = base_repo.remote_url
        else:
            base_repo = self.repo_manager.ensure_repo(owner, repo, remote_url)

        worktree_path = self.get_worktree_path(owner, repo, branch)

        if worktree_path.exists():
            logger.warning(f"Worktree for {owner}/{repo}@{branch} already exists")
            existing = self.get_worktree(owner, repo, branch)
            if existing:
                return existing
            # If worktree path exists but metadata doesn't, continue to create metadata

        # Create parent directory
        worktree_path.parent.mkdir(parents=True, exist_ok=True)

        base_repo_path = self.repo_manager.get_repo_path(owner, repo)

        logger.info(f"Creating worktree for {owner}/{repo}@{branch} at {worktree_path}")

        try:
            # Check if branch exists in the repo (for bare repos, this is refs/heads/<branch>)
            branch_exists = self._branch_exists(base_repo_path, branch)

            if branch_exists:
                # Branch exists (either from bare clone or previous creation)
                # Create worktree using the existing branch
                result = subprocess.run(
                    ["git", "worktree", "add", str(worktree_path), branch],
                    cwd=base_repo_path,
                    capture_output=True,
                    text=True,
                    check=True,
                )
            else:
                # Branch doesn't exist - create it
                # For bare repos, we need to create from origin's branch or HEAD
                remote_branch_exists = self._remote_branch_exists(base_repo_path, branch)
                if remote_branch_exists:
                    # Fetch the specific branch first
                    subprocess.run(
                        ["git", "fetch", "origin", f"{branch}:{branch}"],
                        cwd=base_repo_path,
                        capture_output=True,
                        text=True,
                        check=False,
                    )
                    result = subprocess.run(
                        ["git", "worktree", "add", str(worktree_path), branch],
                        cwd=base_repo_path,
                        capture_output=True,
                        text=True,
                        check=True,
                    )
                else:
                    # Create new branch from HEAD
                    result = subprocess.run(
                        ["git", "worktree", "add", "-b", branch, str(worktree_path)],
                        cwd=base_repo_path,
                        capture_output=True,
                        text=True,
                        check=True,
                    )

            logger.debug(f"Worktree creation output: {result.stdout}")

            # Fix .git file to use relative paths (required for container mounting)
            self._fix_worktree_paths(worktree_path, base_repo_path)

            # Generate workspace ID
            workspace_id = self._generate_workspace_id(owner, repo, branch)

            # Create worktree metadata
            worktree_info = WorktreeInfo(
                owner=owner,
                repo=repo,
                branch=branch,
                local_path=worktree_path,
                workspace_id=workspace_id,
                created_at=datetime.now(),
                last_used=datetime.now(),
            )

            # Save metadata
            self.storage.add_worktree(worktree_info)

            logger.info(f"Successfully created worktree for {owner}/{repo}@{branch}")
            return worktree_info

        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to create worktree: {e.stderr}")
            # Clean up partial worktree
            if worktree_path.exists():
                import shutil

                shutil.rmtree(worktree_path)
            raise RuntimeError(f"Failed to create worktree: {e.stderr}") from e

    def remove_worktree(self, owner: str, repo: str, branch: str) -> None:
        """Remove a git worktree."""
        worktree_path = self.get_worktree_path(owner, repo, branch)
        base_repo_path = self.repo_manager.get_repo_path(owner, repo)

        if not worktree_path.exists():
            logger.warning(f"Worktree {worktree_path} does not exist")
            # Still remove metadata
            self.storage.remove_worktree(owner, repo, branch)
            return

        logger.info(f"Removing worktree for {owner}/{repo}@{branch}")

        try:
            # Remove worktree using git
            result = subprocess.run(
                ["git", "worktree", "remove", str(worktree_path), "--force"],
                cwd=base_repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            logger.debug(f"Worktree removal output: {result.stdout}")

            # Prune worktree references
            subprocess.run(
                ["git", "worktree", "prune"],
                cwd=base_repo_path,
                capture_output=True,
                text=True,
                check=True,
            )

        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to remove worktree: {e.stderr}")
            # Force remove directory
            if worktree_path.exists():
                import shutil

                shutil.rmtree(worktree_path)
                logger.info(f"Force removed worktree directory {worktree_path}")

        # Remove metadata
        self.storage.remove_worktree(owner, repo, branch)

        logger.info(f"Successfully removed worktree for {owner}/{repo}@{branch}")

    def list_worktrees(self, owner: str, repo: str) -> List[WorktreeInfo]:
        """List all worktrees for a repository."""
        return self.storage.list_worktrees(owner, repo)

    def list_all_worktrees(self) -> List[WorktreeInfo]:
        """List all worktrees."""
        return self.storage.list_worktrees()

    def ensure_worktree(
        self, owner: str, repo: str, branch: str, remote_url: Optional[str] = None
    ) -> WorktreeInfo:
        """Ensure worktree exists, create if needed."""
        if self.worktree_exists(owner, repo, branch):
            worktree = self.get_worktree(owner, repo, branch)
            if worktree:
                # Update last used time
                worktree.last_used = datetime.now()
                self.storage.add_worktree(worktree)
                return worktree

        return self.create_worktree(owner, repo, branch, remote_url)

    def worktree_exists(self, owner: str, repo: str, branch: str) -> bool:
        """Check if worktree exists."""
        worktree_path = self.get_worktree_path(owner, repo, branch)
        return worktree_path.exists() and (worktree_path / ".git").exists()

    def get_worktree(self, owner: str, repo: str, branch: str) -> Optional[WorktreeInfo]:
        """Get worktree metadata."""
        worktree = self.storage.get_worktree(owner, repo, branch)

        if worktree and not self.worktree_exists(owner, repo, branch):
            # Worktree metadata exists but directory doesn't
            logger.warning(
                f"Worktree {owner}/{repo}@{branch} metadata exists but directory missing"
            )
            return None

        return worktree

    def _generate_workspace_id(self, owner: str, repo: str, branch: str) -> str:
        """Generate a workspace ID for a worktree.

        Format: owner-repo-branch (e.g., blooop-bencher-main)
        Truncates if necessary to fit within 50 characters.
        """
        sanitized_branch = sanitize_branch_name(branch)
        base = f"{owner}-{repo}"
        max_len = 50
        available_for_branch = max_len - len(base) - 1  # -1 for separator

        if 0 < available_for_branch < len(sanitized_branch):
            sanitized_branch = sanitized_branch[:available_for_branch]

        return f"{base}-{sanitized_branch}"

    def _is_bare_repo(self, repo_path: Path) -> bool:
        """Check if the repository is a bare repo."""
        # Bare repos have HEAD directly in the repo dir, not in .git/
        return (repo_path / "HEAD").exists() and not (repo_path / ".git").exists()

    def _fix_worktree_paths(self, worktree_path: Path, base_repo_path: Path) -> None:
        """Fix worktree .git file to use relative paths.

        Git worktrees use absolute paths by default, which breaks when the
        worktree is mounted inside a container. This converts them to relative
        paths so git works correctly in any mount location.

        Handles both bare and regular repos.
        """
        try:
            git_file = worktree_path / ".git"
            if not git_file.exists() or git_file.is_dir():
                return

            # Read current .git file content
            content = git_file.read_text().strip()
            if not content.startswith("gitdir:"):
                return

            # Extract the absolute path and worktree name
            abs_gitdir = content.replace("gitdir:", "").strip()
            worktree_name = Path(abs_gitdir).name

            is_bare = self._is_bare_repo(base_repo_path)

            if is_bare:
                # Bare repo: worktree metadata is in <repo>/worktrees/<name>
                # worktree is at: base_repo/.worktrees/<branch>
                # gitdir is at: base_repo/worktrees/<name>
                # Relative path: ../../worktrees/<name>
                rel_gitdir = f"../../worktrees/{worktree_name}"
                gitdir_file = base_repo_path / "worktrees" / worktree_name / "gitdir"
                # Reverse path from base_repo/worktrees/<name> to base_repo/.worktrees/<branch>
                rel_worktree = f"../../.worktrees/{worktree_path.name}"
            else:
                # Regular repo: worktree metadata is in <repo>/.git/worktrees/<name>
                # worktree is at: base_repo/.worktrees/<branch>
                # gitdir is at: base_repo/.git/worktrees/<name>
                # Relative path: ../../.git/worktrees/<name>
                rel_gitdir = f"../../.git/worktrees/{worktree_name}"
                gitdir_file = base_repo_path / ".git" / "worktrees" / worktree_name / "gitdir"
                # Reverse path from base_repo/.git/worktrees/<name> to base_repo/.worktrees/<branch>
                rel_worktree = f"../../../.worktrees/{worktree_path.name}"

            # Write relative path to .git file
            git_file.write_text(f"gitdir: {rel_gitdir}\n")
            logger.debug(f"Fixed worktree .git file to use relative path: {rel_gitdir}")

            # Also fix the reverse pointer in worktrees/<name>/gitdir
            if gitdir_file.exists():
                gitdir_file.write_text(f"{rel_worktree}\n")
                logger.debug(f"Fixed worktrees gitdir to use relative path: {rel_worktree}")

        except (OSError, IOError) as e:
            logger.warning(f"Failed to fix worktree paths: {e}")

    def _branch_exists(self, repo_path: Path, branch: str) -> bool:
        """Check if a branch exists locally (in refs/heads/).

        Works with both bare and regular repos.
        """
        try:
            result = subprocess.run(
                ["git", "show-ref", "--verify", f"refs/heads/{branch}"],
                cwd=repo_path,
                capture_output=True,
                text=True,
                check=False,
            )
            return result.returncode == 0
        except (OSError, subprocess.SubprocessError):
            return False

    def _remote_branch_exists(self, repo_path: Path, branch: str) -> bool:
        """Check if a branch exists on the remote."""
        try:
            result = subprocess.run(
                ["git", "ls-remote", "--heads", "origin", branch],
                cwd=repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            return bool(result.stdout.strip())
        except subprocess.CalledProcessError:
            return False

    def sync_with_git(self, owner: str, repo: str) -> None:
        """Sync worktree metadata with actual git worktrees."""
        base_repo_path = self.repo_manager.get_repo_path(owner, repo)

        if not base_repo_path.exists():
            logger.warning(f"Repository {owner}/{repo} does not exist")
            return

        try:
            # Get actual worktrees from git
            result = subprocess.run(
                ["git", "worktree", "list", "--porcelain"],
                cwd=base_repo_path,
                capture_output=True,
                text=True,
                check=True,
            )

            # Parse worktree list
            current_worktree = None
            git_worktrees = {}

            for line in result.stdout.strip().split("\n"):
                if line.startswith("worktree "):
                    current_worktree = Path(line[9:])
                elif line.startswith("branch ") and current_worktree:
                    branch = line[7:].replace("refs/heads/", "")
                    if current_worktree != base_repo_path:  # Skip main worktree
                        git_worktrees[branch] = current_worktree

            # Update metadata to match git state
            stored_worktrees = self.list_worktrees(owner, repo)

            for worktree in stored_worktrees:
                if worktree.branch not in git_worktrees:
                    # Worktree in metadata but not in git
                    logger.info(f"Removing orphaned worktree metadata for {worktree.branch}")
                    self.storage.remove_worktree(owner, repo, worktree.branch)

        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to sync worktrees: {e.stderr}")

    def prune_stale_worktrees(self, days: Optional[int] = None) -> List[WorktreeInfo]:
        """Prune worktrees that haven't been used in the specified number of days.

        Args:
            days: Number of days after which a worktree is considered stale.
                  Defaults to 30 days.

        Returns:
            List of worktrees that were pruned.
        """
        if days is None:
            days = 30

        cutoff = datetime.now() - timedelta(days=days)
        pruned = []

        for worktree in self.list_all_worktrees():
            if worktree.last_used < cutoff:
                logger.info(
                    f"Pruning stale worktree {worktree.owner}/{worktree.repo}@{worktree.branch} "
                    f"(last used: {worktree.last_used.isoformat()})"
                )
                try:
                    self.remove_worktree(worktree.owner, worktree.repo, worktree.branch)
                    pruned.append(worktree)
                except Exception as e:
                    logger.warning(f"Failed to prune worktree: {e}")

        if pruned:
            logger.info(f"Pruned {len(pruned)} stale worktree(s)")
        else:
            logger.info("No stale worktrees to prune")

        return pruned
