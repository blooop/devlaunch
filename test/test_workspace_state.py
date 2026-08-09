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
prove this file agrees with itself, and the one bug these tests caught before
release (an argument order that reported every clone as safe to delete) was
invisible to everything except a real `git log`.
"""

import json
import os
import subprocess
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from devlaunch.dl import main
from devlaunch.workspace_state import CloneState, holds_unsaved_work, read_clone


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


@pytest.fixture
def clone(remote: Path, tmp_path: Path) -> Path:
    """A workspace clone on a pushed branch, as `dl` would leave one."""
    work = tmp_path / "ws"
    git("clone", str(remote), str(work), cwd=tmp_path)
    git("checkout", "-b", "feature", cwd=work)
    (work / "feature.txt").write_text("work\n")
    commit(work, "feature")
    git("push", "-u", "origin", "feature", cwd=work)
    return work


class TestWhatACloneHolds:
    def test_a_pushed_branch_with_a_clean_tree_holds_nothing_unsaved(self, clone):
        assert read_clone(clone) == CloneState(branch="feature", unsaved=None)

    def test_an_unpushed_commit_is_unsaved(self, clone):
        (clone / "more.txt").write_text("more\n")
        commit(clone, "more")
        unsaved = holds_unsaved_work(clone)
        assert unsaved and "1 unpushed commit(s)" in unsaved

    def test_uncommitted_changes_are_unsaved(self, clone):
        (clone / "feature.txt").write_text("edited\n")
        unsaved = holds_unsaved_work(clone)
        assert unsaved and "1 uncommitted change(s)" in unsaved

    def test_untracked_files_are_unsaved_too(self, clone):
        # An agent's scratch notes are not less lost for never having been added.
        (clone / "notes.md").write_text("half a plan\n")
        unsaved = holds_unsaved_work(clone)
        assert unsaved and "uncommitted" in unsaved

    def test_both_kinds_of_loss_are_reported_together(self, clone):
        (clone / "more.txt").write_text("more\n")
        commit(clone, "more")
        (clone / "dirty.txt").write_text("dirty\n")
        unsaved = holds_unsaved_work(clone)
        assert unsaved and "uncommitted" in unsaved and "unpushed" in unsaved

    def test_a_branch_whose_commits_are_on_the_remote_under_another_name_is_saved(
        self, clone, remote
    ):
        # Pushed under a second name: the commits exist elsewhere, so nothing
        # would be lost. Asking about *any* remote ref rather than this branch's
        # upstream is what gets this right.
        git("push", "origin", "feature:review/feature", cwd=clone)
        git("branch", "-m", "feature", "renamed", cwd=clone)
        assert holds_unsaved_work(clone) is None

    def test_a_clone_that_is_not_there_holds_nothing(self, tmp_path):
        # A half-finished delete, or a directory removed by hand. There is no
        # work in it to lose, and nothing here may crash on it.
        assert read_clone(tmp_path / "absent") == CloneState(branch=None, unsaved=None)

    def test_a_directory_that_is_not_a_repository_holds_nothing(self, tmp_path):
        plain = tmp_path / "plain"
        plain.mkdir()
        (plain / "file.txt").write_text("not a repo\n")
        assert read_clone(plain) == CloneState(branch=None, unsaved=None)


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


def _clone_manager(records: dict) -> MagicMock:
    manager = MagicMock()
    manager.storage.get_worktree_by_workspace_id.side_effect = records.get
    return manager


class TestTheJsonListing:
    """`dl --ls --json`: the facts a cleanup tool decides from."""

    def _run(self, tmp_path, clone, capsys):
        cache = tmp_path / "cache" / "devlaunch"
        listing = json.dumps(
            [
                _entry("r-feature-aaa", cache / "repos" / "blooop" / "r" / "r-feature-aaa"),
                _entry("someone-elses", tmp_path / "projects" / "other"),
            ]
        )
        records = {"r-feature-aaa": FakeRecord("blooop", "r", "feature", str(clone))}

        def devpod(args, **kwargs):
            if args[:1] == ["list"]:
                return subprocess.CompletedProcess(args, 0, listing, "")
            if args[:1] == ["status"]:
                return subprocess.CompletedProcess(args, 0, json.dumps({"state": "Stopped"}), "")
            return subprocess.CompletedProcess(args, 0, "", "")

        with (
            patch("devlaunch.dl._get_cache_dir", return_value=cache),
            patch("devlaunch.dl.run_devpod", side_effect=devpod),
            patch("devlaunch.dl._get_clone_manager", return_value=_clone_manager(records)),
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
        assert ours["unsaved"] is None
        assert ours["state"] == "Stopped"

    def test_unsaved_work_is_reported_so_a_caller_can_leave_it_alone(self, tmp_path, clone, capsys):
        (clone / "scratch.txt").write_text("half-finished\n")
        _code, report = self._run(tmp_path, clone, capsys)
        ours = next(w for w in report if w["id"] == "r-feature-aaa")
        assert ours["unsaved"] and "uncommitted" in ours["unsaved"]

    def test_a_workspace_devlaunch_did_not_make_says_so_instead_of_guessing(
        self, tmp_path, clone, capsys
    ):
        _code, report = self._run(tmp_path, clone, capsys)
        foreign = next(w for w in report if w["id"] == "someone-elses")
        assert foreign["devlaunch"] is False
        assert foreign["repo"] is None and foreign["branch"] is None
        # And nothing inspected it: dl has no clone of its own to protect there.
        assert foreign["unsaved"] is None

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
    all. Measured on a real cache, a workspace clone is 39-46 ms of walking, and
    an ordinary devcontainer builds its environment *inside* the clone.
    """

    def _run(self, tmp_path, argv, capsys, payload_mib=2, unreadable=False):
        cache = tmp_path / "cache" / "devlaunch"
        clone = cache / "repos" / "blooop" / "r" / "r-feature-aaa"
        clone.mkdir(parents=True)
        (clone / "payload.bin").write_bytes(b"\0" * (payload_mib * 1024 * 1024))
        if unreadable:
            (clone / "locked").mkdir()
            (clone / "locked").chmod(0o000)
        listing = json.dumps(
            [
                _entry("r-feature-aaa", clone),
                _entry("someone-elses", tmp_path / "projects" / "other"),
            ]
        )
        records = {"r-feature-aaa": FakeRecord("blooop", "r", "feature", str(clone))}

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
                patch("devlaunch.dl._get_clone_manager", return_value=_clone_manager(records)),
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


class TestTheDeleteGuard:
    """`dl <ws> rm` refuses to destroy the only copy of something."""

    def _run(self, tmp_path, clone, argv):
        cache = tmp_path / "cache" / "devlaunch"
        listing = json.dumps(
            [_entry("r-feature-aaa", cache / "repos" / "blooop" / "r" / "r-feature-aaa")]
        )
        records = {"r-feature-aaa": FakeRecord("blooop", "r", "feature", str(clone))}
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

        with (
            patch("devlaunch.dl._get_cache_dir", return_value=cache),
            patch("devlaunch.dl.run_devpod", side_effect=devpod),
            patch("devlaunch.dl._get_clone_manager", return_value=_clone_manager(records)),
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
        assert unsaved and "pixi.lock" in unsaved

    def test_a_modified_tracked_file_keeps_its_first_letter(self, clone):
        # The regression that took real use to find. `git status --porcelain`
        # writes a *modified* tracked file as " M path" -- leading space -- and
        # a full strip() of git's output ate it, so the path was reported one
        # character short ("ixi.lock"). Untracked files start "??" and were
        # unharmed, which is why every test here passed while the feature was
        # printing nonsense. Asserted on the exact rendering, not on a substring.
        (clone / "feature.txt").write_text("edited by the container's build\n")
        assert holds_unsaved_work(clone) == "1 uncommitted change(s) (feature.txt)"

    def test_a_long_list_is_cut_short_rather_than_dumped(self, clone):
        for i in range(6):
            (clone / f"file{i}.txt").write_text("x\n")
        unsaved = holds_unsaved_work(clone)
        assert unsaved and "6 uncommitted change(s)" in unsaved
        # Three names and an ellipsis: enough to recognise, not a wall of text.
        assert unsaved.count(",") == 3 and "…" in unsaved

    def test_a_renamed_path_keeps_both_halves(self, clone):
        git("mv", "feature.txt", "renamed.txt", cwd=clone)
        unsaved = holds_unsaved_work(clone)
        assert unsaved and "renamed.txt" in unsaved
