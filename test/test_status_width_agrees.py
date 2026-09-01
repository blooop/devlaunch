"""`docs/performance.md` spells the listing's fan-out width, so the two are diffed.

`STATUS_TRIPS_AT_ONCE` in `rust/devlaunch-core/src/flows/listing.rs` decides how
many `devpod status` round trips `dl --ls` has in flight at once. The performance
page states that width in prose, and states two figures derived from it, because
a reader deciding whether a slow `--ls` is worth reporting needs to know the shape
of the wait. That makes the page a second hand-maintained copy of one number,
which this repository allows only with a test beside it that diffs the copies.

Nothing else would catch it. `test_bench_doc.py` reads this page for the bench
harness rows and `test_docs_prose.py` reads it for em dashes; neither holds a
sentence to being true. Tuning the constant to sixteen is a one-character edit in
a file no doc guard reads, and the page would go on saying eight.

The instrument is the sentences the page already writes: "batches of eight", "a
pool of eight permits", "eight is a conservative pick". None is a marker added for
this test's benefit, so a rewrite that drops the claim fails here rather than
passing quietly.

`CHANGELOG.md` states the width too and is deliberately out of scope: it records
what was true when it was written, which is the same reason
`test_citations_resolve.py` exempts it.
"""

from __future__ import annotations

import math
import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

PAGE = REPO_ROOT / "docs" / "performance.md"
SOURCE = REPO_ROOT / "rust" / "devlaunch-core" / "src" / "flows" / "listing.rs"

WIDTH = re.compile(r"^const STATUS_TRIPS_AT_ONCE: usize = (\d+);", re.MULTILINE)

# The three ways the page states the width, none of them written for this test.
# Letters only, deliberately: the page also writes "Five batches of 0.45s", and a
# `\w+` here would read that duration as the width.
CLAIMS = re.compile(
    r"batches of ([A-Za-z]+)|pool of ([A-Za-z]+) permits"
    r"|and ([A-Za-z]+) is a conservative pick"
)

# "forty workspaces cost five batches", the one worked example on the page.
WORKED = re.compile(r"([A-Za-z]+) workspaces cost ([A-Za-z]+) batches")

NUMBERS = {
    "one": 1,
    "two": 2,
    "three": 3,
    "four": 4,
    "five": 5,
    "six": 6,
    "seven": 7,
    "eight": 8,
    "nine": 9,
    "ten": 10,
    "twelve": 12,
    "sixteen": 16,
    "twenty": 20,
    "thirty": 30,
    "forty": 40,
    "sixty": 60,
    "sixty-four": 64,
}


def _spelled(word: str) -> int:
    assert word.lower() in NUMBERS, (
        f"{PAGE.relative_to(REPO_ROOT)} spells a number this guard cannot read: "
        f"{word!r}. Add it to NUMBERS rather than rewording the page around the test"
    )
    return NUMBERS[word.lower()]


def _configured_width() -> int:
    found = WIDTH.search(SOURCE.read_text(encoding="utf-8"))
    assert found, (
        "STATUS_TRIPS_AT_ONCE is gone from "
        f"{SOURCE.relative_to(REPO_ROOT)}. If the listing no longer batches its "
        "status trips, retire this guard with the prose it diffs"
    )
    return int(found.group(1))


def _stated_widths() -> list[int]:
    text = PAGE.read_text(encoding="utf-8")
    return [_spelled(next(filter(None, match))) for match in CLAIMS.findall(text)]


def test_the_page_still_states_the_width():
    """A claim that vanished would make the diff below vacuously true."""
    assert _stated_widths(), (
        f"{PAGE.relative_to(REPO_ROOT)} no longer says how many status trips "
        "`dl --ls` has in flight at once. Either restore the claim or retire this "
        "guard with the copy it diffs"
    )


def test_the_page_agrees_with_the_configured_width():
    configured = _configured_width()
    stated = set(_stated_widths())

    assert stated == {configured}, (
        f"STATUS_TRIPS_AT_ONCE is {configured} and "
        f"{PAGE.relative_to(REPO_ROOT)} says {sorted(stated)}. The constant is the "
        "fact and the page is the copy, so change the page"
    )


def test_the_worked_example_divides_by_the_width():
    """The page's "forty workspaces cost five batches" is arithmetic, not a second claim."""
    configured = _configured_width()
    worked = WORKED.search(PAGE.read_text(encoding="utf-8"))
    assert worked, (
        f"{PAGE.relative_to(REPO_ROOT)} dropped the worked example that shows what "
        "the width buys. Restore it or retire this guard with it"
    )

    workspaces, batches = (_spelled(word) for word in worked.groups())
    expected = math.ceil(workspaces / configured)

    assert batches == expected, (
        f"the page says {workspaces} workspaces cost {batches} batches, but at a "
        f"width of {configured} they cost {expected}"
    )
