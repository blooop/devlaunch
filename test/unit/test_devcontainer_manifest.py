"""The shape this repo's own devcontainer is required to have.

Three claims are load-bearing enough that a document contradicting them is a
bug: this devcontainer carries its own Docker daemon, it does not share the
host's network namespace, and it does not hand the container the developer's
`~/.ssh` directory. All three are settled on metal (blooop/devlaunch#94, #97,
#112); these tests are what stops them drifting back.

`pixi run test-e2e` is asserted through the task runner rather than by reading
the manifest back, because "there is a task called test-e2e" is the claim the
README makes to a reader, and only the runner can answer it.

The prebuild tests at the bottom are here for a different reason from the rest.
The others guard claims whose violation *fails*: no daemon, no provider, a
clobbered ssh file. A broken prebuild fails at nothing -- devpod treats every
unanswerable lookup as a cache miss and builds locally, exactly as it did before
prebuilds existed -- so the only symptom is that opening a container is slow
again, months later, with no commit to blame. These are the tests for a
regression that cannot announce itself.
"""

import contextlib
import json
import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest

# stdlib since 3.11, which is the floor now that nothing ships on an interpreter
# (#267). It was `tomli`, a runtime dependency of the Python `dl` that the tests
# borrowed and that went with it.
import tomllib

REPO_ROOT = Path(__file__).resolve().parents[2]
DEVCONTAINER_JSON = REPO_ROOT / ".devcontainer" / "devcontainer.json"
DIND_FEATURE = "ghcr.io/devcontainers/features/docker-in-docker:2"
CONTAINER_SSH_DIR = "/home/vscode/.ssh"
LOCAL_HOME = "${localEnv:HOME}"
SSH_SOURCE_PREFIX = f"{LOCAL_HOME}/.ssh/"
PREBUILD_REPOSITORY = "ghcr.io/blooop/devlaunch-devcontainer"
PREBUILD_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "devcontainer-prebuild.yml"
FEATURE_INSTALLER = REPO_ROOT / ".devcontainer" / "claude-code" / "install.sh"
SHIPPING_PROVISIONER = REPO_ROOT / "rust" / "devlaunch-core" / "src" / "flows" / "provision.rs"
PIXI_GLOBAL_INSTALL = "pixi global install "

DOCKERFILE = REPO_ROOT / ".devcontainer" / "Dockerfile"
CONTEXT_DIR = REPO_ROOT / ".devcontainer"
CONTEXT_DOCKERIGNORE = CONTEXT_DIR / ".dockerignore"

# Every file under the build context, split by whether the *image* can see it.
#
# devpod hashes the context as well as the config, and with includeFiles empty --
# which is what a Dockerfile with no COPY leaves it -- it hashes the whole
# directory (pkg/util/hash.DirectoryHash). So anything in here that the build
# cannot read is a file whose edits throw the published prebuild away and buy
# nothing, and `.dockerignore` is what keeps it out of the hash.
#
# Split by hand rather than derived, because "can the image see this" is not a
# property of the path: `install.sh` and `devcontainer-feature.json` are the
# local feature devpod bakes in, while `init-host.sh` sits beside them and runs
# on the host. The two sets together have to account for every file present, so
# a file added here is a test failure until somebody says which side it is on.
CONTEXT_BUILD_INPUTS = frozenset(
    {
        "Dockerfile",
        "claude-code/install.sh",
        "claude-code/devcontainer-feature.json",
    }
)
CONTEXT_NON_BUILD_INPUTS = frozenset(
    {
        # Prose, here because the Features spec wants it beside its feature.
        "claude-code/README.md",
        "claude-code/TROUBLESHOOTING.md",
        # `initializeCommand`, which runs on the host before the container
        # exists. devpod clears DevContainerActions out of the config half of
        # the hash for exactly that reason, then hashed the script back in as
        # context.
        "claude-code/init-host.sh",
        # devpod read this file to get here; it does not read it out of the
        # context, and the config half of the hash already carries every build
        # input it declares.
        "devcontainer.json",
    }
)
LOCKFILE = REPO_ROOT / "pixi.lock"
#: Lock-file format version -> the lowest pixi release that can read it.
#:
#: Measured, one release at a time, against this repository's own `pixi.lock`:
#: 0.67.0 refuses version 7 and 0.68.0 accepts it. Extend this table when the
#: format moves; the test below fails on an unknown version rather than guessing,
#: because a guess here is indistinguishable from the bug it exists to catch.
MIN_PIXI_FOR_LOCK_VERSION = {6: (0, 40, 0), 7: (0, 68, 0)}
PYPROJECT = REPO_ROOT / "pyproject.toml"
PIXI_LOCK = REPO_ROOT / "pixi.lock"

# The runner each architecture's prebuild is built on, and the pixi platform that
# runner's `pixi install` resolves. Both halves are load-bearing: the target
# architecture is hashed into the prebuild tag, so a missing leg is an
# architecture with no image, and a runner whose platform the workspace does not
# declare is a leg that cannot install its own tools.
PREBUILD_RUNNERS = {
    "ubuntu-latest": "linux-64",
    "ubuntu-24.04-arm": "linux-aarch64",
}


def parse_mount(spec: str) -> dict:
    """Read one `source=…,target=…,type=…` mount string into its fields.

    Valueless words like `readonly` become keys mapped to the empty string, so
    the question to ask of one is `"readonly" in mount` and never
    `mount.get("readonly")` -- the latter is falsy for a flag that is present.

    That question is only honest for the two spellings this refuses, and both
    were silent. `readonly=false` is a key with a value, so `"readonly" in
    mount` is True and a writable mount is counted as protected -- a documented
    protection with nothing behind it, which is what these tests exist to
    catch. Docker's own synonym `ro` parses to a different key, so a genuinely
    read-only mount is counted as writable. Refused here rather than handled,
    because a mount list has no reason to reach for either.
    """
    fields = {}
    for part in spec.split(","):
        key, _, value = part.partition("=")
        fields[key.strip()] = value.strip()
    assert fields.get("readonly", "") == "", (
        f"`readonly={fields['readonly']}` in {spec!r} is read as protected by "
        f'`"readonly" in mount` whatever it says; write the bare flag or drop it'
    )
    assert "ro" not in fields, (
        f"`ro` in {spec!r} is docker's synonym for `readonly` and is read as writable "
        f'by `"readonly" in mount`; spell it `readonly`'
    )
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


def host_path_mounts(mounts: list, agent_socket: str) -> list:
    """Every mount whose source is a path under the developer's home.

    The whole list and not one directory's worth of it, which is the repair for
    a guard that read `~/.ssh/` alone: `~/.config/gh` was mounted for a year
    with nothing creating it, and the first host that had never run
    `gh auth login` -- a fresh machine, so every host eventually -- could not
    start a container at all.

    The agent socket is *included*, unlike in `ssh_file_mounts`, and the
    difference is the point of its third design: nothing can create an agent,
    but the hook can always give the mount a source -- a symlink to the agent
    the host has, or a placeholder when it has none -- and this is the test
    that holds it to that on a host with neither. `agent_socket` is accepted so
    the two helpers read alike at their call sites and so a reader who expects
    the exclusion finds where it went rather than wondering.
    """
    del agent_socket
    return [mount for mount in mounts if mount.get("source", "").startswith(f"{LOCAL_HOME}/")]


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


@pytest.mark.parametrize("flag", ["readonly=false", "readonly=true", "ro"])
def test_a_readonly_spelling_the_membership_test_misreads_is_refused(flag):
    """The two ways `"readonly" in mount` lies, and the one that only looks safe.

    Every protection assertion in this suite is that membership test, so a
    spelling it misreads is a security claim that passes while being false in
    whichever direction the spelling chose. `readonly=true` is refused with
    them: it happens to read correctly, and keeping it out is what stops the
    question of whether the value is consulted from arising at all.
    """
    with pytest.raises(AssertionError):
        parse_mount(f"source=/a,target=/b,type=bind,{flag}")


def dockerignore_patterns() -> list:
    """The `.dockerignore` lines devpod's pattern matcher will actually see.

    Comments and blank lines are dropped, which is what `ignorefile.ReadAll`
    does before the patterns reach `patternmatcher`. Measured on devpod 0.26.1:
    with this file's three patterns the debug line reports `excludeCount=4`, the
    fourth being devpod's own feature-staging folder, which it appends itself.
    """
    lines = CONTEXT_DOCKERIGNORE.read_text().splitlines()
    return [line.strip() for line in lines if line.strip() and not line.startswith("#")]


def excluded_from_the_context_hash(rel_path: str) -> bool:
    """Whether `.dockerignore` keeps `rel_path` out of the context hash.

    A deliberately small matcher, covering only the two pattern forms this
    repository's `.dockerignore` uses: `**/*.ext`, which docker matches at any
    depth including the top level, and an exact context-relative path. Any other
    form raises rather than returning False, so this cannot quietly report "not
    excluded" about a pattern it does not understand -- which is the one way a
    guard like this fails without saying so.
    """
    for pattern in dockerignore_patterns():
        if pattern.startswith("**/*."):
            if rel_path.endswith(pattern[len("**/*") :]):
                return True
        elif "*" in pattern or "?" in pattern or "[" in pattern:
            raise AssertionError(
                f"{CONTEXT_DOCKERIGNORE.name} pattern {pattern!r} is a glob form this "
                "test cannot evaluate; teach it the form or express the exclusion as a path"
            )
        elif rel_path == pattern or rel_path.startswith(pattern + "/"):
            return True
    return False


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


def test_initialize_command_creates_every_host_path_this_manifest_mounts(
    devcontainer, mounts, tmp_path
):
    """Every mounted host path exists before the container is asked to start.

    Docker creates nothing for a bind source: a missing one aborts the start
    outright with `bind mount source path does not exist`, so a `known_hosts`
    the host happens not to have is not a degraded container but no container.
    The `initializeCommand` runs on the host first and is where that is fixed.

    Stated over the *whole* mount list, which is the point and is what this test
    did not used to do. It read `~/.ssh/` only, so it proved nothing about
    `~/.config/gh` -- mounted just below the files it was checking, created by
    nobody, and fatal on any host that had never run `gh auth login`. A fresh
    machine could not open this repo's own devcontainer, and the guard written
    to prevent exactly that passed.
    """
    agent_socket = devcontainer["containerEnv"]["SSH_AUTH_SOCK"]
    result = run_initialize_command(devcontainer, tmp_path)
    assert result.returncode == 0, result.stderr

    checked = host_path_mounts(mounts, agent_socket)
    assert checked, "no host paths are mounted, so this asserts nothing"
    for mount in checked:
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
    # Both streams: pixi prints the task table itself on stderr and only the
    # column header on stdout (0.63), and where a *human-facing listing* goes is
    # not a thing this test has an opinion about. Reading one stream made this
    # assert on pixi's output routing rather than on the task existing, which is
    # how it came to fail against a task list that plainly contained `test-e2e`.
    tasks = (result.stdout + " " + result.stderr).replace(",", " ").split()
    assert "test-e2e" in tasks


def test_devcontainer_build_context_stays_inside_the_devcontainer_directory(devcontainer):
    """The build context must not climb out of `.devcontainer`.

    devpod's prebuild tag is a hash of the build context as well as of the
    config, so a context of `..` -- which is what this was -- makes the tag move
    on every commit to any file in the repository, and no published prebuild can
    ever match one again. Nothing in the Dockerfile copies from the context, so
    widening it buys nothing to weigh against that.

    The failure mode is why this is a test rather than a note: a context that
    escapes does not break a launch, it silently returns every launch to building
    locally.
    """
    context = devcontainer["build"]["context"]
    resolved = (DEVCONTAINER_JSON.parent / context).resolve()
    devcontainer_dir = DEVCONTAINER_JSON.parent.resolve()
    assert resolved == devcontainer_dir or devcontainer_dir in resolved.parents, (
        f"build.context {context!r} resolves to {resolved}, outside {devcontainer_dir}; "
        "the prebuild tag would move on every commit"
    )


def test_devcontainer_declares_the_prebuild_repository_devpod_looks_in(devcontainer):
    """Declared in the manifest, so no flag or env var has to be remembered.

    devpod reads `customizations.devpod.prebuildRepository` on every `up`, which
    is every `dl` launch. Passing `--prebuild-repository` instead would work for
    whoever remembered it and for nobody else, and `dl` passes no such flag.
    """
    assert devcontainer["customizations"]["devpod"]["prebuildRepository"] == PREBUILD_REPOSITORY


def test_the_cache_fallback_points_at_the_same_repository_as_the_prebuild(devcontainer):
    """One registry, written down twice, kept in step.

    `build.cacheFrom` serves the builders that know nothing about devpod
    prebuilds -- VS Code's "Reopen in Container", a plain `devcontainer up` --
    and devpod publishes the `:latest` it names from the same task that publishes
    the prebuild. Pointed at some other repository it would import cache from an
    image nothing pushes, which is a miss on every build and looks like nothing.
    """
    cache_from = devcontainer["build"]["cacheFrom"]
    assert cache_from.startswith(f"{PREBUILD_REPOSITORY}:"), (
        f"build.cacheFrom {cache_from!r} is not a tag of {PREBUILD_REPOSITORY}"
    )


def test_the_prebuild_workflow_watches_the_directory_the_hash_is_computed_from():
    """The workflow's path filter has to cover every input to the tag.

    The filter is what decides whether a commit republishes the image, and the
    hash reads the build config and the build context -- both of which live under
    `.devcontainer/`. A filter narrower than that leaves commits that moved the
    tag with no image at the new tag, which is a silent return to local builds.

    Asserted as text rather than parsed YAML on purpose: pyyaml is not in this
    project's environment, and the claim is about one literal line.
    """
    assert PREBUILD_WORKFLOW.is_file(), f"{PREBUILD_WORKFLOW} is what publishes the prebuild"
    workflow = PREBUILD_WORKFLOW.read_text()
    assert "'.devcontainer/**'" in workflow, (
        "the prebuild workflow does not watch .devcontainer/**, which is where "
        "every input to the prebuild hash lives"
    )


def test_pixi_resolves_a_devcontainer_prebuild_task():
    """`pixi run devcontainer-prebuild` exists, per the task runner itself.

    The workflow invokes it by that name, so a rename that misses one leaves a
    job whose only failure is a task-not-found -- late, and after the runner has
    already paid for a checkout and an environment.
    """
    pixi = shutil.which("pixi")
    assert pixi is not None, "pixi is the project's task runner and must be on PATH"
    result = subprocess.run(
        [pixi, "task", "list", "--manifest-path", str(REPO_ROOT / "pyproject.toml")],
        capture_output=True,
        text=True,
        check=True,
    )
    assert "devcontainer-prebuild" in result.stdout.split()


def test_the_prebuild_workflow_builds_on_every_architecture_dl_is_launched_from():
    """One leg per architecture, because the tag is per architecture.

    devpod hashes the target architecture together with the config and the
    context, so amd64 and arm64 ask the registry for two different tags and
    neither can answer for the other -- there is no multi-arch manifest here and
    devpod would not look one up if there were. A dropped leg is therefore not a
    slower build on that architecture, it is every launch on that architecture
    back to building locally, silently, which is the whole failure mode this
    file's prebuild tests exist for.

    Asserted as text rather than parsed YAML on purpose: pyyaml is not in this
    project's environment. It is matched as a `runner:` line rather than as a
    substring anywhere in the file, because this workflow's comments name both
    runners: a substring search passes on a leg that has been deleted and only
    discussed, which was the first version of this test.
    """
    declared = re.findall(r"^\s*runner:\s*(\S+)\s*$", PREBUILD_WORKFLOW.read_text(), re.MULTILINE)
    assert set(declared) == set(PREBUILD_RUNNERS), (
        f"the prebuild workflow builds on {sorted(set(declared))}, not "
        f"{sorted(PREBUILD_RUNNERS)}; an architecture missing from that list gets "
        "no published prebuild, and one added to it needs a pixi platform too"
    )


def test_no_two_prebuild_legs_publish_the_same_moving_alias():
    """Two legs sharing an alias is a tag whose owner is a race.

    devpod arch-qualifies nothing: `--tag` values are pushed alongside the hash
    tag exactly as given, so if both legs passed `latest` the winner would be
    whichever finished last. `latest` is what `build.cacheFrom` points at, and a
    layer cache imported from the wrong architecture serves no layers -- so the
    symptom is a cache that works or does not from run to run, for reasons
    nothing in a build log would explain.

    `latest` is amd64's, once. The count is checked rather than only the
    uniqueness so that renaming amd64's alias to something arch-suffixed does not
    quietly leave `build.cacheFrom` pointing at a tag nobody pushes any more.
    """
    aliases = re.findall(r"^\s*alias:\s*(\S+)\s*$", PREBUILD_WORKFLOW.read_text(), re.MULTILINE)
    assert len(aliases) == len(PREBUILD_RUNNERS), (
        f"{len(aliases)} legs declare an alias but {len(PREBUILD_RUNNERS)} "
        "architectures are built; every leg needs its own"
    )
    assert len(set(aliases)) == len(aliases), f"two prebuild legs publish the same alias: {aliases}"
    assert aliases.count("latest") == 1, (
        f"exactly one leg may publish the unqualified `latest` that build.cacheFrom reads: {aliases}"
    )


def test_one_architectures_prebuild_failing_does_not_cancel_the_others():
    """`fail-fast: false`, because the legs publish independent tags.

    A cancelled leg leaves the tag it was building absent, which is the same
    silent return to local builds as never having run -- so fail-fast would turn
    one architecture's failure into two architectures with no image, for no gain.

    Matched as a key line, not a substring, for the reason the runner test above
    gives: prose about a setting is not the setting.
    """
    workflow = PREBUILD_WORKFLOW.read_text()
    assert re.search(r"^\s*fail-fast:\s*false\s*$", workflow, re.MULTILINE), (
        "the prebuild matrix is fail-fast, so one architecture failing cancels "
        "the other and leaves both building locally"
    )


def test_the_workspace_declares_a_platform_for_every_prebuild_runner():
    """A leg whose platform is undeclared dies before it builds anything.

    pixi refuses outright on a platform a workspace does not list -- `pixi
    install` exits `unsupported-platform` -- so the arm64 leg's `setup-pixi` step
    fails before `devpod build` is reached. The same line is what lets an arm64
    container run its own `postCreateCommand`, so dropping it would leave an arm64
    prebuild that pulls fast and then cannot come up.

    The lockfile is checked alongside the manifest because both installs are
    frozen -- `frozen: true` in ci.yml, `pixi install --frozen` at container
    create -- and a platform declared but not locked is a lockfile pixi refuses to
    use rather than a platform it solves on the spot. That second half is new: the
    create used to be a bare `pixi install`, which would have solved the missing
    platform on the spot and hidden the gap.
    """
    platforms = tomllib.loads(PYPROJECT.read_text())["tool"]["pixi"]["workspace"]["platforms"]
    lock = PIXI_LOCK.read_text()
    for runner, platform in PREBUILD_RUNNERS.items():
        assert platform in platforms, (
            f"the prebuild builds on {runner} but the workspace does not declare "
            f"{platform}; `pixi install` there exits unsupported-platform"
        )
        assert platform in lock, (
            f"{platform} is declared but absent from pixi.lock, which `frozen: true` needs"
        )


def pixi_global_specs(path) -> list:
    """Every `pixi global install` argument list a file issues, comments excluded.

    All of them rather than the first, because the feature's installer issues the
    command twice -- once through `su` when the build runs as root, once directly
    otherwise -- so a version pinned onto one of those two branches is an image
    whose contents depend on which branch ran, which is worse than either answer
    on its own. Comment lines are dropped because the paragraph above those
    commands is *about* the spec, and a test a comment can satisfy is no test.

    Read as text on purpose. One side of the comparison below is Rust and the
    other is shell, so there is no import that could answer this; the two files
    that have to agree are the two files this reads.
    """
    specs = []
    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.lstrip()
        if PIXI_GLOBAL_INSTALL not in line or stripped.startswith(("#", "//", "///")):
            continue
        spec = line[line.index(PIXI_GLOBAL_INSTALL) + len(PIXI_GLOBAL_INSTALL) :]
        # Trailing shell and Rust noise around the argument list: `|| failed=1`
        # from the rendered script, the closing quote of the `su -c "..."` form,
        # and Rust's raw-string delimiters.
        for terminator in ("||", '"#', "&&"):
            if terminator in spec:
                spec = spec[: spec.index(terminator)]
        specs.append(spec.strip().strip('"'))
    assert specs, f"{path} no longer installs anything with pixi global"
    return specs


def test_the_feature_and_the_shipping_provisioner_state_one_claude_spec():
    """One package, one install policy, written down in two places.

    `.devcontainer/claude-code/install.sh` bakes `claude-shim` into the prebuilt
    image; the shipping provisioner installs the same package into every
    workspace `dl` opens. Both are deliberately unversioned -- the shim carries
    no `claude` binary, the binary is downloaded on first run, so a pin would
    freeze the fetcher and not the thing fetched, and leaving it unpinned is what
    lets every republish refresh it. The argument, with the measurements, is under
    "What the prebuild tag does not promise" in docs/development.md.

    What this guards is the *asymmetry*, which is why it lives in this file rather
    than beside either half. The installer sits inside the prebuild's hash and the
    provisioner does not, so the case for pinning reads as stronger on the
    installer's side -- and pinning that side alone would give one package two
    policies, with the unpinned one the copy that reaches users. Neither half of
    that divergence fails anything on its own, so it has to fail here. A pin added
    to both sides in one change still passes: the two specs are required to agree,
    not required to float.
    """
    installer = pixi_global_specs(FEATURE_INSTALLER)
    shipping = pixi_global_specs(SHIPPING_PROVISIONER)
    claude = [spec for spec in shipping if "claude-shim" in spec]
    assert claude, (
        f"{SHIPPING_PROVISIONER.name} no longer installs claude-shim; this test compares it "
        "against the feature installer and has nothing left to compare"
    )
    baked = [spec for spec in installer if "claude-shim" in spec]
    assert baked, (
        f"{FEATURE_INSTALLER.name} no longer installs claude-shim, which the prebuilt image "
        "is documented as baking"
    )
    for spec in baked + claude:
        assert spec == claude[0], (
            f"claude-shim is installed as {spec!r} in one place and {claude[0]!r} in another; "
            "one package, one policy -- pin both sides or neither"
        )


def parse_version(text: str) -> tuple:
    """`v0.77.0` or `0.77.0` as a comparable tuple."""
    return tuple(int(part) for part in text.lstrip("v").split("."))


def test_the_devcontainers_pixi_can_read_the_lockfile_as_committed():
    """A container whose pixi cannot read `pixi.lock` silently ignores it.

    pixi does not fail on a lock-file newer than it understands. It warns and
    then treats it as missing -- "The lock-file will be treated as missing and
    regenerated" -- so the container solves its own environment instead of the
    committed one. Three costs, none of which is a red tick anywhere: the
    environment in the container is not the locked one, so the reproducibility
    `frozen: true` buys CI is absent exactly where the work happens; every create
    pays a full solve; and `pixi.lock` is tracked, so the regenerated file leaves
    the work tree dirty and, if it is ever committed, inverts the break onto the
    host.

    This is a real occurrence, not a hypothesis: the lock went to version 7 in
    4dd28c8 while the Dockerfile pinned v0.63.1, which reads at most version 6,
    and every container create in between was quietly unlocked.

    The pin is compared against a measured table rather than a rule of thumb, and
    an unrecognised lock version fails rather than passing -- being unable to
    answer the question is not the same as the answer being yes.
    """
    lock_version = int(
        next(
            line.split(":", 1)[1]
            for line in LOCKFILE.read_text(encoding="utf-8").splitlines()
            if line.startswith("version:")
        ).strip()
    )
    assert lock_version in MIN_PIXI_FOR_LOCK_VERSION, (
        f"pixi.lock is version {lock_version}, which MIN_PIXI_FOR_LOCK_VERSION does not know. "
        "Find the lowest pixi that reads it and record it there -- do not delete this test"
    )
    pinned = next(
        line.split("=", 1)[1].strip()
        for line in DOCKERFILE.read_text(encoding="utf-8").splitlines()
        if line.startswith("ARG PIXI_VERSION=")
    )
    required = MIN_PIXI_FOR_LOCK_VERSION[lock_version]
    assert parse_version(pinned) >= required, (
        f"{DOCKERFILE.name} pins pixi {pinned}, which cannot read pixi.lock version "
        f"{lock_version} (needs >= {'.'.join(str(n) for n in required)}); the container would "
        "discard the lock and solve its own environment"
    )


def test_the_devcontainer_installs_the_committed_lock_rather_than_solving_its_own(devcontainer):
    """A create that is allowed to solve is a create that can diverge, and can fail.

    The test above stops the container's pixi lagging the lock. It cannot stop
    the create *ignoring* the lock, because that is not a version question: a
    bare `pixi install` treats an unreadable lock as a missing one and solves a
    fresh environment in its place, and it does so having printed a warning and
    exited 0.

    Both halves of that are costly, and both were measured against this
    repository's own `pyproject.toml` and `pixi.lock` with the pixi the container
    pinned at the time (0.63.1, against the committed version 7):

    - bare `pixi install` exits 0 and rewrites the tracked `pixi.lock` down to
      version 6, so the create silently downgrades a tracked file. Commit that
      and the break inverts onto the host.
    - it reaches the network to do it. Solving pypi dependencies alongside conda
      ones needs a conda-pypi name mapping pixi fetches remotely, and when that
      fetch failed -- "failed to fetch conda-pypi mapping from remote source" --
      the create died in `postCreateCommand` and the workspace never opened. The
      committed resolution needs neither the mapping nor the solve, and that was
      checked rather than assumed: with the mapping cache deleted, `pixi install
      --frozen --offline` installs the default environment and leaves the cache
      absent, where a solving install recreates it.
    - `pixi install --frozen` exits 1 instead, naming the version gap and the
      fix, and leaves `pixi.lock` at version 7.

    So `--frozen` is what makes the environment in the container the environment
    CI tested -- `frozen: true` is what ci.yml installs with -- and turns a
    silent, network-dependent divergence into an immediate, self-describing
    failure. A lock genuinely out of step with the manifest cannot reach a branch
    that has passed CI, so the only creates this can fail are the ones that
    should.
    """
    post_create = devcontainer["postCreateCommand"]
    installs = re.findall(r"pixi install\b([^&|;]*)", post_create)
    assert installs, (
        f"{DEVCONTAINER_JSON.name}'s postCreateCommand no longer runs `pixi install`; "
        "this test guards how that install treats the lock and has nothing left to guard"
    )
    for flags in installs:
        assert "--frozen" in flags, (
            f"postCreateCommand runs `pixi install{flags.rstrip()}` without --frozen, so a "
            "create is free to discard the committed lock, solve its own environment over the "
            "network, and rewrite pixi.lock while it does it"
        )


SEEDED_NAMES = (".credentials.json", ".claude.json")


def _seeding_lines(installer, name):
    """Lines of `installer` that would put `name` on disk.

    A comment naming the file is the one exemption. Nothing else is: any other
    mention counts, including one that only assigns the path to a variable.

    Keying on a list of write verbs was the first attempt and it is the reason
    this is a function with tests of its own. `>`, `tee` and `cp` miss `touch`
    -- which is what `.devcontainer/claude-code/README.md` prescribes for this
    exact file, so it is the likeliest way the stub returns -- they miss
    `install -m 600 /dev/null`, and they miss any spelling that names the path
    on one line and redirects into the variable on the next.
    """
    return [
        line
        for line in installer.splitlines()
        if name in line and not line.lstrip().startswith("#")
    ]


@pytest.mark.parametrize(
    "write",
    [
        'touch "$TARGET_HOME/.claude/.credentials.json"',
        'install -m 600 /dev/null "$TARGET_HOME/.claude/.credentials.json"',
        # Names the path here and redirects into `$cred` on the next line, so
        # neither line carries both the filename and a redirection.
        'cred="$TARGET_HOME/.claude/.credentials.json"',
        ': > "$TARGET_HOME/.claude/.credentials.json"',
        'echo "{}" > "$TARGET_HOME/.claude/.credentials.json"',
    ],
)
def test_the_seeding_guard_catches_a_write_that_is_not_a_redirection(write):
    """The guard has to hold against the spellings, not against one idiom.

    Every line here re-seeds the credentials file and every one of them passed
    the guard as first written, which claimed to hold "over the whole installer
    ... so it cannot come back under another name" while matching only `>`,
    `tee` and `cp`.
    """
    installer = f'    mkdir -p "$TARGET_HOME/.claude"\n    {write}\n'
    assert _seeding_lines(installer, ".credentials.json") == [f"    {write}"]


def test_the_seeding_guard_reads_a_comment_as_a_comment():
    """The exemption the guard does grant, since the fix is 22 lines of comment
    explaining why the file is not seeded, and every one of them names it."""
    assert _seeding_lines("    # writes .credentials.json\n", ".credentials.json") == []


def test_the_feature_seeds_no_empty_credential():
    """The feature must not write a `.credentials.json` into the container.

    An empty ``{}`` there is not a harmless placeholder, it is a logged-out
    session that wins: ``dl`` forwards a profile's login as
    ``CLAUDE_CODE_OAUTH_TOKEN``, Claude Code reads the credentials file first,
    and the agent then asks the operator to log in while a valid token sits in
    its environment.

    It stayed hidden for as long as the feature mounted the host's real
    credentials file *over* the stub, so the bug was invisible in exactly the
    configuration that could not use a forwarded token anyway.

    ``.claude.json`` is held to the same rule because it was seeded by the same
    block for the same retired reason.

    The installer names both files only in comments today, so the guard forbids
    every other mention rather than guessing at write syntax. A future line that
    genuinely needs to name one is meant to be a conversation, not a match the
    filter happens to let through.
    """
    installer = FEATURE_INSTALLER.read_text()
    for name in SEEDED_NAMES:
        seeding = _seeding_lines(installer, name)
        assert not seeding, f"{name} is named outside a comment in install.sh: {seeding}"


def test_the_installer_may_not_create_the_file_that_marks_claude_onboarded(devcontainer):
    """`.claude.json` is forbidden for a second reason, and it is devlaunch's own.

    The provisioner seeds ``{"hasCompletedOnboarding":true}`` into
    ``$CLAUDE_CONFIG_DIR/.claude.json`` and exits early if that file is already
    there. This feature points ``CLAUDE_CONFIG_DIR`` at the very directory the
    installer sets up, so the ``{}`` stub was satisfying that guard: every
    container built from this feature skipped the onboarding seed, which is the
    opposite of what seeding it was for.

    Two files hold that one fact, so this diffs them rather than restating it:
    the name the provisioner exits on has to be a name the installer is
    forbidden to write.
    """
    guarded = re.findall(r'\[ -e \\"\$dir/([^\\]+)\\" \]', SHIPPING_PROVISIONER.read_text())
    assert guarded, (
        f"{SHIPPING_PROVISIONER.name} no longer guards the onboarding seed on an "
        "existing file; this test diffs that name against the installer and has "
        "nothing left to diff"
    )
    config_dir = devcontainer["containerEnv"]["CLAUDE_CONFIG_DIR"]
    assert config_dir.endswith("/.claude"), (
        f"CLAUDE_CONFIG_DIR is {config_dir}, which is no longer the directory "
        "install.sh populates, so the two no longer collide and this test is moot"
    )
    for name in guarded:
        assert name in SEEDED_NAMES, (
            f"the provisioner skips the onboarding seed when {name} exists, but "
            f"install.sh is only forbidden to create {SEEDED_NAMES}, so the feature "
            f"is free to suppress it again"
        )


def test_the_agent_socket_is_bound_from_the_path_the_host_hook_maintains(devcontainer, mounts):
    """The agent socket mount reads a path `init-host.sh` keeps valid, not the host's variable.

    Third design, and the test that pinned the second one is what this replaces.
    That test held the source to ``${localEnv:SSH_AUTH_SOCK}`` on the argument
    that ``init-host.sh`` "cannot paper over it the way it does for
    ``known_hosts``: there is no touching a socket into being an agent". True of
    an agent, and beside the point for a mount: docker needs the *source to
    exist*, and whether an agent answers there is a separate question. Bound
    from the variable, a host with none exported -- a fresh desktop before any
    dotfiles run, the first machine every new user has -- expanded it to
    nothing, and docker refused the whole create with ``field Source must not
    be empty``. The first design, this same path as a bare assumption, refused
    a gpg-agent host with ``bind mount source path does not exist``. Both made
    "no agent" mean "no container".

    So the source is the stable path again, and the hook is what makes it true:
    a symlink to ``$SSH_AUTH_SOCK`` when that is a live socket, a placeholder
    when nothing is. ``test_the_host_hook_gives_the_agent_socket_mount_a_source_on_every_host``
    holds the hook to that; this holds the manifest to binding the path the
    hook maintains and nothing else.
    """
    agent_socket = devcontainer["containerEnv"]["SSH_AUTH_SOCK"]
    sources = {mount.get("source") for mount in mounts if mount.get("target") == agent_socket}
    assert sources == {f"{LOCAL_HOME}/.ssh/agent.sock"}, (
        f"the agent socket must be bound from {LOCAL_HOME}/.ssh/agent.sock, the path "
        f"init-host.sh maintains; found {sources}"
    )


def test_the_host_hook_heals_the_socket_it_is_mounted_from(devcontainer, mounts):
    """The heal list names the path the manifest actually binds.

    Two hand-maintained copies of one fact: the mount source here, and the loop
    in ``init-host.sh`` that detaches a stale one. They disagreed the moment the
    mount moved, and a heal aimed at a path nothing mounts is a heal that never
    fires -- silently, since a stale mount only bites a *nested* create.
    """
    hook = (REPO_ROOT / ".devcontainer" / "claude-code" / "init-host.sh").read_text()
    agent_socket = devcontainer["containerEnv"]["SSH_AUTH_SOCK"]
    source = next(mount["source"] for mount in mounts if mount.get("target") == agent_socket)
    # "${localEnv:HOME}/x" in the manifest is "$HOME/x" in the shell.
    shell_path = source.replace(LOCAL_HOME, "$HOME")
    assert f'"{shell_path}"' in hook, (
        f"init-host.sh does not heal a stale mount at {shell_path}, which is what "
        "the manifest binds the agent socket from"
    )


def _live_unix_socket(path: Path):
    """A real bound socket at `path`, held open for the caller's `with` block.

    Bound and listening, and *not* an ssh-agent: nothing speaks the agent protocol
    on it. That distinction is load-bearing below, because the hook now asks
    whether an agent answers rather than whether a socket exists.
    """
    import socket

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.bind(str(path))
    return sock


@contextlib.contextmanager
def _real_ssh_agent(path: Path):
    """An actual `ssh-agent` listening at `path`, killed on the way out.

    A plain bound socket will not do for the arm that follows a live agent:
    `ssh-add -l` exits 2 against one (it connects and gets no agent protocol) and
    1 against a real agent holding no keys, and 2 is exactly what the hook reads
    as "not an agent".
    """
    agent = shutil.which("ssh-agent")
    if agent is None:
        pytest.skip("ssh-agent is not installed, so the live-agent arm cannot be staged")
    started = subprocess.run([agent, "-a", str(path)], capture_output=True, text=True, check=False)
    assert started.returncode == 0, started.stderr
    try:
        yield
    finally:
        subprocess.run(
            [agent, "-k"],
            env=dict(os.environ, SSH_AUTH_SOCK=str(path)),
            capture_output=True,
            check=False,
        )


def test_the_host_hook_gives_the_agent_socket_mount_a_source_on_every_host(
    devcontainer, mounts, tmp_path
):
    """The three hosts the hook meets, and the source each one ends up with.

    Run against a scratch HOME so the developer's real ``~/.ssh`` is not the
    fixture, with ``SSH_AUTH_SOCK`` set explicitly for each case rather than
    inherited: what the test runner's own session exports is the one thing this
    must not depend on, since a host with no agent is the case under test.

    1. No agent anywhere: a placeholder regular file, so the create is not
       refused. This is the fresh-machine case and the reason the test exists.
    2. ``SSH_AUTH_SOCK`` names a live socket elsewhere (gpg-agent, ssh-agent(1),
       ``ssh -A``): the path becomes a symlink to it, replacing the placeholder,
       so docker binds the agent itself.
    3. A real socket already at the path that is not a symlink -- an agent bound
       there by ``ssh-agent -a``, or the mount seen from inside a container --
       is never touched, even when the variable names a different agent.
       Unlinking it would orphan the agent; writing over the mount is EROFS.
    """
    agent_socket = devcontainer["containerEnv"]["SSH_AUTH_SOCK"]
    source = next(mount["source"] for mount in mounts if mount.get("target") == agent_socket)
    maintained = tmp_path / source[len(f"{LOCAL_HOME}/") :]
    env = dict(os.environ, HOME=str(tmp_path))
    env.pop("SSH_AUTH_SOCK", None)

    def run_hook(**extra):
        result = subprocess.run(
            ["sh", "-c", devcontainer["initializeCommand"]],
            cwd=REPO_ROOT,
            env=dict(env, **extra),
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0, result.stderr

    # 1. No agent.
    run_hook()
    assert maintained.is_file() and not maintained.is_symlink(), (
        f"with no agent, {source} should be a placeholder file; got "
        f"{'a symlink' if maintained.is_symlink() else 'nothing' if not maintained.exists() else maintained.stat().st_mode}"
    )

    # 1b. A socket that exists and answers nothing. This is dl's own placeholder,
    # which it exports as SSH_AUTH_SOCK for the whole `devpod up` so that a
    # manifest binding ${localEnv:SSH_AUTH_SOCK} gets a bindable source -- and this
    # hook is that `devpod up`'s child, so it inherits it. Followed, it would
    # unlink and recreate the bind source every container of this repo is mounted
    # from, for no gain: a different placeholder to the same effect.
    hollow = tmp_path / "no-agent.sock"
    with _live_unix_socket(hollow):
        run_hook(SSH_AUTH_SOCK=str(hollow))
        assert not maintained.is_symlink(), (
            "the hook followed a socket no agent answers on, relinking the bind "
            "source to a placeholder for nothing"
        )
        assert maintained.is_file()

    # 2. A real agent elsewhere.
    elsewhere = tmp_path / "S.gpg-agent.ssh"
    with _real_ssh_agent(elsewhere):
        run_hook(SSH_AUTH_SOCK=str(elsewhere))
        assert maintained.is_symlink(), f"{source} should now be a symlink to the agent"
        assert maintained.resolve() == elsewhere.resolve()
        assert maintained.is_socket(), "and resolve to a socket docker can bind"

        # 2b. Idempotent: a second run with the same agent leaves the link as it is.
        before = maintained.lstat().st_mtime_ns
        run_hook(SSH_AUTH_SOCK=str(elsewhere))
        assert maintained.lstat().st_mtime_ns == before, "the symlink was rewritten needlessly"

    # 3. A real socket bound at the path is never touched, whatever the variable says.
    maintained.unlink()
    with _live_unix_socket(maintained), _live_unix_socket(tmp_path / "other.sock"):
        inode = maintained.lstat().st_ino
        run_hook(SSH_AUTH_SOCK=str(tmp_path / "other.sock"))
        assert not maintained.is_symlink(), "a bound socket at the path was replaced by a symlink"
        assert maintained.lstat().st_ino == inode, "a bound socket at the path was recreated"
        # And with no variable at all, the same.
        run_hook()
        assert maintained.lstat().st_ino == inode


def test_the_no_agent_line_in_the_docs_is_the_line_dl_prints():
    """The docs page quotes `NoSshAgent`; this diffs the quote against the source.

    CLAUDE.md allows a second hand-maintained copy of a fact only where a test
    named beside it diffs it against the first. ``docs/workspace-tools.md``
    reproduces the terminal line verbatim so a reader recognises what they saw,
    and ``render.rs`` is where the words live. Without this, rewording the notice
    leaves the page publishing output ``dl`` no longer prints, with every other
    guard green.

    Compared on the words rather than byte for byte: the doc wraps the line to fit
    the page and the source wraps it to fit the column, and neither wrapping is the
    fact.
    """
    docs = (REPO_ROOT / "docs" / "workspace-tools.md").read_text()
    render = (REPO_ROOT / "rust" / "dl" / "src" / "render.rs").read_text()

    quoted = re.search(r"```\n(ssh-agent: none on this host.*?)\n```", docs, re.S)
    assert quoted, "docs/workspace-tools.md no longer quotes the no-agent line"
    words = " ".join(quoted.group(1).split())

    # The source spells it as a `\`-continued literal: join the pieces the way the
    # compiler does before comparing.
    source = " ".join(render.replace("\\\n", "").split())
    assert words in source, (
        "the no-agent line in docs/workspace-tools.md is not the line render.rs "
        f"prints any more:\n  docs: {words}"
    )


def test_the_dotfiles_line_in_the_docs_is_the_line_dl_prints():
    """The same diff as above, for `DotfilesNotConfigured`.

    This one had already drifted. The notice used to name `devpod context
    set-options DOTFILES_URL=<repo>`, which devpod parses as a stray argument and
    ignores, so the one line whose job is to say where the setting comes from
    handed over a command that left it unset. Fixing `render.rs` left the docs page
    still publishing the broken form in two places: the quoted notice, and the
    `bash` block above it that a reader is likelier to copy from.

    So both are pinned here. The quote is diffed against `render.rs` the way the
    no-agent line is, and the runnable example is held to the same `-o` the notice
    tells people to type. A page that says one thing in prose and another in the
    block you paste is worse than either alone.
    """
    docs = (REPO_ROOT / "docs" / "workspace-tools.md").read_text()
    render = (REPO_ROOT / "rust" / "dl" / "src" / "render.rs").read_text()

    quoted = re.search(r"^(dotfiles: none set in devpod context options.*)$", docs, re.M)
    assert quoted, "docs/workspace-tools.md no longer quotes the dotfiles notice"
    words = " ".join(quoted.group(1).split())

    source = " ".join(render.replace("\\\n", "").split())
    assert words in source, (
        "the dotfiles line in docs/workspace-tools.md is not the line render.rs "
        f"prints any more:\n  docs: {words}"
    )

    # `set-options` takes its KEY=VALUE through `-o`/`--option` and through nothing
    # else (devpod v0.26.1); the bare form exits 0 having written no option.
    commands = re.findall(r"^devpod context set-options .*$", docs, re.M)
    assert commands, "docs/workspace-tools.md no longer shows how to set the option"
    for command in commands:
        assert re.match(r"^devpod context set-options (-o|--option) \S+=", command), (
            f"docs/workspace-tools.md tells the reader to run {command!r}, which devpod "
            "parses as a stray argument and ignores"
        )


def test_the_dockerfile_copies_nothing_out_of_the_build_context():
    """The premise every exclusion below rests on.

    `.dockerignore` is safe here only because the build never reads the context:
    no `COPY`, no `ADD`, so the tar devpod assembles is opened by nothing. Add
    one and the excluded files stop being invisible to the image and start being
    *missing* from it -- a build that fails, or worse, a stale prebuild that
    matches a context it no longer describes.

    So this is the test to read first when `.dockerignore` looks wrong: it is the
    claim, and the exclusions are downstream of it.
    """
    instructions = [
        line.split(maxsplit=1)[0].upper()
        for line in DOCKERFILE.read_text().splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]
    assert not {"COPY", "ADD"} & set(instructions), (
        f"{DOCKERFILE.name} now reads the build context, which .dockerignore is filtering; "
        "either the copied paths have to be un-excluded or the exclusions have to go"
    )


def test_every_file_in_the_build_context_is_classified_as_a_build_input_or_not():
    """A file added under `.devcontainer/` has to be put on one side or the other.

    Left unclassified it defaults to the expensive answer -- hashed, so its edits
    move the prebuild tag -- and defaults to it silently, because nothing about a
    launch changes except that it takes minutes again. Failing here is the cheap
    version of that conversation.

    `.dockerignore` itself is neither: it is what draws the line, and it is
    hashed, correctly, since changing what the hash can see should move the tag.
    """
    present = {
        str(path.relative_to(CONTEXT_DIR))
        for path in CONTEXT_DIR.rglob("*")
        if path.is_file() and path.name != ".dockerignore"
    }
    classified = CONTEXT_BUILD_INPUTS | CONTEXT_NON_BUILD_INPUTS
    assert present == classified, (
        f"the build context holds {sorted(present - classified)} that no test has "
        f"classified, and expects {sorted(classified - present)} that are not there; "
        "add each to CONTEXT_BUILD_INPUTS or CONTEXT_NON_BUILD_INPUTS"
    )


def test_the_prebuild_tag_cannot_move_for_a_file_the_image_never_sees():
    """The fix for a prebuild that stopped being pulled.

    Measured on devpod 0.26.1, amd64, at 4db3427: appending a comment line to
    `claude-code/README.md` and another to `claude-code/init-host.sh` moved the
    tag from `devpod-5bf7be3e3e7e1b3f4fbb01a9b3ab88e7`, which CI had published
    and every launch pulled, to `devpod-90e9641b9d3661f681e640eeedb3a410`, which
    nothing had ever published. Neither file can reach the image. The cost was a
    full local build per launch, on every branch carrying the edit, and the only
    symptom was that opening a container was slow again.
    """
    for rel_path in sorted(CONTEXT_NON_BUILD_INPUTS):
        assert excluded_from_the_context_hash(rel_path), (
            f"{rel_path} cannot reach the image but is still hashed into the prebuild "
            f"tag; {CONTEXT_DOCKERIGNORE.name} has to exclude it or editing it throws "
            "the published prebuild away"
        )


def test_the_prebuild_tag_still_moves_for_every_file_the_image_is_built_from():
    """The other half, and the one that makes the exclusions worth having.

    An exclusion too wide is worse than none: the tag stops moving when the image
    would genuinely differ, so a launch pulls an image built from a Dockerfile or
    a feature installer that is no longer the one on the branch. That failure is
    quieter than a slow build and much harder to attribute -- the container comes
    up, it is simply not the container the branch asks for.
    """
    for rel_path in sorted(CONTEXT_BUILD_INPUTS):
        assert not excluded_from_the_context_hash(rel_path), (
            f"{rel_path} is built into the image but {CONTEXT_DOCKERIGNORE.name} keeps it "
            "out of the prebuild tag, so a launch can pull an image that predates it"
        )


def test_the_excluded_manifest_leaves_its_build_inputs_in_the_hash(devcontainer):
    """Excluding `devcontainer.json` is only safe while devpod hashes what it declares.

    `normalizeConfigForHash` keeps `name`, `features`, `overrideFeatureInstallOrder`,
    `image`, `dockerfile`, `context` and the whole of `build`, and clears the rest --
    `mounts`, `containerEnv`, `postCreateCommand`, `customizations` -- because devpod
    applies those when it creates the container rather than when it builds the image.
    So the config half of the hash already carries every build input this file has,
    and hashing its raw bytes on top added only the comments, which this manifest is
    mostly made of.

    What this test holds is the boundary: every key in here is either one devpod
    hashes or one it deliberately applies at create time. A key that is neither
    would be a build input the tag could not see, which is the stale-image failure
    the test above describes, reached from the other direction.
    """
    assert "devcontainer.json" in dockerignore_patterns(), (
        "this test is about the manifest being excluded from the context hash, and it "
        "is not excluded; either restore the exclusion or drop this test"
    )
    hashed = {
        "name",
        "features",
        "overrideFeatureInstallOrder",
        "image",
        "dockerfile",
        "context",
        "build",
    }
    applied_at_create = {
        "mounts",
        "containerEnv",
        "postCreateCommand",
        "customizations",
        "initializeCommand",
        "runArgs",
        "workspaceFolder",
        "workspaceMount",
        "remoteUser",
        "remoteEnv",
        "privileged",
        "init",
    }
    unaccounted = set(devcontainer) - hashed - applied_at_create
    assert not unaccounted, (
        f"{DEVCONTAINER_JSON.name} declares {sorted(unaccounted)}, which this test cannot "
        "say devpod hashes or applies at create time; if any of them affects the image, the "
        "manifest can no longer be excluded from the context hash"
    )
