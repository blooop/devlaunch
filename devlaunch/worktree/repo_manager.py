"""Repository manager for worktree backend."""

import logging
import subprocess
from datetime import datetime
from pathlib import Path
from typing import Optional

from .models import BaseRepository
from .storage import MetadataStorage

logger = logging.getLogger(__name__)


class RepositoryManager:
    """Manages base git repositories."""

    def __init__(self, repos_dir: Path, storage: Optional[MetadataStorage] = None):
        """Initialize repository manager."""
        self.repos_dir = repos_dir
        self.repos_dir.mkdir(parents=True, exist_ok=True)
        self.storage = storage or MetadataStorage()

    def get_repo_path(self, owner: str, repo: str) -> Path:
        """Get local path for a repository."""
        return self.repos_dir / owner / repo

    def clone_repo(self, owner: str, repo: str, remote_url: str) -> BaseRepository:
        """Clone a new base repository."""
        repo_path = self.get_repo_path(owner, repo)

        if repo_path.exists():
            logger.warning(f"Repository {owner}/{repo} already exists at {repo_path}")
            existing_repo = self.get_repo(owner, repo)
            if existing_repo:
                return existing_repo
            # Repository path exists but metadata doesn't - continue to create metadata

        # Create parent directory
        repo_path.parent.mkdir(parents=True, exist_ok=True)

        logger.info(f"Cloning repository {remote_url} to {repo_path}")

        try:
            # Clone the repository
            result = subprocess.run(
                ["git", "clone", remote_url, str(repo_path)],
                capture_output=True,
                text=True,
                check=True,
            )
            logger.debug(f"Clone output: {result.stdout}")

            # Get default branch
            default_branch = self._get_default_branch(repo_path)

            # Create repository metadata
            base_repo = BaseRepository(
                owner=owner,
                repo=repo,
                remote_url=remote_url,
                local_path=repo_path,
                default_branch=default_branch,
                last_fetched=datetime.now(),
                worktrees=[],
            )

            # Save metadata
            self.storage.add_repository(base_repo)

            logger.info(f"Successfully cloned {owner}/{repo}")
            return base_repo

        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to clone repository: {e.stderr}")
            # Clean up partial clone
            if repo_path.exists():
                import shutil

                shutil.rmtree(repo_path)
            raise RuntimeError(f"Failed to clone repository: {e.stderr}")

    def fetch_repo(self, owner: str, repo: str) -> None:
        """Fetch latest changes from remote."""
        repo_path = self.get_repo_path(owner, repo)

        if not repo_path.exists():
            raise ValueError(f"Repository {owner}/{repo} does not exist locally")

        logger.info(f"Fetching updates for {owner}/{repo}")

        try:
            # Fetch all branches and tags
            result = subprocess.run(
                ["git", "fetch", "--all", "--tags", "--prune"],
                cwd=repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            logger.debug(f"Fetch output: {result.stdout}")

            # Update metadata
            base_repo = self.storage.get_repository(owner, repo)
            if base_repo:
                base_repo.last_fetched = datetime.now()
                self.storage.add_repository(base_repo)

            logger.info(f"Successfully fetched updates for {owner}/{repo}")

        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to fetch repository: {e.stderr}")
            raise RuntimeError(f"Failed to fetch repository: {e.stderr}")

    def ensure_repo(
        self, owner: str, repo: str, remote_url: str, auto_fetch: bool = True
    ) -> BaseRepository:
        """Ensure repo exists locally, clone if needed."""
        if self.repo_exists(owner, repo):
            if auto_fetch:
                try:
                    self.fetch_repo(owner, repo)
                except Exception as e:
                    logger.warning(f"Failed to fetch updates: {e}")
            existing_repo = self.get_repo(owner, repo)
            if existing_repo:
                return existing_repo
            # Metadata doesn't exist but repo exists - fall through to clone (which will add metadata)

        return self.clone_repo(owner, repo, remote_url)

    def repo_exists(self, owner: str, repo: str) -> bool:
        """Check if repository exists locally."""
        repo_path = self.get_repo_path(owner, repo)
        return repo_path.exists() and (repo_path / ".git").exists()

    def get_repo(self, owner: str, repo: str) -> Optional[BaseRepository]:
        """Get repository metadata."""
        base_repo = self.storage.get_repository(owner, repo)

        if base_repo and not self.repo_exists(owner, repo):
            # Repository metadata exists but directory doesn't
            logger.warning(f"Repository {owner}/{repo} metadata exists but directory missing")
            return None

        return base_repo

    def _get_default_branch(self, repo_path: Path) -> str:
        """Get the default branch of a repository."""
        try:
            result = subprocess.run(
                ["git", "symbolic-ref", "refs/remotes/origin/HEAD"],
                cwd=repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            # Output is like "refs/remotes/origin/main"
            branch = result.stdout.strip().split("/")[-1]
            return branch
        except subprocess.CalledProcessError:
            # Fallback to main or master
            try:
                result = subprocess.run(
                    ["git", "branch", "-r"],
                    cwd=repo_path,
                    capture_output=True,
                    text=True,
                    check=True,
                )
                branches = result.stdout.strip()
                if "origin/main" in branches:
                    return "main"
                elif "origin/master" in branches:
                    return "master"
            except subprocess.CalledProcessError:
                pass

        return "main"  # Default fallback

    def list_repositories(self):
        """List all managed repositories."""
        return self.storage.list_repositories()

    def remove_repository(self, owner: str, repo: str, remove_directory: bool = True) -> None:
        """Remove a repository from management."""
        # Remove metadata
        self.storage.remove_repository(owner, repo)

        # Optionally remove directory
        if remove_directory:
            repo_path = self.get_repo_path(owner, repo)
            if repo_path.exists():
                import shutil

                shutil.rmtree(repo_path)
                logger.info(f"Removed repository directory {repo_path}")
