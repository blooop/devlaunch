"""Every relative link the documentation publishes, resolved the way a reader's
browser resolves it.

This exists because of a specific mistake, and the mistake is easy to repeat. The
README was cut back to an orientation document and its reference sections moved
into `docs/`; three links moved with the prose without being rewritten. They had
been written relative to the repository root, where the README lives, and from
`docs/development.md` `](AGENTS.md)` means `docs/AGENTS.md` and
`](docs/rust-rewrite-plan.md)` means `docs/docs/rust-rewrite-plan.md`. Neither
exists. On GitHub both render as ordinary links and 404 on click.

What makes it worth a test rather than a careful read is how the check was got
wrong the first time: a hand-rolled sweep tested each target with `-f "$target"`
from the repository root, so `AGENTS.md` was found -- at the wrong path, for the
wrong file -- and the sweep reported every link healthy. A link is only correct
relative to the document that carries it, so that is the only way to ask.

Anchors are checked in the same pass, against GitHub's slugging rules, because a
moved section takes its anchor with it and a `#fragment` that no longer resolves
is the other half of the same defect.

Not checked: external `http(s)://` links, which would make the suite depend on
the network and on other people's uptime.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent

# The documents a reader is sent to. AGENTS.md is in here because the README
# links to it and it links back; the archival planning documents under docs/ are
# in here because they are linked from docs/development.md.
DOCUMENTS = [
    REPO_ROOT / "README.md",
    REPO_ROOT / "AGENTS.md",
    *sorted((REPO_ROOT / "docs").glob("*.md")),
]

# Inline links only: `[text](target)`. Reference-style definitions and bare
# autolinks carry no relative paths in this repository, and a pattern that tried
# to cover every markdown spelling would match code fences too.
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")

# A fenced block's contents are not links, and this repository's fences are full
# of shell that looks like one (`[ -n "$X" ] && ...`).
FENCE = re.compile(r"^```.*?^```", re.MULTILINE | re.DOTALL)


def _prose(document: Path) -> str:
    """`document` with its fenced code blocks blanked out, lines preserved."""
    text = document.read_text(encoding="utf-8")
    return FENCE.sub(lambda match: "\n" * match.group(0).count("\n"), text)


def _slugs(document: Path) -> set[str]:
    """The anchors GitHub generates for `document`'s headings.

    Lowercase, non-alphanumerics dropped except hyphens and spaces, spaces to
    hyphens. Enough for the headings this repository writes; it does not model
    GitHub's duplicate-heading `-1` suffixes, and nothing here needs it to.
    """
    slugs = set()
    for line in _prose(document).splitlines():
        if not line.startswith("#"):
            continue
        heading = line.lstrip("#").strip()
        slug = re.sub(r"[^a-z0-9 _-]", "", heading.lower()).replace(" ", "-")
        if slug:
            slugs.add(slug)
    return slugs


def _links(document: Path) -> list[tuple[int, str]]:
    """Every inline link target in `document`, with the line it is written on."""
    found = []
    for number, line in enumerate(_prose(document).splitlines(), start=1):
        found.extend((number, target) for target in LINK.findall(line))
    return found


# Anything with a URI scheme in front of it is somebody else's to resolve. Matched
# as a scheme rather than as a list of the ones used today, so adding a `mailto:`
# or an `ftp:` link needs no edit here -- enumerating them would be guessing at
# which schemes the documentation will grow.
HAS_SCHEME = re.compile(r"^[a-z][a-z0-9+.-]*:", re.IGNORECASE)


def relative_links() -> list[tuple[Path, int, str]]:
    """Every link in every document that names a path rather than a URL."""
    collected = []
    for document in DOCUMENTS:
        for number, target in _links(document):
            if HAS_SCHEME.match(target):
                continue
            collected.append((document, number, target))
    return collected


RELATIVE_LINKS = relative_links()


@pytest.mark.unit
def test_the_documents_still_carry_relative_links():
    """The premise the parametrized check rests on.

    A rename that emptied `DOCUMENTS`, or a `LINK` pattern that stopped matching,
    would leave the check below collecting nothing and reporting a clean run --
    which reads exactly like a run that passed. The floor is deliberately low:
    this is a smoke test for the extraction, not an opinion about how many links
    the documentation should carry.
    """
    assert len(RELATIVE_LINKS) >= 20, (
        f"only {len(RELATIVE_LINKS)} relative links were extracted from "
        f"{[document.name for document in DOCUMENTS]}; either the documents stopped "
        "linking to each other or the extraction no longer recognises how they do it"
    )


@pytest.mark.unit
@pytest.mark.parametrize(
    ("document", "line", "target"),
    [pytest.param(*link, id=f"{link[0].name}:{link[1]}:{link[2]}") for link in RELATIVE_LINKS],
)
def test_every_relative_link_resolves_from_the_document_that_writes_it(document, line, target):
    """A relative link is resolved against its own document's directory.

    Both halves are asserted here rather than in two tests, because they are one
    question a reader asks by clicking: the file, and the heading inside it.
    """
    path, _, anchor = target.partition("#")

    if path:
        destination = (document.parent / path).resolve()
        assert destination.exists(), (
            f"{document.relative_to(REPO_ROOT)}:{line} links to {target!r}, which from "
            f"{document.parent.relative_to(REPO_ROOT) or '.'} means "
            f"{destination.relative_to(REPO_ROOT) if REPO_ROOT in destination.parents else destination}"
            " -- and nothing is there"
        )
    else:
        destination = document

    if anchor and destination.suffix == ".md":
        assert anchor in _slugs(destination), (
            f"{document.relative_to(REPO_ROOT)}:{line} links to {target!r}, but "
            f"{destination.relative_to(REPO_ROOT)} has no heading yielding the anchor "
            f"{anchor!r}"
        )
