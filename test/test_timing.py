# pylint: disable=redefined-outer-name
"""Pin the env-gated wall-clock timing summary and its bench harness (#140).

`DEVLAUNCH_TIMING=1` makes every dl process end with one stderr summary naming
each subprocess round trip and the total wall time; unset, dl must write no
timing output at all. Pinned at the same boundaries as test_devpod_spawn_counts:
the CLI entry point on one side, the subprocess module on the other.
"""

import importlib.util
import io
import json
import re
import shlex
import subprocess
import sys
import time
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

from devlaunch import gh_auth, timing, tools
from devlaunch.dl import main, remote_branch_exists, run_ssh, workspace_up
from test_devpod_spawn_counts import DevpodSpawns

# The `total` line carries a trailing note naming which clock it is; the
# per-round-trip lines do not, so the note is optional here.
TIMING_LINE = re.compile(r"^dl-timing: (.+?) \d+\.\d{3}s(?: \(.*\))?$", re.MULTILINE)

# The machine-readable mode's one line: a marker, then the document.
JSON_LINE = re.compile(r"^dl-timing-json: (\{.*\})$", re.MULTILINE)


@contextmanager
def contended_lock():
    """Stand in for hold_lock, reporting that this launch had to wait."""
    yield True


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


def span_labels(stage: dict):
    """The labels of the finer spans nested under *stage*, in order."""
    return [span["label"] for span in stage["spans"]]


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


def stamp_env(monkeypatch, stamp, prewarm) -> None:
    """Put the two seam stamps in the environment. None means unset."""
    monkeypatch.setenv("DEVLAUNCH_TIMING", "json")
    for var, value in ((timing.HANDOFF_VAR, stamp), (timing.PREWARM_VAR, prewarm)):
        if value is None:
            monkeypatch.delenv(var, raising=False)
        else:
            monkeypatch.setenv(var, value)


def document_after_begin(monkeypatch, stamp, prewarm=None) -> dict:
    """The document of a run that began with these stamps in the environment.

    None means the variable is unset — the ordinary case for both of them is a
    dl a human launched from a shell.
    """
    stamp_env(monkeypatch, stamp, prewarm)
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

    def test_the_handoff_is_the_one_stage_that_lies_outside_the_total(self, monkeypatch):
        """It ends where `total` begins — it is the gap this process could not
        have measured from inside itself — so a consumer adding the stages up
        against the total is adding up the others."""
        document = document_after_begin(monkeypatch, str(time.time() - 5))
        assert stage_named(document, "handoff")["seconds"] > document["total"]

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


class TestThePrewarmStamp:
    """`DEVLAUNCH_PREWARM_FIRED_AT` is the seam's other half: when a prewarm
    was fired for this workspace, if one was.

    It carries a past action and nothing else. Whether the prewarm actually
    helped is not something its firer can know — it fires and forgets — so the
    claim is dl's to make, from the launch it then saw.
    """

    def test_no_prewarm_stamp_reports_no_prewarm_at_all(self, monkeypatch):
        assert "prewarm" not in document_after_begin(monkeypatch, str(time.time() - 5))

    def test_the_head_start_is_the_gap_the_prewarm_bought(self, monkeypatch):
        now = time.time()
        document = document_after_begin(monkeypatch, str(now - 5), prewarm=str(now - 35))
        assert document["prewarm"]["head_start_seconds"] == pytest.approx(30, abs=1)

    def test_without_the_keystroke_stamp_there_is_no_head_start_to_report(self, monkeypatch):
        """One stamp is not a gap: the head start is a difference, and half of
        a difference is absent rather than zero."""
        document = document_after_begin(monkeypatch, None, prewarm=str(time.time() - 35))
        assert "head_start_seconds" not in document.get("prewarm", {})

    def test_a_prewarm_fired_after_the_keystroke_reports_no_head_start(self, monkeypatch):
        """A prewarm that fired later than the keystroke gave no head start,
        and a negative one is not a measurement to put in a trend."""
        now = time.time()
        document = document_after_begin(monkeypatch, str(now - 35), prewarm=str(now - 5))
        assert "head_start_seconds" not in document.get("prewarm", {})

    @pytest.mark.parametrize("stamp", ["", "recently", "nan"])
    def test_an_unreadable_prewarm_stamp_reports_no_prewarm(self, stamp, monkeypatch):
        assert "prewarm" not in document_after_begin(monkeypatch, str(time.time() - 5), stamp)


@pytest.mark.usefixtures("spawns")
class TestTheAttachShapeAPrewarmProduced:
    """Which shape the launch turned out to be is dl's own observation — the
    falsifying event (an `up` this launch had to run itself) is only visible
    from in here."""

    def test_a_launch_that_found_the_workspace_already_up_is_a_hit(self, monkeypatch, capsys):
        stamp_env(monkeypatch, None, str(time.time() - 30))
        assert main(["myws"]) == 0
        document = timing_document(capsys.readouterr().err)
        assert document["prewarm"]["shape"] == "hit"
        assert "head_start_seconds" not in document["prewarm"]

    def test_a_launch_with_nothing_prewarmed_claims_no_shape(self, monkeypatch, capsys):
        """Absent, not "miss": no prewarm was fired, so there is no prewarm to
        report the outcome of."""
        stamp_env(monkeypatch, str(time.time() - 5), None)
        assert main(["myws"]) == 0
        assert "prewarm" not in timing_document(capsys.readouterr().err)

    def test_a_launch_that_ran_the_up_itself_is_a_miss(self, spawns, monkeypatch):
        spawns.workspace_ids = ["myws", "brand-new"]
        stamp_env(monkeypatch, None, str(time.time() - 30))
        timing.begin()
        with patch("devlaunch.tools.host_payload", return_value=None):
            workspace_up("brand-new", workspace_id="brand-new", workspace_identity="brand-new")
        buffer = io.StringIO()
        timing.emit(buffer)
        assert timing_document(buffer.getvalue())["prewarm"]["shape"] == "miss"


class TestALaunchThatWaitedForItsPrewarm:
    """The middle case: the prewarm was still running, so this launch queued
    behind it and got a container it did not have to build — but paid the wait
    the prewarm existed to avoid."""

    def test_a_launch_that_waited_for_a_sibling_is_partial(self, monkeypatch):
        stamp_env(monkeypatch, None, str(time.time() - 30))
        timing.begin()
        spawned = []

        def devpod(args, **_kwargs):
            spawned.append(list(args))
            stdout = "{}" if args[:2] == ["context", "options"] else ""
            return subprocess.CompletedProcess(args=list(args), returncode=0, stdout=stdout)

        with (
            patch("devlaunch.dl.run_devpod", side_effect=devpod),
            patch("devlaunch.dl.hold_lock", lambda *_a, **_k: contended_lock()),
            patch("devlaunch.dl.get_workspace_state", return_value="Running"),
            patch("devlaunch.dl.invalidate_workspace_list_cache"),
            patch("devlaunch.dl.tools.ensure_tools"),
        ):
            workspace_up("owner/repo", workspace_id="myws", workspace_identity="myws")
        buffer = io.StringIO()
        timing.emit(buffer)
        assert [args for args in spawned if args[:1] == ["up"]] == []
        assert timing_document(buffer.getvalue())["prewarm"]["shape"] == "partial"


@pytest.mark.usefixtures("spawns")
class TestAWarmLaunchReportsItsStages:
    """The stages a launch reports are the ones it actually walked.

    A warm launch asks devpod whether the workspace is up and then attaches;
    it does no host git work and lends no tools, and the two stages it never
    reached are absent rather than zeroed.
    """

    def test_a_warm_launch_is_the_devpod_probe_and_the_attach(self, monkeypatch, capsys):
        monkeypatch.setenv("DEVLAUNCH_TIMING", "json")
        assert main(["myws"]) == 0
        document = timing_document(capsys.readouterr().err)
        assert stage_names(document) == ["devpod-up", "attach"]

    def test_each_stage_carries_the_round_trips_it_paid_for(self, monkeypatch, capsys):
        """The finer spans nest under their stage, so digging into which trip
        cost the launch its seconds needs no second run."""
        monkeypatch.setenv("DEVLAUNCH_TIMING", "json")
        assert main(["myws"]) == 0
        document = timing_document(capsys.readouterr().err)
        assert span_labels(stage_named(document, "devpod-up")) == ["devpod status"]
        assert span_labels(stage_named(document, "attach")) == ["devpod ssh", "devpod ssh"]

    def test_the_stages_account_for_the_bulk_of_the_total(self, monkeypatch, capsys):
        """A decomposition that leaves the launch's time somewhere else is not
        a decomposition — and the nesting must not charge one second twice."""
        monkeypatch.setenv("DEVLAUNCH_TIMING", "json")
        assert main(["myws"]) == 0
        document = timing_document(capsys.readouterr().err)
        assert sum(stage["seconds"] for stage in document["stages"]) <= document["total"]


class TestAColdStartReportsItsStages:
    """Bringing a workspace up is two owners: devpod, and the tools dl lends
    into the container once devpod has one."""

    def test_the_up_and_the_tools_it_precedes_are_separate_stages(self, spawns, documenting):
        spawns.workspace_ids = ["myws", "brand-new"]
        with patch("devlaunch.tools.host_payload", return_value=None):
            workspace_up("brand-new", workspace_id="brand-new")
        timing.emit(documenting)
        document = timing_document(documenting.getvalue())
        assert stage_names(document) == ["devpod-up", "tools"]
        assert "devpod up" in span_labels(stage_named(document, "devpod-up"))
        assert span_labels(stage_named(document, "tools")) == ["devpod ssh", "devpod ssh"]

    def test_staging_the_lent_payload_is_named_inside_the_tools_stage(
        self, spawns, documenting, tmp_path
    ):
        """The tar is host work between two round trips — the round trips name
        themselves, and without this span the staging is invisible."""
        spawns.workspace_ids = ["myws", "brand-new"]
        lent = tmp_path / "claude"
        lent.write_bytes(b"binary")
        payload = tools.HostPayload(claude_version="1.2.3", members=((lent, ".local/bin/claude"),))
        with patch("devlaunch.tools.host_payload", return_value=payload):
            workspace_up("brand-new", workspace_id="brand-new")
        timing.emit(documenting)
        document = timing_document(documenting.getvalue())
        assert "tools tar" in span_labels(stage_named(document, "tools"))


class TestHostPrepIsAStage:
    """The host's own work before devpod is ever asked for anything: the bare
    clone and its fetches, the locks around them, the LFS probe, and the token
    dl forwards into the workspace."""

    def test_the_bare_clone_is_charged_to_host_prep(
        self, real_managers, local_git_repo, documenting
    ):
        real_managers["repo_manager"].ensure_repo("owner", "repo", local_git_repo["remote_url"])
        timing.emit(documenting)
        document = timing_document(documenting.getvalue())
        assert stage_names(document) == ["host-prep"]
        assert "git clone --bare" in span_labels(stage_named(document, "host-prep"))

    def test_the_token_fetch_is_host_prep_even_when_it_happens_mid_attach(
        self, documenting, monkeypatch
    ):
        """Host prep is an owner, not a region of the timeline: the token trip
        is the host's work wherever on the launch it falls, and the stage it
        interrupts is not charged for it."""
        monkeypatch.setenv(gh_auth.DISABLE_VAR, "0")
        for var in gh_auth.HOST_TOKEN_VARS:
            monkeypatch.delenv(var, raising=False)
        gh_auth.resolve_token.cache_clear()
        answered = subprocess.CompletedProcess(
            ["gh", "auth", "token"], 0, stdout="gho_" + "a" * 36 + "\n", stderr=""
        )
        with timing.stage("attach"):
            with patch("devlaunch.gh_auth.shutil.which", return_value="/usr/bin/gh"):
                with patch("devlaunch.gh_auth.subprocess.run", return_value=answered):
                    assert gh_auth.resolve_token() is not None
        gh_auth.resolve_token.cache_clear()
        timing.emit(documenting)
        document = timing_document(documenting.getvalue())
        assert span_labels(stage_named(document, "host-prep")) == ["gh auth token"]
        assert span_labels(stage_named(document, "attach")) == []


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


def a_document(stages, total: float = 0.3) -> str:
    """One launch's timing document, exactly as dl's json mode writes the line."""
    return f"{timing.JSON_PREFIX} " + json.dumps(
        {
            "total": total,
            "total_epoch": timing.TOTAL_EPOCH,
            "stages": [
                {"stage": name, "seconds": seconds, "outcome": "ok", "spans": []}
                for name, seconds in stages
            ],
        }
    )


def a_launch(tmp_path, *runs, exit_code: int = 0) -> list[str]:
    """A stand-in launch reporting one of *runs*' documents per invocation.

    It reports a document only when the environment asks for one, the way dl
    does, so a bench that never asked gets what the real thing would have
    given it: nothing to record. Runs past the last one described repeat it,
    and *exit_code* is how a launch that reported its stages and then failed
    anyway is asked for.
    """
    lines = [a_document(stages, total) for stages, total in runs]
    counted = tmp_path / "launches"
    counted.mkdir()
    source = (
        "import os, sys\n"
        f"lines = {lines!r}\n"
        f"counted = {str(counted)!r}\n"
        "run = len(os.listdir(counted))\n"
        "open(os.path.join(counted, str(run)), 'w').close()\n"
        f"if os.environ.get({timing.ENV_VAR!r}) == {timing.JSON_VALUE!r}:\n"
        "    print(lines[min(run, len(lines) - 1)], file=sys.stderr)\n"
        f"sys.exit({exit_code})\n"
    )
    return [sys.executable, "-c", source]


class TestBenchRecordsEachRunsStages:
    """#196: the bench writes one machine-readable record per invocation, so a
    published number stops being hand-copied out of prose."""

    def test_a_record_names_the_command_the_run_count_and_every_run(self, tmp_path):
        record = tmp_path / "bench.json"
        command = a_launch(tmp_path, ((("host-prep", 0.10), ("attach", 0.20)), 0.30))
        result = run_bench("-n", "3", "--record", str(record), "--", *command)
        assert result.returncode == 0, result.stderr
        written = json.loads(record.read_text())
        assert written["command"] == command
        assert written["n"] == 3
        assert len(written["runs"]) == 3

    def test_each_run_carries_the_stage_seconds_that_run_reported(self, tmp_path):
        """The record's point: the stages the launch itself measured, not one
        outside wall time a reader has to decompose by hand."""
        record = tmp_path / "bench.json"
        command = a_launch(tmp_path, ((("host-prep", 0.10), ("attach", 0.20)), 0.30))
        result = run_bench("-n", "2", "--record", str(record), "--", *command)
        assert result.returncode == 0, result.stderr
        runs = json.loads(record.read_text())["runs"]
        assert [run["stages"] for run in runs] == [{"host-prep": 0.10, "attach": 0.20}] * 2
        assert [run["total_seconds"] for run in runs] == [0.30, 0.30]

    def test_a_run_that_reported_no_document_is_not_a_record(self, tmp_path):
        """A wall time with no stages behind it is a silent contract break —
        an older dl, or a command that is not a launch at all. Better a loud
        failure at the bench than an empty point in the trend."""
        record = tmp_path / "bench.json"
        result = run_bench("-n", "2", "--record", str(record), "--", sys.executable, "-c", "pass")
        assert result.returncode != 0
        assert not record.exists()
        assert "Traceback" not in result.stderr

    def test_the_committed_point_is_the_median_of_the_runs(self, tmp_path):
        """The trend compares a point against the immediately previous point
        only (#197), so one bench invocation is one point: the median, with
        the runs kept underneath it as the evidence it was taken over."""
        record = tmp_path / "bench.json"
        command = a_launch(
            tmp_path,
            ((("devpod-up", 0.50),), 1.00),
            ((("devpod-up", 0.10),), 3.00),
            ((("devpod-up", 0.30),), 2.00),
        )
        result = run_bench("-n", "3", "--record", str(record), "--", *command)
        assert result.returncode == 0, result.stderr
        median = json.loads(record.read_text())["median"]
        assert median["stages"]["devpod-up"] == {"seconds": 0.30, "runs": 3}
        assert median["total_seconds"] == 2.00

    def test_a_stage_no_run_reported_is_absent_rather_than_zero(self, tmp_path):
        """A warm launch legitimately has no cold-path stages (#195), and a
        zero there would claim the work happened instantly instead of not at
        all — a fiction the trend has no way to tell from a reading."""
        record = tmp_path / "bench.json"
        command = a_launch(tmp_path, ((("devpod-up", 0.50), ("attach", 1.20)), 1.70))
        result = run_bench("-n", "2", "--record", str(record), "--", *command)
        assert result.returncode == 0, result.stderr
        written = json.loads(record.read_text())
        assert set(written["median"]["stages"]) == {"devpod-up", "attach"}
        assert all(set(run["stages"]) == {"devpod-up", "attach"} for run in written["runs"])

    def test_a_stage_only_some_runs_reported_is_a_median_over_those_runs(self, tmp_path):
        """Counting the missing runs as zero would drag the median toward a
        number no run measured, so the median is over the runs that reported
        the stage and says how many that was — a median of two and a median
        of five are not the same claim."""
        record = tmp_path / "bench.json"
        command = a_launch(
            tmp_path,
            ((("attach", 1.00),), 1.00),
            ((("attach", 1.00), ("tools", 4.00)), 5.00),
            ((("attach", 1.00), ("tools", 6.00)), 7.00),
        )
        result = run_bench("-n", "3", "--record", str(record), "--", *command)
        assert result.returncode == 0, result.stderr
        stages = json.loads(record.read_text())["median"]["stages"]
        assert stages["tools"] == {"seconds": 5.00, "runs": 2}
        assert stages["attach"] == {"seconds": 1.00, "runs": 3}

    def test_a_failed_run_writes_no_record_even_having_reported_stages(self, tmp_path):
        """The record is the artifact of a completed measurement, so the "no
        median over a failing command" discipline covers it: a launch that
        reported stages and then failed measured a failure."""
        record = tmp_path / "bench.json"
        command = a_launch(tmp_path, ((("attach", 1.00),), 1.00), exit_code=7)
        result = run_bench("-n", "2", "--record", str(record), "--", *command)
        assert result.returncode == 7
        assert not record.exists()

    def test_a_failed_reset_writes_no_record(self, tmp_path):
        record = tmp_path / "bench.json"
        command = a_launch(tmp_path, ((("attach", 1.00),), 1.00))
        result = run_bench("-n", "2", "--record", str(record), "--before", FAILING, "--", *command)
        assert result.returncode != 0
        assert not record.exists()

    def test_without_the_flag_nothing_about_the_run_changes(self, tmp_path):
        """Recording asks the launch for a document it would not otherwise
        emit, so an unrecorded bench must not be paying for the instrument —
        including on stdout, which #140's callers still read."""
        report = tmp_path / "asked"
        command = [
            sys.executable,
            "-c",
            f"import os; open({str(report)!r}, 'w').write(os.environ.get({timing.ENV_VAR!r}, '<unset>'))",
        ]
        result = run_bench("-n", "2", "--", *command)
        assert result.returncode == 0, result.stderr
        assert report.read_text() == "<unset>"
        assert re.search(r"^median of 2: \d+\.\d{3}s", result.stdout, re.M)

    def test_a_record_reads_a_document_the_timing_module_itself_wrote(self, tmp_path):
        """Every other test here hands the bench a document built to look like
        the launch's. This one has the launch's own emitter write it, so a
        field renamed in the document cannot leave the bench parsing a shape
        nothing emits any more."""
        record = tmp_path / "bench.json"
        emitting = (
            "from devlaunch import timing\n"
            "timing.begin()\n"
            "with timing.stage('devpod-up'):\n"
            "    pass\n"
            "timing.emit()\n"
        )
        result = run_bench("-n", "2", "--record", str(record), "--", sys.executable, "-c", emitting)
        assert result.returncode == 0, result.stderr
        written = json.loads(record.read_text())
        assert set(written["median"]["stages"]) == {"devpod-up"}
        assert written["median"]["stages"]["devpod-up"]["runs"] == 2
        assert written["median"]["total_seconds"] >= 0

    def test_the_record_names_the_shape_the_caller_benched(self, tmp_path):
        """warm and cold-recreate are two trend lines over the same command —
        the `--before` reset is what separates them, so the shape is a label
        the caller carries rather than something the runs reveal."""
        record = tmp_path / "bench.json"
        command = a_launch(tmp_path, ((("attach", 1.00),), 1.00))
        result = run_bench(
            "-n", "2", "--record", str(record), "--shape", "cold-recreate", "--", *command
        )
        assert result.returncode == 0, result.stderr
        assert json.loads(record.read_text())["shape"] == "cold-recreate"

    def test_an_unnamed_shape_is_absent_rather_than_guessed(self, tmp_path):
        record = tmp_path / "bench.json"
        command = a_launch(tmp_path, ((("attach", 1.00),), 1.00))
        assert run_bench("-n", "2", "--record", str(record), "--", *command).returncode == 0
        assert "shape" not in json.loads(record.read_text())

    def test_a_shape_with_nothing_to_record_it_in_is_refused(self, tmp_path):
        """Otherwise the label is silently dropped, which is how a trend line
        ends up unnamed at the far end of a CI job nobody watched."""
        command = a_launch(tmp_path, ((("attach", 1.00),), 1.00))
        result = run_bench("-n", "2", "--shape", "warm", "--", *command)
        assert result.returncode == 2
        assert "--record" in result.stderr
        assert "Traceback" not in result.stderr


class TestAMistypedStageNameFailsWhereItIsWritten:
    """The vocabulary is read from outside this repo, so a name that drifted
    out of it must fail at the site that wrote it — not on the first launch
    somebody happened to measure. A suite that runs with timing off, which is
    every suite, is exactly the run a typo used to survive."""

    def test_decorating_with_a_name_outside_the_vocabulary_is_refused(self):
        with pytest.raises(ValueError):

            @timing.staged("host_prep")
            def _launch():
                pass

    def test_every_name_in_the_vocabulary_decorates_and_calls_through(self):
        for name in timing.STAGES:
            assert timing.staged(name)(lambda: "ran")() == "ran"


def bench_module():
    """The bench script, imported by path — it is a script, not a package."""
    spec = importlib.util.spec_from_file_location("bench_launch", BENCH)
    assert spec is not None and spec.loader is not None, BENCH
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class TestEveryDocumentedInvocationParses:
    """Sibling of the cold-reset guard, for the flags rather than the reset.

    Same lesson from the same lineage (#192): every documented-command defect
    in this script's history was a command nobody had fed to the thing that
    reads it. So each invocation the epilog shows is handed to the real parser
    — a flag that was renamed, or documented before it existed, fails here.
    """

    def test_the_epilog_shows_invocations_the_parser_accepts(self):
        module = bench_module()
        joined = re.sub(r"\\\n\s+", " ", module.EPILOG)
        documented = re.findall(r"bench_launch\.py (.+)$", joined, re.M)
        assert len(documented) >= 3, documented
        for invocation in documented:
            module.build_parser().parse_args(shlex.split(invocation))
