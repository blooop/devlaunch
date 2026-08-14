#!/usr/bin/env python3
"""Run a command N times and report the median wall time (ticket #140).

The repeatable half of the timing instrument: a before/after perf comparison is
this script run once on each side of a change. Stdlib only, on purpose — the
instrument must not cost the project a dependency. See --help for the two
traps: cold medians, and which clock each number is on.
"""

import argparse
import shlex
import statistics
import subprocess
import sys
import time
from typing import List, Optional

EPILOG = """\
warm:  bench_launch.py -n 5 -- dl-next owner/repo -- true

cold:  export DEVPOD_HOME=/tmp/dl-bench/devpod \\
              DEVPOD_SSH_CONFIG=/tmp/dl-bench/ssh_config \\
              XDG_CACHE_HOME=/tmp/dl-bench/cache
       pixi run dev-add-docker    # a fresh devpod home has no provider
       bench_launch.py -n 5 --before 'dl-next owner/repo rm --force' \\
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

This clock starts outside the process, so it includes interpreter startup and
imports; dl's own `dl-timing: total` starts inside main() and excludes them
(on `dl --version`: 0.001s total against a 0.061s median here). Quote one
instrument on both sides of a change.
"""


def bench(n: int, command: List[str], before: Optional[List[str]] = None) -> int:
    """Run *command* *n* times, print each wall time and the median.

    Returns 0 after n clean runs, otherwise the exit code of the first run --
    or untimed *before* reset -- that failed.
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
