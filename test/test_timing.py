# pylint: disable=redefined-outer-name
"""Pin the env-gated wall-clock timing summary (ticket #140).

`DEVLAUNCH_TIMING=1` makes every dl process end with one stderr summary naming
each subprocess round trip and the total wall time, so a perf change can land
with before/after numbers. With the variable unset, dl must write no timing
output at all: the hot path must not pay for its own thermometer.

These tests pin behavior at the same boundaries as test_devpod_spawn_counts:
the CLI entry point on one side, the subprocess module on the other.
"""

import io
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

from devlaunch import gh_auth, timing, workspace_state
from devlaunch.dl import main, remote_branch_exists, run_ssh
from test_devpod_spawn_counts import DevpodSpawns

TIMING_LINE = re.compile(r"^dl-timing: (.+) \d+\.\d{3}s$", re.MULTILINE)


def timing_labels(stderr: str):
    """The labels of every timing line in *stderr*, in order."""
    return TIMING_LINE.findall(stderr)


REPO_ROOT = Path(__file__).parent.parent


class TestSuiteIsHermeticAgainstTheSwitch:
    """The suite must pass for a developer who has DEVLAUNCH_TIMING exported.

    The people this instrument exists for are exactly the people who will leave
    it on in their shell, and the next thing they do is run the tests. A switch
    whose own suite goes red when the switch is on is a trap laid for the agent
    debugging a launch a month from now, so the property is pinned by running a
    slice of the suite in a child process with the variable exported -- the same
    way a developer would meet it.
    """

    def test_suite_passes_with_the_switch_exported(self):
        affected = "test/test_dl.py::TestMissingDevpodBinary"
        result = subprocess.run(
            [sys.executable, "-m", "pytest", affected, "-q", "-p", "no:randomly"],
            cwd=REPO_ROOT,
            env={**os.environ, "DEVLAUNCH_TIMING": "1"},
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0, result.stdout[-2000:]


class TestSummaryGate:
    """Timing lines appear iff DEVLAUNCH_TIMING is set."""

    def test_no_timing_output_when_env_unset(self, monkeypatch, capsys):
        monkeypatch.delenv("DEVLAUNCH_TIMING", raising=False)
        assert main(["--version"]) == 0
        assert "dl-timing:" not in capsys.readouterr().err

    def test_summary_ends_with_total_when_env_set(self, monkeypatch, capsys):
        monkeypatch.setenv("DEVLAUNCH_TIMING", "1")
        assert main(["--version"]) == 0
        assert timing_labels(capsys.readouterr().err) == ["total"]


@pytest.fixture
def spawns():
    """A devpod stub at the subprocess boundary, background updater disabled."""
    recorder = DevpodSpawns(["myws"])
    with patch("devlaunch.dl.subprocess.run", side_effect=recorder):
        with patch("devlaunch.dl.subprocess.Popen", side_effect=recorder.popen):
            with patch("devlaunch.dl.update_cache_background"):
                yield recorder


@pytest.mark.usefixtures("spawns")
class TestDevpodRoundTripsAreNamed:
    """The summary names each devpod round trip, in the order it happened."""

    def test_ls_names_its_single_list_round_trip(self, monkeypatch, capsys):
        monkeypatch.setenv("DEVLAUNCH_TIMING", "1")
        assert main(["--ls"]) == 0
        assert timing_labels(capsys.readouterr().err) == ["devpod list", "total"]

    def test_attach_names_every_round_trip_in_order(self, monkeypatch, capsys):
        """The attach chain from the spawn-count tests, seen as named timings:
        one status, the hostname ssh, then the session ssh."""
        monkeypatch.setenv("DEVLAUNCH_TIMING", "1")
        assert main(["myws"]) == 0
        assert timing_labels(capsys.readouterr().err) == [
            "devpod status",
            "devpod ssh",
            "devpod ssh",
            "total",
        ]


@pytest.fixture
def recording(monkeypatch):
    """An active timing recorder, emitted into a buffer for inspection.

    These tests exercise helpers below main(), so the per-command begin/emit
    that main() does is driven here instead.
    """
    monkeypatch.setenv("DEVLAUNCH_TIMING", "1")
    timing.begin()
    buffer = io.StringIO()
    yield buffer
    timing.emit(buffer)


class TestTransportAndGitGhCallsAreNamed:
    """The other launch chokepoints show up in the summary by name."""

    def test_openssh_transport_is_named(self, recording):
        done = subprocess.CompletedProcess(["ssh", "myws.devpod"], 0)
        with patch("devlaunch.dl.subprocess.run", return_value=done):
            run_ssh(["ssh", "myws.devpod"])
        timing.emit(recording)
        assert timing_labels(recording.getvalue()) == ["ssh", "total"]

    def test_gh_token_round_trip_is_named(self, recording, monkeypatch):
        monkeypatch.setenv(gh_auth.DISABLE_VAR, "0")
        for var in gh_auth.HOST_TOKEN_VARS:
            monkeypatch.delenv(var, raising=False)
        gh_auth.resolve_token.cache_clear()
        answered = subprocess.CompletedProcess(
            ["gh", "auth", "token"], 0, stdout="gho_" + "a" * 36 + "\n", stderr=""
        )
        with patch("devlaunch.gh_auth.shutil.which", return_value="/usr/bin/gh"):
            with patch("devlaunch.gh_auth.subprocess.run", return_value=answered):
                assert gh_auth.resolve_token() is not None
        gh_auth.resolve_token.cache_clear()
        timing.emit(recording)
        assert timing_labels(recording.getvalue()) == ["gh auth token", "total"]

    def test_clone_state_git_reads_are_named(self, recording, tmp_path):
        # A directory that is not a repository still costs the rev-parse and
        # status round trips, and those are what the summary must name.
        workspace_state.read_clone(tmp_path)
        timing.emit(recording)
        assert timing_labels(recording.getvalue()) == [
            "git rev-parse",
            "git status",
            "total",
        ]

    def test_remote_branch_probe_is_named(self, recording):
        answered = subprocess.CompletedProcess(
            ["git", "ls-remote"], 0, stdout="deadbeef\trefs/heads/main\n", stderr=""
        )
        with patch("devlaunch.dl.subprocess.run", return_value=answered):
            assert remote_branch_exists("owner/repo", "main")
        timing.emit(recording)
        assert timing_labels(recording.getvalue()) == ["git ls-remote", "total"]


BENCH = Path(__file__).parent.parent / "scripts" / "bench_launch.py"


def run_bench(*argv: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(BENCH), *argv], capture_output=True, text=True, check=False
    )


class TestBenchHarness:
    """One command per side of a before/after: N runs, a median."""

    def test_reports_each_run_and_the_median(self):
        result = run_bench("-n", "3", "--", sys.executable, "-c", "pass")
        assert result.returncode == 0
        assert len(re.findall(r"^run \d+/3: \d+\.\d{3}s$", result.stdout, re.M)) == 3
        assert re.search(r"^median of 3: \d+\.\d{3}s$", result.stdout, re.M)

    def test_fails_loudly_when_the_command_fails(self):
        """A failing launch's time is not a number to compare against, so the
        bench must not report a median over it."""
        result = run_bench("-n", "3", "--", sys.executable, "-c", "raise SystemExit(7)")
        assert result.returncode != 0
        assert "median" not in result.stdout

    def test_rejects_a_run_count_below_one(self):
        """There is no median of nothing, so the bench refuses to be asked for
        one rather than dying inside statistics."""
        result = run_bench("-n", "0", "--", sys.executable, "-c", "pass")
        assert result.returncode == 2
        assert "Traceback" not in result.stderr


class TestColdBench:
    """#140's "Done when": *a contributor can produce a median warm-launch and
    cold-launch wall time with one documented command*.

    A cold median needs the cold condition re-established before *every* timed
    run: delete once and bench N times and runs 2..N are warm, so the printed
    median is a warm number wearing a cold label. `--before` is that per-run
    reset, and the bench refuses to report a median when the reset fails --
    a run whose cold condition was never established is not a cold measurement.
    """

    def before_writing(self, marker: Path) -> str:
        return shlex.join([sys.executable, "-c", f"open({str(marker)!r}, 'a').write('x')"])

    def test_the_reset_runs_once_before_every_timed_run(self, tmp_path):
        marker = tmp_path / "resets"
        result = run_bench(
            "-n", "3", "--before", self.before_writing(marker), "--", sys.executable, "-c", "pass"
        )
        assert result.returncode == 0, result.stderr
        assert marker.read_text() == "xxx"

    def test_the_reset_is_not_counted_as_launch_time(self):
        """The number reported is the launch's, not the teardown's."""
        slow_reset = shlex.join([sys.executable, "-c", "import time; time.sleep(0.5)"])
        result = run_bench("-n", "1", "--before", slow_reset, "--", sys.executable, "-c", "pass")
        assert result.returncode == 0, result.stderr
        run = re.search(r"^run 1/1: (\d+\.\d{3})s", result.stdout, re.M)
        assert run is not None, result.stdout
        assert float(run.group(1)) < 0.4

    def test_refuses_a_median_when_the_reset_fails(self):
        failing_reset = shlex.join([sys.executable, "-c", "raise SystemExit(3)"])
        result = run_bench("-n", "3", "--before", failing_reset, "--", sys.executable, "-c", "pass")
        assert result.returncode != 0
        assert "median" not in result.stdout
