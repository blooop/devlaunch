"""Storage utilities for worktree metadata."""

import json
import os
from pathlib import Path
from typing import Dict, List, Optional

from .models import BaseRepository, WorktreeInfo


def _get_default_metadata_path() -> Path:
    """Get the default metadata path, honoring XDG_CACHE_HOME."""
    xdg_cache = os.environ.get("XDG_CACHE_HOME")
    if xdg_cache:
        return Path(xdg_cache) / "devlaunch" / "metadata.json"
    return Path.home() / ".cache" / "devlaunch" / "metadata.json"


class MetadataStorage:
    """Handles persistent storage of worktree metadata."""

    def __init__(self, metadata_path: Optional[Path] = None):
        """Initialize metadata storage."""
        if metadata_path is None:
            metadata_path = _get_default_metadata_path()
        self.metadata_path = metadata_path
        self.metadata_path.parent.mkdir(parents=True, exist_ok=True)
        self._load()

    def _load(self) -> None:
        """Load metadata from disk."""
        if self.metadata_path.exists():
            with open(self.metadata_path, "r", encoding="utf-8") as f:
                data = json.load(f)
        else:
            data = {"repositories": {}, "worktrees": {}}

        self.repositories: Dict[str, BaseRepository] = {}
        self.worktrees: Dict[str, WorktreeInfo] = {}

        # Load repositories
        for key, repo_data in data.get("repositories", {}).items():
            self.repositories[key] = BaseRepository.from_dict(repo_data)

        # Load worktrees
        for key, worktree_data in data.get("worktrees", {}).items():
            self.worktrees[key] = WorktreeInfo.from_dict(worktree_data)

    def save(self) -> None:
        """Save metadata to disk."""
        data = {
            "repositories": {key: repo.to_dict() for key, repo in self.repositories.items()},
            "worktrees": {key: worktree.to_dict() for key, worktree in self.worktrees.items()},
        }

        with open(self.metadata_path, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2)

    def add_repository(self, repo: BaseRepository) -> None:
        """Add or update a repository."""
        key = f"{repo.owner}/{repo.repo}"
        self.repositories[key] = repo
        self.save()

    def get_repository(self, owner: str, repo: str) -> Optional[BaseRepository]:
        """Get a repository by owner and name."""
        key = f"{owner}/{repo}"
        return self.repositories.get(key)

    def list_repositories(self) -> List[BaseRepository]:
        """List all repositories."""
        return list(self.repositories.values())

    def remove_repository(self, owner: str, repo: str) -> None:
        """Remove a repository."""
        key = f"{owner}/{repo}"
        if key in self.repositories:
            del self.repositories[key]
            self.save()

    def add_worktree(self, worktree: WorktreeInfo) -> None:
        """Add or update a worktree."""
        key = f"{worktree.owner}/{worktree.repo}/{worktree.branch}"
        self.worktrees[key] = worktree

        # Update repository's worktree list
        repo = self.get_repository(worktree.owner, worktree.repo)
        if repo and worktree.branch not in repo.worktrees:
            repo.worktrees.append(worktree.branch)
            self.add_repository(repo)

        self.save()

    def get_worktree(self, owner: str, repo: str, branch: str) -> Optional[WorktreeInfo]:
        """Get a worktree by repository and branch."""
        key = f"{owner}/{repo}/{branch}"
        return self.worktrees.get(key)

    def list_worktrees(
        self, owner: Optional[str] = None, repo: Optional[str] = None
    ) -> List[WorktreeInfo]:
        """List worktrees, optionally filtered by repository."""
        worktrees = list(self.worktrees.values())

        if owner and repo:
            worktrees = [w for w in worktrees if w.owner == owner and w.repo == repo]
        elif owner:
            worktrees = [w for w in worktrees if w.owner == owner]

        return worktrees

    def remove_worktree(self, owner: str, repo: str, branch: str) -> None:
        """Remove a worktree."""
        key = f"{owner}/{repo}/{branch}"
        if key in self.worktrees:
            del self.worktrees[key]

            # Update repository's worktree list
            repo_obj = self.get_repository(owner, repo)
            if repo_obj and branch in repo_obj.worktrees:
                repo_obj.worktrees.remove(branch)
                self.add_repository(repo_obj)

            self.save()
