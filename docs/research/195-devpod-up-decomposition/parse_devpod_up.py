"""Decompose `devpod up --log-output json` into image / create / lifecycle stages.

Sketch produced for devlaunch research ticket #195, verified against devpod
v0.26.1 (docker provider) on five workspace shapes: cached image, pulled image,
buildx build, warm no-op, and recreate.

INPUT. devpod writes its json log to STDOUT (not stderr, contrary to the
assumption in the ticket). Under --debug one line also appears on stderr, so a
caller that wants every line must merge both streams and sort by `time`; the
nanosecond timestamps make that safe, which is the one thing plain mode cannot
offer (it stamps whole seconds only) and raw mode cannot offer at all (no
timestamps).

WHAT THE SCHEMA GIVES YOU. Three keys: time, message, level. There is no
phase, stage or event field, and no schema version. Every boundary below is
therefore recovered by substring-matching English prose out of `message`.
`message` is omitempty: blank lines arrive as {"time":..,"level":..} with no
message key at all, so .get() is mandatory, not defensive style.

WHY PAIRING IS SEQUENTIAL. devpod logs "ran command: ..." after EVERY
lifecycle hook, not just postCreate. A devcontainer with onCreateCommand and
updateContentCommand -- both perfectly ordinary -- makes a first-match parser
pair postCreate's start with onCreate's end and report a NEGATIVE duration.
Each hook is therefore paired with the next "ran command:" that follows it.
"""

import json
import sys
from datetime import datetime

RUN_ARGS = "running docker command: command=docker, args=run "
HOOK_PREFIX = "running "
HOOK_SUFFIX = "Commands lifecycle hook"
HOOK_END = "ran command: command="

IMAGE_STARTS = (
    ("build", "build with docker buildx build"),
    ("pull", "image not found, pulling image"),
    ("cached", "inspecting image:"),
)
IMAGE_RANK = {path: rank for rank, (path, _) in enumerate(IMAGE_STARTS)}


def load(stream):
    """Yield (timestamp, message) for every json line devpod emitted."""
    for line in stream:
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            continue
        if "time" not in obj:
            continue
        yield datetime.fromisoformat(obj["time"]), obj.get("message", "")


def decompose(events):
    events = list(events)
    if not events:
        return None

    image_start = image_path = None
    create_start = create_end = None
    hooks = []          # (hook_name, start_ts) awaiting an end
    hook_spans = {}     # hook_name -> seconds

    pending_hook = None
    for ts, msg in events:
        # The earliest image line bounds the stage, but it does not classify
        # it: "inspecting image:" is logged BEFORE "image not found, pulling
        # image", so a first-match label calls a real pull a cache hit. Keep
        # the earliest timestamp; take the strongest evidence for the label.
        for rank, (path, needle) in enumerate(IMAGE_STARTS):
            if needle in msg:
                if image_start is None:
                    image_start = ts
                if image_path is None or rank < IMAGE_RANK[image_path]:
                    image_path = path
                break
        if create_start is None and RUN_ARGS in msg:
            create_start = ts
        elif create_end is None and create_start is not None and "setting up container" in msg:
            create_end = ts
        if msg.startswith(HOOK_PREFIX) and HOOK_SUFFIX in msg:
            name = msg[len(HOOK_PREFIX):msg.index(HOOK_SUFFIX)]
            pending_hook = (name, ts)
        elif pending_hook and HOOK_END in msg:
            name, start = pending_hook
            hook_spans[name] = (ts - start).total_seconds()
            hooks.append(name)
            pending_hook = None

    def span(a, b):
        return (b - a).total_seconds() if a and b else None

    window = (events[-1][0] - events[0][0]).total_seconds()
    named = {
        "image-acquire": span(image_start, create_start),
        "container-create": span(create_start, create_end),
    }
    named.update({f"hook:{h}": hook_spans[h] for h in hooks})
    accounted = sum(v for v in named.values() if v is not None)
    return {
        "image-path": image_path or "none (no image work logged)",
        **named,
        "log-window": window,
        "unattributed": round(window - accounted, 3),
    }


def main():
    out = decompose(load(sys.stdin))
    if out is None:
        print("no parseable devpod json on stdin", file=sys.stderr)
        return 1
    for k, v in out.items():
        print(f"{k:24} {format(v, '.3f') if isinstance(v, float) else v}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
