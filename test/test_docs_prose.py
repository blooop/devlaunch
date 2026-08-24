"""Two prose rules the documentation states about itself, asked of the documents.

AGENTS.md tells the next contributor that the reader-facing documentation carries
no em dashes and that a page under `docs/` does not call itself the README. Both
were written as guidance, and guidance that nothing checks is guidance that is
already false somewhere -- the em-dash rule was false the moment it was written,
because it said "in `docs/`" while two archival planning documents in that
directory carry 74 em dashes between them.

So the rule is scoped here to the pages the README's Docs table sends a reader
to, and asserted, which is the only version of it worth stating. The two archival
documents are deliberately outside it: `rust-rewrite-plan.md` and
`rust-port-scope.md` are records of a port that has already happened, and
rewriting their prose would edit a historical record to satisfy a style rule.

The em-dash rule is a house style rather than a correctness property, and it is
here rather than in a linter because it is about six specific files and nothing
else in the tree.
"""

from __future__ import annotations

from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent

# The pages a reader is sent to, which is the scope of both rules below. Named
# individually rather than globbed, because the point of the list is that it
# excludes the archival documents that share the directory.
READER_FACING = [
    REPO_ROOT / "README.md",
    REPO_ROOT / "docs" / "cli.md",
    REPO_ROOT / "docs" / "workspaces.md",
    REPO_ROOT / "docs" / "workspace-tools.md",
    REPO_ROOT / "docs" / "cleanup.md",
    REPO_ROOT / "docs" / "performance.md",
    REPO_ROOT / "docs" / "development.md",
    REPO_ROOT / "docs" / "devcontainer-projects.md",
]

EM_DASH = "—"
EN_DASH = "–"


@pytest.mark.unit
def test_the_reader_facing_pages_exist():
    """A renamed page would otherwise drop silently out of both rules below."""
    missing = [
        page.relative_to(REPO_ROOT).as_posix() for page in READER_FACING if not page.exists()
    ]
    assert not missing, (
        f"{missing} are listed as reader-facing documentation and are not there; "
        "a page that was renamed has to be renamed here too, or it stops being checked"
    )


@pytest.mark.unit
@pytest.mark.parametrize(
    "page", READER_FACING, ids=lambda page: page.relative_to(REPO_ROOT).as_posix()
)
def test_no_em_dashes_in_reader_facing_documentation(page):
    """House style: an em dash reads as machine-written, so end the sentence.

    En dashes count too. They are the same reach for a dash by another name, and
    a numeric range reads as well with "to".
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
    "page",
    [page for page in READER_FACING if page.parent.name == "docs"],
    ids=lambda page: page.relative_to(REPO_ROOT).as_posix(),
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
