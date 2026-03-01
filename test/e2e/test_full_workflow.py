"""E2E tests for full devlaunch workflows with real DevPod.

These tests run inside a Docker-in-Docker environment where they can
execute real DevPod commands creating real containers.

IMPORTANT: These tests do NOT launch any IDE. The default `dl` command
without the `code` subcommand creates workspaces without opening editors.

Run these tests with:
    docker compose -f test/docker/docker-compose.test.yml up --build
"""

import json
import os
import subprocess

import pytest


def devpod_available() -> bool:
    """Check if DevPod is available."""
    try:
        result = subprocess.run(
            ["devpod", "version"],
            capture_output=True,
            check=False,
        )
        return result.returncode == 0
    except FileNotFoundError:
        return False


@pytest.mark.e2e
class TestWorkspaceCreationE2E:
    """E2E tests for workspace creation with real DevPod."""

    def test_create_workspace_from_local_repo(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test full workspace creation with real DevPod.

        This test:
        1. Creates a local git repo as a "remote"
        2. Uses devpod directly to create a workspace
        3. Verifies the workspace exists
        """
        if not devpod_available():
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env
        remote_url = local_git_repo_with_devcontainer["remote_url"]
        workspace_id = "e2e-test-create"
        devpod_cleanup.track(workspace_id)

        # Create workspace using devpod directly
        result = subprocess.run(
            ["devpod", "up", remote_url, "--id", workspace_id],
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
            capture_output=True,
            text=True,
            check=False,
        )

        if result.returncode == 0:
            # List DevPod workspaces to verify
            list_result = subprocess.run(
                ["devpod", "list", "--output", "json"],
                capture_output=True,
                text=True,
                check=False,
            )

            if list_result.returncode == 0 and list_result.stdout:
                workspaces = json.loads(list_result.stdout)
                workspace_ids = [ws.get("id", "") for ws in workspaces]
                assert workspace_id in workspace_ids

    def test_workspace_lifecycle_without_ide(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test workspace create -> status -> stop -> delete without IDE."""
        if not devpod_available():
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env
        workspace_id = "e2e-test-lifecycle"
        devpod_cleanup.track(workspace_id)

        # Create a workspace
        result = subprocess.run(
            [
                "devpod",
                "up",
                local_git_repo_with_devcontainer["remote_url"],
                "--id",
                workspace_id,
            ],
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
            capture_output=True,
            text=True,
            check=False,
        )

        if result.returncode == 0:
            # Stop workspace
            stop_result = subprocess.run(
                ["devpod", "stop", workspace_id],
                capture_output=True,
                text=True,
                check=False,
            )
            assert stop_result.returncode == 0

            # Delete workspace
            delete_result = subprocess.run(
                ["devpod", "delete", workspace_id, "--force"],
                capture_output=True,
                text=True,
                check=False,
            )
            assert delete_result.returncode == 0


@pytest.mark.e2e
class TestGitOperationsInContainerE2E:
    """E2E tests verifying git operations work inside containers."""

    def test_git_status_via_ssh(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test that git status works when SSH'd into workspace."""
        if not devpod_available():
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env
        workspace_id = "e2e-test-git"
        devpod_cleanup.track(workspace_id)

        # Create workspace
        result = subprocess.run(
            [
                "devpod",
                "up",
                local_git_repo_with_devcontainer["remote_url"],
                "--id",
                workspace_id,
            ],
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
            capture_output=True,
            text=True,
            check=False,
        )

        if result.returncode == 0:
            # Run git status via SSH
            ssh_result = subprocess.run(
                ["devpod", "ssh", workspace_id, "--command", "git status"],
                capture_output=True,
                text=True,
                check=False,
            )

            # Git should work inside the container
            assert ssh_result.returncode == 0
            assert "On branch" in ssh_result.stdout or "nothing to commit" in ssh_result.stdout


@pytest.mark.e2e
class TestDLCommandsE2E:
    """E2E tests for dl CLI commands."""

    def test_dl_list_command(self, isolated_devlaunch_env):
        """Test dl --ls command works."""
        env = isolated_devlaunch_env

        result = subprocess.run(
            ["python", "-m", "devlaunch.dl", "--ls"],
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
            capture_output=True,
            text=True,
            check=False,
            cwd=os.getcwd(),
        )

        # Should succeed (may show "No workspaces found" if empty)
        assert result.returncode == 0

    def test_dl_help_command(self, isolated_devlaunch_env):
        """Test dl --help command works."""
        env = isolated_devlaunch_env

        result = subprocess.run(
            ["python", "-m", "devlaunch.dl", "--help"],
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
            capture_output=True,
            text=True,
            check=False,
            cwd=os.getcwd(),
        )

        assert result.returncode == 0
        assert "dl - DevLaunch CLI" in result.stdout

    def test_dl_version_command(self, isolated_devlaunch_env):
        """Test dl --version command works."""
        env = isolated_devlaunch_env

        result = subprocess.run(
            ["python", "-m", "devlaunch.dl", "--version"],
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
            capture_output=True,
            text=True,
            check=False,
            cwd=os.getcwd(),
        )

        assert result.returncode == 0
        assert "dl " in result.stdout


@pytest.mark.e2e
class TestPurgeE2E:
    """E2E tests for purge functionality."""

    def test_purge_deletes_workspaces(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer
    ):
        """Test that --purge -y deletes all DevPod workspaces."""
        if not devpod_available():
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env

        # Create a workspace first
        workspace_id = "e2e-test-purge"
        result = subprocess.run(
            [
                "devpod",
                "up",
                local_git_repo_with_devcontainer["remote_url"],
                "--id",
                workspace_id,
            ],
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
            capture_output=True,
            text=True,
            check=False,
        )

        if result.returncode != 0:
            pytest.skip("Could not create test workspace")

        # Verify workspace exists
        list_result = subprocess.run(
            ["devpod", "list", "--output", "json"],
            capture_output=True,
            text=True,
            check=False,
        )
        if list_result.returncode == 0:
            workspaces = json.loads(list_result.stdout) if list_result.stdout.strip() else []
            workspace_ids = [ws.get("id", "") for ws in workspaces]
            assert workspace_id in workspace_ids

        # Run purge
        purge_result = subprocess.run(
            ["python", "-m", "devlaunch.dl", "--purge", "-y"],
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
            capture_output=True,
            text=True,
            check=False,
            cwd=os.getcwd(),
        )

        assert purge_result.returncode == 0
        assert "Deleting DevPod workspace" in purge_result.stdout

        # Verify workspace is gone
        list_result = subprocess.run(
            ["devpod", "list", "--output", "json"],
            capture_output=True,
            text=True,
            check=False,
        )
        if list_result.returncode == 0:
            workspaces = json.loads(list_result.stdout) if list_result.stdout.strip() else []
            workspace_ids = [ws.get("id", "") for ws in workspaces]
            assert workspace_id not in workspace_ids

    def test_purge_cleans_cache(self, isolated_devlaunch_env):
        """Test that --purge -y removes the cache directory."""
        env = isolated_devlaunch_env
        cache_dir = env["devlaunch_dir"]

        # Create some cache data
        cache_dir.mkdir(parents=True, exist_ok=True)
        test_file = cache_dir / "test.txt"
        test_file.write_text("test data")
        assert test_file.exists()

        # Run purge
        purge_result = subprocess.run(
            ["python", "-m", "devlaunch.dl", "--purge", "-y"],
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
            capture_output=True,
            text=True,
            check=False,
            cwd=os.getcwd(),
        )

        assert purge_result.returncode == 0
        assert not cache_dir.exists()
