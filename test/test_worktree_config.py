"""Tests for worktree configuration."""

import logging
from pathlib import Path

import pytest


from devlaunch.worktree.config import WorktreeConfig, get_config_path


class TestWorktreeConfig:
    """Tests for WorktreeConfig."""

    @pytest.mark.usefixtures("home_cache_default")
    def test_default_config(self):
        """Test default configuration values."""
        config = WorktreeConfig()

        assert config.enabled is True
        assert config.repos_dir == Path.home() / ".cache" / "devlaunch" / "repos"
        assert config.fetch_interval == 3600
        assert config.auto_prune is True
        assert config.prune_after_days == 30

    def test_custom_config(self):
        """Test custom configuration values."""
        config = WorktreeConfig(
            enabled=False,
            repos_dir=Path("/custom/repos"),
            fetch_interval=7200,
            auto_prune=False,
            prune_after_days=60,
        )

        assert config.enabled is False
        assert config.repos_dir == Path("/custom/repos")
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
            fetch_interval=7200,
            auto_prune=False,
            prune_after_days=60,
        )

        data = config.to_dict()
        assert data["worktree"]["enabled"] is False
        assert data["worktree"]["repos_dir"] == "/custom/repos"
        assert "workspaces_dir" not in data["worktree"]
        assert data["worktree"]["fetch_interval"] == 7200
        assert data["worktree"]["cleanup"] == {
            "auto_prune": False,
            "prune_after_days": 60,
        }

    def test_from_dict(self):
        """Test creating config from dict."""
        data = {
            "worktree": {
                "enabled": False,
                "repos_dir": "/custom/repos",
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
        assert config.fetch_interval == 7200
        assert config.auto_prune is False
        assert config.prune_after_days == 60

    @pytest.mark.usefixtures("home_cache_default")
    def test_from_dict_empty(self):
        """Test creating config from empty dict uses defaults."""
        config = WorktreeConfig.from_dict({})

        assert config.enabled is True
        assert config.repos_dir == Path.home() / ".cache" / "devlaunch" / "repos"
        assert config.fetch_interval == 3600
        assert config.auto_prune is True
        assert config.prune_after_days == 30

    @pytest.mark.usefixtures("home_cache_default")
    def test_from_dict_partial(self):
        """Test creating config from partial dict uses defaults for missing values."""
        data = {
            "worktree": {
                "enabled": False,
            }
        }

        config = WorktreeConfig.from_dict(data)
        assert config.enabled is False
        assert config.repos_dir == Path.home() / ".cache" / "devlaunch" / "repos"
        assert config.fetch_interval == 3600
        assert config.auto_prune is True
        assert config.prune_after_days == 30


class TestRetiredAutoFetchKnob:
    """The auto_fetch knob is gone, and stale configs that still name it survive.

    It never gated anything: the config value was never wired to the fetch that
    shared its name, and that fetch is itself gone since the launch path stopped
    sweeping every ref. Deleting an inert knob must not turn someone's existing
    config.toml into an error.
    """

    def test_the_knob_is_absent_from_the_config_surface(self):
        config = WorktreeConfig()

        assert not hasattr(config, "auto_fetch")

    def test_a_serialized_config_carries_no_auto_fetch_key(self):
        config = WorktreeConfig(repos_dir=Path("/custom/repos"))

        assert "auto_fetch" not in config.to_dict()["worktree"]

    def test_a_stale_config_still_naming_the_knob_loads_with_its_other_keys_applied(self):
        """The loader reads the keys it knows by name and ignores the rest, so a
        config written before the knob was retired keeps working untouched."""
        data = {
            "worktree": {
                "auto_fetch": False,
                "enabled": False,
                "fetch_interval": 7200,
            }
        }

        config = WorktreeConfig.from_dict(data)

        assert config.enabled is False
        assert config.fetch_interval == 7200
        assert not hasattr(config, "auto_fetch")

    def test_a_stale_config_is_accepted_in_silence(self, caplog):
        """No unknown-key warning: the loader has never had one, and a retired
        knob is not the thing to introduce nagging for."""
        with caplog.at_level(logging.DEBUG):
            WorktreeConfig.from_dict({"worktree": {"auto_fetch": False}})

        assert caplog.records == []


class TestConfigPath:
    """Which directory config.toml is looked for in."""

    def test_the_variable_is_honoured_when_it_names_a_directory(self, monkeypatch, tmp_path):
        monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path))
        assert get_config_path() == tmp_path / "devlaunch" / "config.toml"

    def test_an_empty_value_means_unset_not_the_working_directory(self, monkeypatch):
        """A shell that exports the variable with no value is not asking for a
        relative path. Both readers go through devlaunch.xdg for this, so the
        directory the gh warning names is the one the loader actually reads."""
        monkeypatch.setenv("XDG_CONFIG_HOME", "")
        assert get_config_path() == Path.home() / ".config" / "devlaunch" / "config.toml"
