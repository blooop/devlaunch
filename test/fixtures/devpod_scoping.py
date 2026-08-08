"""Keep the test suite's devpod state off the developer's machine.

devpod's workspace namespace lives in `~/.devpod`, and no `XDG_*` variable
moves it. That matters because `dl --purge` deletes *every* workspace `devpod
list` returns and an e2e test exercises exactly that, for real: run the e2e
suite on a machine with real workspaces on it and they are gone. The
`-m 'not e2e'` default in pyproject is the only thing standing in the way, and
a default is not a safeguard.

So the suite gives itself a devpod home of its own and points devpod at it
through the process environment, before any test runs. Two variables are needed
rather than one -- `DEVPOD_HOME` does not cover the ssh config, which devpod
resolves against the real `$HOME` regardless.

Scoping rather than guarding is deliberate. A guard is an assertion some future
test can forget to make; this applies to every subprocess the session spawns,
including the `devpod list` inside `dl --purge`, because they all inherit this
environment. It also makes the suite's hardcoded `e2e-test-*` workspace ids
private to the run, so two concurrent runs stop force-deleting each other's
workspaces -- the directory is new every time.
"""

import os
import tempfile
from pathlib import Path

DEVPOD_HOME_VAR = "DEVPOD_HOME"
DEVPOD_SSH_CONFIG_VAR = "DEVPOD_SSH_CONFIG"


def make_scoped_devpod_home(parent: Path) -> Path:
    """Create a devpod home that belongs to this run and no other."""
    return Path(tempfile.mkdtemp(prefix="devlaunch-testrun-", dir=parent))


def scope_devpod_to_this_run() -> Path:
    """Point every devpod subprocess in this process at a private namespace.

    Returns the devpod home it created.

    The directory is deliberately not cleaned up afterwards. It holds the
    metadata devpod needs to find and delete the containers a run created, so
    deleting it after a crashed run would leave those containers orphaned with
    no way to reach them -- worse than a stale directory the OS will reap.
    """
    devpod_home = make_scoped_devpod_home(Path(tempfile.gettempdir()))
    os.environ[DEVPOD_HOME_VAR] = str(devpod_home)
    os.environ[DEVPOD_SSH_CONFIG_VAR] = str(devpod_home / "ssh_config")
    return devpod_home
