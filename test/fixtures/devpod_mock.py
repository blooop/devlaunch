"""Thin DevPod mock for Tier 2 integration tests.

This module provides a minimal mock for DevPod that records calls but doesn't
run actual containers. This allows testing the full git workflow while
avoiding the overhead of real container operations.
"""

import subprocess
from collections.abc import Generator
from typing import Dict, List, Optional
from unittest.mock import patch

import pytest


class DevPodMock:
    """Records devpod calls but doesn't run containers.

    This mock:
    - Records all DevPod command invocations
    - Tracks workspace state (created, started, stopped, deleted)
    - Returns simulated success responses
    - Can be configured to fail specific commands for error testing

    Usage:
        mock = DevPodMock()
        with mock.patch():
            # Your test code that calls devpod
            pass
        assert mock.calls  # Check recorded calls
    """

    def __init__(self):
        """Initialize the mock."""
        self.calls: List[List[str]] = []
        self.workspaces: Dict[str, Dict] = {}
        self.fail_commands: Dict[str, str] = {}  # command -> error message
        self._patcher = None

    def __call__(self, args: List[str], capture: bool = False) -> subprocess.CompletedProcess:
        """Handle a devpod command call.

        Args:
            args: Arguments passed to devpod (not including 'devpod' itself)
            capture: Whether output is being captured

        Returns:
            CompletedProcess with simulated results
        """
        self.calls.append(args)

        # Check if this command should fail
        if args and args[0] in self.fail_commands:
            return subprocess.CompletedProcess(
                args=["devpod"] + args,
                returncode=1,
                stdout="",
                stderr=self.fail_commands[args[0]],
            )

        # Handle specific commands
        if args and args[0] == "up":
            return self._handle_up(args)
        if args and args[0] == "stop":
            return self._handle_stop(args)
        if args and args[0] == "delete":
            return self._handle_delete(args)
        if args and args[0] == "list":
            return self._handle_list(args)
        if args and args[0] == "ssh":
            return self._handle_ssh(args)

        # Default success
        return subprocess.CompletedProcess(
            args=["devpod"] + args,
            returncode=0,
            stdout="",
            stderr="",
        )

    def _handle_up(self, args: List[str]) -> subprocess.CompletedProcess:
        """Handle 'devpod up' command."""
        workspace_id = None
        source = None

        # Parse arguments
        for i, arg in enumerate(args):
            if arg == "--id" and i + 1 < len(args):
                workspace_id = args[i + 1]
            elif not arg.startswith("-") and arg != "up":
                source = arg

        # If no --id, use source as workspace_id
        if workspace_id is None and source:
            workspace_id = source.replace("/", "-").replace(".", "-")

        if workspace_id:
            self.workspaces[workspace_id] = {
                "id": workspace_id,
                "source": source,
                "status": "running",
            }

        return subprocess.CompletedProcess(
            args=["devpod"] + args,
            returncode=0,
            stdout=f"Workspace {workspace_id} is ready",
            stderr="",
        )

    def _handle_stop(self, args: List[str]) -> subprocess.CompletedProcess:
        """Handle 'devpod stop' command."""
        if len(args) > 1:
            workspace_id = args[1]
            if workspace_id in self.workspaces:
                self.workspaces[workspace_id]["status"] = "stopped"

        return subprocess.CompletedProcess(
            args=["devpod"] + args,
            returncode=0,
            stdout="",
            stderr="",
        )

    def _handle_delete(self, args: List[str]) -> subprocess.CompletedProcess:
        """Handle 'devpod delete' command."""
        if len(args) > 1:
            workspace_id = args[1]
            if workspace_id in self.workspaces:
                del self.workspaces[workspace_id]

        return subprocess.CompletedProcess(
            args=["devpod"] + args,
            returncode=0,
            stdout="",
            stderr="",
        )

    def _handle_list(self, args: List[str]) -> subprocess.CompletedProcess:
        """Handle 'devpod list' command."""
        import json

        # Check if JSON output requested
        if "--output" in args and "json" in args:
            output = json.dumps(list(self.workspaces.values()))
        else:
            output = "\n".join(f"{ws['id']}: {ws['status']}" for ws in self.workspaces.values())

        return subprocess.CompletedProcess(
            args=["devpod"] + args,
            returncode=0,
            stdout=output,
            stderr="",
        )

    def _handle_ssh(self, args: List[str]) -> subprocess.CompletedProcess:
        """Handle 'devpod ssh' command."""
        # Find if there's a command after --
        cmd_output = ""
        if "--" in args:
            cmd_idx = args.index("--")
            cmd_args = args[cmd_idx + 1 :]
            if cmd_args and cmd_args[0] == "echo":
                cmd_output = " ".join(cmd_args[1:])

        return subprocess.CompletedProcess(
            args=["devpod"] + args,
            returncode=0,
            stdout=cmd_output,
            stderr="",
        )

    def set_fail(self, command: str, error_message: str) -> None:
        """Configure a command to fail.

        Args:
            command: The command to fail (e.g., "up", "stop")
            error_message: Error message to return
        """
        self.fail_commands[command] = error_message

    def clear_fail(self, command: Optional[str] = None) -> None:
        """Clear failure configuration.

        Args:
            command: Specific command to clear, or None to clear all
        """
        if command is None:
            self.fail_commands.clear()
        elif command in self.fail_commands:
            del self.fail_commands[command]

    def reset(self) -> None:
        """Reset all state."""
        self.calls.clear()
        self.workspaces.clear()
        self.fail_commands.clear()

    def patch(self):
        """Return a context manager that patches run_devpod.

        Usage:
            mock = DevPodMock()
            with mock.patch():
                # Test code here
                pass
        """
        return patch(
            "devlaunch.worktree.workspace_manager.run_devpod",
            side_effect=self,
        )

    def assert_called_with_command(self, command: str) -> bool:
        """Check if a specific command was called.

        Args:
            command: The command to check for (e.g., "up", "stop")

        Returns:
            True if the command was called
        """
        return any(call[0] == command for call in self.calls if call)

    def get_calls_for_command(self, command: str) -> List[List[str]]:
        """Get all calls for a specific command.

        Args:
            command: The command to filter by

        Returns:
            List of argument lists for matching calls
        """
        return [call for call in self.calls if call and call[0] == command]


@pytest.fixture
def mock_devpod() -> Generator[DevPodMock, None, None]:
    """Pytest fixture that provides a DevPodMock instance.

    The mock is automatically reset before each test.

    Usage in tests:
        def test_something(mock_devpod, real_managers):
            with mock_devpod.patch():
                # Create workspace - devpod calls are mocked
                workspace_manager = real_managers["workspace_manager"]
                workspace_manager.create_workspace("owner", "repo", "main", ...)

            # Verify calls
            assert mock_devpod.assert_called_with_command("up")
    """
    mock = DevPodMock()
    yield mock
    mock.reset()
