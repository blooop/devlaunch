"""Configuration management for worktree backend."""

import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, Union

import tomli
import tomli_w


def _get_cache_base() -> Path:
    """Get the base cache directory, honoring XDG_CACHE_HOME."""
    xdg_cache = os.environ.get("XDG_CACHE_HOME")
    if xdg_cache:
        return Path(xdg_cache) / "devlaunch"
    return Path.home() / ".cache" / "devlaunch"


@dataclass
class WorktreeConfig:
    """Configuration for worktree backend."""

    enabled: bool = True  # Enabled by default
    repos_dir: Union[Path, str] = field(default_factory=lambda: _get_cache_base() / "repos")
    worktrees_dir: Union[Path, str] = field(default_factory=lambda: _get_cache_base() / "worktrees")
    auto_fetch: bool = True
    fetch_interval: int = 3600  # Seconds between auto-fetches
    auto_prune: bool = True
    prune_after_days: int = 30

    def __post_init__(self):
        """Ensure paths are Path objects and expand user."""
        if isinstance(self.repos_dir, str):
            self.repos_dir = Path(self.repos_dir).expanduser()
        if isinstance(self.worktrees_dir, str):
            self.worktrees_dir = Path(self.worktrees_dir).expanduser()

        # Ensure directories exist (only if they're under home or temp)
        # This avoids permission errors in tests
        try:
            if str(self.repos_dir).startswith(str(Path.home())) or str(self.repos_dir).startswith(
                "/tmp"
            ):
                self.repos_dir.mkdir(parents=True, exist_ok=True)
            if str(self.worktrees_dir).startswith(str(Path.home())) or str(
                self.worktrees_dir
            ).startswith("/tmp"):
                self.worktrees_dir.mkdir(parents=True, exist_ok=True)
        except (OSError, PermissionError):
            # Ignore permission errors (e.g., in tests)
            pass

    def to_dict(self) -> Dict:
        """Convert to dictionary for TOML serialization."""
        return {
            "worktree": {
                "enabled": self.enabled,
                "repos_dir": str(self.repos_dir),
                "worktrees_dir": str(self.worktrees_dir),
                "auto_fetch": self.auto_fetch,
                "fetch_interval": self.fetch_interval,
                "cleanup": {
                    "auto_prune": self.auto_prune,
                    "prune_after_days": self.prune_after_days,
                },
            }
        }

    @classmethod
    def from_dict(cls, data: Dict) -> "WorktreeConfig":
        """Create from dictionary."""
        worktree_data = data.get("worktree", {})
        cleanup_data = worktree_data.get("cleanup", {})

        return cls(
            enabled=worktree_data.get("enabled", True),
            repos_dir=Path(worktree_data.get("repos_dir", _get_cache_base() / "repos")),
            worktrees_dir=Path(worktree_data.get("worktrees_dir", _get_cache_base() / "worktrees")),
            auto_fetch=worktree_data.get("auto_fetch", True),
            fetch_interval=worktree_data.get("fetch_interval", 3600),
            auto_prune=cleanup_data.get("auto_prune", True),
            prune_after_days=cleanup_data.get("prune_after_days", 30),
        )


def get_config_path() -> Path:
    """Get the path to the config file."""
    config_dir = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
    return config_dir / "devlaunch" / "config.toml"


def load_config() -> Dict:
    """Load configuration from file."""
    config_path = get_config_path()
    if not config_path.exists():
        return {}

    with open(config_path, "rb") as f:
        return tomli.load(f)


def save_config(config: Dict) -> None:
    """Save configuration to file."""
    config_path = get_config_path()
    config_path.parent.mkdir(parents=True, exist_ok=True)

    with open(config_path, "wb") as f:
        tomli_w.dump(config, f)


def get_worktree_config() -> WorktreeConfig:
    """Get worktree configuration, loading from file if exists."""
    config_data = load_config()
    return WorktreeConfig.from_dict(config_data)
