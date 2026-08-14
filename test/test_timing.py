# pylint: disable=redefined-outer-name
"""Pin the env-gated wall-clock timing summary and its bench harness (#140).

`DEVLAUNCH_TIMING=1` makes every dl process end with one stderr summary naming
each subprocess round trip and the total wall time; unset, dl must write no
timing output at all. Pinned at the same boundaries as test_devpod_spawn_counts:
the CLI entry point on one side, the subprocess module on the other.
"""

import io
import re
import shlex
import subprocess
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

from devlaunch import gh_auth, timing
from devlaunch.dl import main, remote_branch_exists, run_ssh
from test_devpod_spawn_counts import DevpodSpawns

# The `total` line carries a trailing note naming which clock it is; the
# per-round-trip lines do not, so the note is optional here.
TIMING_LINE = re.compile(r"^dl-timing: (.+?) \d+\.\d{3}s(?: \(.*\))?$", re.MULTILINE)


def timing_labels(stderr: str):
    """The labels of every timing line in *stderr*, in order."""
    return TIMING_LINE.findall(stderr)


class TestSummaryGate:
    """Timing lines appear iff DEVLAUNCH_TIMING asks for them."""

    @pytest.mark.parametrize("value", [None, "", "0"])
    def test_no_timing_output_when_the_switch_is_off(self, value, monkeypatch, capsys):
        """Unset is the default, and the two ways of writing "off" are off too."""
        if value is None:
            monkeypatch.delenv("DEVLAUNCH_TIMING", raising=False)
        else:
            monkeypatch.setenv("DEVLAUNCH_TIMING", value)
        assert main(["--version"]) == 0
        assert "dl-timing:" not in capsys.readouterr().err

    def test_summary_ends_with_total_when_the_switch_is_on(self, monkeypatch, capsys):
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
    """The summary names each devpod round trip, in the order it happened —
    the chains from the spawn-count tests, seen as named timings."""

    @pytest.mark.parametrize(
        "argv, labels",
        [
            (["--ls"], ["devpod list"]),
            (["myws"], ["devpod status", "devpod ssh", "devpod ssh"]),
        ],
    )
    def test_the_chain_is_named_in_order(self, argv, labels, monkeypatch, capsys):
        monkeypatch.setenv("DEVLAUNCH_TIMING", "1")
        assert main(argv) == 0
        assert timing_labels(capsys.readouterr().err) == [*labels, "total"]


@pytest.fixture
def recording(monkeypatch):
    """An active recorder emitted into a buffer: these tests exercise helpers
    below main(), so the begin/emit main() does is driven here instead."""
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

    def test_remote_branch_probe_is_named(self, recording):
        answered = subprocess.CompletedProcess(
            ["git", "ls-remote"], 0, stdout="deadbeef\trefs/heads/main\n", stderr=""
        )
        with patch("devlaunch.dl.subprocess.run", return_value=answered):
            assert remote_branch_exists("owner/repo", "main")
        timing.emit(recording)
        assert timing_labels(recording.getvalue()) == ["git ls-remote", "total"]

    def test_a_failed_round_trip_is_recorded_and_still_raises(self, recording):
        """A spawn that raised still took time, and the span must not eat the
        exception on its way out."""
        with pytest.raises(RuntimeError):
            with timing.span("devpod up"):
                raise RuntimeError("spawn blew up")
        timing.emit(recording)
        assert timing_labels(recording.getvalue()) == ["devpod up", "total"]


BENCH = Path(__file__).parent.parent / "scripts" / "bench_launch.py"
FAILING = shlex.join([sys.executable, "-c", "raise SystemExit(3)"])


def run_bench(*argv: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(BENCH), *argv], capture_output=True, text=True, check=False
    )


class TestBenchHarness:
    """#140's "Done when": *a contributor can produce a median warm-launch and
    cold-launch wall time with one documented command*."""

    def test_reports_each_run_and_the_median(self):
        result = run_bench("-n", "3", "--", sys.executable, "-c", "pass")
        assert result.returncode == 0
        assert len(re.findall(r"^run \d+/3: \d+\.\d{3}s$", result.stdout, re.M)) == 3
        assert re.search(r"^median of 3: \d+\.\d{3}s", result.stdout, re.M)

    @pytest.mark.parametrize(
        "argv",
        [
            pytest.param(["-n", "3", "--", sys.executable, "-c", "raise SystemExit(7)"], id="run"),
            pytest.param(
                ["-n", "3", "--before", FAILING, "--", sys.executable, "-c", "pass"], id="reset"
            ),
        ],
    )
    def test_no_median_over_a_failed_run_or_a_failed_reset(self, argv):
        """Neither a failing launch nor a run whose cold state was never
        established is a number to compare against."""
        result = run_bench(*argv)
        assert result.returncode != 0
        assert "median" not in result.stdout

    def test_rejects_a_run_count_below_one(self):
        """There is no median of nothing, so it refuses rather than dying
        inside statistics."""
        result = run_bench("-n", "0", "--", sys.executable, "-c", "pass")
        assert result.returncode == 2
        assert "Traceback" not in result.stderr

    def test_the_reset_runs_before_every_timed_run_and_is_not_timed(self, tmp_path):
        """Delete once and bench N times and runs 2..N are warm, so `--before`
        resets per run — and the teardown's time is not the launch's."""
        marker = tmp_path / "resets"
        reset = shlex.join(
            [
                sys.executable,
                "-c",
                f"import time; time.sleep(0.2); open({str(marker)!r}, 'a').write('x')",
            ]
        )
        result = run_bench("-n", "2", "--before", reset, "--", sys.executable, "-c", "pass")
        assert result.returncode == 0, result.stderr
        assert marker.read_text() == "xx"
        runs = re.findall(r"^run \d+/2: (\d+\.\d{3})s$", result.stdout, re.M)
        assert runs and all(float(seconds) < 0.15 for seconds in runs), result.stdout
