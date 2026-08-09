"""How much disk a directory would give back — the one number, measured once.

devlaunch reports two disk figures and they are the same question asked of
different directories: `dl --ls --size` asks it of the clone behind a live
workspace, and the orphan report asks it of a clone no workspace references any
more. Both want *what deleting this would free*, so both call
:func:`exclusive_usage` and neither grows its own walk.

**Exclusive, not apparent, and the difference is the whole point.** A repo's
bare cache holds one copy of its git objects and every workspace clone hardlinks
out of it (devlaunch#154), which is where most of the saving on a repo's second
workspace comes from. Adding up the file sizes under one clone counts that
shared pool in full, so three workspaces of one repo read as three times the
disk they actually occupy -- the design's own saving, reported as if it had
never happened. Measured on a real cache: one ROS workspace reads 755.0 MiB of
apparent size against 488.8 MiB exclusive, and the three workspaces sharing that
bare read as 2.48 GiB against the 1.70 GiB the disk really holds.

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
from typing import Any, Dict, List, NoReturn, Tuple, Union

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
        except OSError:
            unreadable.append(directory)
            continue
        for entry in found:
            try:
                st = entry.stat(follow_symlinks=False)
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
