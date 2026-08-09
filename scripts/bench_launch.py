#!/usr/bin/env python3
"""Run a command N times and report the median wall time.

The repeatable half of the timing instrument (ticket #140): a before/after
perf comparison is this script run once on each side of a change, e.g.

    python scripts/bench_launch.py -n 5 -- dl-next owner/repo -- true

Wall time per run and the median go to stdout; the command's own output is
left on the terminal so a hung launch is visible. If any run exits non-zero
the bench stops with that run's exit code and reports no median — a failing
launch's time is not a number to compare against.

`--before` runs a reset command before each timed run, without counting its
time. That is what makes a *cold* median possible: the state a cold launch
starts from has to be re-established before every run, or runs 2..N are warm
and the median is a warm number under a cold label. If a reset fails the bench
stops with no median, for the same reason a failing run reports none — a run
whose starting condition was never established is not a measurement of it.

The wall time here is measured from outside the process, so it includes
interpreter startup and imports. dl's own `dl-timing: total` line starts inside
main() and excludes them, so the two numbers are close but not the same
quantity; each line says which it is, and a before/after comparison should
quote one instrument on both sides.

Standard library only, on purpose: the instrument must not cost the project
a dependency.
"""

import argparse
import shlex
import statistics
import subprocess
import sys
import time
from typing import List, Optional


def bench(n: int, command: List[str], before: Optional[List[str]] = None) -> int:
    """Run *command* *n* times, print each wall time and the median.

    *before*, when given, runs before each timed run and is not timed.

    Returns the exit code for the process: 0 after n clean runs, otherwise the
    exit code of the first run — or reset — that failed.
    """
    durations = []
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
        result = subprocess.run(command, check=False)
        elapsed = time.perf_counter() - start
        if result.returncode != 0:
            print(
                f"run {i}/{n} exited {result.returncode}; no median over a failing command",
                file=sys.stderr,
            )
            return result.returncode
        print(f"run {i}/{n}: {elapsed:.3f}s")
        durations.append(elapsed)
    median = statistics.median(durations)
    print(f"median of {n}: {median:.3f}s (wall clock, including interpreter startup)")
    return 0


def positive_int(text: str) -> int:
    """A run count there is a median of: one or more."""
    value = int(text)
    if value < 1:
        raise argparse.ArgumentTypeError(f"need at least one run, got {value}")
    return value


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description="Run a command N times and report the median wall time."
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
        "command",
        nargs=argparse.REMAINDER,
        metavar="-- command...",
        help="the command to time, after a `--`",
    )
    args = parser.parse_args(argv)
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("no command given; usage: bench_launch.py -n 5 -- dl <ws> -- true")
    before = shlex.split(args.before) if args.before else None
    if args.before and not before:
        parser.error("--before given an empty command")
    return bench(args.n, command, before)


if __name__ == "__main__":
    sys.exit(main())
