"""The trend workflow's charter, guarded where it can be edited (#198).

Map #193's first decision is *alert-don't-gate*: this workflow notices a
regression and must never be able to block a merge on one, because a wall-clock
measurement on a shared runner is either too loose to fire or flaky. That is a
property of a YAML file, so it is one edit away from being lost — a
`pull_request:` trigger added for convenience, a `fail-on-alert: true` copied
from another project's example, the job wired into the CI gate's `needs`.

These are text assertions on config, deliberately: the invariants live in the
file's own words, there is no YAML parser in this project's dependencies, and
adding one to assert four strings would cost more than it guards. Each check
below is a thing somebody could plausibly write, with the reason it must not be
written.
"""

from pathlib import Path

WORKFLOWS = Path(__file__).parent.parent / ".github" / "workflows"
BENCH = WORKFLOWS / "bench.yml"
CI = WORKFLOWS / "ci.yml"
BENCH_JOB = "bench"


def bench_workflow() -> str:
    assert BENCH.is_file(), f"{BENCH.name} is what publishes the trend"
    return BENCH.read_text(encoding="utf-8")


def bench_settings() -> str:
    """The workflow with its commentary removed.

    Half of what this file says about itself is a comment explaining a key it
    deliberately does *not* set, so a check for "this key is absent" has to
    read the settings rather than the prose about them.
    """
    return "\n".join(
        line for line in bench_workflow().splitlines() if not line.lstrip().startswith("#")
    )


def cold_reset() -> str:
    """The command the cold-recreate shape runs before every timed run.

    A line rather than a parse: `--before` takes one shell-quoted command and
    the step writes it on its own continuation line, so what this returns is
    that command and the properties below are about what it does.
    """
    lines = [line for line in bench_settings().splitlines() if "--before" in line]
    assert len(lines) == 1, "the cold-recreate shape is the only one that resets between runs"
    return lines[0]


class TestItCannotBecomeAMergeGate:
    """Three independent ways this could turn into a required check, and the
    reason each stays shut."""

    def test_it_is_its_own_workflow_rather_than_a_job_in_the_gated_one(self):
        """House convention is that a new job in the CI workflow joins the
        gate's `needs` in the same pull request. A slow, noisy benchmark that
        did that would be exactly the merge gate the charter grilled out."""
        needs = [line for line in CI.read_text(encoding="utf-8").splitlines() if "needs:" in line]
        assert needs, "the CI gate's needs list is what this must stay out of"
        for line in needs:
            assert BENCH_JOB not in line, line

    def test_it_does_not_run_on_pull_requests_at_all(self):
        """Per-PR benching is out of scope under alert-don't-gate — the value
        is the trend line, not a pre-merge verdict — and a job that reports on
        a pull request is a job somebody can require."""
        assert "pull_request" not in bench_settings()

    def test_a_slow_point_alerts_and_does_not_fail_the_build(self):
        """`fail-on-alert` is the whole map in one setting."""
        text = bench_settings()
        assert "fail-on-alert: false" in text
        assert "fail-on-alert: true" not in text


class TestItMeasuresEveryCommitTheSameWay:
    def test_nothing_is_path_filtered(self):
        """A performance regression can arrive in any file, and a trend with
        holes in it cannot be compared against its own previous point."""
        text = bench_settings()
        assert "paths:" not in text
        assert "paths-ignore:" not in text

    def test_it_runs_on_main_and_on_demand(self):
        text = bench_settings()
        assert "branches: [main]" in text
        assert "workflow_dispatch:" in text

    def test_two_merges_cannot_bench_at_once(self):
        """The publishing action commits to a branch; concurrent runs race on
        that push, and two benches sharing one runner-class measure each
        other."""
        assert "concurrency:" in bench_settings()


class TestTheAlertReachesSomebody:
    def test_the_alert_comment_names_a_user(self):
        """On a push there is no pull request, so the action leaves a commit
        comment — and commit comments notify almost nobody. The @-mention is
        what actually generates a notification, so without it the alert is
        functionally invisible (#197)."""
        assert "alert-comment-cc-users" in bench_settings()


class TestTheRunnerStateIsScoped:
    def test_the_bench_writes_to_the_runners_own_scratch_state(self):
        """The job runs real launches, which create real containers and real
        devpod state. Both variables, because they scope different halves:
        `DEVPOD_HOME` is devpod's own, `XDG_CACHE_HOME` is devlaunch's clone
        cache and bookkeeping."""
        text = bench_settings()
        assert "DEVPOD_HOME:" in text
        assert "XDG_CACHE_HOME:" in text


class TestTheColdResetSurvivesAContainerWrittenClone:
    """A launch leaves its clone owned by the container's user — uid 1000 in
    the standard devcontainer base image — and a GitHub runner's own user is
    not that uid. `dl rm` then deletes the workspace, warns that the clone
    would not go, and exits 0; the next launch dies writing `.git/index.lock`
    in a clone it does not own (run 31838698495, follow-up to #198). The runner
    has passwordless sudo, so the reset takes the clone back before removing
    it. This is a property of a CI runner, not of `dl`, which is why the
    recovery lives here."""

    def test_the_reset_takes_the_clone_back_before_removing_it(self):
        reset = cold_reset()
        assert "sudo chown" in reset
        assert "rm --force" in reset
        assert reset.index("sudo chown") < reset.index("rm --force"), (
            "chowning after the remove recovers nothing: the remove is what fails"
        )

    def test_the_reset_only_reclaims_this_jobs_own_clone_cache(self):
        """Scoped to the cache the job already scoped to `/tmp`. A recursive
        chown is a blunt instrument and a wider one would take ownership of
        things this job did not create."""
        assert "XDG_CACHE_HOME" in cold_reset()

    def test_the_recovery_runs_per_run_rather_than_once_before_the_bench(self):
        """Every run recreates those files, so a one-time step before the bench
        recovers run 1 and leaves runs 2..5 with exactly the clone that broke
        it. `--before` is the only place with the right cardinality."""
        assert bench_settings().count("chown") == 1, (
            "a second chown is a one-time step doing per-run work"
        )


class TestTheExclusionsSayWhyInTheFile:
    def test_the_first_ever_cold_exclusion_is_written_down_where_it_is_made(self):
        """Charter decision 4 excludes the first-ever cold launch — its
        dominant cost is an image transfer this repo does not control. On an
        ephemeral runner that exclusion is a step that primes the image before
        anything is timed, which looks exactly like a step somebody could
        delete as redundant. The reason has to be next to it."""
        assert "first-ever" in bench_workflow()
