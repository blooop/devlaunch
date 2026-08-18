"""Tests for WorkspaceCloneManager."""
# pylint: disable=redefined-outer-name,protected-access,unused-argument

import contextlib
from pathlib import Path
from unittest.mock import call, patch, MagicMock

import logging
import os
import subprocess

import pytest

from devlaunch.workspace_id import WorkspaceId
from devlaunch.worktree.repo_manager import (
    FetchFailed,
    RefMissingOnRemote,
    RepositoryManager,
    Updated,
)
from devlaunch.worktree.workspace_clone import FreshBase, StaleBase, WorkspaceCloneManager
from devlaunch.worktree.config import WorktreeConfig


def leaf(branch="nb4", owner="owner", repo="repo"):
    """The clone-directory leaf name for a triple.

    Derived rather than hardcoded on purpose: the leaf and the devpod workspace id
    are the same string by construction, and what that string *is* for a given
    triple is pinned in test_workspace_id.py. Restating it here would only pin the
    same fact twice, and would make these tests fail for the wrong reason.
    """
    return WorkspaceId(owner, repo, branch).value


def stub_git(tracked=(), lfs_files=(), index_readable=True):
    """Stand in for git in the tests that mock the subprocess boundary wholesale.

    Only the two listings the LFS path reads are modelled, each in the shape git
    really returns it: `git ls-files -z --with-tree=HEAD` answers with the union
    of HEAD and the index as NUL-separated bytes, `git lfs ls-files` with the
    same union as newline-separated text. Every other command succeeds silently.
    `tracked` is that union, which is why one list feeds both.
    """

    def run(cmd, *_args, **_kwargs):
        if cmd[:3] == ["git", "lfs", "ls-files"]:
            return MagicMock(returncode=0, stdout="".join(f"{n}\n" for n in lfs_files), stderr="")
        if cmd[:2] == ["git", "ls-files"]:
            if not index_readable:
                return MagicMock(returncode=128, stdout=b"", stderr=b"fatal: broken index")
            return MagicMock(
                returncode=0, stdout=b"".join(f"{n}\0".encode() for n in tracked), stderr=b""
            )
        return MagicMock(returncode=0, stdout="", stderr="")

    return run


@pytest.fixture
def tmp_repos_dir(tmp_path):
    """Create temporary repos directory."""
    repos_dir = tmp_path / "repos"
    repos_dir.mkdir()
    return repos_dir


@pytest.fixture
def config(tmp_repos_dir):
    """Create a WorktreeConfig with temporary directory."""
    return WorktreeConfig(repos_dir=tmp_repos_dir)


@pytest.fixture
def repo_lock(tmp_path):
    """A real RepoLock for owner/repo, minted the only way one can be.

    Real rather than a stub, because ``require`` is a live check inside every
    method these tests call: a mock token would answer it silently, and the
    property that a lock on one repository cannot vouch for another would go
    unexercised in the whole file.

    Minted over a directory of its own rather than over `tmp_repos_dir`, which
    tests here are entitled to create and assert the contents of. What the token
    carries is the pair, not a path, so where the flock behind it sits makes no
    difference to anything under test.
    """
    manager = RepositoryManager(tmp_path / "lock-scope", MagicMock())
    with manager.hold_repo_lock("owner", "repo") as lock:
        yield lock


@pytest.fixture
def mock_repo_manager(tmp_repos_dir, repo_lock):
    """Create a mock RepositoryManager."""
    mgr = MagicMock()
    repo_root = tmp_repos_dir / "owner" / "repo"
    mgr.get_repo_path.return_value = repo_root
    mgr.get_bare_path.return_value = repo_root / ".bare"
    # A real path, not a MagicMock: the lock scope flocks this.
    mgr.lock_path.return_value = repo_root / ".lock"
    # The lock scope hands out the real token the fixture already holds, rather
    # than a mock context manager yielding a mock: what prepare_cold passes down
    # is then the same evidence the production path passes down.
    mgr.hold_repo_lock = lambda owner, repo: contextlib.nullcontext(repo_lock)
    mgr.clone_if_missing.return_value = MagicMock()
    # A real outcome arm, not a MagicMock: ensure_branch dispatches on the type
    # and rejects anything that is not one of the three, so a bare mock here
    # would fail every test with "unhandled fetch outcome" rather than with
    # whatever the test is actually about.
    mgr.fetch_ref.return_value = Updated()
    return mgr


@pytest.fixture
def mock_storage():
    """Create a mock MetadataStorage."""
    return MagicMock()


@pytest.fixture
def mock_branch_manager():
    """Create a mock BranchManager."""
    return MagicMock()


@pytest.fixture
def clone_manager(config, mock_repo_manager, mock_storage, mock_branch_manager):
    """Create a WorkspaceCloneManager with mocked dependencies."""
    return WorkspaceCloneManager(
        config=config,
        repo_manager=mock_repo_manager,
        storage=mock_storage,
        branch_manager=mock_branch_manager,
    )


class TestWorkspaceCloneManagerInit:
    """Tests for WorkspaceCloneManager initialization."""

    def test_uses_config_repos_dir(self, config, mock_repo_manager, mock_storage):
        mgr = WorkspaceCloneManager(
            config=config, repo_manager=mock_repo_manager, storage=mock_storage
        )
        assert mgr.config.repos_dir == config.repos_dir


class TestGetWorkspacePath:
    """Tests for get_workspace_path."""

    def test_returns_correct_path(self, clone_manager, tmp_repos_dir):
        """Test workspace path is a sibling of .bare."""
        path = clone_manager.get_workspace_path("owner", "repo", "nb4")
        assert path == tmp_repos_dir / "owner" / "repo" / leaf("nb4")

    def test_leaf_is_the_workspace_id(self, clone_manager):
        """The clone directory and the devpod workspace share one name.

        A bare branch name was unique only within its parent directory, so any
        consumer keying on a single path component saw every branch of a repo as
        one workspace (kinisi-robotics/kinisi_ros#9766).
        """
        path = clone_manager.get_workspace_path("owner", "repo", "feature/my-branch")
        assert path.name == WorkspaceId("owner", "repo", "feature/my-branch").value

    def test_leaf_is_self_identifying_across_repos(self, clone_manager, mock_repo_manager):
        """Same branch, different repo: the leaf names must differ, not just the parent."""
        mock_repo_manager.get_repo_path.side_effect = lambda o, r: Path("/cache") / o / r
        first = clone_manager.get_workspace_path("owner", "repo-one", "main")
        second = clone_manager.get_workspace_path("owner", "repo-two", "main")
        assert first.name != second.name

    def test_rejects_unvalidated_ref(self, clone_manager):
        """The path that used to be the unguarded one of three.

        `_validate_ref` returned a naked str, so nothing forced this call site to
        use it; now the leaf name cannot be produced without constructing a
        WorkspaceId, and that constructor validates.
        """
        with pytest.raises(ValueError, match="Invalid git ref"):
            clone_manager.get_workspace_path("owner", "repo", "--evil")

    def test_rejects_unvalidated_ref_via_workspace_exists(self, clone_manager):
        with pytest.raises(ValueError, match="Invalid git ref"):
            clone_manager.workspace_exists("owner", "repo", "branch name")

    def test_rejects_unvalidated_ref_via_prepare_cold(self, clone_manager):
        with pytest.raises(ValueError, match="Invalid git ref"):
            clone_manager.prepare_cold("owner", "repo", "--evil", "git@github.com:owner/repo.git")

    def test_rejects_unvalidated_ref_via_remove_workspace(self, clone_manager):
        with pytest.raises(ValueError, match="Invalid git ref"):
            clone_manager.remove_workspace("owner", "repo", "--evil")


class TestWorkspaceExists:
    """Tests for workspace_exists."""

    def test_returns_false_when_no_dir(self, clone_manager):
        """Test returns False when workspace directory doesn't exist."""
        assert clone_manager.workspace_exists("owner", "repo", "main") is False

    def test_returns_false_when_no_git(self, clone_manager, tmp_repos_dir):
        """Test returns False when directory exists but has no .git."""
        ws_dir = tmp_repos_dir / "owner" / "repo" / leaf("main")
        ws_dir.mkdir(parents=True)
        assert clone_manager.workspace_exists("owner", "repo", "main") is False

    def test_returns_true_when_valid(self, clone_manager, tmp_repos_dir):
        """Test returns True when directory has .git."""
        ws_dir = tmp_repos_dir / "owner" / "repo" / leaf("main")
        ws_dir.mkdir(parents=True)
        (ws_dir / ".git").mkdir()
        assert clone_manager.workspace_exists("owner", "repo", "main") is True


class TestEnsureBranch:
    """Tests for ensure_branch."""

    def test_fetches_the_requested_ref_then_ensures(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """The requested branch is fetched by name, then the branch is ensured.

        By name and unconditionally: this one call is what makes "push upstream,
        immediately dl the branch" land on the pushed tip, and it replaced an
        interval-gated fetch of every ref in the repository.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.return_value = Updated()

        clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        mock_repo_manager.fetch_ref.assert_called_once_with("owner", "repo", "newbranch")
        mock_repo_manager.lazy_fetch.assert_not_called()
        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="main",
            use_local_refs=True,
        )

    def test_ref_absent_upstream_fetches_the_default_branch_to_base_on(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """A brand-new branch is based on a *freshly fetched* default branch.

        Without the second targeted fetch the new branch would be cut from
        whatever the cache happened to hold, so a branch created today could start
        from last week's main — the staleness the whole change exists to remove.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.side_effect = [RefMissingOnRemote(), Updated()]

        clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert mock_repo_manager.fetch_ref.call_args_list == [
            call("owner", "repo", "newbranch"),
            call("owner", "repo", "main"),
        ]
        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="main",
            use_local_refs=True,
        )

    def test_absent_ref_costs_exactly_one_extra_fetch(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """The default-branch fetch is not retried when it too finds nothing.

        Pins the bound the contract states — at most two narrow calls on the cold
        path — so a later "just fetch the fallback's fallback" cannot turn one
        launch into a chain of round-trips.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.return_value = RefMissingOnRemote()

        clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert mock_repo_manager.fetch_ref.call_count == 2

    def test_a_failed_default_branch_fetch_warns_with_its_reason(
        self,
        clone_manager,
        mock_repo_manager,
        mock_branch_manager,
        tmp_repos_dir,
        repo_lock,
        caplog,
    ):
        """Losing the base-branch fetch is reported the way losing any fetch is.

        The reason is carried so it can be printed; a FetchFailed on the
        default-branch fetch that vanished silently would cut the new branch
        from a stale cache with nothing telling the user why it is old.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.side_effect = [
            RefMissingOnRemote(),
            FetchFailed("no such host"),
        ]

        with caplog.at_level(logging.WARNING):
            clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert "no such host" in caplog.text
        # Still proceeds: the cache's own default branch is the best remaining
        # start point, exactly as when the first fetch fails.
        mock_branch_manager.ensure_branch_exists.assert_called_once()

    def test_an_unrecognised_base_fetch_outcome_is_rejected_not_absorbed(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """A fourth fetch outcome must be rejected on the base fetch too.

        The sum type exists so that every arm is named and a new one cannot be
        read as an old one — the guarantee the requested-ref dispatch already
        makes. The base-branch fetch answers the same three, so an outcome
        outside them is a bug in the same way: absorbed silently it would cut a
        brand-new branch from the cache while behaving exactly like a clean
        fetch, with nothing distinguishing the two.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.side_effect = [RefMissingOnRemote(), object()]

        with pytest.raises(AssertionError, match="Unhandled fetch outcome"):
            clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

    def test_an_unrecognised_requested_ref_outcome_is_rejected_not_absorbed(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """The same rejection on the requested ref's own fetch.

        The guarantee the arm above is held to; pinned here so the pair cannot
        drift back apart, and so the rejection is a tested promise rather than
        an unexercised branch.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.return_value = object()

        with pytest.raises(AssertionError, match="Unhandled fetch outcome"):
            clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

    def test_an_unsafe_recorded_default_branch_does_not_escape_as_a_valueerror(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """A corrupt default_branch in metadata must not change what ensure_branch raises.

        The requested branch reaches here proven by a WorkspaceId, but the default
        branch is read back from metadata.json and is unproven — so fetch_ref
        rejects it, and the rejection is a ValueError. dl's launch path guards
        ensure_branch with (RuntimeError, OSError), so letting that out turns a
        hand-edited record into a traceback instead of an error message.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "--upload-pack=evil"

        def reject_unsafe(_owner, _repo, ref):
            if ref.startswith("-"):
                raise ValueError(f"Invalid git ref: {ref}")
            return RefMissingOnRemote()

        mock_repo_manager.fetch_ref.side_effect = reject_unsafe

        clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        # Still ensures the branch, leaving the failure to git as it did before.
        mock_branch_manager.ensure_branch_exists.assert_called_once()

    def test_continues_from_cache_when_the_fetch_fails(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """Offline still launches, from whatever the cache holds.

        Proceeding is the point: an unreachable remote must not stop a launch of a
        branch already in the cache. The error for "there is nothing to launch
        from" comes from the branch creation below, the first thing that actually
        consults the cache.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.return_value = FetchFailed("no such host")

        clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        # No default-branch fetch: nothing was learned about the remote, so there
        # is no basis for treating the branch as new.
        mock_repo_manager.fetch_ref.assert_called_once_with("owner", "repo", "newbranch")
        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="main",
            use_local_refs=True,
        )

    def test_falls_back_to_head_if_get_default_branch_fails(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """Test that ensure_branch falls back to HEAD when get_default_branch raises."""
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.side_effect = RuntimeError("no HEAD")

        clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="HEAD",
            use_local_refs=True,
        )

    def test_falls_back_to_head_if_get_default_branch_returns_empty(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """Test that ensure_branch falls back to HEAD when get_default_branch returns empty."""
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = ""

        clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="HEAD",
            use_local_refs=True,
        )

    def test_an_empty_default_branch_name_reports_head_as_the_stale_base(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """An empty recorded name is "no default branch", not a branch called "".

        The second way arm 2 happens: the resolver answers rather than raising,
        but answers with nothing. It reaches the same place a raise does — no
        fetch, and a branch cut from the bare cache's own HEAD — so it owes the
        caller the same report, with a reason that says which of the two it was.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = ""
        mock_repo_manager.fetch_ref.return_value = RefMissingOnRemote()

        base = clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert isinstance(base, StaleBase)
        assert base.base == "HEAD"
        assert base.reason == "no default branch is recorded"
        # No second fetch: there was no name to fetch.
        mock_repo_manager.fetch_ref.assert_called_once_with("owner", "repo", "newbranch")

    def test_a_fresh_requested_ref_reports_a_fresh_base(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """A launch whose tip was fetched this call says so in its return value.

        The report is the channel devlaunch#245 adds: the caller could never
        tell a fresh base from a stale one, because both returned None and the
        difference lived only in a log line.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.return_value = Updated()

        base = clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert isinstance(base, FreshBase)

    def test_a_new_branch_cut_from_a_fresh_default_reports_a_fresh_base(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """The happy new-branch arm is fresh: its base was fetched this call."""
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.side_effect = [RefMissingOnRemote(), Updated()]

        base = clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert isinstance(base, FreshBase)

    def test_a_lost_base_fetch_reports_the_stale_base_and_its_reason(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """Arm 1 of devlaunch#245: the base-branch fetch fails after the remote
        called the branch new. The branch is still cut from the cache's default
        branch — and the return value now says which ref that was and why
        nothing refreshed it, instead of leaving both facts in a warning.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.side_effect = [
            RefMissingOnRemote(),
            FetchFailed("no such host"),
        ]

        base = clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert isinstance(base, StaleBase)
        assert base.base == "main"
        assert "no such host" in base.reason
        # The branch is still cut, from the cache's own default branch.
        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="main",
            use_local_refs=True,
        )

    def test_an_unresolvable_default_branch_reports_head_as_the_stale_base(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """Arm 2 of devlaunch#245: with no default branch to name, nothing is
        fetched and the branch is cut from the cache's own HEAD — a ref of
        unbounded age. The report names HEAD as the base so the caller knows
        exactly how weak the guarantee is.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.side_effect = RuntimeError("no HEAD")
        mock_repo_manager.fetch_ref.return_value = RefMissingOnRemote()

        base = clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert isinstance(base, StaleBase)
        assert base.base == "HEAD"
        assert "no HEAD" in base.reason
        mock_branch_manager.ensure_branch_exists.assert_called_once_with(
            bare_path,
            "newbranch",
            create_remote=False,
            start_point="HEAD",
            use_local_refs=True,
        )

    def test_an_unsafe_recorded_default_branch_reports_the_stale_base(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """Arm 3 of devlaunch#245: a recorded default-branch name the fetch
        refuses means nothing was refreshed, yet that same unproven name is
        what the branch is cut from. The report carries the name and the
        rejection, where before both died as a warning.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "--upload-pack=evil"

        def reject_unsafe(_owner, _repo, ref):
            if ref.startswith("-"):
                raise ValueError(f"Invalid git ref: {ref}")
            return RefMissingOnRemote()

        mock_repo_manager.fetch_ref.side_effect = reject_unsafe

        base = clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert isinstance(base, StaleBase)
        assert base.base == "--upload-pack=evil"
        assert "Invalid git ref" in base.reason

    def test_a_base_the_remote_no_longer_has_reports_the_stale_base(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """The remote answering "no such base branch" leaves the cache's copy
        unverifiable, which is stale in the only sense that matters here:
        nothing this call fetched backs the ref the branch is cut from.
        Pinned so the sum stays total — no arm may answer with silence.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.side_effect = [RefMissingOnRemote(), RefMissingOnRemote()]

        base = clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert isinstance(base, StaleBase)
        assert base.base == "main"

    def test_an_unreachable_remote_reports_the_launched_ref_as_the_stale_base(
        self, clone_manager, mock_repo_manager, mock_branch_manager, tmp_repos_dir, repo_lock
    ):
        """Offline, the ref you launch is itself the one nothing refreshed.

        The launch still proceeds from the cache — that contract is pinned
        elsewhere and untouched — but the report now says the tip is
        unrefreshed rather than claiming nothing.
        """
        bare_path = tmp_repos_dir / "owner" / "repo" / ".bare"
        mock_repo_manager.get_bare_path.return_value = bare_path
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.return_value = FetchFailed("no such host")

        base = clone_manager.ensure_branch(repo_lock, "owner", "repo", "newbranch")

        assert isinstance(base, StaleBase)
        assert base.base == "newbranch"
        assert "no such host" in base.reason


class TestPrepareCold:
    """Tests for prepare_cold, the cold path's one locked entrypoint."""

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_a_base_answer_this_launch_does_not_name_is_refused(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """The exhaustiveness check at the surface that reports staleness.

        A third base arm added later must arrive here as a crash, not as
        silence: the bare ``isinstance(base, StaleBase)`` this replaced read
        anything it had no case for as a fresh base, which is precisely the
        "launched from a stale cache without saying so" this whole change
        exists to make impossible.
        """
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        with patch.object(clone_manager, "ensure_branch", return_value="fresh, honest"):
            with pytest.raises(AssertionError, match="Unhandled branch base"):
                clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_a_stale_base_launch_says_so_in_dls_own_output(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir, caplog
    ):
        """A launch cut from an unrefreshed base names the base, the reason and
        the consequence — the line wf reads (devlaunch#245).

        The report crosses ensure_branch's boundary as a value; this is where
        it becomes output. The path prepare_cold returns is unchanged, because
        the workspace is still handed to devpod exactly as before.
        """
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.side_effect = [
            RefMissingOnRemote(),
            FetchFailed("no such host"),
        ]

        with caplog.at_level(logging.WARNING):
            ws_path = clone_manager.prepare_cold(
                "owner", "repo", "nb4", "git@github.com:owner/repo.git"
            )

        assert ws_path == repo_root / leaf()
        stale_lines = [r.message for r in caplog.records if "may be behind" in r.message]
        assert len(stale_lines) == 1
        assert "owner/repo@nb4" in stale_lines[0]
        assert "'main'" in stale_lines[0]
        assert "no such host" in stale_lines[0]

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_a_fresh_base_launch_claims_nothing_about_staleness(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir, caplog
    ):
        """The warning exists only when it is true, or it trains readers to
        skip it and wf to distrust it."""
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"
        mock_repo_manager.get_default_branch.return_value = "main"
        mock_repo_manager.fetch_ref.return_value = Updated()

        with caplog.at_level(logging.WARNING):
            clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        assert "may be behind" not in caplog.text

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_clones_from_bare_repo(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, mock_storage, tmp_repos_dir
    ):
        """Test that a new workspace is cloned from the bare repo.

        Newly-created workspaces skip ``git fetch origin`` because they were
        just cloned from a freshly-fetched bare repo. git-lfs is absent here,
        so no LFS materialization calls are made.
        """
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        ws_path = repo_root / leaf()

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        # Should have called: git clone, git remote set-url,
        # git show-ref (remote ref check), git checkout -B (no fetch)
        assert mock_run.call_count == 4

        # First call: git clone from bare repo, with LFS smudge disabled
        # (the bare cache has no LFS objects to smudge from)
        clone_call = mock_run.call_args_list[0]
        assert clone_call[0][0] == ["git", "clone", str(bare_path), str(ws_path)]
        assert clone_call[1]["env"]["GIT_LFS_SKIP_SMUDGE"] == "1"

        # Second call: fix remote URL
        remote_call = mock_run.call_args_list[1]
        assert remote_call[0][0] == [
            "git",
            "remote",
            "set-url",
            "origin",
            "git@github.com:owner/repo.git",
        ]

        # Third call: show-ref to check remote branch (returns 0 = exists)
        showref_call = mock_run.call_args_list[2]
        assert showref_call[0][0] == [
            "git",
            "show-ref",
            "--verify",
            "refs/remotes/origin/nb4",
        ]

        # Fourth call: checkout -B from remote ref (no fetch for new workspaces)
        checkout_call = mock_run.call_args_list[3]
        assert checkout_call[0][0] == ["git", "checkout", "-B", "nb4", "origin/nb4"]

        # Should track in metadata
        mock_storage.add_worktree.assert_called_once()

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_re_registering_an_existing_clone_runs_no_fetch(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Re-registration touches the network not at all.

        The clone is on disk but devpod has forgotten the workspace. This path used
        to run an unconditional `git fetch origin` in the clone whose output
        nothing then read: the checkout below is a plain `git checkout <branch>`
        against the local branch and never consults a remote-tracking ref. So it
        was a whole network round-trip per re-registration, bought nothing, and is
        gone.
        """
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        # A workspace clone already on disk — workspace_exists looks for .git
        ws_path = repo_root / leaf()
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        assert [c[0][0] for c in mock_run.call_args_list] == [
            ["git", "checkout", "nb4"],
        ]

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_existing_workspace_retries_unmaterialized_lfs(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """A workspace left holding pointer files must retry on the next run.

        Materialization used to be gated on "did we just clone this", so one
        failed `git lfs pull` left the workspace existing-but-incomplete and every
        later run took the existing-workspace path — building against pointers
        forever, silently.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        # An existing workspace whose LFS file is still a pointer.
        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        big = ws_path / "big.bin"
        big.write_bytes(b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n")

        mock_run.side_effect = stub_git(tracked=["big.bin"], lfs_files=["big.bin"])

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        issued = [c[0][0] for c in mock_run.call_args_list]
        assert ["git", "lfs", "pull", "origin"] in issued

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_materialized_workspace_does_not_refetch_lfs(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Real content present means no pull, so attaching stays fast."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        (ws_path / "big.bin").write_bytes(b"\x00\x01real binary content")

        mock_run.side_effect = stub_git(tracked=["big.bin"], lfs_files=["big.bin"])

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        issued = [c[0][0] for c in mock_run.call_args_list]
        assert ["git", "lfs", "pull", "origin"] not in issued

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_new_workspace_materializes_lfs(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """New workspaces with LFS-tracked files run `git lfs pull` after checkout.

        The clone comes from the local bare cache (no LFS objects), so content
        must be pulled from the real origin once the remote URL is fixed.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        # git clone is mocked, so stand in for what it would have left behind:
        # a tree whose LFS file is still a pointer (cloned with skip-smudge).
        ws_root = repo_root / leaf()
        pointer = ws_root / "assets" / "big.bin"
        pointer.parent.mkdir(parents=True)
        pointer.write_bytes(b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n")

        mock_run.side_effect = stub_git(tracked=["assets/big.bin"], lfs_files=["assets/big.bin"])

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        lfs_calls = [c[0][0] for c in mock_run.call_args_list if c[0][0][:2] == ["git", "lfs"]]
        assert ["git", "lfs", "ls-files", "--name-only"] in lfs_calls
        assert ["git", "lfs", "pull", "origin"] in lfs_calls

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_the_cache_store_is_filled_in_the_bare_for_the_launched_ref(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """The bare-side fetch runs in the bare, names the ref, and is bounded.

        Three things this pins, each of which is silently losable:

        - **cwd is the bare.** That, and nothing else, is what puts the objects
          in `<bare>/lfs` and makes the cache the repo's store. Run in the
          workspace, the same command fills the workspace's own store and shares
          nothing — every assertion about disk would still pass, for one
          workspace.
        - **The ref is named.** A bare `git lfs fetch origin` fetches the default
          ref set, not the branch being launched.
        - **Both `fetchrecent` knobs are zero.** Left at their defaults, git-lfs
          also walks recent refs and recent commits, so launching one branch of a
          busy repo downloads several branches' payloads — the per-launch cost
          this whole path exists to remove, reintroduced at the cache.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        (ws_path / "big.bin").write_bytes(
            b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n"
        )

        mock_run.side_effect = stub_git(tracked=["big.bin"], lfs_files=["big.bin"])

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        fetches = [c for c in mock_run.call_args_list if "fetch" in c[0][0]]
        assert len(fetches) == 1
        assert fetches[0][0][0] == [
            "git",
            "-c",
            "lfs.fetchrecentrefsdays=0",
            "-c",
            "lfs.fetchrecentcommitsdays=0",
            "lfs",
            "fetch",
            "origin",
            "nb4",
        ]
        assert fetches[0][1]["cwd"] == bare_path

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_the_workspace_pulls_from_the_cache_before_it_pulls_from_origin(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """The cache is asked first, by `file://` URL, in the workspace.

        Order is the property, not merely presence: a cache pull issued *after*
        the origin pull would leave every launch paying the download it was
        added to avoid, and every disk assertion would still hold. The remote is
        a bare `file://` argument rather than a configured one because the clone
        is bind-mounted into the devcontainer and the bare is not — a host path
        written into the clone's config breaks every in-container checkout.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        (ws_path / "big.bin").write_bytes(
            b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n"
        )

        mock_run.side_effect = stub_git(tracked=["big.bin"], lfs_files=["big.bin"])

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        pulls = [c[0][0] for c in mock_run.call_args_list if c[0][0][:3] == ["git", "lfs", "pull"]]
        assert pulls == [
            ["git", "lfs", "pull", f"file://{bare_path}"],
            ["git", "lfs", "pull", "origin"],
        ]
        cache_pull = next(
            c for c in mock_run.call_args_list if c[0][0][:3] == ["git", "lfs", "pull"]
        )
        assert cache_pull[1]["cwd"] == ws_path

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_origin_is_not_pulled_when_the_cache_materialized_everything(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """The network phase is entered on surviving pointers, not on exit codes.

        This is the saving, stated as a command that does not run: the cache
        supplied the content, so the forge is never contacted. Deciding it from
        the cache pull's exit status instead would be wrong in both directions —
        git-lfs exits zero having fetched only some objects, and a partial
        failure must still fall through — so the same content predicate that
        opened materialization is what closes it, and this test is the one that
        holds that. The stub materializes the pointer on the cache pull, which
        is exactly what the real command does.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        pointer = ws_path / "big.bin"
        pointer.write_bytes(b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n")

        base = stub_git(tracked=["big.bin"], lfs_files=["big.bin"])

        def run(cmd, *args, **kwargs):
            if cmd[:3] == ["git", "lfs", "pull"] and cmd[3].startswith("file://"):
                pointer.write_bytes(b"real content, no longer a pointer")
            return base(cmd, *args, **kwargs)

        mock_run.side_effect = run

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        issued = [c[0][0] for c in mock_run.call_args_list]
        assert ["git", "lfs", "pull", f"file://{bare_path}"] in issued
        assert ["git", "lfs", "pull", "origin"] not in issued

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_a_failing_cache_phase_degrades_to_origin_rather_than_failing_the_launch(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Neither cache step may take a launch down with it.

        Both of them exit non-zero in ordinary conditions — offline, or an object
        this repo's cache has never held — and both are speculative: the network
        pull behind them is the thing that was always there. Letting either
        failure out would turn a working offline-ish launch of an LFS repo into a
        traceback, which is a *worse* outcome than the state before the cache
        existed.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        (ws_path / "big.bin").write_bytes(
            b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n"
        )

        base = stub_git(tracked=["big.bin"], lfs_files=["big.bin"])

        def run(cmd, *args, **kwargs):
            if "lfs" in cmd and cmd[-1] != "origin" and "ls-files" not in cmd:
                raise subprocess.CalledProcessError(2, cmd)
            return base(cmd, *args, **kwargs)

        mock_run.side_effect = run

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        issued = [c[0][0] for c in mock_run.call_args_list]
        assert ["git", "lfs", "pull", "origin"] in issued

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_a_launch_that_cannot_materialize_at_all_still_says_so(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """When the cache cannot help and neither can origin, the launch fails loudly.

        The other side of the degradation above, and the contract the cache phase
        was not allowed to weaken: a workspace whose pointers nothing could
        resolve must not be handed over as though it were complete, because a
        build against stub files fails much further from the cause. The message
        names the retry, which is real — the gate is pointer content, so the next
        launch tries again.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        (ws_path / "big.bin").write_bytes(
            b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n"
        )

        base = stub_git(tracked=["big.bin"], lfs_files=["big.bin"])

        def run(cmd, *args, **kwargs):
            if "lfs" in cmd and "ls-files" not in cmd:
                raise subprocess.CalledProcessError(2, cmd)
            return base(cmd, *args, **kwargs)

        mock_run.side_effect = run

        with pytest.raises(RuntimeError, match="re-run to retry"):
            clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_workspace_without_pointer_files_never_forks_git_lfs(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """A workspace holding no pointer files must not pay a git-lfs fork.

        The overwhelmingly common repo has no LFS content at all, and probing it
        with `git lfs ls-files` costs a fork on every single launch for an answer
        already sitting in the working tree. Since the cache phase landed there
        are three commands behind this gate rather than one, and the cache fetch
        reaches git as `git -c ... -c ... lfs fetch` — so the assertion looks for
        the word anywhere in the argv rather than at a fixed position, which a
        prefix check would have missed entirely.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        # An existing workspace with ordinary content.
        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        (ws_path / "main.py").write_text("print('hi')\n")

        mock_run.side_effect = stub_git(tracked=["main.py"])

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        issued = [c[0][0] for c in mock_run.call_args_list]
        assert not any("lfs" in cmd for cmd in issued)

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_lfs_path_missing_from_the_working_tree_is_not_pulled_forever(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """An LFS-tracked path that is not on disk is not an unmaterialized pointer.

        A sparse checkout leaves LFS-tracked paths out of the working tree
        altogether, so opening them fails. Reading that failure as "still a
        pointer" would run `git lfs pull origin` — an unbounded, uncaptured
        fetch that can be gigabytes — on every launch of such a workspace,
        forever, because the pull does not put the excluded path on disk and so
        never changes the answer.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        # An existing workspace whose one LFS-tracked path is absent from disk.
        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)

        mock_run.side_effect = stub_git(tracked=["big.bin"], lfs_files=["big.bin"])

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        issued = [c[0][0] for c in mock_run.call_args_list]
        assert ["git", "lfs", "pull", "origin"] not in issued

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs")
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_unlistable_index_fails_open_to_probing(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """If the tracked files can't be listed, the probe runs anyway.

        The cheap check exists to save a fork, not to decide LFS is absent: when
        the listing fails, skipping would silently strand a workspace on pointer
        files — the same degradation the probe itself refuses.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        ws_path = repo_root / leaf()
        (ws_path / ".git").mkdir(parents=True)
        (ws_path / "big.bin").write_bytes(
            b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n"
        )

        mock_run.side_effect = stub_git(lfs_files=["big.bin"], index_readable=False)

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        issued = [c[0][0] for c in mock_run.call_args_list]
        assert ["git", "lfs", "pull", "origin"] in issued

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_new_workspace_new_branch_bases_on_default(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test that a new workspace for a new branch checks out from origin/<default>."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        # show-ref returns non-zero for the requested branch, zero for the default
        def run_side_effect(cmd, *args, **kwargs):
            if cmd[0:3] == ["git", "show-ref", "--verify"]:
                if "new-feature" in cmd[3]:
                    return MagicMock(returncode=1)
                # default branch (main) exists on remote
                return MagicMock(returncode=0)
            return MagicMock(returncode=0)

        mock_run.side_effect = run_side_effect

        base_repo = MagicMock()
        base_repo.default_branch = "main"
        mock_repo_manager.get_repo.return_value = base_repo

        clone_manager.prepare_cold("owner", "repo", "new-feature", "git@github.com:owner/repo.git")

        # Last call should be checkout -B <branch> origin/main
        checkout_call = mock_run.call_args_list[-1]
        assert checkout_call[0][0] == [
            "git",
            "checkout",
            "-B",
            "new-feature",
            "origin/main",
        ]

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_new_workspace_raises_when_no_remote_refs(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test error when neither branch nor default branch exist on remote."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        bare_path = repo_root / ".bare"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = bare_path

        # show-ref always returns non-zero (no remote refs exist)
        def run_side_effect(cmd, *args, **kwargs):
            if cmd[0:3] == ["git", "show-ref", "--verify"]:
                return MagicMock(returncode=1)
            return MagicMock(returncode=0)

        mock_run.side_effect = run_side_effect

        base_repo = MagicMock()
        base_repo.default_branch = "main"
        mock_repo_manager.get_repo.return_value = base_repo

        with pytest.raises(RuntimeError, match="neither 'origin/new-feature' nor 'origin/main'"):
            clone_manager.prepare_cold(
                "owner", "repo", "new-feature", "git@github.com:owner/repo.git"
            )

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_existing_workspace_uses_plain_checkout(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test that existing workspaces use plain checkout to preserve local work."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        # Create existing workspace
        ws_path = repo_root / leaf()
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        mock_run.return_value = MagicMock(returncode=0)

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        # Checkout and nothing else (no clone, no remote set-url, no show-ref,
        # and no fetch)
        assert mock_run.call_count == 1
        checkout_call = mock_run.call_args_list[0]
        assert checkout_call[0][0] == ["git", "checkout", "nb4"]

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_existing_workspace_skips_clone(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test that existing workspace is not re-cloned."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        # Create existing workspace
        ws_path = repo_root / leaf()
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        mock_run.return_value = MagicMock(returncode=0)

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        # Should only call checkout (no clone, no remote set-url, no fetch)
        assert mock_run.call_count == 1
        checkout_call = mock_run.call_args_list[0]
        assert checkout_call[0][0] == ["git", "checkout", "nb4"]

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_ensures_bare_repo_first(self, mock_run, clone_manager, mock_repo_manager, repo_lock):
        """Clone-if-missing runs first, and under the token the scope minted."""
        mock_run.return_value = MagicMock(returncode=0)

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        mock_repo_manager.clone_if_missing.assert_called_once_with(
            repo_lock, "owner", "repo", "git@github.com:owner/repo.git"
        )

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_clone_failure_raises(self, mock_run, clone_manager):
        """Test that clone failure raises RuntimeError."""
        mock_run.side_effect = __import__("subprocess").CalledProcessError(
            1, "git clone", stderr="fatal: error"
        )

        with pytest.raises(RuntimeError, match="Failed to clone workspace"):
            clone_manager.prepare_cold("owner", "repo", "main", "git@github.com:owner/repo.git")

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_workspace_clone_failure_reads_as_a_failure_when_git_said_nothing(
        self, mock_run, clone_manager
    ):
        """A workspace clone that failed silently must still name what happened.

        This clone is local -- bare cache to workspace -- so when it fails it is
        usually for a reason git says nothing about, such as a full disk. That is
        exactly the case that reported "Failed to clone workspace: None".
        """
        mock_run.side_effect = subprocess.CalledProcessError(128, "git clone", stderr=None)

        with pytest.raises(RuntimeError) as excinfo:
            clone_manager.prepare_cold("owner", "repo", "main", "git@github.com:owner/repo.git")

        assert "None" not in str(excinfo.value)
        assert "128" in str(excinfo.value)

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_remote_repoint_failure_reads_as_a_failure_when_git_said_nothing(
        self, mock_run, clone_manager
    ):
        """A remote repoint that failed silently must still name what happened.

        The step between a fresh clone and a usable workspace: until it lands the
        clone still points at the local bare cache, so a failure here has to be
        readable rather than "Failed to set remote URL: None".
        """

        def run(cmd, *_args, **_kwargs):
            if "set-url" in cmd:
                raise subprocess.CalledProcessError(128, cmd, stderr=None)
            return MagicMock(returncode=0, stdout="", stderr="")

        mock_run.side_effect = run

        with pytest.raises(RuntimeError) as excinfo:
            clone_manager.prepare_cold("owner", "repo", "main", "git@github.com:owner/repo.git")

        assert "None" not in str(excinfo.value)
        assert "128" in str(excinfo.value)

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_checkout_failure_reads_as_a_failure_when_git_said_nothing(
        self, mock_run, _mock_which, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """A checkout that failed silently must still name what happened.

        Taken against a workspace that already exists, which is the every-launch
        path: the one git command a warm launch runs, and the one whose failure
        used to read "Failed to checkout branch 'nb4': None".
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        ws_path = repo_root / leaf()
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        mock_run.side_effect = subprocess.CalledProcessError(128, "git checkout", stderr=None)

        with pytest.raises(RuntimeError) as excinfo:
            clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        assert "None" not in str(excinfo.value)
        assert "128" in str(excinfo.value)

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_returns_workspace_path(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Test that prepare_cold returns the workspace path."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"
        mock_run.return_value = MagicMock(returncode=0)

        result = clone_manager.prepare_cold(
            "owner", "repo", "main", "git@github.com:owner/repo.git"
        )

        assert result == repo_root / leaf("main")

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_newly_created_workspace_skips_fetch(
        self, mock_run, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """Newly-created workspaces must not run ``git fetch origin``."""
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        mock_repo_manager.get_bare_path.return_value = repo_root / ".bare"

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        # No call should contain "git fetch origin"
        fetch_calls = [c for c in mock_run.call_args_list if c[0][0] == ["git", "fetch", "origin"]]
        assert fetch_calls == []

    # The companion that asserted an *existing* workspace still fetches is gone
    # with the fetch itself; TestEnsureWorkspace's re-registration test now pins
    # the opposite, which is the contract devlaunch#144 settled.


class TestRemoveWorkspace:
    """Tests for remove_workspace."""

    def test_removes_existing_workspace(
        self, clone_manager, mock_repo_manager, mock_storage, tmp_repos_dir
    ):
        """Test that existing workspace is removed."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        ws_path = repo_root / leaf()
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()
        (ws_path / "file.txt").write_text("content")

        result = clone_manager.remove_workspace("owner", "repo", "nb4")

        assert result is True
        assert not ws_path.exists()
        mock_storage.remove_worktree.assert_called_once_with("owner", "repo", "nb4")

    def test_returns_false_for_nonexistent(self, clone_manager):
        """Test that removing nonexistent workspace returns False."""
        result = clone_manager.remove_workspace("owner", "repo", "nonexistent")
        assert result is False


class TestRemoveWorkspaceById:
    """Tests for remove_workspace_by_id."""

    def test_finds_and_removes(self, clone_manager, mock_storage, mock_repo_manager, tmp_repos_dir):
        """Test that workspace is found by ID and removed."""

        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        ws_path = repo_root / leaf()
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        wt_info = MagicMock()
        wt_info.owner = "owner"
        wt_info.repo = "repo"
        wt_info.branch = "nb4"
        wt_info.local_path = ws_path
        mock_storage.get_worktree_by_workspace_id.return_value = wt_info

        result = clone_manager.remove_workspace_by_id("repo-nb4")

        assert result is True
        mock_storage.get_worktree_by_workspace_id.assert_called_once_with("repo-nb4")

    def test_returns_false_when_not_found(self, clone_manager, mock_storage):
        """Test that returns False when workspace ID is not in metadata."""
        mock_storage.get_worktree_by_workspace_id.return_value = None

        result = clone_manager.remove_workspace_by_id("nonexistent")

        assert result is False

    def test_removes_a_clone_stored_under_the_old_scheme(
        self, clone_manager, mock_storage, mock_repo_manager, tmp_repos_dir
    ):
        """Removal must follow the record, not re-derive the directory name.

        Every workspace created before the new id scheme has a bare-branch-name
        leaf. Re-deriving the leaf here looked for a directory that has never
        existed, so `dl <old-id> rm` deleted the devpod workspace and then returned
        False — orphaning the clone and its metadata entry with no message, since
        the caller only logs on success. WorktreeInfo already stores local_path;
        this is the fix that needs no migration.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        old_path = repo_root / "nb4"  # pre-#64 leaf: the bare branch name
        old_path.mkdir(parents=True)
        (old_path / ".git").mkdir()
        assert old_path.name != leaf("nb4"), "fixture must use the old-scheme name"

        wt_info = MagicMock()
        wt_info.owner = "owner"
        wt_info.repo = "repo"
        wt_info.branch = "nb4"
        wt_info.local_path = old_path
        mock_storage.get_worktree_by_workspace_id.return_value = wt_info

        assert clone_manager.remove_workspace_by_id("repo-nb4") is True
        assert not old_path.exists()
        mock_storage.remove_worktree.assert_called_once_with("owner", "repo", "nb4")

    def test_falls_back_to_derived_path_when_record_has_none(
        self, clone_manager, mock_storage, mock_repo_manager, tmp_repos_dir
    ):
        """A record with no usable local_path still resolves via the derivation."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        ws_path = repo_root / leaf("nb4")
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        wt_info = MagicMock()
        wt_info.owner = "owner"
        wt_info.repo = "repo"
        wt_info.branch = "nb4"
        wt_info.local_path = None
        mock_storage.get_worktree_by_workspace_id.return_value = wt_info

        assert clone_manager.remove_workspace_by_id("repo-nb4") is True
        assert not ws_path.exists()


class TestTheGuardAndTheDeleteNameOneDirectory:
    """devlaunch#174: `dl <ws> rm`'s guard must inspect what the delete removes.

    The guard read `local_path` unconditionally while the delete fell back to the
    derivation whenever that path was not on disk. Every test here is about the
    gap between those two, so each one asserts on
    :meth:`WorkspaceCloneManager.resolve_clone_path` -- the single answer both
    now go through -- rather than on the delete alone. A test that only checked
    the delete would stay green if the guard drifted back.
    """

    def test_a_stale_record_resolves_to_the_directory_that_holds_the_work(
        self, clone_manager, mock_storage, mock_repo_manager, tmp_repos_dir
    ):
        """A recorded path that is no longer on disk must not clear the delete.

        Reproduced before the fix: the guard answered `NothingToLose` about the
        absent recorded directory -- correctly, nothing absent holds anything --
        and the delete then removed the derived one, which held an uncommitted
        file. Exit 0 and no `--force`.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        derived = repo_root / leaf("nb4")
        derived.mkdir(parents=True)
        (derived / ".git").mkdir()

        stale = repo_root / "moved-away"
        assert not stale.exists(), "the fixture's point is that this is gone"

        wt_info = MagicMock()
        wt_info.owner, wt_info.repo, wt_info.branch = "owner", "repo", "nb4"
        wt_info.local_path = stale
        mock_storage.get_worktree_by_workspace_id.return_value = wt_info

        assert clone_manager.resolve_clone_path(wt_info) == derived

    def test_an_empty_recorded_path_is_not_the_working_directory(
        self, clone_manager, mock_storage, mock_repo_manager, tmp_repos_dir
    ):
        """`Path("")` is `Path(".")`: truthy, and `exists()` is True.

        So it passed the old `if wt_info.local_path` test *and* the old
        `.exists()` test, and `shutil.rmtree` was handed dl's own working
        directory -- which it emptied, `.git` included, before failing on
        `os.rmdir(".")`. Absolute is the property that rules it out; truthiness
        does not.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        derived = repo_root / leaf("nb4")
        derived.mkdir(parents=True)

        wt_info = MagicMock()
        wt_info.owner, wt_info.repo, wt_info.branch = "owner", "repo", "nb4"
        wt_info.local_path = Path("")  # what an empty metadata field parses to
        assert wt_info.local_path.exists(), "the hazard is that this is True"

        resolved = clone_manager.resolve_clone_path(wt_info)
        assert resolved == derived
        assert resolved.is_absolute()

    def test_a_recorded_path_dl_cannot_look_at_is_kept_not_derived_away(
        self, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """ "Could not look" is not "not there", and only one of them may derive.

        `Path.exists()` swallows ENOENT/ENOTDIR/EBADF/ELOOP and re-raises the
        rest, so this raised `PermissionError` out of the resolver on 3.10-3.13
        and returned False on 3.14 -- and False is the answer that sends the
        resolver off to name a *different* directory, which is the defect. The
        record is kept instead; `read_clone` then answers `CouldNotTell` about
        it and the delete stops.
        """
        if os.geteuid() == 0:
            pytest.skip("root is refused by nothing, so the closed door would open")

        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        shut = repo_root / "behind-a-closed-door"
        shut.mkdir(parents=True)
        recorded = shut / "clone"
        recorded.mkdir()

        wt_info = MagicMock()
        wt_info.owner, wt_info.repo, wt_info.branch = "owner", "repo", "nb4"
        wt_info.local_path = recorded

        shut.chmod(0o000)
        try:
            resolved = clone_manager.resolve_clone_path(wt_info)
        finally:
            shut.chmod(0o700)

        assert resolved == recorded

    def test_a_record_with_no_path_at_all_still_resolves(
        self, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """`None` used to reach `Path(None)` in the guard, which is a TypeError."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        derived = repo_root / leaf("nb4")
        derived.mkdir(parents=True)

        wt_info = MagicMock()
        wt_info.owner, wt_info.repo, wt_info.branch = "owner", "repo", "nb4"
        wt_info.local_path = None

        assert clone_manager.resolve_clone_path(wt_info) == derived

    def test_a_usable_recorded_path_still_wins(
        self, clone_manager, mock_repo_manager, tmp_repos_dir
    ):
        """The pre-#64 clones this fallback exists for must stay removable."""
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        old_path = repo_root / "nb4"  # the bare-branch-name leaf
        old_path.mkdir(parents=True)
        assert old_path.name != leaf("nb4")

        wt_info = MagicMock()
        wt_info.owner, wt_info.repo, wt_info.branch = "owner", "repo", "nb4"
        wt_info.local_path = old_path

        assert clone_manager.resolve_clone_path(wt_info) == old_path

    def test_a_record_dl_cannot_derive_from_names_no_directory_and_deletes_none(
        self, clone_manager, mock_storage, mock_repo_manager, tmp_repos_dir
    ):
        """A record that survives neither route is a refusal, not an empty answer.

        `get_workspace_path` raises `ValueError` on an unsafe ref, and a
        hand-edited or truncated `metadata.json` can hold one. Letting that
        propagate would take down the whole of `dl --ls --json` for one bad
        record -- the harm `read_clone`'s stat guard exists to prevent -- so it
        becomes None, and None has to mean "remove nothing" at the delete.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        repo_root.mkdir(parents=True)

        wt_info = MagicMock()
        wt_info.owner, wt_info.repo = "owner", "repo"
        wt_info.branch = "--evil"  # refused by WorkspaceId's validator
        wt_info.local_path = repo_root / "not-on-disk"
        mock_storage.get_worktree_by_workspace_id.return_value = wt_info

        assert clone_manager.resolve_clone_path(wt_info) is None
        assert clone_manager.remove_workspace_by_id("repo-evil") is False

    def test_the_delete_removes_exactly_what_resolve_named(
        self, clone_manager, mock_storage, mock_repo_manager, tmp_repos_dir
    ):
        """The binding itself: one resolution, and the delete uses that one.

        Without this, the two could be made to disagree again by editing
        `remove_workspace_by_id` alone and every test above would stay green.
        """
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root

        derived = repo_root / leaf("nb4")
        derived.mkdir(parents=True)
        (derived / ".git").mkdir()

        wt_info = MagicMock()
        wt_info.owner, wt_info.repo, wt_info.branch = "owner", "repo", "nb4"
        wt_info.local_path = repo_root / "moved-away"
        mock_storage.get_worktree_by_workspace_id.return_value = wt_info

        named = clone_manager.resolve_clone_path(wt_info)
        assert clone_manager.remove_workspace_by_id("repo-nb4") is True
        assert not named.exists(), "the delete removed something else"


class TestRefsReachingGit:
    """The refs handed to git are all either proven or checked at the boundary.

    `_validate_ref` used to be a method returning its own argument, so a caller
    could skip it and nothing said so — which is how get_workspace_path ended up
    unguarded. It is gone: refs that name a workspace arrive inside a WorkspaceId,
    and the one ref that does not (the stored default branch) is checked with the
    same predicate where it enters argv.
    """

    @pytest.mark.parametrize("bad", ["--evil", "", "branch name"])
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_remote_ref_exists_rejects_unsafe_names(self, mock_run, bad, clone_manager, tmp_path):
        with pytest.raises(ValueError, match="Invalid git ref name"):
            clone_manager._remote_ref_exists(tmp_path, bad)
        mock_run.assert_not_called()

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_remote_ref_exists_rejects_unsafe_remote(self, mock_run, clone_manager, tmp_path):
        with pytest.raises(ValueError, match="Invalid git remote name"):
            clone_manager._remote_ref_exists(tmp_path, "main", remote="--upload-pack=evil")
        mock_run.assert_not_called()


class TestRemoteRefExists:
    """Tests for _remote_ref_exists."""

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_returns_true_when_ref_exists(self, mock_run, clone_manager, tmp_repos_dir):
        """Test returns True when remote ref exists."""
        mock_run.return_value = MagicMock(returncode=0)
        ws_path = tmp_repos_dir / "ws"

        result = clone_manager._remote_ref_exists(ws_path, "main")

        assert result is True
        mock_run.assert_called_once_with(
            ["git", "show-ref", "--verify", "refs/remotes/origin/main"],
            cwd=ws_path,
            capture_output=True,
            text=True,
            check=False,
        )

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_returns_false_when_ref_missing(self, mock_run, clone_manager, tmp_repos_dir):
        """Test returns False when remote ref does not exist."""
        mock_run.return_value = MagicMock(returncode=1)
        ws_path = tmp_repos_dir / "ws"

        result = clone_manager._remote_ref_exists(ws_path, "no-such-branch")

        assert result is False

    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_custom_remote(self, mock_run, clone_manager, tmp_repos_dir):
        """Test with a custom remote name."""
        mock_run.return_value = MagicMock(returncode=0)
        ws_path = tmp_repos_dir / "ws"

        result = clone_manager._remote_ref_exists(ws_path, "main", remote="upstream")

        assert result is True
        mock_run.assert_called_once_with(
            ["git", "show-ref", "--verify", "refs/remotes/upstream/main"],
            cwd=ws_path,
            capture_output=True,
            text=True,
            check=False,
        )


class TestTheDevpodWorkspaceIdIsWrittenDown:
    """The record says which devpod workspace this clone belongs to.

    devlaunch#88. The field has existed on ``WorktreeInfo`` since the worktree
    backend was written and nothing ever assigned it, so when the id derivation
    moved under #81 there was no second copy of the old id anywhere and every
    workspace created before the change became unaddressable. These pin that the
    id goes on the record at the moment the clone is prepared, which is the last
    point before dl hands that same string to devpod as ``--id``.
    """

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_a_new_clone_records_the_id_dl_hands_devpod(
        self, mock_run, _mock_which, clone_manager, mock_storage, mock_repo_manager, tmp_repos_dir
    ):
        mock_run.return_value = MagicMock(returncode=0)
        mock_repo_manager.get_repo_path.return_value = tmp_repos_dir / "owner" / "repo"

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        recorded = mock_storage.add_worktree.call_args[0][0]
        assert recorded.devpod_workspace_id == leaf()

    @patch("devlaunch.worktree.workspace_clone.shutil.which", return_value=None)
    @patch("devlaunch.worktree.workspace_clone.subprocess.run")
    def test_re_registering_an_existing_clone_records_it_too(
        self, mock_run, _mock_which, clone_manager, mock_storage, mock_repo_manager, tmp_repos_dir
    ):
        """The clone is already on disk and devpod has forgotten the workspace.

        This is the path a record written by an older dl comes back through, so
        it is the one that fills the field in for workspaces that predate it.
        Skipping it would leave exactly the population this ticket is about
        still carrying no id.
        """
        mock_run.return_value = MagicMock(returncode=0)
        repo_root = tmp_repos_dir / "owner" / "repo"
        mock_repo_manager.get_repo_path.return_value = repo_root
        ws_path = repo_root / leaf()
        ws_path.mkdir(parents=True)
        (ws_path / ".git").mkdir()

        clone_manager.prepare_cold("owner", "repo", "nb4", "git@github.com:owner/repo.git")

        recorded = mock_storage.add_worktree.call_args[0][0]
        assert recorded.devpod_workspace_id == leaf()
