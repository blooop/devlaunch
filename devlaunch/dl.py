"""
dl - DevLaunch CLI

A streamlined CLI for devpod with intuitive autocomplete and fzf fuzzy selection.
Provides an renv-like UX for managing devcontainer workspaces.

Usage:
    dl                           # fzf selector for existing workspaces
    dl <workspace>               # open/create workspace, attach shell
    dl <workspace> <command>     # run command in workspace
    dl owner/repo                # create from git repo (github.com)
    dl owner/repo@branch         # specific branch
    dl ./path                    # create from local path
    dl --ls                      # list workspaces
    dl --stop <workspace>        # stop workspace
    dl --rm <workspace>          # delete workspace
    dl --code <workspace>        # open in VS Code
    dl --install                 # install completions
"""

import sys
import subprocess
import contextlib
import functools
import json
import logging
import os
import pathlib
import re
import shlex
import stat
import time
from importlib.metadata import version as pkg_version, PackageNotFoundError, distribution
from pathlib import Path
from typing import Any, Dict, List, Mapping, NoReturn, Optional, Sequence, Tuple
from dataclasses import dataclass
from urllib.parse import urlparse
from urllib.request import url2pathname

from . import devpod_ssh, disk_usage, gh_auth, timing, tools, tty_session, workspace_state
from .completion import install_completions
from .workspace_id import TARGET_LENGTH, WorkspaceId, slug, source_workspace_id, validate_ref_name
from .worktree.config import get_worktree_config
from .worktree.locks import hold_lock, run_if_lock_free
from .worktree.migration import migrate_cache
from .worktree.models import WorktreeInfo
from .worktree.workspace_clone import WorkspaceCloneManager
from .xdg import devlaunch_cache


class MissingBinary(Exception):
    """A binary dl shells out to is not on PATH.

    Deliberately not an OSError (FileNotFoundError is one) and not a
    RuntimeError: dl catches both broadly in a dozen places so that a flaky
    command degrades to an empty list or a "failed to prepare workspace"
    message. A missing binary reported through one of those handlers is
    reported wrongly, so it travels as a type nothing between the spawn helpers
    and main() catches, and main() is the only place that handles it.
    """


class DevpodNotInstalled(MissingBinary):
    """The devpod binary is not on PATH."""


class SshNotInstalled(MissingBinary):
    """OpenSSH is not on PATH, so no command can be given a terminal.

    Its own type rather than DevpodNotInstalled: telling someone to install
    devpod when devpod is present and working would send them the wrong way.
    """


class UnreadableWorkspaceList(Exception):
    """devpod's workspace listing could not be read.

    Distinct from "this machine has no workspaces", which is a listing that
    reads fine and is empty. The two used to share one representation -- an
    empty list -- so `dl --purge` could report that there was nothing to purge
    when the truth was that it never found out, and a workspace spec could be
    called unknown when nothing had been asked.

    Deliberately not a RuntimeError, for the same reason MissingBinary is not:
    dl catches RuntimeError broadly wherever a flaky command should degrade
    rather than abort, and an answer dl could not read is precisely the thing
    that must not degrade. It travels as a type nothing in between catches, and
    main() is the only place that handles it.

    The sibling shape in devpod_provider deliberately reads the same way. Two
    listings devpod can fail to produce, two exceptions that say so, one rule.
    """


# One line, so a completion helper that trips over it cannot spew into the
# user's shell. It names both install routes because devpod ships with the
# pixi/conda package and does not ship with the pip one (see README).
DEVPOD_MISSING_MESSAGE = (
    "devpod not found on PATH: dl cannot manage workspaces without it. "
    "Install devpod from https://devpod.sh/docs/getting-started/install "
    "(pixi/conda installs of devlaunch include it; pip installs do not)."
)

# Same shape, and names the way out that does not need ssh at all: the devpod
# transport still runs commands, it just cannot give them a terminal.
SSH_MISSING_MESSAGE = (
    "ssh not found on PATH: dl needs OpenSSH to give a workspace command a "
    "terminal. Install it, or set DEVLAUNCH_NO_TTY=1 to run commands through "
    "devpod instead (interactive programs will not work)."
)

# The shell's own "command not found" code, which says more than a bare 1 and
# cannot be confused with a devpod command that ran and failed.
DEVPOD_MISSING_EXIT_CODE = 127

# devpod is there and dl asked it, but what came back could not be read. A plain
# failure rather than 127: nothing is missing and nothing needs installing.
UNREADABLE_WORKSPACE_LIST_EXIT_CODE = 1


def _install_provenance() -> Optional[str]:
    """Describe the install this dl was launched from, or None if unremarkable.

    The released build and the editable dev install report the same version, so
    the version alone cannot say which one just ran. PEP 610 records how a dist
    was installed in ``direct_url.json``: ``dir_info.editable`` is true only for
    an editable install, and ``url`` is the tree it resolves to. That is read
    from the dist's own metadata rather than inferred from where the files sit,
    so no path is stat'd and no install location is pattern-matched.

    Everything here is best-effort: a dist with no direct-url metadata, or with
    metadata that does not parse or does not carry the keys, is simply not
    described. --version must never fail over provenance, so this returns None
    instead of raising - an ambiguous version beats a broken one.
    """
    try:
        raw = distribution("devlaunch").read_text("direct_url.json")
        if not raw:
            return None
        info = json.loads(raw)
        dir_info = info.get("dir_info") if isinstance(info, dict) else None
        if not isinstance(dir_info, dict) or not dir_info.get("editable"):
            return None
        url = info.get("url")
        if not isinstance(url, str):
            return None
        parsed = urlparse(url)
        if parsed.scheme != "file":
            return None
        tree = url2pathname(parsed.path)
        if not tree:
            return None
        return f"dev, editable from {tree}"
    except Exception:
        return None


def get_version() -> str:
    """Get the package version, noting the install it came from when notable."""
    try:
        base = pkg_version("devlaunch")
    except PackageNotFoundError:
        return "unknown"
    provenance = _install_provenance()
    return f"{base} ({provenance})" if provenance else base


logging.basicConfig(level=logging.INFO, format="%(message)s")


def _get_cache_dir() -> pathlib.Path:
    """Get the cache directory, honoring XDG_CACHE_HOME.

    The answer comes from devlaunch.xdg so that this, the worktree config's
    default `repos_dir` and metadata.json's default path cannot drift apart --
    is_devlaunch_clone decides what `--purge` may delete by asking whether a
    workspace's source is under this directory, and the clones it is asking
    about were put there by the other two.
    """
    return devlaunch_cache()


# Cache configuration (honors XDG_CACHE_HOME)
CACHE_DIR = _get_cache_dir()
CACHE_FILE = CACHE_DIR / "completions.json"
BASH_CACHE_FILE = CACHE_DIR / "completions.bash"

# How long a completion cache is considered current. Refreshing it means a
# `git ls-remote` per known repo, so doing it on every invocation costs seconds
# of machine for a list of names that barely moves. An hour mirrors
# WorktreeConfig.fetch_interval, which already decides how stale devlaunch is
# willing to let its view of a repo's branches get; branch names are exactly
# what the expensive part of this refresh collects, so the two should agree.
# `dl --refresh` remains the escape hatch when a user wants it now.
COMPLETION_CACHE_TTL_SECONDS = 3600


def get_cache_path() -> pathlib.Path:
    """Get the path to the completion cache file."""
    return CACHE_FILE


def read_completion_cache() -> Optional[Dict[str, Any]]:
    """Read completion data from cache file."""
    cache_path = get_cache_path()
    if not cache_path.exists():
        return None
    try:
        with open(cache_path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError):
        return None


def write_completion_cache(data: Dict[str, Any]) -> None:
    """Write completion data to cache file (JSON format)."""
    cache_path = get_cache_path()
    try:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        # Write to temp file first, then atomic rename
        temp_path = cache_path.with_suffix(".tmp")
        with open(temp_path, "w", encoding="utf-8") as f:
            json.dump(data, f)
        # Atomic rename (on POSIX systems)
        temp_path.replace(cache_path)
    except OSError:
        pass


def write_bash_completion_cache(data: Dict[str, Any]) -> None:
    """Write completion data as a sourceable bash file."""
    try:
        BASH_CACHE_FILE.parent.mkdir(parents=True, exist_ok=True)
        workspaces = " ".join(data.get("workspaces", []))
        repos = " ".join(data.get("repos", []))
        owners = " ".join(data.get("owners", []))
        branches = " ".join(data.get("branches", []))
        lines = [
            "# Auto-generated by dl - do not edit",
            f'DL_WORKSPACES="{workspaces}"',
            f'DL_REPOS="{repos}"',
            f'DL_OWNERS="{owners}"',
            f'DL_BRANCHES="{branches}"',
        ]
        # Write to temp file first, then atomic rename
        temp_path = BASH_CACHE_FILE.with_suffix(".tmp")
        with open(temp_path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
        # Atomic rename (on POSIX systems)
        temp_path.replace(BASH_CACHE_FILE)
    except OSError:
        pass


def get_remote_branches(owner_repo: str) -> List[str]:
    """Get list of branches from a remote GitHub repository."""
    url = f"git@github.com:{owner_repo}.git"
    try:
        result = subprocess.run(
            ["git", "ls-remote", "--heads", url],
            capture_output=True,
            text=True,
            check=False,
            timeout=5,  # Don't hang on slow connections
        )
        if result.returncode == 0:
            branches = []
            for line in result.stdout.strip().split("\n"):
                if line and "refs/heads/" in line:
                    branch = line.split("refs/heads/")[-1]
                    branches.append(branch)
            return branches
    except (OSError, subprocess.SubprocessError, subprocess.TimeoutExpired):
        pass
    return []


def get_local_branches(owner_repo: str) -> List[str]:
    """Get list of local branches from the bare repo cache.

    Discovers branches that exist locally but may not yet be on the remote
    (e.g. branches created via ``dl owner/repo@new-branch``).
    """
    owner, repo = owner_repo.split("/", 1)
    repos_dir = pathlib.Path(get_worktree_config().repos_dir)
    bare_path = repos_dir / owner / repo / ".bare"
    if not bare_path.is_dir():
        return []
    try:
        result = subprocess.run(
            ["git", "for-each-ref", "--format=%(refname:short)", "refs/heads/"],
            cwd=bare_path,
            capture_output=True,
            text=True,
            check=False,
            timeout=2,
        )
        if result.returncode == 0:
            return [b for b in result.stdout.strip().split("\n") if b]
    except (OSError, subprocess.SubprocessError, subprocess.TimeoutExpired):
        pass
    return []


def update_completion_cache() -> Dict[str, Any]:
    """Update the completion cache with current data.

    The one reader of the workspace list that has something to do with a listing
    it cannot read. Everywhere else, an unreadable listing means the question
    being asked cannot be answered at all, and UnreadableWorkspaceList travels
    up to main() to say so. Here the workspace names are one of four things
    being collected, and the other three -- repos, owners, branches -- come off
    the local disk without asking devpod anything. So this catches, logs, and
    builds completions out of what it can still see, deliberately: refusing
    would mean an unreachable devpod stops `dl --install` from installing
    completions at all, when what it costs is the workspace names.

    The failure is logged rather than swallowed, because `dl --refresh` prints
    the workspace count it got and zero-because-we-could-not-ask must not read
    as zero-because-there-are-none. The cache this then writes offers no
    workspace names until a later refresh succeeds -- which is what it did
    before the listing learned to refuse -- and list_workspaces() still declines
    to remember a list it never got, so the next command asks devpod again
    instead of being served this one.
    """
    try:
        workspaces = list_workspaces()
    except UnreadableWorkspaceList as exc:
        logging.warning(f"Completing without workspace names: {exc}")
        workspaces = []
    workspace_ids = [ws.id for ws in workspaces]
    repos = discover_repos_from_workspaces(workspaces)

    # Merge repos discovered from the local cache directory so that repos
    # cloned locally (even without a devpod workspace yet) appear in completions
    for owner, repo_list in discover_repos_from_cache_dir().items():
        if owner not in repos:
            repos[owner] = []
        for repo in repo_list:
            if repo not in repos[owner]:
                repos[owner].append(repo)

    # Flatten repos to list of owner/repo strings
    known_repos = []
    for owner, repo_list in sorted(repos.items()):
        for repo in sorted(repo_list):
            known_repos.append(f"{owner}/{repo}")

    # Extract unique owners
    owners = sorted(repos.keys())

    # Fetch branches for all known repos (as owner/repo@branch strings)
    # Merge remote and local branches so locally-created branches also complete
    all_branches = []
    for owner_repo in known_repos:
        remote = set(get_remote_branches(owner_repo))
        local = set(get_local_branches(owner_repo))
        for branch in sorted(remote | local):
            all_branches.append(f"{owner_repo}@{branch}")

    data = {
        "workspaces": workspace_ids,
        "repos": known_repos,
        "owners": owners,
        "branches": all_branches,
    }
    write_completion_cache(data)
    write_bash_completion_cache(data)
    return data


def completion_cache_age_seconds() -> Optional[float]:
    """Seconds since the completion cache was last written, or None if there is none.

    The timestamp is the cache file's own mtime rather than a field inside it:
    write_completion_cache renames the finished file into place, so mtime marks a
    *completed* refresh; caches written by earlier versions carry no timestamp
    field and would otherwise read as infinitely stale; and a stat() keeps the
    check free on the path whose entire purpose is to do no work.
    """
    try:
        return max(0.0, time.time() - get_cache_path().stat().st_mtime)
    except OSError:
        return None


def completion_cache_is_fresh() -> bool:
    """Whether the completion cache is new enough to leave alone."""
    age = completion_cache_age_seconds()
    return age is not None and age < COMPLETION_CACHE_TTL_SECONDS


# One background refresh per process, at most. A dl process is one command the
# user typed and exits when that command is done, so per-process state is
# per-invocation state -- there is no long-running process here for the latch to
# go stale in. Held in a dict so it can be mutated without a `global`.
_refresh_state: Dict[str, bool] = {"spawned": False}


def cache_refresh_spawned() -> bool:
    """Whether this process has already spawned a background cache refresh."""
    return _refresh_state["spawned"]


def reset_cache_refresh_state() -> None:
    """Forget that a refresh was spawned, so another one may be (tests)."""
    _refresh_state["spawned"] = False


def update_cache_background(force: bool = False) -> None:
    """Refresh the completion cache in a detached process, if it is worth it.

    Skipped entirely when this process already spawned one, and when the cache is
    still fresh. ``force`` is for callers that have just changed what the cache
    describes -- a workspace created, stopped or deleted -- where the cache is
    wrong no matter how recently it was written. A freshness skip deliberately
    does not consume the one spawn: it means "not needed yet", not "already
    done", so a later forced call can still get its refresh.
    """
    if _refresh_state["spawned"]:
        return
    if not force and completion_cache_is_fresh():
        return
    _refresh_state["spawned"] = True
    cmd = [sys.executable, "-m", "devlaunch.dl", "--update-cache"]
    if force:
        # Tell the child its refresh is not subject to the TTL either.
        cmd.append("--force")
    try:
        # pylint: disable=consider-using-with
        subprocess.Popen(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
    except OSError:
        pass


# How long the background sweep may spend fetching one repo before it gives up
# and leaves that repo to the next pass. This is not a performance budget -- an
# incremental fetch of an already-cloned repo is seconds -- it is the ceiling on
# how long a foreground launch of the same repo can be made to wait behind the
# sweep's repo lock. Generous enough that a slow but working remote finishes,
# short enough that a hung one costs one pass rather than the hour until the
# next. The interval itself is 3600s, so a repo that times out every time is
# no worse off than one that is simply unreachable.
BACKGROUND_FETCH_TIMEOUT_SECONDS = 300.0


def sweep_repo_fetches() -> None:
    """Bring the bare-clone cache up to date, one repo at a time.

    The freshness fetch — ``+refs/heads/*`` plus tags plus prune — is a network
    call of unbounded duration, and it used to run on the launch path, under the
    per-repo lock, whenever the interval had elapsed. Whoever drew that straw
    paid for everyone's freshness, and any concurrent launch of the same repo
    queued behind them. Out here it costs a launch nothing: this is the detached
    child, spawned and forgotten, with nobody waiting on its exit.

    Three rules make it safe to run alongside real work:

    - **It never waits.** The repo lock is taken non-blockingly, so a repo some
      launch is mid-clone in is skipped rather than queued for. A sweep that
      waited would be taxing the path it exists to keep clear.
    - **It never holds a repo for long.** The other half of that is not free,
      and saying "background defers to foreground" would overstate it: the lock
      this takes is the one ``ensure_repo`` *blocks* on, so while the sweep is
      fetching, a launch of that same repo waits — and is told only that it is
      "waiting for another dl run", which here is a detached child in its own
      session that the user can neither see nor Ctrl-C. So the honest statement
      is the asymmetric one: **the sweep never queues for a launch, but a launch
      can queue for the sweep.** What keeps that survivable is that the wait has
      an upper bound rather than the network's — hence the timeout below,
      without which a remote that accepts a connection and then goes quiet holds
      the repo for as long as the kernel keeps the socket.
    - **It never complains.** A failed fetch — unreachable remote, a cache entry
      whose clone has been deleted underneath it, a fetch that ran out of time —
      is logged and stepped over, so one bad repo cannot cost the rest their
      refresh, and the interval brings it round again anyway. There is no
      terminal attached to say more.

    The interval itself is unchanged and still recorded in the one shared place
    (``last_fetched`` in metadata), which is what lets the launch path go on
    consulting it while it still does: whichever side fetches first, the other
    sees a fresh clock and does nothing.

    It reaches metadata through ``_get_clone_manager`` like every other dl path
    rather than building its own storage. That is where the one-shot cache
    migration runs, and a detached child is the worst place to skip it: nobody
    is watching it write records in a shape the rest of dl no longer reads, and
    the damage would surface later, somewhere else.
    """
    clone_mgr = _get_clone_manager()
    storage = clone_mgr.storage
    repo_manager = clone_mgr.repo_manager

    def fetch_quietly(owner: str, repo: str) -> None:
        """One repo's refresh, with whatever stopped it stepped over."""
        try:
            repo_manager.lazy_fetch(owner, repo, timeout=BACKGROUND_FETCH_TIMEOUT_SECONDS)
        except (ValueError, RuntimeError, OSError) as exc:
            logging.debug("Background fetch of %s/%s failed: %s", owner, repo, exc)

    for base_repo in storage.list_repositories():
        owner, repo = base_repo.owner, base_repo.repo
        if not run_if_lock_free(
            repo_manager.lock_path(owner, repo), functools.partial(fetch_quietly, owner, repo)
        ):
            logging.debug("Skipping fetch of %s/%s: another dl run holds it", owner, repo)


def _unsaved_work_in(workspace_id: str) -> workspace_state.Unsaved:
    """What deleting *workspace_id* would destroy, as far as dl can establish.

    Answers `NothingToLose` for a workspace devlaunch has no record of, which is
    the honest answer rather than a permissive one: those are workspaces opened
    from a path or a URL that dl never cloned and does not manage, so it has no
    clone of its own to protect and no business inspecting someone's checkout to
    find one.

    A record dl cannot *read* is a different matter and gets a different answer
    (devlaunch#171). Both used to be None, and None meant delete freely -- the
    same conflation the clone-level guard had, one level up: dl does not know
    where the clone is, so it has not established that anything is safe, and the
    delete stops until somebody says `--force`.
    """
    clone_manager = _get_clone_manager()
    try:
        record = clone_manager.storage.get_worktree_by_workspace_id(workspace_id)
    except (OSError, RuntimeError) as e:
        logging.debug(f"could not read the workspace record for {workspace_id}: {e}")
        return workspace_state.CouldNotTell(
            f"could not read the workspace record for {workspace_id}: {e}"
        )
    if record is None:
        return workspace_state.NothingToLose()
    # Resolved rather than read straight off the record, because this has to be
    # the directory the delete will remove and those were two answers
    # (devlaunch#174). See :meth:`WorkspaceCloneManager.resolve_clone_path`.
    clone_path = clone_manager.resolve_clone_path(record)
    if clone_path is None:
        # A record dl cannot turn into a directory is a record it has not
        # established anything about -- the devlaunch#171 rule, one layer out.
        return workspace_state.CouldNotTell(
            f"could not work out which directory {workspace_id}'s clone is in"
        )
    return workspace_state.holds_unsaved_work(clone_path)


def workspaces_as_json(with_size: bool = False) -> int:
    """Print the workspace list as JSON: what exists, and what each one holds.

    The machine-readable half of cleanup. devlaunch does not decide which
    workspaces are finished -- that is a fact about tickets, reviews and intent,
    none of which it knows -- so it reports what it does know and lets the
    caller that knows the rest decide. `wf` is one such caller: it named the
    branches after its tickets, so matching a workspace to a ticket is its
    business, not dl's.

    Every field is something dl can answer for certain:

    - `repo` and `branch` come from the record dl wrote when it made the clone;
      a workspace dl did not make has neither, and says so with `devlaunch:
      false` rather than a guess.
    - `unsaved` is the field a caller must not ignore: an object with exactly
      one key, and the key says which of three answers it is --
      `{"nothingToLose": true}`, `{"wouldLose": "<what>"}`, or
      `{"couldNotTell": "<why>"}`. It is null exactly where `devlaunch` is
      false: there is no clone of dl's own here to inspect. `dl <ws> rm`
      refuses on the last two as well, so a caller that forgets is still
      caught -- but a caller that reads it can leave the workspace alone
      instead of arguing with a refusal.

      It used to be a string or null, and null carried "nothing would be lost"
      as well as "could not find out" -- which is how a broken clone got
      deleted with work in it (devlaunch#171). The break is deliberate and it
      breaks the safe way: a reader that tested the old field for truthiness
      now sees a truthy object for every arm, so it leaves workspaces alone.
    - `path` is the directory `unsaved` and `checkedOut` describe, and is null
      on the same workspaces `unsaved` is.
    - `state` is devpod's, one `devpod status` per workspace, which is why this
      is a command someone runs rather than something on the fast path.
    - `disk` appears only when `--size` was asked for, and describes dl's own
      clone for this workspace -- the same directory the `SIZE` column of the
      table measures, and the same one `--purge` would delete. See
      :func:`_disk_report` for why it is a nested object and why it is not
      answered by default.
    """
    cache_dir = _get_cache_dir()
    workspaces = list_workspaces()
    clone_mgr = _get_clone_manager()
    report: List[Dict[str, Any]] = []
    for ws in workspaces:
        # One question asked once, and `devlaunch`, `path` and `unsaved` all read
        # its answer. `_measurable_clone` returns dl's own clone directory and
        # returns None exactly when the workspace is not dl's, so `mine` is
        # derived from the same value the other two are rather than computed
        # beside them -- which is what makes "`unsaved: null` iff `devlaunch:
        # false`" hold by construction instead of by a second gate that has to
        # agree with the first. It was two gates, and dropping either one of them
        # left the whole suite green.
        clone_path = _measurable_clone(ws, cache_dir)
        mine = clone_path is not None
        record = clone_mgr.storage.get_worktree_by_workspace_id(ws.id) if mine else None
        if record is not None:
            # A record moves the row onto the recorded directory -- that is the
            # one `dl <ws> rm`'s guard reads, and a listing that described a
            # different directory from the guard would be worse than useless to
            # the caller deciding whether to call it. It never makes the row
            # about a workspace dl does not own, because there is no record to
            # read unless `mine` already said so.
            #
            # Reading the record *first* and falling back to the clone was the
            # sentinel bug of devlaunch#171 surviving one layer out. A clone
            # under the cache that dl has no record for -- a metadata write that
            # failed, a record pruned, a cache restored without one -- reported
            # `devlaunch: true` and a measured `disk`, and `unsaved: null` beside
            # them. `null` is documented as "not dl's clone", which that clone is
            # not: dl says it is its own in the same object. This is the
            # identical divergence #165 fixed for `disk` (see the comment on
            # `_measurable_clone` below); `unsaved` was left behind by it.
            #
            # Resolved, not read straight off the record: the sentence above is
            # the whole point of this row, and it was not true when the recorded
            # path was stale or empty -- the guard and the delete then named
            # different directories and this row named a third (devlaunch#174).
            # None means dl could not name a directory for this record at all.
            # Keeping the measured clone rather than dropping to `null` is what
            # holds the row's own invariant: `unsaved: null` means "not dl's",
            # and dl has already said this one is its own on the line above.
            clone_path = clone_mgr.resolve_clone_path(record) or clone_path
        state = workspace_state.read_clone(clone_path) if clone_path else None
        row: Dict[str, Any] = {
            "id": ws.id,
            "devlaunch": mine,
            "repo": f"{record.owner}/{record.repo}" if record else None,
            # The recorded branch is what the workspace was made for; the
            # clone's current HEAD can differ (an agent checked something
            # else out), so both are reported rather than one being made to
            # stand for the other.
            "branch": record.branch if record else None,
            "checkedOut": state.branch if state else None,
            "path": str(clone_path) if clone_path else None,
            "state": get_workspace_state(ws.id),
            "lastUsed": ws.last_used,
            # A nested object with one key, and the key says which kind of
            # answer it is -- the shape `disk` uses, for the same reason
            # (devlaunch#171). `null` keeps only its other meaning, and keeps it
            # exactly: `mine` above is defined as `clone_path is not None`, so
            # `unsaved: null` and `devlaunch: false` are the same set by
            # construction rather than by agreement.
            "unsaved": workspace_state.unsaved_as_json(state.unsaved) if state else None,
        }
        if with_size:
            # Absent unless asked, so a reader can never mistake "nobody asked"
            # for "nothing on disk"; null where there is no clone of dl's own,
            # the same way `repo` and `branch` already say "not mine".
            #
            # The same _measurable_clone the table asks, and for the reason two
            # answers to one question is a defect rather than a duplication:
            # this used to gate on the metadata record while the table gated on
            # the source directory, so a clone under the cache that dl had no
            # record for printed a size in the table and `null` here -- and
            # `null` is documented as "not dl's to measure", which that clone
            # is not. A caller reading the JSON and a person reading the table
            # now measure the same set of workspaces by construction.
            row["disk"] = _disk_report(_measurable_clone(ws, cache_dir))
        report.append(row)
    print(json.dumps(report, indent=2))
    return 0


def _disk_report(clone: Optional[Path]) -> Optional[Dict[str, Any]]:
    """What *clone* would free, as JSON, or null when dl has no clone there.

    A nested object with one key rather than a plain integer, because there are
    two kinds of answer and they must not look alike: `exclusiveBytes` is a
    total, `atLeastBytes` is a floor a walk stopped short of. A caller reading
    an integer field has no way to tell those apart, and would report a
    part-measured clone as small when it is not -- which is exactly the sentinel
    this codebase refuses to write.

    Which bytes count, why they are not what `du` prints, and the measurement
    that settled it: :mod:`devlaunch.disk_usage`, which is the one place any of
    that is written down.
    """
    if clone is None:
        return None
    return disk_usage.usage_as_json(disk_usage.exclusive_usage(clone))


@dataclass(frozen=True)
class Refusal:
    """One path a removal could not remove, and what the system said about it.

    The reason is carried rather than reconstructed because the cause is not
    guessable from the path. A container writing as another user is the common
    one and the one devlaunch#131 is about, but a read-only mount, an immutable
    file and a busy mountpoint all reach here too -- and for the last two the
    advice that fixes the common case does not work. Printing what the errno
    said keeps the report honest about which it is.
    """

    path: pathlib.Path
    reason: str


def _why(error: OSError) -> str:
    """What the system said, in the words it used."""
    return error.strerror or str(error)


def _present(path: pathlib.Path) -> bool:
    """Whether *path* is there, where "cannot tell" counts as there.

    Only `FileNotFoundError` means there is nothing to do. Any other refusal --
    an unreadable parent directory, say -- means something is there that this
    process cannot look at, and treating that as absent is how a purge reports
    a clean sweep over an intact cache.

    `Path.exists()` cannot make that distinction and is not consistent about
    which way it fails: it returns False for an unreadable parent on some Python
    versions and raises PermissionError on others, so the code it replaced here
    answered wrongly on one and crashed on the next. Symlinks count as present
    whether or not they resolve, because the link itself is a thing to remove.
    """
    try:
        os.lstat(path)
    except FileNotFoundError:
        return False
    except OSError:
        return True
    return True


def remove_tree(tree: pathlib.Path) -> Tuple[Refusal, ...]:
    """Remove *tree* and everything under it. Returns what refused, and why.

    `shutil.rmtree` is the obvious way to do this and is the wrong one here,
    because it stops at the first failure. A container writes into its
    bind-mounted clone as its own user -- uid 1000 in the standard devcontainer
    base image -- and where the host user is not also uid 1000 the directories
    it made cannot be emptied by us. That is one clone out of a cache full of
    them, and abandoning the other clones, the completion caches and
    metadata.json on account of it is a worse outcome than the permission error
    (devlaunch#131). So this keeps going, and the refusals are the return value
    rather than an exception.

    **Only the obstruction is named**, which is not the same as the path that
    raised. Unlinking needs write permission on the *directory*, not on the
    file, so a clone directory owned by the container's user refuses every one
    of its children separately -- on a real e2e workspace that is forty-odd
    `.git/objects` entries, hooks and a README, none of them an ancestor of
    another and every one of them the same single fact. So a failure is
    attributed upward to the outermost directory that cannot be written into,
    which is the directory the original errno named and the one a person would
    go and look at.

    A path is then suppressed when something already reported accounts for it:
    a directory that cannot be removed because a child refused adds nothing. A
    *separately* sealed ancestor is not suppressed and should not be, because
    fixing the one below it would not free it -- so a chain of two sealed
    directories is two lines, and each is work somebody has to do.

    **What refused is decided from the disk, not from what raised.** A failure
    during the walk is only a candidate; the report keeps the ones still on disk
    when it is over. Both suppression rules are then applied to that surviving
    list, so a path that vanished after failing can neither be reported nor
    suppress the report of something real.

    That is not a belt-and-braces check, it is load-bearing, and randomised
    trees found the case: `os.walk` cannot scan an unlistable directory and says
    so, but if that directory is *empty* the `rmdir` afterwards succeeds. Noting
    it when it raised named a path that is not there, and -- through the
    ancestor rule -- could have silenced a genuine refusal above it, which is
    the one failure direction that matters here.

    An empty result means the tree is gone, including when it was never there:
    a purge run twice is not a failure the second time.
    """
    # One lstat, three outcomes, none of them inferred. `Path.exists()` and
    # `Path.is_symlink()` cannot be used here: they answer False for a path this
    # process was not allowed to look at on some Python versions and raise
    # PermissionError on others, and neither of those is "there is nothing to
    # remove".
    try:
        info = os.lstat(tree)
    except FileNotFoundError:
        return ()
    except OSError as error:
        # Something is there that we are not allowed to look at. Saying so is
        # the whole point; calling it gone is the failure this guards.
        return (Refusal(tree, _why(error)),)

    # A symlinked root is refused, which is what `shutil.rmtree` did and is the
    # only one of the three available answers that is not a lie.
    #
    # `os.walk`'s `followlinks=False` governs *subdirectories*; the top is
    # always scanned. So following it empties a directory the caller never
    # named. Unlinking just the link is no better and is worse to diagnose: the
    # clones are still on the other disk and the purge says "Removed". A cache
    # root is a symlink because somebody moved their cache, so both of those
    # answers cost them their workspaces -- one by deleting them, one by telling
    # them they are gone.
    #
    # Naming the target matters: `sudo rm -rf <cache>` would remove the link and
    # nothing else, so the reader needs the real location to act on.
    if stat.S_ISLNK(info.st_mode):
        try:
            points_at = f" to {os.readlink(tree)}"
        except OSError:
            points_at = ""
        return (Refusal(tree, f"is a symbolic link{points_at}, which a purge will not follow"),)

    failed: List[Refusal] = []

    def obstruction(path: pathlib.Path) -> pathlib.Path:
        """The outermost path that actually explains a failure to remove *path*.

        `os.access` is advisory -- it answers for the real uid and knows nothing
        about ACLs -- and that is acceptable precisely here, because it only
        decides *which* path is named. A wrong answer makes the report less
        pointed; it can never turn a refusal into a success.

        `path.parent != path` bounds the walk at the filesystem root as well as
        at *tree*. Nothing reaches here from outside *tree* today; the guard is
        so that a future caller that does gets a wrong answer rather than a
        hung purge.
        """
        while path != tree:
            parent = path.parent
            if parent == path:
                break  # the filesystem root: there is nothing above to blame
            if os.access(parent, os.W_OK | os.X_OK):
                break  # this one is reachable, so *path* is where it stops
            path = parent
        return path

    def unreadable(error: OSError) -> None:
        # os.walk reports a directory it could not scan here and then carries on
        # as though it were empty. Without this, an unlistable directory holding
        # files would be walked as though it held none.
        if error.filename:
            failed.append(Refusal(pathlib.Path(error.filename), _why(error)))

    def remove(path: pathlib.Path) -> None:
        try:
            # A symlink is unlinked, never followed -- descending one would put
            # a purge outside the cache directory it was asked to remove.
            if path.is_dir() and not path.is_symlink():
                path.rmdir()
            else:
                path.unlink()
        except OSError as error:
            failed.append(Refusal(path, _why(error)))

    # Bottom-up, so a directory is only attempted once its contents have been.
    for parent, dirs, files in os.walk(tree, topdown=False, onerror=unreadable):
        here = pathlib.Path(parent)
        for name in files:
            remove(here / name)
        for name in dirs:
            remove(here / name)
    # The root is in nobody's `dirs`, so it is removed by name.
    remove(tree)

    # Bottom-up order is what the ancestor rule needs, and `failed` is already
    # in it.
    refused: List[Refusal] = []
    blocked = set()
    for candidate in failed:
        # _present, not `exists()`: a path this process cannot look at must be
        # reported, not dropped. Dropping it is how the filter that exists to
        # prevent phantom refusals would have started causing silent ones.
        if not _present(candidate.path):
            continue  # it went in the end, so there is nothing to report
        path = obstruction(candidate.path)
        if path not in blocked:
            refused.append(Refusal(path, candidate.reason))
            blocked.add(path)
        blocked.add(path.parent)
    return tuple(refused)


def purge_all_data() -> int:
    """Purge devlaunch's data: the workspaces it created, and its caches.

    This:
    1. Deletes the DevPod workspaces devlaunch created -- see
       is_devlaunch_clone for what that means, and for what it leaves alone.
    2. Removes ~/.cache/devlaunch/ which contains:
       - completions.json, completions.bash (completion caches)

    Workspaces devlaunch did not create are not deleted and not reported here;
    the report belongs with the confirmation, before anything is destroyed, so
    it lives in main() where the user still has a decision to make.

    A cache that does not come away completely is reported rather than raised:
    see remove_tree for why it is removed as far as it goes, and the exit code
    below for what that leaves the caller to say.
    """
    cache_dir = _get_cache_dir()

    # First, delete the DevPod workspaces devlaunch made. The list is the same
    # snapshot the caller printed the count from -- list_workspaces() is
    # memoized per command and nothing between the two reads changes what devpod
    # would say -- so the confirmation the user answered and the set actually
    # deleted cannot disagree.
    owned = workspace_ownership(list_workspaces(), cache_dir)
    for ws in owned.mine:
        print(f"Deleting DevPod workspace: {ws.id}")
        result = run_devpod(["delete", ws.id, "--force"], capture=True)
        if result.returncode != 0:
            logging.warning(f"Failed to delete workspace {ws.id}: {result.stderr}")
    if owned.mine:
        invalidate_workspace_list_cache()

    # Then remove local cache. See _present for why this is not `exists()`: a
    # cache that is there but unreachable must be reached for, not reported as
    # nothing to do.
    if not _present(cache_dir):
        if not owned.mine:
            print("No data to purge.")
        return 0

    refused = remove_tree(cache_dir)
    if not refused:
        print(f"Removed: {cache_dir}")
        return 0

    # Not 0: a clone the user was told would go is still on disk. Not silent
    # either -- an exit code cannot distinguish "removed most of it" from
    # "removed none of it", and the difference is the whole news, so the report
    # carries it and the exit code only says the job is unfinished.
    report_refusals(
        refused,
        f"Removed what was permitted under {cache_dir}. These refused:",
        (cache_dir,),
    )
    return 1


def report_refusals(
    refused: Sequence[Refusal], headline: str, remove_by_hand: Sequence[pathlib.Path]
) -> None:
    """Print what would not come away, and the one thing that usually clears it.

    Shared by the two commands that remove directories, because a second copy of
    this advice is a second copy to keep true -- and the advice is the part most
    likely to change, being the only part that is a guess.

    Hedged, because the cause is not knowable from here. A container writing as
    another user is the common one, but a read-only mount, `chattr +i` and a busy
    mountpoint all land in the same report -- and for the last two this command
    does not help either. Saying so flatly would be wrong more often than the
    errno beside each path is.
    """
    print(headline)
    for refusal in refused:
        print(f"  - {refusal.path}: {refusal.reason}")
    print()
    print("Usually this means a container wrote them as a different user, and:")
    # Quoted: these paths descend from $XDG_CACHE_HOME or $HOME, and a space in
    # one turns a pasted `sudo rm -rf` into two targets, the first of them wrong.
    print(f"  sudo rm -rf {' '.join(shlex.quote(str(path)) for path in remove_by_hand)}")
    print("clears them. Check the reasons above first -- it does not fix all of them.")


# Regex to match owner/repo[@branch] format (not a path, not already a URL)

OWNER_REPO_PATTERN = re.compile(r"^[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+(@[a-zA-Z0-9_./%-]+)?$")


def parse_owner_repo_branch(spec: str) -> Optional[tuple]:
    """Parse owner/repo[@branch] spec into (owner_repo, branch) tuple.

    Returns None if spec doesn't match owner/repo format.
    Branch may be None if not specified.
    """
    # Skip paths and full URLs
    if is_path_spec(spec) or "://" in spec:
        return None
    if spec.startswith("github.com/") or spec.startswith("gitlab.com/"):
        return None

    if not OWNER_REPO_PATTERN.match(spec):
        return None

    if "@" in spec:
        owner_repo, branch = spec.split("@", 1)
        return (owner_repo, branch)
    return (spec, None)


def _resolve_devcontainer_ref(ref: str) -> str:
    """Turn a --devcontainer value into a devcontainer.json path for devpod.

    A bare name expands to the spec's one-level-deep variant location,
    `.devcontainer/<name>/devcontainer.json`. Anything containing a separator or
    ending in .json is used as given.

    devpod's own --devcontainer-id takes a bare variant name and looks like the same
    job, but it is silently ignored in devpod 0.26.1: a fresh `devpod up --id x
    --devcontainer-id alt` parses .devcontainer/devcontainer.json and stores no
    devContainerID, while --devcontainer-path selects the variant correctly. Build
    the path here until that is fixed upstream.

    Raises ValueError on a value that cannot be a path, so a missing argument
    (`dl --devcontainer --help`) is an error instead of a nonsense path.
    """
    if not ref or ref.isspace():
        raise ValueError("--devcontainer requires a variant name or path")
    if ref.startswith("-"):
        raise ValueError(f"--devcontainer needs a value, got the flag {ref!r}")
    if "/" in ref or ref.endswith(".json"):
        return ref
    return f".devcontainer/{ref}/devcontainer.json"


# dl options whose value is a separate argument. aid splits its own command line
# before handing it to dl and has to tell such a value from the workspace spec,
# so the list lives here, next to the parsing it describes.
DL_VALUE_OPTIONS = frozenset({"--devcontainer"})


def extract_devcontainer_flag(args: List[str]) -> tuple[List[str], Optional[str]]:
    """Pull `--devcontainer <name-or-path>` out of the argument list.

    Returns (remaining_args, devcontainer_path). Accepts both
    `--devcontainer x` and `--devcontainer=x`.

    Scanning stops at the first bare `--`: everything after it is the shell
    command `dl <ws> -- <command>` runs inside the workspace, and must reach it
    verbatim even when it has flags of its own by the same name.

    Raises ValueError if the flag is given without a usable value.
    """
    remaining: List[str] = []
    selection: Optional[str] = None
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--":
            remaining.extend(args[i:])
            break
        if arg.startswith("--devcontainer="):
            selection = _resolve_devcontainer_ref(arg.split("=", 1)[1])
            i += 1
            continue
        if arg == "--devcontainer":
            if i + 1 >= len(args):
                raise ValueError("--devcontainer requires a variant name or path")
            selection = _resolve_devcontainer_ref(args[i + 1])
            i += 2
            continue
        remaining.append(arg)
        i += 1
    return remaining, selection


def is_path_spec(spec: str) -> bool:
    """Check if spec looks like a filesystem path."""
    return spec.startswith("./") or spec.startswith("/") or spec.startswith("~")


def is_git_spec(spec: str) -> bool:
    """Check if spec looks like a git repo (owner/repo or URL)."""
    # Paths are not git specs
    if is_path_spec(spec):
        return False
    if "://" in spec:
        return True
    if spec.startswith("github.com/") or spec.startswith("gitlab.com/"):
        return True
    return bool(OWNER_REPO_PATTERN.match(spec))


def expand_workspace_spec(spec: str) -> str:
    """Expand owner/repo[@branch] to git@github.com:owner/repo.git[@branch] for devpod."""
    # Don't expand if it's a path
    if is_path_spec(spec):
        return spec
    # Don't expand if it already looks like a URL
    if "://" in spec or spec.startswith("github.com/") or spec.startswith("gitlab.com/"):
        return spec
    # Don't expand if it's already an SSH URL (more specific pattern: user@host:path)
    # Matches patterns like git@github.com:, git@gitlab.com:, etc.
    if re.match(r"^[^@]+@[^:]+:", spec):
        return spec
    # Check if it matches owner/repo[@branch] pattern - use SSH URL format for GitHub
    if OWNER_REPO_PATTERN.match(spec):
        if "@" in spec:
            owner_repo, branch = spec.split("@", 1)
            return f"git@github.com:{owner_repo}.git@{branch}"
        return f"git@github.com:{spec}.git"
    # Otherwise return as-is (existing workspace name)
    return spec


def setup_hostname(workspace_id: str) -> bool:
    """Set the container hostname so the terminal prompt shows the project/branch.

    The hostname appears in the bash prompt (user@hostname:path$), giving users
    a clear indicator of which project and branch they're in.

    Best-effort: silently ignores failures (e.g. if sudo is unavailable via
    devpod ssh). The workspace path already contains the workspace ID.

    Args:
        workspace_id: The DevPod workspace ID (e.g. 'bencher-ws5')

    Returns:
        True if hostname was set successfully, False otherwise
    """
    result = run_devpod(
        ["ssh", workspace_id, "--command", f"sudo hostname {workspace_id}"],
        capture=True,
    )
    return result.returncode == 0


def spec_to_workspace_id(spec: str) -> str:
    """Derive the workspace ID for a given spec.

    For owner/repo@branch this is `WorkspaceId(owner, repo, branch).value` — see
    devlaunch.workspace_id for the format. The other spec shapes are not
    (owner, repo, ref) triples, so they cannot be parsed into one:

    - For owner/repo with branch: <repo-slug>-<branch-slug>-<syllables>
    - For owner/repo without branch: <repo-slug>. Not a workspace identity: a
      workspace is a branch checkout, so every caller that creates one resolves
      the default branch first and passes it. This is only a repo label.
    - For other git URLs: the slugged URL, as devpod would name it
    - For paths: the directory name (e.g., ./my-project -> my-project)
    - For existing IDs: the ID as-is

    Raises:
        ValueError: if the spec names an owner, repo or branch that is not a safe
            git name.
    """
    # Check for @branch suffix
    if "@" in spec:
        base_spec, _ = spec.split("@", 1)
    else:
        base_spec = spec

    # For paths, use the directory name
    if is_path_spec(base_spec):
        return pathlib.Path(base_spec).expanduser().resolve().name

    # For git URLs or owner/repo
    if is_git_spec(base_spec):
        parsed = parse_owner_repo_branch(spec)
        if parsed:
            owner_repo, parsed_branch = parsed
            owner, repo_name = owner_repo.split("/", 1)
            if parsed_branch:
                return WorkspaceId(owner, repo_name, parsed_branch).value
            # A repo label, not an identity — see the docstring. Capped anyway, so no
            # caller can get a string from here that overflows devpod's limit.
            return slug(repo_name)[:TARGET_LENGTH].strip("-")

        # Fallback for non-owner/repo git URLs (github.com/..., https://...)
        full_source = expand_workspace_spec(base_spec)
        # Strip protocol prefix if present
        if "://" in full_source:
            full_source = full_source.split("://", 1)[1]
        # Strip SSH URL prefix (user@host:) if present
        ssh_match = re.match(r"^[^@]+@([^:]+):(.*)", full_source)
        if ssh_match:
            full_source = f"{ssh_match.group(1)}/{ssh_match.group(2)}"
        # Strip .git suffix if present
        if full_source.endswith(".git"):
            full_source = full_source[:-4]
        # Same scheme as a triple: slug for legibility, suffix for identity, capped.
        # The old rule here deleted `_` while the owner/repo path turned it into `-`,
        # so one repo derived two ids; applying only the slug rule instead swapped
        # that for a collision, since `my_repo`, `my-repo` and `my.repo` slug alike.
        return source_workspace_id(full_source)

    # Otherwise assume it's already a workspace ID
    return spec


@dataclass(frozen=True)
class LocalFolder:
    """devpod is opening a directory on this machine."""

    path: str


@dataclass(frozen=True)
class GitRepository:
    """devpod is opening a repository it clones itself, named by URL.

    Never a path on this machine, which is why the purge predicate can refuse
    this arm outright instead of asking where it points.
    """

    url: str


@dataclass(frozen=True)
class UnrecognisedSource:
    """A `devpod list` source that opens no directory on this machine.

    Reachable rather than defensive: devpod's workspace source also carries
    `image` and `container`, so `devpod up ubuntu:24.04` lands here. Holding the
    payload rather than a rendering of it is the whole point -- the field this
    replaced was typed as a path and filled with `str(the dict)`, so the one
    thing a caller could not do with it was read what devpod had said.

    It deliberately has no path and no URL, and that is a *fact about the
    workspace* rather than a gap in devlaunch's reading: an image workspace
    mounts no folder here, so no clone directory can be at risk from it. A
    source that does name a folder and could not be read is the opposite answer
    to that same question and is :class:`UnreadableLocalFolder`.
    """

    payload: Mapping[str, Any]


@dataclass(frozen=True)
class UnreadableLocalFolder:
    """devpod says this workspace opens a folder, and devlaunch cannot say which.

    Split from UnrecognisedSource because the two are opposite answers to the
    one question a deletion has to ask, and sharing an arm made the dangerous
    one silent: `--prune` reads "no path" as "contributes no path and no alarm",
    which is right for an image workspace and wrong for this -- a live workspace
    really is opening a directory, and while devlaunch cannot say which, there
    is no clone it can honestly call unreferenced.

    Reached by a `localFolder` devpod filled with something that is not a
    non-empty string: an object, a number, a list. The payload is kept whole so
    the report can show what devpod actually said.
    """

    payload: Mapping[str, Any]


WorkspaceSource = LocalFolder | GitRepository | UnrecognisedSource | UnreadableLocalFolder


def _unhandled_source(source: NoReturn) -> NoReturn:
    """Reject a source arm nobody handled -- at type-check time, not at runtime.

    `ty` runs in CI, so passing anything but `Never` here is a build failure:
    adding an arm to WorkspaceSource breaks every reader that has not grown a
    case for it. That is the property this type exists for, and it is why this
    body should be unreachable in a passing build.

    Hand-rolled rather than `typing.assert_never`, which is 3.11+ while this
    package supports 3.10. A parameter typed `NoReturn` gets the same treatment
    from the checker.
    """
    raise AssertionError(f"Unhandled workspace source: {source!r}")


def _readable_text(source: Mapping[str, Any], key: str) -> Optional[str]:
    """*key* from *source* if devpod filled it with text, else None.

    devpod's listing is JSON dl did not write, so `source[key]` is whatever the
    JSON held. Returning None for anything else is what keeps the arms below
    from being handed a value of a type their field does not describe.
    """
    value = source.get(key)
    return value if isinstance(value, str) and value else None


def parse_workspace_source(source: Mapping[str, Any]) -> WorkspaceSource:
    """Read one `devpod list --output json` source object.

    The keys are checked in devpod's own order of specificity: a source object
    carrying both is a devpod that has changed under us, and taking the first
    match keeps that from silently becoming the other one.

    An arm is only as honest as the value put into it, so a key has to be
    non-empty text before it counts. Both halves of that are load-bearing rather
    than tidy. `git -C ""` is a no-op that succeeds, so a `LocalFolder("")`
    reaching repo discovery would be credited with whatever repository the
    person running `dl` happened to be standing in. And a `localFolder` that is
    itself an object would put a dict in a field typed as a path -- the exact
    thing this type exists to make unrepresentable -- which the two readers that
    treat it as a path raise `TypeError` on.

    Neither is an unreadable *listing*, unlike a `source` that is not an object
    at all: the object is right here and can be kept whole. It is a source dl
    cannot read, which is an arm -- and *which* arm turns on whether devpod
    claimed a folder here at all. A `localFolder` holding an object or a number
    is a workspace that opens a directory devlaunch cannot name; a source with
    no `localFolder` key, or an empty one, opens no directory here, which is
    what an `image` or `container` workspace is. Only the first can put a clone
    at risk, so only the first is an alarm.

    Note this is the one link in the chain the type checker cannot stand in for
    a test. A *reader* that misses an arm is a build failure, but this is a
    producer, and a producer that quietly stops producing an arm type-checks
    fine -- so the arms it picks are pinned by tests instead.
    """
    path = _readable_text(source, "localFolder")
    if path is not None:
        return LocalFolder(path)
    claimed = source.get("localFolder")
    if claimed is not None and claimed != "":
        return UnreadableLocalFolder(dict(source))
    url = _readable_text(source, "gitRepository")
    if url is not None:
        return GitRepository(url)
    return UnrecognisedSource(dict(source))


def describe_source(source: WorkspaceSource) -> Tuple[str, str]:
    """How a source reads in `dl --ls` and in the fzf picker: (kind, detail).

    One function for both columns, and one function for both callers, so the
    kind shown and the detail shown cannot come from two different readings of
    the same source -- which is what a tag beside a parallel field allowed.

    An unreadable source shows the payload as JSON. That is a debug rendering
    and is meant to be: the listing's job is to put the row on screen rather
    than pass over it, and devlaunch has nothing truer to say about it.
    """
    if isinstance(source, LocalFolder):
        return "local", source.path
    if isinstance(source, GitRepository):
        return "git", source.url
    if isinstance(source, (UnrecognisedSource, UnreadableLocalFolder)):
        return "unknown", json.dumps(source.payload)
    _unhandled_source(source)


@dataclass
class Workspace:
    """Represents a devpod workspace."""

    id: str
    source: WorkspaceSource
    last_used: str
    provider: str
    ide: str

    @classmethod
    def from_json(cls, data: Mapping[str, Any]) -> "Workspace":
        """Parse workspace from devpod JSON output."""
        return cls(
            id=data.get("id", ""),
            source=parse_workspace_source(data.get("source", {})),
            last_used=data.get("lastUsed", ""),
            provider=data.get("provider", {}).get("name", ""),
            ide=data.get("ide", {}).get("name", ""),
        )


def is_devlaunch_clone(workspace: Workspace, cache_dir: pathlib.Path) -> bool:
    """Whether *workspace* is one devlaunch made, rather than someone else's.

    devpod's workspace namespace is shared: `devpod up` by hand, another tool
    and an older devlaunch all land in the same list, and devlaunch has no
    business destroying any of them. The question this answers is the narrow
    one -- *did I make this* -- and it is answered from where the workspace's
    source lives.

    Every workspace `dl owner/repo[@branch]` creates is a clone devlaunch put
    under its own cache directory (`<cache>/repos/<owner>/<repo>/<id>`, see
    WorkspaceCloneManager.get_workspace_path) and then handed to `devpod up` as
    a path, so devpod records that path as the source. That makes the predicate
    say something true of `--purge` rather than merely correlated with it: the
    cache directory is exactly what a purge removes, so the workspaces it
    deletes are the ones whose source it is about to delete anyway. Anything
    else keeps working afterwards, because nothing a purge touches backs it.

    Chosen over reading metadata.json, which also records these ids, for two
    reasons. metadata.json lives *inside* the cache directory a purge removes,
    and `purge_all_data` only warns when `devpod delete` fails -- so one failed
    delete plus one successful purge would leave a workspace no later purge
    could ever recognise. And the record is append-mostly: nothing prunes it,
    so it accumulates entries for workspaces that are long gone. A source path
    is read back from devpod itself every time, and survives the cache it names.

    Deliberately conservative in two places. A `git` or unrecognised source is
    never ours: devlaunch always passes devpod a local path. And the comparison
    is a path containment test, not a string prefix -- `<cache>-scratch` shares
    six characters with `<cache>` and is not inside it.

    That first refusal is now half structural. An unrecognised source has no
    path on it at all, so there is nothing to test for containment and no way
    to write the mistake; a git source does carry a string that could be one --
    `devpod up <path-to-bare-repo>` records a `gitRepository`, and nothing
    stops that repo living in the cache -- so refusing that arm is a decision
    this function still has to make, and it is made by name.

    Not every workspace dl creates is recognised. `dl ./path` and `dl <git-url>`
    open a source dl did not clone and does not record anywhere, and a
    `config.toml` that points `repos_dir` outside the cache puts the clones
    somewhere `--purge` does not remove either -- so all three read as someone
    else's. That is the safe direction to be wrong in, it keeps this predicate
    and what a purge actually destroys answering to the same directory, and
    `--purge` names what it leaves rather than passing over it in silence.
    """
    source = workspace.source
    if isinstance(source, LocalFolder):
        path = pathlib.PurePath(source.path)
        # Purely lexical, so a clone whose directory has already been removed is
        # still recognisable from the source devpod kept.
        return path != pathlib.PurePath(cache_dir) and path.is_relative_to(cache_dir)
    if isinstance(source, (GitRepository, UnrecognisedSource, UnreadableLocalFolder)):
        return False
    _unhandled_source(source)


def _measurable_clone(workspace: Workspace, cache_dir: pathlib.Path) -> Optional[pathlib.Path]:
    """Devlaunch's own clone directory for *workspace*, or None when it has none.

    Where the boundary "is this dl's to touch?" is kept, for every surface that
    needs it. `--ls --size` and the table's `SIZE` column walk it, and a
    `--ls --json` row derives three more fields from this one answer:
    `devlaunch` (there is a clone of dl's), `path` (which directory) and
    `unsaved` (what is in it). Asking once is what stops those from disagreeing
    about the same workspace -- `disk` and then `unsaved` each used to ask a
    question of their own and each came apart from its neighbours in turn
    (devlaunch#165, then #171).

    Only devlaunch's own clones. `dl ./path` opens somebody's project directory
    and `dl <url>` opens a source devlaunch never cloned; walking either would
    put an unbounded stat of a stranger's tree behind a listing command, the
    bytes would not be devlaunch's disk in any case, and dl has no business
    reporting on the state of a checkout it did not make.

    Ownership is the same predicate `--purge` deletes by, not the presence of a
    metadata record. A record can be missing from a cache that is very much on
    disk, and a workspace dl reports as its own must not then be reported as
    unmeasurable -- the disk it would free is the same either way.
    """
    source = workspace.source
    if isinstance(source, LocalFolder) and is_devlaunch_clone(workspace, cache_dir):
        return pathlib.Path(source.path)
    return None


@dataclass(frozen=True)
class WorkspaceOwnership:
    """A `devpod list` answer split by whether devlaunch created each workspace.

    A value rather than a filter applied at the point of deletion: `--purge`
    prints a count and then deletes a set, and those two cannot disagree if they
    read the same object. It also gives the workspaces devlaunch does *not* own
    somewhere to be named from, which is the difference between a purge that
    surprises a user with survivors and one that lists them.
    """

    mine: Tuple[Workspace, ...]
    foreign: Tuple[Workspace, ...]


def workspace_ownership(
    workspaces: Sequence[Workspace], cache_dir: pathlib.Path
) -> WorkspaceOwnership:
    """Split *workspaces* into the ones devlaunch made and the ones it did not.

    Total: every workspace lands in exactly one arm, and listing order is kept
    within each, so what is printed reads in the order devpod gave.
    """
    mine: List[Workspace] = []
    foreign: List[Workspace] = []
    for ws in workspaces:
        # One pass, so the predicate is asked once per workspace and the two
        # arms cannot be built from two different answers to the same question.
        (mine if is_devlaunch_clone(ws, cache_dir) else foreign).append(ws)
    return WorkspaceOwnership(mine=tuple(mine), foreign=tuple(foreign))


@dataclass(frozen=True)
class Referenced:
    """A live devpod workspace opens this exact clone directory."""

    workspace_id: str


@dataclass(frozen=True)
class Orphaned:
    """Nothing opens this directory and no record ties it to a live workspace.

    *unsaved* is what deleting it would destroy, in the words `dl <ws> rm`
    already refuses in -- and it is a three-armed answer rather than a
    description-or-None, because "git would not say" is not "nothing to say".
    It sits inside this arm rather than beside the classification because it is
    only ever actionable here: "unsaved work on a clone that is staying anyway"
    is a sentence this type cannot say.

    *usage* is here for the same reason, and it earns its place twice over. The
    walk behind it is O(files) with no ceiling, and this is the only arm whose
    bytes anybody is going to get back -- so putting it here is what keeps the
    other two arms from being walked at all.
    """

    unsaved: workspace_state.Unsaved
    usage: disk_usage.DiskUsage


@dataclass(frozen=True)
class Disputed:
    """devpod lists the workspace this directory's record names, elsewhere.

    devlaunch#88's shape, and the reason `--prune` does not wait on #88: 36 of
    39 devpod records on that ticket's host pointed at folders that no longer
    existed, and under that state a healthy clone at the *new* path is sourced
    by nobody. Read as an orphan it would be deleted; read as referenced it
    would silently hide disk. It is neither -- it is two records disagreeing,
    and the answer to a disagreement is to keep the directory and say so.
    """

    workspace_id: str
    sourced_at: str


CloneStatus = Referenced | Orphaned | Disputed


def _unhandled_status(status: NoReturn) -> NoReturn:
    """Reject a clone status nobody handled -- at type-check time, not at runtime.

    The same device as _unhandled_source, and it matters more here: the function
    it guards is the only place a directory becomes deletable, so an arm that
    fell through it would fall through into a deletion.
    """
    raise AssertionError(f"Unhandled clone status: {status!r}")


@dataclass(frozen=True)
class Unopposed:
    """Nothing objected to removing this directory."""


@dataclass(frozen=True)
class Insisted:
    """`--force` carried this directory past an objection, named in *despite*.

    Carried on the decision rather than read from the plan's `--force` flag, and
    that difference is a deletion. A plan-wide boolean says "the user insisted"
    about every directory in the plan, including the ones nothing objected to --
    so the later re-probe, which exists to catch work written while the user was
    reading the report, was skipped for clones `--force` had not promoted at
    all. A promotion belongs to the directory it promoted.
    """

    despite: str


Promotion = Unopposed | Insisted


def _unhandled_promotion(promotion: NoReturn) -> NoReturn:
    """Reject a promotion nobody handled -- at type-check time, not at runtime."""
    raise AssertionError(f"Unhandled promotion: {promotion!r}")


@dataclass(frozen=True)
class Remove:
    """This directory goes, this is what it gives back, and this is what was
    insisted past to get here.

    The bytes travel with the decision rather than beside it, so "what this run
    reclaims" is a total over the things it is actually removing and cannot be
    assembled from a different set than the one that dies. The promotion travels
    with it for the same reason one layer down: it is the only record that
    `--force` answered *this* directory's objection, and the report and the
    second pass both need to know which.
    """

    usage: disk_usage.DiskUsage
    promotion: Promotion


@dataclass(frozen=True)
class Keep:
    """This directory stays, and *because* says why, for a person to read."""

    because: str


Decision = Remove | Keep


def _objection(unsaved: workspace_state.Unsaved) -> Optional[str]:
    """What deleting a clone in this state would destroy or risk, or ``None``.

    The one place `--prune` turns devlaunch#171's three answers into the two
    :func:`decide` acts on: something to say, or nothing. ``None`` is safe here
    in a way it was not on the raw probe, because "could not tell" arrives as
    words rather than as an absence -- and it arrives as words that *object*,
    so the clone is kept for the same reason unpushed work keeps one.

    Total over the arms, with :func:`workspace_state.unhandled_unsaved` behind
    it: a fourth answer stops the build rather than falling through into a
    deletion. The clause reads after "holds", so ``--prune``'s report builds its
    own sentence around it -- the same clause ``dl <ws> rm``'s refusal renders
    from the arms directly.
    """
    if isinstance(unsaved, workspace_state.NothingToLose):
        return None
    if isinstance(unsaved, workspace_state.WouldLose):
        return unsaved.description
    if isinstance(unsaved, workspace_state.CouldNotTell):
        return f"work git could not be asked about ({unsaved.reason})"
    workspace_state.unhandled_unsaved(unsaved)


def decide(status: CloneStatus, force: bool) -> Decision:
    """What `--prune` does about one clone directory. The only such place.

    Total over the arms, and deliberately the single point at which anything
    becomes deletable: there is no boolean beside a status that a later caller
    could read without having answered which arm it is, and no path from a
    directory devlaunch could not classify to one it would remove. A fourth arm
    added to CloneStatus stops this build rather than defaulting to a deletion.

    `--force` promotes exactly one arm. It is not a general override:
    Referenced and Disputed are not "refusals to be insisted past", they are
    devlaunch saying the directory is still in use or that its own records
    disagree, and there is nothing for a user to mean by insisting.
    """
    if isinstance(status, Referenced):
        return Keep(f"workspace {status.workspace_id} still opens it")
    if isinstance(status, Orphaned):
        objection = _objection(status.unsaved)
        if objection is None:
            return Remove(status.usage, Unopposed())
        if force:
            return Remove(status.usage, Insisted(f"holds {objection}"))
        return Keep(f"holds {objection} -- add --force to remove it anyway")
    if isinstance(status, Disputed):
        return Keep(
            f"devpod lists workspace {status.workspace_id} and sources it at "
            f"{status.sourced_at}; see devlaunch#88"
        )
    _unhandled_status(status)


@dataclass(frozen=True)
class Reclaimable:
    """One clone directory this run will remove, what it frees, and why it may.

    *promotion* is how the second pass knows whether `--force` was answering
    *this* directory. Without it the flag is plan-wide and turns off the
    re-probe for every directory in the plan.
    """

    path: pathlib.Path
    owner: str
    repo: str
    usage: disk_usage.DiskUsage
    promotion: Promotion


@dataclass(frozen=True)
class Kept:
    """One clone directory this run will leave standing, and why."""

    path: pathlib.Path
    because: str


@dataclass(frozen=True)
class PrunePlan:
    """Everything one `dl --prune` will do, settled before anything is asked.

    A value, for the reason WorkspaceOwnership is one: the report a user answers
    and the set that actually dies must come from the same object, and here the
    difference between them is somebody's directory. The two tuples are built by
    one pass over one `decide` call each, so a directory cannot be in both and
    cannot be in neither.

    There is deliberately no `force` here. It was a field, and a plan-wide
    boolean is exactly the shape `decide` refuses to have beside a status: the
    pass that acts read it and skipped its safety re-check for every directory,
    including the ones `--force` had promoted nothing about. What `--force`
    answered rides on each `Reclaimable` instead.
    """

    root: pathlib.Path
    removing: Tuple[Reclaimable, ...]
    keeping: Tuple[Kept, ...]
    stale_records: Tuple[WorktreeInfo, ...]

    @property
    def nothing_to_do(self) -> bool:
        """Whether this run would change nothing at all."""
        return not self.removing and not self.stale_records


def _canonical(path: str) -> Optional[pathlib.Path]:
    """*path* with every symlink resolved, or None if it could not be followed.

    None means "cannot tell", never "somewhere else": every caller here is
    deciding whether a directory is referenced, and answering that from a lookup
    that failed is how a live clone becomes an orphan.

    A path that is not *there* is not a failure -- resolving canonicalises as
    much of it as exists and leaves the rest, which is the right answer for a
    workspace whose source has been deleted, and there are hosts where that is
    most of them (devlaunch#88). What lands here is a path that could not be
    read as one at all: text devpod's JSON carried that no filesystem call will
    accept (ValueError), and, on Python 3.10 only, a symlink loop, which later
    versions resolve as far as they can instead of raising. All three are
    caught, because the package supports both versions and the difference
    between them must not decide what gets deleted.
    """
    try:
        return pathlib.Path(path).resolve()
    except (OSError, RuntimeError, ValueError):
        return None


@dataclass(frozen=True)
class Placeable:
    """Every place on this machine a source could name -- possibly none.

    Empty is a real answer and not a shrug: an image or container workspace
    opens no directory on this disk, so there is nothing to compare and no clone
    it could be holding.
    """

    paths: Tuple[str, ...]


@dataclass(frozen=True)
class Unplaceable:
    """The source opens a folder here and devlaunch cannot say which one."""

    detail: str


SourcePlaces = Placeable | Unplaceable


def source_places(source: WorkspaceSource) -> SourcePlaces:
    """Where on this machine *source* could be. Total over the arms.

    A `gitRepository` counts, even though devlaunch only ever hands devpod a
    local path, and the reason is which way the mistake runs. `devpod up
    <path-to-a-repo>` records that arm with a path in it, and a path this
    function does not return is a directory `--prune` will call unreferenced.
    is_devlaunch_clone refuses the same arm on purpose -- but refusing there
    means declining to delete somebody else's *workspace*, which is the opposite
    direction, so its answer must not be reused here. That is what the review of
    the disk-size surface meant by "not as-is": the predicate is right for
    reporting and for purging, and this question is neither.

    The two answers that carry no path are kept apart, because reading them
    alike is how a live workspace contributed no path *and* no alarm while the
    command printed that it stops for exactly that. An image or container
    workspace is `Placeable(())` -- nothing here, nothing at risk. A `localFolder`
    devpod filled with something unreadable is `Unplaceable`, and it stops the
    command the same way a source that will not resolve does.
    """
    if isinstance(source, LocalFolder):
        return Placeable((source.path,))
    if isinstance(source, GitRepository):
        return Placeable((source.url,))
    if isinstance(source, UnrecognisedSource):
        return Placeable(())
    if isinstance(source, UnreadableLocalFolder):
        return Unplaceable(json.dumps(source.payload))
    _unhandled_source(source)


@dataclass(frozen=True)
class Misplaced:
    """A live workspace devpod records inside a repository's clone tree, at
    something that is not a clone.

    devlaunch#88's measured shape, and the reason it needs a name of its own.
    On that ticket's host 36 of 39 devpod records named a folder that was gone
    (35) or a config-only stub devpod itself wrote from cache (1), while the
    real checkout sat beside it under the new id scheme. The two records cannot
    be joined by workspace id -- the id is exactly what changed -- so the join
    is made from the path instead: devpod points into `<root>/<owner>/<repo>/`
    at a directory that holds no `.git`, and which of that repository's clones
    the workspace actually needs is unanswerable.
    """

    workspace_id: str
    sourced_at: str


@dataclass(frozen=True)
class WorkspaceLocations:
    """Where devpod's workspaces are on this disk, and which ones are unknown.

    *unlocatable* is not an empty result with a note on it. A live workspace
    whose source cannot be followed is a directory that might be *any* of the
    candidates, so while one exists there is no honest answer to "is this clone
    referenced" -- and `--prune` says so instead of guessing.

    *misplaced* is the same refusal made narrow. A workspace devpod records at a
    non-clone *inside one repository's clone tree* can only be confused with
    that repository's clones, so it disputes those and leaves every other
    repository prunable -- which is what keeps this command usable on the host
    devlaunch#88 describes rather than merely safe on it.
    """

    by_path: Mapping[pathlib.Path, str]
    unlocatable: Tuple[str, ...]
    misplaced: Mapping[Tuple[str, str], Misplaced]

    def holder(self, candidate: pathlib.Path) -> Optional[str]:
        """The live workspace *candidate* holds the checkout for, if any.

        At **or under**, not equal to, and the direction matters in the only way
        this command's mistakes matter. `devpod up <clone>/subproject` records
        the subdirectory, and a clone whose subdirectory a live workspace opens
        is a clone that live workspace needs -- deleting it takes the workspace
        with it. Equality answered no and deleted the parent.

        The containment is between two canonical paths, which is what keeps it
        from being the lexical prefix test the reporting surface uses:
        `<clone>-scratch` is not under `<clone>`, and a symlinked source has
        already been resolved before it gets here.
        """
        workspace_id = self.by_path.get(candidate)
        if workspace_id is not None:
            return workspace_id
        for source, held_by in self.by_path.items():
            if candidate in source.parents:
                return held_by
        return None


@dataclass(frozen=True)
class Outside:
    """The source is not in devlaunch's clone tree, so no clone answers for it."""


@dataclass(frozen=True)
class InAClone:
    """The source is at or under *clone*, a directory that holds a checkout."""

    clone: pathlib.Path


@dataclass(frozen=True)
class InARepositoryOnly:
    """The source is in *(owner, repo)*'s clone tree but at no clone of it."""

    owner: str
    repo: str


@dataclass(frozen=True)
class TooShallow:
    """The source is in the clone tree above any repository, so it names none."""


SourceSite = Outside | InAClone | InARepositoryOnly | TooShallow


def _site_of(source: pathlib.Path, root: pathlib.Path) -> SourceSite:
    """Where a resolved source sits with respect to devlaunch's clone tree.

    Read off the path rather than derived from an id, because on devlaunch#88's
    host the id is what went wrong and the path is what survived: devpod's stale
    record still says `<root>/blooop/devlaunch/<old-leaf>`, which names the
    repository exactly even though the leaf and the workspace id match nothing
    any more.

    The clone is the *third* component under the root and the source may be
    deeper -- `devpod up <clone>/subproject` is a live workspace whose source is
    inside a clone, and the clone is what answers for it.
    """
    try:
        parts = source.relative_to(root).parts
    except ValueError:
        return Outside()
    if len(parts) < 3:
        return TooShallow() if len(parts) < 2 else InARepositoryOnly(parts[0], parts[1])
    clone = root / parts[0] / parts[1] / parts[2]
    if _is_populated_clone(clone):
        return InAClone(clone)
    return InARepositoryOnly(parts[0], parts[1])


def workspace_locations(workspaces: Sequence[Workspace], root: pathlib.Path) -> WorkspaceLocations:
    """Resolve every live workspace's source to a real directory on this disk.

    Both sides of the comparison this feeds are canonical, and that is the whole
    point rather than tidiness. A cache reached through a symlink -- somebody
    moved theirs, or `/tmp` is a link on their machine -- makes a lexical
    comparison say that *no* clone is referenced, which is a total-loss bug in
    the one direction that cannot be undone. The candidates are canonical by
    construction (see prune_plan); this canonicalises the other side.

    Three ways a workspace fails to place itself, and they are not one thing:
    a source devlaunch cannot read at all, and a source that named a folder no
    filesystem call will accept, both mean the workspace could be opening *any*
    candidate and stop the command; a source that lands inside a repository's
    clone tree on something with no `.git` in it means the workspace could be
    opening any of *that repository's* clones, and disputes only those.
    """
    by_path: Dict[pathlib.Path, str] = {}
    unlocatable: List[str] = []
    misplaced: Dict[Tuple[str, str], Misplaced] = {}
    for workspace in workspaces:
        places = source_places(workspace.source)
        if isinstance(places, Unplaceable):
            unlocatable.append(f"{workspace.id}: {places.detail}")
            continue
        for source in places.paths:
            resolved = _canonical(source)
            if resolved is None:
                unlocatable.append(f"{workspace.id}: {source}")
                continue
            site = _site_of(resolved, root)
            if isinstance(site, (Outside, InAClone)):
                by_path[resolved] = workspace.id
            elif isinstance(site, InARepositoryOnly):
                misplaced[(site.owner, site.repo)] = Misplaced(workspace.id, str(resolved))
            elif isinstance(site, TooShallow):
                unlocatable.append(f"{workspace.id}: {source}")
            else:
                _unhandled_site(site)
    return WorkspaceLocations(by_path=by_path, unlocatable=tuple(unlocatable), misplaced=misplaced)


def _unhandled_site(site: NoReturn) -> NoReturn:
    """Reject a source site nobody handled -- at type-check time, not at runtime."""
    raise AssertionError(f"Unhandled source site: {site!r}")


def _is_populated_clone(path: pathlib.Path) -> bool:
    """Whether *path* is a checkout rather than a place one used to be.

    The same question `workspace_exists` asks, and devlaunch#88's own published
    diagnostic (`[ -d "$p/.git" ] || echo BROKEN`). It is what separates a
    devpod record that still describes something from one the id-scheme change
    left behind -- a folder that is gone, or the config-only stub devpod
    reconstitutes from its cache, neither of which any clone can be matched to.

    `os.stat` rather than `Path.exists()`, for the reason
    :func:`devlaunch.worktree.workspace_clone._on_disk` gives at length:
    `exists()` swallows ENOENT, ENOTDIR, EBADF and ELOOP and re-raises the rest
    on Python 3.10-3.13 while 3.14 returns False, so an expression that looks
    like one question is two behaviours across the versions in the `ci` matrix.

    A door this process cannot open reads as **not** a populated clone, and that
    is the safe direction rather than the tidy one. Answering "yes" would say
    devpod's workspace is at *this* clone and nowhere else, which leaves the
    repository's other clones prunable; answering "no" says which clone of the
    repository the workspace wants cannot be established, which disputes all of
    them and keeps them. The second is what a refusal has actually established.
    """
    try:
        os.stat(path / ".git")
    except (OSError, ValueError):
        return False
    return True


def _subdirectories(path: pathlib.Path) -> List[pathlib.Path]:
    """The real directories directly under *path*, sorted, symlinks not followed.

    A symlinked entry is skipped rather than followed. Following one would put a
    candidate outside the cache entirely, and unlinking the link instead would
    report a clone as reclaimed while it sat on another volume -- the same two
    wrong answers remove_tree already refuses for a symlinked root. Skipping is
    that refusal, one step earlier, and it is also what keeps every candidate's
    path canonical without a resolve() that could fail.

    A directory that cannot be listed yields nothing: there is no such thing as
    a clone this process can delete but not see, so the safe reading of a closed
    door is that there is nothing behind it to remove.
    """
    try:
        with os.scandir(path) as entries:
            found = [entry for entry in entries if entry.is_dir(follow_symlinks=False)]
    except OSError:
        return []
    return sorted(pathlib.Path(entry.path) for entry in found)


def _clone_status(
    clone: pathlib.Path,
    owner: str,
    repo: str,
    locations: WorkspaceLocations,
    record_for: Mapping[pathlib.Path, WorktreeInfo],
    listed_at: Mapping[str, str],
) -> CloneStatus:
    """Which arm *clone* is, asked in the order that fails towards keeping it.

    devpod's own listing is consulted first, and by containment rather than
    containment-in-the-cache: the question is whether any live workspace's
    source is at or under *this* directory, which the lexical predicate the
    reporting surface uses cannot answer at all.

    Then the two ways devpod's records and devlaunch's can disagree, and both
    are devlaunch#88's shape:

    - devpod has a live workspace somewhere in *this repository's* clone tree
      that is not a clone -- a folder that is gone, or the config-only stub
      devpod rebuilds from cache. 36 of 39 workspaces on #88's host. Which of
      this repository's clones it needs cannot be answered, so none of them is
      unreferenced.
    - this directory's own record names a workspace devpod still lists and
      sources elsewhere. The narrower shape, and the one that survives when the
      ids still line up.

    A record naming a workspace devpod has forgotten is not a disagreement, it
    is the ordinary stale clone this command exists for -- 34 of the reference
    host's 37.

    The unsaved probe and the disk walk run last and only on the arm that could
    be removed. Together they are the expensive half of a scan (593 ms of git
    over 37 clones on the reference host, plus a walk with no ceiling), and
    asking them about a directory no answer could affect is time spent to learn
    nothing.
    """
    workspace_id = locations.holder(clone)
    if workspace_id is not None:
        return Referenced(workspace_id)
    misplaced = locations.misplaced.get((owner, repo))
    if misplaced is not None:
        return Disputed(misplaced.workspace_id, misplaced.sourced_at)
    record = record_for.get(clone)
    if record is not None:
        elsewhere = listed_at.get(record.workspace_id)
        if elsewhere is not None:
            return Disputed(record.workspace_id, elsewhere)
    return Orphaned(
        workspace_state.holds_unsaved_work(clone),
        disk_usage.exclusive_usage(clone),
    )


def _records_by_directory(
    clone_mgr: WorkspaceCloneManager,
) -> Dict[pathlib.Path, WorktreeInfo]:
    """metadata.json's worktree records, keyed by the directory each names.

    Which directory a record names is :meth:`resolve_clone_path`'s question and
    not this function's, and asking it here instead was the shape devlaunch#174
    was: `local_path` read raw is one of *two* answers a record can give, and the
    other one is what the delete acts on. The consequence here is the dangerous
    direction rather than the merely inconsistent one -- a record that missed
    its clone leaves that clone with no record at all, which drops it out of
    :class:`Disputed` and into :class:`Orphaned`, which is a deletion.

    An empty recorded path is the case that makes this concrete: `Path("")` is
    `Path(".")`, which is truthy and which exists, so it canonicalised to
    whatever directory dl happened to be run from and the record was filed under
    that. :meth:`resolve_clone_path` refuses anything not absolute and derives
    the real directory from the record's own owner/repo/branch instead.

    A record dl cannot name a directory for at all is left out. It cannot be
    matched to a candidate by definition, and there is no path it could be filed
    under that would not be a guess.
    """
    records: Dict[pathlib.Path, WorktreeInfo] = {}
    for record in clone_mgr.storage.list_worktrees():
        directory = clone_mgr.resolve_clone_path(record)
        if directory is None:
            continue
        resolved = _canonical(str(directory))
        if resolved is not None:
            records[resolved] = record
    return records


def _repo_lock(clone_mgr: WorkspaceCloneManager, owner: str, repo: str):
    """The lock a launch of *owner/repo* holds while it fills a clone.

    `--prune` takes the same one, and for the same span it looks at and removes
    that repository's clones: workspace_clone populates a clone fully before it
    returns, so without this a scan can weigh -- or delete -- a directory that
    `git clone` is still writing into.

    It closes that window and not a wider one, and the difference is worth being
    plain about: devpod only learns about a clone *after* the lock is released,
    so a clone whose launch has finished cloning and not yet registered a
    workspace is briefly indistinguishable from a stale one. Closing that would
    take a record devlaunch does not keep (devlaunch#88's ticket is where the
    id it would need is meant to start being written down).
    """
    return hold_lock(
        clone_mgr.repo_manager.lock_path(owner, repo),
        waiting_note=f"another dl run preparing {owner}/{repo}",
    )


def clone_root(clone_mgr: WorkspaceCloneManager) -> pathlib.Path:
    """The directory `--prune` scans, canonicalised once.

    `repos_dir` as the clone manager reports it. Taking it from there rather
    than rebuilding `<cache>/repos` is what keeps the directories scanned, the
    locks taken and the workspace sources compared answering to the same
    configuration: a `config.toml` that moves `repos_dir` moves all three, and
    they cannot drift into scanning one tree while serialising against another
    or comparing against a third.

    Resolved directly rather than through _canonical: a repos_dir this could
    fail on is one RepositoryManager's own mkdir has already refused, so a clone
    manager existing at all is evidence the path is usable. Absent is not a
    failure -- a fresh install has no repos directory yet and resolving one that
    is not there is what says so.
    """
    return pathlib.Path(clone_mgr.repo_manager.repos_dir).resolve()


def prune_plan(
    clone_mgr: WorkspaceCloneManager,
    workspaces: Sequence[Workspace],
    locations: WorkspaceLocations,
    root: pathlib.Path,
    force: bool,
) -> PrunePlan:
    """Classify every clone directory under the cache, one repository at a time.

    Every candidate path is canonical without ever being resolved individually
    -- a resolved root (see :func:`clone_root`) plus real directory names,
    symlinks skipped.
    """
    removing: List[Reclaimable] = []
    keeping: List[Kept] = []
    record_for = _records_by_directory(clone_mgr)
    listed_at = {ws.id: describe_source(ws.source)[1] for ws in workspaces}
    for owner_dir in _subdirectories(root):
        for repo_dir in _subdirectories(owner_dir):
            owner, repo = owner_dir.name, repo_dir.name
            bare = _canonical(str(clone_mgr.repo_manager.get_bare_path(owner, repo)))
            with _repo_lock(clone_mgr, owner, repo):
                for clone in _subdirectories(repo_dir):
                    if clone == bare:
                        # Never a candidate and never reported. Nothing sources
                        # it and no record names it, so every rule above would
                        # call it an orphan -- and it is the copy every clone of
                        # this repo hardlinks its git objects out of, 0.08 GB
                        # for all seven repos on the reference host, and the
                        # reason the next clone is fast.
                        continue
                    status = _clone_status(clone, owner, repo, locations, record_for, listed_at)
                    decision = decide(status, force)
                    if isinstance(decision, Remove):
                        removing.append(
                            Reclaimable(clone, owner, repo, decision.usage, decision.promotion)
                        )
                    else:
                        keeping.append(Kept(clone, decision.because))
    return PrunePlan(
        root=root,
        # Biggest first: the report's job is to be acted on, and "which of these
        # is worth reclaiming" is the comparative question known_bytes exists
        # for. Path breaks ties so two runs over an unchanged cache read alike.
        removing=tuple(sorted(removing, key=lambda r: (-disk_usage.known_bytes(r.usage), r.path))),
        keeping=tuple(keeping),
        stale_records=_records_for_absent_directories(clone_mgr),
    )


def _records_for_absent_directories(
    clone_mgr: WorkspaceCloneManager,
) -> Tuple[WorktreeInfo, ...]:
    """The worktree records whose directory is definitively not there any more.

    metadata.json is append-mostly and nothing has ever pruned it: 49 records
    for 17 live workspaces on the reference host. These are the ones that
    describe nothing at all.

    "Definitively" is _present's distinction and it is load-bearing here too: a
    directory this process is not allowed to look at is still a directory, and
    dropping its record would lose the only note of where a clone lives.

    Which directory the record describes is asked of
    :meth:`resolve_clone_path`, the same resolver the classification, the
    listing and the delete all go through (devlaunch#174). Reading `local_path`
    raw asks about a directory that may not be the one the clone is in: a record
    written before the current id scheme, or one whose recorded path is empty --
    `Path("")` is `Path(".")`, present by construction -- gets an answer about
    somewhere else, and a record dropped for a clone that is still there is the
    only note of where that clone lives, gone.

    A record dl cannot name a directory for is not dropped. "dl could not work
    out where this is" is not "this is not there", and only the second is a
    reason to forget it.
    """
    stale: List[WorktreeInfo] = []
    for record in clone_mgr.storage.list_worktrees():
        directory = clone_mgr.resolve_clone_path(record)
        if directory is not None and not _present(directory):
            stale.append(record)
    return tuple(stale)


def print_prune_plan(plan: PrunePlan) -> None:
    """Say what is going, what is staying and why, before anything is asked.

    Printed whether or not there is anything to do, and in that order, because
    the reason a directory is *staying* is the half a person cannot get anywhere
    else -- `dl --ls` lists workspaces, and a clone with no workspace has no row
    there to appear in.
    """
    print(f"Clone directories under {plan.root}:")
    print()
    if plan.removing:
        freed = disk_usage.describe_usage(disk_usage.total_usage(r.usage for r in plan.removing))
        print(f"Removing {len(plan.removing)} that nothing references -- {freed}:")
        for reclaimable in plan.removing:
            line = f"  - {reclaimable.path} ({disk_usage.describe_usage(reclaimable.usage)})"
            promotion = reclaimable.promotion
            if isinstance(promotion, Insisted):
                # What --force is answering, on the line of the directory it
                # answers for. Without it the plan reads the same for a clone
                # holding an afternoon's uncommitted work as for an empty one,
                # and the confirmation cannot say what it costs.
                line = f"{line} -- {promotion.despite}, removing anyway"
            elif not isinstance(promotion, Unopposed):
                _unhandled_promotion(promotion)
            print(line)
        print()
    if plan.keeping:
        print(f"Leaving {len(plan.keeping)}:")
        for kept in plan.keeping:
            print(f"  - {kept.path}: {kept.because}")
        print()
    if plan.stale_records:
        print(f"Dropping {len(plan.stale_records)} record(s) of directories already gone.")
        print()
    if plan.nothing_to_do:
        print("Nothing to prune.")


def report_unlocatable(locations: WorkspaceLocations) -> None:
    """Say which live workspaces could not be placed, and that nothing went.

    Not a warning above a report. A workspace whose source cannot be followed --
    text no filesystem call will accept, or a `localFolder` devpod filled with
    something that is not a path -- could be opening any of the candidates, so
    while one exists there is no directory this command can honestly call
    unreferenced. Printed by both passes, because both have to be able to stop.
    """
    print("dl --prune cannot follow these live workspaces' sources:")
    for source in locations.unlocatable:
        print(f"  - {source}")
    print()
    print("Nothing was removed: no clone is unreferenced while a workspace is unaccounted for.")


def prune_clones(clone_mgr: WorkspaceCloneManager, plan: PrunePlan, root: pathlib.Path) -> int:
    """Carry out *plan*: remove the directories, then forget them.

    **Every directory is classified again, under the lock, immediately before it
    goes**, and only what this pass *also* finds removable is removed. The report
    a user answered was taken before they answered it, and everything it rests on
    can have moved in between: a container writes into a clone, or a launch that
    was mid-clone when the plan was printed finishes and registers a workspace
    for one of these exact directories -- the clone path for `(owner, repo,
    branch)` is deterministic, so a concurrent launch reuses the very directory
    in the plan. Re-asking only "has it grown unsaved work" caught the first and
    not the second, and the difference was somebody's running workspace.

    That is why this pass pays a second `devpod list`. It is the one question
    whose answer cannot be re-derived from disk, it is O(1) rather than per
    workspace, and it is paid only after a user has said yes to a deletion.

    `--force` is re-applied per directory, from the promotion the plan recorded
    for that directory rather than from a flag over the whole run, so insisting
    past one clone's unsaved work does not turn the re-probe off for the others.
    Referenced and Disputed are not promotable on either pass, so a directory
    that became either since the plan is kept whatever was typed.

    The approved set can therefore shrink between the report and the act, and can
    never grow -- the direction that costs a command rather than a morning's work.

    Classifying again means walking again, since the disk figure lives inside
    the arm that could be removed. That is one extra walk of each directory this
    run is about to delete, and none of the others -- and the bytes reported
    stay the plan's, so what a person is told they got back is what they said
    yes to.
    """
    removed: List[Reclaimable] = []
    refused: List[Refusal] = []
    withheld: List[Tuple[pathlib.Path, str]] = []
    workspaces = list_workspaces(refresh=True)
    locations = workspace_locations(workspaces, root)
    if locations.unlocatable:
        report_unlocatable(locations)
        return 1
    listed_at = {ws.id: describe_source(ws.source)[1] for ws in workspaces}
    record_for = _records_by_directory(clone_mgr)
    by_repo: Dict[Tuple[str, str], List[Reclaimable]] = {}
    for reclaimable in plan.removing:
        by_repo.setdefault((reclaimable.owner, reclaimable.repo), []).append(reclaimable)
    for (owner, repo), reclaimables in sorted(by_repo.items()):
        with _repo_lock(clone_mgr, owner, repo):
            for reclaimable in reclaimables:
                decision = decide(
                    _clone_status(reclaimable.path, owner, repo, locations, record_for, listed_at),
                    isinstance(reclaimable.promotion, Insisted),
                )
                if isinstance(decision, Keep):
                    withheld.append((reclaimable.path, decision.because))
                    continue
                refusals = remove_tree(reclaimable.path)
                if refusals:
                    refused.extend(refusals)
                    continue
                removed.append(reclaimable)
                _forget_clone(clone_mgr, record_for.get(reclaimable.path))
    for record in plan.stale_records:
        _forget_clone(clone_mgr, record)
    freed = disk_usage.describe_usage(disk_usage.total_usage(r.usage for r in removed))
    print(f"Removed {len(removed)} clone director(ies) -- {freed}.")
    for path, because in withheld:
        print(f"Left {path}: {because}. That was not so when the plan above was printed.")
    if not refused:
        return 0
    # Not 0: a directory the user was told would go is still on disk. The
    # clones that did go are still gone, which is why this is a report and not
    # an abort.
    report_refusals(
        refused,
        "Some directories would not come away. These refused:",
        tuple(refusal.path for refusal in refused),
    )
    return 1


def _forget_clone(clone_mgr: WorkspaceCloneManager, record: Optional[WorktreeInfo]) -> None:
    """Drop one worktree record, if there was one to drop.

    Removing a clone without this is what left metadata.json describing
    workspaces that stopped existing years ago; a record kept for a directory
    that is gone is not a safety margin, it is the thing that made the file
    unreadable as a description of anything.
    """
    if record is None:
        return
    try:
        clone_mgr.storage.remove_worktree(record.owner, record.repo, record.branch)
    except OSError as e:
        logging.warning(f"Could not drop the record for {record.local_path}: {e}")


_PRUNE_FLAGS = ("-y", "--yes", "--force")


def prune_command(flags: Sequence[str]) -> int:
    """`dl --prune`: report the clone directories nothing references, then act.

    Shaped like `--purge` on purpose -- print the plan, name what is left
    standing and why, confirm, `-y` to skip -- because a user who has run one
    has already learned this one. What it does is the opposite of `--purge`'s
    all-or-nothing: it removes only the clone directories no live workspace
    opens, leaves every bare cache alone, and never touches a devpod workspace,
    a container, an image or a volume.

    Nothing here runs on its own or on the way to anything else. A full scan
    measured 1017 ms on the reference host -- about two warm launches -- and it
    gets slower exactly as the problem it is for gets worse, so it is a command
    somebody runs, and `dl --prune` answered `n` is how you read the report.
    """
    unknown = [flag for flag in flags if flag not in _PRUNE_FLAGS]
    if unknown:
        # Refused rather than ignored. An ignored option fails safe here by
        # luck rather than by design -- but `dl --prune --dry-run -y` reads as a
        # rehearsal and would have been a deletion, and that is not a mistake to
        # find out about afterwards.
        logging.error(f"Unknown option(s) for --prune: {' '.join(unknown)}")
        return 1
    force = "--force" in flags
    clone_mgr = _get_clone_manager()
    root = clone_root(clone_mgr)
    workspaces = list_workspaces()
    locations = workspace_locations(workspaces, root)
    if locations.unlocatable:
        report_unlocatable(locations)
        return 1
    plan = prune_plan(clone_mgr, workspaces, locations, root, force)
    print_prune_plan(plan)
    if plan.nothing_to_do:
        return 0
    if not any(flag in ("-y", "--yes") for flag in flags):
        if input("Are you sure? [y/N] ").strip().lower() not in ("y", "yes"):
            print("Aborted.")
            return 0
    return prune_clones(clone_mgr, plan, root)


# Regex patterns for parsing git URLs
GIT_URL_PATTERNS = [
    # git@github.com:owner/repo.git
    re.compile(r"git@github\.com:([^/]+)/([^/]+?)(?:\.git)?$"),
    # https://github.com/owner/repo.git or https://github.com/owner/repo
    re.compile(r"https?://github\.com/([^/]+)/([^/]+?)(?:\.git)?$"),
    # github.com/owner/repo
    re.compile(r"^github\.com/([^/]+)/([^/]+?)(?:\.git)?$"),
]


def parse_owner_repo_from_url(url: str) -> Optional[tuple]:
    """Extract (owner, repo) from a git URL."""
    for pattern in GIT_URL_PATTERNS:
        match = pattern.match(url)
        if match:
            return (match.group(1), match.group(2))
    return None


def get_git_remote_url(path: str) -> Optional[str]:
    """Get the origin remote URL from a git repository."""
    try:
        result = subprocess.run(
            ["git", "-C", path, "remote", "get-url", "origin"],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        pass
    return None


def _git_ls_remote(owner_repo: str, *args: str) -> Optional[str]:
    """Run git ls-remote and return stdout, or None on error.

    Uses SSH URL for consistency with other git operations.
    Includes timeout to prevent hanging on slow/unreachable remotes.
    """
    url = f"git@github.com:{owner_repo}.git"
    try:
        with timing.span("git ls-remote"):
            result = subprocess.run(
                ["git", "ls-remote", url, *args],
                capture_output=True,
                text=True,
                check=False,
                timeout=5,
            )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout
    except (OSError, subprocess.SubprocessError, subprocess.TimeoutExpired):
        pass
    return None


def remote_branch_exists(owner_repo: str, branch: str) -> bool:
    """Check if a branch exists on a remote GitHub repository."""
    output = _git_ls_remote(owner_repo, "--heads", branch)
    return bool(output)


def discover_repos_from_workspaces(workspaces: List[Workspace]) -> Dict[str, List[str]]:
    """Discover owner/repo from workspace git remotes.

    Returns dict mapping owner -> list of repos.

    Every source is answered, including the ones devlaunch cannot read. There
    is no owner/repo to be had from an image reference, so the answer for that
    arm is still "nothing discovered" -- but it is said rather than reached by
    falling off the end of the chain, which is what used to make it
    indistinguishable from a source that was read fine and held no repo.
    """
    repos: Dict[str, List[str]] = {}

    for ws in workspaces:
        owner_repo = None
        source = ws.source

        # For git workspaces, parse the source URL directly
        if isinstance(source, GitRepository):
            owner_repo = parse_owner_repo_from_url(source.url)

        # For local workspaces, try to get git remote
        elif isinstance(source, LocalFolder):
            remote_url = get_git_remote_url(source.path)
            if remote_url:
                owner_repo = parse_owner_repo_from_url(remote_url)

        elif isinstance(source, (UnrecognisedSource, UnreadableLocalFolder)):
            logging.warning(
                f"Not looking for a repo in workspace '{ws.id}': "
                f"devpod describes its source as {json.dumps(source.payload)}, "
                "which devlaunch cannot read."
            )

        else:
            _unhandled_source(source)

        if owner_repo:
            owner, repo = owner_repo
            if owner not in repos:
                repos[owner] = []
            if repo not in repos[owner]:
                repos[owner].append(repo)

    return repos


def discover_repos_from_cache_dir() -> Dict[str, List[str]]:
    """Discover owner/repo from bare repos in the local cache directory.

    Scans <repos_dir>/<owner>/<repo>/.bare/ on the filesystem so that repos
    cloned locally (even without a devpod workspace yet) are discovered.

    Returns dict mapping owner -> list of repos (same shape as
    ``discover_repos_from_workspaces``).
    """
    repos: Dict[str, List[str]] = {}
    repos_dir = pathlib.Path(get_worktree_config().repos_dir)
    if not repos_dir.is_dir():
        return repos
    try:
        for owner_dir in repos_dir.iterdir():
            if not owner_dir.is_dir():
                continue
            for repo_dir in owner_dir.iterdir():
                if not repo_dir.is_dir():
                    continue
                if (repo_dir / ".bare").is_dir():
                    owner = owner_dir.name
                    repo = repo_dir.name
                    if owner not in repos:
                        repos[owner] = []
                    if repo not in repos[owner]:
                        repos[owner].append(repo)
    except OSError:
        pass
    return repos


def get_known_repos() -> List[str]:
    """Get list of known owner/repo strings from workspaces."""
    workspaces = list_workspaces()
    repos = discover_repos_from_workspaces(workspaces)
    result = []
    for owner, repo_list in sorted(repos.items()):
        for repo in sorted(repo_list):
            result.append(f"{owner}/{repo}")
    return result


def run_devpod(
    args: List[str],
    capture: bool = False,
    env: Optional[Dict[str, str]] = None,
    stdin_file=None,
) -> subprocess.CompletedProcess:
    """Run a devpod command.

    env replaces devpod's whole environment when given, so a caller that wants
    to add one variable must build it from os.environ. It exists so a secret can
    be handed to devpod without putting it in argv, where ps would expose it.

    stdin_file, when given, becomes the command's stdin -- how tools.py streams
    the host's binaries into a container over `devpod ssh`, the one channel dl
    already holds into every workspace. A file rather than bytes, because the
    payload runs to hundreds of megabytes that have no business in memory.

    Security note: Using list form of subprocess.run (not shell=True) prevents
    command injection. Each list element is passed as a separate argument to
    the executable, so special characters are not interpreted by a shell.

    This is dl's only devpod spawn, so it is also the only place that can tell
    "devpod is not installed" from "devpod ran and failed". The former is
    raised as DevpodNotInstalled rather than folded into a returncode: callers
    branch on returncode and would carry on as though devpod had answered.
    """
    cmd = ["devpod"] + args
    logging.debug("Running: %s", " ".join(cmd))
    try:
        # Timed by subcommand, not full argv: the summary should name each
        # round trip (status, ssh, up...) without leaking workspace ids into it.
        with timing.span(" ".join(cmd[:2])):
            if capture:
                # nosec B603 - using list form, not shell=True; no command injection risk
                return subprocess.run(
                    cmd, capture_output=True, text=True, check=False, env=env, stdin=stdin_file
                )
            # nosec B603 - using list form, not shell=True; no command injection risk
            return subprocess.run(cmd, check=False, env=env, stdin=stdin_file)
    except FileNotFoundError as e:
        raise DevpodNotInstalled(DEVPOD_MISSING_MESSAGE) from e


def run_devpod_session(
    args: List[str], env: Optional[Dict[str, str]] = None
) -> devpod_ssh.SshOutcome:
    """Run a devpod command that hands its stdin/stdout to a terminal session.

    stdin and stdout are inherited untouched — devpod puts the real terminal into
    raw mode through them, and requests a pty on that basis. Only stderr is read,
    which under a pty carries devpod's own warnings and errors and nothing else,
    so that devpod's report of how the session ended can be interpreted rather
    than dumped on the user. See devpod_ssh for why that is necessary.
    """
    cmd = ["devpod"] + args
    logging.debug("Running: %s", " ".join(cmd))
    # The span covers the whole session: what the summary names is the round
    # trip the user waited on, not just the process spawn.
    with timing.span(" ".join(cmd[:2])):
        # nosec B603 - using list form, not shell=True; no command injection risk
        with subprocess.Popen(
            cmd,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=env,
        ) as proc:
            # proc.stderr is a pipe because PIPE was asked for, but Popen's type
            # cannot express that, so the narrowing happens here rather than by
            # widening filter_devpod_stderr to a None it would have no answer for.
            pipe = proc.stderr
            remote_status = (
                devpod_ssh.filter_devpod_stderr(pipe, sys.stderr) if pipe is not None else None
            )
    return devpod_ssh.interpret(proc.returncode, remote_status)


# The memoized `devpod list` snapshot. A dict rather than a module-level
# Optional so the accessors below need no `global`, and so "nothing read yet"
# (no key at all) stays distinguishable from "devpod has no workspaces" (an
# empty list) — the two must not be confused, or a real empty answer would be
# re-read on every call.
_WORKSPACE_LIST_KEY = "workspaces"
_workspace_list_cache: Dict[str, List[Workspace]] = {}


def invalidate_workspace_list_cache() -> None:
    """Forget the memoized `devpod list` snapshot.

    Every dl code path that changes what devpod would list calls this, so a
    later read in the same process re-reads devpod instead of answering from a
    snapshot taken before the change.
    """
    _workspace_list_cache.pop(_WORKSPACE_LIST_KEY, None)


def parse_workspaces(listing: str) -> List[Workspace]:
    """The workspaces in a `devpod list --output json` listing.

    Anything that is not such a listing raises rather than parsing to nothing.
    That includes the empty string: devpod prints `[]` for a machine with no
    workspaces, so silence is devpod failing to answer, not devpod answering
    that there is nothing to list. Silence gets a branch of its own rather than
    falling into the JSON parser, whose report of it -- `not JSON: ''` -- reads
    like a bug in dl rather than a devpod that never spoke.

    A `source` that is not an object is refused here for the same reason. The
    source arms below are total over the object devpod documents, and the arm
    for a source dl cannot read holds that object -- so something that is not
    one is not an unreadable source, it is an unreadable *listing*, and that is
    the answer this function already knows how to give.
    """
    if not listing.strip():
        raise UnreadableWorkspaceList(
            "devpod said nothing when asked to list workspaces; it prints `[]` when there are none"
        )
    try:
        parsed = json.loads(listing)
    except json.JSONDecodeError as exc:
        raise UnreadableWorkspaceList(
            f"devpod's workspace listing is not JSON: {listing[:120]!r}"
        ) from exc
    if not isinstance(parsed, list):
        raise UnreadableWorkspaceList(
            f"expected devpod to list workspaces, got {type(parsed).__name__}"
        )
    for entry in parsed:
        if not isinstance(entry, dict):
            raise UnreadableWorkspaceList(
                f"expected each listed workspace to be an object, got {type(entry).__name__}"
            )
        source = entry.get("source", {})
        if not isinstance(source, dict):
            raise UnreadableWorkspaceList(
                f"expected workspace {entry.get('id', '')!r} to have an object for its "
                f"source, got {type(source).__name__}"
            )
    return [Workspace.from_json(ws) for ws in parsed]


def list_workspaces(refresh: bool = False) -> List[Workspace]:
    """List all devpod workspaces, reading devpod at most once per command.

    `devpod list --output json` costs ~0.45s — five times the entire Python
    startup dl pays — and six call sites read it, so `dl --purge` used to spend
    ~0.9s asking the same question twice. dl is a short-lived single-command
    process, so one snapshot per command is enough; the only reader that could
    see a stale one is a reader that runs after dl itself changed a workspace,
    and every such mutation calls invalidate_workspace_list_cache().

    refresh=True bypasses the snapshot for a caller that must have the
    post-mutation truth even if nothing announced the mutation.

    Only an answer devpod actually gave is remembered, and only an answer devpod
    actually gave is returned: a read that failed or could not be parsed raises
    UnreadableWorkspaceList rather than answering with an empty list, so neither
    a transient failure nor a missing devpod can be served to a caller as "this
    machine has no workspaces".

    Which of the workspaces devpod lists belong to dl is a separate question,
    and not one this function answers. It answers only whether the list can be
    believed at all.
    """
    if not refresh:
        cached = _workspace_list_cache.get(_WORKSPACE_LIST_KEY)
        if cached is not None:
            # A copy: a caller that sorts or filters its list in place must not
            # be rewriting what the next caller sees.
            return list(cached)
    result = run_devpod(["list", "--output", "json"], capture=True)
    if result.returncode != 0:
        # !r for the same reason the parse path uses it: devpod's stderr is
        # routinely several lines, and DEVPOD_MISSING_MESSAGE's comment sets the
        # rule that one of dl's failure messages is one line.
        raise UnreadableWorkspaceList(
            f"`devpod list` exited {result.returncode}: {(result.stderr or '').strip()[:200]!r}"
        )
    workspaces = parse_workspaces(result.stdout or "")
    _workspace_list_cache[_WORKSPACE_LIST_KEY] = workspaces
    return list(workspaces)


def get_workspace_ids() -> List[str]:
    """Get list of workspace IDs for completion."""
    return [ws.id for ws in list_workspaces()]


def print_workspaces(with_size: bool = False):
    """Print workspace list in a nice format.

    *with_size* adds a SIZE column holding the bytes deleting that workspace's
    clone would free -- exclusive bytes, so the git objects it shares with the
    repo's bare cache are billed to nobody rather than to everybody. It is asked
    for rather than always answered because the walk is O(files) with no
    ceiling, while this listing is otherwise one devpod round-trip and no
    filesystem work at all -- and an ordinary devcontainer builds its
    environment inside the clone, so the file count has no ceiling either.
    README's "How much disk a workspace costs" carries the timings and the
    machine they were taken on; they are not repeated here, so re-measuring
    changes one place.
    """
    workspaces = list_workspaces()
    if not workspaces:
        print("No workspaces found.")
        return

    # Describe each source once, so the widths are measured on the same strings
    # that get printed -- sizes included, which is also why the walk happens
    # here and not again in the print loop. An unasked-for column is an empty
    # string per row rather than a cell value, because "there is no column" is
    # a fact about the table and "nothing was measured" is a fact about a
    # workspace; `sized` below drops the empties, and _size_cell never has to
    # mean both things at once.
    cache_dir = _get_cache_dir() if with_size else None
    sizes = (
        [""] * len(workspaces)
        if cache_dir is None
        else [_size_cell(ws, cache_dir) for ws in workspaces]
    )
    rows = [
        (ws, *describe_source(ws.source), size) for ws, size in zip(workspaces, sizes, strict=True)
    ]

    # Calculate column widths
    id_width = max(len(ws.id) for ws, _kind, _detail, _size in rows)
    type_width = max(len(kind) for _ws, kind, _detail, _size in rows)
    source_width = max(len(detail) for _ws, _kind, detail, _size in rows)
    # The heading counts as a cell, or a column of dashes leaves "SIZE" wider
    # than the column it heads and every LAST USED after it shifted right.
    size_width = max([len("SIZE"), *(len(size) for _ws, _kind, _detail, size in rows)])

    def sized(text: str) -> str:
        """*text* in the SIZE column, or nothing at all when it was not asked for."""
        return f"{text:>{size_width}}  " if with_size else ""

    # Print header
    print(
        f"{'WORKSPACE':<{id_width}}  {'TYPE':<{type_width}}  {'SOURCE':<{source_width}}  "
        f"{sized('SIZE')}LAST USED"
    )
    print("-" * (id_width + type_width + source_width + len(sized("SIZE")) + 30))

    # Print rows
    for ws, kind, detail, size in rows:
        last_used = ws.last_used[:19].replace("T", " ") if ws.last_used else "never"
        print(
            f"{ws.id:<{id_width}}  {kind:<{type_width}}  {detail:<{source_width}}  "
            f"{sized(size)}{last_used}"
        )


def _size_cell(workspace: Workspace, cache_dir: pathlib.Path) -> str:
    """What *workspace* costs on disk, for the table's SIZE column.

    A workspace devlaunch did not make reads as `-` rather than `0 B`: nothing
    was measured there, and a zero would say the opposite of that.
    """
    clone = _measurable_clone(workspace, cache_dir)
    if clone is None:
        return "-"
    return disk_usage.describe_usage(disk_usage.exclusive_usage(clone))


def fuzzy_select_workspace() -> Optional[str]:
    """Interactive fuzzy finder for workspace selection."""
    try:
        from iterfzf import iterfzf
    except ImportError:
        logging.error("iterfzf not available. Install with: pip install iterfzf")
        return None

    workspaces = list_workspaces()
    if not workspaces:
        logging.info("No workspaces found. Create one with: dl owner/repo or dl ./path")
        return None

    # Format options for display: "id | type | source"
    options = []
    ws_map = {}
    for ws in workspaces:
        kind, detail = describe_source(ws.source)
        label = f"{ws.id} | {kind} | {detail}"
        options.append(label)
        ws_map[label] = ws.id

    print("Select workspace (type to filter):")
    try:
        selected = iterfzf(options, multi=False)
    except KeyboardInterrupt:
        return None
    if selected:
        return ws_map.get(selected)
    return None


# How long a cached read of `devpod context options` stays current. The two
# options dl reads (DOTFILES_URL, DOTFILES_SCRIPT) change only when somebody
# runs `devpod context set-options`, and the read costs a whole devpod
# round trip in front of every `up`. An hour mirrors the completion cache's
# view of how stale devlaunch is willing to be; deleting the cache file is
# the escape hatch when a changed option is wanted now.
CONTEXT_OPTIONS_TTL_SECONDS = 3600


def _context_options_cache_path() -> pathlib.Path:
    # Resolved at call time, not from the module-level CACHE_DIR: the test
    # suite scopes XDG_CACHE_HOME per test, and this cache must follow it.
    return _get_cache_dir() / "context-options.json"


def _launch_lock_path(workspace_id: str) -> pathlib.Path:
    """The lock two `up`s of one workspace serialize on (see workspace_up).

    Its own directory rather than the repo cache: this lock is keyed by
    workspace, exists for workspaces that have no clone under the cache at
    all (paths, URLs), and must not look like a repo to the cache's walkers.
    Resolved at call time for the same reason as the context-options cache.
    """
    return _get_cache_dir() / "launch-locks" / f"{workspace_id}.lock"


def _devpod_config_path() -> pathlib.Path:
    """devpod's own config file, which holds every context and its options.

    Honours DEVPOD_HOME for the same reason the rest of dl does: it is what
    scopes devpod, and the test suite sets it.
    """
    home = os.environ.get("DEVPOD_HOME")
    root = pathlib.Path(home) if home else pathlib.Path.home() / ".devpod"
    return root / "config.yaml"


def get_context_options() -> dict:
    """The devpod context options, as {name: value}, cached on disk for an hour.

    Only an answer devpod actually gave is cached, so a failed or unreadable
    read costs nothing worse than the uncached behaviour: the empty dict.

    The TTL is not the only thing that expires it. These options are *per
    context*, and this is one cache file, so `devpod context use <other>`
    would otherwise feed the previous context's dotfiles settings to `devpod
    up` for up to an hour -- a wrong answer nobody could connect to a cache
    they did not know existed. Both switching context and setting an option
    rewrite devpod's config file, so a cache older than that file is stale
    whatever its age. One stat, and no round trip to find out.
    """
    cache_path = _context_options_cache_path()
    try:
        cached_at = cache_path.stat().st_mtime
        try:
            config_changed = _devpod_config_path().stat().st_mtime > cached_at
        except OSError:
            # No config file to disagree with; the TTL is the whole test.
            config_changed = False
        if not config_changed and time.time() - cached_at < CONTEXT_OPTIONS_TTL_SECONDS:
            with open(cache_path, encoding="utf-8") as f:
                cached = json.load(f)
            if isinstance(cached, dict):
                return cached
    except (OSError, json.JSONDecodeError):
        pass

    result = run_devpod(["context", "options", "--output", "json"], capture=True)
    if result.returncode != 0 or not result.stdout.strip():
        return {}
    try:
        data = json.loads(result.stdout)
        options = {k: v.get("value") for k, v in data.items() if v.get("value")}
    except (json.JSONDecodeError, AttributeError):
        return {}

    try:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        temp_path = cache_path.with_suffix(".tmp")
        with open(temp_path, "w", encoding="utf-8") as f:
            json.dump(options, f)
        temp_path.replace(cache_path)
    except OSError:
        pass
    return options


def workspace_up(
    workspace: str,
    ide: Optional[str] = None,
    recreate: bool = False,
    reset: bool = False,
    workspace_id: Optional[str] = None,
    workspace_identity: Optional[str] = None,
    devcontainer: Optional[str] = None,
):
    """Start or create a workspace.

    workspace_id is devpod's --id, passed only when creating. workspace_identity
    is the id the workspace is known by either way, injected into the workspace
    initialization environment as DEVLAUNCH_WORKSPACE_ID so a project's host-side
    initializeCommand can tell branch workspaces apart — devpod gives the hook no
    workspace identity of its own (see docs/devcontainer-projects.md).

    devcontainer is a devcontainer.json path from _resolve_devcontainer_ref.

    One `up` per workspace at a time, serialized over a per-workspace lock.
    Two dl processes can want the same workspace up at the same moment --
    wayfinder fires a background `dl <ws> up` the moment a launch is staged,
    and the human's second enter runs the launch itself seconds later -- and
    two concurrent `devpod up`s of one workspace is a race devpod does not
    promise to survive. The loser waits; and a loser that *had* to wait
    re-checks the state before doing anything, because the most likely reason
    for the wait is that the winner just brought this very workspace up. That
    re-check is one status round trip paid only on contention; the everyday
    uncontended `up` pays nothing but the flock itself.

    The skip does not apply when the call is there for a side effect the
    sibling cannot have had: an IDE to open, a recreate or reset to perform.
    """
    args = ["up", workspace]
    if workspace_id:
        args.extend(["--id", workspace_id])
    # Default to no IDE: dl attaches a terminal shell, so devpod's configured
    # default IDE (vscode) must not auto-open a window on every `dl <ws>`.
    # `dl <ws> code` passes ide="vscode" explicitly.
    args.extend(["--ide", ide if ide else "none"])
    if devcontainer:
        args.extend(["--devcontainer-path", devcontainer])
    identity = workspace_identity or workspace_id
    if identity:
        args.extend(["--init-env", f"DEVLAUNCH_WORKSPACE_ID={identity}"])
    if recreate:
        args.append("--recreate")
    if reset:
        args.append("--reset")
    ctx = get_context_options()
    if ctx.get("DOTFILES_URL"):
        args.extend(["--dotfiles", ctx["DOTFILES_URL"]])
    if ctx.get("DOTFILES_SCRIPT"):
        args.extend(["--dotfiles-script", ctx["DOTFILES_SCRIPT"]])
    with contextlib.ExitStack() as stack:
        # Two ways to end up unserialized, and neither is worth failing a
        # launch over. No identity means there is nothing to key a lock on,
        # and the caller shapes that reach here that way are not the
        # concurrent-launch ones. A lock file that cannot be created means an
        # unwritable cache directory -- a container writing as another uid is
        # a documented occurrence in this very cache (see purge_all_data), and
        # a full or read-only disk lands here too. Serialization guards
        # against a race that may not even be happening, so taking the whole
        # command down with an errno traceback in front of a `devpod up` that
        # would have worked is the worse answer.
        waited = False
        if identity:
            try:
                waited = stack.enter_context(
                    hold_lock(
                        _launch_lock_path(identity),
                        waiting_note=f"another launch of {identity}",
                    )
                )
            except OSError as e:
                logging.debug("Could not take the launch lock for %s: %s", identity, e)
        if (
            identity
            and waited
            and not (ide or recreate or reset or devcontainer)
            and get_workspace_state(identity) == "Running"
        ):
            # The sibling this call waited on already brought the workspace
            # up; re-running `devpod up` would re-walk a whole container
            # lifecycle to arrive where we already are.
            #
            # Only for a call that wanted nothing the sibling cannot have
            # done. An IDE to open, a rebuild, a reset and a devcontainer
            # variant are all requests a running workspace is not the answer
            # to -- and the variant especially, since skipping it would hand
            # the user the default container while they asked for another one
            # and say nothing about it.
            logging.info(f"Workspace {identity} was brought up by another dl run.")
            invalidate_workspace_list_cache()
            # The tools are still this call's business. "Running" says the
            # sibling's `devpod up` finished, not that its ensure_tools did:
            # it may have been interrupted between the two, its `up` may have
            # failed after the container started, or it may have run with
            # DEVLAUNCH_NO_TOOLS set where this one does not. ensure_tools is
            # idempotent and costs one probe round trip against a workspace
            # that is already up, which is the cheap way to stop a skipped
            # `up` from meaning a session with no tools.
            tools.ensure_tools(identity, run_devpod)
            return subprocess.CompletedProcess(args=["devpod"] + args, returncode=0)
        # Give every workspace the host's gh login, whatever its
        # devcontainer.json does or doesn't set up for itself.
        with gh_auth.up_args() as token_args:
            args.extend(token_args)
            result = run_devpod(args)
        # `up` creates and starts workspaces, so any snapshot of `devpod list`
        # taken before it is now out of date.
        invalidate_workspace_list_cache()
        # The tools a session always needs, for whatever repo this is: `up` is
        # the one path that runs for every workspace dl opens and is already
        # slow enough to absorb a round-trip. Only after a successful `up` --
        # there is no container to install into otherwise. Inside the lock, so
        # a launch waiting on a prewarm cannot attach before the tools land.
        if result.returncode == 0 and identity:
            tools.ensure_tools(identity, run_devpod)
    return result


def run_ssh(args: List[str], env: Optional[Dict[str, str]] = None) -> subprocess.CompletedProcess:
    """Run OpenSSH, dl's other way into a workspace.

    Separate from run_devpod because it is a different binary with a different
    failure mode, and because the pty transport has to be swappable in tests
    without stubbing out every devpod call as well.

    Takes the whole argv, `ssh` included, because tty_session composes a
    complete command rather than a tail of flags -- unlike run_devpod, whose
    callers each build up their own subcommand.

    Security note: list form, not shell=True, so nothing in the payload is
    interpreted by a host shell.
    """
    logging.debug("Running: %s", " ".join(args))
    try:
        with timing.span("ssh"):
            # nosec B603 B607 - list form, not shell=True; no command injection risk
            return subprocess.run(list(args), check=False, env=env)
    except FileNotFoundError as e:
        raise SshNotInstalled(SSH_MISSING_MESSAGE) from e


def workspace_ssh(
    workspace: str,
    command: Optional[str] = None,
    workdir: Optional[str] = None,
) -> int:
    """SSH into a workspace, optionally running a command.

    A command runs through whichever of the two transports can give it what it
    needs. devpod's --command never requests a pty, which is fine for `make
    test` and fatal for anything interactive -- `claude` reads the pipe as a
    non-interactive invocation and exits instead of starting a session. So when
    dl is itself on a terminal, the command goes to OpenSSH through the host
    alias devpod published, with -t (see tty_session). Otherwise, and when no
    alias exists, it goes to devpod as before.

    A bare attach is left on devpod: it already gets a pty, being the one case
    devpod requests one for.

    Args:
        workspace: The workspace ID to SSH into
        command: Optional command to run (if None, starts interactive shell)
        workdir: Working directory to start in. Leave unset to land in the
            workspaceFolder from devcontainer.json — devpod falls back to $HOME
            if given a path that doesn't exist in the container, so never guess
            one from the workspace id.
    """
    # devpod runs --command under a non-login, non-interactive `bash -c`, which
    # sources neither ~/.profile nor ~/.bashrc -- so PATH entries the image adds
    # there (notably $HOME/.pixi/bin) are missing and the payload dies with
    # "command not found". An interactive attach gets a login shell, so wrap
    # here to give both paths the same PATH. dl launches arbitrary repos, so the
    # parity has to come from the invocation rather than from any particular
    # devcontainer.json.
    #
    # Built once and shared by both transports: two copies of this expression
    # would be two chances for the transports to drift, which is the whole
    # failure this function exists to have fixed.
    payload = f"bash -lc {shlex.quote(command)}" if command else None

    if payload is not None and tty_session.have_terminal():
        if tty_session.devpod_host_configured(workspace):
            return _ssh_with_terminal(workspace, payload, workdir)
        logging.warning(
            "No devpod ssh host entry for %s, so this command gets no terminal; "
            "interactive programs may exit immediately. `dl %s restart` republishes it.",
            workspace,
            workspace,
        )

    args = ["ssh", workspace]
    if workdir:
        args.extend(["--workdir", workdir])
    if payload is not None:
        args.extend(["--command", payload])

    # Attaching to a running workspace skips workspace_up, so the gh login has
    # to be offered here too. Only the variable name lands in args; the token
    # travels in devpod's environment.
    token_args, env = gh_auth.ssh_args_and_env()
    args.extend(token_args)

    logging.info(f"SSH command: devpod {' '.join(args)}")
    outcome = run_devpod_session(args, env=env)

    # The two arms carry the same kind of number from different processes, which
    # is exactly the confusion this used to make: `dl` reported devpod's exit
    # code (always 1) for a session that had ended perfectly normally with, say,
    # 130. Whichever arm this is, the status returned is the session's.
    match outcome:
        case devpod_ssh.RemoteExit(status=status):
            return status
        case devpod_ssh.DevpodFailed(exit_code=exit_code):
            logging.debug("devpod ssh failed with exit code %s", exit_code)
            return exit_code
        case _ as unhandled:
            devpod_ssh.assert_never(unhandled)


def _ssh_with_terminal(workspace: str, payload: str, workdir: Optional[str]) -> int:
    """Run an already-wrapped payload under a pty via OpenSSH.

    No devpod_ssh.SshOutcome here, and nothing to recover: OpenSSH exits with
    the remote program's own status, which is the thing devpod loses by wrapping
    its *ssh.ExitError three times before type-asserting on it. This transport
    never had that bug, so it needs none of the machinery that works around it.
    """
    env_names, env = gh_auth.openssh_env_names_and_env()
    args = tty_session.ssh_command_args(workspace, payload, send_env=env_names, workdir=workdir)
    logging.info("SSH command: %s", " ".join(args))
    return run_ssh(args, env=env).returncode


def attach_workspace(workspace_id: str, shell_command: Optional[str] = None) -> int:
    """Hand the workspace to the user: name its prompt, then ssh in.

    Setting the hostname is a whole extra `devpod ssh` (~0.5s) in front of every
    attach, and it cannot be folded into the attach itself: bash reads the
    hostname once when the shell starts, so it has to be set before the session
    dl hands over begins, and `devpod ssh` exposes no hook inside that session.

    It is skipped for a one-shot `dl <ws> -- cmd`, which renders no prompt for
    the hostname to appear in — the round-trip would buy that command nothing.
    An interactive attach later still names the container, so nothing the user
    can see depends on having paid for it here.
    """
    if shell_command is None:
        setup_hostname(workspace_id)
    return workspace_ssh(workspace_id, shell_command)


def dotfiles_update(workspace_id: str) -> int:
    """Refresh dotfiles inside a running workspace.

    Runs chezmoi update + pixi global sync. Falls back to running install.sh
    if chezmoi is not available (e.g. workspace predates dotfiles setup).
    """
    ctx = get_context_options()
    dotfiles_url = ctx.get("DOTFILES_URL", "")

    if dotfiles_url:
        fallback = (
            f'echo "chezmoi not found, running full install..." && '
            f"DOTFILES_DIR=$(mktemp -d) && "
            f'git clone {shlex.quote(dotfiles_url)} "$DOTFILES_DIR" && '
            f'cd "$DOTFILES_DIR" && bash install.sh && '
            f'rm -rf "$DOTFILES_DIR" && '
            f'echo "Dotfiles installed successfully"'
        )
    else:
        fallback = 'echo "chezmoi not found and no DOTFILES_URL configured" && exit 1'

    update_cmd = (
        "if command -v chezmoi >/dev/null 2>&1; then "
        'echo "Updating dotfiles..." && '
        "chezmoi update --force && "
        'echo "Syncing pixi global packages..." && '
        "pixi global sync && "
        'echo "Dotfiles updated successfully"; '
        f"else {fallback}; fi"
    )
    return workspace_ssh(workspace_id, command=update_cmd)


def workspace_stop(workspace: str) -> int:
    """Stop a workspace."""
    result = run_devpod(["stop", workspace])
    # A stopped workspace still appears in `devpod list`, but with different
    # details, and `restart` calls straight into `up` after this.
    invalidate_workspace_list_cache()
    # The workspace list just changed, so the cache is wrong regardless of age.
    update_cache_background(force=True)
    return result.returncode


def workspace_delete(workspace: str, ignore_missing: bool = False) -> int:
    """Delete a workspace and its local clone (if any).

    The clone is removed only once devpod has actually let go of the workspace.
    devpod re-parses the workspace's devcontainer.json to tear the container
    down, so a config that has since moved or been renamed makes deletion fail —
    and removing the clone regardless strands the workspace for good, because
    devpod can then never find the config to retry with.

    ``ignore_missing`` makes a workspace devpod does not have count as deleted
    (devpod's own --ignore-not-found), so a forced remove is "ensure absent"
    the way `rm -f` is. The clone cleanup below still runs on that path: a
    stale clone with no workspace is exactly what a half-finished delete
    leaves, and what a cold-bench reset (ticket #140) must clear.
    """
    argv = ["delete", workspace] + (["--ignore-not-found"] if ignore_missing else [])
    result = run_devpod(argv)
    # Unconditionally: a delete that reports failure may still have got far
    # enough to change what devpod lists.
    invalidate_workspace_list_cache()
    if result.returncode != 0:
        logging.error(
            f"devpod could not delete {workspace}; keeping the local clone so it "
            f"stays retryable. If its devcontainer.json moved, restore the path or "
            f"run: devpod delete {workspace} --force"
        )
        update_cache_background(force=True)
        return result.returncode

    # Clean up local workspace clone (look up by workspace ID in metadata)
    try:
        clone_mgr = _get_clone_manager()
        if clone_mgr.remove_workspace_by_id(workspace):
            logging.info(f"Removed local clone for {workspace}")
    except Exception as e:
        logging.warning(f"Failed to remove local clone: {e}")
    # The workspace list just changed, so the cache is wrong regardless of age.
    update_cache_background(force=True)
    return result.returncode


def get_workspace_state(workspace_id: str) -> Optional[str]:
    """Get workspace state from devpod (e.g., 'Running', 'Stopped')."""
    result = run_devpod(["status", workspace_id, "--output", "json"], capture=True)
    if result.returncode != 0:
        return None
    try:
        data = json.loads(result.stdout)
        return data.get("state")
    except json.JSONDecodeError:
        logging.warning("Failed to parse devpod status JSON for workspace %s", workspace_id)
        return None


def print_help():
    """Print usage help."""
    help_text = """dl - DevLaunch CLI

Usage:
    dl                               Interactive workspace selector (fzf)
    dl <user/repo>                   Start workspace and attach shell
    dl <user/repo> <cmd>             Run workspace command (stop, code, etc.)
    dl <user/repo> -- <shell>        Run shell command in workspace

Options:
    --devcontainer <variant|path>    Use a non-default devcontainer.json. A bare
                                     name means .devcontainer/<name>/devcontainer.json.
                                     Stored with the workspace, so pass it once.

Environment:
    DEVLAUNCH_NO_GH_TOKEN=1          Do not forward the host's gh login into
                                     workspaces (forwarded as GH_TOKEN by default)

Workspace sources:
    dl myproject                     Existing workspace by name
    dl user/repo                     Create from GitHub repo
    dl user/repo@branch              Create from specific branch
    dl ./path                        Create from local path

Workspace commands:
    dl <user/repo> up                Start the workspace without attaching
    dl <user/repo> stop              Stop the workspace
    dl <user/repo> rm, prune         Delete the workspace. Refuses if its clone
                                     holds uncommitted or unpushed work, or if
                                     git cannot read the clone to find out; add
                                     --force to delete it anyway. --force also
                                     counts an already-absent workspace as
                                     deleted, like rm -f.
    dl <user/repo> code              Open in VS Code
    dl <user/repo> restart           Stop and start (no rebuild)
    dl <user/repo> recreate          Recreate container
    dl <user/repo> reset             Clean slate (remove all, recreate)
    dl <user/repo> dotfiles          Refresh dotfiles (chezmoi update)
    dl <user/repo> -- <command>      Run shell command in workspace

Global commands:
    dl --ls                          List all workspaces
    dl --ls --json                   List them as JSON, with each one's repo,
                                     branch, state, and what it holds that is
                                     not pushed anywhere ("unsaved"). For tools
                                     that decide which workspaces to clean up.
    dl --ls --size                   Add what deleting each workspace's clone
                                     would free. Off by default: it walks every
                                     file in the clone, which a listing should
                                     not do unasked. The number is *exclusive*
                                     bytes -- a repo's workspaces share their
                                     git objects with its bare cache, so those
                                     shared bytes are billed to none of them
                                     and freed only with the last one. It is
                                     therefore what you get back, not what `du`
                                     would print, and the figures do not add up
                                     to the size of the cache. Works with
                                     --json, where the field is `disk`.
    dl --install                     Install shell completions
    dl --refresh                     Refresh completion cache
    dl --prune [-y] [--force]        Remove the clone directories no workspace
                                     opens any more, and forget the records of
                                     directories already gone. Bare caches, live
                                     clones and every devpod workspace are left
                                     alone; a clone holding uncommitted or
                                     unpushed work is named and kept unless
                                     --force. Prints the plan and asks first.
    dl --purge [-y]                  Remove devlaunch's workspaces and caches
    dl --help, -h                    Show this help
    dl --version                     Show version (editable installs name their tree)

Examples:
    dl                               # Select workspace with fzf
    dl devpod                        # Open existing workspace
    dl loft-sh/devpod                # Create from GitHub
    dl blooop/devlaunch@main         # Create from specific branch
    dl ./my-project                  # Create from local folder
    dl blooop/devlaunch code         # Open in VS Code
    dl blooop/devlaunch -- make test # Run command in workspace
    dl blooop/devlaunch stop         # Stop workspace
    dl org/repo --devcontainer robot # Pick a devcontainer variant
"""
    print(help_text)


_CLONE_MANAGER_KEY = "clone_manager"
_cache: dict[str, WorkspaceCloneManager] = {}


def invalidate_clone_manager() -> None:
    """Forget the memoized clone manager, so the next use builds a fresh one.

    The memo below is bound to the cache and config directories that were in
    effect when it was built. A real dl invocation is one short-lived process,
    so those never move under it. A test session is not: without this, the
    first test to build a manager answers every later test's question about the
    cache, and a test asserting that a code path did *not* touch metadata.json
    would pass whether or not the path was still touching it -- it would only
    be observing the earlier test's memo.
    """
    _cache.pop(_CLONE_MANAGER_KEY, None)


def _get_clone_manager() -> WorkspaceCloneManager:
    """Lazy factory for WorkspaceCloneManager, migrating the cache on first use.

    This is where the one-shot id-scheme migration runs, for three reasons. It is
    dl's single construction point for the object that owns every read of a
    workspace path, so nothing can reach a stale path before the rename. It is
    lazy, so the commands that touch no workspace -- `--help`, `--version`,
    `--ls`, the completion commands, `--purge`, opening an existing workspace by
    name, and (since #145) a warm `owner/repo@branch` launch, which attaches to a
    workspace devpod already reports as running -- never reach it, which keeps
    #58's promise that help does no work. Migration therefore does not run on
    those shapes; it runs on the next command that does build the manager.
    And the memo makes it at most once per process.

    On an already-migrated cache this costs one integer comparison, because the
    trigger is the version header the storage load already parsed. Nothing here
    spawns devpod: the orphaned container ids come from metadata.
    """
    if _CLONE_MANAGER_KEY not in _cache:
        manager = WorkspaceCloneManager()
        try:
            # Under the metadata lock so two dl processes cannot migrate at
            # once: the renames are not idempotent mid-flight, and exclusive()
            # reloads first so the version check sees the other side's result.
            # migrate_cache calls save() directly, never a locked mutator.
            with manager.storage.exclusive():
                migrate_cache(manager.storage, pathlib.Path(manager.config.repos_dir))
        except OSError as e:
            # A failed migration must not take the command with it. The renames
            # that did happen are still resumable: the version header is only
            # written by the final save, so an unwritten file means the next run
            # migrates again and finds them already in place.
            logging.warning(f"Could not migrate the workspace cache: {e}")
        _cache[_CLONE_MANAGER_KEY] = manager
    return _cache[_CLONE_MANAGER_KEY]


# Commands that read the completion cache without changing what it describes.
# These are the ones worth warming it for before they run.
_CACHE_READING_COMMANDS = ("--ls", "--repos", "--completion-data")


def wants_startup_cache_refresh(args: List[str]) -> bool:
    """Whether this invocation should warm the completion cache before running.

    Only the commands that read the cache and leave the workspace list alone.
    Everything else either has no use for completions (--help, --version), owns
    the refresh itself (--install, --refresh, --update-cache), is about to delete
    the cache directory (--purge), or is a workspace command -- and those refresh
    once they are finished, when the workspace list they cache is final, instead
    of indexing the state they are about to replace.
    """
    if not args:
        return True  # the fzf picker is a view of the workspace list
    return args[0] in _CACHE_READING_COMMANDS


def main(argv: Optional[List[str]] = None) -> int:
    """Main entry point for dl CLI.

    Thin wrapper so there is exactly one handler for a missing devpod, and one
    for a devpod that answered with something dl could not read, however deep in
    the command either was noticed. Both messages go to stderr because stdout is
    parsed by the completion machinery (--repos, --completion-data).

    argv is the argument list without the program name, defaulting to the real
    one. It is a parameter so that a sibling entry point can hand dl a command
    line it built and get dl's behaviour itself rather than a second copy of it
    (see aid.py) — a copy is what left aid rebuilding containers dl reuses.
    """
    # The workspace-list snapshot is scoped to one command, not to the process:
    # a caller that drives main() twice (a test, a shell wrapper) must not have
    # the first command's view of devpod answer the second command's questions.
    invalidate_workspace_list_cache()
    # Timing is per-command, like the workspace-list snapshot: begin() here so
    # a second main() in the same process starts a fresh summary, emit() in the
    # finally so the summary lands on stderr however the command ended.
    timing.begin()
    try:
        return _run_cli(argv)
    except MissingBinary as e:
        print(e, file=sys.stderr)
        return DEVPOD_MISSING_EXIT_CODE
    except UnreadableWorkspaceList as e:
        print(f"error: {e}", file=sys.stderr)
        return UNREADABLE_WORKSPACE_LIST_EXIT_CODE
    finally:
        timing.emit()


def _run_cli(argv: Optional[List[str]] = None) -> int:
    """Dispatch a dl command line. See main() for the error handling around it."""
    if argv is None:
        argv = sys.argv[1:]
    try:
        args, devcontainer = extract_devcontainer_flag(argv)
    except ValueError as e:
        logging.error(str(e))
        return 1

    # The decision has to come after parsing: `dl --help` must not pay for a
    # refresh it has no use for.
    if wants_startup_cache_refresh(args):
        update_cache_background()

    # No args - try fzf selection
    if not args:
        selected = fuzzy_select_workspace()
        if not selected:
            print_help()
            return 1
        workspace_up(
            selected,
            workspace_identity=selected,
            devcontainer=devcontainer,
        )
        return attach_workspace(selected)

    # Global commands (no workspace required)
    if args[0] in ("--help", "-h"):
        print_help()
        return 0

    if args[0] == "--version":
        print(f"dl {get_version()}")
        return 0

    if args[0] == "--ls":
        with_size = "--size" in args[1:]
        if "--json" in args[1:]:
            return workspaces_as_json(with_size)
        print_workspaces(with_size)
        return 0

    if args[0] == "--repos":
        # Output known repos for bash completion (uses cache if available)
        cache = read_completion_cache()
        if cache and "repos" in cache:
            for repo in cache["repos"]:
                print(repo)
        else:
            for repo in get_known_repos():
                print(repo)
        return 0

    if args[0] == "--update-cache":
        # Silent background update. The TTL is re-checked here as well as in the
        # parent that spawned us: two parents can both see a stale cache before
        # either child has written one, and the second sweep would be pure waste.
        # --force marks a refresh that follows a workspace change, where the
        # cache is wrong however new it is.
        if "--force" not in args[1:] and completion_cache_is_fresh():
            return 0
        update_completion_cache()
        # Completions first, freshness second: the completion cache is what the
        # user's next keystroke reads, while the fetch sweep is for the launch
        # after that. Both are on the same hour, so a child that gets here does
        # both or, when it exits early above, neither.
        sweep_repo_fetches()
        return 0

    if args[0] == "--refresh":
        # Manual refresh with feedback
        print("Refreshing completion cache...")
        data = update_completion_cache()
        print(f"Cache updated: {len(data.get('workspaces', []))} workspaces found")
        return 0

    if args[0] == "--completion-data":
        # Output all completion data as JSON (fast, from cache)
        cache = read_completion_cache()
        if cache:
            print(json.dumps(cache))
        else:
            # No cache, generate and cache it
            data = update_completion_cache()
            print(json.dumps(data))
        return 0

    if args[0] == "--install":
        rc_path = None
        if len(args) > 1:
            rc_path = pathlib.Path(args[1])
        # Generate cache so completions work immediately
        update_completion_cache()
        return install_completions(rc_path)

    if args[0] == "--prune":
        return prune_command(args[1:])

    if args[0] == "--purge":
        # Check for -y flag to skip confirmation
        skip_confirm = len(args) > 1 and args[1] in ("-y", "--yes")
        cache_dir = _get_cache_dir()
        owned = workspace_ownership(list_workspaces(), cache_dir)
        print("This will remove all devlaunch data:")
        print(f"  - {len(owned.mine)} DevPod workspace(s)")
        print(f"  - {cache_dir}/ (workspace clones, repo caches, completions)")
        # Named, not merely excluded from the count: a user who asked for a
        # clean slate and gets survivors should learn it here, while saying no
        # is still an option, rather than from a later `dl --ls`.
        if owned.foreign:
            print()
            print(f"Leaving {len(owned.foreign)} workspace(s) devlaunch did not create:")
            for ws in owned.foreign:
                print(f"  - {ws.id}")
        print()
        if skip_confirm:
            return purge_all_data()
        confirm = input("Are you sure? [y/N] ").strip().lower()
        if confirm in ("y", "yes"):
            return purge_all_data()
        print("Aborted.")
        return 0

    # Workspace commands: dl <workspace> [subcommand] [-- command]
    raw_spec = args[0]
    subcommand = args[1] if len(args) > 1 else None

    # Resolve workspace spec and ID.
    #
    # "Does devpod already know this workspace?" used to be answered with a
    # `devpod list` paid by every workspace command. One `devpod status <id>`
    # answers it for the one workspace the command is about, at the price the
    # fast-attach check below was already paying -- so it is asked once, and
    # the answer (known_state) is threaded down instead of asked again. The
    # spec shapes cannot collide: a workspace id never contains `/`, `:` or a
    # path prefix, so whichever arm matches is the only arm that could.
    #
    # The trade: `devpod status` cannot tell "no such workspace" from a devpod
    # that failed to answer, where the listing raised UnreadableWorkspaceList.
    # A launch made wrongly cold by that redoes idempotent git work and hands
    # devpod a source it already knows; devpod's own error then names the real
    # problem.
    parsed = parse_owner_repo_branch(raw_spec)
    known_state: Optional[str] = None

    if parsed:
        # Git spec (owner/repo[@branch]) — check if workspace already exists first
        owner_repo, branch = parsed
        owner, repo = owner_repo.split("/", 1)

        # Validate owner and repo before anything builds a path out of them. The
        # branch is not resolved yet, so its check waits for the WorkspaceId below,
        # but ensure_repo() joins repos_dir/<owner>/<repo> and would otherwise act on
        # a traversal first and reject it after: `x/..` resolves to repos_dir itself
        # and `../x` leaves it entirely.
        try:
            validate_ref_name(owner, "owner")
            validate_ref_name(repo, "repo")
        except ValueError as e:
            logging.error(str(e))
            return 1

        remote_url = f"git@github.com:{owner_repo}.git"

        repo_ensured = False

        # Resolve branch early so we can compute workspace ID
        if not branch:
            # Constructed here rather than above the branch, because building it
            # reads config.toml, loads metadata.json under the metadata lock and
            # runs the cache migration -- and a spec that already names its
            # branch reaches the fast-attach check below without needing any of
            # it (#145). A bare owner/repo does need it, to name the default
            # branch, so this arm still pays for it. _get_clone_manager memoizes,
            # so a launch that passes through both arms builds it once.
            clone_mgr = _get_clone_manager()
            try:
                clone_mgr.repo_manager.ensure_repo(owner, repo, remote_url)
            except (RuntimeError, OSError) as e:
                logging.error(f"Repository '{owner_repo}': {e}")
                return 1
            repo_ensured = True
            branch = clone_mgr.repo_manager.get_default_branch(owner, repo)

        # Compute workspace ID with resolved branch. Constructing the WorkspaceId is
        # the parse boundary: an unsafe owner, repo or ref is rejected here, before it
        # can name a container, a directory or a git command. Nothing downstream
        # re-checks it, because holding the WorkspaceId is the evidence.
        try:
            workspace = WorkspaceId(owner, repo, branch)
        except ValueError as e:
            logging.error(str(e))
            return 1
        workspace_id = workspace.value

        # Fast path: if devpod already knows this workspace, skip clone manager
        known_state = get_workspace_state(workspace_id)
        if known_state is not None:
            workspace_spec = workspace_id
            custom_id = None
        else:
            # Full path: clone locally and pass local path to DevPod. The cold
            # path is the other place that needs the manager, so it is the other
            # place that builds it.
            clone_mgr = _get_clone_manager()
            if not repo_ensured:
                try:
                    clone_mgr.repo_manager.ensure_repo(owner, repo, remote_url)
                except (RuntimeError, OSError) as e:
                    logging.error(f"Repository '{owner_repo}': {e}")
                    return 1

            # Ensure branch exists in bare repo (create on remote if needed)
            try:
                clone_mgr.ensure_branch(owner, repo, branch)
            except (RuntimeError, OSError) as e:
                logging.error(f"Failed to ensure branch '{branch}': {e}")
                return 1

            custom_id = workspace_id

            # Create workspace clone
            try:
                workspace_path = clone_mgr.ensure_workspace(owner, repo, branch, remote_url)
                workspace_spec = str(workspace_path)
            # ValueError: the branch resolved above does go through WorkspaceId, but
            # ensure_workspace may fall back to the default branch recorded in the
            # repo's stored metadata, and that value reaches git unproven. It is
            # checked where it enters argv, which raises from in here.
            except (RuntimeError, OSError, ValueError) as e:
                logging.error(f"Failed to prepare workspace: {e}")
                return 1
    elif is_path_spec(raw_spec) or is_git_spec(raw_spec):
        workspace_spec = expand_workspace_spec(raw_spec)
        workspace_id = spec_to_workspace_id(raw_spec)
        custom_id = workspace_id  # Pass --id to create with our desired ID
    else:
        # A bare name can only be a workspace devpod already has; everything
        # creatable is a path or a git spec and matched above.
        known_state = get_workspace_state(raw_spec)
        if known_state is None and raw_spec not in get_workspace_ids():
            logging.error(
                f"Unknown workspace '{raw_spec}'. Use 'dl --ls' to list workspaces, "
                f"or specify owner/repo or ./path"
            )
            return 1
        # `status` failing is not the same as the workspace not existing, and
        # the difference decides whether the user can clean it up. `status`
        # consults the provider; `list` only reads devpod's own records. So a
        # workspace whose provider is broken, reconfigured or gone still
        # lists and cannot be described -- and that is precisely the
        # workspace somebody is about to run `dl <ws> rm` on. Answering
        # "Unknown workspace" there would be both a wrong diagnosis and a
        # refusal of the command that fixes it, so the listing gets the final
        # word. It costs a round trip only on the failure path, where a
        # second one is not what is wrong.
        workspace_spec = raw_spec
        workspace_id = raw_spec
        custom_id = None  # Don't pass --id for existing workspaces

    # Handle workspace subcommands
    # Paths below that never call workspace_up cannot honour a config choice.
    # Say so rather than discarding it silently.
    if devcontainer and subcommand in ("stop", "rm", "prune"):
        logging.warning(f"Ignoring --devcontainer: it does not apply to '{subcommand}'.")

    if subcommand == "stop":
        return workspace_stop(workspace_id)

    if subcommand in ("rm", "prune"):
        # The one thing dl refuses on its own account. It is not a judgement
        # about whether the work is finished -- dl has no way to know that --
        # but about whether this clone is the only place the work exists.
        # Cleanup is expected to be driven by something that knows more than dl
        # does (a ticket tool, a script, a person), and this is what keeps a
        # confident caller from destroying an hour of somebody's afternoon.
        forced = "--force" in args[2:]
        if not forced:
            # Three answers, and only one of them is permission (devlaunch#171).
            # Matched arm by arm rather than tested for truth, so that an answer
            # this code does not know about stops the delete instead of sliding
            # through an `else`.
            unsaved = _unsaved_work_in(workspace_id)
            if isinstance(unsaved, workspace_state.WouldLose):
                logging.error(
                    f"{workspace_id} holds {unsaved.description}. Push or commit it, or run: "
                    f"dl {raw_spec} rm --force"
                )
                return 1
            if isinstance(unsaved, workspace_state.CouldNotTell):
                # Refused for not knowing, and it says which. The work is still
                # on disk, and nothing has established that it exists anywhere
                # else -- which is the same standing as unpushed work, and gets
                # the same refusal and the same way past it.
                logging.error(
                    f"{workspace_id}: {unsaved.reason}. devlaunch will not delete a clone it "
                    f"cannot check. Look at it, or run: dl {raw_spec} rm --force"
                )
                return 1
            if not isinstance(unsaved, workspace_state.NothingToLose):
                workspace_state.unhandled_unsaved(unsaved)
        # `--force` also makes "already absent" a success (`rm -f` semantics):
        # a bench reset runs this before every timed run, including the first,
        # where there is nothing to remove yet.
        return workspace_delete(workspace_id, ignore_missing=forced)

    if subcommand == "up":
        # Start (or create) the workspace without attaching: the warm half of
        # a launch, for callers that want the container ready before a user
        # arrives -- e.g. wayfinder prewarming a ticket's workspace while its
        # launch overlay is still open. Idempotent and quiet when already up.
        if custom_id is None and known_state == "Running":
            logging.info(f"Workspace {workspace_id} is already running.")
            # Still top up the tools. `up` is one of the two verbs tools.py
            # names as how a workspace that missed provisioning -- started by
            # something other than dl, or created before provisioning existed
            # -- gets it, and returning here without them would make the
            # documented recovery the one path that cannot recover. It is a
            # probe round trip against a running workspace, and silent when
            # there is nothing to do.
            tools.ensure_tools(workspace_id, run_devpod)
            return 0
        result = workspace_up(
            workspace_spec,
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
        update_cache_background(force=True)
        return result.returncode

    if subcommand == "code":
        result = workspace_up(
            workspace_spec,
            ide="vscode",
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
        update_cache_background(force=True)
        return result.returncode

    if subcommand == "recreate":
        result = workspace_up(
            workspace_spec,
            recreate=True,
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
        if result.returncode != 0:
            return result.returncode
        ret = attach_workspace(workspace_id)
        update_cache_background(force=True)
        return ret

    if subcommand == "restart":
        # Stop and start without rebuilding
        stop_ret = workspace_stop(workspace_id)
        if stop_ret != 0:
            return stop_ret
        result = workspace_up(
            workspace_spec,
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
        if result.returncode != 0:
            return result.returncode
        ret = attach_workspace(workspace_id)
        # workspace_stop already asked for a refresh on the way through; the
        # once-per-process latch is what keeps this from being a second one.
        update_cache_background(force=True)
        return ret

    if subcommand == "reset":
        # Clean slate - remove everything and recreate
        result = workspace_up(
            workspace_spec,
            reset=True,
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
        if result.returncode != 0:
            return result.returncode
        ret = attach_workspace(workspace_id)
        update_cache_background(force=True)
        return ret

    if subcommand == "dotfiles":
        # Ensure workspace is running before refreshing dotfiles
        state = get_workspace_state(workspace_id)
        if state != "Running":
            logging.info(f"Starting workspace {workspace_id}...")
            result = workspace_up(workspace_spec, workspace_id=custom_id)
            if result.returncode != 0:
                return result.returncode
        return dotfiles_update(workspace_id)

    # Check for shell command (after --)
    shell_command = None
    if subcommand == "--" and len(args) > 2:
        shell_command = " ".join(args[2:])
    elif subcommand is not None and subcommand != "--":
        # Unknown subcommand - treat as error
        logging.error(
            f"Unknown command '{subcommand}'. Use 'dl {raw_spec} -- {subcommand}' to run a shell command."
        )
        return 1

    # Fast-attach: skip workspace_up() if workspace is already running.
    # known_state is the status fetched during spec resolution above; every
    # path that leaves custom_id None filled it in.
    if custom_id is None and known_state == "Running":
        logging.info(f"Workspace {workspace_id} is already running, attaching...")
        if devcontainer:
            logging.warning(
                f"Ignoring --devcontainer: {workspace_id} is already running. "
                f"Use 'dl {raw_spec} recreate --devcontainer ...' to switch config."
            )
        ret = attach_workspace(workspace_id, shell_command)
        update_cache_background(force=True)
        return ret

    # Default: start workspace and attach shell
    try:
        result = workspace_up(
            workspace_spec,
            workspace_id=custom_id,
            workspace_identity=workspace_id,
            devcontainer=devcontainer,
        )
    except (RuntimeError, OSError) as e:
        logging.error(f"Failed to create workspace: {e}")
        return 1

    if result.returncode != 0:
        return result.returncode

    # Attach to workspace (naming its prompt first, for an interactive shell)
    ret = attach_workspace(workspace_id, shell_command)

    # Update cache after workspace operations: this path may have created the
    # workspace, so the refresh has to happen now that it exists.
    update_cache_background(force=True)

    return ret


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
