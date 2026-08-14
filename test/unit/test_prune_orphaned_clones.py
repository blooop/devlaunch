# pylint: disable=redefined-outer-name
"""`dl --prune`: remove the clone directories no workspace references any more.

A workspace per branch means clone directories accumulate under the cache and
nothing ever removes them -- measured on the reference host, 52 clone directories
for 17 live devpod workspaces, 4.00 GB of them attached to nothing. `--purge` is
the wrong tool for that: it is all-or-nothing and takes the live clones and the
bare caches with it.

**These tests guard a deletion path, and the two ways of being wrong are not
equal.** A clone wrongly called referenced leaves garbage on disk; a clone
wrongly called orphaned destroys work that exists nowhere else. So every case
below that keeps a directory is the load-bearing one, and the fixture is built
so that a guard which stopped biting would show up as a *deletion* rather than
as a survivor.

The clones are real git repositories with a local bare standing in for GitHub,
which is PR #134's technique: a local path is a real git remote, so pushes,
fetches and remote-tracking refs behave exactly as they do over ssh with no
network, and the one bug that shipped in this area (an argument order that
reported every clone as safe to delete) was invisible to everything except a
real `git log`. Everything lives under the suite's scratch `XDG_CACHE_HOME`, so
`_get_cache_dir`, the metadata file and `repos_dir` all agree without being
patched into agreement -- a fixture whose clones sit outside the directory under
test is how a guard comes to run zero times.
"""

import json
import os
import shlex
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Callable, Dict, List, Optional
from unittest.mock import patch

import pytest

from devlaunch.dl import main
from devlaunch.worktree.models import WorktreeInfo
from devlaunch.worktree.storage import MetadataStorage
from devlaunch.xdg import devlaunch_cache

# root is refused by nothing, so under root a permission test would pass with
# the behaviour it guards fully reverted. Same reasoning as PR #136's.
needs_an_unprivileged_user = pytest.mark.skipif(
    os.geteuid() == 0, reason="root can empty any directory, so nothing here can refuse"
)


def git(*args: str, cwd: Path) -> str:
    """Run git for real, failing the test with git's own words if it refuses."""
    result = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=False)
    assert result.returncode == 0, f"git {' '.join(args)}: {result.stderr}"
    return result.stdout.strip()


def commit(work: Path, message: str) -> None:
    git("add", "-A", cwd=work)
    git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", message, cwd=work)


class World:
    """The cache `dl --prune` is pointed at, and the devpod that answers about it.

    One clone of each kind the classification has an arm for, plus the bare cache
    every one of them was made from.
    """

    def __init__(self, cache: Path):
        self.cache = cache
        self.repo_dir = cache / "repos" / "o" / "r"
        self.bare = self.repo_dir / ".bare"
        self.clones: Dict[str, Path] = {}
        self.listed: List[dict] = []
        self.records: Dict[str, WorktreeInfo] = {}

    @property
    def storage(self) -> MetadataStorage:
        return MetadataStorage(self.cache / "metadata.json")

    def record_branches(self) -> List[str]:
        """The branches metadata.json still holds a worktree record for."""
        return sorted(record.branch for record in self.storage.list_worktrees())


def _entry(workspace_id: str, source) -> dict:
    """One `devpod list --output json` element."""
    return {
        "id": workspace_id,
        "source": {"localFolder": str(source)},
        "provider": {"name": "docker"},
        "ide": {"name": "none"},
        "lastUsed": "2026-08-08T11:43:27Z",
    }


# Captured before anything is patched, because patching `devlaunch.dl.subprocess`
# patches the subprocess *module* -- `dl` imports it rather than a name out of
# it, so there is only one `subprocess.run` in the process and a stub put there
# answers every module's calls.
_real_run = subprocess.run


class RecordedDevpod:
    """Answers `devpod` from the listing it is asked for, and lets everything
    else run.

    The listing is a callable rather than a captured string, because the two
    passes of this command are meant to be able to see different worlds. A
    launch that finishes while the report is on screen registers a workspace,
    and the pass that acts has to be able to learn about it -- a fixture that
    answered both listings from one snapshot could not tell whether it did.

    Letting git through is not a convenience. The unsaved-work guard is a real
    `git status` and a real `git log --not --remotes`, and a stub that answered
    every command would hand it a clean exit with empty output -- which reads as
    "this clone holds nothing", which is the answer that deletes it. Written the
    other way round, this file's central guard passed while guarding nothing,
    and the clone with two unpushed commits in it was removed.
    """

    def __init__(self, listing: Callable[[], str]):
        self.listing = listing
        self.commands: List[List[str]] = []

    def __call__(self, cmd, *args, **kwargs):
        argv = list(cmd)
        if argv[:1] != ["devpod"]:
            # pylint: disable=subprocess-run-check  # the caller's own kwargs carry it
            return _real_run(cmd, *args, **kwargs)
        self.commands.append(argv)
        if argv[1:2] == ["list"]:
            return subprocess.CompletedProcess(argv, 0, self.listing(), "")
        return subprocess.CompletedProcess(argv, 0, "", "")

    @property
    def devpod_commands(self) -> List[List[str]]:
        """Every devpod invocation, without the leading `devpod`, in order."""
        return [argv[1:] for argv in self.commands]


@pytest.fixture
def world(tmp_path) -> World:
    """A cache holding one clone directory of every kind, all of them real repos.

    `referenced` is sourced by a live workspace. `orphan-clean` is sourced by
    nobody and holds nothing unpushed. `orphan-dirty` is sourced by nobody and
    holds both an unpushed commit and an uncommitted file -- 13 of the reference
    host's 37 stale clones were in that state, two of them with real work in
    them. `disputed` has a metadata record naming a workspace devpod still lists
    but sources somewhere else entirely, which is devlaunch#88's shape.
    """
    world = World(devlaunch_cache())
    world.repo_dir.mkdir(parents=True)

    origin = tmp_path / "origin.git"
    seed = tmp_path / "seed"
    git("init", "-b", "main", str(seed), cwd=tmp_path)
    (seed / "README.md").write_text("seed\n")
    commit(seed, "seed")
    git("clone", "--bare", str(seed), str(origin), cwd=tmp_path)
    git("clone", "--bare", str(origin), str(world.bare), cwd=tmp_path)

    storage = world.storage
    for leaf, branch in (
        ("referenced", "ref"),
        ("orphan-clean", "clean"),
        ("orphan-dirty", "dirty"),
        ("disputed", "disp"),
    ):
        clone = world.repo_dir / leaf
        # Cloned from the bare, exactly as dl does it, so the git objects are
        # hardlinked out of the bare and the reported sizes are the real
        # exclusive ones rather than a double count.
        git("clone", str(world.bare), str(clone), cwd=tmp_path)
        git("remote", "set-url", "origin", str(origin), cwd=clone)
        git("checkout", "-b", branch, cwd=clone)
        (clone / f"{branch}.txt").write_text("work\n")
        commit(clone, branch)
        git("push", "-u", "origin", branch, cwd=clone)
        world.clones[leaf] = clone
        record = WorktreeInfo(
            owner="o", repo="r", branch=branch, local_path=clone, workspace_id=leaf
        )
        storage.add_worktree(record)
        world.records[leaf] = record

    # Two megabytes of payload nothing else links to, so the reclaimed figure is
    # a number a test can name rather than whatever git happened to write.
    (world.clones["orphan-clean"] / "payload.bin").write_bytes(b"\0" * 2 * 1024 * 1024)
    git("update-index", "--skip-worktree", "--", "README.md", cwd=world.clones["orphan-clean"])
    (world.clones["orphan-clean"] / ".git" / "info" / "exclude").write_text("payload.bin\n")

    dirty = world.clones["orphan-dirty"]
    (dirty / "later.txt").write_text("later\n")
    commit(dirty, "later")  # committed, never pushed
    (dirty / "scratch.md").write_text("an agent's notes\n")  # never even added

    world.listed = [
        _entry("referenced", world.clones["referenced"]),
        _entry("disputed", tmp_path / "somewhere" / "else"),
    ]
    return world


def reported(out: str, path: Path) -> str:
    """The one report line that names *path*, or a failure saying it is absent.

    Every assertion in this file about a directory *surviving* goes through
    here rather than through `path.exists()`, because "it is still there" is
    true of a clone kept for the right reason and of one kept by a guard that
    was never asked. The line says which.
    """
    lines = [row for row in out.splitlines() if str(path) in row]
    assert len(lines) == 1, f"expected one report line for {path}, got {lines}"
    return lines[0]


def run_prune(world: World, *flags: str, answer: Optional[str] = None):
    """Run `dl --prune` against *world*, with devpod answering its listing."""
    devpod = RecordedDevpod(lambda: json.dumps(world.listed))
    with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
        if answer is None:
            code = main(["--prune", *flags])
        else:
            with patch("builtins.input", return_value=answer):
                code = main(["--prune", *flags])
    return code, devpod


class TestWhichCloneDirectoriesAreRemoved:
    """The three arms, decided from the disk and from devpod's own listing."""

    def test_a_clone_nothing_sources_is_removed(self, world, capsys):
        code, _devpod = run_prune(world, "-y")
        capsys.readouterr()
        assert code == 0
        assert not world.clones["orphan-clean"].exists()

    def test_a_clone_a_live_workspace_sources_is_kept(self, world, capsys):
        """The failure this one guards costs somebody a running workspace.

        Asserted on the *reason* rather than on the directory surviving. Written
        the other way round it passed with the Referenced arm deleted outright:
        every clone in this fixture also carries a metadata record, so one that
        stops being recognised as opened lands in Disputed and survives for the
        wrong reason -- a live workspace described to its owner as a records
        disagreement to go and fix.
        """
        run_prune(world, "-y")
        out = capsys.readouterr().out
        assert world.clones["referenced"].exists()
        assert reported(out, world.clones["referenced"]).endswith(
            "workspace referenced still opens it"
        )

    def test_a_clone_holding_work_saved_nowhere_else_is_kept(self, world, capsys):
        """Nothing sources it, and deleting it would still destroy an unpushed
        commit and an untracked file. 13 of the reference host's 37 stale clones
        were exactly this."""
        run_prune(world, "-y")
        capsys.readouterr()
        assert world.clones["orphan-dirty"].exists()

    def test_a_clone_devpod_sources_somewhere_else_is_kept(self, world, capsys):
        """devlaunch#88's shape: devpod still lists the workspace this directory
        was made for, and records it at a path that is not this one. That is a
        disagreement between two records, not evidence that nothing needs the
        clone, and the safe reading of a disagreement is to keep the disk."""
        run_prune(world, "-y")
        capsys.readouterr()
        assert world.clones["disputed"].exists()

    def test_the_bare_cache_is_never_a_candidate(self, world, capsys):
        """No workspace ever sources `.bare` and no record names it, so every
        rule above would call it an orphan. It is what makes the next clone of
        the repo fast, and all seven on the reference host came to 0.08 GB."""
        run_prune(world, "-y")
        out = capsys.readouterr().out
        assert world.bare.exists()
        assert ".bare" not in out


class TestEveryWayAWorkspaceCanNameADirectory:
    """A source arm this command does not read is a directory it will delete."""

    def test_a_workspace_devpod_records_as_a_git_source_still_holds_its_clone(self, world, capsys):
        """`devpod up <path-to-a-repo>` is recorded as a `gitRepository`, and
        nothing stops that repo living in the cache -- which is why the source
        here is a path inside it rather than a URL, the same shape the purge
        tests use. `--purge`'s predicate refuses this arm on purpose, and that
        answer must not be reused: refusing there means declining to delete
        somebody's *workspace*, and refusing here means deleting their clone.

        Asserted on the *reason*, not on the directory surviving. Written the
        other way round this passed with the arm deleted: a clone that stops
        being recognised as opened still carries a metadata record naming a
        workspace devpod lists, so it lands in Disputed and survives for the
        wrong reason -- a records disagreement reported where there is none, and
        a live clone described to its owner as something to go and fix.
        """
        world.listed[0] = {
            **_entry("referenced", "unused"),
            "source": {"gitRepository": str(world.clones["referenced"])},
        }
        code, _devpod = run_prune(world, answer="n")
        out = capsys.readouterr().out
        assert code == 0
        line = next(row for row in out.splitlines() if str(world.clones["referenced"]) in row)
        assert line.endswith("workspace referenced still opens it"), line

    def test_a_symlink_standing_where_a_clone_would_be_is_left_alone(self, world, capsys):
        """Following it would put this command outside the cache entirely, and
        unlinking it would report a clone as reclaimed while it sat on another
        volume. remove_tree refuses a symlinked root for exactly those two
        reasons; not making one a candidate is that refusal a step earlier.

        Asserted on it never reaching the report, because "the link is still
        there" is true either way and so is "the clone is still there". Followed,
        the link becomes a candidate, is weighed through to the real clone, is
        planned for removal and is then refused by remove_tree -- so both
        survive, and the only difference is a directory in the plan that was
        never devlaunch's to name and an exit code of 1 on a cache with nothing
        wrong with it. That is what this assertion had to be changed to catch.
        """
        elsewhere = world.repo_dir / "points-away"
        elsewhere.symlink_to(world.clones["referenced"])
        code, _devpod = run_prune(world, "-y")
        out = capsys.readouterr().out
        assert code == 0
        assert str(elsewhere) not in out
        assert elsewhere.is_symlink()
        assert world.clones["referenced"].exists()


class TestADevpodThatCouldNotBeRead:
    def test_a_listing_that_failed_is_not_read_as_a_machine_with_no_workspaces(self, world, capsys):
        """The worst thing devpod can do to this command is answer badly, and
        the worst answer is an empty one: every clone in the cache would be
        opened by nobody at once. So a listing that could not be read is not an
        answer, and `--prune` never gets as far as looking at a directory."""

        def broken(cmd, *args, **kwargs):
            argv = list(cmd)
            if argv[:1] != ["devpod"]:
                # pylint: disable=subprocess-run-check  # the caller's own kwargs carry it
                return _real_run(cmd, *args, **kwargs)
            return subprocess.CompletedProcess(argv, 1, "", "devpod: provider is not configured")

        with patch("devlaunch.dl.subprocess.run", side_effect=broken):
            code = main(["--prune", "-y"])
        assert code != 0
        assert all(clone.exists() for clone in world.clones.values())
        assert "Removing" not in capsys.readouterr().out


class TestAnEmptyCache:
    def test_a_machine_with_no_clone_directories_has_nothing_to_prune(self, capsys):
        """The first run on a fresh install, where `repos_dir` does not exist."""
        devpod = RecordedDevpod(lambda: "[]")
        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            code = main(["--prune", "-y"])
        assert code == 0
        assert "Nothing to prune." in capsys.readouterr().out


class TestWhatForcePromotesAndWhatItDoesNot:
    """`--force` is not a general override. It answers exactly one refusal."""

    def test_it_removes_the_clone_that_holds_unsaved_work(self, world, capsys):
        code, _devpod = run_prune(world, "-y", "--force")
        capsys.readouterr()
        assert code == 0
        assert not world.clones["orphan-dirty"].exists()

    def test_it_does_not_touch_the_clone_a_live_workspace_opens(self, world, capsys):
        """There is nothing for a user to mean by insisting past this one: the
        workspace is still there and still opens this directory.

        On the reason, for the same cause as its sibling above: under `--force`
        the unsaved guard is not what is holding this directory, but the
        Disputed arm still would be, and that is not what is being tested.
        """
        run_prune(world, "-y", "--force")
        out = capsys.readouterr().out
        assert world.clones["referenced"].exists()
        assert reported(out, world.clones["referenced"]).endswith(
            "workspace referenced still opens it"
        )

    def test_it_does_not_touch_the_clone_devpod_sources_elsewhere(self, world, capsys):
        """Disputed is never removable, `--force` included. It is not devlaunch
        refusing to act on a user's behalf, it is devlaunch's records and
        devpod's disagreeing -- and forcing a deletion is not an answer to a
        disagreement, it is a way of losing the argument permanently."""
        run_prune(world, "-y", "--force")
        capsys.readouterr()
        assert world.clones["disputed"].exists()

    def test_the_bare_cache_survives_force_too(self, world, capsys):
        run_prune(world, "-y", "--force")
        capsys.readouterr()
        assert world.bare.exists()


class TestTheReportComesBeforeTheQuestion:
    """Everything a person needs to answer `y` is on screen before they do."""

    def test_declining_removes_nothing(self, world, capsys):
        code, _devpod = run_prune(world, answer="n")
        capsys.readouterr()
        assert code == 0
        assert all(clone.exists() for clone in world.clones.values())

    def test_every_survivor_is_named_with_the_reason_it_survived(self, world, capsys):
        """Silence about a survivor is the surprise: a person who asked for a
        tidy-up and gets 4 GB back has no way to learn from `dl --ls` that a
        clone was left behind, because a clone with no workspace has no row
        there to appear in."""
        run_prune(world, answer="n")
        out = capsys.readouterr().out
        assert "workspace referenced still opens it" in out
        assert "unpushed commit(s)" in out and "--force" in out
        assert "devlaunch#88" in out
        assert str(world.clones["disputed"]) in out

    def test_the_plan_is_printed_before_the_question_is_asked(self, world, capsys):
        """`--purge`'s shape: the report is what the question is about, so it
        cannot arrive after the answer."""
        seen = {}

        def answering(_prompt=""):
            seen["out"] = capsys.readouterr().out
            return "n"

        devpod = RecordedDevpod(lambda: json.dumps(world.listed))
        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with patch("builtins.input", side_effect=answering):
                assert main(["--prune"]) == 0
        assert str(world.clones["orphan-clean"]) in seen["out"]

    def test_work_written_while_the_question_was_open_is_not_destroyed(self, world, capsys):
        """The plan was made before the user answered it, and a container
        writing into a clone in between is the ordinary case here rather than
        the exotic one. So each directory is asked once more what it holds,
        under the repository lock, immediately before it goes -- the approved
        set may shrink and may never grow."""

        def answering(_prompt=""):
            (world.clones["orphan-clean"] / "notes.md").write_text("written while deciding\n")
            return "y"

        devpod = RecordedDevpod(lambda: json.dumps(world.listed))
        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with patch("builtins.input", side_effect=answering):
                code = main(["--prune"])
        out = capsys.readouterr().out
        assert code == 0
        assert world.clones["orphan-clean"].exists()
        assert "notes.md" in out

    def test_an_option_it_does_not_know_is_refused_rather_than_ignored(self, world, capsys):
        """`dl --prune --dry-run -y` reads as a rehearsal. Ignoring the flag it
        does not know would make it a deletion instead, and the exit code would
        be 0."""
        code, _devpod = run_prune(world, "--dry-run", "-y")
        capsys.readouterr()
        assert code == 1
        assert all(clone.exists() for clone in world.clones.values())


class TestRunningItAgain:
    def test_a_second_run_finds_nothing_left_to_do(self, world, capsys):
        assert run_prune(world, "-y")[0] == 0
        capsys.readouterr()
        code, _devpod = run_prune(world, "-y")
        out = capsys.readouterr().out
        assert code == 0
        assert "Nothing to prune." in out


class TestTheRecordsFollowTheDirectories:
    """metadata.json is append-mostly, and this is where that stops."""

    def test_the_removed_clone_loses_its_record_and_the_others_keep_theirs(self, world, capsys):
        assert world.record_branches() == ["clean", "dirty", "disp", "ref"]
        run_prune(world, "-y")
        capsys.readouterr()
        assert world.record_branches() == ["dirty", "disp", "ref"]

    def test_a_record_for_a_directory_that_is_already_gone_is_dropped(self, world, capsys):
        """49 records for 17 live workspaces on the reference host. These ones
        describe nothing at all, and nothing else was ever going to drop them."""
        world.storage.add_worktree(
            WorktreeInfo(
                owner="o",
                repo="r",
                branch="long-gone",
                local_path=world.repo_dir / "long-gone",
                workspace_id="long-gone",
            )
        )
        assert "long-gone" in world.record_branches()
        run_prune(world, "-y")
        capsys.readouterr()
        assert "long-gone" not in world.record_branches()

    @needs_an_unprivileged_user
    def test_a_record_whose_directory_cannot_be_looked_at_is_kept(self, world, capsys):
        """ "Gone" is only ever FileNotFoundError. A directory behind a door this
        process cannot open is still a directory, and dropping its record would
        throw away the only note of where a clone lives -- on a machine where
        the container ran as another user, which is the machine this whole
        command is for."""
        sealed = world.cache / "sealed"
        (sealed / "clone").mkdir(parents=True)
        world.storage.add_worktree(
            WorktreeInfo(
                owner="o",
                repo="r",
                branch="behind-a-door",
                local_path=sealed / "clone",
                workspace_id="behind-a-door",
            )
        )
        sealed.chmod(0o000)
        try:
            run_prune(world, "-y")
            capsys.readouterr()
            assert "behind-a-door" in world.record_branches()
        finally:
            sealed.chmod(0o700)


class TestACacheReachedThroughASymlink:
    """The total-loss regression, and the reason it gets a test of its own."""

    def test_a_workspace_that_names_its_clone_through_a_symlink_still_holds_it(
        self, world, tmp_path, capsys
    ):
        """devpod records whatever path it was handed. Somebody who moved their
        cache and left a symlink behind -- or whose `/tmp` is one -- has records
        naming a path that is not the canonical one, and a comparison that
        stopped resolving both sides would find that *no* clone is referenced
        and delete every one of them. There is no undo for that.

        Asserted on the reason, which is the whole difference between this test
        and the one it replaced. "The clone is still there" is true with the
        resolve taken off too: unresolved, the source matches nothing, the
        directory falls through to its own metadata record, and the record names
        a workspace devpod lists -- so it survives on the Disputed arm, reported
        to its owner as a records disagreement, and the test that named this
        regression passed anyway.
        """
        through_a_link = tmp_path / "link-to-cache"
        through_a_link.symlink_to(world.cache)
        world.listed[0] = _entry("referenced", through_a_link / "repos" / "o" / "r" / "referenced")
        code, _devpod = run_prune(world, "-y")
        out = capsys.readouterr().out
        assert code == 0
        assert reported(out, world.clones["referenced"]).endswith(
            "workspace referenced still opens it"
        )
        # And the run was a real one, not a scan that found nothing to do.
        assert not world.clones["orphan-clean"].exists()


class TestWhenAWorkspaceCannotBeLocated:
    def test_a_source_that_cannot_be_read_as_a_path_stops_the_whole_command(self, world, capsys):
        """A live workspace whose source will not resolve could be opening any
        of the candidates. While one exists there is no directory this command
        can honestly call unreferenced, so it says so and removes nothing --
        rather than quietly dropping that workspace out of the comparison,
        which is a deletion decided by a lookup that failed.

        The source here is text devpod's JSON can carry and no filesystem call
        will accept. A source that merely does not *exist* deliberately does not
        land here -- that is most of the workspaces on devlaunch#88's host, and
        stopping on it would make this command unrunnable exactly where it is
        most needed.
        """
        world.listed.append(_entry("unreadable-source", "/tmp/not\0a/path"))
        code, _devpod = run_prune(world, "-y")
        out = capsys.readouterr().out
        assert code == 1
        assert "unreadable-source" in out
        assert all(clone.exists() for clone in world.clones.values())

    def test_a_source_that_is_simply_gone_does_not(self, world, capsys):
        """The other half, and the reason the two are told apart at all."""
        world.listed.append(_entry("source-deleted", world.cache / "went" / "away"))
        code, _devpod = run_prune(world, "-y")
        capsys.readouterr()
        assert code == 0
        assert not world.clones["orphan-clean"].exists()


class TestWhenADirectoryWillNotComeAway:
    """A container writes into its clone as its own user, so this is ordinary."""

    @staticmethod
    def _seal(world: World) -> Path:
        """A second orphan with a door in it this process cannot open."""
        leftover = world.repo_dir / "leftover"
        (leftover / "locked").mkdir(parents=True)
        (leftover / "locked" / "payload.bin").write_bytes(b"\0" * 512 * 1024)
        (leftover / "locked").chmod(0o000)
        return leftover

    @needs_an_unprivileged_user
    def test_what_could_not_be_measured_is_reported_as_a_floor_not_a_total(self, world, capsys):
        """The one number this command must never print is a floor with the
        `≥` taken off. It is a cleanup tool telling somebody a directory is
        small when it is not -- and the total across the plan is where that
        would happen, because a sum of integers has lost which kind of answer
        each of them was."""
        leftover = self._seal(world)
        try:
            run_prune(world, answer="n")
            out = capsys.readouterr().out
        finally:
            (leftover / "locked").chmod(0o700)
        line = next(row for row in out.splitlines() if str(leftover) in row)
        assert "≥" in line, line
        headline = next(row for row in out.splitlines() if row.startswith("Removing "))
        assert "≥" in headline, headline

    @needs_an_unprivileged_user
    def test_a_directory_that_refuses_is_named_and_its_siblings_still_go(self, world, capsys):
        """PR #136's contract, unchanged: what would not come away is named
        with what the system said about it, everything else still goes, and the
        exit code says the job is unfinished rather than untried."""
        leftover = self._seal(world)
        try:
            code, _devpod = run_prune(world, "-y")
            out = capsys.readouterr().out
        finally:
            (leftover / "locked").chmod(0o700)
        assert code == 1
        assert f"{leftover / 'locked'}:" in out
        assert not world.clones["orphan-clean"].exists()
        assert (leftover / "locked").exists()

    @needs_an_unprivileged_user
    def test_the_command_to_paste_names_the_obstruction_and_nothing_else(self, world, capsys):
        """The one line here a person runs as root, so it is the one line that
        must not be widened by accident. Aimed at `plan.root`: pointed there
        instead of at the refusals, this prints an instruction to `rm -rf` every
        clone in the cache, live ones included, and every other assertion in
        this file still holds."""
        leftover = self._seal(world)
        try:
            run_prune(world, "-y")
            out = capsys.readouterr().out
        finally:
            (leftover / "locked").chmod(0o700)
        advice = next(row for row in out.splitlines() if "sudo rm -rf" in row)
        assert shlex.split(advice)[3:] == [str(leftover / "locked")], advice


_LOCK_HOLDER = """
import fcntl, os, sys, time
from pathlib import Path

lock_path, held, release = sys.argv[1:4]
Path(lock_path).parent.mkdir(parents=True, exist_ok=True)
fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
fcntl.flock(fd, fcntl.LOCK_EX)
Path(held).touch()
while not Path(release).exists():
    time.sleep(0.01)
os.close(fd)
"""


class TestItWaitsForALaunchThatIsStillCloning:
    """The window a clone is born in, and why `--prune` may not look into it."""

    def test_it_blocks_while_another_process_holds_the_repository_lock(
        self, world, tmp_path, capsys
    ):
        """A cold launch fills a clone directory completely before it returns,
        under this exact lock. A scan that walked past it would weigh -- and
        then delete -- a directory `git clone` was still writing into, and the
        launch would fail with an error about a repository that was there a
        moment ago.

        Deterministic, in the shape test_concurrent_launches.py already uses: a
        separate process takes the lock and sits on it, and the prune must not
        get past the scan until it lets go. Unserialized, a scan of four small
        clones is finished in well under the grace period.
        """
        script = tmp_path / "holder.py"
        script.write_text(_LOCK_HOLDER)
        held, release = tmp_path / "held", tmp_path / "release"
        lock_path = world.repo_dir / ".lock"
        # Not a `with`: the holder has to outlive the block that starts it and be
        # let go from the `finally` below, which is what makes the release
        # deterministic rather than tied to a scope exit.
        holder = subprocess.Popen(  # pylint: disable=consider-using-with
            [sys.executable, str(script), str(lock_path), str(held), str(release)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            deadline = time.monotonic() + 60
            while not held.exists():
                assert time.monotonic() < deadline, "the lock holder never started"
                time.sleep(0.01)

            done = threading.Event()

            def prune():
                try:
                    run_prune(world, "-y")
                finally:
                    done.set()

            worker = threading.Thread(target=prune, daemon=True)
            worker.start()
            still_blocked = not done.wait(timeout=3)
            still_there = world.clones["orphan-clean"].exists()
            release.touch()
            worker.join(timeout=60)
            capsys.readouterr()
            assert still_blocked, "the scan ran while another process held the repository lock"
            assert still_there
            assert not world.clones["orphan-clean"].exists()
        finally:
            release.touch()
            holder.communicate(timeout=60)


class TestADevpodRecordThatPointsAtNothing:
    """devlaunch#88's measured shape, and the reason `--prune` may ship ahead of it.

    On that ticket's host, **36 of 39** devpod workspaces recorded a source
    folder that was missing (35) or a config-only stub devpod itself rebuilt
    from cache (1), while the real checkout sat beside it under the new id
    scheme. The two sides cannot be joined by workspace id -- the id scheme
    change is what broke them apart -- so the join is made from the path devpod
    still records, which names the repository exactly.

    Without this, `--prune` on such a host keeps the stub, because a live
    workspace really does open it, and deletes the live workspace's only real
    checkout, because nothing sources it and its record names an id devpod no
    longer lists. That is the total loss this whole command is written to avoid,
    on the host class the decision to build it cited.
    """

    @staticmethod
    def _stub(world: World, leaf: str) -> Path:
        """The config-only folder devpod reconstitutes from `workspace_result.json`.

        A real directory with a `devcontainer.json` in it and no `.git` -- one
        file, which is what devlaunch#88 measured.
        """
        stub = world.repo_dir / leaf
        (stub / ".devcontainer").mkdir(parents=True)
        (stub / ".devcontainer" / "devcontainer.json").write_text('{"name": "r"}\n')
        return stub

    def test_a_workspace_recorded_at_a_stub_disputes_that_repositorys_clones(self, world, capsys):
        """The one of #88's 36 whose recorded folder still exists. devpod opens
        it, so it survives on its own account -- and the real checkout beside it
        must not be read as unreferenced just because the workspace id moved."""
        stub = self._stub(world, "r-main-oldscheme")
        world.listed[0] = _entry("r-main", stub)
        code, _devpod = run_prune(world, "-y", "--force")
        out = capsys.readouterr().out
        assert code == 0
        assert world.clones["referenced"].exists()
        assert "devlaunch#88" in reported(out, world.clones["referenced"])
        assert stub.exists()

    def test_a_workspace_recorded_at_a_folder_that_is_gone_disputes_them_too(self, world, capsys):
        """35 of #88's 36. The recorded folder is not there at all, so there is
        nothing to open and nothing to match -- and which of this repository's
        clones the workspace needs is exactly the question #88 exists to
        answer."""
        world.listed[0] = _entry("r-main", world.repo_dir / "main")
        code, _devpod = run_prune(world, "-y", "--force")
        out = capsys.readouterr().out
        assert code == 0
        assert world.clones["referenced"].exists()
        assert world.clones["orphan-clean"].exists()
        assert "devlaunch#88" in reported(out, world.clones["orphan-clean"])

    def test_a_workspace_recorded_at_the_repository_directory_itself_disputes_it(
        self, world, capsys
    ):
        """The shallowest way a record can land in a repository's tree and name
        no clone of it. It is still a live workspace somewhere in there, and
        which clone it wants is still unanswerable -- read as "not in the tree"
        instead, every clone of the repository becomes prunable while the
        workspace is open."""
        world.listed[0] = _entry("r-main", world.repo_dir)
        code, _devpod = run_prune(world, "-y", "--force")
        out = capsys.readouterr().out
        assert code == 0
        assert world.clones["orphan-clean"].exists()
        assert "devlaunch#88" in reported(out, world.clones["orphan-clean"])

    def test_only_that_repositorys_clones_are_disputed(self, world, capsys):
        """Scoped to the repository whose tree the broken record points into,
        which is what keeps this command useful on #88's host rather than merely
        safe on it: seven repositories were affected there and the rest of the
        cache is still somebody's dead disk."""
        elsewhere = world.cache / "repos" / "other" / "project" / "stale"
        elsewhere.mkdir(parents=True)
        (elsewhere / "payload.bin").write_bytes(b"\0" * 1024)
        world.listed[0] = _entry("r-main", world.repo_dir / "main")
        code, _devpod = run_prune(world, "-y")
        capsys.readouterr()
        assert code == 0
        assert not elsewhere.exists()
        assert world.clones["referenced"].exists()


class TestAWorkspaceThatAppearedWhileTheQuestionWasOpen:
    """The plan is a photograph; the act happens later, and the world moved.

    A cold launch fills its clone under the repository lock and only registers a
    devpod workspace after releasing it, so a launch that was mid-clone when the
    report printed can be a *live workspace* by the time the user says yes -- and
    the clone path for `(owner, repo, branch)` is deterministic, so it is one of
    the directories in the plan, not a new one.
    """

    def test_a_clone_a_launch_registered_since_the_plan_is_not_removed(self, world, capsys):
        def answering(_prompt=""):
            world.listed.append(_entry("o-r-clean", world.clones["orphan-clean"]))
            return "y"

        devpod = RecordedDevpod(lambda: json.dumps(world.listed))
        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with patch("builtins.input", side_effect=answering):
                code = main(["--prune"])
        out = capsys.readouterr().out
        assert code == 0
        assert world.clones["orphan-clean"].exists()
        assert "o-r-clean" in out

    def test_it_is_not_removed_under_force_either(self, world, capsys):
        """`--force` promotes unsaved work on an orphan. A directory a live
        workspace opens is not that arm at all, on either pass."""

        def answering(_prompt=""):
            world.listed.append(_entry("o-r-clean", world.clones["orphan-clean"]))
            return "y"

        devpod = RecordedDevpod(lambda: json.dumps(world.listed))
        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with patch("builtins.input", side_effect=answering):
                code = main(["--prune", "--force"])
        capsys.readouterr()
        assert code == 0
        assert world.clones["orphan-clean"].exists()


class TestAWorkspaceThatOpensPartOfAClone:
    def test_a_live_workspace_opening_a_subdirectory_still_holds_the_clone(self, world, capsys):
        """`devpod up <clone>/subproject` is an ordinary thing to do with a
        monorepo, and it records the subdirectory. Compared by equality the
        clone itself is opened by nobody -- and deleting it takes the running
        workspace's checkout with it.

        Run under `--force` and with the subdirectory committed and pushed, so
        the unsaved-work guard has nothing to say and cannot be what saves it.
        """
        clone = world.clones["referenced"]
        sub = clone / "subproject"
        sub.mkdir()
        (sub / "app.py").write_text("print('hi')\n")
        commit(clone, "subproject")
        git("push", cwd=clone)
        world.listed[0] = _entry("referenced", sub)
        code, _devpod = run_prune(world, "-y", "--force")
        out = capsys.readouterr().out
        assert code == 0
        assert clone.exists()
        assert reported(out, clone).endswith("workspace referenced still opens it")


class TestASourceThatNamesAFolderItCannotRead:
    """`UnrecognisedSource` used to answer two opposite questions with one arm."""

    def test_a_local_folder_devlaunch_cannot_read_stops_the_whole_command(self, world, capsys):
        """devpod says this workspace opens a directory here. While devlaunch
        cannot say which, no clone is unreferenced -- and the command already
        prints exactly that for a source that will not resolve. It used to print
        nothing and carry on, because this arm contributed no path *and* no
        alarm."""
        world.listed.append(
            {**_entry("folder-i-cannot-read", "unused"), "source": {"localFolder": {"a": "b"}}}
        )
        code, _devpod = run_prune(world, "-y")
        out = capsys.readouterr().out
        assert code == 1
        assert "folder-i-cannot-read" in out
        assert all(clone.exists() for clone in world.clones.values())

    def test_one_that_appeared_since_the_plan_stops_the_removal_too(self, world, capsys):
        """Both passes have to be able to stop, and for the same reason. A
        workspace devlaunch cannot place could be opening any of the directories
        the user just approved -- the fact that it was placeable when the report
        was printed is not an answer about the world the removal happens in."""

        def answering(_prompt=""):
            world.listed.append(
                {
                    **_entry("folder-i-cannot-read", "unused"),
                    "source": {"localFolder": {"a": "b"}},
                }
            )
            return "y"

        devpod = RecordedDevpod(lambda: json.dumps(world.listed))
        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with patch("builtins.input", side_effect=answering):
                code = main(["--prune"])
        out = capsys.readouterr().out
        assert code == 1
        assert "folder-i-cannot-read" in out
        assert all(clone.exists() for clone in world.clones.values())

    def test_a_workspace_that_opens_no_folder_here_does_not(self, world, capsys):
        """The other half, and the reason the arm had to be split rather than
        made to alarm. `devpod up ubuntu:24.04` mounts no directory on this
        machine, so no clone can be at risk from it -- and stopping on one would
        make `--prune` unrunnable for anybody who has ever run devpod by hand."""
        world.listed.append(
            {**_entry("an-image", "unused"), "source": {"container": "abc123def456"}}
        )
        code, _devpod = run_prune(world, "-y")
        capsys.readouterr()
        assert code == 0
        assert not world.clones["orphan-clean"].exists()


class TestACloneGitWillNotAnswerAbout:
    """ "Could not ask" is not "nothing to lose", and this is where it costs.

    Clones are precisely the directories a container wrote as uid 1000, so a
    repository this process cannot read is the normal case on the machine
    devlaunch exists for -- not the exotic one. Read as clean, such a clone is
    removed with nothing typed.
    """

    @staticmethod
    def _unreadable(clone: Path) -> None:
        (clone / ".git" / "HEAD").unlink()

    def test_a_clone_whose_repository_will_not_open_is_kept_with_nothing_typed(self, world, capsys):
        clone = world.clones["orphan-clean"]
        (clone / "scratch.md").write_text("an agent's notes\n")
        self._unreadable(clone)
        code, _devpod = run_prune(world, "-y")
        out = capsys.readouterr().out
        assert code == 0
        assert clone.exists()
        assert (clone / "scratch.md").exists()
        line = reported(out, clone)
        assert "could not" in line and "--force" in line, line

    def test_force_is_what_removes_it(self, world, capsys):
        """Kept is not unremovable. A clone whose git is broken is exactly what
        somebody runs this command to clear away, and `--force` is the sentence
        that says they meant it."""
        clone = world.clones["orphan-clean"]
        self._unreadable(clone)
        code, _devpod = run_prune(world, "-y", "--force")
        capsys.readouterr()
        assert code == 0
        assert not clone.exists()

    def test_a_directory_that_was_never_a_repository_is_still_removed(self, world, capsys):
        """The distinction the arm rests on. Left-behind notes under the cache
        really do hold nothing git could lose; a repository that will not open
        does not, and the two must not be told apart by asking git, which
        refuses identically for both."""
        stray = world.repo_dir / "notes-i-left-here"
        stray.mkdir()
        (stray / "plan.md").write_text("half a plan\n")
        code, _devpod = run_prune(world, "-y")
        capsys.readouterr()
        assert code == 0
        assert not stray.exists()


class TestWhatForceIsAnsweringAndForWhich:
    """`--force` answers one directory's objection, not the whole plan's."""

    def test_the_plan_names_what_forcing_this_one_destroys(self, world, capsys):
        """Under `--force` the dirty clone used to read exactly like the empty
        one -- a path and a size -- so the confirmation could not say what it
        cost, and there is no later chance to."""
        run_prune(world, "--force", answer="n")
        out = capsys.readouterr().out
        dirty = reported(out, world.clones["orphan-dirty"])
        assert "unpushed commit(s)" in dirty and "removing anyway" in dirty, dirty
        assert "removing anyway" not in reported(out, world.clones["orphan-clean"])

    def test_it_does_not_turn_off_the_re_probe_for_a_clone_it_promoted_nothing_about(
        self, world, capsys
    ):
        """Read from the plan as one boolean, `--force` skipped the pre-removal
        re-check for every directory in the plan -- including the ones it had
        promoted nothing about. Work written into a clean orphan while the user
        was reading the report was then destroyed."""

        def answering(_prompt=""):
            (world.clones["orphan-clean"] / "notes.md").write_text("written while deciding\n")
            return "y"

        devpod = RecordedDevpod(lambda: json.dumps(world.listed))
        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with patch("builtins.input", side_effect=answering):
                code = main(["--prune", "--force"])
        out = capsys.readouterr().out
        assert code == 0
        assert world.clones["orphan-clean"].exists()
        assert "notes.md" in out
        # And the one it *did* promote still goes: this is not --force undone.
        assert not world.clones["orphan-dirty"].exists()


class TestEveryPathThisCommandComparesIsCanonical:
    """Three more places a symlink decides what gets deleted.

    The workspace source has a test of its own above. These are the other side
    of each comparison, and all three survived mutation under the full suite
    while the command was correct in every one of them -- correct and unheld.
    """

    def test_a_repos_dir_reached_through_a_symlink_still_matches_its_clones(self, world, capsys):
        """`config.toml` may point `repos_dir` anywhere, including through a
        link. Unresolved, every candidate path is the link's and every workspace
        source is the target's, so no clone is referenced and the bare cache is
        not recognised either -- the whole repository goes."""
        link = world.cache / "repos-through-a-link"
        link.symlink_to(world.cache / "repos")
        config = Path(os.environ["XDG_CONFIG_HOME"]) / "devlaunch" / "config.toml"
        config.parent.mkdir(parents=True, exist_ok=True)
        config.write_text(f'[worktree]\nrepos_dir = "{link}"\n')
        code, _devpod = run_prune(world, "-y")
        out = capsys.readouterr().out
        assert code == 0
        assert reported(out, world.clones["referenced"]).endswith(
            "workspace referenced still opens it"
        )
        assert world.bare.exists()
        assert ".bare" not in out
        # A real run rather than a scan that found nothing to look at.
        assert not world.clones["orphan-clean"].exists()

    def test_a_record_written_through_a_symlink_still_disputes_its_clone(
        self, world, tmp_path, capsys
    ):
        """The metadata side. devlaunch writes `local_path` from whatever path
        it was handed, so a cache reached through a link puts a non-canonical
        path in the record -- and a record that does not match its directory
        drops that clone from Disputed to Orphaned, which is a deletion.

        Under `--force`, so the unsaved-work guard cannot be what holds it.
        """
        through_a_link = tmp_path / "link-to-cache"
        through_a_link.symlink_to(world.cache)
        storage = world.storage
        storage.remove_worktree("o", "r", "disp")
        storage.add_worktree(
            WorktreeInfo(
                owner="o",
                repo="r",
                branch="disp",
                local_path=through_a_link / "repos" / "o" / "r" / "disputed",
                workspace_id="disputed",
            )
        )
        code, _devpod = run_prune(world, "-y", "--force")
        out = capsys.readouterr().out
        assert code == 0
        assert world.clones["disputed"].exists()
        assert "devlaunch#88" in reported(out, world.clones["disputed"])


class TestARunWhoseOnlyWorkIsTheRecords:
    def test_a_cache_with_only_a_stale_record_left_is_not_nothing_to_prune(self, world, capsys):
        """metadata.json is the other half of what this command tidies, and a
        run that would drop a record and say "Nothing to prune." does nothing
        and reports that it was right to.

        The cache here has no removable *directory* at all -- the clean orphan
        was cleared away by hand, which is how its record came to describe
        nothing -- so the record is the whole of the work.
        """
        shutil.rmtree(world.clones["orphan-clean"])
        assert "clean" in world.record_branches()
        code, _devpod = run_prune(world, "-y")
        out = capsys.readouterr().out
        assert code == 0
        assert "Nothing to prune." not in out
        assert "clean" not in world.record_branches()


class TestTheLockIsHeldWhileItRemoves:
    """The scan takes the repository lock. So must the pass that deletes.

    The concurrency test above proves the *plan* pass waits. The window that
    actually costs a clone is later: a launch that starts while the user is
    reading the report holds this lock while it fills a directory, and a removal
    that walked past it deletes what `git clone` is writing into.
    """

    def test_it_blocks_after_the_answer_while_another_process_holds_the_lock(
        self, world, tmp_path, capsys
    ):
        script = tmp_path / "holder.py"
        script.write_text(_LOCK_HOLDER)
        held, release = tmp_path / "held", tmp_path / "release"
        lock_path = world.repo_dir / ".lock"
        answered = threading.Event()
        may_answer = threading.Event()
        done = threading.Event()

        def answering(_prompt=""):
            answered.set()
            assert may_answer.wait(timeout=60), "the lock holder never took the lock"
            return "y"

        def prune():
            try:
                devpod = RecordedDevpod(lambda: json.dumps(world.listed))
                with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
                    with patch("builtins.input", side_effect=answering):
                        main(["--prune"])
            finally:
                done.set()

        worker = threading.Thread(target=prune, daemon=True)
        worker.start()
        holder = None
        try:
            assert answered.wait(timeout=60), "the plan was never printed"
            # Started only now: the plan pass holds this same lock, so a holder
            # taken earlier would prove nothing about the pass that removes.
            holder = subprocess.Popen(  # pylint: disable=consider-using-with
                [sys.executable, str(script), str(lock_path), str(held), str(release)],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            deadline = time.monotonic() + 60
            while not held.exists():
                assert time.monotonic() < deadline, "the lock holder never started"
                time.sleep(0.01)
            may_answer.set()
            still_blocked = not done.wait(timeout=3)
            still_there = world.clones["orphan-clean"].exists()
            release.touch()
            worker.join(timeout=60)
            capsys.readouterr()
            assert still_blocked, "it removed a clone while another process held the lock"
            assert still_there
            assert not world.clones["orphan-clean"].exists()
        finally:
            release.touch()
            may_answer.set()
            if holder is not None:
                holder.communicate(timeout=60)


class TestACloneWhoseGitIsAFile:
    """A linked git worktree keeps `.git` as a *file*, not a directory.

    devlaunch clones today, so every clone in this cache has a `.git` directory
    -- which is exactly why this needs saying out loud. Both places this command
    asks "is there a repository at this path" would answer no for a worktree,
    and both wrong answers are deletions: the checkout a live workspace opens
    stops counting as a clone and its whole repository is disputed instead, and
    an orphan git could perfectly well have been asked about is read as holding
    nothing and removed.
    """

    @staticmethod
    def _worktree(world: World, leaf: str, branch: str) -> Path:
        path = world.repo_dir / leaf
        git("worktree", "add", "-b", branch, str(path), cwd=world.bare)
        assert (path / ".git").is_file(), "a linked worktree's .git is a file"
        return path

    def test_a_live_workspace_opening_one_still_holds_it(self, world, capsys):
        worktree = self._worktree(world, "worktree-referenced", "wt-ref")
        world.listed[0] = _entry("wt-ref", worktree)
        code, _devpod = run_prune(world, "-y", "--force")
        out = capsys.readouterr().out
        assert code == 0
        assert worktree.exists()
        assert reported(out, worktree).endswith("workspace wt-ref still opens it")

    def test_an_orphaned_one_holding_work_is_still_kept(self, world, capsys):
        worktree = self._worktree(world, "worktree-orphan", "wt-orphan")
        (worktree / "scratch.md").write_text("an agent's notes\n")
        code, _devpod = run_prune(world, "-y")
        out = capsys.readouterr().out
        assert code == 0
        assert worktree.exists()
        assert "uncommitted" in reported(out, worktree)
