# pylint: disable=redefined-outer-name
"""Which id `dl` addresses devpod by, when the derivation and the record differ.

devlaunch#88. `dl` derived the devpod workspace id from `(owner, repo, ref)` on
every command and stored it nowhere, so the derivation was the only copy of it
in existence. PR #81 changed that derivation, and every workspace created under
the old one stopped being addressable in the same instant -- 36 of 39 on the
reporting host. Nothing was corrupted and nothing was deleted; `dl` simply
started asking devpod about an id devpod had never heard of, and devpod
answered, correctly, that there was no such workspace.

**The scheme change is simulated here, never written down.** These tests move
the derivation itself -- one syllable narrower, which is the change the id
format has actually undergone once already (18 bits to 24) -- and then read
back whichever ids that produces. A test that hardcoded "the old id was
`devlaunch-main`" would pin today's algorithm under an assertion about
yesterday's, and would have to be rewritten by the very change it exists to
catch.
"""

import json
import subprocess
from pathlib import Path
from typing import Dict, List, Optional
from unittest.mock import patch

import pytest

from devlaunch import dl
from devlaunch import workspace_id as workspace_id_module
from devlaunch.workspace_id import WorkspaceId
from devlaunch.xdg import devlaunch_cache

OWNER, REPO, BRANCH = "blooop", "devlaunch", "main"


def derived_id(syllables: Optional[int] = None) -> str:
    """The id `dl`'s derivation gives for the fixture triple, optionally moved.

    *syllables* stands in for "a different derivation". It is not a knob dl
    exposes; it is the one internal of the id format that has really changed,
    and moving it is the cheapest honest way to produce a *pair* of ids for one
    triple without either of them being a literal in this file.
    """
    if syllables is None:
        return WorkspaceId(OWNER, REPO, BRANCH).value
    with patch.object(workspace_id_module, "_SYLLABLES", syllables):
        return WorkspaceId(OWNER, REPO, BRANCH).value


class FakeDevpod:
    """Answers `devpod` for the workspaces it knows and denies the rest.

    `status` is the call under test: it is how dl asks "do you have this one",
    and a workspace devpod has never heard of exits non-zero. Everything else
    succeeds and is recorded, so a test can read back the id each subcommand
    was addressed by.
    """

    def __init__(self, known: Dict[str, str]):
        self.known = known
        self.calls: List[List[str]] = []

    def __call__(self, args, capture=False, env=None, stdin_file=None):
        # pylint: disable=unused-argument
        self.calls.append(list(args))
        if args[:1] == ["status"]:
            state = self.known.get(args[1])
            if state is None:
                return subprocess.CompletedProcess(args, 1, "", "workspace not found")
            return subprocess.CompletedProcess(args, 0, json.dumps({"state": state}), "")
        if args[:1] == ["list"]:
            listing = [{"id": ws_id, "source": {"localFolder": "/nowhere"}} for ws_id in self.known]
            return subprocess.CompletedProcess(args, 0, json.dumps(listing), "")
        return subprocess.CompletedProcess(args, 0, "", "")

    def addressed_by(self, subcommand: str) -> List[str]:
        """The workspace ids *subcommand* was invoked against, in order."""
        return [call[1] for call in self.calls if call[:1] == [subcommand]]


def write_record(
    cache: Path,
    *,
    workspace_id: str,
    devpod_workspace_id: Optional[str],
    clone: Path,
) -> None:
    """A metadata.json holding one worktree record, at the current schema.

    Written as a file rather than through MetadataStorage because what is under
    test is dl reading a record it did not write in this process -- which is the
    situation on every host that has one. The version header is current so the
    id-scheme migration is a no-op and cannot move the record out from under the
    assertion.
    """
    record = {
        "owner": OWNER,
        "repo": REPO,
        "branch": BRANCH,
        "local_path": str(clone),
        "workspace_id": workspace_id,
        "created_at": "2026-03-01T18:39:40",
        "last_used": "2026-03-01T18:39:40",
        "devpod_workspace_id": devpod_workspace_id,
    }
    cache.mkdir(parents=True, exist_ok=True)
    (cache / "metadata.json").write_text(
        json.dumps(
            {
                "version": 2,
                "repositories": {},
                "worktrees": {f"{OWNER}/{REPO}/{BRANCH}": record},
            }
        )
    )


@pytest.fixture
def cache() -> Path:
    """The suite's scratch devlaunch cache, with a clone directory in it."""
    root = devlaunch_cache()
    clone = root / "repos" / OWNER / REPO / derived_id()
    clone.mkdir(parents=True)
    (clone / ".git").mkdir()
    return root


def resolve(known: Dict[str, str]) -> dl.KnownWorkspace:
    """Ask dl which workspace the fixture triple is, against a devpod that
    knows exactly *known*."""
    with patch.object(dl, "run_devpod", FakeDevpod(known)):
        return dl.resolve_known_workspace(derived_id(), OWNER, REPO, BRANCH)


class TestFollowingTheRecordRatherThanTheDerivation:
    """The stored id wins over a derived one devpod does not recognise."""

    def test_the_simulated_scheme_change_really_moves_the_id(self):
        """Guards the other tests: if this stops holding they prove nothing."""
        assert derived_id(3) != derived_id()

    def test_a_workspace_created_under_the_old_scheme_is_still_addressable(self, cache):
        """The regression PR #81 caused, and the one this must make impossible.

        The record was written by a dl whose derivation produced the narrower
        id; the running dl derives the wider one. devpod only has the narrower.
        Following the derivation reaches a workspace devpod has never heard of;
        following the record reaches the workspace, and brings back the state
        devpod reported for the id actually addressed.
        """
        old = derived_id(3)
        write_record(
            cache,
            workspace_id=old,
            devpod_workspace_id=old,
            clone=cache / "repos" / OWNER / REPO / derived_id(),
        )

        assert resolve({old: "Stopped"}) == dl.KnownWorkspace(old, "Stopped")

    def test_a_record_that_agrees_with_the_derivation_changes_nothing(self, cache):
        """The everyday case still answers with the id it always did."""
        current = derived_id()
        write_record(
            cache,
            workspace_id=current,
            devpod_workspace_id=current,
            clone=cache / "repos" / OWNER / REPO / current,
        )

        assert resolve({current: "Running"}) == dl.KnownWorkspace(current, "Running")

    def test_a_record_with_no_stored_id_falls_back_to_the_derivation(self, cache):
        """Every record written before this change has the field empty.

        There is nothing to follow, so the derivation is still the answer -- and
        it has to be, or the fix would break the machines it is meant to fix on
        its way to fixing them.
        """
        write_record(
            cache,
            workspace_id=derived_id(),
            devpod_workspace_id=None,
            clone=cache / "repos" / OWNER / REPO / derived_id(),
        )

        assert resolve({}) == dl.KnownWorkspace(derived_id(), None)

    def test_a_stored_id_devpod_also_denies_is_not_used(self, cache):
        """A record can outlive its devpod workspace; then neither id is live.

        metadata.json is append-mostly, so a record naming a workspace deleted
        months ago is ordinary rather than exceptional. The answer has to be the
        derived id -- the one a create would use -- not a workspace that is
        doubly gone.
        """
        old = derived_id(3)
        write_record(
            cache,
            workspace_id=old,
            devpod_workspace_id=old,
            clone=cache / "repos" / OWNER / REPO / derived_id(),
        )

        assert resolve({}) == dl.KnownWorkspace(derived_id(), None)

    def test_no_record_at_all_falls_back_to_the_derivation(self, cache):
        """A fresh cache, or any spec dl has never launched."""
        assert cache.exists()
        assert resolve({}) == dl.KnownWorkspace(derived_id(), None)


class TestTheSubcommandsAddressWhatWasResolved:
    """One resolution feeds every devpod-addressing subcommand.

    `stop`, `rm`, `restart`, `recreate`, `reset` and the attach all read the
    same `workspace_id` the launch path resolved, so pinning one of them pins
    the seam they share. `stop` is the cheapest: it reaches devpod and returns
    without a container.
    """

    def test_a_stop_reaches_the_workspace_the_record_names(self, cache):
        old = derived_id(3)
        write_record(
            cache,
            workspace_id=old,
            devpod_workspace_id=old,
            clone=cache / "repos" / OWNER / REPO / derived_id(),
        )
        devpod = FakeDevpod({old: "Stopped"})

        with patch.object(dl, "run_devpod", devpod):
            assert dl.main([f"{OWNER}/{REPO}@{BRANCH}", "stop"]) == 0

        assert devpod.addressed_by("stop") == [old]


class TestTheWarmPathStillReadsNoMetadata:
    """#145's promise, which the lookup above is placed so as not to break.

    A launch of a workspace devpod already knows must not load metadata.json --
    it is a lock acquisition, a parse and the id-scheme migration's version
    check on the one path that was deliberately cleared of all three. The
    lookup is therefore asked only *after* devpod has denied the derived id,
    which is the only case where the record can say anything new.
    """

    def test_a_workspace_devpod_knows_builds_no_clone_manager(self, cache):
        current = derived_id()
        write_record(
            cache,
            workspace_id=current,
            devpod_workspace_id=current,
            clone=cache / "repos" / OWNER / REPO / current,
        )
        devpod = FakeDevpod({current: "Running"})

        with patch.object(dl, "run_devpod", devpod):
            dl.main([f"{OWNER}/{REPO}@{BRANCH}", "stop"])

        assert dl._CLONE_MANAGER_KEY not in dl._cache  # pylint: disable=protected-access
