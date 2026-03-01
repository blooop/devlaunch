"""Workspace clone manager for DevLaunch.

Orchestrates: bare repo caching → workspace clone → branch checkout.
Clones from the local bare reference repo (fast, saves bandwidth),
then fixes the remote URL so push/pull work against GitHub.

Directory layout:
    repos/<owner>/<repo>/
    ├── .bare/           # bare git repo (hidden)
    ├── main/            # workspace clone
    └── nb4/             # workspace clone
"""

import logging
import re
import shutil
import subprocess
from pathlib import Path
from typing import Optional

from .branch_manager import BranchManager
from .config import WorktreeConfig, get_worktree_config
from .models import WorktreeInfo
from .repo_manager import RepositoryManager
from .storage import MetadataStorage

logger = logging.getLogger(__name__)


def _sanitize_branch_dir(branch: str) -> str:
    """Sanitize a branch name for use as a directory name.

    Converts e.g. 'feature/my-branch' → 'feature-my-branch'.
    """
    return re.sub(r"[^\w.-]", "-", branch).strip("-")


class WorkspaceCloneManager:
    """Manages local workspace clones for DevPod.

    Directory layout:
        ~/.cache/devlaunch/repos/<owner>/<repo>/
        ├── .bare/                        # bare git repo
        ├── main/                         # workspace clone
        └── nb4/                          # workspace clone
    """

    def __init__(
        self,
        config: Optional[WorktreeConfig] = None,
        repo_manager: Optional[RepositoryManager] = None,
        storage: Optional[MetadataStorage] = None,
        branch_manager: Optional[BranchManager] = None,
    ):
        self.config = config or get_worktree_config()
        self.storage = storage or MetadataStorage()

        if repo_manager:
            self.repo_manager = repo_manager
        else:
            self.repo_manager = RepositoryManager(
                repos_dir=Path(self.config.repos_dir),
                storage=self.storage,
                config=self.config,
            )

        self.branch_manager = branch_manager or BranchManager()

    def get_workspace_path(self, owner: str, repo: str, branch: str) -> Path:
        """Get the path for a workspace clone."""
        repo_root = self.repo_manager.get_repo_path(owner, repo)
        return repo_root / _sanitize_branch_dir(branch)

    def workspace_exists(self, owner: str, repo: str, branch: str) -> bool:
        """Check if a workspace clone exists."""
        ws_path = self.get_workspace_path(owner, repo, branch)
        return ws_path.exists() and (ws_path / ".git").exists()

    def ensure_branch(self, owner: str, repo: str, branch: str) -> None:
        """Ensure a branch exists in the bare repo.

        Fetches latest refs, then uses BranchManager to create the branch
        locally if needed. Does not push to the remote.
        """
        bare_path = self.repo_manager.get_bare_path(owner, repo)
        # Fetch first to have latest refs
        try:
            self.repo_manager.fetch_repo(owner, repo)
        except Exception as e:
            logger.warning(f"Failed to fetch before branch ensure: {e}")

        default_branch = self.repo_manager.get_default_branch(owner, repo)
        self.branch_manager.ensure_branch_exists(
            bare_path, branch, create_remote=False, start_point=default_branch
        )

    def ensure_workspace(
        self,
        owner: str,
        repo: str,
        branch: str,
        remote_url: str,
        workspace_id: str,
    ) -> Path:
        """Ensure a workspace clone exists and is on the right branch.

        1. Ensure the bare reference repo is cloned/fetched
        2. Clone from bare repo to workspace path (if not already there)
        3. Fix remote URL to point to GitHub (not the bare repo)
        4. Fetch from origin to get all remote branches
        5. Checkout the requested branch
        6. Track workspace in metadata for deletion

        Returns the workspace path.
        """
        # Step 1: Ensure bare reference repo exists
        self.repo_manager.ensure_repo(owner, repo, remote_url)
        bare_repo_path = self.repo_manager.get_bare_path(owner, repo)

        ws_path = self.get_workspace_path(owner, repo, branch)

        if not self.workspace_exists(owner, repo, branch):
            # Step 2: Clone from bare repo
            logger.info(f"Creating workspace clone at {ws_path}")
            ws_path.parent.mkdir(parents=True, exist_ok=True)

            try:
                subprocess.run(
                    ["git", "clone", str(bare_repo_path), str(ws_path)],
                    capture_output=True,
                    text=True,
                    check=True,
                )
            except subprocess.CalledProcessError as e:
                logger.error(f"Failed to clone workspace: {e.stderr}")
                if ws_path.exists():
                    shutil.rmtree(ws_path)
                raise RuntimeError(f"Failed to clone workspace: {e.stderr}") from e

            # Step 3: Fix remote URL to point to GitHub
            try:
                subprocess.run(
                    ["git", "remote", "set-url", "origin", remote_url],
                    cwd=ws_path,
                    capture_output=True,
                    text=True,
                    check=True,
                )
            except subprocess.CalledProcessError as e:
                logger.error(f"Failed to set remote URL: {e.stderr}")
                raise RuntimeError(f"Failed to set remote URL: {e.stderr}") from e

        # Step 4: Fetch from origin to get all remote branches
        try:
            subprocess.run(
                ["git", "fetch", "origin"],
                cwd=ws_path,
                capture_output=True,
                text=True,
                check=True,
            )
        except subprocess.CalledProcessError as e:
            logger.warning(f"Failed to fetch in workspace: {e.stderr}")

        # Step 5: Checkout branch
        try:
            subprocess.run(
                ["git", "checkout", branch],
                cwd=ws_path,
                capture_output=True,
                text=True,
                check=True,
            )
        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to checkout branch '{branch}': {e.stderr}")
            raise RuntimeError(f"Failed to checkout branch '{branch}': {e.stderr}") from e

        # Step 6: Track in metadata
        try:
            wt_info = WorktreeInfo(
                owner=owner,
                repo=repo,
                branch=branch,
                local_path=ws_path,
                workspace_id=workspace_id,
            )
            self.storage.add_worktree(wt_info)
        except Exception as e:
            logger.warning(f"Failed to save workspace metadata: {e}")

        return ws_path

    def remove_workspace(self, owner: str, repo: str, branch: str) -> bool:
        """Remove a workspace clone.

        Returns True if removed, False if it didn't exist.
        """
        ws_path = self.get_workspace_path(owner, repo, branch)
        if ws_path.exists():
            shutil.rmtree(ws_path)
            logger.info(f"Removed workspace clone: {ws_path}")
            # Clean up metadata
            try:
                self.storage.remove_worktree(owner, repo, branch)
            except Exception as e:
                logger.warning(f"Failed to remove workspace metadata: {e}")
            return True
        return False

    def remove_workspace_by_id(self, workspace_id: str) -> bool:
        """Remove a workspace clone by its workspace ID.

        Looks up the workspace in metadata to find owner/repo/branch,
        then removes the clone directory.

        Returns True if removed, False if not found.
        """
        wt_info = self.storage.get_worktree_by_workspace_id(workspace_id)
        if wt_info:
            return self.remove_workspace(wt_info.owner, wt_info.repo, wt_info.branch)
        return False
