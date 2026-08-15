"""The protection the claude-code feature documents, held against what it mounts.

The feature's README describes granular mounts, with `CLAUDE.md`,
`settings.json`, `agents/`, `commands/` and `hooks/` read-only, and gives the
reason: those files are *executable instructions*. A prompt injection that edits
one of them is not confined to the session that fell for it -- it is on the
host, and it runs again in every later session, in every other container, on the
developer's own machine. The two state files stay writable because refreshing an
OAuth token and recording that onboarding has been seen are writes Claude really
makes, and a read-only mount there is a login prompt on every launch.

The manifest mounted the whole configuration directory read-write, so none of
that was true ([#108](https://github.com/blooop/devlaunch/issues/108)); the
protection was documented and absent from the first commit. These tests are what
stops the two drifting apart again, and they are deliberately written as
*agreement* between the README and the manifest rather than as a list of paths
kept here. A third copy of the list is a third thing to forget: a path added to
one side and not the other has to fail, which a test carrying its own expected
set cannot notice.

What is asserted here is the *text* of the manifest. Whether a devcontainer
Feature's `readonly` mount really reaches Docker as a read-only bind is a claim
about a running container, and is settled in the e2e suite, not here.
"""

import json
import os
import re
from pathlib import Path

import pytest

from test_lending_doc import _section
from unit.test_devcontainer_manifest import (
    parse_mount,
    run_initialize_command,
    strip_jsonc_comments,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
FEATURE_DIR = REPO_ROOT / ".devcontainer" / "claude-code"
FEATURE_JSON = FEATURE_DIR / "devcontainer-feature.json"
FEATURE_README = FEATURE_DIR / "README.md"
DEVCONTAINER_JSON = REPO_ROOT / ".devcontainer" / "devcontainer.json"

LOCAL_HOME = "${localEnv:HOME}"
CONFIG_DIRNAME = ".claude"
HOST_CONFIG_DIR = f"{LOCAL_HOME}/{CONFIG_DIRNAME}"

READ_ONLY_HEADING = "### Read-Only Mounts (Security-Protected)"
READ_WRITE_HEADING = "### Read-Write Mounts (Authentication & State)"

# `~/.claude/agents/` and `~/.claude/CLAUDE.md` differ by one character, and that
# character is the whole difference between a directory the pre-create hook has
# to `mkdir` and a file it has to create. The README's trailing slash is
# therefore read as a declaration rather than as typography.
DOCUMENTED_PATH = re.compile(r"^- `~/\.claude/(?P<path>[^`]+)`")


def documented_paths(heading: str) -> set:
    """The `~/.claude/...` paths the README lists under one mount heading.

    Only the leading code span of a bullet counts. Prose under the heading
    mentions these files too, and a test that matched anywhere in the section
    would be satisfied by a sentence *about* a mount that no longer exists.
    """
    paths = {
        match.group("path")
        for match in (
            DOCUMENTED_PATH.match(line) for line in _section(FEATURE_README, heading).splitlines()
        )
        if match
    }
    assert paths, f"the README lists no mounts under {heading!r}"
    return paths


@pytest.fixture(name="feature")
def feature_fixture() -> dict:
    return json.loads(FEATURE_JSON.read_text())


@pytest.fixture(name="mounts")
def mounts_fixture(feature) -> list:
    return [parse_mount(spec) for spec in feature["mounts"]]


def relative_sources(mounts: list) -> dict:
    """Each mount's host path relative to the configuration directory.

    Keyed by that relative path so it can be compared against what the README
    lists, and carrying the mount so the comparison can also ask whether it is
    read-only.

    Empty is a failure rather than an empty answer. Every caller below states a
    rule over these paths, and a rule over nothing is a test that passes about a
    feature that mounts nothing at all -- which is the shape the whole-directory
    mount this file exists to prevent would leave behind.
    """
    granular = {
        mount["source"][len(f"{HOST_CONFIG_DIR}/") :]: mount
        for mount in mounts
        if mount.get("source", "").startswith(f"{HOST_CONFIG_DIR}/")
    }
    assert granular, "the feature mounts nothing under the host's configuration directory"
    return granular


def test_the_feature_does_not_hand_the_container_the_whole_configuration_directory(mounts):
    """No mount is the configuration directory itself.

    This is the defect in its original form: one bind of `~/.claude`, read-write,
    which makes every file under it writable from the container no matter what
    any other mount says. Asserted on the source rather than the target because
    it is the host's directory that is at stake, and the two are named
    separately.
    """
    assert HOST_CONFIG_DIR not in {mount.get("source") for mount in mounts}


def test_every_mount_is_a_bind_of_a_host_path_under_the_configuration_directory(mounts):
    """Each entry binds a real host path, rather than a volume of its own.

    A `type=volume` entry at the same target would leave the container with a
    plausible-looking, empty configuration and no connection to the host at all,
    which is a mount list that passes every other test in this file.
    """
    assert mounts, "the feature mounts nothing"
    for mount in mounts:
        assert mount.get("type") == "bind", f"{mount.get('target')} is not a bind mount"
        assert mount.get("source", "").startswith(f"{HOST_CONFIG_DIR}/"), (
            f"{mount.get('source')} is outside the host's configuration directory"
        )


def test_the_paths_documented_as_protected_are_exactly_the_read_only_mounts(mounts):
    """The README's read-only list and the manifest's read-only mounts agree.

    Set equality in both directions, because the two failures are equally bad and
    look nothing alike: a documented protection with no mount behind it is the
    defect this ticket reports, and an undocumented read-only mount is a file the
    container cannot write for reasons nobody wrote down.
    """
    read_only = {path for path, mount in relative_sources(mounts).items() if "readonly" in mount}
    documented = {path.rstrip("/") for path in documented_paths(READ_ONLY_HEADING)}
    assert read_only == documented


def test_the_paths_documented_as_writable_are_exactly_the_writable_mounts(mounts):
    """Nothing is writable from the container that the README has not argued for.

    The README makes a specific case for each writable file -- token refresh, and
    onboarding state -- and that case is what a reviewer weighs. A file that
    became writable without one is the interesting failure here, so the assertion
    is equality rather than containment.
    """
    writable = {path for path, mount in relative_sources(mounts).items() if "readonly" not in mount}
    documented = {path.rstrip("/") for path in documented_paths(READ_WRITE_HEADING)}
    assert writable == documented


def test_each_mount_lands_where_the_feature_tells_claude_to_look(feature, mounts):
    """Every host path arrives at the same relative place under `CLAUDE_CONFIG_DIR`.

    The feature sets that variable and mounts these files, and nothing else
    checks that the two agree. They can disagree quietly: a mount whose target
    drifts leaves Claude reading a path that exists, is empty, and is nobody's
    configuration -- a container that comes up fine and behaves as though the
    host had never been set up.
    """
    config_dir = feature["containerEnv"]["CLAUDE_CONFIG_DIR"]
    for relative, mount in relative_sources(mounts).items():
        assert mount.get("target") == f"{config_dir}/{relative}"


def test_the_pre_create_hook_creates_every_host_path_the_feature_mounts(mounts, tmp_path):
    """The mounted paths exist on the host before the container is asked to start.

    A missing bind source is not a degraded container. Docker creates a
    *directory* in its place, so a host that has never run Claude gets a
    directory named `CLAUDE.md` in its own configuration folder -- breaking the
    host's Claude, not only the container's -- and a file mount onto it fails the
    start outright.

    The hook is read out of this repo's devcontainer manifest rather than named
    here, so repointing or deleting it fails this test instead of leaving it
    passing about a script nothing runs. A Feature cannot declare a host-side
    hook of its own; wiring this one up is the consuming devcontainer's job, and
    this is where that stays true.

    Whether each path is a file or a directory is taken from the README's
    trailing slash, so the document that tells a developer what to create is the
    same one this checks the hook against.
    """
    devcontainer = json.loads(strip_jsonc_comments(DEVCONTAINER_JSON.read_text()))
    result = run_initialize_command(devcontainer, tmp_path)
    assert result.returncode == 0, result.stderr

    directories = {
        path.rstrip("/")
        for heading in (READ_ONLY_HEADING, READ_WRITE_HEADING)
        for path in documented_paths(heading)
        if path.endswith("/")
    }
    for relative in relative_sources(mounts):
        created = tmp_path / CONFIG_DIRNAME / relative
        assert created.exists(), f"{relative} is mounted but the hook does not create it"
        assert created.is_dir() == (relative in directories), (
            f"{relative} was created as the wrong kind of thing for what the README documents"
        )


def test_the_pre_create_hook_leaves_a_configuration_that_already_exists_alone(mounts, tmp_path):
    """It creates what is missing and writes to nothing else, mtimes included.

    Two reasons, and the second is what makes this a container-starting concern
    rather than good manners. The developer's real `CLAUDE.md` and `settings.json`
    are on the other end of these mounts, and a hook that rewrites them is a hook
    that eats somebody's configuration. And this hook has to survive running
    *inside* a container it built -- which is the whole point of giving that
    container a Docker daemon -- where these files are the read-only mounts, so a
    write fails with EROFS, and a non-zero pre-create hook aborts `devpod up`
    rather than degrading it.

    Modification times are the assertion because a read-only filesystem is not
    something a unit test can conjure, while "wrote to a file that was already
    there" is exactly what fails on one and is observable anywhere.
    """
    devcontainer = json.loads(strip_jsonc_comments(DEVCONTAINER_JSON.read_text()))
    run_initialize_command(devcontainer, tmp_path)

    existing = [tmp_path / CONFIG_DIRNAME / relative for relative in relative_sources(mounts)]
    assert existing, "no configuration was created, so this asserts nothing"
    for path in existing:
        os.utime(path, ns=(0, 0))

    result = run_initialize_command(devcontainer, tmp_path)
    assert result.returncode == 0, result.stderr
    for path in existing:
        assert path.stat().st_mtime_ns == 0, f"the hook rewrote {path.name}"
