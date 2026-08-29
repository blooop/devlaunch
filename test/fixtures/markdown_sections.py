"""Reading one section out of a markdown document, for tests that assert on prose.

Shared because three files aim assertions at README sections and each of them
needs the same thing: the text under one heading and nothing near it. It used to
live in the Python `test_lending_doc` (retired with the Python tree in #267) and be
imported from there, which made two unrelated test files depend on a third for a
helper that is about markdown rather than about lending.
"""

from __future__ import annotations

import re
from pathlib import Path


def section(document: Path, heading: str) -> str:
    """The text under `heading`, up to the next heading of its own level or above.

    Level-aware, so a `###` subsection ends where the next `###` begins rather
    than swallowing the rest of its `##` parent -- that difference is what lets
    an assertion be aimed at the recipe instead of at everything near it.

    The heading must occur exactly once and at the start of a line: a renamed or
    deleted heading is the likeliest way these tests break and has to say so
    rather than surface as an empty string, and a bare `find` would just as
    happily match the same words quoted inside a code fence.
    """
    text = document.read_text(encoding="utf-8")
    level = len(heading) - len(heading.lstrip("#"))
    matches = list(re.finditer(rf"^{re.escape(heading)}$", text, re.MULTILINE))
    assert len(matches) == 1, (
        f"{document.name} has {len(matches)} headings {heading!r} at the start of a line; "
        "expected exactly one -- it was renamed, removed or duplicated"
    )
    rest = text[matches[0].end() :]
    end = re.search(rf"^#{{1,{level}}} ", rest, re.MULTILINE)
    return rest if end is None else rest[: end.start()]
