"""The guard that notices an entry filed inside an already-shipped release.

blooop/devlaunch#527. `## [Unreleased]` is a stable heading that a release cut
*renames*: it becomes `## [0.25.0] - 2026-08-28` and a fresh empty `[Unreleased]`
is inserted above it. A branch cut before that release carries its entry anchored,
by context, under the old heading -- which is now the release. Git sees lines added
below a heading that still exists, resolves it with no conflict, and the pull
request reports `MERGEABLE` with every check green while a shipped version grows
bullets describing fixes it never contained.

That is why the fixture below performs the **actual merge** rather than writing a
bad changelog by hand. A hand-written fixture would only prove the parser rejects
input someone already knew was wrong. What has to be proved is that the *default
outcome of the ordinary operation* is wrong -- `a_clean_merge_across_a_release_cut_
files_the_entry_inside_the_release` asserts git reports no conflict and the entry
still lands in the wrong section, and that is the whole claim of the ticket.

The scale is why it is worth a job: four times in one afternoon, three agents, none
aware of the others (`wayfinder/devlaunch-{305,308,349,354}`), plus twice in a single
build on #346. Every instance was caught by someone reading the diff.

The other half of the design is what the guard *permits*, tested by
`the_release_cut_that_creates_the_hazard_passes_the_guard`: the rule is keyed by
version, so a release cut adds a heading and modifies none, and passes untouched.
The tempting phrasing -- "the released portion of the file is unchanged" -- fails
that commit, and a guard whose first firing is a false positive on the project's
own release ritual does not survive to catch anything.
"""

import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).parent.parent
GUARD = (ROOT / "scripts" / "changelog_frozen.py").resolve()
CI = ROOT / ".github" / "workflows" / "ci.yml"

PREAMBLE = """# Changelog

All notable changes to this project will be documented in this file.

"""

RELEASED = """## [0.1.0] - 2026-01-01

### Added

- The first release.
"""


def changelog(unreleased: str, *rest: str) -> str:
    return PREAMBLE + f"## [Unreleased]\n{unreleased}\n" + "\n".join((*rest, RELEASED))


def run(*args: str, cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(args, cwd=cwd, capture_output=True, text=True, check=False)


def check(base: str, head: str, tmp_path: Path) -> subprocess.CompletedProcess:
    (tmp_path / "base.md").write_text(base, encoding="utf-8")
    (tmp_path / "head.md").write_text(head, encoding="utf-8")
    return run(
        sys.executable,
        str(GUARD),
        str(tmp_path / "base.md"),
        str(tmp_path / "head.md"),
    )


def git(*args: str, cwd: Path) -> None:
    done = run("git", *args, cwd=cwd)
    assert done.returncode == 0, f"git {' '.join(args)} failed: {done.stderr}"


# Named through the decorator so the fixture function and the parameter that
# receives it are not one name in one module, which pylint reads as shadowing.
@pytest.fixture(name="repo")
def repo_before_the_release_cut(tmp_path: Path) -> Path:
    """A repository at the moment before the release cut.

    `main` has an unreleased entry of its own, so the merge below is a realistic
    one: both sides touched `[Unreleased]`, which is what makes the anchoring
    interesting rather than a trivial fast-forward.
    """
    work = tmp_path / "repo"
    work.mkdir()
    git("init", "-q", "-b", "main", cwd=work)
    git("config", "user.email", "guard@example.invalid", cwd=work)
    git("config", "user.name", "Guard Fixture", cwd=work)
    (work / "CHANGELOG.md").write_text(
        changelog("\n### Fixed\n\n- A fix that shipped in 0.2.0.\n"), encoding="utf-8"
    )
    git("add", "CHANGELOG.md", cwd=work)
    git("commit", "-qm", "before the cut", cwd=work)
    return work


def test_a_clean_merge_across_a_release_cut_files_the_entry_inside_the_release(repo, tmp_path):
    # The ticket's claim, reproduced rather than described. Branch and main both
    # edit `[Unreleased]`; main then *renames* that heading by cutting 0.2.0. The
    # merge is clean -- asserted, because a conflict here would mean git had a
    # signal and the whole premise is that it has none -- and the branch's entry
    # ends up under `## [0.2.0]`, describing a fix that release does not contain.
    base_before = (repo / "CHANGELOG.md").read_text(encoding="utf-8")

    git("checkout", "-q", "-b", "fix/thing", cwd=repo)
    (repo / "CHANGELOG.md").write_text(
        changelog(
            "\n### Fixed\n\n- A fix that shipped in 0.2.0.\n- The branch's own fix, written later.\n"
        ),
        encoding="utf-8",
    )
    git("commit", "-qam", "the branch's entry", cwd=repo)

    git("checkout", "-q", "main", cwd=repo)
    (repo / "CHANGELOG.md").write_text(
        changelog(
            "\n",
            "## [0.2.0] - 2026-02-02\n\n### Fixed\n\n- A fix that shipped in 0.2.0.\n",
        ),
        encoding="utf-8",
    )
    git("commit", "-qam", "Cut 0.2.0", cwd=repo)
    cut = (repo / "CHANGELOG.md").read_text(encoding="utf-8")

    git("checkout", "-q", "fix/thing", cwd=repo)
    merge = run("git", "merge", "--no-edit", "main", cwd=repo)

    assert merge.returncode == 0, (
        f"the premise is that git sees nothing to conflict on: {merge.stderr}"
    )
    merged = (repo / "CHANGELOG.md").read_text(encoding="utf-8")
    assert "- The branch's own fix, written later." in merged.split("## [0.2.0]")[1], (
        "the fixture is only interesting if the entry really did land inside the release"
    )

    done = check(cut, merged, tmp_path)

    assert done.returncode == 1
    assert "'## [0.2.0]' is already released and this branch changes it." in done.stderr
    assert "The branch's own fix, written later." in done.stderr
    assert base_before != cut, "sanity: the cut did rewrite the file"


def test_the_release_cut_that_creates_the_hazard_passes_the_guard(tmp_path):
    # The case the rule is shaped to permit, and the reason it is keyed by version
    # rather than by region. This commit rewrites most of the released portion of
    # the file -- it renames `[Unreleased]` to `[0.2.0]` and inserts a new empty
    # `[Unreleased]` -- and modifies no heading that already existed. "The released
    # portion is unchanged" would fail here, on the one commit that is by
    # definition correct.
    base = changelog("\n### Fixed\n\n- Something.\n")
    head = changelog("\n", "## [0.2.0] - 2026-02-02\n\n### Fixed\n\n- Something.\n")

    done = check(base, head, tmp_path)

    assert done.returncode == 0, done.stderr


def test_an_ordinary_entry_under_unreleased_passes(tmp_path):
    base = changelog("\n")
    head = changelog("\n### Fixed\n\n- The thing this pull request fixes.\n")

    done = check(base, head, tmp_path)

    assert done.returncode == 0, done.stderr


def test_editing_a_released_date_is_caught_by_the_same_rule(tmp_path):
    # The heading line is part of the compared text, so a silently corrected date
    # is the same defect as a silently added bullet: a claim about a release that
    # was made after it shipped.
    base = changelog("\n")
    head = changelog("\n").replace("## [0.1.0] - 2026-01-01", "## [0.1.0] - 2026-01-02")

    done = check(base, head, tmp_path)

    assert done.returncode == 1
    assert "'## [0.1.0]'" in done.stderr


def test_a_changelog_it_cannot_parse_fails_rather_than_passes(tmp_path):
    # A guard that cannot read its input has not checked anything. Reporting that
    # as success is the same species of defect as the one it exists to find --
    # see #517, a merge verdict pinned to a check set that was never complete.
    done = check(changelog("\n"), "no headings here at all\n", tmp_path)

    assert done.returncode == 1
    assert "not passing" in done.stderr


def test_a_duplicated_version_heading_is_refused_rather_than_resolved(tmp_path):
    # Two `## [0.1.0]` headings would otherwise compare against whichever came
    # last, which is a hole precisely where the guard has to be solid.
    done = check(changelog("\n"), changelog("\n") + RELEASED, tmp_path)

    assert done.returncode == 1
    assert "appears more than once" in done.stderr


def test_ci_runs_the_guard_on_pull_requests():
    # A guard nothing invokes is a guard that is not running, which is the failure
    # `test_bench_workflow.py` and `test_review_guard.py` both write about.
    workflow = CI.read_text(encoding="utf-8")
    assert "scripts/changelog_frozen.py" in workflow
