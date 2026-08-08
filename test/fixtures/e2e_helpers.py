"""E2E test helpers for running dl commands safely.

These helpers ensure that E2E tests don't accidentally launch VSCode
or other IDEs, which would break automated testing.
"""

import os
import subprocess
from pathlib import Path
from typing import Dict, List, Optional

import pytest


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

        cmd = ["python", "-m", "devlaunch.dl"] + list(args)
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

    class WorkspaceTracker:
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

    tracker = WorkspaceTracker()
    yield tracker
    tracker.cleanup()


def create_e2e_workspace(
    source: str,
    workspace_id: str,
    *,
    cleanup,
    env: Optional[Dict[str, str]] = None,
    run=subprocess.run,
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
    return result
