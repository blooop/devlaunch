"""The terminal title is written in four prose copies, held to one checked answer.

The title is a fact that has to appear more than once: the README's feature list
is where a reader first meets it, `docs/workspaces.md` tabulates it beside the
other two renderings of the id, `docs/workspace-tools.md` is where the reasoning
for the spelling lives, and `flows/launch.rs` is where it is computed and where
the doc comment explains the derivation to whoever changes it. That is four
hand-maintained copies of one fact, which this repository allows only with a test
beside them that diffs the copies.

The rule earned the test rather than being applied on principle. The README and
`docs/workspace-tools.md` had already drifted apart on this exact sentence, in
opposite directions, about the same example command, and nothing failed:
`test_readme_cli_doc.py` holds a flag to being mentioned, not a claim to being
true.

**None of those four can be the answer, `launch.rs` included.** Diffing prose
against prose only pins that it agrees, which the wrong set of edits satisfies as
easily as the right one: repair the drift by editing a page down to the wrong
string and a prose-only guard blesses it. `launch.rs` looks like it escapes that,
being the module that computes the title, but what this guard can read there is a
`///` comment -- prose that sits beside the code rather than prose the compiler
checks. Move the separator in `WorkspaceId::label`, fix the assertion that pins
it, and every prose copy is stale with `cargo test` green.

So the answer is read from that assertion instead. `TRUTH_CLAIM` matches the one
line in `a_label_is_the_id_with_the_suffix_off_and_an_at_where_the_dash_was` that
states the label for this example, which is the only copy that cannot be wrong
without a test failing: it is compared against `label()`'s real output every time
the suite runs. A separator change fails there first and reaches the prose here
second.

The instrument in each prose copy is the sentence or row it already writes about
this one example, so no page carries a marker for this test's benefit. Anchoring
on the example is also what keeps the guard honest in both directions: `docs/`
documents the cases that are titled *differently* (a path spec, a bare id), and
those name a different command, so documenting one more of them cannot fail this
test. It also cannot collide with "every prompt *renames* the pane", which
`docs/workspace-tools.md` and `flows/provision.rs` both say about the shell
prompt's own later write.

What this guard does not claim is to have found every copy. It pins one example's
answer in the four places that answer it; `docs/workspace-tools.md` also shows the
name inside an escape sequence and a `PS1` snippet, and those are not read here.
"""

from __future__ import annotations

import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# The answer, and the one copy of it a wrong string makes `cargo test` fail on.
TRUTH = Path("rust") / "devlaunch-core" / "src" / "domain" / "workspace_id.rs"
TRUTH_CLAIM = re.compile(
    r'id\(\s*"blooop",\s*"devlaunch",\s*"main",?\s*\)\s*\.label\(\)\s*,\s*"([^"]+)"'
)

# The claim as each page happens to write it, anchored on the example command so a
# differently-titled example cannot fail this test, and across whatever line break
# each happens to wrap at.
NAMES_THE_PANE = re.compile(
    r"`dl blooop/devlaunch` names the pane\s+(?://[/!]?\s*)?`([^`]+)`"
)
TERMINAL_TAB_ROW = re.compile(r"\[terminal tab\]\([^)]*\)\s*\|\s*`([^`]+)`")

PROSE = {
    Path("README.md"): NAMES_THE_PANE,
    Path("docs") / "workspaces.md": TERMINAL_TAB_ROW,
    Path("docs") / "workspace-tools.md": NAMES_THE_PANE,
    Path("rust") / "devlaunch-core" / "src" / "flows" / "launch.rs": NAMES_THE_PANE,
}


def _read(relative: Path) -> str:
    path = REPO_ROOT / relative
    assert path.is_file(), (
        f"{relative} is gone, so this guard cannot read the copy it diffs. Move the "
        "path with the file, or retire the entry along with the copy"
    )
    return path.read_text(encoding="utf-8")


def _computed() -> str:
    """The pane name `cargo test` pins for the example every prose copy writes."""
    stated = TRUTH_CLAIM.findall(_read(TRUTH))
    assert len(stated) == 1, (
        f"{TRUTH} states {len(stated)} pane names for `blooop/devlaunch@main` and "
        f"this guard reads it as the answer, so it has to state exactly one: "
        f"{stated}. The line is the `label()` assertion in "
        "`a_label_is_the_id_with_the_suffix_off_and_an_at_where_the_dash_was`"
    )
    return stated[0]


def test_every_prose_copy_still_states_the_pane_name():
    """A claim that vanished would make the comparison below vacuously true."""
    for relative, claim in PROSE.items():
        assert claim.findall(_read(relative)), (
            f"{relative} no longer says what `dl blooop/devlaunch` names the pane. "
            "Either restore the claim or retire this guard's entry along with the "
            "copy it diffs"
        )


def test_the_prose_agrees_with_the_assertion_that_checks_the_name():
    computed = _computed()

    for relative, claim in PROSE.items():
        stated = sorted(set(claim.findall(_read(relative))))
        assert stated == [computed], (
            f"{relative} says `dl blooop/devlaunch` names the pane {stated}, and "
            f"{TRUTH} pins '{computed}'. That assertion is checked against "
            "`label()` itself, so the prose is what moves unless `label()` changed"
        )
