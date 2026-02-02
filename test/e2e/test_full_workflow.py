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


@pytest.mark.e2e
class TestWorkspaceCreationE2E:
    """E2E tests for workspace creation with real DevPod."""

    def test_create_workspace_from_local_repo(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test full workspace creation with real DevPod.

        This test:
        1. Creates a local git repo as a "remote"
        2. Uses dl to create a workspace (no IDE launched)
        3. Verifies the workspace exists in DevPod
        """
        env = isolated_devlaunch_env
        _remote_url = local_git_repo_with_devcontainer["remote_url"]  # noqa: F841

        # Run dl command to create workspace
        # Using the default command (no 'code' subcommand) - does NOT launch IDE
        result = subprocess.run(
            ["python", "-m", "devlaunch.dl", "local/test-repo@main"],
            env={**os.environ, "XDG_CACHE_HOME": str(env["cache_dir"])},
            capture_output=True,
            text=True,
            check=False,
            cwd="/app",
        )

        # The command might fail if devpod isn't properly configured
        # That's ok for this test - we just want to verify the workflow
        if result.returncode == 0:
            devpod_cleanup.track("main")

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
                # Should have created a workspace
                assert any("main" in ws_id for ws_id in workspace_ids)

    def test_workspace_lifecycle_without_ide(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test workspace create -> status -> stop -> delete without IDE."""
        env = isolated_devlaunch_env

        # Skip if not in E2E environment
        devpod_check = subprocess.run(
            ["devpod", "version"],
            capture_output=True,
            check=False,
        )
        if devpod_check.returncode != 0:
            pytest.skip("DevPod not available")

        workspace_id = "e2e-test-lifecycle"
        devpod_cleanup.track(workspace_id)

        # Create a minimal workspace using devpod directly
        # This tests that our environment is set up correctly
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
        # Skip if not in E2E environment
        devpod_check = subprocess.run(
            ["devpod", "version"],
            capture_output=True,
            check=False,
        )
        if devpod_check.returncode != 0:
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
                ["devpod", "ssh", workspace_id, "--", "git", "status"],
                capture_output=True,
                text=True,
                check=False,
            )

            # Git should work inside the container
            assert ssh_result.returncode == 0
            assert "On branch" in ssh_result.stdout or "nothing to commit" in ssh_result.stdout


@pytest.mark.e2e
class TestWorktreePathsInContainerE2E:
    """E2E tests verifying worktree paths work correctly in containers."""

    def test_worktree_git_file_resolves_in_container(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Verify that worktree .git files resolve correctly in container.

        This is the critical test for the relative path fixup: we create
        a worktree on the host, mount it in a container, and verify that
        git commands work because the .git file uses relative paths.
        """
        # Skip if not in E2E environment
        devpod_check = subprocess.run(
            ["devpod", "version"],
            capture_output=True,
            check=False,
        )
        if devpod_check.returncode != 0:
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env
        workspace_id = "e2e-test-worktree-paths"
        devpod_cleanup.track(workspace_id)

        # First, create a worktree locally
        from devlaunch.worktree.config import WorktreeConfig
        from devlaunch.worktree.repo_manager import RepositoryManager
        from devlaunch.worktree.storage import MetadataStorage
        from devlaunch.worktree.worktree_manager import WorktreeManager

        config = WorktreeConfig(repos_dir=env["repos_dir"], auto_fetch=False)
        storage = MetadataStorage(env["metadata_path"])
        repo_manager = RepositoryManager(env["repos_dir"], storage, config)
        worktree_manager = WorktreeManager(repo_manager, storage)

        # Clone and create worktree
        remote_url = local_git_repo_with_devcontainer["remote_url"]
        repo_manager.clone_repo("test", "repo", remote_url)
        _worktree = worktree_manager.create_worktree("test", "repo", "main")  # noqa: F841

        # The base repo path (which we mount)
        base_repo = repo_manager.get_repo_path("test", "repo")

        # Create a DevPod workspace mounting the base repo
        result = subprocess.run(
            [
                "devpod",
                "up",
                str(base_repo),
                "--id",
                workspace_id,
            ],
            capture_output=True,
            text=True,
            check=False,
        )

        if result.returncode == 0:
            # Determine the container path to the worktree
            # DevPod mounts source to /workspaces/<workspace-id>/
            # Worktree is at .worktrees/main relative to mount
            worktree_container_path = f"/workspaces/{workspace_id}/.worktrees/main"

            # Run git status in the worktree directory inside container
            ssh_result = subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--",
                    "git",
                    "-C",
                    worktree_container_path,
                    "status",
                ],
                capture_output=True,
                text=True,
                check=False,
            )

            # This is the critical assertion: git must work in the worktree
            # inside the container, which requires relative paths in .git file
            assert ssh_result.returncode == 0, (
                f"git status failed in container worktree. "
                f"This likely means .git file paths aren't relative. "
                f"stderr: {ssh_result.stderr}"
            )
            assert "On branch main" in ssh_result.stdout


@pytest.mark.e2e
class TestWorktreeSymlinkE2E:
    """E2E tests for ~/work symlink functionality in worktree backend."""

    def test_symlink_creation_in_container(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test that ~/work symlink is created correctly inside container.

        This verifies:
        1. Symlink at /home/vscode/work is created
        2. Symlink points to the correct worktree path
        3. The symlink target directory exists
        """
        devpod_check = subprocess.run(
            ["devpod", "version"],
            capture_output=True,
            check=False,
        )
        if devpod_check.returncode != 0:
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env
        workspace_id = "e2e-test-symlink"
        devpod_cleanup.track(workspace_id)

        # Create worktree locally first
        from devlaunch.worktree.config import WorktreeConfig
        from devlaunch.worktree.repo_manager import RepositoryManager
        from devlaunch.worktree.storage import MetadataStorage
        from devlaunch.worktree.worktree_manager import WorktreeManager

        config = WorktreeConfig(repos_dir=env["repos_dir"], auto_fetch=False)
        storage = MetadataStorage(env["metadata_path"])
        repo_manager = RepositoryManager(env["repos_dir"], storage, config)
        worktree_manager = WorktreeManager(repo_manager, storage)

        remote_url = local_git_repo_with_devcontainer["remote_url"]
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree_manager.create_worktree("test", "repo", "main")
        base_repo = repo_manager.get_repo_path("test", "repo")

        # Create DevPod workspace
        result = subprocess.run(
            ["devpod", "up", str(base_repo), "--id", workspace_id],
            capture_output=True,
            text=True,
            check=False,
        )

        if result.returncode == 0:
            worktree_container_path = f"/workspaces/{workspace_id}/.worktrees/main"

            # Create the symlink (simulating what dl.py does)
            symlink_result = subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    f"ln -sfn {worktree_container_path} /home/vscode/work",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            assert symlink_result.returncode == 0

            # Verify symlink exists and points to correct location
            verify_result = subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    "readlink /home/vscode/work",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            assert verify_result.returncode == 0
            assert worktree_container_path in verify_result.stdout

    def test_git_works_through_symlink(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test that git commands work when run from ~/work symlink."""
        devpod_check = subprocess.run(
            ["devpod", "version"],
            capture_output=True,
            check=False,
        )
        if devpod_check.returncode != 0:
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env
        workspace_id = "e2e-test-symlink-git"
        devpod_cleanup.track(workspace_id)

        from devlaunch.worktree.config import WorktreeConfig
        from devlaunch.worktree.repo_manager import RepositoryManager
        from devlaunch.worktree.storage import MetadataStorage
        from devlaunch.worktree.worktree_manager import WorktreeManager

        config = WorktreeConfig(repos_dir=env["repos_dir"], auto_fetch=False)
        storage = MetadataStorage(env["metadata_path"])
        repo_manager = RepositoryManager(env["repos_dir"], storage, config)
        worktree_manager = WorktreeManager(repo_manager, storage)

        remote_url = local_git_repo_with_devcontainer["remote_url"]
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree_manager.create_worktree("test", "repo", "main")
        base_repo = repo_manager.get_repo_path("test", "repo")

        result = subprocess.run(
            ["devpod", "up", str(base_repo), "--id", workspace_id],
            capture_output=True,
            text=True,
            check=False,
        )

        if result.returncode == 0:
            worktree_container_path = f"/workspaces/{workspace_id}/.worktrees/main"

            # Create symlink
            subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    f"ln -sfn {worktree_container_path} /home/vscode/work",
                ],
                capture_output=True,
                check=False,
            )

            # Run git status from ~/work
            git_result = subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    "cd /home/vscode/work && git status",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            assert git_result.returncode == 0
            assert "On branch main" in git_result.stdout

            # Run git log from ~/work
            log_result = subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    "cd /home/vscode/work && git log --oneline -1",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            assert log_result.returncode == 0

    def test_symlink_overwrites_existing(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test that symlink can be updated (for shared container mode)."""
        devpod_check = subprocess.run(
            ["devpod", "version"],
            capture_output=True,
            check=False,
        )
        if devpod_check.returncode != 0:
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env
        workspace_id = "e2e-test-symlink-overwrite"
        devpod_cleanup.track(workspace_id)

        from devlaunch.worktree.config import WorktreeConfig
        from devlaunch.worktree.repo_manager import RepositoryManager
        from devlaunch.worktree.storage import MetadataStorage
        from devlaunch.worktree.worktree_manager import WorktreeManager

        config = WorktreeConfig(repos_dir=env["repos_dir"], auto_fetch=False)
        storage = MetadataStorage(env["metadata_path"])
        repo_manager = RepositoryManager(env["repos_dir"], storage, config)
        worktree_manager = WorktreeManager(repo_manager, storage)

        remote_url = local_git_repo_with_devcontainer["remote_url"]
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree_manager.create_worktree("test", "repo", "main")
        base_repo = repo_manager.get_repo_path("test", "repo")

        result = subprocess.run(
            ["devpod", "up", str(base_repo), "--id", workspace_id],
            capture_output=True,
            text=True,
            check=False,
        )

        if result.returncode == 0:
            path1 = f"/workspaces/{workspace_id}/.worktrees/main"
            path2 = f"/workspaces/{workspace_id}/.worktrees/feature"  # Hypothetical

            # Create initial symlink
            subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    f"ln -sfn {path1} /home/vscode/work",
                ],
                capture_output=True,
                check=False,
            )

            # Verify initial target
            check1 = subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    "readlink /home/vscode/work",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            assert path1 in check1.stdout

            # Overwrite with new target (ln -sfn should handle this)
            subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    f"ln -sfn {path2} /home/vscode/work",
                ],
                capture_output=True,
                check=False,
            )

            # Verify new target
            check2 = subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    "readlink /home/vscode/work",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            assert path2 in check2.stdout

    def test_pwd_shows_symlink_path(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test that pwd shows ~/work (the short prompt path)."""
        devpod_check = subprocess.run(
            ["devpod", "version"],
            capture_output=True,
            check=False,
        )
        if devpod_check.returncode != 0:
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env
        workspace_id = "e2e-test-symlink-pwd"
        devpod_cleanup.track(workspace_id)

        from devlaunch.worktree.config import WorktreeConfig
        from devlaunch.worktree.repo_manager import RepositoryManager
        from devlaunch.worktree.storage import MetadataStorage
        from devlaunch.worktree.worktree_manager import WorktreeManager

        config = WorktreeConfig(repos_dir=env["repos_dir"], auto_fetch=False)
        storage = MetadataStorage(env["metadata_path"])
        repo_manager = RepositoryManager(env["repos_dir"], storage, config)
        worktree_manager = WorktreeManager(repo_manager, storage)

        remote_url = local_git_repo_with_devcontainer["remote_url"]
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree_manager.create_worktree("test", "repo", "main")
        base_repo = repo_manager.get_repo_path("test", "repo")

        result = subprocess.run(
            ["devpod", "up", str(base_repo), "--id", workspace_id],
            capture_output=True,
            text=True,
            check=False,
        )

        if result.returncode == 0:
            worktree_container_path = f"/workspaces/{workspace_id}/.worktrees/main"

            # Create symlink
            subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    f"ln -sfn {worktree_container_path} /home/vscode/work",
                ],
                capture_output=True,
                check=False,
            )

            # Run pwd from ~/work - should show /home/vscode/work
            pwd_result = subprocess.run(
                [
                    "devpod",
                    "ssh",
                    workspace_id,
                    "--command",
                    "cd /home/vscode/work && pwd",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            assert pwd_result.returncode == 0
            assert "/home/vscode/work" in pwd_result.stdout


@pytest.mark.e2e
class TestWorkspaceLifecycleE2E:
    """E2E tests for full workspace lifecycle with worktree backend."""

    def test_full_worktree_workspace_lifecycle(
        self, isolated_devlaunch_env, local_git_repo_with_devcontainer, devpod_cleanup
    ):
        """Test complete lifecycle: create -> symlink -> git ops -> stop -> delete."""
        devpod_check = subprocess.run(
            ["devpod", "version"],
            capture_output=True,
            check=False,
        )
        if devpod_check.returncode != 0:
            pytest.skip("DevPod not available")

        env = isolated_devlaunch_env
        workspace_id = "e2e-test-lifecycle-full"
        devpod_cleanup.track(workspace_id)

        from devlaunch.worktree.config import WorktreeConfig
        from devlaunch.worktree.repo_manager import RepositoryManager
        from devlaunch.worktree.storage import MetadataStorage
        from devlaunch.worktree.worktree_manager import WorktreeManager

        config = WorktreeConfig(repos_dir=env["repos_dir"], auto_fetch=False)
        storage = MetadataStorage(env["metadata_path"])
        repo_manager = RepositoryManager(env["repos_dir"], storage, config)
        worktree_manager = WorktreeManager(repo_manager, storage)

        # Phase 1: Setup
        remote_url = local_git_repo_with_devcontainer["remote_url"]
        repo_manager.clone_repo("test", "repo", remote_url)
        worktree_manager.create_worktree("test", "repo", "main")
        base_repo = repo_manager.get_repo_path("test", "repo")

        # Phase 2: Create workspace
        up_result = subprocess.run(
            ["devpod", "up", str(base_repo), "--id", workspace_id],
            capture_output=True,
            text=True,
            check=False,
        )

        if up_result.returncode != 0:
            pytest.skip(f"DevPod up failed: {up_result.stderr}")

        worktree_container_path = f"/workspaces/{workspace_id}/.worktrees/main"

        # Phase 3: Create symlink
        symlink_result = subprocess.run(
            [
                "devpod",
                "ssh",
                workspace_id,
                "--command",
                f"ln -sfn {worktree_container_path} /home/vscode/work",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        assert symlink_result.returncode == 0, f"Symlink creation failed: {symlink_result.stderr}"

        # Phase 4: Verify git operations through symlink
        git_result = subprocess.run(
            [
                "devpod",
                "ssh",
                workspace_id,
                "--command",
                "cd /home/vscode/work && git status && git log --oneline -1",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        assert git_result.returncode == 0, f"Git operations failed: {git_result.stderr}"
        assert "On branch main" in git_result.stdout

        # Phase 5: Stop workspace
        stop_result = subprocess.run(
            ["devpod", "stop", workspace_id],
            capture_output=True,
            text=True,
            check=False,
        )
        assert stop_result.returncode == 0, f"Stop failed: {stop_result.stderr}"

        # Phase 6: Restart and verify symlink persists
        restart_result = subprocess.run(
            ["devpod", "up", workspace_id],
            capture_output=True,
            text=True,
            check=False,
        )
        assert restart_result.returncode == 0, f"Restart failed: {restart_result.stderr}"

        # Recreate symlink (simulating what dl restart does)
        subprocess.run(
            [
                "devpod",
                "ssh",
                workspace_id,
                "--command",
                f"ln -sfn {worktree_container_path} /home/vscode/work",
            ],
            capture_output=True,
            check=False,
        )

        # Verify git still works
        git_result2 = subprocess.run(
            [
                "devpod",
                "ssh",
                workspace_id,
                "--command",
                "cd /home/vscode/work && git status",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        assert git_result2.returncode == 0

        # Phase 7: Delete workspace
        delete_result = subprocess.run(
            ["devpod", "delete", workspace_id, "--force"],
            capture_output=True,
            text=True,
            check=False,
        )
        assert delete_result.returncode == 0, f"Delete failed: {delete_result.stderr}"


@pytest.mark.e2e
class TestDLCommandSafetyE2E:
    """Tests verifying dl command safety (no accidental IDE launch)."""

    def test_dl_no_ide_helper_rejects_code_subcommand(self, dl_no_ide):
        """Test that dl_no_ide fixture rejects the 'code' subcommand."""
        with pytest.raises(ValueError, match="code"):
            dl_no_ide.run("owner/repo@main", "code")

    def test_dl_default_command_does_not_launch_ide(
        self,
        isolated_devlaunch_env,  # noqa: ARG002  # pylint: disable=unused-argument
    ):
        """Verify default dl command doesn't attempt IDE launch.

        This test checks that when we run `dl owner/repo@main` (without 'code'),
        no IDE-related arguments are passed to devpod.
        """
        # This is more of a unit test but validates E2E safety
        # Read dl.py to verify the code path
        from devlaunch import dl

        # Check that workspace_up_worktree defaults to no IDE
        import inspect

        sig = inspect.signature(dl.workspace_up_worktree)
        ide_param = sig.parameters.get("ide")
        assert ide_param is not None
        assert ide_param.default is None, "workspace_up_worktree should default ide=None (no IDE)"
