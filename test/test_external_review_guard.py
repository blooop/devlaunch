"""The guard that notices when the external reviewer stopped reviewing.

Between 2026-08-22 and 2026-08-24, twenty-six consecutive pull requests merged
with no external review. Sourcery had run out of weekly quota and was answering

    Sorry @blooop, you have reached your weekly rate limit of 500000 diff
    characters.

-- posted *as a review*. So `gh pr view --json reviews` showed a review present,
the merge path was satisfied, and the outage was invisible for a day and a half.
The largest changes in the repo went through in that window, including a
breaking CLI grammar change and a 1,862-line feature that was withdrawn again
twenty-two hours later.

The strings below are the real refusals, copied from those pull requests, and
the accepted body is the real "reviewed your changes and they look great"
Sourcery posts when it found nothing -- which is the case the guard must NOT
fail, and the case a naive "did the review say anything useful" check gets
wrong.

`gate`'s `needs` is asserted here for the same reason `test_bench_workflow.py`
asserts the bench job is *absent* from it: the wiring is one edit away from
being lost, and a guard nothing requires is not a guard.
"""

import subprocess
from pathlib import Path

ROOT = Path(__file__).parent.parent
VERDICT = ROOT / "scripts" / "external_review_verdict.sh"
CI = ROOT / ".github" / "workflows" / "ci.yml"
JOB = "external-review"

# Verbatim from PRs #343, #344, #345, #347, #348, #350-#368 and #370-#376.
WEEKLY_QUOTA = (
    "Sorry @blooop, you have reached your weekly rate limit of 500000 diff "
    "characters.\n\nPlease try again later or [upgrade](https://app.sourcery.ai/"
    "login?connection=github)"
)
# Verbatim from PR #369, the 1,631-line `--rm` grammar change.
OVER_DIFF_CAP = (
    "Sorry @blooop, your pull request is larger than the review limit of 150000 diff characters"
)
# Verbatim from PRs #330, #331, #333, #334, #337 and #342.
REVIEWED_AND_CLEAN = (
    "Hey - I've reviewed your changes and they look great!\n\n***\n\n"
    "<details>\n<summary>Sourcery is free for open source</summary>"
)
REVIEWED_WITH_FINDINGS = "Hey - I've found 2 issues\n\n## Individual Comments"


def verdict(bodies: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["bash", str(VERDICT)],
        input=bodies,
        capture_output=True,
        text=True,
        check=False,
    )


def test_the_weekly_quota_refusal_is_not_a_review():
    result = verdict(WEEKLY_QUOTA)
    assert result.returncode == 1, "the outage that started this ran for 26 PRs"
    assert "out of weekly quota" in result.stdout


def test_a_pull_request_over_the_diff_cap_is_not_reviewed():
    # The worse of the two: it fires on size, so it exempts exactly the changes
    # that least tolerate going unread. #369 was 1,631 lines of CLI grammar.
    result = verdict(OVER_DIFF_CAP)
    assert result.returncode == 1
    assert "per-diff cap" in result.stdout


def test_no_review_at_all_is_not_a_review():
    for empty in ["", "\n", "   \n  "]:
        result = verdict(empty)
        assert result.returncode == 1, f"{empty!r} left the reviewer unaccounted for"
        assert "no external review" in result.stdout


def test_a_review_that_found_nothing_passes():
    # The case that makes this guard usable rather than merely loud: most
    # reviews find nothing, and failing those would get the job deleted.
    result = verdict(REVIEWED_AND_CLEAN)
    assert result.returncode == 0, result.stdout


def test_a_review_that_found_something_passes():
    result = verdict(REVIEWED_WITH_FINDINGS)
    assert result.returncode == 0, result.stdout


def test_a_refusal_among_several_reviews_still_fails():
    # Bodies arrive concatenated. A re-review after a push can leave a real
    # review and a later refusal side by side, and the refusal is the newer
    # fact: the current head is what went unread.
    result = verdict(REVIEWED_AND_CLEAN + "\n" + WEEKLY_QUOTA)
    assert result.returncode == 1


def test_the_gate_requires_the_job():
    ci = CI.read_text(encoding="utf-8")
    assert f"\n  {JOB}:\n" in ci, f"{JOB} is the job this file is about"
    needs = next(line for line in ci.splitlines() if line.strip().startswith("needs: [ci,"))
    assert JOB in needs, (
        "a job outside `gate`'s needs is not required by the branch ruleset, "
        "which is the same shape of nothing-gating-anything this guard exists "
        "to catch"
    )


def test_the_workflow_runs_this_script_rather_than_its_own_copy():
    ci = CI.read_text(encoding="utf-8")
    assert "./scripts/external_review_verdict.sh" in ci, (
        "the classification must be the one under test; a copy inside a `run:` "
        "block is what goes stale"
    )
