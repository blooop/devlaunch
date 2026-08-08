"""The shape this repo's own devcontainer is required to have.

Two claims are load-bearing enough that a document contradicting them is a bug:
this devcontainer carries its own Docker daemon, and it does not share the host's
network namespace. Both are settled on metal (blooop/devlaunch#94, #97); these
tests are what stops them drifting back.

`pixi run test-e2e` is asserted through the task runner rather than by reading
the manifest back, because "there is a task called test-e2e" is the claim the
README makes to a reader, and only the runner can answer it.
"""

import json
import shutil
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
DEVCONTAINER_JSON = REPO_ROOT / ".devcontainer" / "devcontainer.json"
DIND_FEATURE = "ghcr.io/devcontainers/features/docker-in-docker:2"


def strip_jsonc_comments(text: str) -> str:
    """Remove `//` and `/* */` comments, leaving string literals alone.

    devcontainer.json is JSON with comments; `json` cannot read it directly.
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


def test_devcontainer_manifest_is_readable_as_jsonc(devcontainer):
    """A devcontainer runtime has to be able to parse this file at all."""
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
