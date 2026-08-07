"""Storage utilities for worktree metadata."""

import contextlib
import json
import os
import shutil
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

from .models import BaseRepository, WorktreeInfo, unknown_fields

# Version of the on-disk metadata.json format. A file without a "version" key
# predates versioning and is treated as version 1.
SCHEMA_VERSION = 1

# Top-level keys this build writes, and therefore the only ones a rewrite keeps.
_KNOWN_SECTIONS = frozenset({"version", "repositories", "worktrees"})

# Errors raised when a single stored entry cannot be rebuilt into a model:
# KeyError for a missing field, TypeError for an unknown/bad-typed field
# (from_dict does cls(**data)), ValueError for an unparsable timestamp.
_ENTRY_ERRORS = (KeyError, TypeError, ValueError)


def _get_default_metadata_path() -> Path:
    """Get the default metadata path, honoring XDG_CACHE_HOME."""
    xdg_cache = os.environ.get("XDG_CACHE_HOME")
    if xdg_cache:
        return Path(xdg_cache) / "devlaunch" / "metadata.json"
    return Path.home() / ".cache" / "devlaunch" / "metadata.json"


def _warn(message: str) -> None:
    """Emit a single warning line on stderr (stdout is parsed by completions)."""
    print(f"dl: {message}", file=sys.stderr)


def _file_mode(path: Path) -> Optional[int]:
    """Return the permission bits of ``path``, or None if it does not exist."""
    try:
        return stat.S_IMODE(path.stat().st_mode)
    except OSError:
        return None


def _resolve_link(path: Path) -> Path:
    """Return the real file behind ``path``, following it if it is a symlink.

    Only the final component is resolved. Writing atomically means renaming a
    temp file over the target, which would replace a symlink with a regular
    file; anyone who points metadata.json at a synced directory would silently
    lose the link and every later write. Resolving once up front keeps every
    file operation (write, quarantine, backup) on the real file.
    """
    if path.is_symlink():
        return Path(os.path.realpath(path))
    return path


class MetadataStorage:
    """Handles persistent storage of worktree metadata."""

    def __init__(self, metadata_path: Optional[Path] = None):
        """Initialize metadata storage."""
        if metadata_path is None:
            metadata_path = _get_default_metadata_path()
        self.metadata_path = metadata_path
        self.metadata_path.parent.mkdir(parents=True, exist_ok=True)
        # Every file operation targets the real file, not a symlink pointing at it.
        self._file_path = _resolve_link(self.metadata_path)
        self._load()

    def _quarantine(self, reason: str) -> None:
        """Move an unusable metadata file aside so the data stays inspectable.

        A single quarantine slot is used, overwritten on repeat corruption.
        """
        corrupt_path = self._file_path.with_name(self._file_path.name + ".corrupt")
        try:
            self._file_path.replace(corrupt_path)
        except OSError as exc:
            _warn(
                f"{reason}; could not move it aside to {corrupt_path} ({exc}); "
                "starting with empty metadata"
            )
        else:
            _warn(f"{reason}; moved it to {corrupt_path} and started with empty metadata")

    def _read_file(self) -> Optional[Dict[str, Any]]:
        """Read and sanity-check the metadata file, quarantining it if unusable."""
        if not self._file_path.exists():
            return None
        try:
            with open(self._file_path, "r", encoding="utf-8") as f:
                data = json.load(f)
        # ValueError covers both json.JSONDecodeError (a ValueError subclass) and
        # the UnicodeDecodeError that non-UTF-8 bytes raise from inside json.load.
        except (OSError, ValueError) as exc:
            self._quarantine(f"could not read metadata file {self._file_path} ({exc})")
            return None
        if not isinstance(data, dict):
            self._quarantine(
                f"metadata file {self._file_path} is not a JSON object "
                f"(found {type(data).__name__})"
            )
            return None
        return data

    def _backup(self) -> None:
        """Copy the on-disk file aside before a lossy rewrite can overwrite it.

        This runs at load time, while the original bytes are still on disk: the
        next mutation rewrites the file from what was loaded, so anything _load
        could not round-trip is gone by then. A single backup slot is used,
        overwritten on repeat, kept separate from the quarantine slot so the two
        recovery cases cannot clobber each other.
        """
        backup_path = self._file_path.with_name(self._file_path.name + ".bak")
        reason = (
            f"rewriting {self._file_path} in this build's format will drop "
            "information it currently holds"
        )
        try:
            shutil.copy2(self._file_path, backup_path)
        except OSError as exc:
            _warn(f"{reason}; could not preserve the original at {backup_path} ({exc})")
        else:
            _warn(f"{reason}; preserved the original at {backup_path}")

    def _load_section(
        self, data: Dict[str, Any], section: str, model: Any
    ) -> Tuple[Dict[str, Any], bool]:
        """Rebuild one section, skipping (not discarding) individually broken entries.

        Returns the loaded entries and whether anything stored was left behind --
        an entry that could not be rebuilt at all, or one carrying a field this
        build does not declare and so would drop on the next write.
        """
        entries = data.get(section, {})
        if not isinstance(entries, dict):
            _warn(
                f'ignoring the "{section}" section of {self._file_path}: '
                f"expected an object, found {type(entries).__name__}"
            )
            return {}, True

        loaded: Dict[str, Any] = {}
        lossy = False
        for key, entry in entries.items():
            if not isinstance(entry, dict):
                lossy = True
                _warn(
                    f"skipping malformed {section} entry {key!r} in {self._file_path}: "
                    f"expected an object, found {type(entry).__name__}"
                )
                continue
            try:
                loaded[key] = model.from_dict(entry)
            except _ENTRY_ERRORS as exc:
                lossy = True
                _warn(f"skipping malformed {section} entry {key!r} in {self._file_path}: {exc!r}")
                continue
            # The entry loaded, but a field only a newer build knows about is not
            # carried into the rebuilt model and disappears on the next write.
            extra = unknown_fields(model, entry)
            if extra:
                lossy = True
                _warn(
                    f"{section} entry {key!r} in {self._file_path} has field(s) this build "
                    f"does not understand ({', '.join(extra)}); they are dropped when it "
                    "is rewritten"
                )
        return loaded, lossy

    def _load_version(self, data: Dict[str, Any]) -> Tuple[int, bool]:
        """Interpret the version header, returning the version and whether it is lossy."""
        if "version" not in data:
            # An absent version means a legacy pre-versioning file: same shape as v1.
            return SCHEMA_VERSION, False

        raw = data["version"]
        # JSON has a single number type, so tools freely normalize 1 to 1.0; an
        # integral number is that version. bool is an int subclass in Python, but
        # a true/false header is nonsense rather than version 1.
        if isinstance(raw, int) and not isinstance(raw, bool):
            version = raw
        elif isinstance(raw, float) and raw.is_integer():
            version = int(raw)
        else:
            # The entries do not depend on the header, so never discard them over
            # it: warn, read the file as legacy v1, and preserve the original
            # because the rewritten header will not match what is there now.
            _warn(
                f'metadata file {self._file_path} has an invalid "version" header '
                f"({raw!r}); reading it as schema version {SCHEMA_VERSION}"
            )
            return SCHEMA_VERSION, True

        if version > SCHEMA_VERSION:
            _warn(
                f"{self._file_path} was written by a newer devlaunch (schema version "
                f"{version}, this build understands {SCHEMA_VERSION}); its entries are "
                f"loaded as-is, and the next change rewrites the whole file as schema "
                f"version {SCHEMA_VERSION}"
            )
            return version, True

        # A version below SCHEMA_VERSION is an older shape, upgraded on the next
        # write; the value is exposed unchanged so a migration can branch on it.
        return version, False

    def _load(self) -> None:
        """Load metadata from disk, never raising on damaged input."""
        self.repositories: Dict[str, BaseRepository] = {}
        self.worktrees: Dict[str, WorktreeInfo] = {}
        self.schema_version: int = SCHEMA_VERSION

        data = self._read_file()
        if data is None:
            return

        self.schema_version, version_is_lossy = self._load_version(data)
        self.repositories, repos_skipped = self._load_section(data, "repositories", BaseRepository)
        self.worktrees, worktrees_skipped = self._load_section(data, "worktrees", WorktreeInfo)

        unknown = sorted(set(data) - _KNOWN_SECTIONS)
        if unknown:
            _warn(
                f"{self._file_path} has top-level key(s) this build does not understand "
                f"({', '.join(unknown)}); they are dropped when it is rewritten"
            )

        if version_is_lossy or repos_skipped or worktrees_skipped or unknown:
            self._backup()

    def save(self) -> None:
        """Save metadata to disk atomically.

        Writes to a fresh temp file, fsyncs it, then renames it over the real
        path, so an interrupted write can never leave a truncated metadata.json
        behind. Write failures are deliberately not swallowed: silently losing
        workspace metadata is worse than an error.
        """
        data = {
            "version": SCHEMA_VERSION,
            "repositories": {key: repo.to_dict() for key, repo in self.repositories.items()},
            "worktrees": {key: worktree.to_dict() for key, worktree in self.worktrees.items()},
        }

        # mkstemp gives a name no other writer can be holding, in the same
        # directory so the rename stays atomic, created 0600 so the contents are
        # never briefly world-readable.
        fd, temp_name = tempfile.mkstemp(
            dir=self._file_path.parent, prefix=f"{self._file_path.name}.", suffix=".tmp"
        )
        temp_path = Path(temp_name)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                json.dump(data, f, indent=2)
                f.flush()
                os.fsync(f.fileno())
            # Renaming a fresh file would otherwise reset the mode to the umask
            # default, silently widening a metadata.json the user locked down.
            mode = _file_mode(self._file_path)
            if mode is not None:
                os.chmod(temp_path, mode)
            temp_path.replace(self._file_path)
        finally:
            # No-op after a successful rename; cleans up a failed write.
            with contextlib.suppress(OSError):
                temp_path.unlink(missing_ok=True)

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

        # Update repository's worktree list in memory, then write once.
        repo = self.get_repository(worktree.owner, worktree.repo)
        if repo and worktree.branch not in repo.worktrees:
            repo.worktrees.append(worktree.branch)
            self.repositories[f"{worktree.owner}/{worktree.repo}"] = repo

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

    def get_worktree_by_workspace_id(self, workspace_id: str) -> Optional[WorktreeInfo]:
        """Look up a worktree by its DevPod workspace ID."""
        for worktree in self.worktrees.values():
            if worktree.workspace_id == workspace_id:
                return worktree
        return None

    def remove_worktree(self, owner: str, repo: str, branch: str) -> None:
        """Remove a worktree."""
        key = f"{owner}/{repo}/{branch}"
        if key in self.worktrees:
            del self.worktrees[key]

            # Update repository's worktree list in memory, then write once.
            repo_obj = self.get_repository(owner, repo)
            if repo_obj and branch in repo_obj.worktrees:
                repo_obj.worktrees.remove(branch)
                self.repositories[f"{owner}/{repo}"] = repo_obj

            self.save()
