#!/usr/bin/env python3
"""A branch may not propose a version that has already been published.

blooop/devlaunch#591. Two branches bumped to 0.36.0 independently: #593 cut it and
published, and #591 -- open across that release -- carried a `release: 0.36.0`
commit of its own. Git merged the collision *silently*, because picking the same
number means both sides made the identical edit: one `version = "0.36.0"` line in
`rust/Cargo.toml` with nothing to resolve, and one byte-identical
`## [0.36.0]` heading in CHANGELOG.md with the differing bodies merged in
underneath as separate additions. Choosing 0.37.0 instead would have conflicted
loudly in `Cargo.toml` and been caught at the merge.

The cost is that the second merge publishes **nothing**. `publish.yml` sees the
version already tagged and reports `nothing to publish`, which is also what it
says for every ordinary push, so the fix sat merged and unreleased with no check
red and no line in any log to act on.

**Why this runs at pull-request time and not at publish time.** By the time the
merge reaches `main` the collision is no longer visible: the merge commit's
version equals its first parent's -- both are 0.36.0 -- so "did this push change
the version" is false and there is nothing for `publish.yml` to notice. The
branch is the last place the two numbers still differ.

**Why comparing against the base commit is enough.** `pull_request.base.sha` is
the base as of the pull request, not the moving tip of `main`: for #591 it was
`908a24e`, the commit before #593 landed, where the version was still 0.35.0. So
the comparison sees 0.35.0 -> 0.36.0, a proposed bump, and asks the one question
worth asking about it -- is `v0.36.0` already out there?

Usage:
    version_untaken.py <base-cargo.toml> <head-cargo.toml>

Exits 0 when the branch proposes no new version, or proposes one that has never
been tagged. Exits 1 when it proposes a version that is already published, and 1
(never 0) when either side cannot be read -- a guard that cannot read its input
has checked nothing.
"""

import re
import subprocess
import sys
from pathlib import Path

# `[workspace.package] version` is the first `version = "..."` line in the file;
# the crates inherit it with `version.workspace = true`. `publish.yml` and
# `conda-publish.yml` both read it exactly this way -- keep the three in step.
VERSION = re.compile(r'^version = "(.*)"', re.MULTILINE)


class Unreadable(Exception):
    """The manifest is not one this guard can take a version out of."""


def version_in(path: Path, where: str) -> str:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as problem:
        raise Unreadable(f"{where}: cannot be read ({problem})") from problem
    found = VERSION.search(text)
    if not found:
        raise Unreadable(f"{where}: no 'version = \"...\"' line")
    return found.group(1)


def is_tagged(version: str, run=subprocess.run) -> bool | None:
    """Whether `v<version>` already names a commit.

    `None` is "could not tell" -- no git, or a git that failed for a reason of its
    own -- and the caller refuses on it. A missing tag is a confident `False` and
    the ordinary answer for a release being cut.
    """
    try:
        done = run(
            ["git", "rev-parse", "-q", "--verify", f"refs/tags/v{version}^{{commit}}"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if done.returncode == 0:
        return True
    # `rev-parse -q --verify` exits 1 for "no such ref" and reserves other codes
    # for being unable to answer, which is not the same thing and must not read as
    # "free to publish".
    return False if done.returncode == 1 else None


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(f"usage: {Path(argv[0]).name} <base-cargo.toml> <head-cargo.toml>", file=sys.stderr)
        return 2
    try:
        base = version_in(Path(argv[1]), "base rust/Cargo.toml")
        head = version_in(Path(argv[2]), "head rust/Cargo.toml")
    except Unreadable as problem:
        print(f"the version could not be checked, so it is not passing: {problem}", file=sys.stderr)
        return 1

    if base == head:
        print(f"this branch proposes no new version (still {head})")
        return 0

    taken = is_tagged(head)
    if taken is None:
        print(
            f"could not ask git whether v{head} is already tagged, so this is not passing",
            file=sys.stderr,
        )
        return 1
    if taken:
        print(
            f"this branch bumps {base} -> {head}, and v{head} is already tagged.\n\n"
            f"Someone else cut {head} while this branch was open, so merging it would\n"
            f"publish nothing: `publish.yml` finds the tag, reports 'nothing to publish',\n"
            f"and the work lands on main released nowhere. Git will not catch it for you --\n"
            f"both sides wrote the same version line, so there is nothing to conflict on.\n\n"
            f"Bump to the next unused version instead, and check that the CHANGELOG entry\n"
            f"is under that heading rather than under {head}'s.\n",
            file=sys.stderr,
        )
        return 1
    print(f"this branch bumps {base} -> {head}, which has never been tagged")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
