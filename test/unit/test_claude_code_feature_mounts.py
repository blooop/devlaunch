"""The protection the claude-code feature documents, held against what it mounts.

The feature's README describes a read-write bind of `~/.claude` with the
subdirectories holding *executable instructions* -- `agents/`, `commands/`,
`hooks/`, `skills/` and `wf-skills/` -- mounted read-only on top of it, and gives
the reason: those are files a prompt injection that edits one of them is not
confined by. The edit is on the host, and it runs again in every later session,
in every other container, on the developer's own machine.

Two failures have to be prevented here, and they pull in opposite directions.

**A protection that is documented and absent.** The manifest once mounted the
whole configuration directory read-write while the README described granular
read-only mounts ([#108](https://github.com/blooop/devlaunch/issues/108)), so
none of it was true. The assertions below are written as *agreement* between the
README and the manifest rather than as a list of paths kept here: a third copy of
the list is a third thing to forget, and a path added to one side and not the
other has to fail.

**A protection that is real on the day it is written and gone by Tuesday.** This
is the one that is not obvious from reading a manifest, and it is why
`test_every_mount_source_is_a_directory` exists. A bind mount of a *file* does
not survive its source being replaced by rename -- the mount hangs off the
dentry, the rename puts a new one at that name, and the mount leaves the
namespace. Under the read-write parent this feature needs, a read-only file
mount is therefore enforced only until the developer next edits the file, after
which the path falls through to the writable parent and `docker inspect` still
lists a mount that is no longer there.

That is measured rather than reasoned about, both ways round, because the
direction of the failure follows the parent and neither direction is safe:
a read-write parent leaves the file writable, a read-only parent leaves it
read-only and breaks a token refresh. A mount of a *directory* survives the same
rename intact. So the rule the manifest is held to is not "these paths are
read-only" but "every source is a directory", which is the form of the rule that
cannot rot -- and the tempting change it rejects is naming `CLAUDE.md` and
`settings.json` in the mount list, which passes review, appears in `docker
inspect`, and stops being true on the next edit.

The same rename is what the read-write directory mount is *for*. A file mount
pinned the inode that existed when the container was created, so an account
switch on the host reached no running container: it went on authenticating as the
account the developer had left. Only a directory mount resolves names per access.

What is asserted here is the *text* of the manifest. Whether a devcontainer
Feature's `readonly` mount really reaches Docker as a read-only bind, and whether
it survives the rename, are claims about a running container, and are settled in
the e2e suite, not here.
"""

import json
import os
import re
from pathlib import Path

import pytest

from fixtures.markdown_sections import section as _section
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
# to `mkdir` and a file it must not mount. The README's trailing slash is
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


@pytest.fixture(name="host_config")
def host_config_fixture(tmp_path) -> Path:
    """A host `~/.claude` as the pre-create hook leaves it, on a fresh machine.

    Every test that asks what *kind* of thing a mount source is needs a host to
    look at, and this is the only honest one to use: the hook is what creates
    these paths on a machine that has never run Claude, so the answer it gives is
    the answer Docker will get.
    """
    devcontainer = json.loads(strip_jsonc_comments(DEVCONTAINER_JSON.read_text()))
    result = run_initialize_command(devcontainer, tmp_path)
    assert result.returncode == 0, result.stderr
    return tmp_path / CONFIG_DIRNAME


def resolve(source: str, host_config: Path) -> Path:
    """A manifest mount source as a path under the test's scratch home."""
    assert source.startswith(HOST_CONFIG_DIR), f"{source} is outside the configuration directory"
    return (
        host_config / source[len(f"{HOST_CONFIG_DIR}/") :]
        if source != HOST_CONFIG_DIR
        else host_config
    )


def nested_sources(mounts: list) -> dict:
    """Each mount *inside* the configuration directory, keyed by relative path.

    The bind of the configuration directory itself is excluded, because every
    rule below says something different about it: it is the one mount that is
    supposed to be writable, and the one whose relative path is empty.
    """
    granular = {
        mount["source"][len(f"{HOST_CONFIG_DIR}/") :]: mount
        for mount in mounts
        if mount.get("source", "").startswith(f"{HOST_CONFIG_DIR}/")
    }
    assert granular, "the feature mounts nothing under the host's configuration directory"
    return granular


def config_dir_mount(mounts: list) -> dict:
    """The bind of `~/.claude` itself."""
    for mount in mounts:
        if mount.get("source") == HOST_CONFIG_DIR:
            return mount
    raise AssertionError(
        "the feature does not mount the configuration directory itself, so the files "
        "reached through it -- credentials above all -- are not mounted at all"
    )


def test_every_mount_is_a_bind_of_a_host_path(mounts):
    """Each entry binds a real host path, rather than a volume of its own.

    A `type=volume` entry at the same target would leave the container with a
    plausible-looking, empty configuration and no connection to the host at all,
    which is a mount list that passes every other test in this file: the source
    still names a directory that exists, and the read-only flags still line up
    with the README.
    """
    assert mounts, "the feature mounts nothing"
    for mount in mounts:
        assert mount.get("type") == "bind", f"{mount.get('target')} is not a bind mount"


def test_every_mount_source_is_a_directory(mounts, host_config):
    """No mount names a file, whatever flags it would carry.

    This is the rule that keeps the read-only list honest, and it is stated over
    *all* mounts rather than only the read-only ones because both directions of
    the file-mount failure are bugs. A read-only file mount under this feature's
    read-write parent is a protection that ends at the developer's next edit; a
    read-write file mount is how the container came to hold a dead inode and go on
    authenticating as an account the developer had already left.

    Asserted against a host the pre-create hook has just set up, so "is a
    directory" is measured rather than inferred from the path's spelling.
    """
    for mount in mounts:
        source = resolve(mount["source"], host_config)
        assert source.is_dir(), (
            f"{mount['source']} is mounted but is not a directory. A bind mount of a file "
            f"does not survive its source being replaced by rename: the mount leaves the "
            f"namespace and the path falls through to the parent mount, taking any "
            f"'readonly' with it. Mount the directory that contains it instead."
        )


def test_the_configuration_directory_is_mounted_read_write(mounts):
    """`~/.claude` itself is bound, and is not read-only.

    Two claims in one, because each fails differently. Without the mount, the
    files that have no mount of their own -- `.credentials.json`, `.claude.json` --
    are not in the container at all, and Claude comes up logged out. With it
    read-only, they are there and current but a token refresh cannot write them,
    which is a login prompt on every launch.
    """
    assert "readonly" not in config_dir_mount(mounts)


def test_the_paths_documented_as_protected_are_exactly_the_read_only_mounts(mounts):
    """The README's read-only list and the manifest's read-only mounts agree.

    Set equality in both directions, because the two failures are equally bad and
    look nothing alike: a documented protection with no mount behind it is the
    defect this ticket reports, and an undocumented read-only mount is a file the
    container cannot write for reasons nobody wrote down.
    """
    read_only = {path for path, mount in nested_sources(mounts).items() if "readonly" in mount}
    documented = {path.rstrip("/") for path in documented_paths(READ_ONLY_HEADING)}
    assert read_only == documented


def test_nothing_nested_inside_the_configuration_directory_is_writable(mounts):
    """Every mount over the directory mount is read-only.

    A read-write mount nested inside a read-write parent buys nothing -- the path
    is already writable through the parent -- and costs the thing the parent was
    chosen for, because it is a file mount again and goes stale on the first
    rename. So the only reason to nest is to take write access away, and a nested
    mount that does not is a mistake with no upside to weigh it against.
    """
    writable = {path for path, mount in nested_sources(mounts).items() if "readonly" not in mount}
    assert not writable, (
        f"{sorted(writable)} are mounted read-write inside a read-write mount, which only "
        f"pins them to a stale inode"
    )


def test_the_paths_documented_as_writable_have_no_mount_of_their_own(mounts):
    """The writable state files are reached through the directory, not bound directly.

    This is the regression in its original form, asserted from the README's side.
    Both files were mounted individually and read-write, which is how a container
    ended up reading the inode that existed when it was created: an account switch
    on the host changed the file by rename, and the container never saw it.

    They still have to be *documented* as writable -- the README argues for each,
    token refresh and onboarding state, and that argument is what a reviewer
    weighs -- so the check is that the argument survives while the mount does not.
    """
    documented = {path.rstrip("/") for path in documented_paths(READ_WRITE_HEADING)}
    assert documented, "the README no longer says which files must stay writable"
    mounted = set(nested_sources(mounts))
    assert not (documented & mounted), (
        f"{sorted(documented & mounted)} are documented as writable and mounted individually; "
        f"a file mount pins the inode, so the container stops seeing host changes to them"
    )


def test_each_mount_lands_where_the_feature_tells_claude_to_look(feature, mounts):
    """Every host path arrives at the same relative place under `CLAUDE_CONFIG_DIR`.

    The feature sets that variable and mounts these paths, and nothing else
    checks that the two agree. They can disagree quietly: a mount whose target
    drifts leaves Claude reading a path that exists, is empty, and is nobody's
    configuration -- a container that comes up fine and behaves as though the
    host had never been set up.
    """
    config_dir = feature["containerEnv"]["CLAUDE_CONFIG_DIR"]
    assert config_dir_mount(mounts)["target"] == config_dir
    for relative, mount in nested_sources(mounts).items():
        assert mount.get("target") == f"{config_dir}/{relative}"


def test_the_pre_create_hook_creates_every_host_path_the_feature_mounts(mounts, host_config):
    """The mounted paths exist on the host before the container is asked to start.

    A missing bind source is not a degraded container: the create is refused
    outright with `bind mount source path does not exist`, measured on devpod
    0.26.1.

    The hook is read out of this repo's devcontainer manifest rather than named
    here, so repointing or deleting it fails this test instead of leaving it
    passing about a script nothing runs. A Feature cannot declare a host-side
    hook of its own; wiring this one up is the consuming devcontainer's job, and
    this is where that stays true.
    """
    for mount in mounts:
        source = resolve(mount["source"], host_config)
        assert source.exists(), f"{mount['source']} is mounted but the hook does not create it"


def test_the_pre_create_hook_creates_no_file_the_feature_does_not_mount(host_config):
    """It seeds no `{}` placeholders for paths nothing binds any more.

    The empty `.credentials.json` and `.claude.json` this used to write existed
    only to satisfy a bind source. With the files no longer mounted individually,
    a missing one cannot refuse the create, and Claude writes each itself on first
    use -- while an empty credentials file on a host that has never run Claude is
    indistinguishable from a logged-out session.
    """
    stray = [path.name for path in host_config.iterdir() if path.is_file()]
    assert not stray, f"the hook creates {sorted(stray)}, which nothing mounts"


def test_the_pre_create_hook_leaves_a_configuration_that_already_exists_alone(mounts, tmp_path):
    """It creates what is missing and writes to nothing else, mtimes included.

    Two reasons, and the second is what makes this a container-starting concern
    rather than good manners. The developer's real configuration is on the other
    end of these mounts, and a hook that rewrites it is a hook that eats
    somebody's setup. And this hook has to survive running *inside* a container it
    built -- which is the whole point of giving that container a Docker daemon --
    where the instruction directories are the read-only mounts, so a write fails
    with EROFS, and a non-zero pre-create hook aborts `devpod up` rather than
    degrading it.

    Modification times are the assertion because a read-only filesystem is not
    something a unit test can conjure, while "wrote to a path that was already
    there" is exactly what fails on one and is observable anywhere.
    """
    devcontainer = json.loads(strip_jsonc_comments(DEVCONTAINER_JSON.read_text()))
    run_initialize_command(devcontainer, tmp_path)

    existing = [tmp_path / CONFIG_DIRNAME / relative for relative in nested_sources(mounts)]
    assert existing, "no configuration was created, so this asserts nothing"
    for path in existing:
        os.utime(path, ns=(0, 0))

    result = run_initialize_command(devcontainer, tmp_path)
    assert result.returncode == 0, result.stderr
    for path in existing:
        assert path.stat().st_mtime_ns == 0, f"the hook rewrote {path.name}"
