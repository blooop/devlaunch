"""The public-API snapshots have one owner, and it is findable (#312, #338).

Three files are checked in now instead of one -- the promised `api` tier, the
binary-surface tripwire, and the runner crate's own seam -- and what makes that
worth having is a single definition of which row belongs where. That definition
is a `grep` in ``scripts/public-api-snapshots.sh``. Copy it into the workflow
"just to check" and the copies drift: the day they disagree, the promise file
still exists and still passes, while the promise itself has quietly moved into
the file reviewers skim.

So this guards the wiring rather than the surface -- CI's own diff is what
guards the surface. Each check below is a thing somebody could plausibly write,
with the reason it must not be written.
"""

from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "public-api-snapshots.sh"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
README = REPO_ROOT / "README.md"
RUST = REPO_ROOT / "rust"

# The three files, and the promise each one carries.
SNAPSHOTS = (
    RUST / "devlaunch-core" / "public-api.api.txt",
    RUST / "devlaunch-core" / "public-api.rest.txt",
    RUST / "devlaunch-runner" / "public-api.txt",
)
# The classification itself: the pattern that decides "promise" from "rest".
API_ROW_PATTERN = "devlaunch_core::api\\b"


def ci_job(name: str) -> str:
    """One job of ci.yml, as text.

    A slice rather than a parse: there is no YAML parser in this project's
    dependencies, and the jobs are the only things at that indentation.
    """
    text = CI_WORKFLOW.read_text(encoding="utf-8")
    start = text.index(f"\n  {name}:\n")
    rest = text[start + 1 :]
    end = rest.find("\n\n  ")
    return rest if end == -1 else rest[:end]


@pytest.mark.unit
def test_every_snapshot_the_split_promises_is_checked_in():
    missing = [str(path.relative_to(REPO_ROOT)) for path in SNAPSHOTS if not path.is_file()]
    assert not missing, f"the split names these files and the repo does not carry them: {missing}"


@pytest.mark.unit
def test_the_one_snapshot_the_split_replaced_is_gone():
    combined = RUST / "devlaunch-core" / "public-api.txt"
    assert not combined.exists(), (
        "the pre-split snapshot is still here; a second file describing the same "
        "surface is one nobody regenerates and everybody trusts"
    )


@pytest.mark.unit
def test_the_classification_lives_in_the_script_alone():
    assert API_ROW_PATTERN in SCRIPT.read_text(encoding="utf-8"), (
        "the split filter is not in the regeneration script"
    )
    assert "devlaunch_core::api" not in CI_WORKFLOW.read_text(encoding="utf-8"), (
        "ci.yml classifies rows itself; it must run the script instead, or the two "
        "definitions of 'promised' will drift apart"
    )


@pytest.mark.unit
def test_ci_checks_all_three_snapshots_by_running_the_script():
    job = ci_job("public-api")
    assert "scripts/public-api-snapshots.sh" in job, "the public-api job does not run the script"
    for path in SNAPSHOTS:
        relative = str(path.relative_to(RUST))
        assert relative in job, f"the public-api job never diffs {relative}"


@pytest.mark.unit
def test_ci_installs_the_version_the_script_pins():
    job = ci_job("public-api")
    assert "--print-pin" in job, (
        "ci.yml names a cargo-public-api version of its own; the pin belongs to the "
        "script that renders the snapshots, since a different renderer makes a "
        "whole-file diff that says nothing"
    )


@pytest.mark.unit
def test_regenerating_is_documented_outside_the_ci_error_string():
    readme = README.read_text(encoding="utf-8")
    assert "### The public-API snapshots" in readme, (
        "README has no section on the snapshots; a red tick that only explains "
        "itself in a workflow's error string is a thing people learn by breaking"
    )
    assert "scripts/public-api-snapshots.sh" in readme, (
        "the README section does not name the command that regenerates them"
    )
