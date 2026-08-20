"""The record `bench_launch.py` writes is the record `bench_points.py` reads (#221).

Two scripts, one JSON shape, and until now nothing that failed when they drifted.
`test_bench_points.py` drives the real reader against records written as
literals — deliberately, because what it has under test is the mapping. So the
reader is pinned against its own copy of the shape, and a field renamed in the
writer *and in the writer's own assertions* would leave that suite green while
the trend job silently stopped publishing.

This file is the seam itself: one record produced by the real writer, handed to
the real reader, with no hand-built dict anywhere between them. Nothing here
names a field of the record, on purpose — a third written copy of the schema
would drift like the other two. What it asserts is the numbers that come out
the far end, so the only way it passes is the two scripts agreeing about how
they got there.
"""

import json
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, Sequence, Tuple

from fixtures.bench_harness import a_launch, run_bench

POINTS = Path(__file__).parent.parent / "scripts" / "bench_points.py"

Run = Tuple[Sequence[Tuple[str, float]], float]

# A cold recreate as dl reports one: every stage `bench_points.py` requires by
# default, and a total they add up to. The numbers are literals so what the
# trend publishes is traceable to this file rather than recomputed the way
# either script computes it.
COLD: Run = ((("host-prep", 0.10), ("devpod-up", 4.00), ("tools", 1.20), ("attach", 0.60)), 5.90)


def run_points(*argv: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(POINTS), *argv], capture_output=True, text=True, check=False
    )


def published(tmp_path: Path, *runs: Run, shape: str = "cold-recreate") -> Dict[str, Any]:
    """Bench *runs*, publish the record that produced, return the points by name.

    Both ends are the shipped scripts, run as the CI job runs them: the bench
    writes the file, the converter is handed the path, and neither is reached
    into. `--require-stages-on` is passed because it is what CI passes, and it
    reads a part of the record the points alone do not.
    """
    record = tmp_path / f"{shape}.json"
    out = tmp_path / "bench.json"
    wrote = run_bench(
        "-n",
        str(len(runs)),
        "--record",
        str(record),
        "--shape",
        shape,
        "--",
        *a_launch(tmp_path, *runs),
    )
    assert wrote.returncode == 0, wrote.stderr
    read = run_points(str(record), "--out", str(out), "--require-stages-on", shape)
    assert read.returncode == 0, read.stderr
    return {point["name"]: point for point in json.loads(out.read_text())}


class TestARecordTheBenchWroteIsARecordTheConverterCanPublish:
    """The one thing neither suite could say on its own."""

    def test_every_stage_the_launch_reported_arrives_as_its_own_trend_case(self, tmp_path):
        """The decomposition survives the trip. A stage that reached the record
        under one name and is looked for under another produces no case at all,
        which in a chart is a line that quietly stops rather than an error."""
        points = published(tmp_path, COLD, COLD, COLD)
        assert set(points) == {
            "cold-recreate / host-prep",
            "cold-recreate / devpod-up",
            "cold-recreate / tools",
            "cold-recreate / attach",
            "cold-recreate / total",
        }

    def test_the_published_value_is_the_median_of_the_runs_the_bench_timed(self, tmp_path):
        """Three runs, one point, and the number on it is the middle one. The
        median is computed by the writer and read by the reader, so this is the
        one assertion that covers both halves of that handover."""
        points = published(
            tmp_path,
            ((("host-prep", 0.10), ("devpod-up", 1.00), ("tools", 1.20), ("attach", 0.60)), 2.90),
            ((("host-prep", 0.10), ("devpod-up", 5.00), ("tools", 1.20), ("attach", 0.60)), 6.90),
            ((("host-prep", 0.10), ("devpod-up", 3.00), ("tools", 1.20), ("attach", 0.60)), 4.90),
        )
        assert points["cold-recreate / devpod-up"]["value"] == 3.00
        assert points["cold-recreate / total"]["value"] == 4.90

    def test_the_spread_and_the_outside_stopwatch_ride_along_with_it(self, tmp_path):
        """`range` and `extra` are made of parts of the record the value is not:
        the per-run seconds behind the median, the count of runs that reported
        the stage, and the wall clock the bench took from outside the process.
        A point that published only its value would hide all three going stale."""
        points = published(tmp_path, COLD, COLD, COLD)
        total = points["cold-recreate / total"]
        assert total["range"] == "± 0.0"
        assert total["extra"].startswith("runs=3/3 wall=")
        assert points["cold-recreate / tools"]["extra"] == "runs=3/3"

    def test_a_stage_the_launch_never_reported_publishes_nothing_end_to_end(self, tmp_path):
        """A warm launch lends no tools. The absence has to survive both scripts
        as an absence — the writer must not record a zero and the reader must
        not invent one — because in the trend an instantaneous lend and no lend
        at all are the same flat line."""
        warm: Run = ((("host-prep", 0.02), ("devpod-up", 0.45), ("attach", 1.43)), 1.90)
        record = tmp_path / "warm.json"
        out = tmp_path / "bench.json"
        wrote = run_bench(
            "-n", "2", "--record", str(record), "--shape", "warm", "--", *a_launch(tmp_path, warm)
        )
        assert wrote.returncode == 0, wrote.stderr
        read = run_points(str(record), "--out", str(out))
        assert read.returncode == 0, read.stderr
        names = {point["name"] for point in json.loads(out.read_text())}
        assert "warm / tools" not in names
        assert "warm / attach" in names

    def test_a_shape_that_lost_a_stage_fails_the_job_on_a_real_record(self, tmp_path):
        """The requirement is the trend's tripwire, and it reads the per-stage
        run counts the writer put in the median rather than the points. Driven
        against a record the bench actually wrote, so the counts it compares are
        the counts the bench produced."""
        record = tmp_path / "cold-recreate.json"
        out = tmp_path / "bench.json"
        missing: Run = ((("host-prep", 0.10), ("devpod-up", 4.00), ("attach", 0.60)), 4.70)
        wrote = run_bench(
            "-n",
            "2",
            "--record",
            str(record),
            "--shape",
            "cold-recreate",
            "--",
            *a_launch(tmp_path, missing),
        )
        assert wrote.returncode == 0, wrote.stderr
        read = run_points(str(record), "--out", str(out), "--require-stages-on", "cold-recreate")
        assert read.returncode == 1
        assert "'tools'" in read.stderr
        assert not out.exists()
