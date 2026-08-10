"""What a workspace holds — the facts a cleanup decision is made from elsewhere.

A workspace per branch means workspaces accumulate, and something has to remove
the finished ones. That something is **not devlaunch**: whether a piece of work
is finished is a fact about a ticket, a review or a person's intent, and dl
knows about none of those. It knows about clones and containers.

So the split is mechanism here, policy in the caller:

- ``dl --ls --json`` reports what exists and what each workspace holds, which is
  what a caller needs to decide anything at all.
- ``dl <ws> rm`` deletes one, and refuses when the clone holds work that exists
  nowhere else.

The refusal is the one judgement dl does make, and it is not a policy about
finished work: it is dl declining to destroy the only copy of something. A
caller that means it says ``--force``.

The alternative — dl inferring "finished" from the branch (merged into the
default, or deleted from the remote) — was built first and thrown away. It reads
as a git fact but it is a guess at intent: a squash-merged branch and an
abandoned one are indistinguishable, a branch merged upstream may still have
work to do, and a repo whose flow does not delete branches gets nothing. The
caller that knows the answer should say the answer.

**Three answers, not two (devlaunch#171).** This module used to answer
``Optional[str]``: a description of what would be lost, or ``None``. ``None``
carried two meanings — "nothing would be lost" and "I could not find out" — and
the destructive caller read both as permission. That is not a hypothetical
conflation; it shipped, and it destroyed the wrong thing:

``_git`` ran ``git … cwd=clone`` with no ``--git-dir``, no ``--work-tree`` and
no ceiling, so git's repository **discovery walked up the parent chain**. A
clone whose ``.git`` was unusable — truncated, half-removed by an interrupted
delete, or never finished — did not make git refuse. It made git find an
*ancestor* repository and answer confidently about that one. With ``dl``'s cache
under ``$XDG_CACHE_HOME`` and a dotfiles repository in ``$HOME``, that ancestor
is common; when it was clean and fully pushed, ``holds_unsaved_work`` returned
``None`` for a clone holding untracked work and ``dl <ws> rm`` deleted it
without so much as asking for ``--force``. The failure needed a *tidy* host to
appear, because a dirty ancestor made the guard fire — for the wrong reason,
about the wrong repository — and hid it.

Both halves are fixed here and neither is sufficient alone:

1. Every git command names its repository explicitly (:func:`_git`), so
   discovery is switched off and an unusable ``.git`` produces a refusal.
2. A refusal has somewhere to go: :data:`Unsaved` is a total sum —
   :class:`NothingToLose`, :class:`WouldLose`, :class:`CouldNotTell` — and
   every caller must name the arm it is handling. "Could not tell" refuses a
   delete exactly as "would lose" does.
"""

import logging
import os
import stat
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, NoReturn, Optional, Union

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class NothingToLose:
    """Everything in the clone exists somewhere else. Deleting it costs nothing.

    Carries no payload on purpose: there is nothing to say. It is a distinct
    type rather than a ``None`` so that a caller cannot reach it by accident —
    the whole of devlaunch#171 was a caller reaching it by accident.
    """


@dataclass(frozen=True)
class WouldLose:
    """Deleting the clone would destroy *description*.

    A description rather than a flag, and that is load-bearing: it is printed to
    a person deciding whether to force the delete, and "3 uncommitted change(s)
    (pixi.lock, notes.md, …) and 2 unpushed commit(s)" is the thing that answers
    them. See :func:`_name_a_few` for why the count alone is not enough.
    """

    description: str

    def __post_init__(self) -> None:
        # An empty description would print as "workspace holds ." and read as a
        # bug in dl rather than as a reason to stop. If there is nothing to
        # name, the answer is NothingToLose, not a WouldLose with no words.
        if not self.description:
            raise ValueError("WouldLose needs something to say; use NothingToLose() instead")


@dataclass(frozen=True)
class CouldNotTell:
    """git could not be asked about this clone, and *reason* is what it said.

    Not a failure to report — a report. It is the answer for a directory that is
    there but is not a repository git can read, and it must stop a delete for
    the same reason :class:`WouldLose` does: the files are still on disk, and
    nothing has established that they exist anywhere else.

    The reason is carried rather than reconstructed at the point of printing,
    because the cause is not guessable from the path: an interrupted delete, a
    ``.git`` written by a container as another user, a truncated gitfile and a
    directory that was never a clone all arrive here and read differently to
    the person who has to decide what to do about it. It names the directory it
    is about, which is the specific thing the shipped bug got wrong.
    """

    reason: str


# Three arms and no fourth. "The directory is not there" is not a fourth: it is
# NothingToLose, because there is no work in it to lose -- the same answer
# disk_usage gives such a directory (Measured(0)), and what lets a caller clear
# away a workspace whose clone was already removed by hand.
Unsaved = Union[NothingToLose, WouldLose, CouldNotTell]


def unhandled_unsaved(unsaved: NoReturn) -> NoReturn:
    """Reject an unsaved arm nobody handled -- at type-check time, not at runtime.

    Exported because the arms are read outside this module (``dl.py``'s delete
    guard and its JSON listing), and an ``else`` hand-rolled at a call site is
    exactly how a fourth arm would be silently read as "safe to delete".

    Hand-rolled rather than :func:`typing.assert_never`, which is 3.11+ while
    this package supports 3.10; a parameter typed ``NoReturn`` gets the same
    treatment from the checker. The same shape as
    :func:`devlaunch.disk_usage._unhandled_usage`, deliberately.
    """
    raise AssertionError(f"Unhandled unsaved-work answer: {unsaved!r}")


def unsaved_as_json(unsaved: Unsaved) -> Dict[str, Any]:
    """How an answer reads to a tool: one key, and the key says which kind it is.

    Deliberately not a nullable string. A caller that reads ``nothingToLose``
    has been told nothing would be lost; a caller that reads ``couldNotTell``
    has been told dl does not know, and cannot have got there by finding a
    field absent or null. The shape :func:`devlaunch.disk_usage.usage_as_json`
    already uses, for the same reason.

    This is a deliberate break in ``dl --ls --json``: the field used to be a
    string or ``null``. It breaks in the safe direction — a reader that tested
    the old field for truthiness now sees an object, which is truthy for every
    arm, so it leaves workspaces alone rather than deleting them. ``null``
    survives one level up, in ``dl.py``, where it keeps its other meaning:
    there is no clone of dl's own there to inspect.
    """
    if isinstance(unsaved, NothingToLose):
        return {"nothingToLose": True}
    if isinstance(unsaved, WouldLose):
        return {"wouldLose": unsaved.description}
    if isinstance(unsaved, CouldNotTell):
        return {"couldNotTell": unsaved.reason}
    unhandled_unsaved(unsaved)


@dataclass(frozen=True)
class CloneState:
    """What one workspace clone holds, as far as git can tell.

    ``branch`` is what the clone has checked out, or ``None`` when git could not
    say — an unusable ``.git``, or an unborn HEAD.

    The two fields are independent, and an earlier draft of this docstring said
    otherwise ("``None`` in every case where ``unsaved`` is a
    :class:`CouldNotTell`"). A clone git *can* read as a repository but whose
    remote-tracking refs are broken gives ``CloneState(branch='feature',
    unsaved=CouldNotTell(...))``: ``git status`` answered, so the branch is
    known, and only the later ``git log … --not --remotes`` refused. That is the
    shape ``test_a_readable_repo_whose_remote_refs_are_broken_is_could_not_tell``
    builds, and the behaviour is right — it was the invariant that was wrong.

    ``branch`` is reported beside the recorded branch rather than
    instead of it (``dl --ls --json`` prints both), so a clone an agent moved
    off its branch is visible as such.
    """

    branch: Optional[str]
    unsaved: Unsaved


@dataclass(frozen=True)
class GitSaid:
    """git ran and exited 0. ``output`` may be empty; empty is an answer."""

    output: str


@dataclass(frozen=True)
class GitRefused:
    """git could not answer, and ``reason`` is what it said about that."""

    reason: str


# The distinction the old `Optional[str]` could not make safely. `""` and
# `None` are both falsey, so `if status:` read a *refused* `git status` as a
# clean tree -- the same sentinel bug as the one above, one layer down, and it
# would have survived the discovery fix on its own.
GitAnswer = Union[GitSaid, GitRefused]


def _git(repo: Path, *args: str) -> GitAnswer:
    """Ask git about *repo* — and only about *repo*.

    ``--git-dir`` and ``--work-tree`` are the fix for devlaunch#171 and are not
    optional decoration. Passing ``cwd=`` alone leaves git's repository
    discovery switched on, and discovery walks up the parent chain: on a clone
    whose ``.git`` is unusable git does not refuse, it finds an **ancestor**
    repository and answers about that. Naming the git directory switches
    discovery off entirely, so the only repository git can reach is this one and
    an unusable ``.git`` becomes a refusal — which is what the caller needs.

    Verified against real git (2.55.0) rather than assumed, on each shape a
    broken clone actually takes: a ``.git`` directory holding garbage, an empty
    ``.git`` directory, a ``.git`` with HEAD and nothing else, a real clone with
    its object store deleted, and a truncated gitfile. All five refuse here.
    **Four of the five** answered about the ancestor under plain ``cwd=``: the
    truncated gitfile did not, because git treats an unreadable gitfile as a
    hard error (``fatal: invalid gitfile format``) rather than continuing
    discovery upward, so that one shape was never part of the bug — it is listed
    because it is a shape a broken clone takes, not because ``cwd=`` mishandled
    it. A healthy clone and a *linked worktree* (whose ``.git`` is a gitfile,
    which git follows) both still answer normally, so pinning the clone down
    costs nothing.

    ``--work-tree`` earns its place separately from ``--git-dir``, and the suite
    goes red without it. ``core.worktree`` in the clone's own config points the
    work tree at another directory, and ``--git-dir`` alone honours it — so
    ``git status --porcelain`` compares the clone's index against *that*
    directory. What comes back depends on what is in it, and only one of the two
    outcomes is a fail-open. Both were run against git 2.55.0 on the fixture the
    tests build (a clone on a pushed branch, ``README.md`` and ``feature.txt``
    tracked, an untracked ``an-hour-of-work.md`` in the clone):

    - the other directory does **not** hold HEAD's files — an empty directory is
      the easiest case — and git prints ``" D README.md\\n D feature.txt"`` at
      rc 0. That is a :class:`WouldLose` naming two files that are not missing,
      about a directory nobody asked about: wrong, and worth fixing, but it is a
      *refusal*, so it does not destroy anything.
    - the other directory **mirrors HEAD** — a second checkout of the same
      commit, which is what ``core.worktree`` is normally pointed at — and git
      prints **nothing at rc 0**. A :class:`NothingToLose` on a clone holding an
      hour of work that exists nowhere else. That is devlaunch#171's failure
      class reached by a second route, and it is the fail-*open* one.

    So the mirrored shape is the one that makes ``--work-tree`` load-bearing,
    and it is the one ``TestGitIsPinnedToItsWorkTreeToo`` builds in
    ``test/test_workspace_state.py``. ``core.bare = true`` is the neighbouring
    shape and fails closed as well (``fatal: this operation must be run in a
    work tree``, a refusal).

    ``GIT_CEILING_DIRECTORIES`` was the other candidate and is not used: it
    bounds discovery instead of switching it off, so it has to be an absolute
    path that matches what git resolved the clone's parent to, and when it does
    not match it fails *open* — back to the ancestor, silently. ``--git-dir``
    fails closed.

    ``cwd=repo`` is kept even though it no longer selects the repository, so
    that ``git status`` keeps printing paths relative to the clone root exactly
    as before.

    Only trailing newlines are trimmed, never leading whitespace. A full
    ``strip()`` here was wrong in a way that took real use to notice: the first
    line of ``git status --porcelain`` for a *modified tracked* file begins with
    a space (`` M pixi.lock``), so stripping ate the status column and
    :func:`_name_a_few` then reported ``ixi.lock``. Untracked entries start
    ``??`` and were unharmed, which is exactly why the tests missed it.
    """
    root = repo.resolve()
    try:
        result = subprocess.run(
            ["git", f"--git-dir={root / '.git'}", f"--work-tree={root}", *args],
            cwd=repo,
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError) as e:
        logger.debug(f"git {' '.join(args)} in {repo}: {e}")
        return GitRefused(str(e))
    if result.returncode != 0:
        stderr = result.stderr.strip()
        logger.debug(f"git {' '.join(args)} in {repo}: {stderr}")
        return GitRefused(stderr or f"git {' '.join(args)} exited {result.returncode}")
    return GitSaid(result.stdout.rstrip("\n"))


def read_clone(clone: Path) -> CloneState:
    """Report what *clone* holds. The only function here that talks to git.

    A directory that is **not there** holds nothing: there is no work in it to
    lose. That is the truth about it rather than a special case, and it is what
    lets a caller clear away a workspace whose clone was already removed by
    hand.

    A directory that *is* there and that git cannot read as a repository is a
    different answer, and used to be given the same one. It holds whatever files
    are in it, and with no repository to consult nothing has established that
    they exist anywhere else — so it is a :class:`CouldNotTell`, and the delete
    stops. See this module's docstring for what that cost before it did.

    A directory dl cannot even *look at* is a third answer, and ``Path.is_dir()``
    has no way to give it — which is why the ``os.stat`` below is written out
    rather than left as ``clone.is_dir()``. ``is_dir()`` collapses every failure
    into ``False``, and it does not even do that consistently across the
    versions this package supports: up to and including Python 3.13 it swallows
    ENOENT, ENOTDIR, EBADF and ELOOP and *re-raises* the rest, so a clone whose
    parent is mode ``000`` raised ``PermissionError`` straight out of here
    (``dl <ws> rm`` failed closed by crashing; ``dl --ls --json`` became a
    traceback for the whole listing because of one workspace). On 3.14 the same
    call returns ``False`` instead, which read as "not there, so nothing to
    lose" — a clone that may be full of work, reported as free to delete,
    because dl was not allowed to look. One sentinel each way, from the same
    expression.

    The boundary is 3.14, and it is written from execution rather than from the
    changelog: the mode-``000`` parent was run on this repo's own environments,
    **3.10.20, 3.11.15, 3.12.13 and 3.13.14 raise; 3.14.6 returns ``False``**.
    Patch levels between those were not run, so "3.13 and earlier" is the
    minor-version claim those five interpreters support, not a claim about every
    release. Every one of them is now in the ``ci`` matrix, 3.14 included — it
    was not, which is how a boundary off by a whole minor version survived here
    for two rounds of review.

    ``os.stat`` raises for all of them and the errno says which: ENOENT and
    ENOTDIR mean there is no directory there to hold anything, and everything
    else means dl was stopped before it could find out, which is exactly a
    :class:`CouldNotTell`. The answer is now the same on every supported Python.

    ``ValueError`` is caught alongside ``OSError`` because ``os.stat`` is total
    over the first and not over the second: a path with a NUL byte in it — which
    a hand-edited or truncated ``metadata.json`` can put in a record — is not an
    errno, it is a ``ValueError`` raised before the syscall. Uncaught it takes
    down the whole of ``dl --ls --json`` for one bad record, which is the exact
    harm the stat guard was written to stop.
    """
    try:
        present = stat.S_ISDIR(os.stat(clone).st_mode)
    except (FileNotFoundError, NotADirectoryError):
        return CloneState(branch=None, unsaved=NothingToLose())
    except (OSError, ValueError) as e:
        logger.debug(f"could not look at {clone}: {e}")
        return CloneState(branch=None, unsaved=CouldNotTell(f"could not look at {clone}: {e}"))
    if not present:
        return CloneState(branch=None, unsaved=NothingToLose())
    head = _git(clone, "rev-parse", "--abbrev-ref", "HEAD")
    branch = head.output or None if isinstance(head, GitSaid) else None
    return CloneState(branch=branch, unsaved=_unsaved(clone, branch))


def _name_a_few(changed: List[str], limit: int = 3) -> str:
    """Name the first few changed paths from `git status --porcelain` lines.

    A count alone is not enough to decide anything with. A devcontainer that
    runs `pixi install` in its `postCreateCommand` leaves the tracked lockfile
    modified in *every* workspace it builds, so "1 uncommitted change(s)" is the
    permanent state of an otherwise untouched clone — and a person told only the
    count has no way to tell that from an hour of unsaved work. Told the name,
    they can.

    The porcelain format is two status characters, a space, then the path, so
    the path starts at offset 3; a rename reads `old -> new`, and the whole
    field is kept rather than split, because both halves are the news.
    """
    names = [line[3:].strip() for line in changed[:limit] if len(line) > 3]
    if len(changed) > limit:
        names.append("…")
    return ", ".join(names)


def _unsaved(clone: Path, branch: Optional[str]) -> Unsaved:
    """What deleting *clone* would destroy, in words — or that git could not say.

    Two kinds of loss, reported together because someone deciding whether to
    force a delete wants both:

    - a dirty tree, **untracked files included** — an agent's scratch notes are
      not less lost for never having been added — with the first few paths
      named, because a count alone cannot be judged (see :func:`_name_a_few`);
    - commits no remote-tracking ref contains. ``--not --remotes`` asks about
      *any* remote ref rather than this branch's upstream, so work that was
      pushed under another name, or merged and fetched back, is correctly not
      counted as lost.

    ``git status`` is asked first and doubles as the repository probe: with
    ``--git-dir`` naming the clone (see :func:`_git`) it succeeds on any
    repository git can read — including one with no commits yet — and refuses
    on every unusable ``.git``. A refusal here is therefore not "clean", it is
    :class:`CouldNotTell`, and so is a refusal from the ``git log`` below: once
    the repository has been shown readable, a command that then fails has
    failed for a reason nobody here can account for, and accounting for it by
    saying "nothing to lose" is the bug this module exists to not have.

    *branch* being ``None`` after a successful ``status`` means HEAD names no
    commit — a clone of an empty repository, or one checked out to a ref that
    does not exist yet. There is no commit to be unpushed, so there is nothing
    to ask ``git log`` about.
    """
    status = _git(clone, "status", "--porcelain")
    if isinstance(status, GitRefused):
        return CouldNotTell(f"git could not read {clone}: {status.reason}")

    losses: List[str] = []
    if status.output:
        changed = status.output.splitlines()
        losses.append(f"{len(changed)} uncommitted change(s) ({_name_a_few(changed)})")
    if branch:
        # Argument order is load-bearing: `--not` flips the sense of every ref
        # *after* it, so the branch has to be named before it. `log --not
        # --remotes <branch>` excludes the branch as well and is silently always
        # empty — which would report every clone as safe to delete.
        unpushed = _git(clone, "log", "--oneline", branch, "--not", "--remotes")
        if isinstance(unpushed, GitRefused):
            return CouldNotTell(
                f"git could not list unpushed commits on {branch} in {clone}: {unpushed.reason}"
            )
        if unpushed.output:
            losses.append(f"{len(unpushed.output.splitlines())} unpushed commit(s)")
    return WouldLose(" and ".join(losses)) if losses else NothingToLose()


def holds_unsaved_work(clone: Path) -> Unsaved:
    """What would be lost by deleting *clone*, as far as git can be made to say.

    The guard `dl <ws> rm` consults. Thin on purpose: the interesting behaviour
    is in :func:`read_clone`, and this is the name the guard reads by. Total —
    every path returns one of the three arms, and none of them means "go ahead"
    by default.
    """
    return read_clone(clone).unsaved
