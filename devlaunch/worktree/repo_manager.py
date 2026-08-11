"""Repository manager for worktree backend."""

import logging
import shutil
import subprocess
from datetime import datetime
from pathlib import Path
from typing import Optional, TYPE_CHECKING

from .locks import hold_lock
from .models import BaseRepository
from .storage import MetadataStorage

if TYPE_CHECKING:
    from .config import WorktreeConfig

logger = logging.getLogger(__name__)


def _branch_of(ref: str) -> str:
    """The branch a symbolic ref names, with its namespace prefix removed.

    Not the last path segment: `release/1.0` and `feature/auth` are ordinary
    branch names, and taking the segment after the final slash renames them to
    `1.0` and `auth` -- refs the repository does not have.
    """
    for prefix in ("refs/remotes/origin/", "refs/heads/"):
        if ref.startswith(prefix):
            return ref[len(prefix) :]
    return ref.rsplit("/", maxsplit=1)[-1]


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
        """Get root directory for a repository (contains .bare/ and branch clones)."""
        return self.repos_dir / owner / repo

    def get_bare_path(self, owner: str, repo: str) -> Path:
        """Get the bare git directory for a repository."""
        return self.get_repo_path(owner, repo) / ".bare"

    def lock_path(self, owner: str, repo: str) -> Path:
        """The lock every process takes before mutating repos/<owner>/<repo>.

        A file, not a directory, inside the repo dir: every walker of the cache
        filters on ``is_dir()``, so it is invisible to discovery, migration and
        completion scans.
        """
        return self.get_repo_path(owner, repo) / ".lock"

    def clone_repo(self, owner: str, repo: str, remote_url: str) -> BaseRepository:
        """Clone a new base repository as bare (no working directory).

        Using --bare ensures no branch is checked out, so all branches can have
        worktrees created without conflicts.

        Layout:
            repos/<owner>/<repo>/.bare/   # bare git data
            repos/<owner>/<repo>/<branch>/ # workspace clones (managed by WorkspaceCloneManager)
        """
        bare_path = self.get_bare_path(owner, repo)

        if bare_path.exists():
            logger.warning(f"Repository {owner}/{repo} already exists at {bare_path}")
            existing_repo = self.get_repo(owner, repo)
            if existing_repo:
                return existing_repo
            if (bare_path / "HEAD").exists():
                # The bare clone is already on disk but this process has no
                # record of it -- another process just made it (this process's
                # metadata was loaded before that one saved), or an earlier run
                # died between clone and save. Either way the clone on disk is
                # the authority and the record is derived state: rebuild the
                # record. Cloning over it instead is not an option -- git
                # refuses the non-empty destination, and the failure cleanup
                # below would then delete a cache another launch is using.
                return self._register_existing_bare(owner, repo, remote_url, bare_path)
            # No HEAD: a dead run's partial clone. Holding the repo lock (every
            # caller comes through ensure_repo) means no live process owns it,
            # so clear it and clone fresh.
            logger.warning(f"Removing partial clone at {bare_path}")
            shutil.rmtree(bare_path)

        # Create parent directory
        bare_path.parent.mkdir(parents=True, exist_ok=True)

        logger.info(f"Cloning repository {remote_url} to {bare_path}")

        try:
            # Clone as bare repo - no working directory, all branches available for worktrees
            result = subprocess.run(
                ["git", "clone", "--bare", remote_url, str(bare_path)],
                capture_output=True,
                text=True,
                check=True,
            )
            logger.debug(f"Clone output: {result.stdout}")

            # Get default branch
            default_branch = self._get_default_branch(bare_path)

            # Create repository metadata
            base_repo = BaseRepository(
                owner=owner,
                repo=repo,
                remote_url=remote_url,
                local_path=bare_path,
                default_branch=default_branch,
                last_fetched=datetime.now(),
                worktrees=[],
            )

            # Save metadata
            self.storage.add_repository(base_repo)

            logger.info(f"Successfully cloned {owner}/{repo}")
            return base_repo

        except subprocess.CalledProcessError as e:
            logger.debug(f"Failed to clone repository: {e.stderr}")
            # Clean up the partial clone. Safe to delete: the exists-cases were
            # all handled above, so this directory is one this call created.
            if bare_path.exists():
                shutil.rmtree(bare_path)
            raise RuntimeError(f"Failed to clone repository: {e.stderr}") from e

    def _register_existing_bare(
        self, owner: str, repo: str, remote_url: str, bare_path: Path
    ) -> BaseRepository:
        """Rebuild the metadata record for a bare clone already on disk."""
        base_repo = BaseRepository(
            owner=owner,
            repo=repo,
            remote_url=remote_url,
            local_path=bare_path,
            default_branch=self._get_default_branch(bare_path),
            last_fetched=datetime.now(),
            worktrees=[],
        )
        self.storage.add_repository(base_repo)
        return base_repo

    def fetch_repo(self, owner: str, repo: str) -> None:
        """Fetch latest changes from remote."""
        bare_path = self.get_bare_path(owner, repo)

        if not bare_path.exists():
            raise ValueError(f"Repository {owner}/{repo} does not exist locally")

        logger.info(f"Fetching updates for {owner}/{repo}")

        try:
            # Fetch all branches and tags
            result = subprocess.run(
                ["git", "fetch", "origin", "+refs/heads/*:refs/heads/*", "--tags", "--prune"],
                cwd=bare_path,
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
            logger.debug(f"Failed to fetch repository: {e.stderr}")
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

    def lazy_fetch(self, owner: str, repo: str) -> bool:
        """Fetch only if the fetch interval has elapsed since the last fetch.

        Returns True if a fetch was performed, False if skipped.
        Raises ValueError if the repository is not in metadata.
        """
        base_repo = self.storage.get_repository(owner, repo)
        if not base_repo:
            raise ValueError(f"Repository {owner}/{repo} not found in metadata")
        if self._should_fetch(base_repo):
            self.fetch_repo(owner, repo)
            return True
        return False

    def ensure_repo(
        self, owner: str, repo: str, remote_url: str, auto_fetch: bool = True
    ) -> BaseRepository:
        """Ensure repo exists locally, clone if needed.

        Uses lazy fetch: only fetches if fetch_interval has elapsed since last fetch.

        The whole exists-check-then-clone sequence runs under the repo lock:
        without it, two processes launching the same repo at once both saw no
        clone and both ran ``git clone --bare`` into the same path — and the
        loser's cleanup in clone_repo deleted the winner's half-written cache.
        Serialized, the loser just waits and then reuses the winner's clone.
        clone_repo and fetch_repo rely on this lock rather than taking it
        themselves (hold_lock is not reentrant).
        """
        with hold_lock(
            self.lock_path(owner, repo),
            waiting_note=f"another dl run preparing {owner}/{repo}",
        ):
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
                # Metadata doesn't exist but repo exists - fall through to clone
                # (which will add metadata)

            return self.clone_repo(owner, repo, remote_url)

    def repo_exists(self, owner: str, repo: str) -> bool:
        """Check if repository exists locally."""
        bare_path = self.get_bare_path(owner, repo)
        return bare_path.exists() and (bare_path / "HEAD").exists()

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
            # Output is like "refs/heads/main". The prefix is stripped rather
            # than the last path segment taken: a branch name may contain
            # slashes, so `split("/")[-1]` turned a default branch of
            # `release/1.0` into `1.0` -- a ref the repository does not have,
            # recorded as the one every later operation targets.
            return _branch_of(result.stdout.strip())
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
            return _branch_of(result.stdout.strip())
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

    def get_default_branch(self, owner: str, repo: str) -> str:
        """Get the default branch for a repository.

        Checks local repo first, then queries remote. Falls back to 'main'.
        """
        # Check if repo exists locally
        existing_repo = self.get_repo(owner, repo)
        if existing_repo and existing_repo.default_branch:
            return existing_repo.default_branch

        # Try to get from remote
        remote_url = f"git@github.com:{owner}/{repo}.git"
        try:
            result = subprocess.run(
                ["git", "ls-remote", "--symref", remote_url, "HEAD"],
                capture_output=True,
                text=True,
                check=False,
                timeout=10,
            )
            if result.returncode == 0:
                # Parse output like: ref: refs/heads/main\tHEAD
                for line in result.stdout.strip().split("\n"):
                    if line.startswith("ref:") and "HEAD" in line:
                        ref_part = line.split()[1]
                        if ref_part.startswith("refs/heads/"):
                            return ref_part[len("refs/heads/") :]
        except (OSError, subprocess.SubprocessError, subprocess.TimeoutExpired):
            pass

        return "main"

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
                shutil.rmtree(repo_path)
                logger.info(f"Removed repository directory {repo_path}")
