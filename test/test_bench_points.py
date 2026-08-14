"""The trend points a bench record makes (#198).

`bench_launch.py --record` (#196) writes one JSON object per bench invocation;
`benchmark-action/github-action-benchmark` (#197) reads a flat array of
`{name, unit, value, range, extra}`. This is the seam between them, and it is
where every rule the two blockers settled actually has teeth: the median is the
point, an absent stage is absent rather than zero, and a stage that has gone
missing on the shape it is known to appear on fails the *job* rather than
publishing a hole in the trend.

Records here are written as literals, medians included, rather than computed by
running a bench: what is under test is the mapping, and a test that recomputed
the median the way the converter does would assert nothing about it.
"""

import importlib.util
import json
import re
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict

POINTS = Path(__file__).parent.parent / "scripts" / "bench_points.py"


def run_points(*argv: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(POINTS), *argv], capture_output=True, text=True, check=False
    )


def write_record(path: Path, document: Dict[str, Any]) -> str:
    path.write_text(json.dumps(document, indent=2), encoding="utf-8")
    return str(path)


# A warm launch as #196 records one: no `tools` stage, because a warm launch
# lends nothing, and no `handoff`, because nothing handed off to it. The
# medians are the published numbers and are written here as literals.
WARM: Dict[str, Any] = {
    "command": ["dl", "owner/repo", "--", "true"],
    "n": 3,
    "shape": "warm",
    "runs": [
        {
            "wall_seconds": 2.0,
            "total_seconds": 1.9,
            "stages": {"host-prep": 0.02, "devpod-up": 0.45, "attach": 1.43},
        },
        {
            "wall_seconds": 2.2,
            "total_seconds": 2.1,
            "stages": {"host-prep": 0.03, "devpod-up": 0.46, "attach": 1.61},
        },
        {
            "wall_seconds": 2.4,
            "total_seconds": 2.3,
            "stages": {"host-prep": 0.04, "devpod-up": 0.47, "attach": 1.79},
        },
    ],
    "median": {
        "wall_seconds": 2.2,
        "total_seconds": 2.1,
        "stages": {
            "host-prep": {"seconds": 0.03, "runs": 3},
            "devpod-up": {"seconds": 0.46, "runs": 3},
            "attach": {"seconds": 1.61, "runs": 3},
        },
    },
}

# The other shape the charter benches, with every non-handoff stage present.
COLD: Dict[str, Any] = {
    "command": ["dl", "owner/repo", "--", "true"],
    "n": 3,
    "shape": "cold-recreate",
    "runs": [
        {
            "wall_seconds": 16.0,
            "total_seconds": 15.9,
            "stages": {"host-prep": 2.2, "devpod-up": 5.8, "tools": 10.2, "attach": 2.5},
        },
        {
            "wall_seconds": 16.0,
            "total_seconds": 15.9,
            "stages": {"host-prep": 2.2, "devpod-up": 5.8, "tools": 10.2, "attach": 2.5},
        },
        {
            "wall_seconds": 16.0,
            "total_seconds": 15.9,
            "stages": {"host-prep": 2.2, "devpod-up": 5.8, "tools": 10.2, "attach": 2.5},
        },
    ],
    "median": {
        "wall_seconds": 16.0,
        "total_seconds": 15.9,
        "stages": {
            "host-prep": {"seconds": 2.2, "runs": 3},
            "devpod-up": {"seconds": 5.8, "runs": 3},
            "tools": {"seconds": 10.2, "runs": 3},
            "attach": {"seconds": 2.5, "runs": 3},
        },
    },
}


class TestARecordBecomesTheShapesTrendPoints:
    """One case per stage the shape reported, plus the shape's own total."""

    def test_every_stage_and_the_total_are_named_by_their_shape(self, tmp_path):
        record = write_record(tmp_path / "warm.json", WARM)
        out = tmp_path / "bench.json"
        result = run_points(record, "--out", str(out))
        assert result.returncode == 0, result.stderr
        points = json.loads(out.read_text())
        assert [point["name"] for point in points] == [
            "warm / host-prep",
            "warm / devpod-up",
            "warm / attach",
            "warm / total",
        ]

    def test_the_published_value_is_the_median_in_seconds(self, tmp_path):
        record = write_record(tmp_path / "warm.json", WARM)
        out = tmp_path / "bench.json"
        assert run_points(record, "--out", str(out)).returncode == 0
        points = json.loads(out.read_text())
        assert [point["value"] for point in points] == [0.03, 0.46, 1.61, 2.1]
        assert {point["unit"] for point in points} == {"s"}

    def test_the_total_is_the_in_process_clock_with_the_wall_clock_beside_it(self, tmp_path):
        """Two clocks measure a launch and they are not the same quantity, so
        one of them is the trend line and the other is evidence next to it.
        The stages sum to `total_seconds`, so that is the one the chart plots;
        the outside stopwatch rides along in `extra` where it cannot be read
        as a second, disagreeing trend."""
        record = write_record(tmp_path / "warm.json", WARM)
        out = tmp_path / "bench.json"
        assert run_points(record, "--out", str(out)).returncode == 0
        total = json.loads(out.read_text())[-1]
        assert total["value"] == 2.1
        assert "wall=2.2s" in total["extra"]

    def test_several_records_publish_side_by_side_in_the_order_given(self, tmp_path):
        warm = write_record(tmp_path / "warm.json", WARM)
        cold = write_record(tmp_path / "cold.json", COLD)
        out = tmp_path / "bench.json"
        assert run_points(warm, cold, "--out", str(out)).returncode == 0
        names = [point["name"] for point in json.loads(out.read_text())]
        assert names[:4] == [
            "warm / host-prep",
            "warm / devpod-up",
            "warm / attach",
            "warm / total",
        ]
        assert names[4:] == [
            "cold-recreate / host-prep",
            "cold-recreate / devpod-up",
            "cold-recreate / tools",
            "cold-recreate / attach",
            "cold-recreate / total",
        ]


class TestAnAbsentStageIsAbsent:
    """#195's rule, and the reason the whole trend is trustworthy: a stage
    that did not run has no point, never a zero. A warm launch lends no tools;
    a zero there would claim the lend happened instantly and would drag the
    line down exactly where a regression should show."""

    def test_a_stage_no_run_reported_gets_no_point_rather_than_a_zero(self, tmp_path):
        record = write_record(tmp_path / "warm.json", WARM)
        out = tmp_path / "bench.json"
        assert run_points(record, "--out", str(out)).returncode == 0
        points = json.loads(out.read_text())
        assert "warm / tools" not in [point["name"] for point in points]
        assert 0 not in [point["value"] for point in points]

    def test_a_stage_only_some_runs_reported_says_how_many_it_is_a_median_of(self, tmp_path):
        """A median of two and a median of five are not the same claim, and
        the difference is invisible in a chart unless the point carries it."""
        partial: Dict[str, Any] = dict(WARM)
        partial["runs"] = [
            dict(run, stages=dict(run["stages"], **({"tools": 9.0} if index else {})))
            for index, run in enumerate(WARM["runs"])
        ]
        partial["median"] = dict(
            WARM["median"],
            stages=dict(WARM["median"]["stages"], tools={"seconds": 9.0, "runs": 2}),
        )
        record = write_record(tmp_path / "warm.json", partial)
        out = tmp_path / "bench.json"
        assert run_points(record, "--out", str(out)).returncode == 0
        tools = next(p for p in json.loads(out.read_text()) if p["name"] == "warm / tools")
        assert tools["value"] == 9.0
        assert "runs=2/3" in tools["extra"]


class TestThePointCarriesTheSpreadBehindIt:
    """The median is the point; the runs are its evidence. A chart with no
    error bar on a quantity that spreads 15-20s on a quiet host invites
    reading noise as movement."""

    def test_the_range_is_the_spread_of_the_runs_the_median_is_of(self, tmp_path):
        record = write_record(tmp_path / "warm.json", WARM)
        out = tmp_path / "bench.json"
        assert run_points(record, "--out", str(out)).returncode == 0
        ranges = {p["name"]: p["range"] for p in json.loads(out.read_text())}
        assert ranges["warm / host-prep"] == "± 0.01"
        assert ranges["warm / total"] == "± 0.2"

    def test_one_run_has_no_spread_and_so_publishes_no_range(self, tmp_path):
        single: Dict[str, Any] = dict(WARM, n=1, runs=WARM["runs"][:1])
        single["median"] = {
            "wall_seconds": 2.0,
            "total_seconds": 1.9,
            "stages": {"host-prep": {"seconds": 0.02, "runs": 1}},
        }
        record = write_record(tmp_path / "single.json", single)
        out = tmp_path / "bench.json"
        assert run_points(record, "--out", str(out)).returncode == 0
        assert all("range" not in point for point in json.loads(out.read_text()))


def without(record: Dict[str, Any], stage: str) -> Dict[str, Any]:
    """*record* as a launch that never reported *stage* — the shape a devpod
    upgrade that renamed a log line would leave behind."""
    return dict(
        record,
        runs=[
            dict(run, stages={k: v for k, v in run["stages"].items() if k != stage})
            for run in record["runs"]
        ],
        median=dict(
            record["median"],
            stages={k: v for k, v in record["median"]["stages"].items() if k != stage},
        ),
    )


class TestAShapeThatLostAStageFailsTheJob:
    """#195's discipline, applied to the vocabulary that exists.

    The expected failure of measuring somebody else's process is not a wrong
    number, it is an absence: a devpod upgrade renames what it prints, or dl
    stops entering an arm, and the stage silently goes missing while the total
    keeps working. That is the good failure only if it is loud — so the shape
    where every stage is known to be present asserts them, and a run that lost
    one fails *the job*, publishing nothing, rather than quietly flattening a
    trend line nobody is watching that week.
    """

    def test_a_stage_missing_from_the_shape_it_is_required_on_is_refused(self, tmp_path):
        record = write_record(tmp_path / "cold.json", without(COLD, "tools"))
        out = tmp_path / "bench.json"
        result = run_points(record, "--out", str(out), "--require-stages-on", "cold-recreate")
        assert result.returncode != 0
        assert "tools" in result.stderr
        assert "Traceback" not in result.stderr
        assert not out.exists(), "a run that refused to publish must publish nothing"

    def test_a_stage_only_some_runs_reported_is_refused_on_that_shape(self, tmp_path):
        """Half a stage is the same story mid-way through: it is going, and
        the point that would be published is a median of a different N than
        the one beside it."""
        partial: Dict[str, Any] = dict(
            COLD,
            median=dict(
                COLD["median"],
                stages=dict(COLD["median"]["stages"], tools={"seconds": 10.2, "runs": 2}),
            ),
        )
        record = write_record(tmp_path / "cold.json", partial)
        out = tmp_path / "bench.json"
        result = run_points(record, "--out", str(out), "--require-stages-on", "cold-recreate")
        assert result.returncode != 0
        assert "tools" in result.stderr
        assert "Traceback" not in result.stderr
        assert not out.exists()

    def test_another_shape_may_legitimately_lack_what_that_one_requires(self, tmp_path):
        """A warm launch lends nothing, so requiring `tools` of it would fail
        every green run. The assertion is scoped to the shape, not the trend."""
        warm = write_record(tmp_path / "warm.json", WARM)
        cold = write_record(tmp_path / "cold.json", COLD)
        out = tmp_path / "bench.json"
        result = run_points(warm, cold, "--out", str(out), "--require-stages-on", "cold-recreate")
        assert result.returncode == 0, result.stderr
        assert len(json.loads(out.read_text())) == 9

    def test_a_requirement_that_matches_no_benched_shape_is_itself_a_failure(self, tmp_path):
        """Same argument this repo's CI gate already makes: a check that
        covers nothing reads exactly like a check that passed. A misspelt
        shape here would leave the assertion asserting nothing, silently."""
        record = write_record(tmp_path / "cold.json", COLD)
        out = tmp_path / "bench.json"
        result = run_points(record, "--out", str(out), "--require-stages-on", "cold-recreat")
        assert result.returncode != 0
        assert "cold-recreat" in result.stderr
        assert "Traceback" not in result.stderr
        assert not out.exists()

    def test_the_handoff_stage_is_not_required_of_anything(self, tmp_path):
        """Nothing hands off to dl in CI, so `handoff` is legitimately absent
        from every shape a runner benches — and it is also the one stage that
        lies outside the total."""
        record = write_record(tmp_path / "cold.json", COLD)
        out = tmp_path / "bench.json"
        result = run_points(record, "--out", str(out), "--require-stages-on", "cold-recreate")
        assert result.returncode == 0, result.stderr
        assert "cold-recreate / handoff" not in [p["name"] for p in json.loads(out.read_text())]


class TestWhatItRefusesToPublish:
    def test_a_record_with_no_shape_cannot_name_a_trend_line(self, tmp_path):
        """`--shape` is the caller's to say and #196 refuses to guess it, so
        an unlabelled record has no case name to publish under."""
        unlabelled = {k: v for k, v in WARM.items() if k != "shape"}
        record = write_record(tmp_path / "warm.json", unlabelled)
        out = tmp_path / "bench.json"
        result = run_points(record, "--out", str(out))
        assert result.returncode != 0
        assert "shape" in result.stderr
        assert "Traceback" not in result.stderr
        assert not out.exists()

    def test_a_record_that_was_never_written_names_the_file_it_wanted(self, tmp_path):
        """The step before this one can pass without writing its record — run
        31840842480 is a bench whose arguments were mangled, whose script
        exited 2, and whose step went green anyway. This converter is where
        that surfaces, so it has to surface as a sentence naming the missing
        record, like every other refusal, rather than as a traceback."""
        out = tmp_path / "bench.json"
        result = run_points(str(tmp_path / "cold-recreate.json"), "--out", str(out))
        assert result.returncode != 0
        assert "cold-recreate.json" in result.stderr
        assert "Traceback" not in result.stderr
        assert result.stderr.startswith("bench_points:")
        assert not out.exists()

    def test_a_record_that_is_not_readable_json_is_refused(self, tmp_path):
        """A truncated record is what an interrupted bench leaves behind, and
        it is an absence like any other."""
        record = tmp_path / "warm.json"
        record.write_text('{"shape": "warm", "runs": [', encoding="utf-8")
        out = tmp_path / "bench.json"
        result = run_points(str(record), "--out", str(out))
        assert result.returncode != 0
        assert "warm.json" in result.stderr
        assert "Traceback" not in result.stderr
        assert result.stderr.startswith("bench_points:")
        assert not out.exists()

    def test_a_record_without_the_runs_and_median_it_publishes_is_refused(self, tmp_path):
        """Readable JSON is not yet a bench record. These two keys are what
        every point is made of, so a document missing either has nothing to
        publish and says so once — rather than raising a KeyError further in,
        at a line that names neither the file nor the reason."""
        for missing in ("median", "runs"):
            record = write_record(
                tmp_path / f"{missing}.json", {k: v for k, v in WARM.items() if k != missing}
            )
            out = tmp_path / f"{missing}-bench.json"
            result = run_points(record, "--out", str(out))
            assert result.returncode != 0, missing
            assert missing in result.stderr
            assert "Traceback" not in result.stderr
            assert result.stderr.startswith("bench_points:")
            assert not out.exists()

    def test_two_records_of_the_same_shape_would_collide(self, tmp_path):
        """The case name is the trend's key. Two records both labelled `warm`
        publish two points per case in one commit, and the chart has no way to
        say which is the point."""
        first = write_record(tmp_path / "one.json", WARM)
        second = write_record(tmp_path / "two.json", WARM)
        out = tmp_path / "bench.json"
        result = run_points(first, second, "--out", str(out))
        assert result.returncode != 0
        assert "warm" in result.stderr
        assert "Traceback" not in result.stderr
        assert not out.exists()


def points_module():
    """The converter, imported by path — it is a script, not a package."""
    spec = importlib.util.spec_from_file_location("bench_points", POINTS)
    assert spec is not None and spec.loader is not None, POINTS
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class TestTheVocabularyItAssertsIsTheOneDlEmits:
    def test_every_stage_but_the_handoff_is_required_by_default(self):
        """Written out in the script rather than imported, like its sibling,
        so it stays runnable from a checkout that was never installed — which
        makes this the test that catches the two drifting apart."""
        from devlaunch import timing  # pylint: disable=import-outside-toplevel

        module = points_module()
        assert module.REQUIRED_STAGES == tuple(
            stage for stage in timing.STAGES if stage != timing.HANDOFF_STAGE
        )


class TestEveryDocumentedInvocationParses:
    """Sibling of the same guard on the bench script (#192): every documented
    invocation is handed to the real parser, so a flag renamed or documented
    before it existed fails here."""

    def test_the_epilog_shows_invocations_the_parser_accepts(self):
        module = points_module()
        joined = re.sub(r"\\\n\s+", " ", module.EPILOG)
        documented = re.findall(r"bench_points\.py (.+)$", joined, re.M)
        assert documented, module.EPILOG
        for invocation in documented:
            module.build_parser().parse_args(shlex.split(invocation))
