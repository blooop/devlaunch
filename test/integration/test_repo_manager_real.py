"""Integration tests for RepositoryManager with real git operations.

These tests run real git commands against temporary local repositories.
They verify that git command construction, cloning, and fetching work correctly.
"""
# pylint: disable=redefined-outer-name

import subprocess
from pathlib import Path

import pytest

from devlaunch.worktree.workspace_clone import WorkspaceCloneManager


@pytest.mark.integration
class TestRepoManagerRealClone:
    """Tests for real git clone operations."""

    def test_clone_from_local_remote(self, real_managers, local_git_repo):
        """Test cloning a repository from a local 'remote'."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone the repository
        result = repo_manager.clone_repo("test", "repo", remote_url)

        assert result is not None
        assert result.owner == "test"
        assert result.repo == "repo"
        assert result.remote_url == remote_url

        # Verify the clone is a bare repo inside .bare/
        bare_path = repo_manager.get_bare_path("test", "repo")
        assert bare_path.exists()
        # Bare repos have HEAD directly in the directory
        assert (bare_path / "HEAD").exists()
        # The parent repo dir should exist but .bare should not have .git
        assert not (bare_path / ".git").exists()

    def test_clone_preserves_branches(self, real_managers, local_git_repo):
        """Test that cloning preserves all branches from remote."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone the repository
        repo_manager.clone_repo("test", "repo", remote_url)
        bare_path = repo_manager.get_bare_path("test", "repo")

        # List branches in the bare repo
        result = subprocess.run(
            ["git", "branch", "-a"],
            cwd=bare_path,
            capture_output=True,
            text=True,
            check=True,
        )

        # Should have main and feature/test
        assert "main" in result.stdout
        assert "feature/test" in result.stdout or "feature-test" in result.stdout

    def test_clone_idempotent(self, real_managers, local_git_repo):
        """Test that cloning the same repo twice returns existing repo."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone twice
        result1 = repo_manager.clone_repo("test", "repo", remote_url)
        result2 = repo_manager.clone_repo("test", "repo", remote_url)

        assert result1 is not None
        assert result2 is not None
        assert result1.owner == result2.owner
        assert result1.repo == result2.repo

    def test_clone_invalid_url_fails(self, real_managers):
        """Test that cloning from invalid URL raises error."""
        repo_manager = real_managers["repo_manager"]

        with pytest.raises(RuntimeError, match="Failed to clone"):
            repo_manager.clone_repo("test", "repo", "/nonexistent/path.git")


@pytest.mark.integration
class TestRepoManagerRealFetch:
    """Tests for real git fetch operations."""

    def test_fetch_after_clone(self, real_managers, local_git_repo):
        """Test fetching updates after initial clone."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]
        work_dir = local_git_repo["work_dir"]

        # Clone the repository
        repo_manager.clone_repo("test", "repo", remote_url)
        bare_path = repo_manager.get_bare_path("test", "repo")

        # Get initial commit count
        before_result = subprocess.run(
            ["git", "rev-list", "--count", "HEAD"],
            cwd=bare_path,
            capture_output=True,
            text=True,
            check=True,
        )
        count_before = int(before_result.stdout.strip())

        # Make a new commit in the remote working copy
        new_file = work_dir / "new_file.txt"
        new_file.write_text("new content")
        subprocess.run(["git", "add", "new_file.txt"], cwd=work_dir, check=True)
        subprocess.run(
            ["git", "commit", "-m", "Add new file"],
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

        # For bare repos cloned with --bare, we need to fetch and update the local branch
        # The remote is configured as origin, so fetch will update origin/* refs
        # But the local heads need to be updated too

        # First, verify fetch completes without error
        repo_manager.fetch_repo("test", "repo")

        # After fetch, the new commit should be reachable
        # Check that we can see the new commit via rev-list
        after_result = subprocess.run(
            ["git", "rev-list", "--count", "--all"],
            cwd=bare_path,
            capture_output=True,
            text=True,
            check=True,
        )
        count_after = int(after_result.stdout.strip())

        # Should have at least one more commit after fetch
        assert count_after > count_before, (
            f"Expected more commits after fetch. Before: {count_before}, After: {count_after}"
        )

    def test_fetch_nonexistent_repo_fails(self, real_managers):
        """Test that fetching non-existent repo raises error."""
        repo_manager = real_managers["repo_manager"]

        with pytest.raises(ValueError, match="does not exist"):
            repo_manager.fetch_repo("nonexistent", "repo")


@pytest.mark.integration
class TestRepoManagerEnsure:
    """Tests for ensure_repo which combines clone and fetch."""

    def test_ensure_clones_if_not_exists(self, real_managers, local_git_repo):
        """Test ensure_repo clones if repo doesn't exist."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Ensure repo (should clone)
        result = repo_manager.ensure_repo("test", "repo", remote_url)

        assert result is not None
        assert result.owner == "test"
        assert repo_manager.repo_exists("test", "repo")

    def test_ensure_returns_existing(self, real_managers, local_git_repo):
        """Test ensure_repo returns existing repo without re-cloning."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone first
        repo_manager.clone_repo("test", "repo", remote_url)
        repo_path = repo_manager.get_repo_path("test", "repo")

        # Create a marker file to verify it's the same directory
        marker = repo_path / "marker.txt"
        marker.write_text("marker")

        # Ensure repo (should return existing, not re-clone)
        result = repo_manager.ensure_repo("test", "repo", remote_url)

        assert result is not None
        assert marker.exists()  # Directory wasn't replaced


@pytest.mark.integration
class TestRepoManagerDefaultBranch:
    """Tests for default branch detection."""

    def test_detects_main_as_default(self, real_managers, local_git_repo):
        """Test that main is detected as default branch."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        result = repo_manager.clone_repo("test", "repo", remote_url)

        assert result.default_branch == "main"

    def test_detects_default_from_bare_repo(self, real_managers, local_git_repo):
        """Test default branch detection works for bare repos."""
        repo_manager = real_managers["repo_manager"]
        remote_url = local_git_repo["remote_url"]

        # Clone
        repo_manager.clone_repo("test", "repo", remote_url)
        bare_path = repo_manager.get_bare_path("test", "repo")

        # Verify HEAD points to main
        result = subprocess.run(
            ["git", "symbolic-ref", "HEAD"],
            cwd=bare_path,
            capture_output=True,
            text=True,
            check=True,
        )
        assert "main" in result.stdout


def _commit_on(work_dir, branch, filename, message, push=True):
    """Add one commit to *branch* in the working copy and push it.

    Returns the pushed commit sha, which is what a workspace's HEAD is compared
    against — the sha is the contract, not the file content.
    """
    subprocess.run(
        ["git", "checkout", "-B", branch],
        cwd=work_dir,
        check=True,
        capture_output=True,
    )
    (work_dir / filename).write_text(f"{message}\n")
    subprocess.run(["git", "add", filename], cwd=work_dir, check=True, capture_output=True)
    subprocess.run(["git", "commit", "-m", message], cwd=work_dir, check=True, capture_output=True)
    if push:
        subprocess.run(
            ["git", "push", "origin", branch],
            cwd=work_dir,
            check=True,
            capture_output=True,
        )
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=work_dir,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def _head_sha(path):
    """The commit a clone is actually sitting on."""
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=path,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


@pytest.fixture
def clone_manager(real_managers):
    """A real WorkspaceCloneManager over the isolated cache and real git."""
    return WorkspaceCloneManager(
        config=real_managers["config"],
        repo_manager=real_managers["repo_manager"],
        storage=real_managers["storage"],
    )


@pytest.mark.integration
class TestStalenessContract:
    """What you get when you push upstream and immediately launch the branch.

    The user-facing half of devlaunch#144, over real git against a local
    file-path "remote". These are the tests that would notice if the targeted
    fetch were dropped or made conditional: the unit pins say which git commands
    run, and these say the commands add up to the promise.
    """

    def test_a_commit_pushed_after_the_cache_was_built_is_what_you_launch(
        self, clone_manager, local_git_repo
    ):
        """Push, then immediately dl the branch → the workspace is on the pushed tip.

        The headline promise. The cache is deliberately built *first* and its
        interval left unelapsed, which is exactly the state in which the old
        lazy-fetch path would have skipped the network and handed back a workspace
        one commit behind the remote.
        """
        remote_url = local_git_repo["remote_url"]
        work_dir = local_git_repo["work_dir"]

        # A cache built before the push exists — the stale-cache starting state.
        clone_manager.repo_manager.ensure_repo("test", "repo", remote_url)

        pushed = _commit_on(work_dir, "main", "after_cache.txt", "Pushed after the cache")

        ws_path = clone_manager.prepare_cold("test", "repo", "main", remote_url)

        assert _head_sha(ws_path) == pushed

    def test_a_branch_that_reached_the_remote_after_the_cache_still_launches(
        self, clone_manager, local_git_repo
    ):
        """A branch the cache has never heard of launches, at its own tip.

        Distinct from the previous case: there the ref existed in the cache and
        moved, here it is absent from the cache entirely, which is the path that
        used to depend on the broad sweep having happened to run.
        """
        remote_url = local_git_repo["remote_url"]
        work_dir = local_git_repo["work_dir"]

        clone_manager.repo_manager.ensure_repo("test", "repo", remote_url)

        pushed = _commit_on(work_dir, "feature/late", "late.txt", "Pushed later")

        ws_path = clone_manager.prepare_cold("test", "repo", "feature/late", remote_url)

        assert _head_sha(ws_path) == pushed

    def test_a_brand_new_branch_starts_from_the_current_default_branch(
        self, clone_manager, local_git_repo
    ):
        """A branch nobody has pushed is cut from main's *fresh* tip.

        The RefMissingOnRemote arm end to end. main moves after the cache is
        built, and the new branch must still start from the moved tip — otherwise
        every branch created on a cold cache silently starts from history.
        """
        remote_url = local_git_repo["remote_url"]
        work_dir = local_git_repo["work_dir"]

        clone_manager.repo_manager.ensure_repo("test", "repo", remote_url)

        main_tip = _commit_on(work_dir, "main", "moved.txt", "main moved on")

        ws_path = clone_manager.prepare_cold("test", "repo", "brand/new", remote_url)

        assert _head_sha(ws_path) == main_tip
        assert (
            subprocess.run(
                ["git", "rev-parse", "--abbrev-ref", "HEAD"],
                cwd=ws_path,
                capture_output=True,
                text=True,
                check=True,
            ).stdout.strip()
            == "brand/new"
        )

    def test_a_ref_that_exists_nowhere_at_all_still_fails(self, clone_manager, tmp_path):
        """With nothing to launch from, the launch fails rather than inventing one.

        An empty remote: neither the requested branch nor a default branch to base
        it on. Pinned because the three-way outcome deliberately keeps
        "the remote says no" separate from "the remote did not answer", and neither
        of them is licence to hand back a workspace built on nothing.
        """
        empty_remote = tmp_path / "empty_remote.git"
        subprocess.run(
            ["git", "init", "--bare", "--initial-branch=main", str(empty_remote)],
            check=True,
            capture_output=True,
        )

        clone_manager.repo_manager.ensure_repo("test", "empty", str(empty_remote))

        # The branch step runs before the workspace step inside the one locked
        # scope, so its branch creation is where the empty cache is discovered --
        # and the whole preparation fails there rather than handing back a
        # workspace built on nothing.
        with pytest.raises(RuntimeError):
            clone_manager.prepare_cold("test", "empty", "nosuch", str(empty_remote))

    def test_an_unreachable_remote_launches_from_the_cache(self, clone_manager, local_git_repo):
        """Offline, a branch already in the cache still launches.

        The FetchFailed arm's whole point: the fetch is best-effort, so losing the
        network costs you freshness and not the workspace. The remote is made
        unreachable by moving it, which is indistinguishable to git from a host
        that is not answering.
        """
        remote_url = local_git_repo["remote_url"]
        cached_tip = _head_sha(local_git_repo["work_dir"])

        clone_manager.repo_manager.ensure_repo("test", "repo", remote_url)
        Path(remote_url).rename(Path(remote_url).with_name("moved_away.git"))

        ws_path = clone_manager.prepare_cold("test", "repo", "main", remote_url)

        assert _head_sha(ws_path) == cached_tip
