#!/usr/bin/env python3
"""A version section that already exists on the base branch must be untouched.

blooop/devlaunch#527. A branch cut before a release and rebased after it files
its new entry *inside the shipped section*, and git resolves that without a
conflict -- so `MERGEABLE` is green, every test passes, and a released version
quietly grows bullets describing fixes it did not contain.

The mechanism is that `## [Unreleased]` is a *stable heading that gets renamed*.
A release cut turns it into `## [0.25.0] - 2026-08-28` and inserts a fresh empty
`[Unreleased]` above. A branch whose entry sits under the old heading is anchored
by context to text that is now the release, and from git's point of view the
branch merely added lines below a heading that still exists. There is nothing for
a merge to conflict on, which is why nothing catches it.

It is not a slip that better attention fixes. Over one afternoon it happened four
times independently to three agents who did not know of each other, on branches
`wayfinder/devlaunch-{305,308,349,354}`, and twice in a single build on #346. Every
instance was caught by a human or an agent reading the diff. Nothing automated
caught any, because until this there was nothing to catch them.

**Why the rule is phrased about sections rather than about a region of the file.**
The obvious form -- "the released portion of the file is unchanged" -- is wrong on
its own terms, not merely inconvenient: a release cut *necessarily* rewrites that
region, so the first thing the guard would ever fail is the one commit that is
definitionally correct. A guard whose opening act is a false positive on the
project's own release ritual is switched off before it catches anything. Keyed by
version instead, the release cut passes by construction: it adds a heading that
was not there and modifies none that was.

Everything below the newest release is frozen, which is what a changelog is for.
Editing an old entry stays possible; it just has to be a visible, deliberate
override rather than a thing that happens to you during a rebase.

Usage:
    changelog_frozen.py <base-changelog> <head-changelog>

Exits 0 when every version present in both is byte-identical, 1 otherwise, and
1 (never 0) when either side cannot be read or parsed -- see `sections`.
"""

import difflib
import re
import sys
from pathlib import Path

# `## [Unreleased]`, `## [0.25.0] - 2026-08-28`. The bracketed version is the key;
# the rest of the line is part of the body to compare, so that editing a release's
# date is caught by the same rule that catches editing its bullets.
HEADING = re.compile(r"^## \[([^\]]+)\]")

# The one heading whose body is *expected* to differ on every branch: it is where
# a new entry is supposed to go.
MOVING = "Unreleased"


class Unparsable(Exception):
    """The file is not a changelog this guard can reason about.

    Raised rather than returned, and never caught into a pass. A guard that
    cannot read its input has not checked anything, and reporting that as
    success is the same class of defect as the one it exists to find.
    """


def sections(text: str, where: str) -> dict[str, str]:
    """Split a changelog into `{version: section text, heading included}`.

    Anything before the first heading -- the title and the Keep a Changelog
    preamble -- is not a version and is dropped. It is not compared, because it
    belongs to no release and a change to it is an ordinary edit.

    A version heading appearing twice raises. It would otherwise silently take
    whichever copy came last, and a comparison whose subject is ambiguous is a
    hole in exactly the place this guard is supposed to be solid.
    """
    found: dict[str, list[str]] = {}
    order: list[str] = []
    current: str | None = None
    for line in text.splitlines(keepends=True):
        match = HEADING.match(line)
        if match:
            current = match.group(1)
            if current in found:
                raise Unparsable(f"{where}: '## [{current}]' appears more than once")
            found[current] = []
            order.append(current)
        if current is not None:
            found[current].append(line)
    if not order:
        raise Unparsable(f"{where}: no '## [version]' headings found at all")
    return {version: "".join(lines) for version, lines in found.items()}


def read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as problem:
        raise Unparsable(f"{path}: cannot be read ({problem})") from problem


def frozen_sections_differ(base: dict[str, str], head: dict[str, str]) -> list[str]:
    """Report every version present in both whose text is not identical.

    Versions only on `head` are new -- that is a release cut, and the case the
    rule is shaped to permit. Versions only on `base` are a deletion, which this
    does not police: removing a section is loud in a diff and has never been the
    silent failure. What is silent is a section growing, and that is what is
    compared here.
    """
    complaints = []
    for version, base_text in base.items():
        if version == MOVING or version not in head:
            continue
        head_text = head[version]
        if head_text == base_text:
            continue
        diff = "".join(
            difflib.unified_diff(
                base_text.splitlines(keepends=True),
                head_text.splitlines(keepends=True),
                fromfile=f"base CHANGELOG.md  ## [{version}]",
                tofile=f"head CHANGELOG.md  ## [{version}]",
            )
        )
        complaints.append(
            f"'## [{version}]' is already released and this branch changes it.\n\n"
            f"{diff}\n"
            f"If this entry describes a change that is not in {version} -- which is what a\n"
            f"rebase across a release cut produces, cleanly and with nothing else red --\n"
            f"move it up into '## [Unreleased]'. If you really do mean to edit a shipped\n"
            f"section, say so in the pull request; this guard is meant to make that a\n"
            f"decision rather than an accident.\n"
        )
    return complaints


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(f"usage: {Path(argv[0]).name} <base-changelog> <head-changelog>", file=sys.stderr)
        return 2
    try:
        base = sections(read(Path(argv[1])), "base")
        head = sections(read(Path(argv[2])), "head")
    except Unparsable as problem:
        print(
            f"CHANGELOG.md could not be checked, so it is not passing: {problem}", file=sys.stderr
        )
        return 1
    complaints = frozen_sections_differ(base, head)
    for complaint in complaints:
        print(complaint, file=sys.stderr)
    if complaints:
        return 1
    print(f"every released section is untouched ({len(base) - 1} compared)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
