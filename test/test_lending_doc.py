"""The lending contract README publishes, checked against the code that keeps it.

README's "Tools in every workspace" section tells an image author what to bake so
that a launch does no provisioning work at all. That is the rare piece of prose in
this repository somebody *acts* on -- they put it in a Dockerfile -- and the tree
can silently invalidate it: rename the constant that spells the official claude
layout, move the symlink the transfer writes, or stop baking the shim the section
warns about, and the instructions become confidently wrong with nothing failing.

Two rules follow from that, and this file exists to obey both.

**Assert against the code, not against the prose.** A path that merely *appears*
in the README would pass just as happily against a path nothing checks for;
asserting the path the module compares against is what makes the two move
together. So the layout comes from the module constant, and the symlink and the
PATH directory are parsed out of a script the module generates.

**Assert inside the span whose meaning is at stake.** That is the harder half,
and the one an earlier version of this file got wrong: the layout assertion ran
against the whole section, and was satisfied by the narrative paragraph that
happens to mention the versions directory while the bake recipe one screenful
below -- the six lines an author actually copies -- could be rewritten to name a
path `dl` will never recognise with every test still green. Every assertion here
is therefore scoped to the subsection, and where it matters the individual
bullet, that carries the claim; and where the claim is a *warning*, its polarity
is asserted too, because a paragraph inverted to say the opposite keeps every
code span the old assertions looked for.

The rest of the section -- the trip-by-trip narrative, the rationale -- is
explanation, and is left alone for the same reason most of AGENTS.md is. The one
exception is the set of readings a probe can produce, because that set is not
prose: it is an enum, and the section has to keep covering all of it.

The changelog claim is guarded the other way round: the unreleased entry may not
advertise a verification of the payload that the lend does not perform.
"""

import re
from pathlib import Path, PurePosixPath

import pytest

from devlaunch.tools import (
    CLAUDE_VERSIONS_RELPATH,
    REQUIRED_TOOLS,
    HostPayload,
    ProbeResult,
    _is_official_claude,
    probe_script,
    transfer_script,
)

REPO_ROOT = Path(__file__).resolve().parent.parent
README = REPO_ROOT / "README.md"
CHANGELOG = REPO_ROOT / "CHANGELOG.md"
CLAUDE_FEATURE_INSTALLER = Path(".devcontainer") / "claude-code" / "install.sh"

# Matched on the heading rather than on any phrase under it, so the prose is free
# to be rewritten while the assertions stay pointed at one span. The recipe and
# the non-goals are named separately from their parent section precisely because
# a section-wide assertion is one that any paragraph in the section can satisfy.
NARRATIVE_HEADING = "### How they get there"
BAKE_HEADING = "### What to bake so a launch does no work at all"
NON_GOALS_HEADING = "### What this deliberately does not do"
UNRELEASED_HEADING = "## [Unreleased]"


def _section(document: Path, heading: str) -> str:
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


def _bake_recipe() -> str:
    """The subsection an image author copies into a Dockerfile, and nothing else."""
    return _section(README, BAKE_HEADING)


def _bullets(text: str) -> list:
    """The top-level `- ` bullets of `text`, each with its continuation lines.

    Assertions land on a single bullet rather than on the subsection because a
    subsection-wide assertion is satisfied by any sentence in it -- including a
    parenthetical listing the guarded spans next to a recipe that says something
    else entirely.
    """
    bullets: list = []
    in_bullet = False
    for line in text.splitlines():
        if line.startswith("- "):
            bullets.append(line)
            in_bullet = True
        elif in_bullet and line.startswith("  "):
            bullets[-1] += "\n" + line
        elif line.strip():
            in_bullet = False
    return bullets


def _bullet_naming(text: str, term: str) -> str:
    """The one bullet of `text` whose bolded lead is `term`."""
    lead = f"- **{term}**"
    matching = [bullet for bullet in _bullets(text) if bullet.startswith(lead)]
    assert len(matching) == 1, (
        f"expected exactly one bullet starting {lead!r}, found {len(matching)}"
    )
    return matching[0]


def _a_transfer_script() -> str:
    """The script a real lend runs, generated the way `_transfer` generates it.

    The version is arbitrary because nothing asserted here depends on it; what
    matters is that the paths below come out of the module's own string
    assembly, not out of this file.
    """
    return transfer_script(HostPayload(claude_version="1.2.3", members=()))


def _lent_claude_symlink() -> str:
    """Where a lend puts the `claude` a login shell will find, as `~/...`."""
    match = re.search(r'ln -sfn "\$HOME/[^"]+" "\$HOME/(?P<link>[^"]+)"', _a_transfer_script())
    assert match, "the transfer script no longer creates a claude symlink under $HOME"
    return f"~/{match.group('link')}"


def _prepended_path_dir() -> str:
    """The directory a lend puts in front of the login PATH, as `~/...`."""
    match = re.search(r'export PATH="\$HOME/(?P<dir>[^:"]+):\$PATH"', _a_transfer_script())
    assert match, "the transfer script no longer prepends a directory under $HOME to PATH"
    return f"~/{match.group('dir')}"


@pytest.mark.unit
@pytest.mark.parametrize("state", list(ProbeResult), ids=lambda state: state.value)
def test_the_narrative_explains_every_reading_a_probe_can_produce(state):
    """The three outcomes a launch branches on, named where they are explained.

    The narrative is otherwise explanation and gets no assertions -- but the set
    of readings is not prose, it is `ProbeResult`, and a fourth state added to
    that enum without a paragraph saying what it costs a reader leaves the
    section describing a flow that no longer exists.
    """
    assert f"**{state.value}**" in _section(README, NARRATIVE_HEADING), (
        f"README no longer explains the {state.value!r} reading a probe can produce"
    )


@pytest.mark.unit
@pytest.mark.parametrize("tool", REQUIRED_TOOLS, ids=lambda tool: tool.command)
def test_the_recipe_gives_every_tool_a_workspace_is_promised_its_own_instruction(tool):
    """A tool this module provisions but the recipe never tells you to bake.

    One bullet per tool, not one mention per section: a recipe that names both
    tools in passing and then says "ask your platform team" has told an image
    author nothing, and that is a shape a token count cannot tell from a
    contract. Each bullet has to name a place a *login shell* will find the tool
    -- the login PATH, or a path under `~` reached from it -- because that is
    the only question the probe asks, and a bullet saying `dl` will find it
    anywhere on the filesystem is an instruction that costs the reader the lend.
    """
    assert f"command -v {tool.command}" in probe_script(), (
        f"the probe no longer resolves {tool.command} by name, so README's recipe may be "
        "describing the wrong precondition"
    )
    bullet = _bullet_naming(_bake_recipe(), f"`{tool.command}`")
    assert "login PATH" in bullet or "~/" in bullet, (
        f"README's bake recipe names `{tool.command}` without saying where a login shell "
        "will find it"
    )


@pytest.mark.unit
def test_the_recipe_names_the_claude_layout_dl_actually_looks_for():
    """The one path an image author reproduces, taken from the module.

    Asserted inside the `claude` bullet of the recipe, in the versioned form an
    author copies, and with its closing backtick: the section's narrative names
    the versions directory too, so a section-wide assertion is satisfied while
    the recipe says something else -- and a longer path that merely starts with
    the right one (`.../<version>/bin/claude`, which the host refuses) contains
    the unclosed span but not the closed one.
    """
    expected = f"`~/{CLAUDE_VERSIONS_RELPATH}/<version>`"
    assert expected in _bullet_naming(_bake_recipe(), "`claude`"), (
        f"README's bake recipe no longer tells an image author to bake claude into {expected}"
    )


@pytest.mark.unit
def test_the_recipe_says_the_binary_is_a_direct_child_and_the_host_agrees():
    """Both halves of "anything deeper does not count", stated and enforced.

    `_is_official_claude` is a parent-equality test, so a binary one directory
    deeper is refused -- exactly the `versions/latest/bin/claude` shape a
    downloader parks there. An author who is not told that builds an image that
    reads *lendable* forever, so the recipe has to say it and the predicate has
    to keep meaning it.
    """
    home = "/containers/workspace-home"
    versions = f"{home}/{CLAUDE_VERSIONS_RELPATH}"
    assert _is_official_claude(versions, f"{versions}/1.2.3"), (
        "the layout README tells image authors to bake is no longer recognised"
    )
    assert not _is_official_claude(versions, f"{versions}/1.2.3/bin/claude"), (
        "a binary nested under the versions directory now counts; the recipe says it does not"
    )
    assert "direct child" in _bullet_naming(_bake_recipe(), "`claude`"), (
        "README's bake recipe no longer says the binary must be a direct child of the "
        "versions directory, which is what dl requires"
    )


@pytest.mark.unit
def test_the_recipe_names_the_symlink_a_lend_creates():
    """The second half of the layout: the link, not just the versioned binary."""
    expected = f"`{_lent_claude_symlink()}`"
    assert expected in _bullet_naming(_bake_recipe(), "`claude`"), (
        f"README's bake recipe no longer names {expected}, the symlink a lend writes"
    )


@pytest.mark.unit
def test_the_recipe_states_the_login_path_precondition_the_symlink_depends_on():
    """The precondition that decides whether a perfectly baked image is seen at all.

    The probe resolves the tools by name under a login shell, so the directory
    the `claude` symlink lives in has to be on that PATH; an image that
    satisfies every other bullet and leaves it off is read as *absent* and pays
    the whole lend. Taken from the symlink the transfer writes, because that is
    the relation being asserted -- this directory matters *because* it is where
    the link is -- and given its own bullet because the requirement is not the
    symlink's: the symlink's path contains this directory, so the bullet naming
    the symlink would satisfy any assertion that only looked for the span.
    """
    symlink_dir = PurePosixPath(_lent_claude_symlink()).parent
    assert _bullet_naming(_bake_recipe(), f"`{symlink_dir}` on the login PATH"), (
        f"README's bake recipe does not require `{symlink_dir}`, where the claude symlink "
        "lives, on the login PATH"
    )


@pytest.mark.unit
def test_the_recipe_says_a_lend_puts_that_directory_in_front_of_the_login_path():
    """The PATH consequence documented as intended behaviour, not left to be found.

    It is what makes a lent binary shadow a baked shim from then on, and it is a
    separate claim from the precondition above -- the same directory, said about
    the lend rather than about the image -- so it is asserted against the
    sentence that makes it rather than against the span, which by now appears in
    two other places in the recipe.
    """
    prepended = _prepended_path_dir()
    # `\s+` rather than a space: the recipe is hard-wrapped, so the span and the
    # verb are one reflow away from being on separate lines, and that is not a
    # change of meaning.
    assert re.search(rf"lend prepends\s+`{re.escape(prepended)}`", _bake_recipe()), (
        f"README's bake recipe no longer says a lend prepends `{prepended}` to the login PATH"
    )


@pytest.mark.unit
def test_the_recipe_warns_that_this_repo_s_own_feature_does_not_satisfy_it():
    """The warning, its polarity, and the thing it warns about, all three.

    The recipe tells the reader that an image built from this repo's own
    devcontainer feature does *not* meet the contract. Naming the installer is
    not enough -- a paragraph rewritten to say the feature already satisfies the
    contract names it just as happily, and that inversion is the one failure
    mode a reader is actually harmed by. So the denial itself is asserted, and
    so is its premise: the day the feature installs the official layout instead,
    this fails and the paragraph goes rather than quietly misleading people.
    """
    installer = REPO_ROOT / CLAUDE_FEATURE_INSTALLER
    assert installer.is_file(), f"README's bake recipe points at {CLAUDE_FEATURE_INSTALLER}"
    assert "claude-shim" in installer.read_text(encoding="utf-8"), (
        f"{CLAUDE_FEATURE_INSTALLER} no longer installs a shim; README's warning about it is stale"
    )
    recipe = _bake_recipe()
    assert CLAUDE_FEATURE_INSTALLER.as_posix() in recipe, (
        "README's bake recipe no longer warns that this repo's own feature bakes a shim"
    )
    assert re.search(r"\*\*[^*]*\bbakes a shim\b[^*]*\*\*", recipe), (
        "README's bake recipe no longer states in bold that this repo's own feature bakes a shim"
    )
    assert re.search(r"does \*not\* meet the contract", recipe), (
        "README's bake recipe no longer denies that an image built from this repo's own "
        "feature meets the contract"
    )


@pytest.mark.unit
def test_the_shim_the_recipe_warns_about_is_not_the_official_layout():
    """The recipe's premise, asked of the predicate that decides it for real.

    "A shim does not count" is the whole reason the contract is worth writing
    down, and it is a claim about `_is_official_claude` -- the one relation both
    ends of the pipe are read through. A home that is not under `/home` keeps
    this free of any machine's real layout.
    """
    home = "/containers/workspace-home"
    versions = f"{home}/{CLAUDE_VERSIONS_RELPATH}"
    shim = f"{home}/.pixi/envs/claude-shim/bin/claude"
    assert not _is_official_claude(versions, shim), (
        "a pixi shim now counts as the official layout; README's contract says it does not"
    )


# What #141 settled that the section has to keep saying, because a reader plans
# around both: nothing here is a claim the code can invalidate, which is exactly
# why they had no guard at all and could be deleted wholesale in silence.
DOCUMENTED_NON_GOALS = ("No per-tool transfer.", "No version sync.")


@pytest.mark.unit
@pytest.mark.parametrize("non_goal", DOCUMENTED_NON_GOALS)
def test_the_section_keeps_stating_the_non_goals_it_committed_to(non_goal):
    """Each accepted non-goal survives as its own bullet, in the bold a reader skims.

    An image author sizing a build plans around these: that a half-provisioned
    image is sent both tools, and that a `claude` already there is never
    upgraded. Deleting them costs nothing anywhere else in the tree, which is
    the only reason this test exists.
    """
    assert _bullet_naming(_section(README, NON_GOALS_HEADING), non_goal), (
        f"README's non-goals no longer state {non_goal!r}"
    )


# How the unreleased entry about the lend introduces itself. Scoped to that one
# entry rather than to the whole `[Unreleased]` section, because the claim being
# guarded is this entry's: a future entry about something else is entitled to
# talk about signatures and checksums, and a guard that failed on it would be a
# guard people delete.
LEND_ENTRY_LEAD = "A cold container is lent the host's own"

# The vocabulary of a claim about the payload's *bytes* -- the class of check a
# reader hears as "corruption or tampering would have been caught". The lend
# performs none of it: its only gate is execution, and the entry said
# "checksum-verified" for months. Banning the one word it happened to use would
# leave every equivalent claim ("GPG-signature-verified", "hash-checked")
# available, so the class is what is named here. The day a lend really does
# verify bytes, this tuple is edited in the same change -- deliberately, by
# somebody re-reading what the entry now promises.
UNPERFORMED_INTEGRITY_CLAIMS = (
    "checksum",
    "sha256",
    "sha-256",
    "md5",
    "blake2",
    "signature",
    "signed",
    "gpg",
    "cryptograph",
    "attest",
)


def _unreleased_lend_entry() -> str:
    """The unreleased changelog entry that describes the lend, and only that one."""
    matching = [
        entry
        for entry in _bullets(_section(CHANGELOG, UNRELEASED_HEADING))
        if LEND_ENTRY_LEAD in entry
    ]
    assert len(matching) == 1, (
        f"expected exactly one unreleased entry containing {LEND_ENTRY_LEAD!r}, "
        f"found {len(matching)} -- it was reworded, split or released"
    )
    return matching[0]


def _transfer_gates_on_execution() -> bool:
    """Whether the generated transfer really runs the binaries before moving any."""
    lines = _a_transfer_script().splitlines()
    version_checks = [i for i, line in enumerate(lines) if "--version" in line]
    moves = [i for i, line in enumerate(lines) if line.startswith("mv -f ")]
    return bool(version_checks) and bool(moves) and max(version_checks) < min(moves)


@pytest.mark.unit
def test_the_unreleased_entry_describes_the_gate_the_transfer_actually_has():
    """The positive half: what the entry says happened is what the script does.

    The entry earns the word "proved" only while the generated script really
    runs both lent binaries and moves nothing until they have answered. Read out
    of the script rather than trusted, so reordering the gate in code turns this
    red instead of leaving the changelog describing a version of the transfer
    that no longer exists.
    """
    assert _transfer_gates_on_execution(), (
        "the transfer no longer runs the lent binaries before moving them into place; "
        "the unreleased entry says it does"
    )
    assert "proved to run in a staging directory" in _unreleased_lend_entry(), (
        "the unreleased lend entry no longer describes the staging gate, which is the only "
        "verification the lend performs"
    )


@pytest.mark.unit
@pytest.mark.parametrize("claim", UNPERFORMED_INTEGRITY_CLAIMS)
def test_the_unreleased_entry_advertises_no_integrity_check_the_lend_does_not_perform(claim):
    """The negative half: no check on the bytes may be advertised, in any wording.

    Stated as a failing assertion and never as a skip. An earlier version of
    this test stood down whenever the word appeared anywhere under `devlaunch/`
    -- including in a comment saying no checksum is computed -- and a disarmed
    guard is how a broken run looks clean; `test/fixtures/e2e_guard.py` writes
    the repo's rule down. There is nothing here this test needs and cannot get.
    """
    assert not re.search(rf"\b{re.escape(claim)}", _unreleased_lend_entry(), re.IGNORECASE), (
        f"the unreleased lend entry advertises {claim!r}; the lend verifies nothing about the "
        "bytes it sends -- its only gate is running both binaries in a staging directory"
    )
