"""The guard that refuses a bump onto a version somebody already published.

blooop/devlaunch#591. Two branches bumped to 0.36.0 independently: #593 cut it and
published, and #591 -- open across that release -- carried its own
`release: 0.36.0`. Git merged the collision without a conflict, because picking
the same number means both sides made the identical edit, and the second merge
published nothing at all: `publish.yml` found the tag, said `nothing to publish`
(what it says for every ordinary push too), and the fix landed on main released
nowhere with no check red.

Each test builds its own repository and its own tag rather than reading the real
history back. The first draft did read it -- #591's actual two manifests by SHA --
and it was wrong twice over: CI checks out shallow, so those commits are not
there, and the guard's oracle is `git rev-parse refs/tags/v<version>` in whatever
directory it runs in, so it would have been asking the checkout about tags the
checkout may not have fetched either. What is worth holding is the shape and the
numbers, and a repository built here has both under the test's own control.
"""

import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).parent.parent
GUARD = (ROOT / "scripts" / "version_untaken.py").resolve()
CI = ROOT / ".github" / "workflows" / "ci.yml"


def run(*args: str, cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(args, cwd=cwd, capture_output=True, text=True, check=False)


def git(*args: str, cwd: Path) -> None:
    done = run("git", *args, cwd=cwd)
    assert done.returncode == 0, f"git {' '.join(args)} failed: {done.stderr}"


def manifest(version: str) -> str:
    return f'[workspace.package]\nversion = "{version}"\nedition = "2024"\n'


# Named through the decorator so the fixture function and the parameter that
# receives it are not one name in one module, which pylint reads as shadowing.
@pytest.fixture(name="released")
def a_repository_with_a_release_already_out(tmp_path: Path) -> Path:
    """A repository where v0.36.0 has been cut and tagged, as #593 left main."""
    work = tmp_path / "repo"
    work.mkdir()
    git("init", "-q", "-b", "main", ".", cwd=work)
    git("config", "user.email", "version@example.invalid", cwd=work)
    git("config", "user.name", "Version Fixture", cwd=work)
    (work / "seed.txt").write_text("the release\n", encoding="utf-8")
    git("add", "-A", cwd=work)
    git("commit", "-qm", "cut 0.36.0", cwd=work)
    git("tag", "v0.36.0", cwd=work)
    return work


def check(base: str, head: str, tmp_path: Path, cwd: Path) -> subprocess.CompletedProcess:
    """Run the guard in `cwd`, which is the repository whose tags it will consult."""
    (tmp_path / "base.toml").write_text(base, encoding="utf-8")
    (tmp_path / "head.toml").write_text(head, encoding="utf-8")
    return run(
        sys.executable,
        str(GUARD),
        str(tmp_path / "base.toml"),
        str(tmp_path / "head.toml"),
        cwd=cwd,
    )


def test_the_collision_that_happened_is_refused(released, tmp_path):
    """#591's numbers: 0.35.0 at its base, 0.36.0 at its head, v0.36.0 already out.

    `pull_request.base.sha` is the base as of the pull request rather than the
    moving tip of main -- for #591 it was 908a24e, before the rival release, where
    the version was still 0.35.0. That is why the comparison sees a bump at all.
    """
    done = check(manifest("0.35.0"), manifest("0.36.0"), tmp_path, released)

    assert done.returncode == 1
    assert "v0.36.0 is already tagged" in done.stderr
    assert "publish nothing" in done.stderr


def test_a_merge_commit_cannot_see_the_collision_its_branch_could(released, tmp_path):
    """Why the guard runs on the branch and not after the merge.

    Once #591 merged, the merge commit's version and its first parent's were the
    same string -- both 0.36.0 -- so every "did this push bump the version" test
    downstream answers no. There is nothing left to catch by then, which is the
    whole argument for checking earlier, and the reason `publish.yml`'s own error
    arm is not a second answer to this case.
    """
    done = check(manifest("0.36.0"), manifest("0.36.0"), tmp_path, released)

    assert done.returncode == 0, done.stderr
    assert "proposes no new version" in done.stdout


def test_an_ordinary_branch_that_never_touches_the_version_passes(released, tmp_path):
    """The common case, and the one a false positive here would block outright."""
    done = check(manifest("0.36.0"), manifest("0.36.0"), tmp_path, released)

    assert done.returncode == 0, done.stderr


def test_a_release_cut_to_a_version_nobody_has_taken_passes(released, tmp_path):
    """The ritual this guard must not break."""
    done = check(manifest("0.36.0"), manifest("0.37.0"), tmp_path, released)

    assert done.returncode == 0, done.stderr
    assert "never been tagged" in done.stdout


def test_a_manifest_with_no_version_fails_rather_than_passes(released, tmp_path):
    """A guard that cannot read its input has checked nothing."""
    done = check("[workspace.package]\n", manifest("0.37.0"), tmp_path, released)

    assert done.returncode == 1
    assert "not passing" in done.stderr


def test_a_manifest_that_is_not_there_fails_rather_than_passes(released, tmp_path):
    done = run(
        sys.executable,
        str(GUARD),
        str(tmp_path / "absent.toml"),
        str(tmp_path / "also.toml"),
        cwd=released,
    )

    assert done.returncode == 1
    assert "not passing" in done.stderr


def test_ci_runs_the_guard_on_pull_requests():
    """It is worth a step only if a step actually runs it."""
    ci = CI.read_text(encoding="utf-8")

    assert "scripts/version_untaken.py" in ci
    assert "A version that is already published is not one to bump to" in ci
