"""Workspace manager for DevPod integration with worktrees."""

import fcntl
import logging
import subprocess
from pathlib import Path
from typing import List, Optional, Tuple

from .models import WorktreeInfo
from .storage import MetadataStorage
from .worktree_manager import WorktreeManager, sanitize_branch_name

logger = logging.getLogger(__name__)


def run_devpod(args: List[str], capture: bool = False) -> subprocess.CompletedProcess:
    """Run a devpod command.

    Args:
        args: Arguments to pass to devpod (not including 'devpod' itself)
        capture: Whether to capture stdout/stderr (hides output if True)

    Returns:
        CompletedProcess result
    """
    cmd = ["devpod"] + args
    logging.debug("Running: %s", " ".join(cmd))
    if capture:
        return subprocess.run(cmd, capture_output=True, text=True, check=False)
    return subprocess.run(cmd, check=False)


class WorkspaceManager:
    """Manages DevPod workspaces backed by worktrees."""

    def __init__(
        self, worktree_manager: WorktreeManager, storage: Optional[MetadataStorage] = None
    ):
        """Initialize workspace manager."""
        self.worktree_manager = worktree_manager
        self.storage = storage or MetadataStorage()

    def create_workspace(
        self,
        owner: str,
        repo: str,
        branch: str,
        workspace_id: Optional[str] = None,
        remote_url: Optional[str] = None,
        devcontainer_path: Optional[str] = None,
        ide: Optional[str] = None,
    ) -> Tuple[WorktreeInfo, str]:
        """Create a workspace from a worktree.

        The worktree is created inside the base repo directory (.worktrees/branch)
        and DevPod mounts the base repo so git commands work inside the container.

        Returns:
            Tuple of (WorktreeInfo, devpod_output)
        """
        # Acquire lock to prevent race conditions with parallel operations
        lock_dir = Path.home() / ".devlaunch" / "locks"
        lock_dir.mkdir(parents=True, exist_ok=True)
        lock_file = lock_dir / f"{owner}-{repo}.lock"

        with open(lock_file, "w", encoding="utf-8") as f:
            fcntl.flock(f, fcntl.LOCK_EX)
            try:
                return self._create_workspace_locked(
                    owner, repo, branch, workspace_id, remote_url, devcontainer_path, ide
                )
            finally:
                fcntl.flock(f, fcntl.LOCK_UN)

    def _create_workspace_locked(
        self,
        owner: str,
        repo: str,
        branch: str,
        workspace_id: Optional[str] = None,
        remote_url: Optional[str] = None,
        devcontainer_path: Optional[str] = None,
        ide: Optional[str] = None,
    ) -> Tuple[WorktreeInfo, str]:
        """Internal method to create workspace (called while holding lock)."""
        # Ensure worktree exists
        worktree = self.worktree_manager.ensure_worktree(owner, repo, branch, remote_url)

        # Always use sanitized branch name as workspace ID for consistency
        if not workspace_id:
            workspace_id = sanitize_branch_name(branch)

        logger.info(
            f"Creating DevPod workspace {workspace_id} from worktree at {worktree.local_path}"
        )

        # Mount the worktree directory directly - it has a .git file that links
        # to the main repo, so git commands work inside the container
        args = ["up", str(worktree.local_path), "--id", workspace_id]

        if devcontainer_path:
            args.extend(["--devcontainer-path", devcontainer_path])

        if ide:
            args.extend(["--ide", ide])

        # Run DevPod command - don't capture output so user sees the build
        result = run_devpod(args, capture=False)

        if result.returncode != 0:
            raise RuntimeError(f"DevPod failed with exit code {result.returncode}")

        # Update worktree metadata with DevPod workspace ID
        worktree.devpod_workspace_id = workspace_id
        self.storage.add_worktree(worktree)

        logger.info(f"Successfully created workspace {workspace_id}")
        return worktree, ""

    def start_workspace(self, workspace_id: str) -> str:
        """Start an existing workspace.

        Returns:
            DevPod command output
        """
        logger.info(f"Starting workspace {workspace_id}")

        result = run_devpod(["up", workspace_id], capture=False)
        if result.returncode != 0:
            raise RuntimeError(f"Failed to start workspace (exit code {result.returncode})")
        logger.info(f"Successfully started workspace {workspace_id}")
        return ""

    def stop_workspace(self, workspace_id: str) -> str:
        """Stop a workspace.

        Returns:
            DevPod command output
        """
        logger.info(f"Stopping workspace {workspace_id}")

        result = run_devpod(["stop", workspace_id], capture=True)
        if result.returncode != 0:
            logger.error(f"Failed to stop workspace: {result.stderr}")
            raise RuntimeError(f"Failed to stop workspace: {result.stderr}")
        logger.info(f"Successfully stopped workspace {workspace_id}")
        return result.stdout or ""

    def delete_workspace(self, workspace_id: str, remove_worktree: bool = False) -> str:
        """Delete a DevPod workspace and optionally remove the worktree.

        Returns:
            DevPod command output
        """
        logger.info(f"Deleting workspace {workspace_id} (remove_worktree={remove_worktree})")

        # Find associated worktree if we need to remove it
        worktree_to_remove = None
        if remove_worktree:
            for worktree in self.storage.list_worktrees():
                if workspace_id in (worktree.devpod_workspace_id, worktree.workspace_id):
                    worktree_to_remove = worktree
                    break

        # Delete DevPod workspace
        result = run_devpod(["delete", workspace_id], capture=True)
        if result.returncode != 0:
            logger.error(f"Failed to delete workspace: {result.stderr}")
        else:
            logger.info(f"Successfully deleted DevPod workspace {workspace_id}")

        # Remove worktree if requested
        if remove_worktree and worktree_to_remove:
            try:
                self.worktree_manager.remove_worktree(
                    worktree_to_remove.owner, worktree_to_remove.repo, worktree_to_remove.branch
                )
                logger.info(f"Removed associated worktree for {worktree_to_remove.branch}")
            except Exception as e:
                logger.error(f"Failed to remove worktree: {e}")

        return result.stdout if result.stdout else ""

    def list_workspaces(self) -> List[dict]:
        """List all workspaces, including worktree information.

        Returns:
            List of workspace dictionaries with worktree info added
        """
        import json

        # Get DevPod workspaces
        result = run_devpod(["list", "--output", "json"], capture=True)
        if result.returncode != 0 or not result.stdout:
            devpod_workspaces = []
        else:
            try:
                devpod_workspaces = json.loads(result.stdout)
            except json.JSONDecodeError as e:
                logger.error(f"Failed to parse DevPod workspace list: {e}")
                devpod_workspaces = []

        # Enhance with worktree information
        worktrees_by_id = {}
        for worktree in self.storage.list_worktrees():
            if worktree.devpod_workspace_id:
                worktrees_by_id[worktree.devpod_workspace_id] = worktree
            worktrees_by_id[worktree.workspace_id] = worktree

        for workspace in devpod_workspaces:
            workspace_id = workspace.get("id", "")
            if workspace_id in worktrees_by_id:
                worktree = worktrees_by_id[workspace_id]
                workspace["worktree"] = {
                    "owner": worktree.owner,
                    "repo": worktree.repo,
                    "branch": worktree.branch,
                    "path": str(worktree.local_path),
                    "created_at": worktree.created_at.isoformat(),
                    "last_used": worktree.last_used.isoformat(),
                }
                workspace["backend"] = "worktree"
            else:
                workspace["backend"] = "devpod"

        return devpod_workspaces

    def get_workspace_info(self, workspace_id: str) -> Optional[dict]:
        """Get information about a specific workspace.

        Returns:
            Workspace dictionary with worktree info if available
        """
        workspaces = self.list_workspaces()
        for workspace in workspaces:
            if workspace.get("id") == workspace_id:
                return workspace
        return None

    def workspace_from_worktree(self, owner: str, repo: str, branch: str) -> Optional[str]:
        """Get DevPod workspace ID for a worktree.

        Returns:
            DevPod workspace ID if exists, None otherwise
        """
        worktree = self.worktree_manager.get_worktree(owner, repo, branch)
        if worktree:
            return worktree.devpod_workspace_id or worktree.workspace_id
        return None

    def sync_workspaces(self) -> None:
        """Sync worktree metadata with actual DevPod workspaces."""
        # Get all DevPod workspaces
        workspaces = self.list_workspaces()
        devpod_ids = {ws.get("id") for ws in workspaces}

        # Check all worktrees
        for worktree in self.storage.list_worktrees():
            if worktree.devpod_workspace_id and worktree.devpod_workspace_id not in devpod_ids:
                # DevPod workspace no longer exists
                logger.info(f"Clearing DevPod workspace ID for worktree {worktree.branch}")
                worktree.devpod_workspace_id = None
                self.storage.add_worktree(worktree)
