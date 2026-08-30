"""The terminal title is stated on two pages, so the two statements are diffed.

The README orients and `docs/` explains, and the terminal title is one of the
facts that has to appear in both: the README's feature list is where a reader
first meets it, and `docs/workspace-tools.md` is where the reasoning for the
spelling lives. That makes it a second hand-maintained copy of one fact, which
this repository allows only with a test beside it that diffs the copies.

The rule earned its test rather than being applied on principle. The README said
`dl blooop/devlaunch` names the pane `devlaunch-main-3j1t`, "the workspace id",
where the docs page said `devlaunch@main` and contrasted it against exactly the
string the README asserted. `titled()` in
`rust/devlaunch-core/src/flows/launch.rs` answers the label whenever the devpod
id is the derived one, so the docs page was right and the README was describing
behaviour that same page already called what it "used to be". Nothing caught it:
`test_readme_cli_doc.py` holds a flag to being mentioned, not a claim to being
true, so the most read sentence about this feature was the least checked one.

The instrument is the sentence both pages already write. Each says
"names the pane `<name>`" of the same example command, so the name is
extractable without either page carrying a marker for this test's benefit, and
a page that stops making the claim fails here rather than passing quietly.
"""

from __future__ import annotations

import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# The pages that state it. Both, or the diff below has nothing to compare.
PAGES = (REPO_ROOT / "README.md", REPO_ROOT / "docs" / "workspace-tools.md")

# "names the pane `devlaunch@main`", across the line break either page may wrap at.
CLAIM = re.compile(r"names the pane\s+`([^`]+)`")


def _claims(page: Path) -> list[str]:
    return CLAIM.findall(page.read_text(encoding="utf-8"))


def test_both_pages_still_state_the_pane_name():
    """A claim that vanished would make the diff below vacuously true."""
    for page in PAGES:
        assert _claims(page), (
            f"{page.relative_to(REPO_ROOT)} no longer says what the pane is named. "
            "Either restore the claim or retire this guard with the copy it diffs"
        )


def test_the_pages_agree_on_what_the_pane_is_named():
    readme, reference = (set(_claims(page)) for page in PAGES)

    assert readme == reference, (
        f"README.md says the pane is named {sorted(readme)} and "
        f"docs/workspace-tools.md says {sorted(reference)}. One of them has "
        "drifted from what `titled()` actually writes; the docs page owns the "
        "reasoning, so check it against the code before editing either"
    )
