"""E2E tests for full devlaunch workflows with real DevPod.

These tests execute real DevPod commands creating real containers, so they
need a Docker daemon. Run them from inside this repo's devcontainer, which
carries a daemon of its own, or on a machine you do not mind them writing to.

IMPORTANT: These tests do NOT launch any IDE. The default `dl` command
without the `code` subcommand creates workspaces without opening editors.

Run these tests with:
    pixi run test-e2e
"""

import json
import os
import subprocess
from pathlib import Path

import pytest

from fixtures.e2e_helpers import create_e2e_workspace, devpod_available


def real_devpod_workspace_ids() -> set:
    """Workspace ids in the developer's own ~/.devpod, read straight off disk.

    Read from the filesystem rather than from `devpod list`, because the whole
    point of the assertion below is that `devpod list` no longer answers for
    that directory.
    """
    contexts = Path.home() / ".devpod" / "contexts"
    if not contexts.is_dir():
        return set()
    return {
        workspace.name
        for context in contexts.iterdir()
        for workspace in (context / "workspaces").glob("*")
        if workspace.is_dir()
    }


@pytest.mark.e2e
class TestSuiteIsolationE2E:
    """The destructive half of this suite must not be able to reach real state."""

    def test_devpod_in_this_session_cannot_see_the_developers_workspaces(self):
        """Proves the scoping is live in the session that could do the damage.

        Every devpod call in this file -- including the one inside `dl --purge`
        -- inherits this process's environment, so what `devpod list` reports
        here is exactly what `--purge` would delete.

        Any set at all is disjoint from an empty one, so where there is no real
        devpod state to be disjoint from -- CI, or a fresh DinD container --
        this skips rather than passing on nothing.
        """
        if not devpod_available():
            pytest.skip("DevPod not available")

        real_ids = real_devpod_workspace_ids()
        if not real_ids:
            pytest.skip("no workspaces in ~/.devpod on this host; nothing to be isolated from")

        result = subprocess.run(
            ["devpod", "list", "--output", "json"],
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0

        visible = {ws.get("id", "") for ws in json.loads(result.stdout or "[]")}
        assert visible.isdisjoint(real_ids)


def workspace_ids() -> list:
    """The ids devpod currently knows about.

    A listing that could not be read is not a listing with nothing in it, so
    this raises rather than handing back an empty list an assertion would then
    read as "the workspace is gone".
    """
    result = subprocess.run(
        ["devpod", "list", "--output", "json"],
        capture_output=True,
        text=True,
        check=True,
    )
    workspaces = json.loads(result.stdout) if result.stdout.strip() else []
    return [ws.get("id", "") for ws in workspaces]


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

        create_e2e_workspace(
            remote_url,
            workspace_id,
            cleanup=devpod_cleanup,
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
        )

        assert workspace_id in workspace_ids()

    def test_workspace_lifecycle_without_ide(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test workspace create -> status -> stop -> delete without IDE."""
        if not devpod_available():
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env
        workspace_id = "e2e-test-lifecycle"

        create_e2e_workspace(
            local_git_repo_with_devcontainer["remote_url"],
            workspace_id,
            cleanup=devpod_cleanup,
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
        )

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

        create_e2e_workspace(
            local_git_repo_with_devcontainer["remote_url"],
            workspace_id,
            cleanup=devpod_cleanup,
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
        )

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
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test that --purge -y deletes all DevPod workspaces."""
        if not devpod_available():
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env

        # Create a workspace first
        workspace_id = "e2e-test-purge"
        create_e2e_workspace(
            local_git_repo_with_devcontainer["remote_url"],
            workspace_id,
            cleanup=devpod_cleanup,
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
        )

        # Verify workspace exists
        assert workspace_id in workspace_ids()

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
        assert workspace_id not in workspace_ids()

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
