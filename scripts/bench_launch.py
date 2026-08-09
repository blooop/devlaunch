#!/usr/bin/env python3
"""Run a command N times and report the median wall time.

The repeatable half of the timing instrument (ticket #140): a before/after
perf comparison is this script run once on each side of a change, e.g.

    python scripts/bench_launch.py -n 5 -- dl-next owner/repo -- true

Wall time per run and the median go to stdout; the command's own output is
left on the terminal so a hung launch is visible. If any run exits non-zero
the bench stops with that run's exit code and reports no median — a failing
launch's time is not a number to compare against.

Standard library only, on purpose: the instrument must not cost the project
a dependency.
"""

import argparse
import statistics
import subprocess
import sys
import time
from typing import List


def bench(n: int, command: List[str]) -> int:
    """Run *command* *n* times, print each wall time and the median.

    Returns the exit code for the process: 0 after n clean runs, otherwise
    the first failing run's exit code.
    """
    durations = []
    for i in range(1, n + 1):
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
    print(f"median of {n}: {statistics.median(durations):.3f}s")
    return 0


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        description="Run a command N times and report the median wall time."
    )
    parser.add_argument("-n", type=int, default=5, help="number of runs (default: 5)")
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
    return bench(args.n, command)


if __name__ == "__main__":
    sys.exit(main())
