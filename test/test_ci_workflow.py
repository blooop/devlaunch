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


def run_lines(job: str) -> list[str]:
    """Every command line in a job, comments dropped."""
    return [line.strip() for line in settings(job).splitlines() if line.strip()]


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
    """
    unbounded = [name for name in job_names() if "timeout-minutes:" not in settings(ci_job(name))]
    assert not unbounded, (
        f"these ci.yml jobs run unbounded: {unbounded}. GitHub's own default is "
        "six hours, which is long enough that the run is abandoned rather than read"
    )
