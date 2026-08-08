"""Make the run's private devpod home usable by real devpod commands.

The root conftest points `DEVPOD_HOME` at a directory created for this run. A
brand new devpod home has no providers in it at all, so nothing can be brought
up there until one is installed. Doing that here, autouse and session-scoped,
means every test in this directory gets it without asking and no other kind of
test pays for it.
"""

import shutil
import subprocess

import pytest


@pytest.fixture(scope="session", autouse=True)
def docker_provider_in_scoped_devpod_home():
    """Install the docker provider into this run's devpod home."""
    if shutil.which("devpod") is None:
        return

    subprocess.run(
        ["devpod", "provider", "add", "docker", "--use"],
        capture_output=True,
        check=False,
    )
