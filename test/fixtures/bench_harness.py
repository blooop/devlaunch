"""Driving `scripts/bench_launch.py` in a test, and standing in for a launch.

The bench measures `dl`, but nothing here needs a real one: what these tests are
about is the bench and the record it writes, so the thing being measured is a
stand-in that reports a timing document and exits. That keeps the tests fast,
hermetic, and able to describe launches that would be awkward to arrange for real
(a run that reported its stages and then failed).

These helpers lived in the Python `test_timing`, which judged that
implementation's timing module and went with it (#267). The bench scripts are
language-agnostic tooling and stayed, so the harness that drives them stayed too
-- with the timing vocabulary now read from the Rust source that owns it rather
than imported.
"""

from __future__ import annotations

import json
import shlex
import subprocess
import sys
from pathlib import Path
from typing import List, Sequence, Tuple

from fixtures import rust_source
from fixtures.e2e_helpers import dl_command

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
BENCH = REPO_ROOT / "scripts" / "bench_launch.py"

# A `--before` command that fails, for asking what the bench does when the cold
# state it was told to establish was never established.
FAILING = shlex.join([sys.executable, "-c", "raise SystemExit(3)"])


def run_bench(*argv: str) -> subprocess.CompletedProcess:
    """Run the real bench script, as a contributor would."""
    return subprocess.run(
        [sys.executable, str(BENCH), *argv], capture_output=True, text=True, check=False
    )


def a_document(stages: Sequence[Tuple[str, float]], total: float = 0.3) -> str:
    """One launch's timing document, exactly as dl's json mode writes the line."""
    return f"{rust_source.timing_json_prefix()} " + json.dumps(
        {
            "total": total,
            "total_epoch": rust_source.timing_total_epoch(),
            "stages": [
                {"stage": name, "seconds": seconds, "outcome": "ok", "spans": []}
                for name, seconds in stages
            ],
        }
    )


def a_launch(tmp_path: Path, *runs, exit_code: int = 0) -> List[str]:
    """A stand-in launch reporting one of *runs*' documents per invocation.

    It reports a document only when the environment asks for one, the way dl
    does, so a bench that never asked gets what the real thing would have
    given it: nothing to record. Runs past the last one described repeat it,
    and *exit_code* is how a launch that reported its stages and then failed
    anyway is asked for.
    """
    lines = [a_document(stages, total) for stages, total in runs]
    counted = tmp_path / "launches"
    counted.mkdir()
    source = (
        "import os, sys\n"
        f"lines = {lines!r}\n"
        f"counted = {str(counted)!r}\n"
        "run = len(os.listdir(counted))\n"
        "open(os.path.join(counted, str(run)), 'w').close()\n"
        f"if os.environ.get({rust_source.timing_env_var()!r}) == "
        f"{rust_source.timing_json_value()!r}:\n"
        "    print(lines[min(run, len(lines) - 1)], file=sys.stderr)\n"
        f"sys.exit({exit_code})\n"
    )
    return [sys.executable, "-c", source]


def a_reset_from_the_absent_state(shim, reset: str) -> int:
    """Run a documented `--before` *reset* against a devpod that has nothing.

    That is the state every cold bench's first reset meets, and the one the
    recipe has to survive. Shared so both documents that publish a reset can be
    driven through the same starting state: the bench script's epilog, and the
    README's "Measuring launch time" section.

    The reset is run for real -- the binary under test against the fake devpod on
    PATH -- where this used to call the Python `main()` in-process behind three
    mocks (#267). The mocks encoded what real devpod does with the calls a forced
    remove makes; the shim is that, as a program, so there is nothing left to
    encode. The `dl-next` the recipes name is a working-tree build (see dev.sh),
    which is why the first word is dropped rather than executed.
    """
    argv = shlex.split(reset)[1:]
    env = shim.env()
    result = subprocess.run(
        [*dl_command(), *argv], env=env, capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        # The reason travels with the verdict: a caller asserting `== 0` would
        # otherwise report a bare exit code for a refusal that said why.
        print(result.stdout, result.stderr, sep="\n")
    return result.returncode
