"""Worktree manager for worktree backend."""

import logging
import re
import subprocess
from datetime import datetime
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
    """Manages git worktrees."""

    def __init__(
        self,
        worktrees_dir: Path,
        repo_manager: RepositoryManager,
        storage: Optional[MetadataStorage] = None,
    ):
        """Initialize worktree manager."""
        self.worktrees_dir = worktrees_dir
        self.worktrees_dir.mkdir(parents=True, exist_ok=True)
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
            # Check if branch exists remotely
            remote_branch_exists = self._remote_branch_exists(base_repo_path, branch)

            if remote_branch_exists:
                # Create worktree tracking remote branch
                result = subprocess.run(
                    ["git", "worktree", "add", str(worktree_path), f"origin/{branch}"],
                    cwd=base_repo_path,
                    capture_output=True,
                    text=True,
                    check=True,
                )
            else:
                # Create worktree with new branch
                result = subprocess.run(
                    ["git", "worktree", "add", "-b", branch, str(worktree_path)],
                    cwd=base_repo_path,
                    capture_output=True,
                    text=True,
                    check=True,
                )

                # Set upstream to track origin
                subprocess.run(
                    ["git", "branch", f"--set-upstream-to=origin/{branch}", branch],
                    cwd=worktree_path,
                    capture_output=True,
                    text=True,
                    check=False,  # May fail if remote branch doesn't exist yet
                )

            logger.debug(f"Worktree creation output: {result.stdout}")

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

    def _generate_workspace_id(self, owner: str, repo: str, branch: str) -> str:  # noqa: ARG002  # pylint: disable=unused-argument
        """Generate a workspace ID for a worktree."""
        # Use branch name as workspace ID for simplicity
        # Could be made more sophisticated if needed
        return sanitize_branch_name(branch)

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
