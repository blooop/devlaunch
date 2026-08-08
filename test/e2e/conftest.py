"""Make the run's private devpod home usable by real devpod commands.

The root conftest points `DEVPOD_HOME` at a directory created for this run. A
brand new devpod home has no providers in it at all, so nothing can be brought
up there until one is installed. Doing that here, autouse and session-scoped,
means every test in this directory gets it without asking and no other kind of
test pays for it.
"""

import os
import subprocess

import pytest

from devpod_scoping import DEVPOD_HOME_VAR
from fixtures.e2e_helpers import devpod_available


@pytest.fixture(scope="session", autouse=True)
def docker_provider_in_scoped_devpod_home():
    """Install the docker provider into this run's devpod home.

    `--use` rewrites the *default provider* of whichever devpod home is live, so
    this is the one unconditional write to a devpod home the suite performs. Its
    safety rests entirely on the scoping the root conftest sets up, which makes
    it the one place worth asserting that scoping rather than assuming it: if
    DEVPOD_HOME ever stops being set, this fixture is what would reach into the
    developer's own ~/.devpod.

    The install is a precondition, not a teardown, so a failure is raised rather
    than swallowed -- otherwise every later e2e test fails against a
    provider-less devpod home with an unrelated error, while the stderr that
    explains it was captured and discarded.
    """
    assert os.environ.get(DEVPOD_HOME_VAR), (
        "refusing to add a devpod provider: DEVPOD_HOME is unset, so `--use` "
        "would rewrite the default provider in the developer's real ~/.devpod"
    )

    if not devpod_available():
        return

    result = subprocess.run(
        ["devpod", "provider", "add", "docker", "--use"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        pytest.fail(
            f"could not install the docker provider into {os.environ[DEVPOD_HOME_VAR]}:\n{result.stderr}"
        )
