"""Bring a cache written by an older devlaunch onto the current id scheme.

Before blooop/devlaunch#64 a clone directory's leaf was the flattened branch name
(``<cache>/repos/blooop/devlaunch/main``) and the devpod workspace id was a second,
separately derived string. Now :class:`~devlaunch.workspace_id.WorkspaceId` derives
one id that names both (``devlaunch-main-zovomobo``). Every clone directory written
by an older build therefore sits under a name nothing looks for any more.

**Renaming is the right answer, not orphaning.** A workspace is a ``git clone``
whose ``origin`` points at the ``.bare`` path, and ``.bare`` does not move, so a
plain ``rename`` is lossless: the clone keeps working and **uncommitted work
survives**. That work is the one thing in the cache that is not cheaply
recreatable, which is what decides the strategy (see #55).

**The trigger is the version header, not the directory name.** ``metadata.json``
carries a ``version`` (#56), so this migration runs exactly when
``schema_version < SCHEMA_VERSION`` and then writes the new version. Sniffing the
leaf for "a dash plus consonant-vowel pairs" was considered and rejected: a branch
literally named ``foo-bexoza`` false-positives, and the header makes the trigger
deterministic and idempotent by construction.

**Write ordering.** All renames happen first; then a single
:meth:`MetadataStorage.save` writes the new paths, and the new version header
*only if every rename succeeded*, in one atomic replace. Nothing writes the
header early, so "header says 2" always means "every rename this migration could
ever perform is done". The two outcomes it can never perform -- a collision with
a directory another record owns, and a branch no legal id derives from -- are
reported and deliberately left behind, because retrying those would never end
differently. A crash anywhere in the renames leaves the header at 1, so the next run
migrates again and finds each already-renamed directory as "destination present,
source gone" -- which it treats as a resumed rename and simply catches metadata
up to. The reverse ordering has no safe resume: saving first would bump the
header to 2 while directories were still under their old names, and the next run
would skip them for good.

**A refusal is held to the crash standard** (#180). A rename the filesystem
declines -- read-only mount, tightened permissions, full disk -- is not a crash,
but stranding its records would be just as permanent, so the header stays at 1
and the next run retries exactly the refused directories. The save still happens:
the renames that did work are recorded immediately, and the resume path above is
what stops them being redone. That is why :meth:`MetadataStorage.save` writes the
storage object's own version rather than the constant -- the migration is not the
only writer, and a save from any other operation would otherwise re-strand the
records this run deliberately left behind. A permanently refusing cache therefore
re-reports on every invocation, which is correct: the walk is bounded and the
notice names directories that really do still need a hand.
"""

import os
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Optional, Tuple

from ..workspace_id import WorkspaceId
from .storage import SCHEMA_VERSION, MetadataStorage

#: The bare reference repo shares the parent of the clone directories and is never
#: one of them. It is skipped by name because it is the layout's one fixed leaf.
BARE_DIR_NAME = ".bare"

#: Old devpod workspace ids, one per line, for the cleanup command in the notice.
ORPHAN_LIST_NAME = "orphaned-workspaces.txt"

#: Clone directories the migration deliberately did not rename, one path per line.
UNMIGRATED_LIST_NAME = "unmigrated-clones.txt"


def _notice(message: str) -> None:
    """Emit one line on stderr (stdout is parsed by the completion machinery)."""
    print(f"dl: {message}", file=sys.stderr)


@dataclass
class MigrationReport:
    """What one migration run did, for the caller and for the notices."""

    #: ``(source, destination)`` for each directory actually renamed.
    renamed: List[Tuple[Path, Path]] = field(default_factory=list)
    #: ``(source, destination, error)`` for each rename the filesystem refused.
    failed: List[Tuple[Path, Path, OSError]] = field(default_factory=list)
    #: Recorded paths that no longer exist, so there was nothing to rename.
    missing: List[Path] = field(default_factory=list)
    #: Directories left under their old name because no record names their ref.
    unmigrated: List[Path] = field(default_factory=list)
    #: ``(directory, branch)`` for records holding a ref no id can be derived from.
    unusable: List[Tuple[Path, str]] = field(default_factory=list)
    #: ``(source, destination)`` where the derived name is another record's clone.
    blocked: List[Tuple[Path, Path]] = field(default_factory=list)
    #: Old devpod workspace ids, now orphaned because the id derivation changed.
    orphaned_ids: List[str] = field(default_factory=list)


def _clone_dirs(repos_dir: Path) -> List[Path]:
    """Every workspace clone directory under ``repos_dir/<owner>/<repo>/``.

    The layout is exactly three levels deep, so this is a bounded walk rather than
    an ``rglob``: descending into the clones themselves would traverse every
    checked-out working tree in the cache.

    **One unreadable directory costs that directory and no more.** The refusal is
    caught at each level rather than around the whole walk, because a single
    ``try`` around all three meant the first unreadable owner ended the scan for
    every owner after it -- and since the owners are walked in sorted order, an
    unreadable ``acme`` silently abandoned every unmigrated clone under ``blooop``
    with no notice naming them. What the caller does with this list is decide
    which directories to *report* as left behind, so a short list is not a
    smaller job, it is a quieter one.
    """
    found: List[Path] = []

    def children(directory: Path) -> List[Path]:
        try:
            return sorted(p for p in directory.iterdir() if p.is_dir())
        except OSError as exc:
            _notice(f"could not scan {directory} for old workspace clones ({exc})")
            return []

    if not repos_dir.is_dir():
        return found
    for owner_dir in children(repos_dir):
        for repo_dir in children(owner_dir):
            found.extend(p for p in children(repo_dir) if p.name != BARE_DIR_NAME)
    return found


def _rename(src: Path, dest: Path, report: MigrationReport) -> bool:
    """Move *src* to *dest*, recording the outcome. False if it did not happen.

    ``os.rename`` and not ``shutil.move``: a rename either happens or does not,
    while a copying fallback could leave a half-written duplicate of a clone that
    holds uncommitted work. A cross-filesystem cache is rare enough to report and
    leave to the user.
    """
    try:
        dest.parent.mkdir(parents=True, exist_ok=True)
        os.rename(src, dest)
    except OSError as exc:
        report.failed.append((src, dest, exc))
        return False
    report.renamed.append((src, dest))
    return True


def _migrate_record(record, repos_dir: Path, claimed, report: MigrationReport) -> None:
    """Put one record's directory under its derived name and update the record.

    ``local_path`` as stored is the source, never a recomputed old path: the record
    is the truth about where the clone is now, which is the same principle that made
    removal work for old-scheme workspaces (#64).

    ``claimed`` is every path some record pointed at before this run started.
    """
    try:
        workspace = WorkspaceId(record.owner, record.repo, record.branch)
    except ValueError:
        # The old derivation coerced unsafe refs instead of rejecting them, so a
        # stored branch is not necessarily a legal ref. No id can be derived, so
        # there is no name to rename to; leave the record and the directory as
        # they are and say so.
        report.unusable.append((Path(record.local_path), record.branch))
        return

    src = Path(record.local_path)
    dest = repos_dir / record.owner / record.repo / workspace.value

    if dest != src and dest in claimed:
        # The derived name is a directory some *other* record owns. Only possible
        # when a branch was literally named after another branch's derived id --
        # #55's `foo-bexoza` case, now needing an exact hash match. Rename nothing
        # and, unlike every other outcome, do not repoint the record either:
        # adopting a clone another record owns is how one workspace's `rm` deletes
        # another's work, which is the class of bug #9766 was.
        report.blocked.append((src, dest))
        return

    if dest.exists():
        # Either an interrupted earlier run already renamed this clone, or a
        # newer-scheme clone was created alongside the old one. Rename nothing.
        # The record follows the canonically named directory, so that a later
        # `dl ... rm` deletes the clone devpod is actually using; a leftover src is
        # reported below, because it becomes a directory no record points at.
        pass
    elif src.exists():
        if not _rename(src, dest, report):
            return
    else:
        # Already stale before this run: the record outlived its directory. Not a
        # failure -- repointing it at the derived path is what a fresh clone would
        # use, and `workspace_exists` reads the filesystem, so nothing is misled.
        report.missing.append(src)

    old_id = record.workspace_id
    if old_id != workspace.value:
        report.orphaned_ids.append(old_id)
    record.local_path = dest
    # The record carries the derived id, because `remove_workspace_by_id` looks up
    # records by exactly the id dl derives from the spec. `devpod_workspace_id` is
    # left alone: #55 flagged holding two ids in one record as a modelling defect,
    # and giving that field a second meaning ("the orphaned old container") would
    # make the defect worse. The orphaned ids go in the notice instead.
    record.workspace_id = workspace.value


def _announce(report: MigrationReport, cache_dir: Path) -> None:
    """Tell the user what changed, in one line per kind of outcome."""
    if report.renamed:
        src, dest = report.renamed[0]
        _notice(
            f"migrated {len(report.renamed)} workspace clone director"
            f"{'y' if len(report.renamed) == 1 else 'ies'} to the new id scheme "
            f"(e.g. {src.name} -> {dest.name})"
        )
    for src, dest, exc in report.failed:
        _notice(f"could not rename {src} to {dest} ({exc}); it was left where it is")
    if report.missing:
        _notice(
            f"{len(report.missing)} metadata record(s) pointed at a clone directory that is "
            "no longer there; they now point at their new-scheme path"
        )
    for path, branch in report.unusable:
        _notice(
            f"left {path} as it is: its recorded branch {branch!r} is not a usable git ref, "
            "so no id can be derived for it"
        )
    for src, dest in report.blocked:
        _notice(
            f"left {src} as it is: its new name {dest.name} is already another workspace's "
            "clone directory; move or delete one of them by hand"
        )
    if report.unmigrated:
        listing = _write_lines(
            cache_dir / UNMIGRATED_LIST_NAME, [str(p) for p in report.unmigrated]
        )
        _notice(
            f"{len(report.unmigrated)} clone director"
            f"{'y' if len(report.unmigrated) == 1 else 'ies'} could not be renamed (no metadata "
            f"record, so the branch they were cloned for is unknown) and were left as they are"
            + (f"; listed in {listing}" if listing else "")
        )
    if report.orphaned_ids:
        listing = _write_lines(cache_dir / ORPHAN_LIST_NAME, sorted(report.orphaned_ids))
        cleanup = (
            f"xargs -r -n1 devpod delete < {listing}"
            if listing
            else "devpod delete <old-id>, one per workspace"
        )
        _notice(
            f"{len(report.orphaned_ids)} devpod container(s) still carry the old workspace ids "
            f"and are now orphaned; dl does not delete containers for you -- remove them with: "
            f"{cleanup}"
        )


def _write_lines(path: Path, lines: List[str]) -> Optional[Path]:
    """Write one line per entry, returning the path, or None if it could not be."""
    try:
        path.write_text("".join(f"{line}\n" for line in lines), encoding="utf-8")
    except OSError as exc:
        _notice(f"could not write {path} ({exc})")
        return None
    return path


def migrate_cache(storage: MetadataStorage, repos_dir: Path) -> Optional[MigrationReport]:
    """Migrate *storage* and the clone directories under *repos_dir*, once.

    Returns None when the cache is already current, which is the common case and
    costs a single integer comparison -- no filesystem scan and no devpod call.
    """
    if storage.schema_version >= SCHEMA_VERSION:
        return None

    report = MigrationReport()
    # Snapshotted before any record is touched: it has to describe the layout the
    # run started from, not one the run is halfway through rewriting.
    claimed = {Path(record.local_path) for record in storage.worktrees.values()}
    for record in storage.worktrees.values():
        _migrate_record(record, repos_dir, claimed, report)

    # Anything still under an old-scheme name that no record claims. Computed after
    # the renames so it picks up both never-recorded directories and the leftover
    # side of a collision, and excludes everything just moved into place.
    recorded = {Path(record.local_path) for record in storage.worktrees.values()}
    report.unmigrated = [path for path in _clone_dirs(repos_dir) if path not in recorded]

    # One atomic write, last: it carries the new paths and the new version header
    # together, so the header can never claim more than the filesystem has done.
    # A refusal is held to the crash standard -- the module docstring's paragraph
    # of that name is why the header only moves when nothing was refused while
    # the save happens either way (#180).
    if not report.failed:
        storage.schema_version = SCHEMA_VERSION
    storage.save()
    _announce(report, storage.metadata_path.parent)
    return report
