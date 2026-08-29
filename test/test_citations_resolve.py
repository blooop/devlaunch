"""A citation spelled as a path is a pointer, so it has to point somewhere.

Comments and documents all over this repository name the test that pins the
behaviour being described. That habit is the repository's decision-vs-accident
record: a reader who wants to know whether something is load-bearing follows the
citation and reads the guard. Retiring the Python tree (#267) deleted about
forty of the files those citations named, and the citations stayed. A pointer
into nothing is worse than no pointer, because it still reads as evidence -- the
reader is told a guard exists, goes looking, finds nothing, and cannot tell
whether the guard moved, was renamed, or never existed.

So the rule is about the spelling rather than about tense. **A token spelled the
way a Python test file is spelled has to resolve to a file in this tree.** A
suite that retired is named without the extension, in a sentence that says it
retired -- `test_purge_ownership` (Python, retired in #267) rather than a path
somebody can try to open. That keeps the history, which is worth keeping and is
still greppable in `git log`, and it stops the history from impersonating a
live guard.

Two rules, because there are two spellings to catch:

- Any name shaped like a Python test module, wherever it appears: a path is
  checked as written, a bare filename against every test file in the tree. A
  bare name is allowed to resolve anywhere under `test/` on purpose, since a
  comment saying which directory a test lives in would rot on the first move.
- Any repository-relative path into `test/` or `rust/tools/`, whatever its
  extension. That second directory is gone entirely, and the fixtures under the
  first moved when the Python tests that used them went.

`CHANGELOG.md` and `docs/rust-port-scope.md` are out of scope, for the same
reason `test_docs_prose.py` excludes the archives: both are records of what was
true when they were written. A changelog entry describing a file that a later
release deleted is accurate history, and rewriting it would be editing the
record to satisfy a test.

`docs/rust-rewrite-plan.md` is *in* scope, unlike in that test, and the
difference is what the two rules are about. Its prose is an archive, but its
divergence table is cited by row number from comments and tests throughout
`rust/` and its rows say what pins each row -- which makes those particular
sentences live pointers however old the document is.
"""

from __future__ import annotations

import os
import re
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent

# Records rather than pointers: what they say was true when it was written.
ARCHIVAL = {"CHANGELOG.md", "docs/rust-port-scope.md"}

# The trees a citation can be written in. Everything else (a pixi environment, a
# vendored dependency, whatever a tool drops) is not prose this repository owns.
SOURCE_TREES = (".github", "docs", "rust", "scripts", "test")
# Cargo's build output lives inside one of them and is nobody's prose. It is also
# there or not there depending on whether anything has been built, which would
# make the set of cases this test collects depend on the state of the machine.
BUILD_OUTPUT = "rust/target"
SOURCE_SUFFIXES = {".md", ".py", ".rs", ".sh", ".toml", ".yaml", ".yml"}
TOP_LEVEL_DOCS = ("AGENTS.md", "CLAUDE.md", "README.md")

# Both patterns open on a lookbehind that refuses a match starting mid-token.
# Without it the second one reads `latest/install-guide.html` in an NVIDIA
# documentation URL as a path under `test/`, and the first reads any module
# whose name happens to end in the same letters as a citation of a shorter one.
BOUNDARY = r"(?<![\w./-])"
# A Python test module, with or without a directory in front of it.
TEST_MODULE = re.compile(BOUNDARY + r"(?:[\w.-]+/)*test_\w+\.py(?!\w)")
# A repository-relative path into a directory whose contents the retirement moved.
RETIRED_TREE_PATH = re.compile(BOUNDARY + r"(?:test|rust/tools)/[\w./-]+\.\w+")


def sources() -> list[Path]:
    """Every file in this repository that can carry a citation.

    Globbed rather than listed. A hand-written list is a list the next file is
    missing from, and a citation nothing reads is exactly the state this test
    exists to end.
    """
    found: list[Path] = []
    for name in TOP_LEVEL_DOCS:
        page = REPO_ROOT / name
        if page.is_file():
            found.append(page)
    found.extend(page for page in REPO_ROOT.glob("*.toml") if page.is_file())
    for tree in SOURCE_TREES:
        for directory, subdirectories, files in os.walk(REPO_ROOT / tree):
            here = Path(directory)
            # Pruned rather than filtered afterwards: cargo's output holds tens of
            # thousands of files and walking it is most of this test's runtime.
            subdirectories[:] = sorted(
                name
                for name in subdirectories
                if not (here / name).is_relative_to(REPO_ROOT / BUILD_OUTPUT)
            )
            found.extend(
                here / name for name in sorted(files) if Path(name).suffix in SOURCE_SUFFIXES
            )
    return [path for path in found if path.relative_to(REPO_ROOT).as_posix() not in ARCHIVAL]


SOURCES = sources()

# Every Python test file that exists, by basename, for the bare-name rule.
TEST_FILES = {path.name for path in (REPO_ROOT / "test").rglob("test_*.py") if path.is_file()}


def _resolves(citation: str) -> bool:
    # `is_file` rather than `exists`: every citation these patterns match is
    # spelled like a file, and the failure message tells the reader to open one.
    # A directory that happens to answer to the name would satisfy `exists` and
    # nothing a reader wants.
    if "/" in citation:
        return (REPO_ROOT / citation).is_file()
    return citation in TEST_FILES


def dangling(text: str) -> list[tuple[int, str]]:
    """Every citation in `text` that names nothing, with the line it sits on."""
    found: list[tuple[int, str]] = []
    for number, line in enumerate(text.splitlines(), start=1):
        for pattern in (TEST_MODULE, RETIRED_TREE_PATH):
            for match in pattern.finditer(line):
                citation = match.group(0)
                if not _resolves(citation) and (number, citation) not in found:
                    found.append((number, citation))
    return found


@pytest.mark.unit
def test_the_scope_covers_the_trees_citations_are_written_in():
    """The premise the rule below rests on.

    A glob that stopped matching would leave the rule collecting no files and
    reporting a clean run, which reads exactly like a run that passed. Asserted
    against the shape of the repository rather than a count, so that a file
    moving does not fail here for the wrong reason.
    """
    assert len(SOURCES) > 100, (
        f"only {len(SOURCES)} files are in scope; the glob no longer describes "
        "this repository and the rule below is checking almost nothing"
    )
    for tree in SOURCE_TREES:
        assert any(path.is_relative_to(REPO_ROOT / tree) for path in SOURCES), (
            f"nothing under {tree}/ is in scope, so citations written there are unchecked"
        )
    assert len(TEST_FILES) > 10, (
        "the set of Python test files this rule resolves bare names against is "
        f"{sorted(TEST_FILES)}, which cannot be right"
    )
    for name in ARCHIVAL:
        assert (REPO_ROOT / name).exists(), (
            f"{name} is excluded as a record and is not there; an exclusion that "
            "names nothing hides whether the rule still has the coverage it claims"
        )


@pytest.mark.unit
@pytest.mark.parametrize(
    "source", SOURCES, ids=lambda source: source.relative_to(REPO_ROOT).as_posix()
)
def test_every_cited_test_file_is_a_file(source):
    """A citation names a guard somebody can open, or it is not written as a path.

    The fix for a failure here is one of two things and never a third. If the
    behaviour is still pinned, cite the test that pins it now. If the test
    retired with the Python tree, name the suite without the `.py` and say it
    retired, so the sentence reads as the history it is.
    """
    offenders = dangling(source.read_text(encoding="utf-8"))
    assert not offenders, (
        f"{source.relative_to(REPO_ROOT)} cites {len(offenders)} path(s) that no "
        "file answers to. Re-point at the guard that pins the behaviour now, or "
        "name the retired suite without the extension:\n  "
        + "\n  ".join(f"{number}: {citation}" for number, citation in offenders[:8])
    )
