"""Env-gated wall-clock timing for dl's subprocess round trips.

Set ``DEVLAUNCH_TIMING=1`` and every dl process ends with one summary on
stderr: one ``dl-timing: <label> <seconds>s`` line per recorded subprocess
call, then a ``total`` line for the whole command. With the variable unset
(or ``0``) nothing is recorded and nothing is printed — the hot path must not
pay for its own thermometer, so the off state is a single ``None`` check.

stderr because stdout is parsed by the completion machinery, and one summary
per process (rather than a line per event as it happens) so the output
composes with one-shot commands: the numbers arrive after the command's own
output, not interleaved with it.

The on/off state is one optional recorder, not a flag plus fields that must
agree with it: when timing is off there is nothing to hold, and every code
path that records goes through the same ``span`` gate.
"""

import os
import sys
import time
from dataclasses import dataclass, field
from typing import List, Optional, TextIO, Tuple

ENV_VAR = "DEVLAUNCH_TIMING"


@dataclass
class _Recorder:
    """Wall-clock records for one dl process: a start instant and spans."""

    started: float
    entries: List[Tuple[str, float]] = field(default_factory=list)


_recorder: Optional[_Recorder] = None


class _Span:
    """Times one ``with`` block and records it, exceptions included.

    A spawn that failed still took time, and a summary that silently dropped
    it would make the remaining numbers add up to less than the total.
    """

    __slots__ = ("_recorder", "_label", "_start")

    def __init__(self, recorder: _Recorder, label: str):
        self._recorder = recorder
        self._label = label
        self._start = 0.0

    def __enter__(self) -> "_Span":
        self._start = time.perf_counter()
        return self

    def __exit__(self, *_exc) -> bool:
        self._recorder.entries.append((self._label, time.perf_counter() - self._start))
        return False


class _NoSpan:
    """The span handed out when timing is off: enter, exit, remember nothing."""

    def __enter__(self) -> "_NoSpan":
        return self

    def __exit__(self, *_exc) -> bool:
        return False


_NOOP_SPAN = _NoSpan()


def begin() -> None:
    """Start recording for this process iff DEVLAUNCH_TIMING asks for it.

    Called once at the top of main(). Replaces any recorder left over from an
    earlier main() call in the same process (a test, a shell wrapper), so one
    command's spans never leak into the next command's summary.
    """
    global _recorder  # pylint: disable=global-statement
    if os.environ.get(ENV_VAR, "").strip() in ("", "0"):
        _recorder = None
        return
    _recorder = _Recorder(started=time.perf_counter())


def span(label: str):
    """A context manager timing one subprocess round trip as *label*.

    Free when timing is off: no allocation, no clock read, just the shared
    no-op span.
    """
    recorder = _recorder
    if recorder is None:
        return _NOOP_SPAN
    return _Span(recorder, label)


def emit(stream: Optional[TextIO] = None) -> None:
    """Write the summary and stop recording; silent when recording never began.

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
    print(f"dl-timing: total {total:.3f}s", file=out)
