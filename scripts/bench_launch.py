#!/usr/bin/env python3
"""Run a command N times and report the median wall time (ticket #140).

The repeatable half of the timing instrument: a before/after perf comparison is
this script run once on each side of a change. Stdlib only, on purpose — the
instrument must not cost the project a dependency. See --help for the two
traps: cold medians, and which clock each number is on.
"""

import argparse
import json
import os
import shlex
import statistics
import subprocess
import sys
import time
from typing import Any, Dict, List, Optional

EPILOG = """\
warm:  bench_launch.py -n 5 -- dl-next owner/repo -- true

cold:  export DEVPOD_HOME=/tmp/dl-bench/devpod \\
              DEVPOD_SSH_CONFIG=/tmp/dl-bench/ssh_config \\
              XDG_CACHE_HOME=/tmp/dl-bench/cache
       pixi run dev-add-docker    # a fresh devpod home has no provider
       bench_launch.py -n 5 --before 'dl-next owner/repo rm --force' \\
              -- dl-next owner/repo -- true

record: bench_launch.py -n 5 --record warm.json --shape warm \\
              -- dl-next owner/repo -- true

A cold median needs the reset per *run*: delete once and bench five times and
runs 2..5 are warm, so the median is a warm number under a cold label. The
reset is `rm --force`, which also succeeds when there is nothing to remove
yet -- the state the first run starts from. A failed reset stops with no
median, like a failed run: a run whose starting condition was never
established is not a measurement of it.

What the scoping does and does not do: the workspace id is derived from
owner/repo@branch alone, so the bench's workspace is the SAME id as your real
one -- no cache variable changes that. What keeps the bench's workspaces in a
namespace of their own is DEVPOD_HOME (plus DEVPOD_SSH_CONFIG, which `devpod
up` writes outside that home) -- the same two variables the test suite scopes
in test/devpod_scoping.py. XDG_CACHE_HOME moves only devlaunch's clone cache
and bookkeeping. The containers are still real docker containers on this
machine. The bare-repo cache survives the reset on purpose: "cold" here means
no workspace, no worktree, no container -- not a fresh network clone per run.

--record writes one JSON object for the whole invocation: the command, the run
count, every run's wall time and per-stage seconds, and the medians of those.
It asks the launch for its stages itself (DEVLAUNCH_TIMING=json), so a run that
reports no timing document is an error and no record, exactly as a failing run
is no median. The median is the published number, not the runs -- a trend
compares a point against the immediately previous point, so N runs published as
N points would read ordinary spread as a regression. A stage no run reported is
absent rather than zero, and a stage only some runs reported is a median over
those runs with a count saying how many. --shape labels which launch shape was
benched (warm, cold-recreate); nothing is guessed from the runs if it is
omitted. Nothing else goes in: a CI job stamps its own commit and clock better
than this script can. Finding the document means capturing the run's stderr, so
a recorded run shows the launch's own chatter only when the run failed, and the
whole of it then; stdout is untouched either way.

This clock starts outside the process, so it includes interpreter startup and
imports; dl's own `dl-timing: total` starts inside main() and excludes them
(on `dl --version`: 0.001s total against a 0.061s median here). Quote one
instrument on both sides of a change. The record carries both, as
`wall_seconds` and `total_seconds` -- per run and in the median.
"""


# How the launch is asked for its stages, and how its answer is found again in
# a stderr that also carries devpod's chatter. These three are devlaunch's, not
# this script's -- see `devlaunch/timing.py` -- but the script stays stdlib-only
# and importable from a checkout that was never installed, so it names them
# rather than importing them. `test_timing.py` drives this script against a
# stand-in launch built from timing.py's own constants, so a rename there fails
# here rather than quietly recording nothing.
TIMING_VAR = "DEVLAUNCH_TIMING"
TIMING_DOCUMENT = "json"
DOCUMENT_PREFIX = "dl-timing-json:"


def recording_env() -> Dict[str, str]:
    """This process's environment, with the launch asked for its document.

    Overriding rather than deferring to an exported `DEVLAUNCH_TIMING=1`: the
    prose summary is not what a record is made of, and a bench run that
    silently recorded nothing because of the developer's shell would be found
    much later, in the trend.
    """
    env = dict(os.environ)
    env[TIMING_VAR] = TIMING_DOCUMENT
    return env


def stage_document(stderr: str) -> Optional[dict]:
    """The one timing document *stderr* carries, or None if it carries no
    readable one -- which is the same answer for a launch too old to emit one
    and for output that was never a document."""
    found = [
        line[len(DOCUMENT_PREFIX) :]
        for line in stderr.splitlines()
        if line.startswith(DOCUMENT_PREFIX)
    ]
    if len(found) != 1:
        return None
    try:
        document = json.loads(found[0])
    except ValueError:
        return None
    return document if isinstance(document, dict) and "total" in document else None


def bench(
    n: int,
    command: List[str],
    before: Optional[List[str]] = None,
    record: Optional[str] = None,
    shape: Optional[str] = None,
) -> int:
    """Run *command* *n* times, print each wall time and the median.

    Returns 0 after n clean runs, otherwise the exit code of the first run --
    or untimed *before* reset -- that failed. With *record* given, the same
    runs are also written there as one JSON record; without it nothing about
    the run changes, down to the stdout.
    """
    durations = []
    runs: List[Dict[str, Any]] = []
    for i in range(1, n + 1):
        if before is not None:
            # nosec B603 - list form, not shell=True; the reset is the user's own argv
            reset = subprocess.run(before, check=False)
            if reset.returncode != 0:
                print(
                    f"reset before run {i}/{n} exited {reset.returncode}; "
                    "no median over runs whose starting state was not established",
                    file=sys.stderr,
                )
                return reset.returncode
        start = time.perf_counter()
        # nosec B603 - list form, not shell=True; the command is the user's own argv
        result = subprocess.run(
            command,
            check=False,
            env=recording_env() if record is not None else None,
            stderr=subprocess.PIPE if record is not None else None,
            text=True,
        )
        elapsed = time.perf_counter() - start
        if result.returncode != 0:
            if result.stderr:
                # Captured for the document, so it has to be handed back: a
                # failing run the bench swallowed the error of is worse than
                # no bench.
                sys.stderr.write(result.stderr)
            print(
                f"run {i}/{n} exited {result.returncode}; no median over a failing command",
                file=sys.stderr,
            )
            return result.returncode
        print(f"run {i}/{n}: {elapsed:.3f}s")
        durations.append(elapsed)
        if record is not None:
            document = stage_document(result.stderr)
            if document is None:
                sys.stderr.write(result.stderr)
                print(
                    f"run {i}/{n} reported no {DOCUMENT_PREFIX} document; "
                    "nothing to record its stages from",
                    file=sys.stderr,
                )
                return 1
            runs.append(
                {
                    "wall_seconds": round(elapsed, 6),
                    "total_seconds": document["total"],
                    "stages": {
                        stage["stage"]: stage["seconds"] for stage in document.get("stages", [])
                    },
                }
            )
    median = statistics.median(durations)
    print(f"median of {n}: {median:.3f}s (wall clock, including interpreter startup)")
    if record is not None:
        write_record(record, command=command, runs=runs, shape=shape)
    return 0


def median_of(runs: List[Dict[str, Any]]) -> Dict[str, Any]:
    """The one point these runs make: a median per quantity they reported.

    The trend's baseline is the immediately previous point and nothing older
    (#197), so what gets published has to be the median already -- N runs
    published as N points would compare a run against the run before it and
    call ordinary spread a regression.
    """
    stages: Dict[str, List[float]] = {}
    for run in runs:
        for name, seconds in run["stages"].items():
            stages.setdefault(name, []).append(seconds)
    return {
        "wall_seconds": round(statistics.median([run["wall_seconds"] for run in runs]), 6),
        "total_seconds": round(statistics.median([run["total_seconds"] for run in runs]), 6),
        "stages": {
            name: {"seconds": round(statistics.median(seconds), 6), "runs": len(seconds)}
            for name, seconds in stages.items()
        },
    }


def write_record(
    path: str, command: List[str], runs: List[Dict[str, Any]], shape: Optional[str] = None
) -> None:
    """Write one bench invocation as one JSON object at *path*.

    Deliberately boring: what was run, how many times, what each run cost and
    what the medians of those costs are. No timestamp, no commit, no host --
    a trend job knows all three about itself better than this script can, and
    a field it would have to overwrite is a field that can disagree with it.
    """
    document = {
        "command": list(command),
        "n": len(runs),
        "runs": runs,
        "median": median_of(runs),
    }
    if shape is not None:
        # Absent rather than guessed when the caller did not say: the same
        # command benches warm or cold depending on a `--before` this script
        # is in no position to interpret, and a wrong label is worse in a
        # trend than a missing one.
        document["shape"] = shape
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(document, handle, indent=2, sort_keys=True)
        handle.write("\n")


def positive_int(text: str) -> int:
    """A run count there is a median of: one or more."""
    value = int(text)
    if value < 1:
        raise argparse.ArgumentTypeError(f"need at least one run, got {value}")
    return value


def build_parser() -> argparse.ArgumentParser:
    """The command line, built where a test can also ask it what it accepts.

    Separate from :func:`main` so the documented invocations in EPILOG can be
    parsed without being run: every documented-command defect in this script's
    history was a command nobody had ever fed to the thing that reads it.
    """
    parser = argparse.ArgumentParser(
        description="Run a command N times and report the median wall time.",
        epilog=EPILOG,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("-n", type=positive_int, default=5, help="number of runs (default: 5)")
    parser.add_argument(
        "--before",
        metavar="RESET",
        help=(
            "shell-quoted command to run before each timed run, untimed; "
            "use it to re-establish a cold launch's starting state every run"
        ),
    )
    parser.add_argument(
        "--record",
        metavar="PATH",
        help="also write this invocation's runs, per-stage seconds and medians there as JSON",
    )
    parser.add_argument(
        "--shape",
        metavar="NAME",
        help=(
            "the launch shape this invocation benched (e.g. warm, cold-recreate), "
            "recorded as the label of the trend line it belongs to"
        ),
    )
    parser.add_argument(
        "command",
        nargs=argparse.REMAINDER,
        metavar="-- command...",
        help="the command to time, after a `--`",
    )
    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("no command given; usage: bench_launch.py -n 5 -- dl <ws> -- true")
    before = shlex.split(args.before) if args.before else None
    if args.before and not before:
        parser.error("--before given an empty command")
    if args.shape and args.record is None:
        parser.error("--shape has nothing to name without --record")
    return bench(args.n, command, before, record=args.record, shape=args.shape)


if __name__ == "__main__":
    sys.exit(main())
