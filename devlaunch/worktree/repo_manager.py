"""Repository manager for worktree backend."""

import logging
import subprocess
from datetime import datetime
from pathlib import Path
from typing import Optional, TYPE_CHECKING

from .models import BaseRepository
from .storage import MetadataStorage

if TYPE_CHECKING:
    from .config import WorktreeConfig

logger = logging.getLogger(__name__)


class RepositoryManager:
    """Manages base git repositories."""

    def __init__(
        self,
        repos_dir: Path,
        storage: Optional[MetadataStorage] = None,
        config: Optional["WorktreeConfig"] = None,
    ):
        """Initialize repository manager."""
        self.repos_dir = repos_dir
        self.repos_dir.mkdir(parents=True, exist_ok=True)
        self.storage = storage or MetadataStorage()
        self.config = config
        # Default fetch interval: 1 hour
        self.fetch_interval = config.fetch_interval if config else 3600

    def get_repo_path(self, owner: str, repo: str) -> Path:
        """Get local path for a repository."""
        return self.repos_dir / owner / repo

    def clone_repo(self, owner: str, repo: str, remote_url: str) -> BaseRepository:
        """Clone a new base repository as bare (no working directory).

        Using --bare ensures no branch is checked out, so all branches can have
        worktrees created without conflicts.
        """
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
            # Clone as bare repo - no working directory, all branches available for worktrees
            result = subprocess.run(
                ["git", "clone", "--bare", remote_url, str(repo_path)],
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
            raise RuntimeError(f"Failed to clone repository: {e.stderr}") from e

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
            raise RuntimeError(f"Failed to fetch repository: {e.stderr}") from e

    def _should_fetch(self, repo: BaseRepository) -> bool:
        """Check if repository should be fetched based on fetch_interval.

        Returns True if:
        - Repository has never been fetched
        - Time since last fetch exceeds fetch_interval
        """
        if not repo.last_fetched:
            return True
        elapsed = (datetime.now() - repo.last_fetched).total_seconds()
        return elapsed > self.fetch_interval

    def ensure_repo(
        self, owner: str, repo: str, remote_url: str, auto_fetch: bool = True
    ) -> BaseRepository:
        """Ensure repo exists locally, clone if needed.

        Uses lazy fetch: only fetches if fetch_interval has elapsed since last fetch.
        """
        if self.repo_exists(owner, repo):
            existing_repo = self.get_repo(owner, repo)
            if existing_repo:
                # Only fetch if interval has elapsed (lazy fetch)
                if auto_fetch and self._should_fetch(existing_repo):
                    try:
                        self.fetch_repo(owner, repo)
                    except Exception as e:
                        logger.warning(f"Failed to fetch updates: {e}")
                return existing_repo
            # Metadata doesn't exist but repo exists - fall through to clone (which will add metadata)

        return self.clone_repo(owner, repo, remote_url)

    def repo_exists(self, owner: str, repo: str) -> bool:
        """Check if repository exists locally.

        Supports both bare repos (HEAD at root) and regular repos (.git subdir).
        """
        repo_path = self.get_repo_path(owner, repo)
        if not repo_path.exists():
            return False
        # Bare repo has HEAD directly in the repo dir
        # Regular repo has .git subdirectory
        return (repo_path / "HEAD").exists() or (repo_path / ".git").exists()

    def get_repo(self, owner: str, repo: str) -> Optional[BaseRepository]:
        """Get repository metadata."""
        base_repo = self.storage.get_repository(owner, repo)

        if base_repo and not self.repo_exists(owner, repo):
            # Repository metadata exists but directory doesn't
            logger.warning(f"Repository {owner}/{repo} metadata exists but directory missing")
            return None

        return base_repo

    def _get_default_branch(self, repo_path: Path) -> str:
        """Get the default branch of a repository.

        Works with both bare repos and regular repos.
        """
        try:
            # For bare repos, HEAD points directly to refs/heads/<branch>
            result = subprocess.run(
                ["git", "symbolic-ref", "HEAD"],
                cwd=repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
            # Output is like "refs/heads/main"
            branch = result.stdout.strip().split("/")[-1]
            return branch
        except subprocess.CalledProcessError:
            pass

        # Fallback: try the remote HEAD (for regular repos)
        try:
            result = subprocess.run(
                ["git", "symbolic-ref", "refs/remotes/origin/HEAD"],
                cwd=repo_path,
                capture_output=True,
                text=True,
                check=True,
            )
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
                if "origin/master" in branches:
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
