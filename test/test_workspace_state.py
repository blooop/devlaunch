# pylint: disable=redefined-outer-name,unused-argument
"""What a workspace holds, and dl's refusal to destroy the only copy of it.

A workspace per branch means workspaces accumulate. Removing the finished ones
is not dl's decision -- "finished" is a fact about tickets and intent, and dl
knows about clones and containers -- so dl provides the two halves a caller that
*does* know needs, and these tests pin both:

1. `dl --ls --json`: what exists, what each workspace is for, and what it holds
   that exists nowhere else.
2. `dl <ws> rm`: deletes one, and refuses when that would destroy unpushed work.

The git tests use real repositories with a local bare as their remote -- a local
path is a real git remote, so push, fetch and the remote-tracking refs all
behave exactly as they do over ssh, with no network. Mocking git here would only
prove this file agrees with itself, and the two bugs these tests caught (an
argument order that reported every clone as safe to delete, and git's discovery
walking up into an ancestor repository) were both invisible to everything except
a real git.
"""

import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import Optional
from unittest.mock import MagicMock, patch

import pytest

from devlaunch.dl import main
from devlaunch.workspace_id import WorkspaceId
from devlaunch.worktree.workspace_clone import WorkspaceCloneManager
from devlaunch.workspace_state import (
    CloneState,
    CouldNotTell,
    NothingToLose,
    WouldLose,
    holds_unsaved_work,
    read_clone,
    unsaved_as_json,
)


def git(*args: str, cwd: Path) -> str:
    """Run git for real, failing the test with git's own words if it refuses."""
    result = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=False)
    assert result.returncode == 0, f"git {' '.join(args)}: {result.stderr}"
    return result.stdout.strip()


def commit(work: Path, message: str) -> None:
    git("add", "-A", cwd=work)
    git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", message, cwd=work)


@pytest.fixture
def remote(tmp_path: Path) -> Path:
    """A bare repo standing in for GitHub, with one commit on `main`."""
    origin = tmp_path / "origin.git"
    seed = tmp_path / "seed"
    git("init", "-b", "main", str(seed), cwd=tmp_path)
    (seed / "README.md").write_text("seed\n")
    commit(seed, "seed")
    git("clone", "--bare", str(seed), str(origin), cwd=tmp_path)
    return origin


def _make_clone(remote: Path, work: Path) -> Path:
    """A workspace clone on a pushed branch, as `dl` would leave one."""
    work.parent.mkdir(parents=True, exist_ok=True)
    git("clone", str(remote), str(work), cwd=work.parent)
    git("checkout", "-b", "feature", cwd=work)
    (work / "feature.txt").write_text("work\n")
    commit(work, "feature")
    git("push", "-u", "origin", "feature", cwd=work)
    return work


@pytest.fixture
def clone(remote: Path, tmp_path: Path) -> Path:
    return _make_clone(remote, tmp_path / "ws")


@pytest.fixture
def clone_in_the_cache(remote: Path, tmp_path: Path) -> Path:
    """The same clone, at the path `dl` actually puts one: under its own cache.

    Ownership is established from the source path devpod kept, so this is a clone
    `dl` can prove is its own with or without a metadata record of it. It is the
    path `TestTheJsonListing` hands devpod as the workspace source.
    """
    return _make_clone(
        remote, tmp_path / "cache" / "devlaunch" / "repos" / "blooop" / "r" / "r-feature-aaa"
    )


@pytest.fixture
def ancestor(tmp_path: Path) -> Path:
    """A repository that is clean, fully pushed, and ignores `.cache/`.

    The dotfiles-in-`$HOME` case, and the reason devlaunch#171 only shows itself
    on a tidy host: git's discovery walking up out of a broken clone lands here,
    and a repository with nothing to report answers "nothing to report".
    """
    host = tmp_path / "host"
    host.mkdir()
    git("init", "-q", "-b", "main", ".", cwd=host)
    (host / ".gitignore").write_text(".cache/\n")
    commit(host, "seed")
    origin = tmp_path / "host-origin.git"
    git("init", "-q", "--bare", str(origin), cwd=tmp_path)
    git("remote", "add", "origin", str(origin), cwd=host)
    git("push", "-q", "-u", "origin", "main", cwd=host)
    # The premise, asserted rather than assumed: if this repo were dirty or had
    # an unpushed commit the guard would fire for the wrong reason and the bug
    # these tests are about would be invisible.
    assert git("status", "--porcelain", cwd=host) == ""
    assert git("log", "--oneline", "main", "--not", "--remotes", cwd=host) == ""
    return host


@pytest.fixture
def broken_clone_under_ancestor(ancestor: Path) -> Path:
    """A clone whose `.git` is unusable, holding scratch work, nested in a repo.

    What an interrupted delete, a truncated write or a half-copied cache leaves
    behind. The directory is there and holds a file that exists nowhere else.
    """
    clone = ancestor / ".cache" / "devlaunch" / "ws"
    clone.mkdir(parents=True)
    (clone / ".git").mkdir()
    (clone / ".git" / "HEAD").write_text("garbage\n")
    (clone / "scratch.md").write_text("half a plan\n")
    return clone


class TestWhatACloneHolds:
    def test_a_pushed_branch_with_a_clean_tree_holds_nothing_unsaved(self, clone):
        assert read_clone(clone) == CloneState(branch="feature", unsaved=NothingToLose())

    def test_an_unpushed_commit_is_unsaved(self, clone):
        (clone / "more.txt").write_text("more\n")
        commit(clone, "more")
        unsaved = holds_unsaved_work(clone)
        assert isinstance(unsaved, WouldLose) and "1 unpushed commit(s)" in unsaved.description

    def test_uncommitted_changes_are_unsaved(self, clone):
        (clone / "feature.txt").write_text("edited\n")
        unsaved = holds_unsaved_work(clone)
        assert isinstance(unsaved, WouldLose) and "1 uncommitted change(s)" in unsaved.description

    def test_untracked_files_are_unsaved_too(self, clone):
        # An agent's scratch notes are not less lost for never having been added.
        (clone / "notes.md").write_text("half a plan\n")
        unsaved = holds_unsaved_work(clone)
        assert isinstance(unsaved, WouldLose) and "uncommitted" in unsaved.description

    def test_both_kinds_of_loss_are_reported_together(self, clone):
        (clone / "more.txt").write_text("more\n")
        commit(clone, "more")
        (clone / "dirty.txt").write_text("dirty\n")
        unsaved = holds_unsaved_work(clone)
        assert isinstance(unsaved, WouldLose)
        assert "uncommitted" in unsaved.description and "unpushed" in unsaved.description

    def test_a_branch_whose_commits_are_on_the_remote_under_another_name_is_saved(
        self, clone, remote
    ):
        # Pushed under a second name: the commits exist elsewhere, so nothing
        # would be lost. Asking about *any* remote ref rather than this branch's
        # upstream is what gets this right.
        git("push", "origin", "feature:review/feature", cwd=clone)
        git("branch", "-m", "feature", "renamed", cwd=clone)
        assert holds_unsaved_work(clone) == NothingToLose()

    def test_a_clone_that_is_not_there_holds_nothing(self, tmp_path):
        # A half-finished delete, or a directory removed by hand. There is no
        # work in it to lose, and nothing here may crash on it.
        assert read_clone(tmp_path / "absent") == CloneState(branch=None, unsaved=NothingToLose())

    def test_a_path_with_a_file_at_it_rather_than_a_clone_also_holds_nothing(self, tmp_path):
        # Neither a clone nor a directory: the same answer as nothing at all,
        # which is what this gave before the errno was read directly.
        (tmp_path / "ws").write_text("not a clone\n")
        assert read_clone(tmp_path / "ws") == CloneState(branch=None, unsaved=NothingToLose())

    def test_a_clone_under_something_that_is_not_a_directory_holds_nothing(self, tmp_path):
        # ENOTDIR rather than ENOENT: a parent component is a file. Still no
        # clone at that path, so still nothing in it to lose.
        (tmp_path / "file").write_text("not a directory\n")
        assert read_clone(tmp_path / "file" / "ws") == CloneState(
            branch=None, unsaved=NothingToLose()
        )

    def test_a_clone_nested_in_a_repository_still_answers_about_itself(self, ancestor, remote):
        """A healthy clone under a repository is not confused with its ancestor.

        The other half of devlaunch#171: pinning the clone down must not cost
        the ordinary answer. `dl`'s cache lives under `$XDG_CACHE_HOME`, which
        on a great many machines is inside a dotfiles repository.
        """
        work = ancestor / ".cache" / "devlaunch" / "ws"
        git("clone", "-q", str(remote), str(work), cwd=ancestor)
        git("checkout", "-q", "-b", "feature", cwd=work)
        (work / "mine.txt").write_text("mine\n")
        state = read_clone(work)
        assert state.branch == "feature"
        assert isinstance(state.unsaved, WouldLose) and "mine.txt" in state.unsaved.description


class TestWhenGitCannotBeAsked:
    """devlaunch#171: a refusal is an answer of its own, and it is not "clean".

    `_git` used to run with `cwd=` alone -- no `--git-dir`, no `--work-tree`, no
    ceiling -- so git's repository discovery walked up the parent chain. A clone
    whose `.git` was unusable did not make git refuse; it made git answer about
    an **ancestor** repository. On a tidy ancestor that answer is "nothing to
    report", `holds_unsaved_work` returned `None`, and `None` meant delete
    freely. The scratch file in the clone went with it.

    Two things had to change and both are pinned here: git is now asked about
    one directory and cannot leave it, and "could not tell" is an arm of its own
    that refuses the delete exactly as "would lose" does.
    """

    def test_an_unusable_git_is_could_not_tell_not_nothing_to_lose(
        self, broken_clone_under_ancestor
    ):
        unsaved = holds_unsaved_work(broken_clone_under_ancestor)
        assert isinstance(unsaved, CouldNotTell), unsaved
        # And it must say so of *this* directory, not of the one git wandered
        # into: the reason is what a person reads before deciding to force.
        assert str(broken_clone_under_ancestor) in unsaved.reason

    def test_it_does_not_borrow_the_ancestors_branch_either(self, broken_clone_under_ancestor):
        # The shipped bug reported `branch='main'` -- the ancestor's checked-out
        # branch -- and `dl --ls --json` printed it as this clone's `checkedOut`.
        assert read_clone(broken_clone_under_ancestor).branch is None

    def test_an_unusable_git_with_no_ancestor_at_all_is_also_could_not_tell(self, tmp_path):
        # The same directory with nothing above it to walk into. git refuses
        # either way now, so the answer does not depend on what the machine
        # happens to have in a parent directory.
        clone = tmp_path / "ws"
        clone.mkdir()
        (clone / ".git").mkdir()
        (clone / ".git" / "HEAD").write_text("garbage\n")
        (clone / "scratch.md").write_text("half a plan\n")
        assert isinstance(holds_unsaved_work(clone), CouldNotTell)

    def test_a_directory_that_is_not_a_repository_cannot_be_judged(self, tmp_path):
        """A present directory that is not a repository is not an empty one.

        This function used to answer `None` here, documented as "a directory
        that is not there, or is not a repository, holds nothing". Half of that
        is true and stays true -- a directory that is *not there* holds nothing,
        which is what lets a caller clear away a workspace whose clone was
        removed by hand. The other half was the bug: a directory that *is*
        there and is not a repository holds whatever files are in it, and git,
        having no repository to read, cannot say whether they exist anywhere
        else. That is a refusal, and a refusal is not permission.
        """
        plain = tmp_path / "plain"
        plain.mkdir()
        (plain / "file.txt").write_text("not a repo\n")
        state = read_clone(plain)
        assert state.branch is None
        assert isinstance(state.unsaved, CouldNotTell)

    def test_a_half_removed_clone_is_could_not_tell(self, clone):
        """An interrupted delete: a real clone with its object store gone.

        Named separately from the garbage-`.git` case because it is the one the
        issue describes reaching in the wild, and because it is the shape where
        the *files* are still all there to lose.
        """
        (clone / "scratch.md").write_text("half a plan\n")
        subprocess.run(["rm", "-rf", str(clone / ".git" / "objects")], check=True)
        assert isinstance(holds_unsaved_work(clone), CouldNotTell)

    def test_a_readable_repo_whose_remote_refs_are_broken_is_could_not_tell(self, clone):
        """The second refusal, which the first would otherwise hide.

        `git status` succeeds -- it never looks at remote-tracking refs -- so
        the repository probe passes and the clone reads as clean right up until
        `git log … --not --remotes` is asked, which refuses on a ref pointing at
        an object that is not there. Answering "nothing to lose" on the strength
        of the half that worked is the same bug in a narrower place: the unpushed
        commits were never counted. A real broken ref, not a patched subprocess,
        because the point is what git does.
        """
        bogus = clone / ".git" / "refs" / "remotes" / "origin" / "bogus"
        bogus.parent.mkdir(parents=True, exist_ok=True)
        bogus.write_text("0123456789abcdef0123456789abcdef01234567\n")
        unsaved = holds_unsaved_work(clone)
        assert isinstance(unsaved, CouldNotTell), unsaved
        assert "unpushed commits" in unsaved.reason and "feature" in unsaved.reason

    def test_git_that_cannot_be_run_at_all_is_could_not_tell(self, clone):
        # No git on PATH, a fork that fails: the process-level refusal, which
        # never reaches a return code to inspect.
        with patch("devlaunch.workspace_state.subprocess.run", side_effect=OSError("no git")):
            unsaved = holds_unsaved_work(clone)
        assert isinstance(unsaved, CouldNotTell) and "no git" in unsaved.reason


class TestGitIsPinnedToItsWorkTreeToo:
    """`--git-dir` names the repository; `--work-tree` names the files.

    The flags are not one belt and one brace. `core.worktree` in a clone's own
    config points the work tree at another directory, and `--git-dir` alone
    honours it -- so `git status --porcelain` compares the clone's index against
    *that* directory. Dropping `--work-tree` and keeping `--git-dir` left the
    rest of this suite green, so this class exists to make that mutation red.

    What the mutation does depends on what the other directory holds, and the
    first version of this class pinned the wrong one of the two. Run against git
    2.55.0:

    - the other directory does not hold HEAD's files (an empty directory) and
      git prints `" D README.md\\n D feature.txt"` at rc 0 -- a `WouldLose`
      naming two files that are not missing. Wrong, but a *refusal*: it destroys
      nothing, and a test that only checks which filenames appear is red under
      the mutation for a reason that is not the safety property.
    - the other directory mirrors HEAD (a second checkout of the same commit,
      which is what `core.worktree` is normally pointed at) and git prints
      nothing at rc 0 -- `NothingToLose()` on a clone holding an hour of work
      that exists nowhere else. Devlaunch#171's failure class reached by a
      second route, and the fail-*open* one.

    `test_a_clone_whose_other_work_tree_mirrors_head...` builds the second shape,
    so it goes red on the arm rather than on a filename. Its neighbour
    `core.bare = true` is not a third case: with it, `--git-dir` alone says
    `fatal: this operation must be run in a work tree` at rc 128, which is a
    refusal and therefore already safe.
    """

    @staticmethod
    def _pointed_elsewhere(clone: Path, elsewhere: Path, mirror_head: bool) -> None:
        elsewhere.mkdir()
        if mirror_head:
            # A checkout of the same commit, built from what git says is tracked
            # rather than from a list written here, so it mirrors HEAD by
            # construction and stays mirrored if the fixture gains a file.
            for tracked in git("ls-files", cwd=clone).splitlines():
                (elsewhere / tracked).write_bytes((clone / tracked).read_bytes())
        git("config", "core.worktree", str(elsewhere), cwd=clone)

    def test_a_clone_whose_other_work_tree_mirrors_head_still_reports_its_own_work(
        self, clone, tmp_path
    ):
        """The fail-open, and the assertion that matters is the arm.

        Without `--work-tree` git looks at the mirror, finds it identical to the
        index, and says nothing at rc 0: `NothingToLose()` on the clone below,
        which holds a file that exists nowhere else. `isinstance` is what goes
        red; the filename is checked afterwards because a `WouldLose` about the
        wrong directory would be a different defect.
        """
        self._pointed_elsewhere(clone, tmp_path / "elsewhere", mirror_head=True)
        (clone / "an-hour-of-work.md").write_text("half a plan\n")
        unsaved = holds_unsaved_work(clone)
        assert isinstance(unsaved, WouldLose), unsaved
        assert "an-hour-of-work.md" in unsaved.description

    def test_a_clean_clone_is_not_made_dirty_by_the_other_work_trees_absences(
        self, clone, tmp_path
    ):
        # The other outcome, pinned so it cannot be mistaken for the one above:
        # an empty other work tree makes `--git-dir` alone report HEAD's files as
        # deleted, so this clone -- which is clean -- would be refused for work
        # it does not hold. Also red under the mutation, and for the complaint a
        # user would actually file.
        self._pointed_elsewhere(clone, tmp_path / "elsewhere", mirror_head=False)
        assert isinstance(holds_unsaved_work(clone), NothingToLose)


class TestADirectoryThatCannotBeLookedAt:
    """Not knowing whether the clone is there is not knowing what it holds.

    A clone whose parent directory `dl` has no search permission on. This used
    to be `clone.is_dir()`, which gave two different wrong answers depending on
    which Python was running it: up to and including 3.13 it re-raised
    `PermissionError`, so `dl <ws> rm` failed closed by crashing and
    `dl --ls --json` became a traceback for the whole listing because of one
    workspace; on 3.14 the same call returns `False`, which read as "not there,
    so nothing to lose" -- a clone that may be full of work, reported as free to
    delete. A version-independent assertion is the point of this class.

    The boundary is 3.14 and it was executed, not read off a changelog: the
    mode-`000` parent below was run on this repo's own environments and
    **3.10.20, 3.11.15, 3.12.13 and 3.13.14 raise; 3.14.6 returns `False`**. It
    was written here as "3.13+" for two rounds of review, which is a whole minor
    version out. No test here would have said so: what this class asserts is the
    same on every version -- that is the point of it -- so it is green whichever
    side of the boundary the prose names, and what corrected it was somebody
    running the mode-`000` parent on 3.14 rather than reading about it. The `ci`
    matrix in `.github/workflows/ci.yml` ran py310 through py313 only and now
    includes the `default` environment, which is 3.14, so "this suite runs on
    both sides of the boundary" is true of GitHub rather than only of whoever
    ran it locally.
    """

    def test_a_clone_behind_a_closed_door_is_could_not_tell(self, tmp_path):
        if os.geteuid() == 0:
            pytest.skip("root is refused by nothing, so the closed door would open")
        parent = tmp_path / "locked"
        clone = parent / "ws"
        clone.mkdir(parents=True)
        (clone / "an-hour-of-work.md").write_text("half a plan\n")
        parent.chmod(0o000)
        try:
            state = read_clone(clone)
        finally:
            parent.chmod(0o700)
        assert isinstance(state.unsaved, CouldNotTell), state
        assert str(clone) in state.unsaved.reason

    def test_a_recorded_path_that_is_not_a_path_at_all_is_could_not_tell(self, tmp_path):
        # `os.stat` is total over `OSError` and not over `ValueError`: a NUL byte
        # in the path is rejected before the syscall, and a hand-edited or
        # truncated `metadata.json` is how one gets into a record. Uncaught it
        # takes the whole of `dl --ls --json` down for one bad row, which is the
        # harm the stat guard was written to stop.
        state = read_clone(Path(f"{tmp_path / 'ws'}\0truncated"))
        assert isinstance(state.unsaved, CouldNotTell), state


class TestTheAnswersAreTotal:
    """Nothing may reach a caller as an answer the caller does not name.

    Three arms, and every place that reads them says which arm it is handling.
    A fourth would raise here rather than being read as permission to delete --
    which is what the `Optional[str]` this replaced did with anything it had no
    case for.
    """

    def test_an_arm_nobody_handles_is_refused_rather_than_rendered(self):
        with pytest.raises(AssertionError, match="Unhandled unsaved-work answer"):
            unsaved_as_json("nothing to lose, honest")  # type: ignore[arg-type]

    def test_would_lose_cannot_be_built_with_nothing_to_say(self):
        # "workspace holds ." reads as a bug in dl rather than as a reason to
        # stop, and the arm for having nothing to report already exists.
        with pytest.raises(ValueError):
            WouldLose("")

    def test_each_arm_renders_as_one_key_that_names_it(self):
        assert unsaved_as_json(NothingToLose()) == {"nothingToLose": True}
        assert unsaved_as_json(WouldLose("1 unpushed commit(s)")) == {
            "wouldLose": "1 unpushed commit(s)"
        }
        assert unsaved_as_json(CouldNotTell("git said no")) == {"couldNotTell": "git said no"}


def _entry(workspace_id: str, source: Path) -> dict:
    return {
        "id": workspace_id,
        "source": {"localFolder": str(source)},
        "provider": {"name": "docker"},
        "ide": {"name": "none"},
        "lastUsed": "2026-08-08T11:43:27Z",
    }


class FakeRecord:
    def __init__(self, owner, repo, branch, local_path):
        self.owner, self.repo, self.branch, self.local_path = owner, repo, branch, local_path


def _clone_manager(records: dict, repos_dir: Optional[Path] = None) -> MagicMock:
    manager = MagicMock()
    manager.storage.get_worktree_by_workspace_id.side_effect = records.get

    def get_workspace_path(owner: str, repo: str, branch: str) -> Path:
        """The derived clone directory, for the tests that reach the fallback.

        Raises rather than returning a `Mock` when a test has not said where the
        cache is: a `Mock` would flow into `read_clone` and be reported as an
        unreadable directory, which is a plausible-looking answer to a question
        the test never meant to ask.
        """
        if repos_dir is None:
            raise AssertionError(
                "this test reached the path derivation; pass repos_dir to _clone_manager"
            )
        return Path(repos_dir) / owner / repo / WorkspaceId(owner, repo, branch).value

    manager.get_workspace_path.side_effect = get_workspace_path

    # The real method, called with the double as `self`, rather than a copy of
    # its rule. devlaunch#174 was two places naming one directory and drifting
    # apart; a test double holding a third copy is the same mistake again, and
    # it would go on passing after the rule changed under it.
    manager.resolve_clone_path.side_effect = lambda record: (
        WorkspaceCloneManager.resolve_clone_path(manager, record)
    )

    def remove_workspace_by_id(workspace_id: str) -> bool:
        """What `WorkspaceCloneManager.remove_workspace_by_id` does, and only that.

        Both halves, because the interesting one is the refusal: with no record
        it removes nothing and returns False (`worktree/workspace_clone.py`), so
        a workspace dl has no record for keeps its clone whatever `rm` returns.
        A `MagicMock` returns a truthy `Mock` there and deletes nothing, which
        makes "the clone is still on disk" true of every test for the wrong
        reason.
        """
        record = records.get(workspace_id)
        if record is None:
            return False
        shutil.rmtree(record.local_path, ignore_errors=True)
        return True

    manager.remove_workspace_by_id.side_effect = remove_workspace_by_id
    return manager


class TestTheJsonListing:
    """`dl --ls --json`: the facts a cleanup tool decides from."""

    def _run(self, tmp_path, clone, capsys, recorded=True, foreign_recorded=False):
        cache = tmp_path / "cache" / "devlaunch"
        listing = json.dumps(
            [
                _entry("r-feature-aaa", cache / "repos" / "blooop" / "r" / "r-feature-aaa"),
                _entry("someone-elses", tmp_path / "projects" / "other"),
            ]
        )
        records = (
            {"r-feature-aaa": FakeRecord("blooop", "r", "feature", str(clone))} if recorded else {}
        )
        if foreign_recorded:
            # dl wrote a record and the workspace still reads as someone else's.
            # `is_devlaunch_clone`'s docstring names the way in: a `config.toml`
            # pointing `repos_dir` outside the cache puts dl's clones somewhere
            # `--purge` does not reach, so they are foreign by the only test that
            # keeps ownership and deletion answering to the same directory --
            # and the record dl wrote for them outlives the config change.
            records["someone-elses"] = FakeRecord("blooop", "other", "main", str(clone))

        def devpod(args, **kwargs):
            if args[:1] == ["list"]:
                return subprocess.CompletedProcess(args, 0, listing, "")
            if args[:1] == ["status"]:
                return subprocess.CompletedProcess(args, 0, json.dumps({"state": "Stopped"}), "")
            return subprocess.CompletedProcess(args, 0, "", "")

        with (
            patch("devlaunch.dl._get_cache_dir", return_value=cache),
            patch("devlaunch.dl.run_devpod", side_effect=devpod),
            patch(
                "devlaunch.dl._get_clone_manager",
                return_value=_clone_manager(records, repos_dir=cache / "repos"),
            ),
        ):
            code = main(["--ls", "--json"])
        return code, json.loads(capsys.readouterr().out)

    def test_it_reports_the_repo_branch_and_that_nothing_is_unsaved(self, tmp_path, clone, capsys):
        code, report = self._run(tmp_path, clone, capsys)
        assert code == 0
        ours = next(w for w in report if w["id"] == "r-feature-aaa")
        assert ours["devlaunch"] is True
        assert ours["repo"] == "blooop/r"
        assert ours["branch"] == "feature"
        assert ours["unsaved"] == {"nothingToLose": True}
        assert ours["state"] == "Stopped"

    def test_unsaved_work_is_reported_so_a_caller_can_leave_it_alone(self, tmp_path, clone, capsys):
        (clone / "scratch.txt").write_text("half-finished\n")
        _code, report = self._run(tmp_path, clone, capsys)
        ours = next(w for w in report if w["id"] == "r-feature-aaa")
        assert "uncommitted" in ours["unsaved"]["wouldLose"]

    def test_a_clone_git_cannot_read_is_not_reported_as_nothing_to_lose(
        self, tmp_path, broken_clone_under_ancestor, capsys
    ):
        """The JSON surface of devlaunch#171.

        One key, and the key says which kind of answer it is -- the shape
        `disk` already uses, for the same reason. A caller cannot arrive at
        "nothing would be lost" by reading a field that is absent, and the old
        `null` cannot be mistaken for it either, because `null` here now means
        only "not `dl`'s clone to inspect".
        """
        _code, report = self._run(tmp_path, broken_clone_under_ancestor, capsys)
        ours = next(w for w in report if w["id"] == "r-feature-aaa")
        assert "nothingToLose" not in ours["unsaved"]
        assert str(broken_clone_under_ancestor) in ours["unsaved"]["couldNotTell"]
        # And not the ancestor's branch, reported as this clone's.
        assert ours["checkedOut"] is None

    def test_a_clone_dl_owns_but_has_no_record_for_is_still_inspected(
        self, tmp_path, clone_in_the_cache, capsys
    ):
        """`unsaved: null` must mean "not `dl`'s clone", and only that.

        A clone under the cache that `dl` has no metadata record for -- a
        metadata write that failed, a record pruned, a cache restored without
        one -- is still `dl`'s clone: `devlaunch: true` says so, `--size`
        measures it, and `--purge` deletes it. This used to gate on the record
        while its neighbours gated on the clone directory, so that row read
        `devlaunch: true` with a measured `disk` and `unsaved: null` beside
        them, and `null` is documented as "no clone of `dl`'s here to inspect".
        That is devlaunch#171's own sentinel surviving one layer out: `null`
        carrying both "not mine" and "mine, never examined". Exactly the
        divergence PR #165 closed for `disk`, which `unsaved` was left behind
        by.
        """
        (clone_in_the_cache / "an-hour-of-work.md").write_text("half a plan\n")
        _code, report = self._run(tmp_path, clone_in_the_cache, capsys, recorded=False)
        ours = next(w for w in report if w["id"] == "r-feature-aaa")
        assert ours["devlaunch"] is True
        # No record, so nothing to report for the two fields that come from one.
        assert ours["repo"] is None and ours["branch"] is None
        # But the clone is there, and so is the work in it.
        assert "an-hour-of-work.md" in ours["unsaved"]["wouldLose"]
        assert ours["path"] == str(clone_in_the_cache)
        assert ours["checkedOut"] == "feature"
        # And on the same listing, `null` still means the one thing it means.
        foreign = next(w for w in report if w["id"] == "someone-elses")
        assert foreign["devlaunch"] is False and foreign["unsaved"] is None

    def test_a_clone_that_is_simply_gone_is_still_read_rather_than_assumed(self, tmp_path, capsys):
        """An absent clone is an answer `read_clone` gives, not one a caller may skip.

        `dl <ws> rm` clears away a workspace whose clone was removed by hand
        precisely because an absent directory is `NothingToLose()` rather than a
        refusal, and this row has to say the same thing about it. Not calling
        `read_clone` for a directory that is not on disk -- a plausible "don't
        shell out to git for something that isn't there" edit, and the one
        mutation of `dl.py`'s single-question restructure that the rest of this
        class leaves green -- writes `unsaved: null` onto a row that says
        `devlaunch: true` and names a `path` on the lines above it. That is
        devlaunch#171's sentinel again: `null` meaning "mine, not examined"
        rather than "not mine".
        """
        gone = tmp_path / "cache" / "devlaunch" / "repos" / "blooop" / "r" / "r-feature-aaa"
        assert not gone.exists()
        _code, report = self._run(tmp_path, gone, capsys)
        ours = next(w for w in report if w["id"] == "r-feature-aaa")
        assert ours["devlaunch"] is True
        # Not `gone` itself. The recorded path is not on disk, so the row names
        # the directory `dl <ws> rm` would actually act on -- which is the whole
        # point of devlaunch#174: `path`, `unsaved` and the delete are one
        # directory or they are a trap. In production these two strings are the
        # same, because a clone's leaf *is* its workspace id; only this fixture
        # gives the workspace a shorter id than the derivation would.
        derived = (
            tmp_path
            / "cache"
            / "devlaunch"
            / "repos"
            / "blooop"
            / "r"
            / WorkspaceId("blooop", "r", "feature").value
        )
        assert ours["path"] == str(derived)
        assert ours["unsaved"] == {"nothingToLose": True}
        # Nothing there to have a branch checked out, and no guess about one.
        assert ours["checkedOut"] is None

    def test_one_unreachable_clone_does_not_take_the_whole_listing_down(
        self, tmp_path, clone_in_the_cache, capsys
    ):
        # The harm `read_clone`'s stat guard prevents, at the surface it was
        # visible from: one workspace behind a closed door used to raise out of
        # the loop and the caller got a traceback instead of a listing.
        if os.geteuid() == 0:
            pytest.skip("root is refused by nothing, so the closed door would open")
        clone_in_the_cache.parent.chmod(0o000)
        try:
            _code, report = self._run(tmp_path, clone_in_the_cache, capsys)
        finally:
            clone_in_the_cache.parent.chmod(0o700)
        ours = next(w for w in report if w["id"] == "r-feature-aaa")
        assert "couldNotTell" in ours["unsaved"], ours["unsaved"]
        # And the rest of the listing is still there.
        assert next(w for w in report if w["id"] == "someone-elses")["devlaunch"] is False

    def test_a_workspace_devlaunch_did_not_make_says_so_instead_of_guessing(
        self, tmp_path, clone, capsys
    ):
        _code, report = self._run(tmp_path, clone, capsys)
        foreign = next(w for w in report if w["id"] == "someone-elses")
        assert foreign["devlaunch"] is False
        assert foreign["repo"] is None and foreign["branch"] is None
        # And nothing inspected it: dl has no clone of its own to protect there.
        assert foreign["unsaved"] is None

    def test_a_leftover_record_cannot_make_a_foreign_workspace_dls_own(
        self, tmp_path, clone, capsys
    ):
        """`unsaved: null` iff `devlaunch: false`, on the input that can break it.

        The two fields used to come from two questions -- `is_devlaunch_clone`
        for `devlaunch`, the metadata record for the directory `unsaved`
        describes -- and the claim that they name the same set was a claim that
        the two agree. They do not have to: a record can outlive the reason its
        workspace counts as dl's, so a `devlaunch: false` row with a record
        behind it reported a `wouldLose` about a directory the same row called
        `path: null`, on a checkout dl has no business inspecting. Nothing here
        caught that -- deleting the ownership gate on the record lookup left the
        suite green.

        `mine` is now defined as "there is a clone dl owns here", and the record
        can only move the row from one directory of dl's to another, so this row
        cannot be reached by a record at all.
        """
        _code, report = self._run(tmp_path, clone, capsys, foreign_recorded=True)
        foreign = next(w for w in report if w["id"] == "someone-elses")
        assert foreign["devlaunch"] is False
        assert foreign["unsaved"] is None
        # And none of the fields a record would have filled in, either -- above
        # all `path`, which is what `unsaved` would have been about.
        assert foreign["path"] is None
        assert foreign["repo"] is None and foreign["branch"] is None
        assert foreign["checkedOut"] is None
        # The row that is dl's is unaffected: this is not "inspect nothing".
        ours = next(w for w in report if w["id"] == "r-feature-aaa")
        assert ours["devlaunch"] is True and ours["unsaved"] == {"nothingToLose": True}

    def test_a_checked_out_branch_that_differs_from_the_record_is_reported_too(
        self, tmp_path, clone, capsys
    ):
        # An agent moved off the branch the workspace was made for. Both are
        # facts; neither is made to stand for the other.
        git("checkout", "-b", "sidequest", cwd=clone)
        _code, report = self._run(tmp_path, clone, capsys)
        ours = next(w for w in report if w["id"] == "r-feature-aaa")
        assert ours["branch"] == "feature"
        assert ours["checkedOut"] == "sidequest"


class TestReportingWhatAWorkspaceCostsOnDisk:
    """`dl --ls --size`: the bytes deleting a workspace's clone would free.

    Asked for rather than always answered, because the walk is O(files) with no
    ceiling and plain `--ls` is one devpod round-trip and no filesystem work at
    all -- and an ordinary devcontainer builds its environment *inside* the
    clone, so the file count is unbounded too. The measured cost and the machine
    it was measured on live in README, once.
    """

    def _run(
        self,
        tmp_path,
        argv,
        capsys,
        payload_mib=2,
        unreadable=False,
        only_foreign=False,
        recorded=True,
    ):
        cache = tmp_path / "cache" / "devlaunch"
        clone = cache / "repos" / "blooop" / "r" / "r-feature-aaa"
        clone.mkdir(parents=True, exist_ok=True)
        (clone / "payload.bin").write_bytes(b"\0" * (payload_mib * 1024 * 1024))
        if unreadable:
            (clone / "locked").mkdir(exist_ok=True)
            (clone / "locked").chmod(0o000)
        entries = [_entry("someone-elses", tmp_path / "projects" / "other")]
        if not only_foreign:
            entries.insert(0, _entry("r-feature-aaa", clone))
        listing = json.dumps(entries)
        records = (
            {"r-feature-aaa": FakeRecord("blooop", "r", "feature", str(clone))} if recorded else {}
        )

        def devpod(args, **kwargs):
            if args[:1] == ["list"]:
                return subprocess.CompletedProcess(args, 0, listing, "")
            if args[:1] == ["status"]:
                return subprocess.CompletedProcess(args, 0, json.dumps({"state": "Stopped"}), "")
            return subprocess.CompletedProcess(args, 0, "", "")

        try:
            with (
                patch("devlaunch.dl._get_cache_dir", return_value=cache),
                patch("devlaunch.dl.run_devpod", side_effect=devpod),
                patch(
                    "devlaunch.dl._get_clone_manager",
                    return_value=_clone_manager(records, repos_dir=cache / "repos"),
                ),
            ):
                code = main(argv)
        finally:
            if unreadable:
                (clone / "locked").chmod(0o700)
        return code, capsys.readouterr().out

    def _json(self, tmp_path, argv, capsys, **kwargs):
        code, out = self._run(tmp_path, argv, capsys, **kwargs)
        assert code == 0
        return json.loads(out)

    def test_the_listing_says_nothing_about_disk_unless_asked(self, tmp_path, capsys):
        report = self._json(tmp_path, ["--ls", "--json"], capsys)
        assert all("disk" not in row for row in report)

    def test_asking_reports_the_bytes_the_clone_would_free(self, tmp_path, capsys):
        report = self._json(tmp_path, ["--ls", "--json", "--size"], capsys)
        ours = next(row for row in report if row["id"] == "r-feature-aaa")
        # Two MiB of payload plus its directory, and nothing else claimed.
        assert 2 * 1024 * 1024 <= ours["disk"]["exclusiveBytes"] < 3 * 1024 * 1024

    def test_a_workspace_devlaunch_did_not_make_is_not_walked(self, tmp_path, capsys):
        # Its source is somebody's own project directory. dl has no clone there
        # to measure and no business walking one.
        report = self._json(tmp_path, ["--ls", "--json", "--size"], capsys)
        foreign = next(row for row in report if row["id"] == "someone-elses")
        assert foreign["devlaunch"] is False
        assert foreign["disk"] is None

    def test_what_could_not_be_read_is_reported_as_a_floor_not_a_total(self, tmp_path, capsys):
        if os.geteuid() == 0:
            pytest.skip("root is refused by nothing, so the closed door would open")
        report = self._json(tmp_path, ["--ls", "--json", "--size"], capsys, unreadable=True)
        ours = next(row for row in report if row["id"] == "r-feature-aaa")
        # A container writes into a clone as another user, so this is ordinary.
        # The one thing that must not happen is a floor read as a total.
        assert "exclusiveBytes" not in ours["disk"]
        assert ours["disk"]["atLeastBytes"] >= 2 * 1024 * 1024
        assert ours["disk"]["unreadable"] == 1

    def test_the_table_gains_a_size_column_when_asked(self, tmp_path, capsys):
        _code, out = self._run(tmp_path, ["--ls", "--size"], capsys)
        assert "SIZE" in out
        assert "2.0 MiB" in out

    def test_the_table_says_nothing_about_disk_unless_asked(self, tmp_path, capsys):
        _code, out = self._run(tmp_path, ["--ls"], capsys)
        assert "SIZE" not in out

    def test_the_table_leaves_a_foreign_workspace_unmeasured(self, tmp_path, capsys):
        _code, out = self._run(tmp_path, ["--ls", "--size"], capsys)
        foreign = next(line for line in out.splitlines() if line.startswith("someone-elses"))
        assert "-" in foreign.split()

    def test_the_two_renderings_measure_the_same_workspaces(self, tmp_path, capsys):
        """A clone with no metadata record is still `dl`'s clone on disk.

        The table and the JSON used to answer "may we measure this" from
        different things -- the source directory and the metadata record -- and
        a clone under the cache that `dl` had no record for was sized in the
        table and reported as `null` in the JSON. `null` is documented as "not
        `dl`'s to measure", which that clone is not: `--purge` deletes it, and a
        cleanup tool reading the JSON would leave behind disk a person reading
        the table can see. Both now ask the same question, so both answer it the
        same way.
        """
        report = self._json(tmp_path, ["--ls", "--json", "--size"], capsys, recorded=False)
        ours = next(row for row in report if row["id"] == "r-feature-aaa")
        assert ours["devlaunch"] is True
        assert 2 * 1024 * 1024 <= ours["disk"]["exclusiveBytes"] < 3 * 1024 * 1024
        # And the field a cleanup tool must not ignore is answered on the same
        # row, for the same reason: `unsaved: null` beside a measured `disk` on
        # a row that says `devlaunch: true` is `null` meaning two things again.
        # (This clone is a plain directory, so the answer is a refusal.)
        assert "couldNotTell" in ours["unsaved"]

        _code, out = self._run(tmp_path, ["--ls", "--size"], capsys, recorded=False)
        row = next(line for line in out.splitlines() if line.startswith("r-feature-aaa"))
        assert "2.0 MiB" in row

    def test_the_size_column_lines_up_with_its_heading(self, tmp_path, capsys):
        # A dash is one character and "SIZE" is four, so a column sized only
        # from its cells leaves every date after it shifted.
        _code, out = self._run(tmp_path, ["--ls", "--size"], capsys, only_foreign=True)
        header, _rule, *body = out.splitlines()
        for line in body:
            assert line.index("2026-08-08") == header.index("LAST USED")


class TestTheDeleteGuard:
    """`dl <ws> rm` refuses to destroy the only copy of something."""

    def _run(self, tmp_path, clone, argv, record_read_raises=None, recorded=True):
        cache = tmp_path / "cache" / "devlaunch"
        listing = json.dumps(
            [_entry("r-feature-aaa", cache / "repos" / "blooop" / "r" / "r-feature-aaa")]
        )
        records = (
            {"r-feature-aaa": FakeRecord("blooop", "r", "feature", str(clone))} if recorded else {}
        )
        deleted = []

        def devpod(args, **kwargs):
            if args[:1] == ["list"]:
                return subprocess.CompletedProcess(args, 0, listing, "")
            if args[:1] == ["status"]:
                # The spec is resolved with one `devpod status`, not a listing.
                return subprocess.CompletedProcess(args, 0, '{"state": "Stopped"}', "")
            if args[:1] == ["delete"]:
                deleted.append(args[1])
            return subprocess.CompletedProcess(args, 0, "", "")

        manager = _clone_manager(records, repos_dir=cache / "repos")
        if record_read_raises is not None:
            manager.storage.get_worktree_by_workspace_id.side_effect = record_read_raises

        with (
            patch("devlaunch.dl._get_cache_dir", return_value=cache),
            patch("devlaunch.dl.run_devpod", side_effect=devpod),
            patch("devlaunch.dl._get_clone_manager", return_value=manager),
            patch("devlaunch.dl.update_cache_background"),
        ):
            code = main(argv)
        return code, deleted

    def test_a_clean_workspace_is_deleted(self, tmp_path, clone):
        code, deleted = self._run(tmp_path, clone, ["r-feature-aaa", "rm"])
        assert code == 0
        assert deleted == ["r-feature-aaa"]

    def test_unpushed_work_stops_the_delete_and_says_how_to_insist(self, tmp_path, clone, caplog):
        (clone / "more.txt").write_text("more\n")
        commit(clone, "more")
        code, deleted = self._run(tmp_path, clone, ["r-feature-aaa", "rm"])
        assert code == 1
        assert deleted == []
        assert "unpushed" in caplog.text and "--force" in caplog.text

    def test_a_stale_record_does_not_let_the_delete_past_the_guard(self, tmp_path, remote, caplog):
        """devlaunch#174, at the surface it destroys things from.

        The guard read the recorded path; the delete fell back to the derived one
        when that path was not on disk. So a record pointing somewhere stale had
        the guard answering `NothingToLose` about an absent directory --
        correctly, nothing absent holds anything -- while the delete removed the
        derived directory, which held an unpushed commit. Exit 0, no `--force`,
        nothing logged.

        Reproduced before the fix, and this is the shape it was reproduced in:
        the guard and the delete must resolve the *same* directory, so the only
        assertion that pins it is one where those two would differ.
        """
        cache = tmp_path / "cache" / "devlaunch"
        derived = cache / "repos" / "blooop" / "r" / WorkspaceId("blooop", "r", "feature").value
        _make_clone(remote, derived)
        (derived / "more.txt").write_text("more\n")
        commit(derived, "more")

        stale = cache / "repos" / "blooop" / "r" / "moved-away"
        assert not stale.exists(), "the fixture's premise is that the record is stale"

        code, deleted = self._run(tmp_path, stale, ["r-feature-aaa", "rm"])

        assert code == 1, "the guard cleared a delete that destroys an unpushed commit"
        assert deleted == []
        assert derived.exists(), "the work the guard did not look at is gone"
        assert "unpushed" in caplog.text and "--force" in caplog.text

    def test_force_deletes_it_anyway(self, tmp_path, clone):
        (clone / "more.txt").write_text("more\n")
        commit(clone, "more")
        code, deleted = self._run(tmp_path, clone, ["r-feature-aaa", "rm", "--force"])
        assert code == 0
        assert deleted == ["r-feature-aaa"]

    def test_uncommitted_changes_stop_it_too(self, tmp_path, clone):
        (clone / "feature.txt").write_text("edited\n")
        code, deleted = self._run(tmp_path, clone, ["r-feature-aaa", "rm"])
        assert code == 1
        assert deleted == []

    def test_a_clone_git_cannot_read_stops_the_delete_too(
        self, tmp_path, broken_clone_under_ancestor, caplog
    ):
        """devlaunch#171, at the surface it destroys things from.

        This is the whole ticket: on the shipped code this returned 0 and
        deleted the workspace, because git had answered about the ancestor
        repository and the answer was "nothing to report". "Could not tell"
        must refuse exactly as "would lose" does -- and say `--force`, because
        a refusal a person cannot get past is a bug of its own.
        """
        code, deleted = self._run(tmp_path, broken_clone_under_ancestor, ["r-feature-aaa", "rm"])
        assert code == 1
        assert deleted == []
        assert "--force" in caplog.text
        assert str(broken_clone_under_ancestor) in caplog.text
        # The clone is still there, with the work still in it.
        assert (broken_clone_under_ancestor / "scratch.md").exists()

    def test_force_still_gets_past_a_refusal_it_cannot_explain(
        self, tmp_path, broken_clone_under_ancestor
    ):
        # The caller who means it is not blocked by dl declining to guess.
        code, deleted = self._run(
            tmp_path, broken_clone_under_ancestor, ["r-feature-aaa", "rm", "--force"]
        )
        assert code == 0
        assert deleted == ["r-feature-aaa"]

    def test_a_workspace_whose_record_cannot_be_read_is_not_deleted_either(
        self, tmp_path, clone, caplog
    ):
        """A record dl cannot read is not a record saying "nothing here".

        The same conflation one level up: `_unsaved_work_in` used to answer
        `None` both for "this is not dl's workspace, it has no clone to
        protect" and for "dl could not read its own metadata", and `None` meant
        delete freely.
        """
        code, deleted = self._run(
            tmp_path, clone, ["r-feature-aaa", "rm"], record_read_raises=OSError("disk gone")
        )
        assert code == 1
        assert deleted == []
        assert "--force" in caplog.text

    def test_an_answer_this_guard_does_not_name_stops_the_delete(self, tmp_path, clone):
        # The exhaustiveness check, at the surface that destroys things. A
        # fourth arm added later must arrive here as a crash, not as consent:
        # the `if unsaved:` this replaced would have deleted on anything falsey
        # and stopped on anything else, without either being a decision.
        with patch("devlaunch.dl._unsaved_work_in", return_value="looks empty to me"):
            with pytest.raises(AssertionError, match="Unhandled unsaved-work answer"):
                self._run(tmp_path, clone, ["r-feature-aaa", "rm"])

    def test_a_clone_dl_has_no_record_for_is_left_alone_rather_than_refused(
        self, tmp_path, clone_in_the_cache, caplog
    ):
        """What the guard does *not* cover, pinned so README cannot overstate it.

        A clone under dl's cache with no metadata record. `dl --ls --json`
        reports what it holds -- `wouldLose`, correctly -- but `rm` does not
        refuse: the guard reads the record, there is none, and "dl has no record
        of a clone here" is answered `NothingToLose`. So the workspace goes and
        the clone stays, exit 0, no `--force` asked for.

        No work is destroyed, which is why this is not a fail-open and not fixed
        here: the delete reads the same absent record and removes nothing
        (`worktree/workspace_clone.py`). But it is not a refusal either, and
        README said it was. What is asserted below is exactly what happens.
        """
        (clone_in_the_cache / "an-hour-of-work.md").write_text("half a plan\n")
        code, deleted = self._run(
            tmp_path, clone_in_the_cache, ["r-feature-aaa", "rm"], recorded=False
        )
        assert code == 0
        assert deleted == ["r-feature-aaa"]
        assert "--force" not in caplog.text
        # The work is still there, and now nothing dl records points at it.
        assert (clone_in_the_cache / "an-hour-of-work.md").read_text() == "half a plan\n"

    def test_a_recorded_clone_is_removed_with_the_workspace(self, tmp_path, clone):
        # The other half, so the assertion above is a fact about the record and
        # not about a fake that never removes anything.
        code, deleted = self._run(tmp_path, clone, ["r-feature-aaa", "rm"])
        assert code == 0 and deleted == ["r-feature-aaa"]
        assert not clone.exists()

    def test_a_clone_already_removed_by_hand_is_still_deleted(self, tmp_path, clone):
        # The reason the "not there" arm answers `NothingToLose` rather than
        # refusing: clearing up after a half-finished delete must not need
        # `--force`.
        subprocess.run(["rm", "-rf", str(clone)], check=True)
        code, deleted = self._run(tmp_path, clone, ["r-feature-aaa", "rm"])
        assert code == 0
        assert deleted == ["r-feature-aaa"]


class TestNamingWhatIsUnsaved:
    """A count cannot be judged; a name can.

    The case this exists for is real and permanent: this repo's own devcontainer
    runs `pixi install` in its postCreateCommand, which leaves the tracked
    `pixi.lock` modified in *every* workspace it builds. Reported as
    "1 uncommitted change(s)", an untouched clone is indistinguishable from an
    hour of someone's unsaved work, and a cleanup tool that believes the count
    never cleans anything.
    """

    def test_the_changed_paths_are_named(self, clone):
        (clone / "pixi.lock").write_text("churned by the container's own build\n")
        unsaved = holds_unsaved_work(clone)
        assert isinstance(unsaved, WouldLose) and "pixi.lock" in unsaved.description

    def test_a_modified_tracked_file_keeps_its_first_letter(self, clone):
        # The regression that took real use to find. `git status --porcelain`
        # writes a *modified* tracked file as " M path" -- leading space -- and
        # a full strip() of git's output ate it, so the path was reported one
        # character short ("ixi.lock"). Untracked files start "??" and were
        # unharmed, which is why every test here passed while the feature was
        # printing nonsense. Asserted on the exact rendering, not on a substring.
        (clone / "feature.txt").write_text("edited by the container's build\n")
        assert holds_unsaved_work(clone) == WouldLose("1 uncommitted change(s) (feature.txt)")

    def test_a_long_list_is_cut_short_rather_than_dumped(self, clone):
        for i in range(6):
            (clone / f"file{i}.txt").write_text("x\n")
        unsaved = holds_unsaved_work(clone)
        assert isinstance(unsaved, WouldLose) and "6 uncommitted change(s)" in unsaved.description
        # Three names and an ellipsis: enough to recognise, not a wall of text.
        assert unsaved.description.count(",") == 3 and "…" in unsaved.description

    def test_a_renamed_path_keeps_both_halves(self, clone):
        git("mv", "feature.txt", "renamed.txt", cwd=clone)
        unsaved = holds_unsaved_work(clone)
        assert isinstance(unsaved, WouldLose) and "renamed.txt" in unsaved.description
