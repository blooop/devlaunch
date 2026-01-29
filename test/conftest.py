"""Shared pytest configuration and fixtures for devlaunch tests.

This module provides:
- Test markers for unit, integration, and e2e tests
- Shared fixtures imported from test/fixtures/
- pytest configuration hooks
"""

import sys
from pathlib import Path

import pytest

# Add test directory to path for imports
test_dir = Path(__file__).parent
if str(test_dir) not in sys.path:
    sys.path.insert(0, str(test_dir))

# Import fixtures from the fixtures package to make them available to all tests
# Note: pytest automatically discovers fixtures in conftest.py
# noqa: E402 - imports must come after sys.path modification
from fixtures.git_fixtures import (  # noqa: E402
    isolated_devlaunch_env,
    local_git_repo,
    local_git_repo_with_devcontainer,
    real_managers,
)
from fixtures.devpod_mock import DevPodMock, mock_devpod  # noqa: E402
from fixtures.e2e_helpers import dl_no_ide, devpod_cleanup  # noqa: E402


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


def pytest_collection_modifyitems(config, items):
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
