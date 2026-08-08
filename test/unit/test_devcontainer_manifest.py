"""The shape this repo's own devcontainer is required to have.

Three claims are load-bearing enough that a document contradicting them is a
bug: this devcontainer carries its own Docker daemon, it does not share the
host's network namespace, and it does not hand the container the developer's
`~/.ssh` directory. All three are settled on metal (blooop/devlaunch#94, #97,
#112); these tests are what stops them drifting back.

`pixi run test-e2e` is asserted through the task runner rather than by reading
the manifest back, because "there is a task called test-e2e" is the claim the
README makes to a reader, and only the runner can answer it.
"""

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
DEVCONTAINER_JSON = REPO_ROOT / ".devcontainer" / "devcontainer.json"
INITIALIZE_COMMAND = REPO_ROOT / ".devcontainer" / "claude-code" / "init-host.sh"
DIND_FEATURE = "ghcr.io/devcontainers/features/docker-in-docker:2"
CONTAINER_SSH_DIR = "/home/vscode/.ssh"
LOCAL_HOME = "${localEnv:HOME}"


def parse_mount(spec: str) -> dict:
    """Read one `source=…,target=…,type=…` mount string into its fields.

    Valueless words like `readonly` become keys with an empty value, so a
    caller can ask whether a mount is read-only without a second parser.
    """
    fields = {}
    for part in spec.split(","):
        key, _, value = part.partition("=")
        fields[key.strip()] = value.strip()
    return fields


def strip_jsonc_comments(text: str) -> str:
    """Remove `//` and `/* */` comments, leaving string literals alone.

    devcontainer.json is JSON with comments; `json` cannot read it directly.

    This is a comment stripper, not a JSONC parser, and it is deliberately
    *stricter* than a devcontainer runtime: what it leaves behind still has to
    satisfy `json.loads`, so a trailing comma -- which the spec allows and every
    real runtime accepts -- fails here while building fine. That is a house rule
    on one file we own, not a claim about what the runtime will take, and the
    tests below are worded to promise only the former.
    """
    out = []
    i = 0
    in_string = False
    while i < len(text):
        ch = text[i]
        if in_string:
            out.append(ch)
            if ch == "\\":
                if i + 1 < len(text):
                    out.append(text[i + 1])
                    i += 2
                    continue
            elif ch == '"':
                in_string = False
            i += 1
            continue
        if ch == '"':
            in_string = True
            out.append(ch)
            i += 1
            continue
        if text.startswith("//", i):
            end = text.find("\n", i)
            i = len(text) if end == -1 else end
            continue
        if text.startswith("/*", i):
            end = text.find("*/", i + 2)
            i = len(text) if end == -1 else end + 2
            continue
        out.append(ch)
        i += 1
    return "".join(out)


@pytest.fixture(name="devcontainer")
def devcontainer_fixture() -> dict:
    return json.loads(strip_jsonc_comments(DEVCONTAINER_JSON.read_text()))


@pytest.fixture(name="mounts")
def mounts_fixture(devcontainer) -> list:
    return [parse_mount(spec) for spec in devcontainer["mounts"]]


def test_devcontainer_manifest_is_this_repos_and_parses(devcontainer):
    """Comments stripped, this file is JSON, and it is the manifest we mean.

    The `name` assertion is the second half doing real work: every other test
    here reads through the same fixture, so if the path ever pointed at a
    different manifest they would all pass while checking nothing.
    """
    assert devcontainer["name"] == "devlaunch"


def test_devcontainer_enables_docker_in_docker(devcontainer):
    """The daemon is enabled, not commented out for a later day."""
    assert DIND_FEATURE in devcontainer["features"]


def test_devcontainer_does_not_share_the_host_network_namespace():
    """No run argument puts this container on the host's netns.

    Asserted against the raw text rather than the parsed object, because a
    re-added `--network=host` inside a comment is exactly the resurrection this
    guards: the flag was documented as required for a while, and the documents
    saying so were wrong.
    """
    raw = DEVCONTAINER_JSON.read_text()
    assert "--network=host" not in raw


def test_devcontainer_declares_no_run_args(devcontainer):
    assert "runArgs" not in devcontainer


def test_devcontainer_does_not_mount_the_developers_ssh_directory(mounts):
    """`/home/vscode/.ssh` stays container-local, so what devpod writes is too.

    devpod running *inside* this container writes `Host <id>.devpod` blocks into
    `$HOME/.ssh/config`, each with a `ProxyCommand` naming a binary that exists
    only in here. Handing it the directory means those blocks land on the
    developer's real config and outlive the container that could honour them.
    Keeping the directory container-local makes them die with it.

    The target is compared exactly rather than by prefix: mounting individual
    files *underneath* that path is the whole point of the arrangement, so a
    prefix check would forbid the fix along with the defect.
    """
    assert not [mount for mount in mounts if mount.get("target") == CONTAINER_SSH_DIR]


def test_agent_socket_is_mounted_where_the_container_looks_for_it(devcontainer, mounts):
    """`SSH_AUTH_SOCK` names a path something actually puts a socket at.

    The environment variable and the mount list are two halves of one fact, and
    nothing else checks that they agree. Once the directory around it is no
    longer mounted, a stale `SSH_AUTH_SOCK` no longer fails loudly at container
    start -- it fails at the first `git push`, in a container that came up fine.
    """
    auth_sock = devcontainer["containerEnv"]["SSH_AUTH_SOCK"]
    assert auth_sock in {mount.get("target") for mount in mounts}


def test_initialize_command_creates_the_ssh_files_this_manifest_mounts(mounts, tmp_path):
    """Every mounted ssh file exists before the container is asked to start.

    Docker creates nothing for a *file* bind source: a missing one aborts the
    start outright with `bind source path does not exist`, so a `known_hosts`
    the host happens not to have is not a degraded container but no container.
    The `initializeCommand` runs on the host first and is where that is fixed.

    Stated as a rule over the mount list rather than as one filename, so adding
    a fourth ssh file cannot quietly reintroduce the abort. The agent socket is
    the standing exception and the only one: it is owned by the developer's
    running ssh-agent, and a script that fabricated it would be manufacturing
    the absence of key forwarding rather than repairing it.
    """
    env = dict(os.environ, HOME=str(tmp_path))
    subprocess.run([str(INITIALIZE_COMMAND)], env=env, check=True)

    ssh_prefix = f"{LOCAL_HOME}/.ssh/"
    agent_socket_sources = {
        mount["source"] for mount in mounts if mount.get("target", "").endswith("/agent.sock")
    }
    for mount in mounts:
        source = mount.get("source", "")
        if not source.startswith(ssh_prefix) or source in agent_socket_sources:
            continue
        created = tmp_path / source[len(f"{LOCAL_HOME}/") :]
        assert created.exists(), f"{source} is mounted but the initializeCommand does not create it"


def test_devcontainer_registers_a_devpod_provider_for_its_own_daemon(devcontainer):
    """A container that cannot reach a provider cannot run `dl` at all.

    `dl`'s only subprocess is `devpod`, and a container starts with an empty
    `~/.devpod`: `devpod up` there exits 1 with "no default provider found"
    before it does anything. Registering the provider on create is what makes
    the daemon this container carries usable by the tool it was added for.
    """
    assert "dev-add-docker" in devcontainer["postCreateCommand"]


def test_pixi_resolves_a_test_e2e_task():
    """`pixi run test-e2e` is a task that exists, per the task runner itself."""
    pixi = shutil.which("pixi")
    assert pixi is not None, "pixi is the project's task runner and must be on PATH"
    result = subprocess.run(
        [pixi, "task", "list", "--manifest-path", str(REPO_ROOT / "pyproject.toml")],
        capture_output=True,
        text=True,
        check=True,
    )
    tasks = result.stdout.split()
    assert "test-e2e" in tasks
