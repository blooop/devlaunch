"""The one line `dl --purge` and `dl --prune` end with, and what it may not say.

Both commands remove disk and neither removes the disk a user is usually looking
at. `docker system df` on the host devlaunch#160 was worked on read **86.5 GB
reclaimable images, 43.18 GB volumes, 13.88 GB build cache** — none of it
devlaunch's to delete, and none of it named by a command that had just reported
freeing a few gigabytes. So both commands end by naming the boundary.

The line is documentation, which is the kind of thing that rots quietly, so it is
held here rather than only in README.md. Two properties matter beyond it being
printed at all:

- **The two commands say it in the same words.** A user who has read one has read
  the other, and two copies of a sentence are two things to keep true. Asserted
  by comparing what the two commands actually printed, not by comparing each with
  a constant, which would pass with the constant itself wrong.
- **It informs and never offers.** `docker image prune -a` is unscoped: it would
  destroy images devlaunch did not build, which is the exact footgun PR #129 took
  out of `--purge`. devpod's images carry no devlaunch label, so there is nothing
  here that could name only devlaunch's own — a command that guessed would be
  handing someone a `docker image rm` for another tool's images.

The wording below is written out as a literal, traceable to the ticket, rather
than imported from the code: a test that imports the string it checks passes
whatever the string says.
"""

import json
import pathlib
import subprocess
from typing import List
from unittest.mock import patch

import pytest

from devlaunch.dl import main

# Written out, not imported. See the module docstring.
NOT_OURS = "devlaunch does not manage Docker images or volumes"
STILL_HOLDING = "the containers these workspaces used may still hold disk"
THE_TOOL_THAT_KNOWS = "docker system df"


class RecordedProcesses:
    """Answers `devpod list` with an empty listing and records every argv.

    Records *everything*, not just devpod, because one of the properties under
    test is about a process that must never start.
    """

    def __init__(self) -> None:
        self.commands: List[List[str]] = []

    def __call__(self, cmd, *_args, **_kwargs) -> subprocess.CompletedProcess:
        argv = list(cmd)
        self.commands.append(argv)
        if argv[:1] == ["devpod"] and argv[1:2] == ["list"]:
            return subprocess.CompletedProcess(argv, 0, json.dumps([]), "")
        return subprocess.CompletedProcess(argv, 0, "", "")

    @property
    def programs(self) -> List[str]:
        return [argv[0] for argv in self.commands if argv]


@pytest.fixture(name="cache")
def fixture_cache(tmp_path) -> pathlib.Path:
    """A devlaunch cache with something in it, so a purge has work to do."""
    path = tmp_path / "devlaunch"
    (path / "repos").mkdir(parents=True)
    (path / "completions.json").write_text("{}")
    return path


def run_purge(cache: pathlib.Path) -> RecordedProcesses:
    """A whole `dl --purge -y` against *cache*, with devpod listing nothing."""
    recorder = RecordedProcesses()
    with patch("devlaunch.dl.subprocess.run", side_effect=recorder):
        with patch("devlaunch.dl._get_cache_dir", return_value=cache):
            with patch("devlaunch.dl.update_cache_background"):
                assert main(["--purge", "-y"]) == 0
    return recorder


def run_prune() -> RecordedProcesses:
    """A whole `dl --prune -y` against the suite's own scratch cache."""
    recorder = RecordedProcesses()
    with patch("devlaunch.dl.subprocess.run", side_effect=recorder):
        assert main(["--prune", "-y"]) == 0
    return recorder


def boundary_line(out: str) -> str:
    """The one line naming the Docker disk, or a failure saying it is absent."""
    lines = [line for line in out.splitlines() if NOT_OURS in line]
    assert len(lines) == 1, f"expected one boundary line, got {lines}"
    return lines[0]


class TestWhatAPurgeSaysAboutTheDiskItDidNotFree:
    def test_it_names_the_docker_disk_it_does_not_manage(self, cache, capsys):
        run_purge(cache)
        assert NOT_OURS in capsys.readouterr().out

    def test_it_says_that_disk_may_still_be_held(self, cache, capsys):
        run_purge(cache)
        assert STILL_HOLDING in capsys.readouterr().out

    def test_it_points_at_the_command_that_measures_it(self, cache, capsys):
        run_purge(cache)
        assert THE_TOOL_THAT_KNOWS in capsys.readouterr().out


class TestWhatAPruneSaysAboutTheDiskItDidNotFree:
    def test_it_names_the_docker_disk_it_does_not_manage(self, capsys):
        run_prune()
        assert NOT_OURS in capsys.readouterr().out

    def test_it_says_that_disk_may_still_be_held(self, capsys):
        run_prune()
        assert STILL_HOLDING in capsys.readouterr().out

    def test_it_points_at_the_command_that_measures_it(self, capsys):
        run_prune()
        assert THE_TOOL_THAT_KNOWS in capsys.readouterr().out


class TestTheTwoCommandsCannotDriftApart:
    def test_they_name_the_boundary_in_the_same_words(self, cache, capsys):
        run_purge(cache)
        purged = boundary_line(capsys.readouterr().out)
        run_prune()
        pruned = boundary_line(capsys.readouterr().out)
        assert purged == pruned


class TestWhatTheBoundaryMayNotDo:
    def test_neither_command_offers_to_delete_an_image(self, cache, capsys):
        """Information, never an invitation. `docker image prune -a` is unscoped
        and `docker image rm` needs an id devlaunch cannot honestly supply."""
        run_purge(cache)
        purged = capsys.readouterr().out
        run_prune()
        printed = purged + capsys.readouterr().out
        for footgun in ("image prune", "docker image rm", "docker rmi", "system prune"):
            assert footgun not in printed

    def test_naming_it_starts_no_docker_process(self, cache, capsys):
        """Nothing to fail on a machine where Docker is absent or stopped. The
        line is a sentence, not a measurement."""
        purge = run_purge(cache)
        prune = run_prune()
        capsys.readouterr()
        assert "docker" not in purge.programs
        assert "docker" not in prune.programs
