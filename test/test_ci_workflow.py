"""Two properties of the CI workflow that nothing else can notice going wrong.

Both are about the same failure: **a check nobody ran reads exactly like a check
that passed**. `gate`'s own comment in the workflow says so, and the `rust` job
is where it keeps happening, because that job names its test suites one at a
time. Naming them buys real things -- a per-suite `timeout` so a wedged binary
fails its own short step, and a step title that says which suite died even when
the runner loses contact before uploading logs -- but it makes the list, rather
than the workspace, the definition of what runs. A suite missing from the list
is built by the `Build` step and run by nothing, and there is no red tick
anywhere to say so.

That is not hypothetical. `devlaunch-test-support` was off the list once and got
added back with a comment about it; `rust/dl/tests/picker.rs` and
`rust/dl/tests/terminal.rs` were both off it when this file was written, eight
tests between them, one of them the guard on a fix that shipped the week before.
So the backstop asserted here is a `cargo test --workspace` step: the list stays
for triage, and the workspace decides what runs.

The second property is the timeout. Three jobs carried none, in a workflow that
argues for them in three separate comment blocks -- and `review` polls a remote
API in a sleep loop, which is the shape that hangs. Asserted for every job
rather than for the three that were missing, because the point is the next job
somebody adds.

These are string assertions on config, in the same spirit as
`test_bench_workflow.py` and `test_public_api_snapshots_doc.py`: there is no
YAML parser in this project's dependencies, and adding one to assert three
things would cost more than it guards.
"""

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).parent.parent
CI = ROOT / ".github" / "workflows" / "ci.yml"

# The indentation a job's own key sits at, and the one its settings sit at.
JOB_KEY = re.compile(r"^  ([A-Za-z][\w-]*):\s*$", re.MULTILINE)


def workflow() -> str:
    assert CI.is_file(), "ci.yml is the gate; without it there is nothing to guard"
    return CI.read_text(encoding="utf-8")


def jobs_block() -> str:
    """Everything under `jobs:`, with a leading newline.

    Sliced first so the top-level `on:` keys -- `push:`, `pull_request:` -- are
    not mistaken for job names: they sit at the same indentation. The leading
    newline is kept so the first job's key looks like every other job's key to
    the searches below.
    """
    text = workflow()
    return text[text.index("\njobs:\n") + len("\njobs:") :]


def job_names() -> list[str]:
    names = JOB_KEY.findall(jobs_block())
    # A slice that found no jobs would make every assertion below vacuous, which
    # is the exact failure these tests exist to stop being possible.
    assert len(names) > 3, f"ci.yml parsed as {names}, which is not this workflow's jobs"
    return names


def ci_job(name: str) -> str:
    """One job of ci.yml, as text.

    A slice rather than a parse, ending at the next line that starts a sibling
    key at the jobs' own indentation -- not at the first blank line, which a
    cosmetic blank line inside a `run:` block would otherwise be mistaken for.
    """
    block = jobs_block()
    start = block.index(f"\n  {name}:\n") + 1
    following = JOB_KEY.search(block, start + 1)
    job = block[start : following.start()] if following else block[start:]
    assert "runs-on:" in job, f"the {name} job slice does not contain the job"
    return job


def settings(job: str) -> str:
    """A job with its commentary removed.

    Half of what this workflow says about itself is a comment, and every one of
    the assertions below is about what the file *does* -- a comment mentioning
    `--workspace` or `timeout-minutes` must not satisfy them.
    """
    return "\n".join(line for line in job.splitlines() if not line.lstrip().startswith("#"))


# A `key:` at some indentation. Deliberately unable to match a step, because a
# step is a list item (`      - name: ...`) and the leading `-` is not in here:
# telling a job's own keys from a step's is the whole job of this pattern.
KEY = re.compile(r"^( *)([A-Za-z][\w-]*):")
# The same, where the value is a block scalar (`run: |`) rather than on the line.
BLOCK = re.compile(r"^( *)([A-Za-z][\w-]*): *[|>][-+]? *$")
# A job's own settings sit here. `runs-on:`, `needs:`, `timeout-minutes:`.
JOB_INDENT = 4


def _walk(job: str) -> tuple[list[str], list[str]]:
    """Split a job into its own keys and the shell it actually runs.

    Both halves exist because the naive version of each was wrong, and in the
    same direction -- a string found somewhere in the job slice was taken for a
    setting the job has.

    `timeout-minutes` is a valid key on a *step* as well as on a job, and a step
    timeout bounds the step and leaves the job unbounded. So the keys returned
    here are only those at the job's own indentation, and a `run:` script is
    skipped entirely rather than scanned -- a job's shell is free to contain a
    line shaped like a key.

    The `run:` halves are returned separately because a step *title* is not a
    command: a step called `disabled: cargo test --workspace` runs nothing, and
    a check that reads the whole slice cannot tell it from one that does.
    """
    keys: list[str] = []
    scripts: list[str] = []
    lines = settings(job).splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        i += 1
        block = BLOCK.match(line)
        if block:
            indent, key = len(block.group(1)), block.group(2)
            if indent == JOB_INDENT:
                keys.append(key)
            # Everything more-indented than the key belongs to its value, and is
            # not searched for keys of its own.
            while i < len(lines):
                body = lines[i]
                if body.strip() and len(body) - len(body.lstrip()) <= indent:
                    break
                if key == "run":
                    scripts.append(body.strip())
                i += 1
            continue
        key_match = KEY.match(line)
        if key_match:
            indent, key = len(key_match.group(1)), key_match.group(2)
            if indent == JOB_INDENT:
                keys.append(key)
            if key == "run":
                scripts.append(line.split("run:", 1)[1].strip())
    return keys, scripts


def job_keys(job: str) -> list[str]:
    """The keys the job itself carries, not the ones its steps carry."""
    return _walk(job)[0]


def run_lines(job: str) -> list[str]:
    """Every line of shell a job runs, and nothing that merely sits near some."""
    return [line for line in _walk(job)[1] if line]


# ---------------------------------------------------------------------------
# the workspace decides what runs, not the list
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_the_rust_job_runs_the_whole_workspace_somewhere():
    """One step that no new suite can be left out of.

    Without it, adding `rust/<crate>/tests/<name>.rs` and forgetting the step
    that names it is a green tick over tests that never ran. With it, the worst
    a forgotten step costs is the triage the per-suite steps give.
    """
    lines = [line for line in run_lines(ci_job("rust")) if "cargo test" in line]
    backstop = [line for line in lines if "--workspace" in line]
    assert backstop, (
        "the rust job runs no `cargo test --workspace`; every suite it runs is one "
        "somebody remembered to name, so a new test binary runs nowhere and says "
        "nothing"
    )
    assert all("--locked" in line for line in backstop), (
        "the workspace backstop does not pass --locked, so it may resolve "
        "dependencies the lockfile does not name"
    )


@pytest.mark.unit
def test_the_backstop_has_not_replaced_the_per_suite_steps():
    """The list is still there, because the list is what triage reads.

    `--workspace` alone is one step for ~1500 tests: a wedged binary takes the
    job's whole budget and the failure names no suite. Both halves, or neither
    is worth having.
    """
    named = [line for line in run_lines(ci_job("rust")) if "cargo test" in line and " -p " in line]
    assert len(named) >= 10, (
        f"the rust job runs {len(named)} per-suite steps; the backstop is a "
        "safety net under those, not a replacement for them"
    )
    assert any("devlaunch-test-support" in line for line in named), (
        "devlaunch-test-support is off the named list again -- its tests are the "
        "fake devpod the integration suites use as their whole devpod"
    )


# ---------------------------------------------------------------------------
# nothing here runs unbounded
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_every_job_carries_a_timeout():
    """Unbounded, a stalled job hangs for six hours and reports a timeout nobody reads.

    Per job rather than per named job: the three that were missing one are
    fixable once, and the next job somebody adds is what this is for.

    At the job's own indentation, because `timeout-minutes` is a legal key on a
    step too and a step timeout bounds the step while leaving the job unbounded.
    A substring search over the job's text calls that covered; it is the same
    mistake this file is about, one level further in.
    """
    unbounded = [name for name in job_names() if "timeout-minutes" not in job_keys(ci_job(name))]
    assert not unbounded, (
        f"these ci.yml jobs run unbounded: {unbounded}. GitHub's own default is "
        "six hours, which is long enough that the run is abandoned rather than read"
    )
