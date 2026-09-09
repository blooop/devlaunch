"""The four answers `publish.yml` can give, exercised as the shell it actually is.

The step decides whether a push to `main` is a release, and it now has four arms
rather than two. Three of them decline to publish and one of those is an *error*,
which is the whole point of the change: blooop/devlaunch#591 merged a version bump
onto a number another branch had already released, and the workflow reported
`nothing to publish` -- the same sentence an ordinary push gets -- so the work sat
on main released nowhere with no check red.

The script is extracted from the workflow and run, rather than reimplemented here.
A rewritten copy would be a second hand-maintained version of the logic and would
pass while the real one rotted; running the real text means the test fails if the
step is edited into disagreeing with it.

What this does **not** claim: that the error arm would have caught #591. It would
not have, and `test_the_case_that_needs_the_pull_request_guard` pins that instead.
By the time #591 reached main its merge carried the same version as its first
parent, so there was no bump left to see. `scripts/version_untaken.py` is the guard
for that, and it runs on the pull request.
"""

import os
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).parent.parent
PUBLISH = ROOT / ".github" / "workflows" / "publish.yml"
STEP = "      - name: Decide whether there is a release to publish\n"


def decide_script() -> str:
    """The `run:` body of the decision step, dedented out of the workflow."""
    text = PUBLISH.read_text(encoding="utf-8")
    assert STEP in text, "the decision step is not named what this test looks for"
    body = text.split(STEP, 1)[1].split("run: |\n", 1)[1]
    lines = []
    for line in body.splitlines():
        if line.strip() and not line.startswith(" " * 10):
            break
        lines.append(line[10:])
    return "\n".join(lines)


def git(*args: str, cwd: Path) -> None:
    done = subprocess.run(("git", *args), cwd=cwd, capture_output=True, text=True, check=False)
    assert done.returncode == 0, f"git {' '.join(args)} failed: {done.stderr}"


@pytest.fixture(name="repo")
def a_repository_to_decide_about(tmp_path: Path) -> Path:
    work = tmp_path / "repo"
    (work / "rust").mkdir(parents=True)
    git("init", "-q", "-b", "main", ".", cwd=work)
    git("config", "user.email", "publish@example.invalid", cwd=work)
    git("config", "user.name", "Publish Fixture", cwd=work)
    return work


def commit(repo: Path, version: str) -> None:
    (repo / "rust" / "Cargo.toml").write_text(
        f'[workspace.package]\nversion = "{version}"\n', encoding="utf-8"
    )
    git("add", "-A", cwd=repo)
    git("commit", "-qm", f"v{version}", cwd=repo)


def decide(repo: Path, tmp_path: Path) -> subprocess.CompletedProcess:
    output = tmp_path / "github-output"
    output.write_text("", encoding="utf-8")
    script = tmp_path / "decide.sh"
    script.write_text(decide_script(), encoding="utf-8")
    return subprocess.run(
        ("bash", str(script)),
        cwd=repo,
        capture_output=True,
        text=True,
        check=False,
        # The runner's own PATH, not a guessed one: the script needs `git` and
        # `sed`, and hardcoding where they live is a test that passes here and
        # fails on a machine that puts them somewhere else.
        env={"PATH": os.environ["PATH"], "GITHUB_OUTPUT": str(output)},
    )


def test_a_bump_to_a_version_nobody_has_taken_publishes(repo, tmp_path):
    commit(repo, "0.36.0")
    commit(repo, "0.37.0")

    done = decide(repo, tmp_path)

    assert done.returncode == 0, done.stderr
    assert "publishing" in done.stdout


def test_a_re_run_over_the_commit_that_published_declines_quietly(repo, tmp_path):
    """Idempotence, and the ordinary reason the tag is found at all.

    `gh release create --target $GITHUB_SHA` puts the tag on the commit that
    published, so a re-run sees it on HEAD and must do nothing rather than fail.
    """
    commit(repo, "0.36.0")
    commit(repo, "0.37.0")
    git("tag", "v0.37.0", cwd=repo)

    done = decide(repo, tmp_path)

    assert done.returncode == 0, done.stderr
    assert "published from this commit" in done.stdout


def test_a_later_push_that_did_not_touch_the_version_declines_quietly(repo, tmp_path):
    commit(repo, "0.36.0")
    commit(repo, "0.37.0")
    git("tag", "v0.37.0", cwd=repo)
    (repo / "unrelated.txt").write_text("a change that is not a release\n", encoding="utf-8")
    git("add", "-A", cwd=repo)
    git("commit", "-qm", "later", cwd=repo)

    done = decide(repo, tmp_path)

    assert done.returncode == 0, done.stderr
    assert "already tagged; nothing to publish" in done.stdout


def test_a_bump_onto_a_version_already_tagged_is_an_error_not_a_shrug(repo, tmp_path):
    """The arm that is new: declining, but loudly enough to act on."""
    commit(repo, "0.36.0")
    git("tag", "v0.37.0", cwd=repo)
    commit(repo, "0.37.0")

    done = decide(repo, tmp_path)

    assert done.returncode == 1
    assert "::error::" in done.stdout
    assert "already tagged" in done.stdout


def test_the_case_that_needs_the_pull_request_guard(repo, tmp_path):
    """#591's shape: the merge carries the version its first parent already had.

    Nothing here can see a bump, so this step correctly says nothing to publish and
    correctly does not error. That silence is exactly why the collision has to be
    refused on the branch instead, which is `scripts/version_untaken.py`.
    """
    commit(repo, "0.36.0")
    git("tag", "v0.36.0", cwd=repo)
    (repo / "unrelated.txt").write_text("the merge's other side\n", encoding="utf-8")
    git("add", "-A", cwd=repo)
    git("commit", "-qm", "a merge carrying the same version", cwd=repo)

    done = decide(repo, tmp_path)

    assert done.returncode == 0, done.stderr
    assert "::error::" not in done.stdout
