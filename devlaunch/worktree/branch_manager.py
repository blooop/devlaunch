"""Branch management for worktree backend."""

import logging
import subprocess
from pathlib import Path
from typing import List, Optional

logger = logging.getLogger(__name__)


class BranchManager:
    """Manages git branch operations."""

    def ensure_branch_exists(
        self,
        base_repo_path: Path,
        branch: str,
        remote: str = "origin",
        create_remote: bool = True,
        ssh_key_path: Optional[str] = None,
    ) -> None:
        """Ensure branch exists locally and optionally remotely."""
        # Check if branch exists locally
        local_exists = self.local_branch_exists(base_repo_path, branch)

        # Check if branch exists remotely
        remote_exists = self.remote_branch_exists(base_repo_path, branch, remote)

        if local_exists and remote_exists:
            logger.info(f"Branch {branch} already exists locally and remotely")
            return

        if not local_exists and remote_exists:
            # Create local branch tracking remote
            self.create_local_branch(base_repo_path, branch, f"{remote}/{branch}")
            self.track_remote_branch(base_repo_path, branch, remote)
            logger.info(f"Created local branch {branch} tracking {remote}/{branch}")
            return

        if not local_exists:
            # Create new local branch
            self.create_local_branch(base_repo_path, branch)
            logger.info(f"Created local branch {branch}")

        if not remote_exists and create_remote:
            # Push branch to remote
            self.push_branch_to_remote(base_repo_path, branch, remote, ssh_key_path)
            logger.info(f"Pushed branch {branch} to {remote}")

        # Set up tracking
        self.track_remote_branch(base_repo_path, branch, remote)

    def create_local_branch(
        self, base_repo_path: Path, branch: str, start_point: str = "HEAD"
    ) -> None:
        """Create a new local branch."""
        try:
            result = subprocess.run(
                ["git", "branch", branch, start_point],
                cwd=base_repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            logger.debug(f"Branch creation output: {result.stdout}")
        except subprocess.CalledProcessError as e:
            # Branch might already exist
            if "already exists" in e.stderr:
                logger.debug(f"Branch {branch} already exists")
            else:
                logger.error(f"Failed to create branch: {e.stderr}")
                raise RuntimeError(f"Failed to create branch: {e.stderr}") from e

    def track_remote_branch(
        self, base_repo_path: Path, branch: str, remote: str = "origin"
    ) -> None:
        """Set up tracking for a remote branch."""
        try:
            result = subprocess.run(
                ["git", "branch", f"--set-upstream-to={remote}/{branch}", branch],
                cwd=base_repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            logger.debug(f"Branch tracking output: {result.stdout}")
        except subprocess.CalledProcessError as e:
            # Tracking might fail if remote branch doesn't exist yet
            logger.debug(f"Failed to set up tracking (might be expected): {e.stderr}")

    def local_branch_exists(self, base_repo_path: Path, branch: str) -> bool:
        """Check if a branch exists locally."""
        try:
            result = subprocess.run(
                ["git", "show-ref", "--verify", f"refs/heads/{branch}"],
                cwd=base_repo_path,
                capture_output=True,
                text=True,
                check=False,
            )
            return result.returncode == 0
        except Exception:
            return False

    def remote_branch_exists(
        self, base_repo_path: Path, branch: str, remote: str = "origin"
    ) -> bool:
        """Check if a branch exists on the remote."""
        try:
            result = subprocess.run(
                ["git", "ls-remote", "--heads", remote, branch],
                cwd=base_repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            return bool(result.stdout.strip())
        except subprocess.CalledProcessError:
            return False

    def get_remote_branches(self, base_repo_path: Path, remote: str = "origin") -> List[str]:
        """Get list of branches on the remote."""
        try:
            result = subprocess.run(
                ["git", "ls-remote", "--heads", remote],
                cwd=base_repo_path,
                capture_output=True,
                text=True,
                check=True,
            )

            branches = []
            for line in result.stdout.strip().split("\n"):
                if line:
                    # Format: <hash> refs/heads/<branch>
                    parts = line.split("\t")
                    if len(parts) == 2:
                        branch_ref = parts[1]
                        if branch_ref.startswith("refs/heads/"):
                            branches.append(branch_ref[11:])  # Remove refs/heads/

            return branches
        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to get remote branches: {e.stderr}")
            return []

    def push_branch_to_remote(
        self,
        base_repo_path: Path,
        branch: str,
        remote: str = "origin",
        ssh_key_path: Optional[str] = None,
    ) -> None:
        """Push a branch to the remote."""
        env = None
        if ssh_key_path:
            # Set up SSH command with specific key
            ssh_command = f"ssh -i {ssh_key_path} -o IdentitiesOnly=yes"
            env = {"GIT_SSH_COMMAND": ssh_command}

        try:
            result = subprocess.run(
                ["git", "push", "-u", remote, branch],
                cwd=base_repo_path,
                capture_output=True,
                text=True,
                env=env,
                check=True,
            )
            logger.debug(f"Push output: {result.stdout}")
        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to push branch: {e.stderr}")
            raise RuntimeError(f"Failed to push branch to remote: {e.stderr}") from e

    def create_remote_branch_via_ssh(
        self, owner: str, repo: str, branch: str, ssh_key_path: Optional[str] = None
    ) -> bool:
        """Create a remote branch on GitHub via SSH (legacy method from dl.py)."""
        logger.info(f"Creating remote branch {branch} for {owner}/{repo} via SSH")

        ssh_command = ["ssh"]
        if ssh_key_path:
            ssh_command.extend(["-i", ssh_key_path])

        ssh_command.extend(["git@github.com", "create", f"{owner}/{repo}", branch])

        try:
            result = subprocess.run(
                ssh_command, capture_output=True, text=True, timeout=10, check=False
            )

            if result.returncode == 0:
                logger.info(f"Successfully created remote branch {branch}")
                return True
            # Check if branch already exists
            if "branch already exists" in result.stderr.lower():
                logger.info(f"Branch {branch} already exists on remote")
                return True
            logger.warning(f"Failed to create remote branch: {result.stderr}")
            return False

        except subprocess.TimeoutExpired:
            logger.warning("SSH command timed out")
            return False
        except Exception as e:
            logger.warning(f"Error creating remote branch via SSH: {e}")
            return False

    def checkout_branch(self, repo_path: Path, branch: str) -> None:
        """Checkout a branch in a repository or worktree."""
        try:
            result = subprocess.run(
                ["git", "checkout", branch],
                cwd=repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            logger.debug(f"Checkout output: {result.stdout}")
        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to checkout branch: {e.stderr}")
            raise RuntimeError(f"Failed to checkout branch: {e.stderr}") from e
