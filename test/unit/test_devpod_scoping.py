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
import tempfile
from pathlib import Path

from devpod_scoping import (
    DEVPOD_HOME_VAR,
    DEVPOD_SSH_CONFIG_VAR,
    scope_devpod_to_this_run,
)


def test_devpod_commands_are_pointed_away_from_the_developers_devpod_home():
    """DEVPOD_HOME is what scopes `devpod list`, and so `dl --purge`'s blast radius."""
    scoped = os.environ.get(DEVPOD_HOME_VAR)

    assert scoped, "the suite must set DEVPOD_HOME; without it devpod reads ~/.devpod"

    scoped_path = Path(scoped).resolve()
    home = Path.home().resolve()
    assert scoped_path != home / ".devpod"
    assert home not in scoped_path.parents


def test_devpod_up_is_pointed_away_from_the_developers_ssh_config():
    """`devpod up` rewrites ~/.ssh/config unless DEVPOD_SSH_CONFIG redirects it.

    DEVPOD_HOME does not cover this: devpod resolves the ssh config against the
    real home either way.

    The claim is deliberately narrower than the variable's name suggests.
    `--ssh-config` is registered on `devpod up` alone -- measured on devpod
    v0.26.1, `ssh`, `delete` and `stop` do not accept it and still read the
    developer's real `~/.ssh/config`. `up` is the subcommand that *writes*, so
    the harm is covered; nothing here claims the variable is global.
    """
    scoped = os.environ.get(DEVPOD_SSH_CONFIG_VAR)

    assert scoped, (
        "the suite must set DEVPOD_SSH_CONFIG; without it `devpod up` edits ~/.ssh/config"
    )

    scoped_path = Path(scoped).resolve()
    home = Path.home().resolve()
    assert scoped_path != home / ".ssh" / "config"
    assert home not in scoped_path.parents


def test_each_run_gets_a_devpod_home_of_its_own(tmp_path, monkeypatch):
    """Two concurrent runs must not share a namespace.

    The e2e workspace ids are hardcoded constants, so a namespace shared between
    runs means each run's teardown force-deletes the other's workspaces. A
    per-run directory is what makes those constants private to the run.

    This drives `scope_devpod_to_this_run` itself, twice, rather than the
    directory-making it happens to use -- so it goes red if the per-run
    directory is ever "simplified" into a fixed path, which is the regression it
    exists to catch. `tempfile.tempdir` sends the two throwaway homes under
    `tmp_path`, and `monkeypatch.setenv` restores the session's real scoping
    afterwards, since the function under test writes to `os.environ` by design.
    """
    monkeypatch.setattr(tempfile, "tempdir", str(tmp_path))
    monkeypatch.setenv(DEVPOD_HOME_VAR, "")
    monkeypatch.setenv(DEVPOD_SSH_CONFIG_VAR, "")

    first = scope_devpod_to_this_run()
    first_env = os.environ[DEVPOD_HOME_VAR]
    second = scope_devpod_to_this_run()
    second_env = os.environ[DEVPOD_HOME_VAR]

    assert first != second
    assert first_env != second_env
    assert os.environ[DEVPOD_SSH_CONFIG_VAR] == str(second / "ssh_config")
