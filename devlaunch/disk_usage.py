"""How much disk a directory would give back — the one number, measured once.

devlaunch reports two disk figures and they are the same question asked of
different directories: `dl --ls --size` asks it of the clone behind a live
workspace, and the orphan report asks it of a clone no workspace references any
more. Both want *what deleting this would free*, so both call
:func:`exclusive_usage` and neither grows its own walk.

**Exclusive, not apparent, and the difference is the whole point.** A repo's
bare cache holds one copy of its git objects and every workspace clone hardlinks
out of it (devlaunch#154), which is where most of the saving on a repo's second
workspace comes from. Walking one clone on its own and adding up the blocks its
files occupy -- what `du` reports when pointed at one directory -- counts that
shared pool in full, so a repo's workspaces each read as most of the disk they
share: the design's own saving, reported as if it had never happened.

**The measurement this module is documented from.** One real clone of a ROS
repo, made by `git clone` from the bare in devlaunch's own cache, on Ubuntu
24.04 / ext4 with a warm page cache:

===============================================  ==============
``du -s --block-size=1`` on the clone alone      353,230,848 B
``exclusive_usage`` on the clone                  68,050,944 B
``exclusive_usage`` on the bare it clones from       651,264 B
``du -sc --block-size=1`` over both together     353,882,112 B
===============================================  ==============

`du` bills that one workspace **5.2x** what deleting it would free. The
difference is one 270,823,424-byte pack file with two links, one in the clone
and one in the bare: removing either end frees nothing, and the bytes go to
whichever is last. Note the last two rows against each other -- the exclusive
figures sum to 68,702,208 B while the disk holds 353,882,112 B, which is the
first of the two consequences below and not an error.

Every number here was taken with the shipped code on one machine and is quoted
with its conditions for that reason: a figure whose conditions are lost cannot
be checked later, and this repository has twice found an inherited working
number to be wrong once somebody finally measured it.

So a file's bytes are billed to a tree only when **every one of its links lies
inside that tree**. Two consequences worth stating plainly, because they are
properties rather than bugs:

- Exclusive bytes do not sum to total disk. A pool shared by three workspaces is
  billed to none of them, because deleting any one of them frees none of it. It
  becomes the last holder's the moment it is the last holder, which is exactly
  when deleting it would free the bytes.
- The figure is a fact about the tree *and its neighbours*, so it changes when a
  sibling is deleted without the tree itself changing. That is the truth about
  shared storage; a number that stayed still would be the lie.

Sizes are bytes actually allocated (``st_blocks``), not lengths: a sparse file
and a tree of many small files both cost what the filesystem gave them, which is
what a reclamation figure has to be about.
"""

import os
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, Iterable, List, NoReturn, Tuple, Union

# st_blocks is counted in 512-byte units by POSIX, whatever the filesystem's own
# block size is.
_BLOCK = 512

_UNITS = ("B", "KiB", "MiB", "GiB", "TiB", "PiB")


@dataclass(frozen=True)
class Measured:
    """A complete walk: *exclusive_bytes* is what removing the tree would free."""

    exclusive_bytes: int


@dataclass(frozen=True)
class PartlyUnreadable:
    """A walk that hit a door it could not open, and what it got to before that.

    A clone is written into by a container running as another user, so a
    directory this process cannot read is the ordinary case here rather than the
    exotic one. The bytes behind that door are unknown, which makes
    *at_least_bytes* a floor and not a total -- and reporting a floor as a total
    is how a cleanup tool tells someone a workspace is small when it is not.

    The paths are carried rather than counted so a caller can say which door,
    which is the difference between a report someone can act on and a caveat.
    """

    at_least_bytes: int
    unreadable: Tuple[Path, ...]


# Two arms and no third: "the directory is not there" is not a failure to
# measure, it is a measurement of nothing (Measured(0)), the same answer
# workspace_state gives for a clone that has already been removed by hand.
DiskUsage = Union[Measured, PartlyUnreadable]


def _unhandled_usage(usage: NoReturn) -> NoReturn:
    """Reject a usage arm nobody handled -- at type-check time, not at runtime.

    Hand-rolled rather than `typing.assert_never`, which is 3.11+ while this
    package supports 3.10; a parameter typed `NoReturn` gets the same treatment
    from the checker.
    """
    raise AssertionError(f"Unhandled disk usage: {usage!r}")


def _allocated(st: os.stat_result) -> int:
    """Bytes the filesystem actually gave this inode."""
    return st.st_blocks * _BLOCK


def exclusive_usage(tree: Path) -> DiskUsage:
    """What removing *tree* would free, or how far the walk got before a refusal.

    One `lstat` per entry and no subprocess, so this is safe to call from
    anywhere that is not a hot path -- but it is O(files) with no ceiling, which
    is why `--ls` asks for it only when told to.

    Symlinks are weighed as themselves and never followed: a link into someone's
    home directory is a few bytes of this tree, not a claim on what it points
    at. Directories are always this tree's -- Linux does not hardlink them -- so
    their own blocks count without consulting the link count, which for a
    directory says how many children it has rather than whether it is shared.

    A tree being used changes under the walk, so anything that stops existing
    between being named and being weighed is worth nothing rather than unknown:
    it frees nothing now, which is a measurement. Only a door that will not open
    makes the answer a floor.
    """
    try:
        root = os.lstat(tree)
    except FileNotFoundError:
        # Nothing there is nothing to free, and that is an answer.
        return Measured(0)
    except OSError:
        return PartlyUnreadable(0, (tree,))

    if not stat.S_ISDIR(root.st_mode):
        return Measured(_allocated(root))

    total = _allocated(root)
    unreadable: List[Path] = []
    # inode -> [links found inside this tree, its bytes, its total link count].
    files: Dict[Tuple[int, int], List[int]] = {}
    pending = [tree]
    while pending:
        directory = pending.pop()
        try:
            with os.scandir(directory) as entries:
                found = list(entries)
        except FileNotFoundError:
            # Gone between being seen and being opened -- the same race as
            # below, one level up, and answered the same way.
            continue
        except OSError:
            unreadable.append(directory)
            continue
        for entry in found:
            try:
                st = entry.stat(follow_symlinks=False)
            except FileNotFoundError:
                # The name came back from the directory and the entry was gone
                # before it could be weighed. A live cache does that on its own
                # -- git repacks, a container writes -- and something that is
                # not there frees nothing, which is the same answer the root of
                # a walk gets for a directory that is not there. Calling a race
                # a closed door would turn an ordinary listing into a floor.
                continue
            except OSError:
                unreadable.append(Path(entry.path))
                continue
            if stat.S_ISDIR(st.st_mode):
                pending.append(Path(entry.path))
                total += _allocated(st)
                continue
            seen = files.get((st.st_dev, st.st_ino))
            if seen is None:
                files[(st.st_dev, st.st_ino)] = [1, _allocated(st), st.st_nlink]
            else:
                seen[0] += 1

    total += sum(size for links_here, size, links in files.values() if links_here >= links)
    if unreadable:
        return PartlyUnreadable(total, tuple(sorted(unreadable)))
    return Measured(total)


def _human(count: int) -> str:
    """*count* bytes in the largest binary unit that leaves it above one."""
    if count < 1024:
        return f"{count} {_UNITS[0]}"
    size = float(count)
    for unit in _UNITS[1:]:
        size /= 1024
        if size < 1024 or unit == _UNITS[-1]:
            return f"{size:.1f} {unit}"
    raise AssertionError("unreachable: the loop returns on its last unit")


def describe_usage(usage: DiskUsage) -> str:
    """How a usage reads to a person: a size, or a floor marked as one."""
    if isinstance(usage, Measured):
        return _human(usage.exclusive_bytes)
    if isinstance(usage, PartlyUnreadable):
        return f"≥{_human(usage.at_least_bytes)}"
    _unhandled_usage(usage)


def known_bytes(usage: DiskUsage) -> int:
    """The bytes this usage accounts for, whichever arm it is.

    For the callers that have to put usages in an order or add them up --
    "which of these is worth reclaiming first" -- where a floor and a total are
    both usable because the question is comparative. Exported so a caller does
    not reach into the arms and hand-roll an `else` that a third arm would
    walk straight through; the exhaustiveness check lives here.

    It is deliberately not how a usage is *reported*: printing this number
    stripped of its arm is exactly how a floor gets read as a total, which is
    what :func:`describe_usage` and :func:`usage_as_json` exist to prevent.
    """
    if isinstance(usage, Measured):
        return usage.exclusive_bytes
    if isinstance(usage, PartlyUnreadable):
        return usage.at_least_bytes
    _unhandled_usage(usage)


def _unreadable_in(usage: DiskUsage) -> Tuple[Path, ...]:
    """The doors this usage could not open -- none, for a complete walk."""
    if isinstance(usage, Measured):
        return ()
    if isinstance(usage, PartlyUnreadable):
        return usage.unreadable
    _unhandled_usage(usage)


def total_usage(usages: Iterable[DiskUsage]) -> DiskUsage:
    """What removing all of *usages*' trees together would free.

    A sum with one floor in it is a floor, and this returns the arm that says
    so. That is the whole reason a total lives here rather than in the caller:
    adding :func:`known_bytes` up gives an integer that has lost which kind of
    answer it is, and an integer printed as a size is a floor read as a total --
    the one mistake every other function in this module is shaped to prevent.

    Which bytes these are, and why they do not add up to the disk a cache holds,
    is the module docstring's business and is not restated here.
    """
    known = 0
    unreadable: List[Path] = []
    for usage in usages:
        known += known_bytes(usage)
        unreadable.extend(_unreadable_in(usage))
    if unreadable:
        return PartlyUnreadable(known, tuple(unreadable))
    return Measured(known)


def usage_as_json(usage: DiskUsage) -> Dict[str, Any]:
    """How a usage reads to a tool: one key, and the key says which kind it is.

    Deliberately not a bare integer with a flag beside it. A caller that reads
    `exclusiveBytes` has a total; a caller that reads `atLeastBytes` has a floor
    and cannot have got there by ignoring a field.
    """
    if isinstance(usage, Measured):
        return {"exclusiveBytes": usage.exclusive_bytes}
    if isinstance(usage, PartlyUnreadable):
        return {"atLeastBytes": usage.at_least_bytes, "unreadable": len(usage.unreadable)}
    _unhandled_usage(usage)
