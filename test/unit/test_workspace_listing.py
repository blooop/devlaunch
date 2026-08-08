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
import subprocess
from typing import List
from unittest.mock import patch

import pytest

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

    def test_json_of_the_wrong_shape_is_reported(self):
        """devpod answering with valid JSON that is not a sequence of workspaces
        is still an answer we cannot read."""
        devpod = RecordedDevpod('{"workspaces": []}')

        with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
            with pytest.raises(UnreadableWorkspaceList):
                list_workspaces()

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
