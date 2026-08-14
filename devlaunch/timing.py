"""Env-gated wall-clock timing for dl's subprocess round trips.

Set ``DEVLAUNCH_TIMING=1`` and every dl process ends with one summary on
stderr: a ``dl-timing: <label> <seconds>s`` line per recorded subprocess call,
then a ``total`` for the whole command. Unset (or ``0``) records nothing and
prints nothing — the hot path must not pay for its own thermometer, so the off
state is a single ``None`` check. stderr because stdout is parsed by the
completion machinery, and one summary at the end rather than a line per event,
so the numbers land after the command's own output, not interleaved with it.

Set ``DEVLAUNCH_TIMING=json`` instead and the same run reports as one
machine-readable document on a single ``dl-timing-json:`` line, carrying the
named stages of :data:`STAGES` with the finer spans nested inside them. That
mode exists for a trend job that wants stage seconds without scraping prose;
``=1`` stays exactly the human summary it always was, down to the labels.
"""

import contextlib
import json
import math
import os
import sys
import time
from dataclasses import dataclass, field
from typing import Dict, Iterator, List, Optional, TextIO, Tuple

ENV_VAR = "DEVLAUNCH_TIMING"

# The value that asks for the document instead of the prose. Any other value
# that is not "off" keeps the prose, so a habit of `DEVLAUNCH_TIMING=true`
# still gets what it always got.
JSON_VALUE = "json"

# The marker the document's line carries, so a consumer can find it in stderr
# that also holds devpod's own chatter, and so the prose's `dl-timing:` prefix
# cannot be mistaken for it.
JSON_PREFIX = "dl-timing-json:"

# The stage vocabulary: one name per actionable owner of launch latency, in
# the order a launch meets them. It is a contract read from outside this repo
# (a trend that decomposes a launch, and the wf side of the handoff), so a
# name here is renamed only deliberately.
#
# `handoff` is the only stage nobody in this process runs: it is the gap
# between whoever handed off to dl and dl starting, measured from the stamp
# below. The rest bracket real arms of the launch — the host's git work, the
# devpod round trips that get a container running, lending the tools in, and
# the last trip into the running command.
HANDOFF_STAGE = "handoff"
STAGES = (HANDOFF_STAGE, "host-prep", "devpod-up", "tools", "attach")

# The stamp a hand-off writes for dl to read: Unix epoch seconds as a decimal
# string, which is what `date +%s.%N` prints. Wall clock rather than a
# monotonic counter because the two ends are different processes, and there is
# no clock they share whose zero survives an exec.
#
# Unset is the ordinary case (dl launched by a human), and it reports **no
# handoff stage at all** — never a zero, which would claim an instant handoff
# that never happened. So does a stamp that cannot be read as a number, or one
# in the future: neither is a measurement, and inventing one from it would put
# a fiction into a trend.
HANDOFF_VAR = "DEVLAUNCH_HANDOFF_T0"

# A stage's outcome, two-valued *because the third is absence*: a stage that
# was never reached has no record here at all, which is what keeps "ran fine"
# and "never ran" from collapsing into one 0.000s.
OK = "ok"
FAILED = "failed"

# `total` runs from the top of main(), so it is a smaller quantity than the wall
# time an outside stopwatch (`scripts/bench_launch.py`) reports for the same
# command: interpreter startup and this package's imports happen before main().
# The two get quoted side by side, so each line carries its epoch.
TOTAL_EPOCH = "in-process, excluding interpreter startup"


@dataclass
class _Stage:
    """One owner's arm: how long it held the launch, and what it spawned.

    *seconds* accumulates, because an owner's work is not always one
    contiguous region — a token fetch is host prep whenever it happens.
    "Never reached" is not a value this can hold: it is the absence of the
    record.
    """

    name: str
    seconds: float = 0.0
    outcome: str = OK
    spans: List[Tuple[str, float]] = field(default_factory=list)


@dataclass
class _Open:
    """A stage currently on the clock, and the instant it last started.

    Two stages can be open at once (`tools` runs inside the launch that
    `devpod-up` brackets), so the outer one is paused for the duration of the
    inner one rather than charged for it twice.
    """

    stage: _Stage
    since: float


@dataclass
class _Recorder:
    """One dl process's records: a start instant and the spans since.

    *document* is which of the two shapes this run will report in. It is
    settled once, at :func:`begin`, so nothing downstream has to re-read the
    environment or agree with it.
    """

    started: float
    document: bool = False
    entries: List[Tuple[str, float]] = field(default_factory=list)
    stages: Dict[str, _Stage] = field(default_factory=dict)
    open: List[_Open] = field(default_factory=list)


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
    asked = os.environ.get(ENV_VAR, "").strip()
    if asked in ("", "0"):
        _recorder = None
        return
    _recorder = _Recorder(started=time.perf_counter(), document=asked.lower() == JSON_VALUE)
    _read_handoff(_recorder)


def _read_handoff(recorder: _Recorder) -> None:
    """Record the `handoff` stage from the stamp, if there is one to read.

    Read here, at the top of main(), because that is dl's own end of the gap —
    the same epoch `total` starts from, and the same caveat: the interpreter
    starting and this package importing happen before it, so they land on the
    handoff's side of the boundary rather than being lost between the two.

    Nothing readable means no stage. See :data:`HANDOFF_VAR` for why silence
    is the only honest answer to a stamp that is missing, malformed, or ahead
    of this clock.
    """
    stamped = os.environ.get(HANDOFF_VAR, "").strip()
    if not stamped:
        return
    try:
        started = float(stamped)
    except ValueError:
        return
    if not math.isfinite(started):
        return
    seconds = time.time() - started
    if seconds < 0:
        return
    recorder.stages[HANDOFF_STAGE] = _Stage(name=HANDOFF_STAGE, seconds=seconds)


@contextlib.contextmanager
def _record(recorder: _Recorder, label: str) -> Iterator[None]:
    """Time the block, recording in ``finally`` and re-raising: a spawn that
    failed still took time, and dropping it would make the parts add up to
    less than the total."""
    start = time.perf_counter()
    try:
        yield
    finally:
        seconds = time.perf_counter() - start
        recorder.entries.append((label, seconds))
        if recorder.open:
            recorder.open[-1].stage.spans.append((label, seconds))


@contextlib.contextmanager
def _record_stage(recorder: _Recorder, name: str) -> Iterator[None]:
    """Charge the block to the stage called *name*, and the spans inside it too.

    A stage already on the clock is not re-entered: the arm is instrumented at
    several of its own entry points (a clone that fetches, a fetch that takes
    the lock), and an inner ``with`` there is the same owner's same work, not
    a second visit to charge for.
    """
    if any(entry.stage.name == name for entry in recorder.open):
        yield
        return
    paused = recorder.open[-1] if recorder.open else None
    stage = recorder.stages.get(name)
    if stage is None:
        stage = recorder.stages[name] = _Stage(name=name)
    now = time.perf_counter()
    if paused is not None:
        paused.stage.seconds += now - paused.since
    recorder.open.append(_Open(stage=stage, since=now))
    try:
        yield
    except BaseException:
        stage.outcome = FAILED
        raise
    finally:
        # Same discipline as a span's: an arm that died still held the launch
        # for as long as it did, and the failure is the reading, not a reason
        # to drop it.
        ended = time.perf_counter()
        stage.seconds += ended - recorder.open.pop().since
        if paused is not None:
            paused.since = ended


def stage(name: str):
    """Charge everything in the block to the ownership-boundary stage *name*.

    *name* must be one of :data:`STAGES` — the vocabulary is read by tooling
    outside this repo, so a name that drifted out of it would reach a consumer
    as data rather than as the mistake it is. The check runs only while
    recording, which is the same reason the off state stays a single ``None``
    check: an unmeasured launch pays for none of this.
    """
    recorder = _recorder
    if recorder is None:
        return contextlib.nullcontext()
    if name not in STAGES:
        raise ValueError(f"{name!r} is not a timing stage; expected one of {STAGES}")
    return _record_stage(recorder, name)


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
    if recorder.document:
        _emit_document(recorder, total, out)
        return
    for label, seconds in recorder.entries:
        print(f"dl-timing: {label} {seconds:.3f}s", file=out)
    print(f"dl-timing: total {total:.3f}s ({TOTAL_EPOCH})", file=out)


def _emit_document(recorder: _Recorder, total: float, out: TextIO) -> None:
    """Write the whole run as one JSON object on one marked line.

    One line rather than indented JSON: a consumer greps stderr for the marker
    and parses what follows, which no amount of surrounding output can break.
    """
    document = {
        "total": round(total, 6),
        "total_epoch": TOTAL_EPOCH,
        "stages": [
            {
                "stage": stage.name,
                "seconds": round(stage.seconds, 6),
                "outcome": stage.outcome,
                "spans": [
                    {"label": label, "seconds": round(seconds, 6)}
                    for label, seconds in stage.spans
                ],
            }
            for stage in recorder.stages.values()
        ],
    }
    print(f"{JSON_PREFIX} {json.dumps(document)}", file=out)
