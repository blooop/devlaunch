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
