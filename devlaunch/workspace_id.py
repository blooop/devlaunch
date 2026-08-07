"""Workspace identity: the one place a workspace id is derived.

A workspace is identified by the triple ``(owner, repo, ref)``. :class:`WorkspaceId`
is the parsed form of that triple, and its ``value`` names **both** the devpod
workspace and the clone-directory leaf. There is deliberately no function here that
takes an already-derived, id-shaped ``str``: the only way to obtain an id is to
construct a ``WorkspaceId``, and constructing one validates the triple. That is what
makes it impossible to reach a workspace path or a devpod id with an unvalidated ref.

The id format is::

    <repo-slug>-<ref-slug>-<suffix>

``suffix`` is six characters of hashed identity and is never truncated. ``repo-slug``
is the readable context and is cut to at most :data:`REPO_SLUG_LENGTH` characters,
never shorter. ``ref-slug`` absorbs all remaining truncation. Because only the slugs
are ever shortened and the suffix hashes the *unsanitized* triple, truncation cannot
produce a collision — two different triples always differ in the suffix.

See blooop/devlaunch#55 for the reasoning and #64 for the implementation ticket.
"""

import hashlib
import re
from dataclasses import dataclass

# Total id budget. devpod's own ceiling is 48, but ``setup_hostname`` sets the
# container hostname to the workspace id and downstream tooling stacks prefixes and
# suffixes on it against a 64-byte limit (kinisi-robotics/kinisi_ros#9766 already sat
# at 62/64). 48 is a ceiling others eat into, not a budget to fill, so aim at 38.
TARGET_LENGTH = 38

# The repo slug is cut to this length when the id would otherwise overflow, and is
# never cut below it. It carries no identity — only legibility — so trimming it is
# safe, but trimming it to nothing would make `devpod list` unreadable.
REPO_SLUG_LENGTH = 20

# 16 consonants x 4 vowels = 64 combinations = exactly 6 bits per syllable, so three
# syllables encode 18 bits of the digest in 6 pronounceable characters ("zovomo",
# "hesiro", "leneve") with no wordlist to keep in sync across languages.
_CONSONANTS = "bdfghjklmnprstvz"
_VOWELS = "aeio"
_SYLLABLES = 3

#: Length of the identity-bearing suffix, in characters.
SUFFIX_LENGTH = _SYLLABLES * 2

# A name safe to hand to git and to use as a path component: starts with a word
# character (so it can never be read as a flag) and holds only word characters,
# dots, slashes and dashes.
_SAFE_NAME_RE = re.compile(r"^[\w][\w./-]*$")

_NON_ALNUM_RUN_RE = re.compile(r"[^a-z0-9]+")


def validate_ref_name(name: str, kind: str = "ref") -> None:
    """Raise ``ValueError`` unless *name* is safe as a git ref and path component.

    Bad input gets exactly one response in this codebase: it is rejected. The
    previous split — ``_validate_ref`` raising while ``_sanitize_branch_dir``
    coerced — meant the same bad ref produced a hard error on one path and a
    silently different workspace on another. Coercion is also what made the old
    derivation non-injective, since five distinct refs coerced to one directory
    name. Rejecting is the honest answer: a ref that is not a ref cannot be
    checked out later anyway, so the only choice is where the user hears about it.
    """
    if not _SAFE_NAME_RE.match(name):
        raise ValueError(f"Invalid git {kind} name: {name!r}")


def slug(text: str) -> str:
    """Lowercase *text* and collapse every run of non-alphanumerics to one dash.

    This is the only slug rule in devlaunch. It applies to the repo part and the
    ref part alike, so ``my_repo`` becomes ``my-repo`` everywhere rather than
    ``my-repo`` in one derivation and ``myrepo`` in another.
    """
    return _NON_ALNUM_RUN_RE.sub("-", text.lower()).strip("-")


def _syllable_suffix(owner: str, repo: str, ref: str) -> str:
    """Six characters of hashed identity for the triple.

    The digest is taken over the **unsanitized** triple, NUL-delimited. That is what
    makes the derivation injective: ``feature/auth`` and ``feature-auth`` share a slug
    but hash differently, and the delimiter keeps ``(a, bc)`` distinct from ``(ab, c)``.

    This algorithm is frozen. Every workspace directory and devpod workspace on disk
    is named by its output, and the planned Rust port has to reproduce it byte for
    byte, so the tables, the syllable count, the digest, the byte slice and the
    delimiter are all pinned by test.
    """
    digest = hashlib.sha256(f"{owner}\0{repo}\0{ref}".encode()).digest()
    bits = int.from_bytes(digest[:8], "big")
    out = ""
    for _ in range(_SYLLABLES):
        out += _CONSONANTS[(bits >> 2) & 15] + _VOWELS[bits & 3]
        bits >>= 6
    return out


def _fit_ref(ref: str, room: int) -> str:
    """Slug *ref* down to *room* characters, dropping whole segments first.

    Refs are path-shaped, and their middle segments are usually taxonomy while the
    ends carry the meaning. Truncating characters first turns
    ``dependabot/github_actions/codecov/codecov-action-6`` into
    ``dependabot-github-actions-``, which says nothing about *which* action.
    Dropping middle segments instead keeps ``dependabot-codecov-action-6``.
    """
    segments = [s for s in (slug(part) for part in ref.split("/")) if s]
    while len(segments) > 2 and len("-".join(segments)) > room:
        del segments[1]
    return "-".join(segments)[:room].strip("-")


@dataclass(frozen=True)
class WorkspaceId:
    """A validated ``(owner, repo, ref)`` triple and the id derived from it.

    Constructing this type is the parse boundary. It validates all three parts, so
    holding a ``WorkspaceId`` is itself the evidence that the ref was checked — no
    caller has to remember to validate, and no code path can skip it. ``value`` is
    the derived id, used verbatim as the devpod workspace id and as the clone
    directory's leaf name.

    Raises:
        ValueError: if any part is unsafe as a git ref or path component.
    """

    owner: str
    repo: str
    ref: str

    def __post_init__(self) -> None:
        validate_ref_name(self.owner, "owner")
        validate_ref_name(self.repo, "repo")
        validate_ref_name(self.ref, "ref")

    @property
    def suffix(self) -> str:
        """The six-character identity suffix. Never truncated."""
        return _syllable_suffix(self.owner, self.repo, self.ref)

    @property
    def value(self) -> str:
        """The derived id, at most :data:`TARGET_LENGTH` characters."""
        suffix = self.suffix
        repo_part = slug(self.repo)
        # Cut the repo slug only when the id would otherwise overflow. Capping it at
        # REPO_SLUG_LENGTH leaves at least TARGET_LENGTH - 20 - 1 - 1 - 6 = 10
        # characters for the ref, so the ref budget can never go non-positive — the
        # hole that let a 47-char repo name skip truncation altogether.
        if len(self._join(repo_part, _fit_ref(self.ref, TARGET_LENGTH), suffix)) > TARGET_LENGTH:
            repo_part = repo_part[:REPO_SLUG_LENGTH].strip("-")
        separators = 2 if repo_part else 1
        room = TARGET_LENGTH - len(suffix) - len(repo_part) - separators
        return self._join(repo_part, _fit_ref(self.ref, max(room, 0)), suffix)

    @staticmethod
    def _join(*parts: str) -> str:
        return "-".join(part for part in parts if part)

    def __str__(self) -> str:
        return self.value
