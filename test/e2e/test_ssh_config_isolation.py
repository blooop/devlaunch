"""Whether a container built from this repo can reach the developer's ssh config.

devpod running *inside* this devcontainer writes a `Host <id>.devpod` block into
`$HOME/.ssh/config` for every workspace it brings up, and the block's
`ProxyCommand` names a devpod binary that exists only inside that container.
While the host's whole `~/.ssh` was bind-mounted, those blocks landed on the
developer's real config and outlived the container that could have honoured
them.

The test below is shaped by one fact that makes the obvious version useless:
**a lifecycle that ends in `devpod delete` passes against the broken
configuration too**, because delete unconditionally removes the block it added.
Only *abandonment* -- a nested workspace still alive when its container is
destroyed -- tells the two configurations apart, so abandonment is not a detail
of the test, it is the test.

Two supports keep a green run from being vacuous. In-container liveness, so a
configuration that reaches nothing at all cannot pass by being inert. And a
positive control inside the container, so "the host's file did not change" is
read as *the block went somewhere else* rather than *no block was ever written*.

It is safe against a real ssh config by construction: what it asserts is that
nothing was written, so a passing run has written nothing.
"""

import subprocess
from pathlib import Path

import pytest

from fixtures.e2e_guard import opt_out
from fixtures.e2e_helpers import create_e2e_workspace

REPO_ROOT = Path(__file__).resolve().parents[2]
HOST_SSH_DIR = Path.home() / ".ssh"
HOST_SSH_CONFIG = HOST_SSH_DIR / "config"
HOST_AGENT_SOCKET = HOST_SSH_DIR / "agent.sock"

OUTER_WORKSPACE_ID = "e2e-ssh-isolation"
NESTED_WORKSPACE_ID = "e2e-ssh-isolation-nested"

if not HOST_AGENT_SOCKET.exists():
    opt_out(
        "no ssh agent socket at the path this devcontainer mounts, so the "
        "container cannot be built on this host at all",
        module_level=True,
    )

# Reaching GitHub is done from outside any working copy. `git ls-remote <url>`
# still asks where it is before it asks what it was told to fetch, so run from
# a checkout whose `.git` is a worktree pointer -- which every worktree's is --
# and it exits 128 on the dangling gitdir without ever opening a connection.
REACH_GITHUB = "cd / && git ls-remote git@github.com:blooop/devlaunch.git HEAD"

# A nested workspace has one job here -- to make the devpod inside the container
# write an ssh config block -- so it is the smallest image that can hold a
# devpod agent rather than anything resembling this project's own container.
NESTED_UP = f"""
set -eu
mkdir -p /tmp/nested/.devcontainer
printf '{{"image":"alpine:3.19"}}' > /tmp/nested/.devcontainer/devcontainer.json
devpod up /tmp/nested --id {NESTED_WORKSPACE_ID} --ide none
"""


def ssh_config_state():
    """Every byte of the developer's ssh config, or None if they have no file.

    Absence is carried as a value rather than skipped over, because "devpod
    created a config for a developer who had none" is the same defect as
    "devpod appended to the one they had", and a test that only knew how to
    compare bytes would miss it.
    """
    try:
        return HOST_SSH_CONFIG.read_bytes()
    except FileNotFoundError:
        return None


def devpod_blocks(state) -> int:
    """How many devpod-written stanzas a config holds, for the failure message.

    Byte equality is the assertion; this exists so a failure says *what* landed
    rather than only that something did. Nothing from the file itself is ever
    put in the message: it is somebody's real ssh config.
    """
    return (state or b"").count(b"DevPod Start")


def in_container(command: str) -> subprocess.CompletedProcess:
    """Run a command in the outer workspace, failing loudly with its output.

    devpod reports a failed remote command as its own exit 1 and buries the
    remote error in its log stream, so a bare returncode comparison says only
    that something went wrong somewhere in the tunnel.
    """
    result = subprocess.run(
        ["devpod", "ssh", OUTER_WORKSPACE_ID, "--command", command],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, (
        f"in-container command exited {result.returncode}: {command}\n"
        f"stdout: {result.stdout[-2000:]}\nstderr: {result.stderr[-2000:]}"
    )
    return result


@pytest.mark.e2e
@pytest.mark.creates_workspace
def test_abandoned_nested_workspace_cannot_reach_the_developers_ssh_config(devpod_cleanup):
    """A nested workspace outliving its container leaves the host's config alone."""
    before = ssh_config_state()

    create_e2e_workspace(str(REPO_ROOT), OUTER_WORKSPACE_ID, cleanup=devpod_cleanup)

    # Forwarding survives the narrowed mount, and the two mounted files are
    # separately load-bearing: the socket is what authenticates, known_hosts is
    # what stops the connection being refused before authentication. Without
    # this pair, a container that mounted nothing of ~/.ssh at all would pass
    # everything below.
    identities = in_container("ssh-add -l")
    assert identities.stdout.strip(), "the forwarded agent holds no identities"
    in_container(REACH_GITHUB)

    in_container(NESTED_UP)

    # The positive control: devpod really did write a block, in there.
    written = in_container(f'grep -c "{NESTED_WORKSPACE_ID}.devpod" "$HOME/.ssh/config"')
    assert int(written.stdout.strip()) > 0

    # Abandonment. The nested workspace is never deleted -- destroying its
    # container is what a developer closing a branch actually does, and it is
    # the only sequence the two configurations disagree about.
    destroyed = subprocess.run(
        ["devpod", "delete", OUTER_WORKSPACE_ID, "--force"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert destroyed.returncode == 0, f"could not destroy the outer workspace: {destroyed.stderr}"

    after = ssh_config_state()
    assert after == before, (
        "the developer's ssh config changed while a nested workspace was "
        f"abandoned: it held {devpod_blocks(before)} devpod stanzas before and "
        f"{devpod_blocks(after)} after"
    )
