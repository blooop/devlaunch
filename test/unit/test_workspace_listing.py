"""An unreadable workspace listing is not an empty one.

`devpod list --output json` can fail in two ways that used to arrive at the same
place: it can exit non-zero, and it can exit zero having printed something that
is not a listing. Both came back as `[]`, which is also how devpod says "this
machine has no workspaces" -- so a caller could not tell "there are none" from
"I could not ask", and none of them tried.

The empty answer here is a recording, not a guess: `devpod list --output json`
against a devpod home that has never held a workspace prints `[]`. Nothing at all
is therefore not devpod's way of saying empty, and this suite is entitled to
treat it as unreadable.
"""

import json
import logging
import subprocess
from typing import List
from unittest.mock import patch

import pytest

from devlaunch import dl
from devlaunch.dl import (
    UnreadableWorkspaceList,
    list_workspaces,
    main,
    purge_all_data,
)

# What devpod prints for a machine with no workspaces.
NOTHING_LISTED = "[]"

ONE_WORKSPACE = json.dumps(
    [
        {
            "id": "ws1",
            "source": {"localFolder": "/cache/ws1"},
            "provider": {"name": "docker"},
            "ide": {"name": "none"},
            "lastUsed": "2026-01-01T00:00:00Z",
        }
    ]
)


class RecordedDevpod:
    """Stands in for the devpod process, answering `list` with a recording."""

    def __init__(self, listing: str, rc: int = 0, stderr: str = ""):
        self.listing = listing
        self.rc = rc
        self.stderr = stderr
        self.commands: List[List[str]] = []

    def __call__(self, cmd, *_args, **_kwargs) -> subprocess.CompletedProcess:
        argv = list(cmd)
        self.commands.append(argv)
        if argv[1:2] == ["list"]:
            return subprocess.CompletedProcess(argv, self.rc, self.listing, self.stderr)
        return subprocess.CompletedProcess(argv, 0, "", "")

    @property
    def deleted(self) -> List[str]:
        return [c[2] for c in self.commands if c[1:2] == ["delete"]]


class TestAFailedReadIsNotAnEmptyList:
    """devpod exiting non-zero -- the first of the two modes that used to merge."""

    def test_a_non_zero_exit_is_reported_rather_than_returned_as_empty(self):
        devpod = RecordedDevpod("", rc=1, stderr="context not found")

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with pytest.raises(UnreadableWorkspaceList):
                list_workspaces()

    def test_the_report_carries_what_devpod_said(self):
        """devpod's own explanation is the only thing that tells anyone what to
        do about it, so it must survive into the message."""
        devpod = RecordedDevpod("", rc=1, stderr="context not found")

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with pytest.raises(UnreadableWorkspaceList) as raised:
                list_workspaces()

        assert "context not found" in str(raised.value)

    def test_the_report_stays_on_one_line(self):
        """DEVPOD_MISSING_MESSAGE's comment sets the rule for dl's failure
        messages -- one line, so a completion helper that trips over one cannot
        spew into the user's shell -- and devpod's stderr is routinely several."""
        devpod = RecordedDevpod("", rc=1, stderr="Error: line one\nline two\nline three")

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with pytest.raises(UnreadableWorkspaceList) as raised:
                list_workspaces()

        assert "\n" not in str(raised.value)
        assert "line three" in str(raised.value), "the message must still carry what devpod said"


class TestAnUnparsableReadIsNotAnEmptyList:
    """devpod exiting zero with output that is not a listing -- the second mode."""

    def test_output_that_is_not_json_is_reported(self):
        devpod = RecordedDevpod("Error: no context selected\n")

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with pytest.raises(UnreadableWorkspaceList):
                list_workspaces()

    def test_saying_nothing_at_all_is_reported(self):
        """A successful `devpod list` prints `[]` when there is nothing to list,
        so silence is a listing we cannot read, not a listing with nothing in it."""
        devpod = RecordedDevpod("")

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with pytest.raises(UnreadableWorkspaceList):
                list_workspaces()

    def test_saying_nothing_at_all_is_reported_as_silence(self):
        """And is reported as silence rather than as a parse failure: `not JSON:
        ''` reads like a bug in dl for the one case the parser singles out."""
        devpod = RecordedDevpod("   \n")

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with pytest.raises(UnreadableWorkspaceList) as raised:
                list_workspaces()

        assert "said nothing" in str(raised.value)
        assert "not JSON" not in str(raised.value)

    @pytest.mark.parametrize("listing", ['{"workspaces": []}', "null", "42", '"ws1"'])
    def test_json_of_the_wrong_shape_is_reported(self, listing):
        """devpod answering with valid JSON that is not a sequence of workspaces
        is still an answer we cannot read.

        The message is asserted, not merely the type. `{"workspaces": []}` is
        JSON that a `for entry in parsed` loop will happily walk -- it yields the
        object's keys, which are strings -- so without the message assertion the
        per-entry guard rescues this test and the top-level shape check can be
        deleted with the whole suite still green.
        """
        devpod = RecordedDevpod(listing)

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with pytest.raises(UnreadableWorkspaceList) as raised:
                list_workspaces()

        assert "expected devpod to list workspaces" in str(raised.value)

    def test_entries_that_are_not_workspaces_are_reported(self):
        devpod = RecordedDevpod('["ws1", "ws2"]')

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with pytest.raises(UnreadableWorkspaceList):
                list_workspaces()


class TestAnEmptyListingIsStillEmpty:
    """The distinction cuts both ways: an answer that reads fine and says
    nothing is listed still answers, and still answers with an empty list."""

    def test_a_machine_with_no_workspaces_lists_none(self):
        devpod = RecordedDevpod(NOTHING_LISTED)

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            assert list_workspaces() == []

    def test_a_machine_with_workspaces_lists_them(self):
        devpod = RecordedDevpod(ONE_WORKSPACE)

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            assert [ws.id for ws in list_workspaces()] == ["ws1"]


class TestPurgeWillNotActOnAListItCouldNotRead:
    """The caller the ticket is named for: a purge that quietly did nothing used
    to look exactly like a purge that had nothing to do."""

    def test_an_unreadable_listing_stops_the_purge(self, tmp_path):
        devpod = RecordedDevpod("", rc=1, stderr="context not found")
        cache_dir = tmp_path / "devlaunch"
        cache_dir.mkdir()

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with patch("devlaunch.dl._get_cache_dir", return_value=cache_dir):
                with pytest.raises(UnreadableWorkspaceList):
                    purge_all_data()

        assert devpod.deleted == []
        assert cache_dir.exists(), "a purge that could not read the list must not half-run"

    def test_the_command_says_so_instead_of_reporting_success(self, tmp_path, capsys):
        """`dl --purge -y` against a devpod it cannot read exits non-zero and
        says why, where it used to print that there was nothing to purge."""
        devpod = RecordedDevpod("", rc=1, stderr="context not found")
        cache_dir = tmp_path / "devlaunch"
        cache_dir.mkdir()

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with patch("devlaunch.dl.update_cache_background"):
                with patch("devlaunch.dl._get_cache_dir", return_value=cache_dir):
                    assert main(["--purge", "-y"]) != 0

        captured = capsys.readouterr()
        assert "No data to purge" not in captured.out
        assert "context not found" in captured.err
        assert cache_dir.exists()


class TestCompletionsAreBuiltFromWhateverCanBeRead:
    """The one reader that has something to do with an unreadable listing.

    Every other reader of the workspace list is answering a question about
    workspaces, so a list it cannot believe leaves it nothing to say. Building
    shell completions is not that: the workspace names are one of four things it
    offers, and the other three -- repos, owners, branches -- come off the local
    disk and are unaffected by whether devpod can be reached. Refusing would mean
    a devpod that cannot answer stops `dl --install` from installing completions
    at all, which is a worse answer than completing on repos alone, and which is
    not what it did before the listing learned to refuse.
    """

    @staticmethod
    def _scoped_cache(monkeypatch, tmp_path):
        """Point the completion caches at tmp_path and nothing at $HOME."""
        monkeypatch.setattr(dl, "CACHE_FILE", tmp_path / "completions.json")
        monkeypatch.setattr(dl, "BASH_CACHE_FILE", tmp_path / "completions.bash")
        monkeypatch.setenv("DEVLAUNCH_COMPLETION_FILE", str(tmp_path / "completions.sh"))

    def test_install_still_installs_when_the_listing_cannot_be_read(self, tmp_path, monkeypatch):
        """`dl --install` warms the cache before installing, and a devpod it
        cannot read used to take the install down with it."""
        self._scoped_cache(monkeypatch, tmp_path)
        devpod = RecordedDevpod("", rc=1, stderr="context not found")
        rc_file = tmp_path / "bashrc"

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            assert main(["--install", str(rc_file)]) == 0

        assert (tmp_path / "completions.sh").exists(), "the completion script was not written"
        assert "devlaunch completions" in rc_file.read_text(encoding="utf-8")

    def test_completion_data_still_offers_the_repos_it_can_see(self, tmp_path, monkeypatch, capsys):
        """Repos come off the local cache directory, so an unreachable devpod
        costs the workspace names and nothing else."""
        self._scoped_cache(monkeypatch, tmp_path)
        devpod = RecordedDevpod("", rc=1, stderr="context not found")

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with patch("devlaunch.dl.update_cache_background"):
                with patch(
                    "devlaunch.dl.discover_repos_from_cache_dir",
                    return_value={"blooop": ["devlaunch"]},
                ):
                    with patch("devlaunch.dl.get_remote_branches", return_value=[]):
                        with patch("devlaunch.dl.get_local_branches", return_value=[]):
                            assert main(["--completion-data"]) == 0

        data = json.loads(capsys.readouterr().out)
        assert data["repos"] == ["blooop/devlaunch"]
        assert data["workspaces"] == []

    def test_the_missing_workspaces_are_explained_rather_than_silently_absent(
        self, tmp_path, monkeypatch, caplog
    ):
        """Degrading quietly would make `dl --refresh` report zero workspaces as
        if it had counted them, so the reason goes to the log."""
        self._scoped_cache(monkeypatch, tmp_path)
        devpod = RecordedDevpod("", rc=1, stderr="context not found")

        with caplog.at_level(logging.WARNING, logger="root"):
            with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
                with patch("devlaunch.dl.discover_repos_from_cache_dir", return_value={}):
                    dl.update_completion_cache()

        assert "context not found" in caplog.text
