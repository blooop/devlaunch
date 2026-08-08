"""The suite must never be able to reach the developer's real devpod state.

`dl --purge` deletes every workspace `devpod list` returns, and an e2e test runs
it for real. `devpod list` reads `~/.devpod`, which no `XDG_*` variable
relocates, so the suite's existing XDG isolation buys nothing against it: a
`pytest -m e2e` on a developer's machine destroys their whole workspace list.

What stands between the two is the process environment every subprocess in this
session inherits, set once before collection. These tests run on the *default*
suite -- not under `-m e2e` -- so removing the scoping breaks the ordinary test
run rather than waiting for the run that would do the damage.
"""

import os
from pathlib import Path

from fixtures.devpod_scoping import make_scoped_devpod_home


def test_devpod_commands_are_pointed_away_from_the_developers_devpod_home():
    """DEVPOD_HOME is what scopes `devpod list`, and so `dl --purge`'s blast radius."""
    scoped = os.environ.get("DEVPOD_HOME")

    assert scoped, "the suite must set DEVPOD_HOME; without it devpod reads ~/.devpod"

    scoped_path = Path(scoped).resolve()
    home = Path.home().resolve()
    assert scoped_path != home / ".devpod"
    assert home not in scoped_path.parents


def test_devpod_ssh_config_is_pointed_away_from_the_developers_ssh_config():
    """`devpod up` rewrites ~/.ssh/config unless DEVPOD_SSH_CONFIG redirects it.

    DEVPOD_HOME does not cover this: devpod resolves the ssh config against the
    real home either way.
    """
    scoped = os.environ.get("DEVPOD_SSH_CONFIG")

    assert scoped, "the suite must set DEVPOD_SSH_CONFIG; without it devpod edits ~/.ssh/config"

    scoped_path = Path(scoped).resolve()
    home = Path.home().resolve()
    assert scoped_path != home / ".ssh" / "config"
    assert home not in scoped_path.parents


def test_each_run_gets_a_devpod_home_of_its_own(tmp_path):
    """Two concurrent runs must not share a namespace.

    The e2e workspace ids are hardcoded constants, so a namespace shared between
    runs means each run's teardown force-deletes the other's workspaces. A
    per-run directory is what makes those constants private to the run.
    """
    first = make_scoped_devpod_home(tmp_path)
    second = make_scoped_devpod_home(tmp_path)

    assert first != second
