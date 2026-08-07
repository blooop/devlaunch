"""Shared pytest configuration and fixtures for devlaunch tests.

This module provides:
- Test markers for unit, integration, and e2e tests
- Shared fixtures imported from test/fixtures/
- pytest configuration hooks
"""

import sys
import tempfile
from pathlib import Path

import pytest

# Add test directory to path for imports
test_dir = Path(__file__).parent
if str(test_dir) not in sys.path:
    sys.path.insert(0, str(test_dir))

# Import fixtures from the fixtures package to make them available to all tests
# Note: pytest automatically discovers fixtures in conftest.py
# noqa: E402 - imports must come after sys.path modification
from devlaunch import dl, gh_auth  # noqa: E402
from fixtures.git_fixtures import (  # noqa: E402
    isolated_devlaunch_env,
    local_git_repo,
    local_git_repo_with_devcontainer,
    real_managers,
)
from fixtures.devpod_mock import DevPodMock, mock_devpod  # noqa: E402
from fixtures.e2e_helpers import dl_no_ide, devpod_cleanup  # noqa: E402


@pytest.fixture(autouse=True)
def no_gh_token_forwarding(monkeypatch):
    """Keep the suite away from the host's real GitHub credentials.

    Token forwarding runs on every workspace_up and workspace_ssh, so without
    this the tests would shell out to `gh` and their assertions would depend on
    whether the machine running them happens to be logged in. Tests that cover
    forwarding opt back in by patching gh_auth directly.
    """
    monkeypatch.setenv(gh_auth.DISABLE_VAR, "1")
    gh_auth.resolve_token.cache_clear()
    yield
    gh_auth.resolve_token.cache_clear()


@pytest.fixture(autouse=True)
def isolated_completion_cache(monkeypatch):
    """Give each test its own freshly written completion cache.

    Two pieces of dl's refresh scheduling are per-process, which in a test
    session means per-*session*: the "already spawned a refresh" latch, and the
    TTL check that reads the cache file. Left alone, one test's spawn would
    silence the next one's, and whether a refresh looked necessary would depend
    on the age of the developer's real ~/.cache/devlaunch/completions.json. A
    per-test cache that starts out fresh also means no test spawns a real
    background git sweep unless it deliberately backdates the file.

    It gets a directory of its own rather than `tmp_path`, which tests are
    entitled to assert the exact contents of.
    """
    with tempfile.TemporaryDirectory() as tmpdir:
        cache = Path(tmpdir) / "completions.json"
        cache.write_text('{"workspaces": [], "repos": [], "owners": [], "branches": []}')
        monkeypatch.setattr(dl, "CACHE_FILE", cache)
        monkeypatch.setattr(dl, "BASH_CACHE_FILE", Path(tmpdir) / "completions.bash")
        dl.reset_cache_refresh_state()
        yield
        dl.reset_cache_refresh_state()


def pytest_configure(config):
    """Register custom markers."""
    config.addinivalue_line(
        "markers",
        "unit: Pure logic tests with no external commands. Fast, runs everywhere.",
    )
    config.addinivalue_line(
        "markers",
        "integration: Real git commands, mocked DevPod. Catches git errors and path issues.",
    )
    config.addinivalue_line(
        "markers",
        "e2e: Full E2E with Docker-in-Docker. Real DevPod creating real containers.",
    )


def pytest_collection_modifyitems(config, items):  # noqa: ARG001  # pylint: disable=unused-argument
    """Automatically mark tests based on their location."""
    for item in items:
        # Get the test file path relative to the test directory
        test_path = str(item.fspath)

        if "/test/unit/" in test_path:
            item.add_marker(pytest.mark.unit)
        elif "/test/integration/" in test_path:
            item.add_marker(pytest.mark.integration)
        elif "/test/e2e/" in test_path:
            item.add_marker(pytest.mark.e2e)


# Re-export fixtures so they're available without explicit imports
__all__ = [
    "isolated_devlaunch_env",
    "local_git_repo",
    "local_git_repo_with_devcontainer",
    "real_managers",
    "DevPodMock",
    "mock_devpod",
    "dl_no_ide",
    "devpod_cleanup",
]
