"""The guard that refuses a bump onto a version somebody already published.

blooop/devlaunch#591. Two branches bumped to 0.36.0 independently: #593 cut it and
published, and #591 -- open across that release -- carried its own
`release: 0.36.0`. Git merged the collision without a conflict, because picking
the same number means both sides made the identical edit, and the second merge
published nothing at all: `publish.yml` found the tag, said `nothing to publish`
(what it says for every ordinary push too), and the fix landed on main released
nowhere with no check red.

The tests below are written against that history rather than around it.
`the_collision_that_happened_is_refused` replays #591's own two manifests, so what
is asserted is the real event and not a hand-built imitation of it.

**Why this is a pull-request guard.** By the time the merge reaches `main` the
collision is invisible: the merge commit's version equals its first parent's --
both 0.36.0 -- so nothing downstream can see a bump to notice. The branch is the
last place the two numbers still differ, which is why
`a_merge_commit_cannot_see_the_collision_its_branch_could` pins that fact directly
against the history rather than leaving it as a claim in a comment.
"""

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).parent.parent
GUARD = (ROOT / "scripts" / "version_untaken.py").resolve()
CI = ROOT / ".github" / "workflows" / "ci.yml"

# The two commits of #591, and the release that beat it to the number.
BASE_OF_591 = "908a24ee21a14207e150071d3af65c5366da13c7"
HEAD_OF_591 = "d318cad8bb6d9c5e7f7870c6bcbfbe88a3fb002b"
MERGE_OF_591 = "84cfaf13792f17c251e183306bd65903919c8bc6"


def run(*args: str, cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(args, cwd=cwd, capture_output=True, text=True, check=False)


def manifest(version: str) -> str:
    return f'[workspace.package]\nversion = "{version}"\nedition = "2024"\n'


def check(base: str, head: str, tmp_path: Path) -> subprocess.CompletedProcess:
    (tmp_path / "base.toml").write_text(base, encoding="utf-8")
    (tmp_path / "head.toml").write_text(head, encoding="utf-8")
    return run(
        sys.executable,
        str(GUARD),
        str(tmp_path / "base.toml"),
        str(tmp_path / "head.toml"),
        cwd=ROOT,
    )


def at(commit: str, path: str) -> str:
    done = run("git", "show", f"{commit}:{path}", cwd=ROOT)
    assert done.returncode == 0, f"git show {commit}:{path} failed: {done.stderr}"
    return done.stdout


def test_the_collision_that_happened_is_refused(tmp_path):
    """#591's own manifests: 0.35.0 at its base, 0.36.0 at its head, v0.36.0 taken."""
    done = check(at(BASE_OF_591, "rust/Cargo.toml"), at(HEAD_OF_591, "rust/Cargo.toml"), tmp_path)

    assert done.returncode == 1
    assert "v0.36.0 is already tagged" in done.stderr
    assert "publish nothing" in done.stderr


def test_a_merge_commit_cannot_see_the_collision_its_branch_could(tmp_path):
    """Why the guard runs on the branch and not after the merge.

    The merge's version and its first parent's are the same string, so every
    "did this push bump the version" test downstream answers no. There is nothing
    left to catch by then, which is the whole argument for checking earlier.
    """
    merged = at(MERGE_OF_591, "rust/Cargo.toml")
    first_parent = at(f"{MERGE_OF_591}^", "rust/Cargo.toml")

    assert 'version = "0.36.0"' in merged
    assert 'version = "0.36.0"' in first_parent

    done = check(first_parent, merged, tmp_path)
    assert done.returncode == 0
    assert "proposes no new version" in done.stdout


def test_an_ordinary_branch_that_never_touches_the_version_passes(tmp_path):
    done = check(manifest("0.37.0"), manifest("0.37.0"), tmp_path)

    assert done.returncode == 0, done.stderr
    assert "proposes no new version" in done.stdout


def test_a_release_cut_to_a_version_nobody_has_taken_passes(tmp_path):
    """The ritual this guard must not break, for a version that cannot be tagged."""
    done = check(manifest("0.37.0"), manifest("99999.0.0"), tmp_path)

    assert done.returncode == 0, done.stderr
    assert "never been tagged" in done.stdout


def test_a_manifest_with_no_version_fails_rather_than_passes(tmp_path):
    """A guard that cannot read its input has checked nothing."""
    done = check("[workspace.package]\n", manifest("99999.0.0"), tmp_path)

    assert done.returncode == 1
    assert "not passing" in done.stderr


def test_a_manifest_that_is_not_there_fails_rather_than_passes(tmp_path):
    done = run(
        sys.executable, str(GUARD), str(tmp_path / "absent.toml"), str(tmp_path / "also.toml")
    )

    assert done.returncode == 1
    assert "not passing" in done.stderr


def test_ci_runs_the_guard_on_pull_requests():
    """It is worth a step only if a step actually runs it."""
    ci = CI.read_text(encoding="utf-8")

    assert "scripts/version_untaken.py" in ci
    assert "A version that is already published is not one to bump to" in ci
