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

import os
import re
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "public-api-snapshots.sh"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
# The development material moved out of the README into docs/ when the README was
# cut back to an orientation document. The guard is about a reader having the
# canonical-path limit written down somewhere they are sent, not about which file
# carries it.
DEV_DOC = REPO_ROOT / "docs" / "development.md"

# What the failure messages call it, so a renamed target renames itself in all of
# them rather than in none.
DOC = DEV_DOC.relative_to(REPO_ROOT).as_posix()
RUST = REPO_ROOT / "rust"

# The step whose shell the executable checks at the bottom of this file run.
CI_CHECK_STEP = "The public surface is the snapshots the repo carries"
# The classification itself: the pattern that decides "promise" from "rest".
API_ROW_PATTERN = "devlaunch_core::api\\b"
# The other snapshot, named in the docs because the widened classifier still does
# not reach everything a promised signature does: a type the `api` module never
# re-exports but a promised method returns is reachable from outside and lives
# only here, so a diff in it can still be a contract change.
REST_SNAPSHOT = "public-api.rest.txt"


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
    document = DEV_DOC.read_text(encoding="utf-8")
    assert "### The public-API snapshots" in document, (
        f"{DOC} has no section on the snapshots; a red tick that only explains "
        "itself in a workflow's error string is a thing people learn by breaking"
    )
    assert "scripts/public-api-snapshots.sh" in document, (
        f"the {DOC} section does not name the command that regenerates them"
    )


@pytest.mark.unit
def test_the_ci_error_string_sends_a_reader_to_the_document_that_has_the_section():
    """The other half of the test above, and the half that was missing.

    That one asserts the section exists somewhere; this one asserts the failing
    job points at the file it is in. Both passed while the error string still
    said README.md and the section had moved to docs/ -- so the guard against "a
    red tick that only explains itself in a workflow's error string" was
    satisfied by a red tick that explained itself by naming the wrong document.
    """
    job = ci_job("public-api")
    assert "The public-API snapshots" in job, (
        "the public-api job no longer quotes the section name, so this check "
        "cannot tell which document it is sending a reader to"
    )
    assert DOC in job, (
        f"the public-api job quotes 'The public-API snapshots' but does not name "
        f"{DOC}, which is the document that carries it; a reader following the "
        "error string is sent somewhere the section is not"
    )


@pytest.mark.unit
def test_the_docs_say_how_the_promise_file_is_filled_and_what_it_still_misses():
    """The overclaim this section is one edit away from becoming again.

    `cargo public-api` renders methods and impls only at a type's canonical
    path, so the classifier reaches `api::Launch::run` by resolving each `api`
    re-export back to the path it names (#352). Two things a reader has to be
    told, and both live wherever the promise file is described: that the
    canonical-path rows in it are there on purpose and are not strays, and that
    the rest file can still carry a contract change, because a type the `api`
    module never re-exports but a promised signature hands back is reachable
    from outside and is classified as binary surface.
    """
    for path in (DEV_DOC, SCRIPT, RUST / "devlaunch-core" / "src" / "lib.rs"):
        text = path.read_text(encoding="utf-8")
        assert "canonical" in text, (
            f"{path.name} describes the promise file without the canonical-path "
            "rendering that decides how it is filled"
        )
        assert REST_SNAPSHOT in text, (
            f"{path.name} describes the promise file without naming {REST_SNAPSHOT}, "
            "which is where a promised signature's own types are still classified"
        )


def classify(kind: str, rows: list[str]) -> list[str]:
    """One side of the split, as the script itself decides it.

    The classification is a filter over rows, and the script is the only place
    it exists -- so this exercises the real one on rows chosen to be awkward,
    rather than restating the pattern here where the restatement is what would
    be tested.
    """
    done = subprocess.run(
        [str(SCRIPT), "--classify", kind],
        input="".join(f"{row}\n" for row in rows),
        capture_output=True,
        text=True,
        check=True,
    )
    return done.stdout.splitlines()


# Rows in the shape `cargo public-api` renders, naming types this crate really
# has, so the classifier is exercised against the `api` module as it stands
# rather than against a fiction. `Launch` and `Host` are re-exported; the branch
# manager is not.
PROMISED_ROWS = [
    "pub struct devlaunch_core::api::Launch<'a, 'r, 'l>",
    "impl<'a, 'r, 'l> devlaunch_core::flows::launch::Launch<'a, 'r, 'l>",
    "pub fn devlaunch_core::flows::launch::Launch<'a, 'r, 'l>::run(&mut self) -> ()",
    "impl core::fmt::Debug for devlaunch_core::flows::launch::Host",
]
UNPROMISED_ROWS = [
    "pub mod devlaunch_core",
    "pub struct devlaunch_core::flows::branch_manager::BranchManager<'a>",
    # The trap the whole rule turns on: a promised type in an argument, on a
    # method of something that is not promised at all.
    "pub fn devlaunch_core::flows::branch_manager::BranchManager<'a>::adopt"
    "(&self, &devlaunch_core::flows::launch::Host) -> ()",
    # And the boundary the `\b` is for.
    "pub mod devlaunch_core::apiary",
]


@pytest.mark.unit
def test_a_promised_types_canonical_rows_are_classified_as_promise():
    """The finding this ticket is about, at the seam that decides it.

    `Launch::run` is rendered at `flows::launch::Launch`, never at the `api`
    path it is re-exported under, so a classifier that matches the `api` path
    alone keeps the type's declaration and drops its only method.
    """
    kept = classify("api", PROMISED_ROWS + UNPROMISED_ROWS)
    assert kept == PROMISED_ROWS, (
        "the classifier does not claim a promised type's canonical-path rows, so "
        "renaming Launch::run leaves the promise file byte-identical"
    )


@pytest.mark.unit
def test_naming_a_promised_type_in_a_signature_does_not_promise_the_signature():
    """The control, and the reason the rule is anchored rather than a substring.

    Promised types are arguments and return types all over this crate. Claiming
    every row that mentions one would pull most of the binary surface into the
    promise file, which is the failure the split exists to prevent, wearing the
    other hat.
    """
    left = classify("rest", PROMISED_ROWS + UNPROMISED_ROWS)
    assert left == UNPROMISED_ROWS, (
        "the classifier claimed a row that only mentions a promised type, or "
        "dropped one that mentions nothing promised at all"
    )


@pytest.mark.unit
def test_the_two_sides_of_the_classification_are_a_partition():
    rows = PROMISED_ROWS + UNPROMISED_ROWS
    kept, left = classify("api", rows), classify("rest", rows)
    assert sorted(kept + left) == sorted(rows), (
        "a row was dropped by both sides or kept by both; the two files are "
        "complements or they are not a split at all"
    )


def ci_step_script(job: str, step_name: str) -> str:
    """The shell of one step of a job, dedented and runnable.

    The checks below run the workflow's own text rather than a paraphrase of
    it, because a paraphrase is another copy: the day the two disagree, the
    test still passes and the thing that runs on a runner is the other one.
    """
    lines = job.splitlines()
    at = next(i for i, line in enumerate(lines) if line.strip() == f"- name: {step_name}")
    run = next(i for i in range(at, len(lines)) if lines[i].strip() == "run: |")
    body: list[str] = []
    indent = None
    for line in lines[run + 1 :]:
        if not line.strip():
            body.append("")
            continue
        here = len(line) - len(line.lstrip())
        if indent is None:
            indent = here
        elif here < indent:
            break
        body.append(line[indent:])
    assert body, f"the {step_name!r} step has no shell to run"
    return "\n".join(body)


def run_the_ci_check(tmp_path: Path, listed: list[str], differing: str | None = None):
    """Run ci.yml's check step in a fake checkout, against a stubbed script.

    The stub stands in for the regeneration -- there is no nightly toolchain in
    this environment, and none is needed to test the *checking*. It writes
    whatever it claims to write, so the step's diff has something real to
    compare; ``listed`` is what its ``--print-files`` prints.
    """
    root = tmp_path / "checkout"
    (root / "scripts").mkdir(parents=True)
    stub = root / "scripts" / "public-api-snapshots.sh"
    stub.write_text(
        "#!/usr/bin/env bash\n"
        "set -euo pipefail\n"
        f"listed=({' '.join(listed)})\n"
        'if [ "${1:-}" = "--print-files" ]; then\n'
        '  [ ${#listed[@]} -eq 0 ] || printf "%s\\n" "${listed[@]}"\n'
        "  exit 0\n"
        "fi\n"
        'dest="$1"\n'
        'for file in ${listed[@]+"${listed[@]}"}; do\n'
        '  mkdir -p "$dest/$(dirname "$file")"\n'
        '  echo generated > "$dest/$file"\n'
        "done\n",
        encoding="utf-8",
    )
    stub.chmod(0o755)
    for name in listed:
        checked_in = root / "rust" / name
        checked_in.parent.mkdir(parents=True, exist_ok=True)
        checked_in.write_text("drifted\n" if name == differing else "generated\n", encoding="utf-8")
    runner_temp = tmp_path / "runner-temp"
    runner_temp.mkdir()
    return subprocess.run(
        ["bash", "-c", ci_step_script(ci_job("public-api"), CI_CHECK_STEP)],
        cwd=root,
        env={"PATH": os.environ["PATH"], "RUNNER_TEMP": str(runner_temp)},
        capture_output=True,
        text=True,
        check=False,
    )


@pytest.mark.unit
def test_the_ci_check_passes_when_every_snapshot_matches(tmp_path):
    """The control: without it, the two failures below prove only a broken harness."""
    done = run_the_ci_check(tmp_path, ["a/one.txt", "b/two.txt", "c/three.txt"])
    assert done.returncode == 0, done.stdout + done.stderr


@pytest.mark.unit
def test_the_ci_check_fails_when_a_snapshot_differs(tmp_path):
    done = run_the_ci_check(tmp_path, ["a/one.txt", "b/two.txt"], differing="b/two.txt")
    assert done.returncode != 0, "a changed surface passed the check"


@pytest.mark.unit
def test_the_ci_check_fails_when_it_compared_nothing(tmp_path):
    """A job that compares no file must not report that nothing changed.

    `set -euo pipefail` does not see a process substitution fail, and a
    `while read` over no input runs its body zero times and leaves the
    changed-flag at 0 -- so an empty file list, or a `--print-files` that
    exited non-zero, reported success having diffed nothing at all.
    """
    done = run_the_ci_check(tmp_path, [])
    assert done.returncode != 0, (
        "the public-api job passed having compared nothing; a tripwire that "
        "reports success without checking is the failure this ticket is about"
    )
    assert "compared" in done.stdout + done.stderr, (
        "the failure does not say that nothing was compared, so whoever hits it "
        "will look for a surface change that is not there"
    )
