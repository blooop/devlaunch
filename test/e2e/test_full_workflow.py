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

from fixtures.e2e_guard import opt_out
from fixtures.e2e_helpers import create_e2e_workspace


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
        this opts out rather than passing on nothing. That is a genuine opt-out
        and not a thing that went wrong, which is why it goes through the call
        the guard recognises: on a runner it is the only skip in the file, and
        an unexplained one there would be the interesting kind.
        """
        real_ids = real_devpod_workspace_ids()
        if not real_ids:
            opt_out("no workspaces in ~/.devpod on this host; nothing to be isolated from")

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

    @pytest.mark.creates_workspace
    def test_create_workspace_from_local_repo(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test full workspace creation with real DevPod.

        This test:
        1. Creates a local git repo as a "remote"
        2. Uses devpod directly to create a workspace
        3. Verifies the workspace exists
        """
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

    @pytest.mark.creates_workspace
    def test_workspace_lifecycle_without_ide(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test workspace create -> status -> stop -> delete without IDE."""
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

    @pytest.mark.creates_workspace
    def test_git_status_via_ssh(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test that git status works when SSH'd into workspace.

        The source is the working copy and not the bare repo its siblings use.
        A bare repo has no work tree, and devpod opens a local path as the
        workspace folder rather than cloning it, so pointing this test at the
        fixture's stand-in remote put the container inside `objects/`, `refs/`
        and a `core.bare=true` config -- where `git status` exits 128 saying so,
        on every machine, always. It had never once been observed doing anything
        else: until the creation step was made unskippable, `devpod up` without
        `--ide none` exited 1 on any headless machine and both assertions below
        sat behind an `if` that was never true. The first run that reached them
        was the first run of this suite in CI.
        """
        env = isolated_devlaunch_env
        workspace_id = "e2e-test-git"

        create_e2e_workspace(
            str(local_git_repo_with_devcontainer["work_dir"]),
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

        # Git should work inside the container. The output goes in the message
        # because devpod reports a failed remote command as its own exit 1 and
        # buries git's line in its logging, so a bare rc comparison says only
        # that something went wrong somewhere in the tunnel.
        assert ssh_result.returncode == 0, (
            f"`git status` over devpod ssh exited {ssh_result.returncode}\n"
            f"stdout: {ssh_result.stdout}\nstderr: {ssh_result.stderr}"
        )
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

    @pytest.mark.creates_workspace
    def test_purge_deletes_devlaunchs_workspaces_and_leaves_everyone_elses(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Two real workspaces, one of each kind, and `--purge -y` sorts them.

        The second half used to be asserted the other way round -- that `--purge`
        deleted the workspace `devpod up` had just built for it -- which is the
        defect #107 is about, written down as an assertion. devpod's namespace is
        shared, and a workspace made by hand is somebody's work devlaunch cannot
        recreate.

        Both are built with `devpod up <source> --id <id>`, and the only thing
        separating them is where the source lives. That is exactly the claim the
        predicate rests on, so it is the thing worth measuring on a real devpod
        rather than a recorded listing: what devlaunch creates is a clone under
        its own cache, and devpod records that path back as the workspace source.
        """
        env = isolated_devlaunch_env
        devpod_env = {**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])}
        remote_url = local_git_repo_with_devcontainer["remote_url"]

        # A workspace in the shape devlaunch creates them: a clone it made under
        # its own cache, handed to devpod as a path. `dl owner/repo` does exactly
        # this -- see WorkspaceCloneManager.ensure_workspace.
        # Neither id may contain the other. `e2e-test-purge` inside
        # `e2e-test-purge-mine` made `"Deleting ... e2e-test-purge" not in
        # stdout` fail on correct output, and `theirs in stdout` pass on no
        # evidence -- both from the same substring.
        mine = "e2e-purge-devlaunchs"
        clone = env["repos_dir"] / "blooop" / "e2e-repo" / mine
        clone.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "clone", remote_url, str(clone)], check=True, capture_output=True)
        create_e2e_workspace(str(clone), mine, cleanup=devpod_cleanup, env=devpod_env)

        # And one devlaunch did not create: same command, a source outside its cache.
        theirs = "e2e-purge-hand-made"
        create_e2e_workspace(remote_url, theirs, cleanup=devpod_cleanup, env=devpod_env)

        listed = workspace_ids()
        assert mine in listed
        assert theirs in listed

        # Run purge
        purge_result = subprocess.run(
            ["python", "-m", "devlaunch.dl", "--purge", "-y"],
            env=devpod_env,
            capture_output=True,
            text=True,
            check=False,
            cwd=os.getcwd(),
        )

        # The output goes in every message: `--purge` reports what it deleted,
        # what it left and what it could not remove on stdout, and a bare rc
        # comparison says only that something somewhere went wrong.
        report = (
            f"`dl --purge -y` exited {purge_result.returncode}\n"
            f"stdout: {purge_result.stdout}\nstderr: {purge_result.stderr}"
        )
        # Whole lines, not substrings: `x in stdout` is what let one id being a
        # prefix of the other assert nothing at all.
        printed = purge_result.stdout.splitlines()
        assert [line for line in printed if line.startswith("Deleting DevPod workspace:")] == [
            f"Deleting DevPod workspace: {mine}"
        ], report
        # Named rather than silently passed over.
        assert "Leaving 1 workspace(s) devlaunch did not create:" in printed, report
        assert f"  - {theirs}" in printed, report

        listed_after = workspace_ids()
        assert mine not in listed_after, report
        assert theirs in listed_after, report

        # The cache half of the purge is a different subject, and on a runner it
        # is refused for a reason that has nothing to do with which workspaces
        # are devlaunch's. The container writes into the bind-mounted clone as
        # its own user -- `vscode`, uid 1000 in the fixture image -- and where
        # that is not the uid running the suite (`runner` is 1001), those files
        # are not this process's to remove and no exit code can make them so.
        #
        # What a purge does about it is what changed: it removes everything it
        # is permitted to and names the rest, instead of stopping at the first
        # refusal and leaving the removable half of the cache standing. So a
        # non-zero exit here is still expected on a runner, and must be that
        # refusal reported in the way a person can act on -- any other non-zero
        # exit is this test's to fail on. `test_purge_cleans_cache` covers the
        # cache half against a cache no container has touched.
        if purge_result.returncode != 0:
            assert "Not permitted to remove:" in printed, report
            assert [
                line
                for line in printed
                if line.startswith(("Removed what it could from ", "Removed nothing from "))
            ], report

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
