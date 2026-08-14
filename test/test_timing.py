# pylint: disable=redefined-outer-name
"""Pin the env-gated wall-clock timing summary and its bench harness (#140).

`DEVLAUNCH_TIMING=1` makes every dl process end with one stderr summary naming
each subprocess round trip and the total wall time; unset, dl must write no
timing output at all. Pinned at the same boundaries as test_devpod_spawn_counts:
the CLI entry point on one side, the subprocess module on the other.
"""

import io
import json
import re
import shlex
import subprocess
import sys
import time
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from devlaunch import gh_auth, timing
from devlaunch.dl import main, remote_branch_exists, run_ssh
from test_devpod_spawn_counts import DevpodSpawns

# The `total` line carries a trailing note naming which clock it is; the
# per-round-trip lines do not, so the note is optional here.
TIMING_LINE = re.compile(r"^dl-timing: (.+?) \d+\.\d{3}s(?: \(.*\))?$", re.MULTILINE)

# The machine-readable mode's one line: a marker, then the document.
JSON_LINE = re.compile(r"^dl-timing-json: (\{.*\})$", re.MULTILINE)


def timing_labels(stderr: str):
    """The labels of every timing line in *stderr*, in order."""
    return TIMING_LINE.findall(stderr)


def timing_document(stderr: str) -> dict:
    """The one timing document in *stderr*, parsed. Exactly one, or fail."""
    found = JSON_LINE.findall(stderr)
    assert len(found) == 1, f"expected one timing document, got {len(found)} in: {stderr!r}"
    return json.loads(found[0])


def stage_names(document: dict):
    """The names of the stages in *document*, in the order reported."""
    return [stage["stage"] for stage in document["stages"]]


def stage_named(document: dict, name: str) -> dict:
    """The one stage called *name*. Fails if the stage is absent."""
    found = [stage for stage in document["stages"] if stage["stage"] == name]
    assert len(found) == 1, f"expected one {name!r} stage, got {found} in {document}"
    return found[0]


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


class TestMachineReadableMode:
    """`DEVLAUNCH_TIMING=json` reports the same run as one parseable document.

    The mode exists so a trend job can read stage seconds without scraping
    prose; the prose mode is what a human still gets from `=1`.
    """

    def test_json_asks_for_one_document_and_no_prose(self, monkeypatch, capsys):
        monkeypatch.setenv("DEVLAUNCH_TIMING", "json")
        assert main(["--version"]) == 0
        err = capsys.readouterr().err
        document = timing_document(err)
        assert document["total"] >= 0
        assert document["stages"] == []
        assert "dl-timing: " not in err

    def test_the_document_names_the_clock_its_total_came_from(self, monkeypatch, capsys):
        """The prose total carries that caveat inline; a consumer of the
        document must not have to know it out of band."""
        monkeypatch.setenv("DEVLAUNCH_TIMING", "json")
        assert main(["--version"]) == 0
        assert timing_document(capsys.readouterr().err)["total_epoch"] == timing.TOTAL_EPOCH

    def test_prose_mode_emits_no_document(self, monkeypatch, capsys):
        monkeypatch.setenv("DEVLAUNCH_TIMING", "1")
        assert main(["--version"]) == 0
        assert "dl-timing-json:" not in capsys.readouterr().err

    @pytest.mark.parametrize("value", [None, "", "0"])
    def test_off_emits_neither_shape(self, value, monkeypatch, capsys):
        if value is None:
            monkeypatch.delenv("DEVLAUNCH_TIMING", raising=False)
        else:
            monkeypatch.setenv("DEVLAUNCH_TIMING", value)
        assert main(["--version"]) == 0
        assert "dl-timing" not in capsys.readouterr().err


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
def documenting(monkeypatch):
    """An active recorder in document mode, emitted into a buffer.

    The stage tests drive begin/emit themselves for the same reason the
    `recording` fixture does: they exercise the recorder below main().
    """
    monkeypatch.setenv("DEVLAUNCH_TIMING", "json")
    monkeypatch.delenv(timing.HANDOFF_VAR, raising=False)
    timing.begin()
    buffer = io.StringIO()
    yield buffer
    timing.emit(buffer)


class TestStageVocabulary:
    """The named stages are one per actionable owner, and they are the
    contract a consumer outside this repo reads the document against."""

    def test_the_vocabulary_is_the_ownership_boundary_stages(self):
        assert timing.STAGES == ("handoff", "host-prep", "devpod-up", "tools", "attach")

    def test_a_stage_carries_the_spans_recorded_inside_it(self, documenting):
        with timing.stage("tools"):
            with timing.span("devpod ssh"):
                pass
        timing.emit(documenting)
        stage = stage_named(timing_document(documenting.getvalue()), "tools")
        assert [span["label"] for span in stage["spans"]] == ["devpod ssh"]

    def test_a_stage_totals_over_its_arm_not_just_over_its_spans(self, documenting):
        """The stage is the arm's whole cost — the host-side work between the
        round trips is time the owner spent too, and a trend that dropped it
        would show parts that never add up to the total."""
        with timing.stage("host-prep"):
            with timing.span("git ls-remote"):
                pass
            time.sleep(0.05)
        timing.emit(documenting)
        stage = stage_named(timing_document(documenting.getvalue()), "host-prep")
        assert stage["seconds"] >= sum(span["seconds"] for span in stage["spans"]) + 0.05

    def test_a_stage_entered_again_totals_over_both_of_its_arms(self, documenting):
        """One owner's work is not always one contiguous region — the token
        fetch is host prep whenever it happens — so a stage accumulates rather
        than reporting only its last visit."""
        for _ in range(2):
            with timing.stage("host-prep"):
                time.sleep(0.03)
        timing.emit(documenting)
        document = timing_document(documenting.getvalue())
        assert stage_names(document) == ["host-prep"]
        assert stage_named(document, "host-prep")["seconds"] >= 0.06

    def test_a_stage_inside_another_is_charged_to_the_inner_owner(self, documenting):
        """Nesting must not double-count: `tools` runs inside the launch that
        `devpod-up` brackets, and the seconds belong to one of them."""
        with timing.stage("devpod-up"):
            with timing.stage("tools"):
                time.sleep(0.05)
        timing.emit(documenting)
        document = timing_document(documenting.getvalue())
        assert stage_named(document, "tools")["seconds"] >= 0.05
        assert stage_named(document, "devpod-up")["seconds"] < 0.05

    def test_a_stage_that_never_ran_is_absent_rather_than_zero(self, documenting):
        """Absence is the "not reached" of the three-valued outcome: a stage
        reporting 0.000s claims it ran and cost nothing, which is a different
        and false statement."""
        with timing.stage("attach"):
            pass
        timing.emit(documenting)
        assert stage_names(timing_document(documenting.getvalue())) == ["attach"]

    def test_a_stage_that_failed_reports_its_span_up_to_the_failure(self, documenting):
        with pytest.raises(RuntimeError):
            with timing.stage("devpod-up"):
                time.sleep(0.03)
                raise RuntimeError("up blew up")
        timing.emit(documenting)
        stage = stage_named(timing_document(documenting.getvalue()), "devpod-up")
        assert stage["outcome"] == "failed"
        assert stage["seconds"] >= 0.03

    def test_a_stage_that_returned_is_reported_ok(self, documenting):
        with timing.stage("attach"):
            pass
        timing.emit(documenting)
        assert stage_named(timing_document(documenting.getvalue()), "attach")["outcome"] == "ok"

    def test_prose_mode_keeps_its_flat_span_lines_through_a_stage(self, monkeypatch, capsys):
        """Constraint 1: `=1` is the summary it always was. A stage wrapper
        around a span must not add a line to it or rename one."""
        monkeypatch.setenv("DEVLAUNCH_TIMING", "1")
        timing.begin()
        with timing.stage("tools"):
            with timing.span("devpod ssh"):
                pass
        timing.emit()
        assert timing_labels(capsys.readouterr().err) == ["devpod ssh", "total"]

    def test_a_stage_costs_nothing_and_records_nothing_when_timing_is_off(
        self, monkeypatch, capsys
    ):
        """The off state stays free: new stages on the launch path must not
        make an unmeasured run pay for them."""
        monkeypatch.delenv("DEVLAUNCH_TIMING", raising=False)
        timing.begin()
        ran = False
        with timing.stage("tools"):
            ran = True
        timing.emit()
        assert ran
        assert capsys.readouterr().err == ""


def document_after_begin(monkeypatch, stamp) -> dict:
    """The document of a run that began with *stamp* in the handoff variable.

    None means the variable is unset — the ordinary case, a dl a human
    launched.
    """
    monkeypatch.setenv("DEVLAUNCH_TIMING", "json")
    if stamp is None:
        monkeypatch.delenv(timing.HANDOFF_VAR, raising=False)
    else:
        monkeypatch.setenv(timing.HANDOFF_VAR, stamp)
    timing.begin()
    buffer = io.StringIO()
    timing.emit(buffer)
    return timing_document(buffer.getvalue())


class TestTheHandoffStamp:
    """`DEVLAUNCH_HANDOFF_T0` is the seam: whoever hands off to dl stamps it,
    dl reads it, and the gap between them becomes the `handoff` stage.

    It is the one stage nothing in this process runs, so it is also the one
    that can only be absent or measured — never zero.
    """

    def test_a_stamp_reports_the_gap_between_the_stamp_and_dl_starting(self, monkeypatch):
        handoff = stage_named(document_after_begin(monkeypatch, str(time.time() - 5)), "handoff")
        assert 5 <= handoff["seconds"] < 60
        assert handoff["outcome"] == "ok"
        assert handoff["spans"] == []

    def test_the_handoff_is_reported_ahead_of_the_stages_dl_ran(self, monkeypatch):
        """It is the earliest thing the document describes, and a reader
        should meet the stages in the order the launch met them."""
        monkeypatch.setenv("DEVLAUNCH_TIMING", "json")
        monkeypatch.setenv(timing.HANDOFF_VAR, str(time.time() - 1))
        timing.begin()
        with timing.stage("host-prep"):
            pass
        buffer = io.StringIO()
        timing.emit(buffer)
        assert stage_names(timing_document(buffer.getvalue())) == ["handoff", "host-prep"]

    @pytest.mark.parametrize(
        "stamp",
        [
            pytest.param(None, id="unset"),
            pytest.param("", id="empty"),
            pytest.param("   ", id="blank"),
            pytest.param("a while ago", id="not-a-number"),
            pytest.param("nan", id="nan"),
            pytest.param("inf", id="inf"),
        ],
    )
    def test_no_readable_stamp_reports_no_handoff_stage_at_all(self, stamp, monkeypatch):
        """Absent, not zero: reporting 0.000s would claim an instantaneous
        handoff, and a trend cannot tell that apart from a real one."""
        assert stage_names(document_after_begin(monkeypatch, stamp)) == []

    def test_a_stamp_in_the_future_reports_no_handoff_stage(self, monkeypatch):
        """Two clocks that disagree produce a negative gap, which is not a
        measurement of anything — so it is reported as the absence it is."""
        assert stage_names(document_after_begin(monkeypatch, str(time.time() + 60))) == []

    def test_the_prose_summary_is_untouched_by_a_stamp(self, monkeypatch, capsys):
        monkeypatch.setenv("DEVLAUNCH_TIMING", "1")
        monkeypatch.setenv(timing.HANDOFF_VAR, str(time.time() - 5))
        assert main(["--version"]) == 0
        assert timing_labels(capsys.readouterr().err) == ["total"]


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


class TestTheDocumentedColdReset:
    """The cold recipe in --help names a reset dl actually accepts.

    This exact defect shipped twice: a README recipe whose reset was a
    subcommand dl does not have, then the same recipe moved into --help
    unfixed. So the documented reset is exercised rather than read: extracted
    from the epilog and run against a devpod that has nothing to delete —
    the state every cold bench's first reset meets.
    """

    def test_the_documented_reset_succeeds_from_the_absent_state(self):
        recipe = re.search(r"--before '([^']+)'", BENCH.read_text())
        assert recipe, "the cold recipe documents no --before reset"
        # dl-next is this working tree's install of main() (see dev.sh).
        argv = shlex.split(recipe.group(1))[1:]

        def devpod(args, **_kwargs):
            # Real devpod v0.26.1 against the recipe's starting state: nothing
            # exists, so everything fails except an ignore-not-found delete.
            if args[:1] == ["delete"] and "--ignore-not-found" in args:
                return subprocess.CompletedProcess(args, 0, "", "")
            return subprocess.CompletedProcess(args, 1, "", "workspace not found")

        manager = MagicMock()
        manager.repo_manager.get_default_branch.return_value = "main"
        with (
            patch("devlaunch.dl.run_devpod", side_effect=devpod),
            patch("devlaunch.dl._get_clone_manager", return_value=manager),
            patch("devlaunch.dl.update_cache_background"),
        ):
            assert main(argv) == 0
