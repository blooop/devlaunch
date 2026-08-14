"""Env-gated wall-clock timing for dl's subprocess round trips.

Set ``DEVLAUNCH_TIMING=1`` and every dl process ends with one summary on
stderr: a ``dl-timing: <label> <seconds>s`` line per recorded subprocess call,
then a ``total`` for the whole command. Unset (or ``0``) records nothing and
prints nothing — the hot path must not pay for its own thermometer, so the off
state is a single ``None`` check. stderr because stdout is parsed by the
completion machinery, and one summary at the end rather than a line per event,
so the numbers land after the command's own output, not interleaved with it.
"""

import contextlib
import os
import sys
import time
from dataclasses import dataclass, field
from typing import Iterator, List, Optional, TextIO, Tuple

ENV_VAR = "DEVLAUNCH_TIMING"

# `total` runs from the top of main(), so it is a smaller quantity than the wall
# time an outside stopwatch (`scripts/bench_launch.py`) reports for the same
# command: interpreter startup and this package's imports happen before main().
# The two get quoted side by side, so each line carries its epoch.
TOTAL_EPOCH = "in-process, excluding interpreter startup"


@dataclass
class _Recorder:
    """One dl process's records: a start instant and the spans since."""

    started: float
    entries: List[Tuple[str, float]] = field(default_factory=list)


# On/off is this one optional recorder, not a flag plus fields that would have
# to agree with it: off, there is nothing to hold.
_recorder: Optional[_Recorder] = None


def begin() -> None:
    """Start recording iff DEVLAUNCH_TIMING asks for it.

    Called once at the top of main(), replacing any recorder left from an
    earlier main() in the same process, so one command's spans never leak into
    the next command's summary.
    """
    global _recorder  # pylint: disable=global-statement
    if os.environ.get(ENV_VAR, "").strip() in ("", "0"):
        _recorder = None
        return
    _recorder = _Recorder(started=time.perf_counter())


@contextlib.contextmanager
def _record(recorder: _Recorder, label: str) -> Iterator[None]:
    """Time the block, recording in ``finally`` and re-raising: a spawn that
    failed still took time, and dropping it would make the parts add up to
    less than the total."""
    start = time.perf_counter()
    try:
        yield
    finally:
        recorder.entries.append((label, time.perf_counter() - start))


def span(label: str):
    """Time one subprocess round trip as *label*; when timing is off, hand back
    the stdlib no-op instead — no clock read, nothing recorded."""
    recorder = _recorder
    if recorder is None:
        return contextlib.nullcontext()
    return _record(recorder, label)


def emit(stream: Optional[TextIO] = None) -> None:
    """Write the summary and stop recording; silent if recording never began.

    *stream* defaults to sys.stderr resolved now, not at import, so capture
    fixtures and redirections see the output.
    """
    global _recorder  # pylint: disable=global-statement
    recorder = _recorder
    _recorder = None
    if recorder is None:
        return
    out = stream if stream is not None else sys.stderr
    total = time.perf_counter() - recorder.started
    for label, seconds in recorder.entries:
        print(f"dl-timing: {label} {seconds:.3f}s", file=out)
    print(f"dl-timing: total {total:.3f}s ({TOTAL_EPOCH})", file=out)
