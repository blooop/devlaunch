"""Branch management for worktree backend."""

import logging
import os
import subprocess
from pathlib import Path
from typing import List, Optional

logger = logging.getLogger(__name__)

# Constants for git paths
REFS_HEADS_PREFIX = "refs/heads/"


class BranchManager:
    """Manages git branch operations."""

    def ensure_branch_exists(
        self,
        base_repo_path: Path,
        branch: str,
        remote: str = "origin",
        create_remote: bool = True,
        ssh_key_path: Optional[str] = None,
        start_point: str = "HEAD",
        use_local_refs: bool = False,
    ) -> None:
        """Ensure branch exists locally and optionally remotely.

        Args:
            use_local_refs: When True, infer remote branch existence from local
                refs instead of calling ``git ls-remote``.  Safe in bare repos
                whose refspec maps remote heads to local heads (the default
                ``+refs/heads/*:refs/heads/*``).
        """
        # Check if branch exists locally
        local_exists = self.local_branch_exists(base_repo_path, branch)

        # Check if branch exists remotely
        if use_local_refs:
            remote_exists = local_exists
        else:
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
            self.create_local_branch(base_repo_path, branch, start_point)
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
                # The already-exists arm below is classified from git's stderr
                # text, which git translates -- so git is addressed in the C
                # locale, or on a German host an ordinary re-launch of a branch
                # that is already there raises instead of being swallowed.
                # LANGUAGE is pinned too: under gettext it outranks a non-C
                # LC_ALL, and the guarantee should not hang on the one glibc
                # rule that exempts C.
                env={**os.environ, "LC_ALL": "C", "LANGUAGE": "C"},
            )
            logger.debug(f"Branch creation output: {result.stdout}")
        except subprocess.CalledProcessError as e:
            # Branch might already exist
            if "already exists" in e.stderr:
                logger.debug(f"Branch {branch} already exists")
            else:
                logger.debug(f"Failed to create branch: {e.stderr}")
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
                ["git", "show-ref", "--verify", f"{REFS_HEADS_PREFIX}{branch}"],
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
                        if branch_ref.startswith(REFS_HEADS_PREFIX):
                            branches.append(branch_ref[len(REFS_HEADS_PREFIX) :])

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
            logger.debug(f"Failed to push branch: {e.stderr}")
            raise RuntimeError(f"Failed to push branch to remote: {e.stderr}") from e
