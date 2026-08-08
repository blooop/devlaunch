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
"""

import logging
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class CloneState:
    """What one workspace clone holds, as far as git can tell.

    ``unsaved`` is the load-bearing field and is deliberately a description
    rather than a flag: it is printed to a person who is deciding whether to
    force a delete, and "3 uncommitted change(s) and 2 unpushed commit(s)" is
    the thing that answers them. ``None`` means the clone holds nothing that
    does not also exist on a remote.
    """

    branch: Optional[str]
    unsaved: Optional[str]


def _git(repo: Path, *args: str) -> Optional[str]:
    """Run git in *repo*, returning stdout, or ``None`` if it refused.

    A refusal is "cannot tell", never an answer: a clone that is broken, gone or
    not a repository must not stop the other workspaces being reported, and must
    never be reported as *safe to delete* on the strength of a failed command —
    every caller here treats ``None`` as "no information", and the one place
    that matters (:func:`holds_unsaved_work`) fails safe explicitly.
    """
    try:
        result = subprocess.run(
            ["git", *args], cwd=repo, capture_output=True, text=True, check=False, timeout=30
        )
    except (OSError, subprocess.SubprocessError) as e:
        logger.debug(f"git {' '.join(args)} in {repo}: {e}")
        return None
    if result.returncode != 0:
        logger.debug(f"git {' '.join(args)} in {repo}: {result.stderr.strip()}")
        return None
    return result.stdout.strip()


def read_clone(clone: Path) -> CloneState:
    """Report what *clone* holds. The only function here that talks to git.

    A directory that is not there, or is not a repository, holds nothing: there
    is no work in it to lose. That is the truth about it rather than a special
    case, and it is what lets a caller clear away a workspace whose clone was
    already removed by hand.
    """
    if not clone.is_dir():
        return CloneState(branch=None, unsaved=None)
    branch = _git(clone, "rev-parse", "--abbrev-ref", "HEAD")
    return CloneState(branch=branch or None, unsaved=_unsaved(clone, branch))


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


def _unsaved(clone: Path, branch: Optional[str]) -> Optional[str]:
    """What deleting *clone* would destroy, in words, or ``None`` if nothing.

    Two kinds of loss, reported together because someone deciding whether to
    force a delete wants both:

    - a dirty tree, **untracked files included** — an agent's scratch notes are
      not less lost for never having been added — with the first few paths
      named, because a count alone cannot be judged (see :func:`_name_a_few`);
    - commits no remote-tracking ref contains. ``--not --remotes`` asks about
      *any* remote ref rather than this branch's upstream, so work that was
      pushed under another name, or merged and fetched back, is correctly not
      counted as lost.
    """
    losses: List[str] = []
    status = _git(clone, "status", "--porcelain")
    if status:
        changed = status.splitlines()
        losses.append(f"{len(changed)} uncommitted change(s) ({_name_a_few(changed)})")
    if branch:
        # Argument order is load-bearing: `--not` flips the sense of every ref
        # *after* it, so the branch has to be named before it. `log --not
        # --remotes <branch>` excludes the branch as well and is silently always
        # empty — which would report every clone as safe to delete.
        unpushed = _git(clone, "log", "--oneline", branch, "--not", "--remotes")
        if unpushed:
            losses.append(f"{len(unpushed.splitlines())} unpushed commit(s)")
    return " and ".join(losses) if losses else None


def holds_unsaved_work(clone: Path) -> Optional[str]:
    """What would be lost by deleting *clone*, or ``None`` if nothing would be.

    The guard `dl <ws> rm` consults. Thin on purpose: the interesting behaviour
    is in :func:`read_clone`, and this is the name the guard reads by.
    """
    return read_clone(clone).unsaved
