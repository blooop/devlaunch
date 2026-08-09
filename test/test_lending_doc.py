"""The lending contract README publishes, checked against the code that keeps it.

README's "Tools in every workspace" section tells an image author what to bake so
that a launch does no provisioning work at all. That is the rare piece of prose in
this repository somebody *acts* on -- they put it in a Dockerfile -- and the tree
can silently invalidate it: rename the constant that spells the official claude
layout, move the symlink the transfer writes, or stop baking the shim the section
warns about, and the instructions become confidently wrong with nothing failing.

So every claim guarded here is read out of the code rather than typed in again.
Asserting that a path *appears* in the README would pass just as happily against
a path nothing checks for; asserting that the path the module compares against
appears in the README is what makes the two move together. The rest of the
section -- the trip-by-trip narrative, the rationale, the two non-goals -- is
explanation, and gets no test, for the same reason most of AGENTS.md gets none.

The changelog claim is guarded the other way round: the unreleased entry may not
advertise a verification the code does not perform.
"""

import re
from pathlib import Path

import pytest

from devlaunch.tools import (
    CLAUDE_VERSIONS_RELPATH,
    REQUIRED_TOOLS,
    HostPayload,
    _is_official_claude,
    transfer_script,
)

REPO_ROOT = Path(__file__).resolve().parent.parent
README = REPO_ROOT / "README.md"
CHANGELOG = REPO_ROOT / "CHANGELOG.md"
CLAUDE_FEATURE_INSTALLER = Path(".devcontainer") / "claude-code" / "install.sh"

# Matched on the heading rather than on any phrase under it, so the prose is free
# to be rewritten while the assertions stay pointed at one section.
TOOLS_HEADING = "## Tools in every workspace"
UNRELEASED_HEADING = "## [Unreleased]"


def _section(document: Path, heading: str) -> str:
    """The text under `heading`, up to the next same-level heading.

    A renamed or deleted heading is the likeliest way these tests break, and it
    has to say so rather than surface as an empty string that fails every
    assertion below for an unrelated-looking reason.
    """
    text = document.read_text(encoding="utf-8")
    start = text.find(heading)
    assert start != -1, f"{document.name} has no heading {heading!r}; it was renamed or removed"
    rest = text[start + len(heading) :]
    end = rest.find("\n## ")
    return rest if end == -1 else rest[:end]


def _tools_section() -> str:
    return _section(README, TOOLS_HEADING)


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
@pytest.mark.parametrize("tool", REQUIRED_TOOLS, ids=lambda tool: tool.command)
def test_the_section_names_every_tool_a_workspace_is_promised(tool):
    """A tool this module provisions but the section never mentions is a broken promise."""
    assert f"`{tool.command}`" in _tools_section(), (
        f"README's tools section never mentions `{tool.command}`, which dl provisions"
    )


@pytest.mark.unit
def test_the_section_names_the_claude_layout_dl_actually_looks_for():
    """The one path an image author has to reproduce, taken from the module.

    Written as the module spells it, so moving the layout in code without
    moving it in the README fails here instead of shipping a Dockerfile recipe
    that builds something dl will refuse to recognise.
    """
    expected = f"`~/{CLAUDE_VERSIONS_RELPATH}`"
    assert expected in _tools_section(), (
        f"README's tools section no longer tells an image author to bake claude into {expected}"
    )


@pytest.mark.unit
def test_the_section_names_the_symlink_a_lend_creates():
    """The second half of the layout: the link, not just the versioned binary."""
    expected = f"`{_lent_claude_symlink()}`"
    assert expected in _tools_section(), (
        f"README's tools section no longer names {expected}, the symlink a lend writes"
    )


@pytest.mark.unit
def test_the_section_names_the_directory_a_lend_puts_in_front_of_the_path():
    """The PATH consequence is documented as intended behaviour, not left to be found.

    It is what makes a lent binary shadow a baked shim from then on, so a reader
    deciding what to bake needs it, and it is asserted as its own code span --
    the symlink's path contains this directory, so a section naming only the
    symlink must not satisfy this.
    """
    expected = f"`{_prepended_path_dir()}`"
    assert expected in _tools_section(), (
        f"README's tools section no longer says a lend prepends {expected} to the login PATH"
    )


@pytest.mark.unit
def test_the_section_warns_about_the_shim_this_repo_still_bakes():
    """The warning and the thing it warns about have to stay true together.

    The section tells the reader that an image built from this repo's own
    devcontainer feature does not meet the contract. That is only worth saying
    while the feature really does install a shim -- so both halves are checked
    here, and the day the feature installs the official layout instead, this
    fails and the paragraph goes rather than quietly misleading people.
    """
    installer = REPO_ROOT / CLAUDE_FEATURE_INSTALLER
    assert installer.is_file(), f"README's tools section points at {CLAUDE_FEATURE_INSTALLER}"
    assert "claude-shim" in installer.read_text(encoding="utf-8"), (
        f"{CLAUDE_FEATURE_INSTALLER} no longer installs a shim; README's warning about it is stale"
    )
    assert CLAUDE_FEATURE_INSTALLER.as_posix() in _tools_section(), (
        "README's tools section no longer warns that this repo's own feature bakes a shim"
    )


@pytest.mark.unit
def test_the_shim_the_section_warns_about_is_not_the_official_layout():
    """The section's premise, asked of the predicate that decides it for real.

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
    assert _is_official_claude(versions, f"{versions}/1.2.3"), (
        "the layout README tells image authors to bake is no longer recognised"
    )


@pytest.mark.unit
def test_the_unreleased_entry_claims_no_verification_the_code_does_not_perform():
    """The changelog said the lend was "checksum-verified". Nothing computes one.

    Stated as the implication rather than a flat ban on the word, because the
    entry becomes legal the moment the code earns it: what is forbidden is
    advertising a check to users that no shipped code performs. The gate the
    transfer really has is execution -- both binaries are run in a staging
    directory before anything is moved into place.
    """
    sources = "\n".join(
        path.read_text(encoding="utf-8") for path in sorted((REPO_ROOT / "devlaunch").rglob("*.py"))
    )
    if "checksum" in sources.lower():
        pytest.skip("the code computes a checksum now; the entry is free to say so")
    unreleased = _section(CHANGELOG, UNRELEASED_HEADING)
    assert "checksum" not in unreleased.lower(), (
        "the unreleased changelog claims a checksum, and no code in devlaunch/ computes one"
    )
