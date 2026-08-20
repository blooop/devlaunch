#!/usr/bin/env python3
"""Turn bench records into the trend's points (ticket #198).

`bench_launch.py --record` writes one JSON object per bench invocation; the
trend (`benchmark-action/github-action-benchmark`, ticket #197) reads a flat
array of `{name, unit, value}`. This is the one step between them, and it is
deliberately the only place that knows both shapes.

Stdlib only, like its sibling: the instrument must not cost the project a
dependency. See --help for what it refuses to publish.
"""

import argparse
import json
import statistics
import sys
from typing import Any, Dict, List, Optional

EPILOG = """\
bench_points.py warm.json cold-recreate.json --out bench.json

CI: bench_points.py warm.json cold-recreate.json --out bench.json \\
        --require-stages-on cold-recreate --note 'devpod 0.26.1, ubuntu-latest'

One case per stage each record's shape reported, plus that shape's `total`:
`warm / host-prep`, `cold-recreate / devpod-up`, `warm / total`. The case name
is the trend's key, so it is an interface -- renaming a stage starts a new,
empty series next to a frozen old one.

The value is the record's MEDIAN, never a run: the trend compares a point
against the immediately previous point and nothing older, so N runs published
as N points would read ordinary spread as a regression. The spread those runs
had rides along as the point's `range`, and the outside stopwatch as part of
`extra` -- evidence beside the number rather than a second trend line.

Two things it refuses to publish, both of them absences that would otherwise
reach a chart looking like measurements:

  - A stage no run reported gets no point, never a zero. A warm launch lends
    no tools, and a zero there claims an instantaneous lend.
  - With --require-stages-on, a shape that lost a stage fails the JOB and
    writes no file at all. Point it at the shape where every stage is known
    to be present: a cold recreate that reports no `tools` means the launch
    changed shape, and a red job says so where a flattening trend line does
    not. Naming a shape that was not benched is itself a failure, for the
    same reason: an assertion that covers nothing reads like one that passed.

A record that is missing, unreadable or not a bench record takes the same
exit: `bench_points: <reason>`, and no output file. The step that writes the
records can go green without having written one, so that absence is the first
thing this reads and the first thing it can report.
"""

UNIT = "s"

# The clock the stages add up to. The record carries both this and the outside
# stopwatch; publishing both as cases would be two trend lines for one launch
# that disagree by a constant, so the other one rides in `extra` instead.
TOTAL = "total"

# What a shape under `--require-stages-on` must have reported, from every run.
#
# This is `Stage::name()` in `rust/devlaunch-core/src/timing.rs` minus `handoff`,
# written out rather than imported for the same reason its sibling writes out the
# timing variables: a script cannot import a Rust constant, and this one stays
# stdlib-only and runnable from a bare checkout. `test_bench_points.py` pins the
# two together -- reading the vocabulary out of that source -- so a rename there
# fails in the suite rather than quietly asserting a stage nothing emits.
#
# `handoff` is excluded because nothing hands off to dl on a runner: it is the
# gap before dl started, stamped by whoever launched it, and requiring it would
# fail every green CI run. It is also the one stage that lies outside `total`.
REQUIRED_STAGES = ("host-prep", "devpod-up", "tools", "attach")


def points_of(record: Dict[str, Any], note: Optional[str] = None) -> List[Dict[str, Any]]:
    """The trend points *record* makes, in the record's own stage order.

    A stage the record does not carry produces no point at all. That is #195's
    rule and it is the property the whole trend rests on: a warm launch lends
    no tools, and a zero there would claim the lend happened instantly rather
    than not at all -- indistinguishable, in a chart, from a lend that got
    faster.
    """
    shape = shape_of(record)
    median = record["median"]
    runs = record["runs"]
    total_runs = len(runs)
    points = [
        point(
            name=f"{shape} / {stage}",
            value=summary["seconds"],
            samples=[run["stages"][stage] for run in runs if stage in run["stages"]],
            extra=f"runs={summary['runs']}/{total_runs}",
            note=note,
        )
        for stage, summary in median["stages"].items()
    ]
    points.append(
        point(
            name=f"{shape} / {TOTAL}",
            value=median["total_seconds"],
            samples=[run["total_seconds"] for run in runs],
            extra=f"runs={total_runs}/{total_runs} wall={median['wall_seconds']}s",
            note=note,
        )
    )
    return points


class Refused(Exception):
    """Why nothing is being published.

    Raised rather than returned so that every refusal takes the same exit --
    a message and no output file, the discipline `bench_launch.py` already
    holds to for a failed run. Half a trend point written before the reason to
    refuse was found would be worse than none.
    """


def shape_of(record: Dict[str, Any]) -> str:
    """The trend line this record belongs to, or a refusal.

    #196 refuses to guess a shape from the runs -- the same command benches
    warm or cold depending on a reset it cannot interpret -- so an unlabelled
    record has no case name to publish under, and inventing one here would put
    the guess back in at the last possible moment.
    """
    shape = record.get("shape")
    if not shape:
        raise Refused(
            "this record carries no shape, so its points have no trend line to "
            "belong to; re-run the bench with --shape"
        )
    return str(shape)


def load_record(path: str) -> Dict[str, Any]:
    """The bench record at *path*, or a refusal saying which one and why.

    Reading is a refusal like every other one here, and it earns that the same
    way: the bench step upstream can go green without writing its record at all
    -- a mangled `--before` exited the bench 2 and the step passed anyway (run
    31840842480) -- and what this script printed then was a raw traceback whose
    first readable line named a Python builtin rather than the absent file. A
    missing record is a fact about the step before this one, so it is reported
    like one instead of raised through it.
    """
    try:
        with open(path, encoding="utf-8") as handle:
            document = json.load(handle)
    except OSError as unreadable:
        raise Refused(
            f"cannot read the record {path}: {unreadable.strerror}. The bench step that "
            "writes it can exit 0 without having written it, so this is a report about "
            "that step rather than about this one"
        ) from unreadable
    except ValueError as unparsable:
        raise Refused(
            f"the record {path} is not readable JSON ({unparsable}); a bench interrupted "
            "mid-write leaves exactly this"
        ) from unparsable
    if not isinstance(document, dict):
        raise Refused(
            f"the record {path} is a {type(document).__name__}, not the one JSON object "
            "per bench invocation this reads"
        )
    missing = [key for key in ("median", "runs") if key not in document]
    if missing:
        raise Refused(
            f"the record {path} carries no {' and no '.join(missing)}, which is what its "
            "points are made of -- so there is no point in it to publish"
        )
    return document


def check_one_record_per_shape(records: List[Dict[str, Any]]) -> None:
    """Refuse two records of the same shape.

    The case name is the trend's key, so two `warm` records publish two points
    per case for one commit and the chart has no way to say which one the
    commit's point is.
    """
    seen = set()
    for record in records:
        shape = shape_of(record)
        if shape in seen:
            raise Refused(
                f"two records both labelled {shape!r}: the shape names the trend line, "
                "so publishing both would put two points per case on one commit"
            )
        seen.add(shape)


def check_required(records: List[Dict[str, Any]], shapes: List[str], required: List[str]) -> None:
    """Refuse unless every named shape reported every required stage.

    The expected way this decomposition breaks is an absence, not a wrong
    number: dl stops entering an arm, or devpod renames what it prints, and a
    stage quietly stops appearing while the total keeps working. So the shape
    where every stage is known to be present asserts them, and a run that lost
    one fails the *job* -- publishing nothing at all, rather than a trend line
    that flattens where nobody is looking.

    A named shape that was not benched is itself a refusal, for the reason
    this repo's CI gate already gives about covering nothing: an assertion
    that matched no shape reads exactly like one that passed.
    """
    benched = {shape_of(record): record for record in records}
    for shape in shapes:
        record = benched.get(shape)
        if record is None:
            raise Refused(
                f"--require-stages-on names {shape!r}, which no record here benched "
                f"(benched: {', '.join(sorted(benched)) or 'nothing'}); an assertion "
                "that covers nothing reads exactly like one that passed"
            )
        runs = len(record["runs"])
        stages = record["median"]["stages"]
        for stage in required:
            reported = stages.get(stage, {}).get("runs", 0)
            if reported < runs:
                raise Refused(
                    f"the {shape!r} shape reported {stage!r} in {reported} of its {runs} "
                    "runs, and it is required in all of them -- so the launch changed "
                    "shape or the stage stopped being emitted. Publishing nothing: a "
                    "trend line that goes flat is harder to notice than a red job"
                )


def point(
    name: str, value: float, samples: List[float], extra: str, note: Optional[str]
) -> Dict[str, Any]:
    """One case in the trend's own shape.

    *samples* are the runs the median is of, and they are here for the error
    bar alone. Two of them are the fewest there is a spread of, so a single
    run publishes no range rather than a `± 0` claiming a repeatability
    nothing measured.
    """
    published: Dict[str, Any] = {"name": name, "unit": UNIT, "value": value}
    if len(samples) > 1:
        published["range"] = f"± {round(statistics.stdev(samples), 6)}"
    published["extra"] = f"{extra} {note}" if note else extra
    return published


def write_points(path: str, points: List[Dict[str, Any]]) -> None:
    with open(path, "w", encoding="utf-8") as handle:
        # The `±` in a range stays a `±` rather than an escape: this file is
        # uploaded as an artifact and read by a human after an alert.
        json.dump(points, handle, indent=2, ensure_ascii=False)
        handle.write("\n")


def build_parser() -> argparse.ArgumentParser:
    """The command line, built where a test can also ask it what it accepts."""
    parser = argparse.ArgumentParser(
        description="Turn bench records into the trend's points.",
        epilog=EPILOG,
        formatter_class=argparse.RawDescriptionHelpFormatter,
        allow_abbrev=False,
    )
    parser.add_argument("records", nargs="+", metavar="RECORD", help="bench records to publish")
    parser.add_argument("--out", required=True, metavar="PATH", help="where to write the points")
    parser.add_argument(
        "--note",
        metavar="TEXT",
        help="appended to every point's `extra` (e.g. the devpod version and the runner)",
    )
    parser.add_argument(
        "--require-stages-on",
        metavar="SHAPE",
        action="append",
        default=[],
        dest="require_stages_on",
        help=(
            "publish nothing unless this shape reported every required stage in every "
            "run; name the shape where they are all known to be present (cold-recreate)"
        ),
    )
    parser.add_argument(
        "--require-stages",
        metavar="A,B",
        default=",".join(REQUIRED_STAGES),
        help=f"what --require-stages-on requires (default: {','.join(REQUIRED_STAGES)})",
    )
    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        records = [load_record(path) for path in args.records]
        check_one_record_per_shape(records)
        check_required(
            records, args.require_stages_on, [s for s in args.require_stages.split(",") if s]
        )
        published: List[Dict[str, Any]] = []
        for record in records:
            published.extend(points_of(record, note=args.note))
    except Refused as refusal:
        print(f"bench_points: {refusal}", file=sys.stderr)
        return 1
    write_points(args.out, published)
    return 0


if __name__ == "__main__":
    sys.exit(main())
