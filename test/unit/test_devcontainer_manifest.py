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
DIND_FEATURE = "ghcr.io/devcontainers/features/docker-in-docker:2"
CONTAINER_SSH_DIR = "/home/vscode/.ssh"
LOCAL_HOME = "${localEnv:HOME}"
SSH_SOURCE_PREFIX = f"{LOCAL_HOME}/.ssh/"


def parse_mount(spec: str) -> dict:
    """Read one `source=…,target=…,type=…` mount string into its fields.

    Valueless words like `readonly` become keys mapped to the empty string, so
    the question to ask of one is `"readonly" in mount` and never
    `mount.get("readonly")` -- the latter is falsy for a flag that is present.
    """
    fields = {}
    for part in spec.split(","):
        key, _, value = part.partition("=")
        fields[key.strip()] = value.strip()
    return fields


def ssh_file_mounts(mounts: list, agent_socket: str) -> list:
    """The mounted ssh files the container only ever reads.

    The agent socket is excluded everywhere it appears below, and always for the
    same reason: it is owned by the developer's running ssh-agent rather than by
    this repo. Nothing can create one, and connecting to one is a write.
    """
    return [
        mount
        for mount in mounts
        if mount.get("source", "").startswith(SSH_SOURCE_PREFIX)
        and mount.get("target") != agent_socket
    ]


def run_initialize_command(devcontainer: dict, home: Path) -> subprocess.CompletedProcess:
    """Run the manifest's own `initializeCommand` against a scratch HOME.

    Taken from the manifest rather than named here, so repointing or deleting
    the hook is a test failure instead of a test that keeps passing about a
    script nothing runs any more. Invoked the way a devcontainer runtime invokes
    it -- through `sh -c`, from the workspace folder -- because that is what
    decides whose exit status becomes the hook's.
    """
    return subprocess.run(
        ["sh", "-c", devcontainer["initializeCommand"]],
        cwd=REPO_ROOT,
        env=dict(os.environ, HOME=str(home)),
        capture_output=True,
        text=True,
        check=False,
    )


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


def test_the_ssh_files_the_container_reads_are_mounted_read_only(devcontainer, mounts):
    """Nothing in the container may edit the developer's ssh files.

    Stated over the mount list rather than about `known_hosts` by name, so a
    fourth file cannot arrive writable. Read-only is not only a courtesy here:
    it is what the test below has to survive.
    """
    agent_socket = devcontainer["containerEnv"]["SSH_AUTH_SOCK"]
    read_only = ssh_file_mounts(mounts, agent_socket)
    assert read_only, "no ssh files are mounted, so this asserts nothing"
    for mount in read_only:
        assert "readonly" in mount, f"{mount['source']} is mounted writable"


def test_initialize_command_creates_the_ssh_files_this_manifest_mounts(
    devcontainer, mounts, tmp_path
):
    """Every mounted ssh file exists before the container is asked to start.

    Docker creates nothing for a *file* bind source: a missing one aborts the
    start outright with `bind source path does not exist`, so a `known_hosts`
    the host happens not to have is not a degraded container but no container.
    The `initializeCommand` runs on the host first and is where that is fixed.

    Stated as a rule over the mount list rather than as one filename, so adding
    a fourth ssh file cannot quietly reintroduce the abort.
    """
    agent_socket = devcontainer["containerEnv"]["SSH_AUTH_SOCK"]
    result = run_initialize_command(devcontainer, tmp_path)
    assert result.returncode == 0, result.stderr

    for mount in ssh_file_mounts(mounts, agent_socket):
        source = mount["source"]
        created = tmp_path / source[len(f"{LOCAL_HOME}/") :]
        assert created.exists(), f"{source} is mounted but the initializeCommand does not create it"


def test_initialize_command_leaves_ssh_files_that_already_exist_alone(
    devcontainer, mounts, tmp_path
):
    """It creates what is missing and touches nothing else -- including mtimes.

    This is what lets the container build itself. Run from *inside* it, which is
    the whole point of giving it a Docker daemon, the ssh files the hook wants
    to create are the read-only mounts, and writing to one fails with EROFS. The
    hook's last command is its exit status, and a non-zero `initializeCommand`
    aborts `devpod up` outright rather than degrading it -- so a hook that
    rewrites a file it finds is a container that cannot start.

    Asserted on modification times because a read-only filesystem is not
    something a unit test can conjure, while "wrote to a file that was already
    there" is exactly the behaviour that fails on one, and is observable
    anywhere. A `touch` that bumps an mtime is the failing case.

    The timestamps are set rather than read back, so the assertion does not
    depend on the filesystem's clock resolution being finer than the gap
    between two runs of a two-line script.
    """
    agent_socket = devcontainer["containerEnv"]["SSH_AUTH_SOCK"]
    run_initialize_command(devcontainer, tmp_path)

    existing = [
        tmp_path / mount["source"][len(f"{LOCAL_HOME}/") :]
        for mount in ssh_file_mounts(mounts, agent_socket)
    ]
    assert existing, "no ssh files were created, so this asserts nothing"
    for path in existing:
        os.utime(path, ns=(0, 0))

    result = run_initialize_command(devcontainer, tmp_path)
    assert result.returncode == 0, result.stderr
    for path in existing:
        assert path.stat().st_mtime_ns == 0, f"the initializeCommand rewrote {path.name}"


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
