"""Whether the claude-code feature's read-only mounts are read-only in a container.

The unit tests beside this one state what the feature's manifest *says*. They
cannot answer the question this file exists for, which is whether a devcontainer
**Feature's** mount entry carrying `readonly` reaches Docker as a read-only bind.
That is not a formality. The published Feature schema has no `readonly` at all --
a Feature mount there is an object of `source`, `target` and `type`, with
`additionalProperties: false`, and no string form -- while every implementation
in use accepts the `source=…,target=…,type=bind,readonly` string this feature is
written in. A protection whose only evidence is a schema that does not describe
it is a protection worth measuring.

**The developer's own Claude configuration is never in the blast radius.** The
container is built against a `HOME` this test created, so `${localEnv:HOME}`
resolves into a scratch directory, and "a write from the container is refused"
can be demonstrated by actually attempting the write instead of by reading flags
and hoping. A run of this test cannot touch `~/.claude`, passing or failing.

That scratch `HOME` earns its keep twice, because it is also a host that has
never run Claude: the pre-create hook creating every mounted path from nothing is
what this exercises, and a missing bind source is not a warning -- devpod refuses
to create the container at all, measured, with `bind mount source path does not
exist`.

**What this deliberately does not exercise.** The project is a copy of the
shipped feature directory with its installer replaced by a stub. The mount list
under test is the shipped one, byte for byte; installing the CLI is a different
claim, it needs a base image with `curl` and a network round trip to a package
index, and nothing about it changes where a mount lands.
"""

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

from fixtures.e2e_helpers import create_e2e_workspace
from unit.test_claude_code_feature_mounts import (
    CONFIG_DIRNAME,
    FEATURE_DIR,
    FEATURE_JSON,
    READ_ONLY_HEADING,
    READ_WRITE_HEADING,
    documented_paths,
)

WORKSPACE_ID = "e2e-test-claude-config-protection"
RENAME_WORKSPACE_ID = "e2e-test-claude-config-rename"

# Small, and already on any machine that has run this suite. It needs no `curl`
# and no `vscode` user because the installer is stubbed out: what has to be real
# here is the mount plumbing, and that is the same for every image.
BASE_IMAGE = "debian:bookworm-slim"

INSTALLER_STUB = """#!/bin/sh
echo "stubbed installer: this workspace exists to test mounts, not installation"
"""


def in_container(command: str, workspace: str = WORKSPACE_ID) -> subprocess.CompletedProcess:
    """Run a command in the workspace, failing loudly with its output.

    devpod reports a failed remote command as its own exit 1 and buries the
    remote error in its log stream, so a bare returncode comparison says only
    that something went wrong somewhere in the tunnel.
    """
    result = subprocess.run(
        ["devpod", "ssh", workspace, "--command", command],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, (
        f"in-container command exited {result.returncode}: {command}\n"
        f"stdout: {result.stdout[-2000:]}\nstderr: {result.stderr[-2000:]}"
    )
    return result


def consumer_project(root: Path) -> Path:
    """A devcontainer that consumes the shipped feature, and nothing else.

    A local Feature has to live under the project that uses it, so the feature
    directory is copied rather than referenced. It is copied whole and then has
    exactly one file overwritten, so that a change to the mount list is
    exercised here without anyone remembering to update this file.

    The pre-create hook is wired up the way this repo's own devcontainer wires
    it, because a Feature cannot declare one: that a consumer has to do this is
    part of what is being tested, not a detail of the fixture.
    """
    project = root / "consumer"
    (project / ".devcontainer").mkdir(parents=True)
    shutil.copytree(FEATURE_DIR, project / ".devcontainer" / FEATURE_DIR.name)
    installer = project / ".devcontainer" / FEATURE_DIR.name / "install.sh"
    installer.write_text(INSTALLER_STUB)
    installer.chmod(0o755)
    (project / ".devcontainer" / "devcontainer.json").write_text(
        json.dumps(
            {
                "image": BASE_IMAGE,
                "initializeCommand": f".devcontainer/{FEATURE_DIR.name}/init-host.sh",
                "features": {f"./{FEATURE_DIR.name}": {}},
            }
        )
    )
    return project


@pytest.fixture(name="workspace_cleanup")
def workspace_cleanup_fixture(devpod_cleanup):
    """Delete devpod's root-owned scratch from the project folder, from inside it.

    devpod writes `.devpod-internal` into the folder it mounts as the workspace,
    owned by the container's root. Nothing outside can remove it, so pytest's own
    tear-down of its temporary directories fails on it -- once per run, forever,
    for every later run too. The container can remove it, because in there that
    root is the user, and this runs while the container is still alive: it is
    torn down before `devpod_cleanup`, which is what deletes it.
    """
    yield devpod_cleanup
    subprocess.run(
        [
            "devpod",
            "ssh",
            WORKSPACE_ID,
            "--command",
            f"rm -rf /workspaces/{WORKSPACE_ID}/.devcontainer/.devpod-internal",
        ],
        capture_output=True,
        check=False,
    )


@pytest.mark.e2e
@pytest.mark.creates_workspace
def test_the_container_cannot_write_the_host_files_the_feature_protects(
    workspace_cleanup, tmp_path
):
    """A prompt injection in this container cannot reach the host's instructions.

    The four checks are separate claims and all four are needed.

    *Refused* is the one that matters, and it is behavioural: a real write, from
    inside the container, at each protected path. Directories are probed with a
    new file rather than an existing one, so nothing has to be overwritten to
    find out.

    *Unchanged* is what stops a refusal being read too generously. A container
    that quietly wrote into a copy -- a `type=volume` mount, or a target the
    manifest and the environment disagree about -- would also leave the host's
    bytes alone, so the two together say the write reached the real file and
    was stopped there.

    *Writable* is the positive control, and it is the half a security test is
    likeliest to be missing: mounting nothing at all passes every read-only
    assertion ever written. Claude has to be able to refresh a token and record
    that onboarding was seen, and it has to be able to write its own session
    state into the configuration directory around these mounts, or the
    protection has been bought by breaking the tool.

    *Survivable* is the one that only a read-only filesystem can answer, and the
    unit tests approximate it with modification times because they have no such
    filesystem to hand. The pre-create hook runs again, in here, where every path
    it would create is a read-only mount -- which is what a container that builds
    containers of its own does -- and its exit status is the assertion, because a
    non-zero pre-create hook aborts `devpod up` rather than degrading it.
    """
    home = tmp_path / "home"
    home.mkdir()
    project = consumer_project(tmp_path)

    create_e2e_workspace(
        str(project),
        WORKSPACE_ID,
        cleanup=workspace_cleanup,
        env={**os.environ, "HOME": str(home)},
    )

    # Taken from the README rather than from the manifest, and that is the point
    # of the file. The manifest is the thing on trial: a mount that quietly lost
    # its `readonly` would drop out of a list derived from it and stop being
    # probed, so the list comes from the document that promises the protection.
    config_dir = json.loads(FEATURE_JSON.read_text())["containerEnv"]["CLAUDE_CONFIG_DIR"]
    protected = documented_paths(READ_ONLY_HEADING)
    writable = documented_paths(READ_WRITE_HEADING)
    host_config = home / CONFIG_DIRNAME

    before = {name: host_config.joinpath(name).stat().st_mtime_ns for name in protected}

    for name in protected:
        # The README's trailing slash says whether this is a directory, and a
        # directory is probed with a new file: finding out costs nothing that
        # was already there.
        probe = f"{config_dir}/{name}injected.md" if name.endswith("/") else f"{config_dir}/{name}"
        attempt = in_container(
            f'if echo injected >> "{probe}" 2>/dev/null; then echo accepted; else echo refused; fi'
        )
        assert "accepted" not in attempt.stdout, f"the container wrote through the {name} mount"
        assert "refused" in attempt.stdout

    for name, mtime in before.items():
        host_path = host_config / name
        assert host_path.stat().st_mtime_ns == mtime, f"the container changed the host's {name}"
        if host_path.is_dir():
            assert not list(host_path.iterdir()), f"the container added a file to the host's {name}"

    for name in writable:
        in_container(f'echo "{{}}" > "{config_dir}/{name}"')
    in_container(f'mkdir -p "{config_dir}/projects"')

    # Run under `sh -e`, which the hook is not run under in production, and that
    # is deliberate rather than sloppy. The hook takes its exit status from its
    # last command, so a write that fails in the middle of it prints an error and
    # is otherwise invisible -- exactly the failure worth catching, and the one a
    # bare run cannot report. `-e` turns "attempted a write it could not make"
    # into an exit status, which is the claim, without asserting on the wording
    # of an operating system's error message.
    in_container(
        f"cd /workspaces/{WORKSPACE_ID} "
        f'&& HOME="$(dirname "{config_dir}")" sh -e .devcontainer/{FEATURE_DIR.name}/init-host.sh'
    )


@pytest.mark.e2e
@pytest.mark.creates_workspace
def test_the_container_follows_the_host_replacing_a_file_by_rename(devpod_cleanup, tmp_path):
    """The container reads what the host has now, after the host swaps the inode.

    This is the defect the layout exists to prevent, and it cannot be observed
    anywhere but here. Claude does not edit `.credentials.json` in place: it
    writes a new file and renames it over the old one, which is what a token
    refresh and a change of account both do. A bind mount of that *file* is
    attached to the dentry, so the rename leaves the mount pointing at an inode
    with no name -- the container reads it happily, forever, and a developer who
    switches account finds every running workspace still authenticating as the
    account they left.

    The mount of the *directory* is what fixes it, because a directory mount
    resolves names on each access. So the assertion is about a file with no mount
    of its own, read through the directory that has one, across a rename.

    The protected directories are re-probed afterwards for the reverse failure.
    A read-only mount nested in a read-write parent is exactly the arrangement
    that used to be relied on for `CLAUDE.md` and `settings.json`, and a *file*
    mount there does not survive this rename either -- it leaves the namespace
    and the path falls through to the writable parent, so the protection is gone
    with the manifest still advertising it. Directory mounts survive, which is
    why the read-only list is only directories, and this is what says so.
    """
    home = tmp_path / "home"
    home.mkdir()
    project = consumer_project(tmp_path)
    host_config = home / CONFIG_DIRNAME

    create_e2e_workspace(
        str(project),
        RENAME_WORKSPACE_ID,
        cleanup=devpod_cleanup,
        env={**os.environ, "HOME": str(home)},
    )

    config_dir = json.loads(FEATURE_JSON.read_text())["containerEnv"]["CLAUDE_CONFIG_DIR"]
    state = next(iter(documented_paths(READ_WRITE_HEADING)))

    (host_config / state).write_text('{"account": "before"}')
    assert "before" in in_container(f'cat "{config_dir}/{state}"', RENAME_WORKSPACE_ID).stdout

    # Precisely what Claude does, and the reason a plain overwrite would not do:
    # the temporary file is a different inode, and the rename is what moves the
    # name onto it.
    replacement = host_config / f"{state}.tmp"
    replacement.write_text('{"account": "after"}')
    replacement.replace(host_config / state)

    seen = in_container(f'cat "{config_dir}/{state}"', RENAME_WORKSPACE_ID).stdout
    assert "after" in seen, (
        f"the container still reads the pre-rename {state}: it is pinned to a dead inode, "
        f"which is a host account switch reaching no running workspace"
    )

    for name in documented_paths(READ_ONLY_HEADING):
        probe = f"{config_dir}/{name}injected.md"
        attempt = in_container(
            f'if echo injected >> "{probe}" 2>/dev/null; then echo accepted; else echo refused; fi',
            RENAME_WORKSPACE_ID,
        )
        assert "accepted" not in attempt.stdout, (
            f"the {name} mount stopped protecting the host after an unrelated rename"
        )
