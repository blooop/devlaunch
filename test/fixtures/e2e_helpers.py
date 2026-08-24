"""E2E test helpers for running dl commands safely.

These helpers ensure that E2E tests don't accidentally launch VSCode
or other IDEs, which would break automated testing.

They also own the two other things an e2e test needs before it can reach a
workspace at all: the argv prefix of the implementation under test (the
`DEVLAUNCH_DL_CMD` seam), and -- for the pty transport -- an environment in
which both ssh lookups resolve this run's own ssh config. See `route_ssh_through`.
"""

import os
import shlex
import shutil
import stat
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional

import pytest

from devpod_scoping import DEVPOD_SSH_CONFIG_VAR
from fixtures.e2e_guard import LEDGER


# The real runner, captured once. `create_e2e_workspace` compares against it to
# decide whether anything was actually built; see the note there.
_REAL_RUN = subprocess.run


REPO_ROOT = Path(__file__).resolve().parent.parent.parent


def _release_binary(name: str) -> Path:
    """Where `cargo build --release` leaves `name`.

    `CARGO_TARGET_DIR` is honoured because cargo honours it: a developer who has
    redirected their build output is not someone this harness should send looking
    in a directory cargo stopped writing to.
    """
    target = os.environ.get("CARGO_TARGET_DIR") or str(REPO_ROOT / "rust" / "target")
    return Path(target) / "release" / name


def _command_seam(env_var: str, binary: str) -> List[str]:
    """The argv prefix that runs one of devlaunch's entry points.

    This is the acceptance harness's one knob (#252): unset, the suite judges the
    release binary this checkout builds; set, the same tests judge whatever command
    the variable names. The value is shell-split, so it may be a bare path or a
    command with arguments — `DEVLAUNCH_DL_CMD='cargo run -q -p dl --bin dl --'` is
    how you point it at a debug build without building a release one.

    An empty or blank value counts as unset: it is what a shell leaves behind
    (`DEVLAUNCH_DL_CMD= pytest ...`), not a request to run the empty command.

    The default is a *path*, and a missing one fails the test that asked rather
    than being built on the spot. Building here would put a compile inside whichever
    test got there first -- including the ones that measure time -- and would leave
    the suite testing a binary it built from whatever the tree said at that moment,
    which is not a thing the report would mention. `pixi run test` and `test-e2e`
    build up front for exactly this reason.
    """
    override = os.environ.get(env_var, "").strip()
    if override:
        return shlex.split(override)

    path = _release_binary(binary)
    if not path.exists():
        pytest.fail(
            f"there is no {binary} to test at {path}. Build it first --\n"
            f"    cd rust && cargo build --release -p dl -p aid\n"
            f"(`pixi run test-e2e` does this for you), or point the harness at "
            f"another build with {env_var}."
        )
    return [str(path)]


def dl_command() -> List[str]:
    """The command under test for `dl`, as an argv prefix."""
    return _command_seam("DEVLAUNCH_DL_CMD", "dl")


def aid_command() -> List[str]:
    """The command under test for `aid`, as an argv prefix.

    A seam of its own rather than a suffix rule on DEVLAUNCH_DL_CMD: the two
    entry points are separate binaries, and pointing the harness at one must not
    silently redirect the other.
    """
    return _command_seam("DEVLAUNCH_AID_CMD", "aid")


def require_devpod() -> None:
    """Fail the session -- not skip it -- if devpod cannot be executed.

    devpod is a pixi dependency of this project, so a run that cannot execute
    it has a broken environment rather than one that declined to test devpod.
    `pytest -m e2e` is an explicit request for exactly the tests that need it,
    and answering that request with grey text is how a suite reports nothing
    and passes.

    Asked once for the whole e2e directory, from the conftest: an
    installed-but-unrunnable devpod and a missing one are the same thing to a
    test, and two checks that can disagree are worse than one that cannot.
    """
    try:
        runnable = (
            subprocess.run(["devpod", "version"], capture_output=True, check=False).returncode == 0
        )
    except FileNotFoundError:
        runnable = False

    if not runnable:
        pytest.fail(
            "`devpod version` did not run, so no e2e test in this session can "
            "do anything. devpod is a pixi dependency of this project -- run "
            "the suite as `pixi run test-e2e` rather than a bare pytest."
        )


class DLRunner:
    """Helper to run dl commands safely without launching IDE.

    This class ensures the 'code' subcommand is never used in E2E tests,
    preventing VSCode from launching during automated testing.
    """

    def __init__(self, env: Optional[Dict[str, str]] = None):
        """Initialize the runner.

        Args:
            env: Environment variables to use. If None, uses current environment.
        """
        self.env = env or dict(os.environ)
        self.last_result: Optional[subprocess.CompletedProcess] = None

    def run(
        self,
        *args: str,
        check: bool = False,
        capture_output: bool = True,
    ) -> subprocess.CompletedProcess:
        """Run a dl command.

        Args:
            *args: Arguments to pass to dl
            check: Whether to raise on non-zero exit
            capture_output: Whether to capture stdout/stderr

        Returns:
            CompletedProcess result

        Raises:
            ValueError: If 'code' subcommand is used (would launch VSCode)
        """
        # Ensure 'code' subcommand is not used
        if "code" in args:
            raise ValueError(
                "E2E tests must not use 'code' subcommand (launches VSCode). "
                "Use the default command or '--' to run commands instead."
            )

        cmd = dl_command() + list(args)
        self.last_result = subprocess.run(
            cmd,
            env=self.env,
            capture_output=capture_output,
            text=True,
            check=check,
        )
        return self.last_result

    def run_with_spec(
        self,
        spec: str,
        *extra_args: str,
        check: bool = False,
    ) -> subprocess.CompletedProcess:
        """Run dl with a spec like 'owner/repo@branch'.

        This is the safe way to create workspaces without IDE.

        Args:
            spec: Repository spec (e.g., "owner/repo@main")
            *extra_args: Additional arguments (must not include 'code')
            check: Whether to raise on non-zero exit

        Returns:
            CompletedProcess result
        """
        return self.run(spec, *extra_args, check=check)

    def ssh(
        self,
        workspace_id: str,
        *command: str,
        check: bool = False,
    ) -> subprocess.CompletedProcess:
        """SSH into a workspace and optionally run a command.

        Args:
            workspace_id: The workspace to SSH into
            *command: Optional command to run (passed after --)
            check: Whether to raise on non-zero exit

        Returns:
            CompletedProcess result
        """
        args = ["ssh", workspace_id]
        if command:
            args.extend(["--"] + list(command))
        return self.run(*args, check=check)

    def list_workspaces(self) -> subprocess.CompletedProcess:
        """List all workspaces.

        Returns:
            CompletedProcess result with JSON output
        """
        return self.run("list", "--json")


@pytest.fixture
def dl_no_ide(isolated_devlaunch_env: Dict[str, Path]) -> DLRunner:
    """Pytest fixture that provides a safe dl command runner.

    The runner is configured with isolated environment variables
    and prevents accidental IDE launches.

    Usage in E2E tests:
        @pytest.mark.e2e
        def test_workspace_creation(dl_no_ide, local_git_repo):
            # Safe - no IDE launched
            result = dl_no_ide.run_with_spec(f"test/{local_git_repo['remote_url']}@main")
            assert result.returncode == 0

            # This would raise ValueError:
            # dl_no_ide.run("owner/repo@main", "code")  # Error!
    """
    env = dict(os.environ)
    env["XDG_CACHE_HOME"] = str(isolated_devlaunch_env["cache_dir"])

    return DLRunner(env=env)


class WorkspaceTracker:
    """The workspaces one scope created, and the promise to delete them.

    A class at module level rather than a closure inside the fixture below,
    because the scope that needs one is not always a test. A module-scoped
    fixture cannot request a function-scoped one, so a module that builds a
    single workspace for all its tests -- `test_interactive_session.py` -- owns a
    tracker of its own and calls `cleanup()` from its own teardown. One
    implementation either way: a leak is a leak whichever scope leaked it.
    """

    def __init__(self):
        self.workspaces: List[str] = []

    def track(self, workspace_id: str) -> None:
        """Track a workspace for cleanup."""
        self.workspaces.append(workspace_id)

    def cleanup(self) -> None:
        """Delete all tracked workspaces."""
        for workspace_id in self.workspaces:
            try:
                subprocess.run(
                    ["devpod", "delete", workspace_id, "--force"],
                    capture_output=True,
                    check=False,
                )
            except Exception:
                pass  # Best effort cleanup


@pytest.fixture
def devpod_cleanup():
    """Fixture that tracks and cleans up DevPod workspaces after tests.

    Usage:
        @pytest.mark.e2e
        def test_something(devpod_cleanup):
            devpod_cleanup.track("my-workspace-id")
            # ... test code ...
            # Workspace automatically deleted after test
    """
    tracker = WorkspaceTracker()
    yield tracker
    tracker.cleanup()


def create_e2e_workspace(
    source: str,
    workspace_id: str,
    *,
    cleanup,
    env: Optional[Dict[str, str]] = None,
    run=_REAL_RUN,
) -> subprocess.CompletedProcess:
    """Create the workspace an e2e test needs, or fail the test saying why.

    Every e2e workspace goes through here, because the two ways of getting this
    wrong are only visible when the creation step is written out by hand:

    - **No IDE, ever.** devpod's default is to open an editor once the workspace
      is up. On a headless machine that means `xdg-open` is missing, devpod
      exits 1, and the workspace it already built is left running with the
      caller convinced nothing happened.
    - **Registered for cleanup before creation is attempted, not after.**
      Registering afterwards registers only on the happy path, which is the one
      path that did not need it.

    A non-zero rc fails rather than skips. `pytest.skip` on a step the test
    cannot proceed without turns a real outcome -- a leak, a broken daemon, a
    misconfigured provider -- into a green run with a line of grey text.

    Being the only door is also what makes it the only honest counter. The
    session ledger asks "did this run build anything", and no report of a
    passing test answers that -- inferring a container from a green test is the
    mistake that let a run with a dead registry in it look healthy. Recorded
    after the rc check, so the count is of workspaces that exist.

    `run` is injectable so this function's own logic can be unit-tested without
    devpod, which is the one way the count above could be made to lie: a stub
    that returns rc 0 built nothing. So the ledger is credited only when the
    real runner was the thing that ran, which no stub can be. Without that,
    `pytest -m ""` -- unit tests and e2e in one session -- would credit three
    workspaces that never existed and clear the session floor with them.
    """
    cleanup.track(workspace_id)
    result = run(
        ["devpod", "up", source, "--id", workspace_id, "--ide", "none"],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        pytest.fail(
            f"devpod up exited {result.returncode} creating workspace {workspace_id!r} "
            f"from {source!r}; it is registered for cleanup either way.\n"
            f"stdout: {(result.stdout or '').strip()[-2000:]}\n"
            f"stderr: {(result.stderr or '').strip()[-2000:]}"
        )
    if run is _REAL_RUN:
        LEDGER.record_workspace_created(workspace_id)
    return result


# --------------------------------------------------------------------------
# Reaching a workspace over the pty transport
#
# Let dl's pty transport find the run's own ssh config instead of the developer's.
#
# `dl <ws> -- <cmd>` reaches a workspace two ways, and the interesting one goes
# through OpenSSH: `ssh -t <workspace>.devpod <payload>`, using the host alias
# `devpod up` publishes. The suite scopes that publication away from the developer
# with `DEVPOD_SSH_CONFIG` (see `test/devpod_scoping.py`), which leaves the alias
# somewhere nothing looks for it by default. Two separate lookups have to reach
# it, and only one of them needs help from here.
#
# **dl's own check** -- "did devpod publish an alias for this workspace, or should
# this command fall back to the transport with no terminal?" -- resolves
# `DEVPOD_SSH_CONFIG` itself, ahead of `~/.ssh/config`, because that is where
# devpod wrote the alias and the only place it wrote it (devlaunch#421). So the
# variable the suite already exports is the whole of it, and the scratch `HOME`
# below deliberately holds **no** `.ssh/config` at all: dl finding the alias
# anyway is what proves the resolution order, and a symlink standing in for it
# would hide a regression back to the hardcoded path.
#
# **OpenSSH itself** does not read `DEVPOD_SSH_CONFIG` or `$HOME`. It expands `~`
# for the default user config through `getpwuid(getuid())`, so the scratch home is
# invisible to it and `ssh <alias>` fails with "Could not resolve hostname" --
# measured on this suite's own ssh, and the reason this needs a shim rather than a
# one-line `monkeypatch.setenv`. What does work is `-F <path>`, so an `ssh` shim
# first on `PATH` supplies it.
#
# The shim passes `-F` *before* the caller's arguments, which makes it a default
# and not an override: OpenSSH takes the last `-F` on the command line, so a
# command that names its own config still gets it. Nothing else about the
# invocation is touched -- the argv dl composed, `-t` included, is what reaches
# ssh, so what the tests measure is still dl's transport and a real tunnel into a
# real container.
# --------------------------------------------------------------------------


def scoped_ssh_config() -> Path:
    """The ssh config this run's `devpod up` writes its host aliases into.

    Asserted rather than defaulted. Unset means `devpod up` published the alias
    into the developer's real `~/.ssh/config`, and a test that quietly read it
    from there would be passing on the strength of the write this suite exists
    to prevent.
    """
    configured = os.environ.get(DEVPOD_SSH_CONFIG_VAR)
    if not configured:
        pytest.fail(
            f"{DEVPOD_SSH_CONFIG_VAR} is unset, so `devpod up` published its host "
            "alias into the developer's real ~/.ssh/config rather than into this "
            "run's own -- see test/devpod_scoping.py"
        )
    return Path(configured)


@dataclass(frozen=True)
class ScopedSsh:
    """Where the redirected home and the `ssh` shim live, and the env that uses them."""

    home: Path
    bin_dir: Path
    config: Path

    def env(self, base: Optional[Dict[str, str]] = None) -> Dict[str, str]:
        """An environment in which both ssh lookups find this run's config.

        Built on the caller's (or the current) environment, so everything the
        suite already scopes -- `DEVPOD_HOME`, `DEVPOD_SSH_CONFIG`,
        `XDG_CACHE_HOME` -- rides along untouched.

        Only ever handed to a subprocess. Putting `HOME` into this process's own
        environment would move it under the feet of the tests that read the
        developer's real `~/.ssh` to prove nothing was written there.
        """
        env = dict(os.environ if base is None else base)
        env["HOME"] = str(self.home)
        env["PATH"] = f"{self.bin_dir}{os.pathsep}{env.get('PATH', '')}"
        return env


def route_ssh_through(config: Path, root: Path) -> ScopedSsh:
    """Materialize a scratch home and an `ssh` shim that resolves `config`.

    `root` is a directory this run owns; two subdirectories are created under it.

    The home is deliberately bare of a `.ssh/config`: dl resolves
    `DEVPOD_SSH_CONFIG` for itself, so putting one there would only hide a
    regression to the hardcoded `~/.ssh/config` (devlaunch#421). It stays scoped
    so nothing under test reads the developer's home.
    """
    real_ssh = shutil.which("ssh")
    if real_ssh is None:
        pytest.fail(
            "no `ssh` on PATH, so dl's pty transport cannot run at all -- it is "
            "OpenSSH that carries the terminal into the workspace"
        )

    home = root / "home"
    home.mkdir(parents=True, exist_ok=True)

    bin_dir = root / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    shim = bin_dir / "ssh"
    shim.write_text(f'#!/bin/sh\nexec {shlex.quote(real_ssh)} -F {shlex.quote(str(config))} "$@"\n')
    shim.chmod(shim.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    return ScopedSsh(home=home, bin_dir=bin_dir, config=config)
