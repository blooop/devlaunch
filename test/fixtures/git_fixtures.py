"""Real git repository fixtures for integration tests.

These fixtures create actual git repositories in temp directories for testing
real git operations without mocking subprocess calls.
"""

import os
import subprocess
from pathlib import Path
from typing import Any, Dict, Generator, cast

import pytest

from devlaunch.worktree.config import WorktreeConfig
from devlaunch.worktree.repo_manager import RepositoryManager
from devlaunch.worktree.storage import MetadataStorage


@pytest.fixture
def isolated_devlaunch_env(tmp_path: Path) -> Generator[Dict[str, Path], None, None]:
    """Redirect devlaunch storage to temp directory via XDG_CACHE_HOME.

    This fixture isolates all devlaunch storage to a temporary directory by
    setting XDG_CACHE_HOME. This works because devlaunch/worktree/config.py
    and devlaunch/worktree/storage.py both honor XDG_CACHE_HOME.

    Yields:
        Dictionary containing paths to isolated directories:
        - cache_dir: The XDG_CACHE_HOME directory
        - devlaunch_dir: The devlaunch data directory
        - repos_dir: Directory for cloned repositories
        - metadata_path: Path to the metadata.json file
    """
    cache_dir = tmp_path / "cache"
    cache_dir.mkdir()

    # Save and set XDG_CACHE_HOME
    old_xdg = os.environ.get("XDG_CACHE_HOME")
    os.environ["XDG_CACHE_HOME"] = str(cache_dir)

    # Create devlaunch directory structure
    devlaunch_dir = cache_dir / "devlaunch"
    repos_dir = devlaunch_dir / "repos"
    repos_dir.mkdir(parents=True)
    metadata_path = devlaunch_dir / "metadata.json"

    yield {
        "cache_dir": cache_dir,
        "devlaunch_dir": devlaunch_dir,
        "repos_dir": repos_dir,
        "metadata_path": metadata_path,
        "tmp_path": tmp_path,
    }

    # Restore environment
    if old_xdg is None:
        os.environ.pop("XDG_CACHE_HOME", None)
    else:
        os.environ["XDG_CACHE_HOME"] = old_xdg


@pytest.fixture
def local_git_repo(tmp_path: Path) -> Dict[str, Any]:
    """Create a real local git repository as a 'remote'.

    Creates a bare git repository that can be used as a remote, along with
    a working copy that has commits and branches set up.

    Returns:
        Dictionary containing:
        - remote_url: Path to the bare repository (usable as git remote)
        - work_dir: Path to the working copy
        - branches: List of branch names available
        - default_branch: The default branch name
    """
    # Create bare repository (acts as "remote")
    remote_dir = tmp_path / "remote_repo.git"
    subprocess.run(
        ["git", "init", "--bare", "--initial-branch=main", str(remote_dir)],
        check=True,
        capture_output=True,
    )

    # Create working copy and set up commits
    work_dir = tmp_path / "work_repo"
    subprocess.run(
        ["git", "clone", str(remote_dir), str(work_dir)],
        check=True,
        capture_output=True,
    )

    # Ensure we're on main branch (needed for older git versions)
    subprocess.run(
        ["git", "checkout", "-b", "main"],
        cwd=work_dir,
        check=False,  # May fail if already on main
        capture_output=True,
    )

    # Configure git for commits
    subprocess.run(
        ["git", "config", "user.email", "test@example.com"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "Test User"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )

    # Create initial commit on main branch
    readme = work_dir / "README.md"
    readme.write_text("# Test Repository\n\nThis is a test repository.\n")
    subprocess.run(["git", "add", "README.md"], cwd=work_dir, check=True, capture_output=True)
    subprocess.run(
        ["git", "commit", "-m", "Initial commit"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )

    # Push to remote
    subprocess.run(
        ["git", "push", "-u", "origin", "main"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )

    # Create a feature branch with additional commits
    subprocess.run(
        ["git", "checkout", "-b", "feature/test"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )
    feature_file = work_dir / "feature.txt"
    feature_file.write_text("Feature content\n")
    subprocess.run(["git", "add", "feature.txt"], cwd=work_dir, check=True, capture_output=True)
    subprocess.run(
        ["git", "commit", "-m", "Add feature"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "push", "-u", "origin", "feature/test"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )

    # Go back to main
    subprocess.run(
        ["git", "checkout", "main"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )

    return {
        "remote_url": str(remote_dir),
        "work_dir": work_dir,
        "branches": ["main", "feature/test"],
        "default_branch": "main",
    }


@pytest.fixture
def local_git_repo_with_devcontainer(local_git_repo: Dict[str, Any]) -> Dict[str, Any]:  # pylint: disable=redefined-outer-name
    """Extend local_git_repo with a devcontainer.json file.

    This creates a repository that has devcontainer configuration,
    which is needed for DevPod to work without --fallback-image.
    """
    work_dir = cast(Path, local_git_repo["work_dir"])

    # Create devcontainer.json
    devcontainer_dir = work_dir / ".devcontainer"
    devcontainer_dir.mkdir()
    devcontainer_json = devcontainer_dir / "devcontainer.json"
    devcontainer_json.write_text(
        """{
    "name": "Test Container",
    "image": "mcr.microsoft.com/devcontainers/base:ubuntu"
}
"""
    )

    # Commit and push
    subprocess.run(
        ["git", "add", ".devcontainer/devcontainer.json"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "commit", "-m", "Add devcontainer configuration"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "push", "origin", "main"],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )

    return {
        **local_git_repo,
        "has_devcontainer": True,
    }


@pytest.fixture
def real_managers(
    isolated_devlaunch_env: Dict[str, Path],  # pylint: disable=redefined-outer-name
) -> Dict[str, Any]:
    """Create actual manager instances using isolated directories.

    This fixture creates real RepositoryManager instances that operate on
    isolated temp directories. The managers will perform real git operations,
    but DevPod calls need to be mocked separately for Tier 2 tests.

    Returns:
        Dictionary containing:
        - config: WorktreeConfig instance
        - storage: MetadataStorage instance
        - repo_manager: RepositoryManager instance
        - env: The isolated_devlaunch_env dict
    """
    env = isolated_devlaunch_env

    # Create config with auto_fetch disabled to avoid network calls
    config = WorktreeConfig(
        repos_dir=env["repos_dir"],
        auto_fetch=False,
        fetch_interval=0,
    )

    # Create storage with explicit metadata path
    storage = MetadataStorage(env["metadata_path"])

    # Create managers
    repo_manager = RepositoryManager(
        repos_dir=env["repos_dir"],
        storage=storage,
        config=config,
    )

    return {
        "config": config,
        "storage": storage,
        "repo_manager": repo_manager,
        "env": env,
    }
