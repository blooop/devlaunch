"""Shared pytest configuration and fixtures for devlaunch tests.

This module provides:
- Test markers for unit, integration, and e2e tests
- Shared fixtures imported from test/fixtures/
- pytest configuration hooks

What this suite is, since the Python implementation was retired (#267): the
acceptance harness. It judges the shipped binaries from outside -- through the
`DEVLAUNCH_DL_CMD` seam, against a real devpod or the fake one on PATH -- plus the
repo's own artifacts (README claims, the devcontainer manifest, `scripts/`).
Nothing in here reaches inside `dl` any more, because there is no longer an inside
to reach: it is a subprocess.

That is also why several fixtures are gone rather than ported. `isolated_completion_cache`,
`fresh_workspace_list_cache` and `fresh_clone_manager` all existed to defeat
*per-process memoization* -- `dl` ran in this interpreter, so one test's memo
answered the next test's question. A binary spawned per test has no memo to
share, and the cache it reads is decided by `XDG_CACHE_HOME`, which the
`isolated_devlaunch_cache` fixture below still scopes.
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
)
from devpod_scoping import scope_devpod_to_this_run  # noqa: E402
from fixtures.e2e_helpers import dl_no_ide, devpod_cleanup  # noqa: E402
from fixtures.shim_fixtures import devpod_shim  # noqa: E402

# The environment variables `dl` reads that this suite has an opinion about.
# Spelled out here because the binary owns them now and a test process cannot
# import its constants; each is the literal `rust/devlaunch-core` declares.
#
# `gh.rs`'s DISABLE_VAR:
NO_GH_TOKEN_VAR = "DEVLAUNCH_NO_GH_TOKEN"
# `timing.rs`'s ENV_VAR, HANDOFF_VAR and PREWARM_VAR:
TIMING_VAR = "DEVLAUNCH_TIMING"
TIMING_HANDOFF_VAR = "DEVLAUNCH_HANDOFF_T0"
TIMING_PREWARM_VAR = "DEVLAUNCH_PREWARM_FIRED_AT"
# The dotfiles-on-attach switch, as `dl --help` documents it:
DOTFILES_ON_ATTACH_VAR = "DEVLAUNCH_DOTFILES_ON_ATTACH"


@pytest.fixture(autouse=True)
def isolated_devlaunch_cache(tmp_path_factory, monkeypatch):
    """Keep every test out of the developer's real ~/.cache/devlaunch.

    That cache holds workspace clones with uncommitted work in them, and the first
    command that touches a workspace path can rename those directories and rewrite
    metadata.json. A test reaching that path with XDG_CACHE_HOME unset does it to
    the machine running the suite. Reading the developer's cache was already wrong
    -- assertions that depend on whichever repos they happen to have cloned -- but
    writing to it is a different order of wrong, so the whole suite gets its own
    cache.

    XDG_CONFIG_HOME goes with it: config.toml can point repos_dir back at the real
    cache, which would defeat the isolation from the other direction. The scratch-run
    recipe in AGENTS.md trades that guard away rather than contradicting it -- scoping
    the variable hides the host's gh login, which the suite has already given up in
    no_gh_token_forwarding below and a real `dl-next` run has not.
    """
    root = tmp_path_factory.mktemp("xdg")
    monkeypatch.setenv("XDG_CACHE_HOME", str(root / "cache"))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(root / "config"))


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

    Token forwarding runs on every launch and every attach, so without this the
    binary under test would shell out to `gh` and assertions would depend on
    whether the machine running them happens to be logged in. A test that covers
    forwarding sets the variable back itself.
    """
    monkeypatch.setenv(NO_GH_TOKEN_VAR, "1")


@pytest.fixture(autouse=True)
def timing_switch_off(monkeypatch):
    """Start every test with the timing switch off, whatever the shell says.

    DEVLAUNCH_TIMING makes dl append a timing summary to stderr, and a good few
    tests assert stderr is empty. The developers this instrument exists for are
    the ones who will have it exported when they run the suite, so the suite
    scrubs it rather than going red in their shell. A test that wants the switch
    on sets it itself.

    Both seam stamps go with it: they are exported by whatever hands off to
    dl, and a developer whose shell carries them would otherwise see a handoff
    stage, or a prewarm report, in tests that ask for the summary and have no
    reason to expect either. The pair is scrubbed together because they are
    one seam — scrubbing the handoff stamp alone leaves the trap set for the
    next test that observes an attach shape without stamping the environment
    itself.
    """
    for variable in (TIMING_VAR, TIMING_HANDOFF_VAR, TIMING_PREWARM_VAR):
        monkeypatch.delenv(variable, raising=False)
    # The dotfiles-on-attach switch is scrubbed for the same reason as the
    # timing switch above: the developers who opt in are the ones running the
    # suite with it exported, and two attach-shape pins go red in exactly
    # their shells. A test that wants the refresh on sets it itself.
    monkeypatch.delenv(DOTFILES_ON_ATTACH_VAR, raising=False)


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
        elif "/test/e2e/" in test_path:
            item.add_marker(pytest.mark.e2e)


# Re-export fixtures so they're available without explicit imports
__all__ = [
    "isolated_devlaunch_cache",
    "isolated_devlaunch_env",
    "local_git_repo",
    "local_git_repo_with_devcontainer",
    "dl_no_ide",
    "devpod_cleanup",
    "devpod_shim",
]
