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
from devpod_scoping import scope_devpod_to_this_run  # noqa: E402
from fixtures.devpod_mock import DevPodMock, mock_devpod  # noqa: E402
from fixtures.e2e_helpers import dl_no_ide, devpod_cleanup  # noqa: E402
from fixtures.permissions import (  # noqa: E402
    refuses_access,
    refuses_reads,
    refuses_writes,
)


@pytest.fixture(autouse=True)
def isolated_devlaunch_cache(tmp_path_factory, monkeypatch):
    """Keep every test out of the developer's real ~/.cache/devlaunch.

    That cache holds workspace clones with uncommitted work in them, and since the
    id-scheme migration landed, the first command that touches a workspace path
    renames those directories and rewrites metadata.json. A test reaching that path
    with XDG_CACHE_HOME unset does it to the machine running the suite:
    `test_workspace_delete` did, because workspace_delete() builds a real clone
    manager. Reading the developer's cache was already wrong -- assertions that
    depend on whichever repos they happen to have cloned -- but writing to it is a
    different order of wrong, so the whole suite gets its own cache.

    XDG_CONFIG_HOME goes with it: config.toml can point repos_dir back at the real
    cache, which would defeat the isolation from the other direction. The scratch-run
    recipe in AGENTS.md trades that guard away rather than contradicting it -- scoping
    the variable hides the host's gh login, which the suite has already given up in
    no_gh_token_forwarding below and a real `dl-next` run has not.

    The few tests that assert what the *unset* default location is opt out with the
    `home_cache_default` fixture.
    """
    root = tmp_path_factory.mktemp("xdg")
    monkeypatch.setenv("XDG_CACHE_HOME", str(root / "cache"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(root / "config"))
    # dl memoizes the clone manager because a real invocation is one short-lived
    # command; the memo is bound to the cache directory that was current when it
    # was built. A session that moves that directory per test has to move the
    # memo with it, or one test reads and migrates the previous test's cache.
    dl.reset_clone_manager()
    yield
    dl.reset_clone_manager()


@pytest.fixture
def home_cache_default(monkeypatch):
    """Drop XDG_CACHE_HOME so the home-relative fallback is what gets tested."""
    monkeypatch.delenv("XDG_CACHE_HOME", raising=False)


@pytest.fixture(autouse=True)
def no_interactive_git_credentials(monkeypatch):
    """Make a git command that wants a password fail instead of waiting for one.

    A test that reaches a real remote is already wrong, but the way it goes wrong
    decides whether anyone finds out: prompting, it sits there until some
    deadline elsewhere gives up, and the run reads as slow rather than as having
    left the machine. Refusing to prompt turns that into an immediate failure at
    the command that did it. Nothing here needs a prompt -- the suite's git work
    is on local paths, and token auth is unaffected.
    """
    monkeypatch.setenv("GIT_TERMINAL_PROMPT", "0")


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


@pytest.fixture(autouse=True)
def fresh_workspace_list_cache():
    """Give every test its own `devpod list` snapshot.

    list_workspaces() memoizes for the life of the process because a real dl
    invocation is one short-lived command. A test session is not: without this,
    the first test to read the list would answer every later test's read.
    """
    dl.invalidate_workspace_list_cache()
    yield
    dl.invalidate_workspace_list_cache()


@pytest.fixture(autouse=True)
def fresh_clone_manager():
    """Give every test its own clone manager, bound to its own cache.

    _get_clone_manager() memoizes for the life of the process, and the manager
    it holds is bound to the XDG_CACHE_HOME that isolated_devlaunch_cache set
    up for whichever test built it first. Left alone, every later test's reads
    and writes of the cache go to a directory that belongs to a test that has
    already finished -- and, worse, a test asserting that some code path did
    *not* touch the cache passes for the wrong reason, because the memo makes
    the construction free no matter where it is called from. That is not a
    hypothetical: it silently neutered the warm-launch guard in
    test_devpod_spawn_counts.py, which was green in file order with the
    regression it guards against fully reintroduced.

    This is autouse rather than something a test asks for because the test that
    must not forget it is the one nobody has written yet.
    """
    dl.invalidate_clone_manager()
    yield
    dl.invalidate_clone_manager()


def pytest_configure():
    """Scope this run's devpod state.

    This happens here, before collection, rather than in a fixture: a fixture is
    something a test has to ask for, and the test that must not forget is the
    one nobody has written yet. Everything the session spawns inherits this
    process's environment, so one assignment covers the whole suite -- including
    the `devpod list` that decides what `dl --purge` deletes.

    The markers are registered in `[tool.pytest.ini_options] markers` and not
    also here: two copies of the same sentence is one that goes stale.
    """
    scope_devpod_to_this_run()


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
    "isolated_devlaunch_cache",
    "home_cache_default",
    "isolated_devlaunch_env",
    "local_git_repo",
    "local_git_repo_with_devcontainer",
    "real_managers",
    "DevPodMock",
    "mock_devpod",
    "dl_no_ide",
    "devpod_cleanup",
    "refuses_access",
    "refuses_reads",
    "refuses_writes",
]
