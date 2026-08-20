"""Real git repository fixtures for integration tests.

These fixtures create actual git repositories in temp directories for testing
real git operations without mocking subprocess calls.
"""

import os
import subprocess
from pathlib import Path
from typing import Any, Dict, Generator, cast

import pytest


@pytest.fixture
def isolated_devlaunch_env(tmp_path: Path) -> Generator[Dict[str, Path], None, None]:
    """Redirect devlaunch storage to temp directory via XDG_CACHE_HOME.

    This fixture isolates all devlaunch storage to a temporary directory by
    setting XDG_CACHE_HOME. This works because everything `dl` stores resolves
    through it -- `rust/devlaunch-core/src/domain/xdg.rs` is the one place that
    is decided.

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


# The devcontainer every fixture here writes. One image for the whole suite: an
# e2e run pays for each distinct image it pulls, and nothing under test depends
# on which one it is beyond it holding a shell and a devpod agent.
DEVCONTAINER_JSON = """{
    "name": "Test Container",
    "image": "mcr.microsoft.com/devcontainers/base:ubuntu"
}
"""


def build_repo_with_devcontainer(root: Path) -> Path:
    """A committed git repo with a devcontainer, built outside pytest's scoping.

    The same thing `local_git_repo_with_devcontainer` yields, reachable from a
    scope that cannot ask for it: fixtures are resolved by scope, and a
    module-scoped fixture requesting a function-scoped one is an error. A module
    that builds one workspace for all its tests needs the repo to live as long as
    the workspace does, so it calls this instead.

    Smaller than the fixture chain on purpose -- one branch, one commit, no bare
    stand-in remote. What devpod is given is a working copy, so a work tree is
    the whole requirement; the branches the fixture publishes exist for tests
    about resolving branches, and this is not one.

    Returns the working copy, which is what `devpod up` takes as its source.
    """
    root.mkdir(parents=True, exist_ok=True)

    def run(*args: str) -> None:
        subprocess.run(list(args), cwd=root, check=True, capture_output=True)

    run("git", "init", "--initial-branch=main")
    run("git", "config", "user.email", "test@example.com")
    run("git", "config", "user.name", "Test User")
    (root / "README.md").write_text("# Test Repository\n")
    devcontainer = root / ".devcontainer"
    devcontainer.mkdir(exist_ok=True)
    (devcontainer / "devcontainer.json").write_text(DEVCONTAINER_JSON)
    run("git", "add", "-A")
    run("git", "commit", "-m", "Initial commit")
    return root


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
    devcontainer_json.write_text(DEVCONTAINER_JSON)

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
