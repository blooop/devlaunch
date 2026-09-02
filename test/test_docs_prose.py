"""Two prose rules the documentation states about itself, asked of the documents.

AGENTS.md tells the next contributor that the reader-facing documentation carries
no em or en dashes and that a page under `docs/` does not call itself the README.
Both were written as guidance, and guidance that nothing checks is guidance that
is already false somewhere -- the dash rule was false the moment it was written,
because it said "in `docs/`" while two archival planning documents in that
directory carry 74 em dashes between them.

So the scope is derived here rather than trusted, and asserted, which is the only
version of the rule worth stating. The two archival documents are the one thing
excluded by hand, with the reason: they record a port that has already happened,
and rewriting their prose would edit a record to satisfy a style rule.

The dash rule is house style rather than a correctness property, and it is here
rather than in a linter because it is about a handful of documents and nothing
else in the tree.
"""

from __future__ import annotations

from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent

# The two documents under `docs/` that these rules do not reach, and the only
# thing about the scope that is written down by hand. Both are records of a port
# that has already happened; rewriting their prose to satisfy a style rule would
# be editing a record to make a test pass.
ARCHIVAL = {"rust-rewrite-plan.md", "rust-port-scope.md"}


def reader_facing() -> list[Path]:
    """The pages a reader is sent to: the README, and `docs/` minus the archives.

    Derived rather than listed, and that direction matters. A hand-written list
    is a list a new page is missing from, so the page would be added, linked from
    the README's Docs table, and silently checked by nothing -- which is the
    failure mode every guard in this repository is written to avoid. Globbing
    instead makes a new page in scope the moment it exists, and takes the only
    judgement call, "this one is an archive", out into ARCHIVAL above where
    dropping a page is a visible edit rather than an omission.
    """
    return [
        REPO_ROOT / "README.md",
        *sorted(page for page in (REPO_ROOT / "docs").glob("*.md") if page.name not in ARCHIVAL),
    ]


READER_FACING = reader_facing()
DOCS_PAGES = [page for page in READER_FACING if page.parent.name == "docs"]

EM_DASH = "—"
EN_DASH = "–"


@pytest.mark.unit
def test_the_scope_resolves_to_the_pages_a_reader_is_sent_to():
    """The premise both parametrized rules rest on.

    A glob that matched nothing, or an ARCHIVAL that grew until it covered the
    directory, would leave the rules below collecting no cases and reporting a
    clean run -- which reads exactly like a run that passed. Asserted for both
    lists, because the second is a filter of the first and can empty on its own.
    """
    assert len(READER_FACING) >= 5, (
        f"only {[page.name for page in READER_FACING]} resolved as reader-facing "
        "documentation; the glob or ARCHIVAL no longer describes this repository"
    )
    assert DOCS_PAGES, "no pages under docs/ are in scope, so the README rule below checks nothing"
    for name in ARCHIVAL:
        assert (REPO_ROOT / "docs" / name).exists(), (
            f"docs/{name} is excluded as archival and is not there; an exclusion "
            "that names nothing hides whether the rule still has the coverage it claims"
        )


@pytest.mark.unit
@pytest.mark.parametrize(
    "page", READER_FACING, ids=lambda page: page.relative_to(REPO_ROOT).as_posix()
)
def test_no_em_or_en_dashes_in_reader_facing_documentation(page):
    """House style: an em dash reads as machine-written, so end the sentence.

    En dashes count too, which is why they are in the name. They are the same
    reach for a dash by another name, and a numeric range reads as well with
    "to". AGENTS.md states both.
    """
    text = page.read_text(encoding="utf-8")
    offenders = []
    for number, line in enumerate(text.splitlines(), start=1):
        if EM_DASH in line or EN_DASH in line:
            offenders.append(f"{number}: {line.strip()}")
    assert not offenders, (
        f"{page.relative_to(REPO_ROOT)} uses an em or en dash on "
        f"{len(offenders)} line(s). Use a full stop or a comma, or end the "
        "sentence:\n  " + "\n  ".join(offenders[:5])
    )


@pytest.mark.unit
@pytest.mark.parametrize(
    "page", DOCS_PAGES, ids=lambda page: page.relative_to(REPO_ROOT).as_posix()
)
def test_no_docs_page_calls_itself_the_readme(page):
    """The reference material was one document with the README, and says so.

    Six sentences survived the split still describing the README's layout from
    inside a `docs/` page -- "the rest of this README", "the figures above are
    this README's". They are cheap to write and invisible to read, because they
    were true where they were written.
    """
    text = page.read_text(encoding="utf-8")
    assert "this README" not in text, (
        f"{page.relative_to(REPO_ROOT)} refers to itself as 'this README'; it is a "
        "page under docs/. Name the page, or name the README explicitly if that is "
        "what is meant"
    )


# The four line prefixes git writes into a file it could not merge for you. The
# base marker is the one that matters here: `merge.conflictStyle = diff3` (and
# `zdiff3`) adds it, so a resolver written against the familiar three leaves it
# behind, and it is the only marker whose line carries prose after it that can
# read as a heading.
CONFLICT_MARKERS = ("<<<<<<<", "|||||||", "=======", ">>>>>>>")


@pytest.mark.unit
@pytest.mark.parametrize(
    "page", READER_FACING, ids=lambda page: page.relative_to(REPO_ROOT).as_posix()
)
def test_no_reader_facing_page_carries_a_merge_conflict_marker(page):
    """A resolved conflict that left a marker in the published prose.

    `check-merge-conflict` in .pre-commit-config.yaml is not this: without
    `--assume-in-merge` it does nothing unless the working tree is mid-merge, so
    a conflict resolved out of `git stash pop` -- which leaves no MERGE_HEAD --
    walks straight past it. That is how `||||||| Stash base` reached line 771 of
    workspace-tools.md and was committed.

    Nothing else caught it either, and the near misses are worth naming. The
    contract tests read one named section each, so a marker in a neighbouring
    section is invisible to them. The dash rule above reads every line of the
    same pages and had no reason to look at these seven characters. And the
    marker renders as literal text, so the page is wrong only for a reader.

    Scoped to the pages a reader is sent to rather than the tree, because that is
    where a stray marker is a published defect rather than a mess in a branch.
    """
    offenders = [
        f"{number}: {line.rstrip()}"
        for number, line in enumerate(page.read_text(encoding="utf-8").splitlines(), start=1)
        if line.startswith(CONFLICT_MARKERS)
    ]
    assert not offenders, (
        f"{page.relative_to(REPO_ROOT)} carries {len(offenders)} merge conflict "
        "marker line(s); a conflict was resolved without removing them:\n  "
        + "\n  ".join(offenders[:5])
    )
