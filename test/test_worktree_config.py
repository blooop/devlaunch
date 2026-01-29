"""Tests for worktree configuration."""

from pathlib import Path


from devlaunch.worktree.config import WorktreeConfig


class TestWorktreeConfig:
    """Tests for WorktreeConfig."""

    def test_default_config(self):
        """Test default configuration values."""
        config = WorktreeConfig()

        assert config.enabled is True
        assert config.repos_dir == Path.home() / ".cache" / "devlaunch" / "repos"
        assert config.auto_fetch is True
        assert config.fetch_interval == 3600
        assert config.auto_prune is True
        assert config.prune_after_days == 30

    def test_custom_config(self):
        """Test custom configuration values."""
        config = WorktreeConfig(
            enabled=False,
            repos_dir=Path("/custom/repos"),
            auto_fetch=False,
            fetch_interval=7200,
            auto_prune=False,
            prune_after_days=60,
        )

        assert config.enabled is False
        assert config.repos_dir == Path("/custom/repos")
        assert config.auto_fetch is False
        assert config.fetch_interval == 7200
        assert config.auto_prune is False
        assert config.prune_after_days == 60

    def test_string_paths(self):
        """Test that string paths are converted to Path objects."""
        config = WorktreeConfig(repos_dir="~/custom/repos")

        assert isinstance(config.repos_dir, Path)
        assert config.repos_dir == Path("~/custom/repos").expanduser()

    def test_to_dict(self):
        """Test converting config to dict."""
        config = WorktreeConfig(
            enabled=False,
            repos_dir=Path("/custom/repos"),
            auto_fetch=False,
            fetch_interval=7200,
            auto_prune=False,
            prune_after_days=60,
        )

        data = config.to_dict()
        assert data == {
            "worktree": {
                "enabled": False,
                "repos_dir": "/custom/repos",
                "auto_fetch": False,
                "fetch_interval": 7200,
                "cleanup": {
                    "auto_prune": False,
                    "prune_after_days": 60,
                },
            }
        }

    def test_from_dict(self):
        """Test creating config from dict."""
        data = {
            "worktree": {
                "enabled": False,
                "repos_dir": "/custom/repos",
                "auto_fetch": False,
                "fetch_interval": 7200,
                "cleanup": {
                    "auto_prune": False,
                    "prune_after_days": 60,
                },
            }
        }

        config = WorktreeConfig.from_dict(data)
        assert config.enabled is False
        assert config.repos_dir == Path("/custom/repos")
        assert config.auto_fetch is False
        assert config.fetch_interval == 7200
        assert config.auto_prune is False
        assert config.prune_after_days == 60

    def test_from_dict_empty(self):
        """Test creating config from empty dict uses defaults."""
        config = WorktreeConfig.from_dict({})

        assert config.enabled is True
        assert config.repos_dir == Path.home() / ".cache" / "devlaunch" / "repos"
        assert config.auto_fetch is True
        assert config.fetch_interval == 3600
        assert config.auto_prune is True
        assert config.prune_after_days == 30

    def test_from_dict_partial(self):
        """Test creating config from partial dict uses defaults for missing values."""
        data = {
            "worktree": {
                "enabled": False,
                "auto_fetch": False,
            }
        }

        config = WorktreeConfig.from_dict(data)
        assert config.enabled is False
        assert config.repos_dir == Path.home() / ".cache" / "devlaunch" / "repos"
        assert config.auto_fetch is False
        assert config.fetch_interval == 3600
        assert config.auto_prune is True
        assert config.prune_after_days == 30
