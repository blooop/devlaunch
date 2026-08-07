"""Data models for worktree backend."""

from dataclasses import asdict, dataclass, field, fields
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional


def unknown_fields(model: Any, data: Dict) -> List[str]:
    """Return the keys in ``data`` that ``model`` declares no field for.

    A newer devlaunch writing an extra field is the expected source. Callers use
    this to report what a rewrite would drop; ``from_dict`` ignores those keys.
    """
    known = {f.name for f in fields(model)}
    return sorted(key for key in data if key not in known)


def _drop_unknown(model: Any, data: Dict) -> Dict:
    """Copy ``data`` keeping only the fields ``model`` declares.

    Ignoring an unrecognized field keeps the entry loadable. Rebuilding it with
    ``cls(**data)`` instead would raise TypeError, and since one stored field is
    shaped like every other, a single field added by a newer build would make
    every entry unreadable at once.
    """
    known = {f.name for f in fields(model)}
    return {key: value for key, value in data.items() if key in known}


@dataclass
class BaseRepository:
    """Represents a base git repository."""

    owner: str
    repo: str
    remote_url: str
    local_path: Path
    default_branch: str = "main"
    last_fetched: Optional[datetime] = None
    worktrees: List[str] = field(default_factory=list)  # List of active worktree branch names

    def to_dict(self) -> Dict:
        """Convert to dictionary for JSON serialization."""
        data = asdict(self)
        data["local_path"] = str(self.local_path)
        data["last_fetched"] = self.last_fetched.isoformat() if self.last_fetched else None
        return data

    @classmethod
    def from_dict(cls, data: Dict) -> "BaseRepository":
        """Create from dictionary, ignoring fields this build does not declare."""
        data = _drop_unknown(cls, data)
        data["local_path"] = Path(data["local_path"])
        if data.get("last_fetched"):
            data["last_fetched"] = datetime.fromisoformat(data["last_fetched"])
        return cls(**data)


@dataclass
class WorktreeInfo:
    """Represents a git worktree."""

    owner: str
    repo: str
    branch: str
    local_path: Path
    workspace_id: str
    created_at: datetime = field(default_factory=datetime.now)
    last_used: datetime = field(default_factory=datetime.now)
    devpod_workspace_id: Optional[str] = None  # Associated DevPod workspace

    def to_dict(self) -> Dict:
        """Convert to dictionary for JSON serialization."""
        data = asdict(self)
        data["local_path"] = str(self.local_path)
        data["created_at"] = self.created_at.isoformat()
        data["last_used"] = self.last_used.isoformat()
        return data

    @classmethod
    def from_dict(cls, data: Dict) -> "WorktreeInfo":
        """Create from dictionary, ignoring fields this build does not declare."""
        data = _drop_unknown(cls, data)
        data["local_path"] = Path(data["local_path"])
        data["created_at"] = datetime.fromisoformat(data["created_at"])
        data["last_used"] = datetime.fromisoformat(data["last_used"])
        return cls(**data)
