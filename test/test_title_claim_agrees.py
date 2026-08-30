"""The terminal title is stated in three places, so the three statements are diffed.

The title is a fact that has to appear more than once: the README's feature list
is where a reader first meets it, `docs/workspace-tools.md` is where the
reasoning for the spelling lives, and `flows/launch.rs` is where it is computed
and where the doc comment explains the derivation to whoever changes it. That
makes two hand-maintained copies of what the third one does, which this
repository allows only with a test beside them that diffs the copies.

The rule earned the test rather than being applied on principle. The README and
the docs page had already drifted apart on this exact sentence, in opposite
directions, about the same example command, and nothing failed:
`test_readme_cli_doc.py` holds a flag to being mentioned, not a claim to being
true.

**`launch.rs` is in the list, and it is the reason this guard means anything.**
Diffing the two prose pages against each other would only pin that they agree,
which the wrong pair of edits satisfies as easily as the right one: had the
drift been repaired by editing the docs page down to the README's wrong string,
a two-page guard would have blessed it. The module that computes the title is
the one copy that cannot be wrong without the behaviour being wrong, so it is
the one the prose is held against.

The instrument is the sentence all three already write about one example
command, so no page carries a marker for this test's benefit. Anchoring on the
command is also what keeps the guard honest in both directions: `docs/` documents
the cases that are titled *differently* (a path spec, a bare id), and those
sentences name a different command, so documenting one more of them cannot fail
this test. It also cannot collide with "every prompt *renames* the pane", which
`docs/workspace-tools.md` and `flows/provision.rs` both say about the shell
prompt's own later write.
"""

from __future__ import annotations

import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# The three places the fact is written. `launch.rs` is not optional: see the
# module docstring for why a prose-only diff would pass the inverted repair.
SOURCES = (
    Path("README.md"),
    Path("docs") / "workspace-tools.md",
    Path("rust") / "devlaunch-core" / "src" / "flows" / "launch.rs",
)

# The claim, anchored on the one example command all three use, across whatever
# line break each happens to wrap at. Anchored rather than matched loosely so
# that a page documenting a differently-titled example cannot fail this test.
CLAIM = re.compile(r"`dl blooop/devlaunch` names the pane\s+(?://[/!]?\s*)?`([^`]+)`")


def _claims(relative: Path) -> list[str]:
    return CLAIM.findall((REPO_ROOT / relative).read_text(encoding="utf-8"))


def test_every_source_still_states_the_pane_name():
    """A claim that vanished would make the comparison below vacuously true."""
    for relative in SOURCES:
        assert _claims(relative), (
            f"{relative} no longer says what `dl blooop/devlaunch` names the pane. "
            "Either restore the claim or retire this guard along with the copy it "
            "diffs"
        )


def test_the_prose_agrees_with_the_module_that_computes_the_title():
    truth = Path("rust") / "devlaunch-core" / "src" / "flows" / "launch.rs"
    computed = set(_claims(truth))

    assert len(computed) == 1, (
        f"{truth} states more than one pane name for the same command: "
        f"{sorted(computed)}. This guard reads it as the answer, so it has to be "
        "one answer"
    )

    for relative in SOURCES:
        assert set(_claims(relative)) == computed, (
            f"{relative} says `dl blooop/devlaunch` names the pane "
            f"{sorted(set(_claims(relative)))}, and {truth} says "
            f"{sorted(computed)}. The module is where the title is computed, so "
            "the prose is what moves unless `titled()` itself changed"
        )
