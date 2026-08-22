"""The public-API snapshots have one owner, and it is findable (#312, #338).

Three files are checked in now instead of one -- the promised `api` tier, the
binary-surface tripwire, and the runner crate's own seam -- and what makes that
worth having is a single definition of which row belongs where, and which files
exist at all. Both live in ``scripts/public-api-snapshots.sh``. Copy either into
the workflow "just to check" and the copies drift: the day they disagree, the
promise file still exists and still passes, while the promise itself has
quietly moved into the file reviewers skim, or a fourth snapshot is generated
and never diffed.

So this guards the wiring rather than the surface -- CI's own diff is what
guards the surface -- plus the one documentation claim that a reader would act
on: what the promise file does *not* cover. Each check below is a thing
somebody could plausibly write, with the reason it must not be written.
"""

import re
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "public-api-snapshots.sh"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
README = REPO_ROOT / "README.md"
RUST = REPO_ROOT / "rust"

# The classification itself: the pattern that decides "promise" from "rest".
API_ROW_PATTERN = "devlaunch_core::api\\b"
# The ticket that widens the classifier to cover a promised type's methods and
# impls. Named in the docs so the gap is a known follow-up rather than folklore.
WIDENING_TICKET = "352"


def script_files() -> list[str]:
    """The snapshots the script says it writes, as it says them.

    Asking the script rather than listing them here for the same reason CI asks
    it: this file is one of the places a stale copy could hide.
    """
    printed = subprocess.run(
        [str(SCRIPT), "--print-files"],
        capture_output=True,
        text=True,
        check=True,
    )
    return printed.stdout.split()


def ci_job(name: str) -> str:
    """One job of ci.yml, as text.

    A slice rather than a parse: there is no YAML parser in this project's
    dependencies. The slice ends at the next line that starts a sibling key at
    the jobs' own indentation -- two spaces then non-space -- rather than at the
    first blank line, which a cosmetic blank line inside a job's `run:` block
    would otherwise be mistaken for.
    """
    text = CI_WORKFLOW.read_text(encoding="utf-8")
    start = text.index(f"\n  {name}:\n") + 1
    sibling = re.compile(r"^  \S", re.MULTILINE)
    following = sibling.search(text, start + 1)
    job = text[start : following.start()] if following else text[start:]
    # A slice that lost the job's own body would make every assertion below
    # vacuous, and a slice that ran to EOF would make them all trivially true.
    assert "runs-on:" in job, f"the {name} job slice does not contain the job"
    return job


@pytest.mark.unit
def test_every_snapshot_the_script_writes_is_checked_in():
    missing = [name for name in script_files() if not (RUST / name).is_file()]
    assert not missing, f"the script writes these files and the repo does not carry them: {missing}"


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
    # Settings, not commentary: the job's comment block explains the tiers and
    # names the api path to do it, which is where that explanation belongs.
    settings = "\n".join(
        line for line in ci_job("public-api").splitlines() if not line.lstrip().startswith("#")
    )
    assert "devlaunch_core::api" not in settings, (
        "ci.yml classifies rows itself; it must run the script instead, or the two "
        "definitions of 'promised' will drift apart"
    )


@pytest.mark.unit
def test_ci_takes_the_file_list_from_the_script_rather_than_repeating_it():
    job = ci_job("public-api")
    assert "scripts/public-api-snapshots.sh" in job, "the public-api job does not run the script"
    assert "--print-files" in job, (
        "the public-api job does not ask the script which files to diff; a list it "
        "keeps itself is a list a fourth snapshot can fall off"
    )
    # The machinery, not the prose: the error message may well name the promise
    # file, because telling the developer which diff means what is its whole
    # job. What must not name one is the check itself -- a `diff` line with a
    # path in it is a list of files to compare that lives here.
    comparisons = [line for line in job.splitlines() if "diff -u" in line]
    assert comparisons, "the public-api job compares nothing"
    for line in comparisons:
        duplicated = [name for name in script_files() if name in line]
        assert not duplicated, (
            f"the diff in ci.yml names snapshot paths of its own ({duplicated}); "
            "--print-files is there so it does not have to"
        )


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


@pytest.mark.unit
def test_the_docs_say_what_the_promise_file_does_not_cover():
    """The overclaim this section is one edit away from becoming again.

    `cargo public-api` renders methods and impls only at a type's canonical
    path, so `api::Launch::run` is in the *rest* file and renaming it leaves the
    promise file byte-identical. A guard that is trusted and silently does not
    fire is worse than no guard, so the limit is documented where the guard is,
    and the ticket that closes it is named.
    """
    for path in (README, SCRIPT, RUST / "devlaunch-core" / "src" / "lib.rs"):
        text = path.read_text(encoding="utf-8")
        assert "canonical" in text, (
            f"{path.name} describes the promise file without the canonical-path limit "
            "that decides what it can see"
        )
        assert WIDENING_TICKET in text, (
            f"{path.name} states the limit without naming issue #{WIDENING_TICKET}, "
            "which is what turns a known gap into a tracked one"
        )
