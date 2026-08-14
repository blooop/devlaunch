# pylint: disable=redefined-outer-name
"""`dl --reconcile`: re-point devpod records the id-scheme change left behind.

devlaunch#88's second half. Persisting the workspace id stops the disagreement
happening again; it repairs nothing that has already happened, because the
records that disagree were written before there was a field to write it in. On
the reporting host 36 of 39 devpod workspaces recorded a source folder that was
missing (35) or a config-only stub devpod itself wrote from its cache (1), while
the real checkout sat beside it under the new leaf name.

**The join is by path, never by id.** The id is the thing that changed, so it
cannot connect the two records; the recorded source path still names owner and
repo exactly, and its leaf still names the branch the way `dl` used to name
directories. `--prune`'s placement rules already reason this way (see
`_site_of`), and this command reuses them rather than growing a second opinion
about where a workspace is.

**Nothing here is deleted, ever.** An orphan `dl` cannot adopt is reported and
left standing -- the same rule `--prune` follows for a disputed clone, and the
reason `Disputed` was carved out as its own arm rather than folded into
`Orphaned`. A wrongly-adopted workspace costs a rebuild; a wrongly-deleted one
costs whatever was in it.

The clones here are plain directories with a `.git` in them rather than real git
repositories, and that is not the corner-cutting it would be in
`test_prune_orphaned_clones.py`. That file guards a deletion, so its guard is a
real `git status` and a stub would answer "holds nothing" -- the reply that
deletes. This command asks git nothing at all: what it needs to know is whether
a directory is a checkout, which is `.git`'s presence and devlaunch#88's own
published diagnostic.
"""

import json
from pathlib import Path
from typing import List, Optional
from unittest.mock import patch

import pytest

from devlaunch import dl
from devlaunch.workspace_id import WorkspaceId
from devlaunch.xdg import devlaunch_cache

OWNER, REPO = "blooop", "devlaunch"


class World:
    """A devlaunch cache and a devpod home that disagree about where things are.

    Both are real directories under the suite's scratch roots, and the devpod
    side is written in devpod's own on-disk shape -- `contexts/<context>/
    workspaces/<id>/workspace.json` with a `source.localFolder` -- because
    re-pointing that file is what the command does and a test that mocked it
    would be asserting against its own idea of devpod's format.
    """

    def __init__(self, cache: Path, devpod_home: Path):
        self.cache = cache
        self.devpod_home = devpod_home
        self.repo_dir = cache / "repos" / OWNER / REPO
        self.repo_dir.mkdir(parents=True)
        (self.repo_dir / ".bare").mkdir()
        self.listed: List[dict] = []

    def clone(self, branch: str) -> Path:
        """A healthy workspace clone at the leaf this build would give it."""
        path = self.repo_dir / WorkspaceId(OWNER, REPO, branch).value
        path.mkdir()
        (path / ".git").mkdir()
        return path

    def record(self, branch: str, clone: Path, devpod_workspace_id: Optional[str] = None) -> None:
        """One metadata.json worktree record, written through dl's own storage."""
        # Imported here so the module reads as "the cache dl keeps", not as a
        # second implementation of it.
        from devlaunch.worktree.models import WorktreeInfo  # pylint: disable=import-outside-toplevel
        from devlaunch.worktree.storage import (  # pylint: disable=import-outside-toplevel
            MetadataStorage,
        )

        storage = MetadataStorage(self.cache / "metadata.json")
        storage.add_worktree(
            WorktreeInfo(
                owner=OWNER,
                repo=REPO,
                branch=branch,
                local_path=clone,
                workspace_id=WorkspaceId(OWNER, REPO, branch).value,
                devpod_workspace_id=devpod_workspace_id,
            )
        )

    def workspace(self, workspace_id: str, source: Path, context: str = "default") -> Path:
        """A devpod workspace record sourced at *source*, however dead."""
        directory = self.devpod_home / "contexts" / context / "workspaces" / workspace_id
        directory.mkdir(parents=True)
        path = directory / "workspace.json"
        path.write_text(
            json.dumps(
                {
                    "id": workspace_id,
                    "provider": {"name": "docker"},
                    "ide": {"name": "none"},
                    "source": {"localFolder": str(source)},
                    "creationTimestamp": "2026-03-01T18:39:40Z",
                    "lastUsed": "2026-03-01T18:39:40Z",
                    "context": context,
                }
            )
        )
        self.listed.append(
            {
                "id": workspace_id,
                "source": {"localFolder": str(source)},
                "provider": {"name": "docker"},
                "ide": {"name": "none"},
                "context": context,
                "lastUsed": "2026-03-01T18:39:40Z",
            }
        )
        return path

    def sourced_at(self, workspace_id: str, context: str = "default") -> str:
        directory = self.devpod_home / "contexts" / context / "workspaces" / workspace_id
        return json.loads((directory / "workspace.json").read_text())["source"]["localFolder"]

    def stored_id(self, branch: str) -> Optional[str]:
        from devlaunch.worktree.storage import (  # pylint: disable=import-outside-toplevel
            MetadataStorage,
        )

        record = MetadataStorage(self.cache / "metadata.json").get_worktree(OWNER, REPO, branch)
        return record.devpod_workspace_id if record else None


class FakeDevpod:
    """Answers `devpod list` from the world and records everything else.

    Recording the rest is the assertion that matters most here: this command
    must never reach `devpod delete`, and a mock that quietly succeeded at one
    would let that regression through green.
    """

    def __init__(self, world: World):
        self.world = world
        self.calls: List[List[str]] = []

    def __call__(self, args, capture=False, env=None, stdin_file=None):
        # pylint: disable=unused-argument
        self.calls.append(list(args))
        if args[:1] == ["list"]:
            import subprocess  # pylint: disable=import-outside-toplevel

            return subprocess.CompletedProcess(args, 0, json.dumps(self.world.listed), "")
        import subprocess  # pylint: disable=import-outside-toplevel

        return subprocess.CompletedProcess(args, 0, "", "")

    def subcommands(self) -> List[str]:
        return [call[0] for call in self.calls if call]


@pytest.fixture
def world(tmp_path, monkeypatch) -> World:
    monkeypatch.setenv("DEVPOD_HOME", str(tmp_path / "devpod"))
    return World(devlaunch_cache(), tmp_path / "devpod")


def reconcile(world: World, *flags: str) -> tuple:
    """Run the command, returning its exit code and the devpod it spoke to."""
    devpod = FakeDevpod(world)
    with patch.object(dl, "run_devpod", devpod):
        return dl.main(["--reconcile", *flags]), devpod


class TestAdoptingAnOrphanedRecord:
    """devpod points at a folder that is gone; the checkout is next door."""

    def test_the_workspace_is_re_pointed_at_the_clone_that_holds_the_checkout(self, world):
        """The reporting host's exact state, in miniature.

        devpod records `<repo>/main`, which no longer exists; the record and the
        checkout are at `<repo>/<new-leaf>`. Nothing joins them but the path.
        """
        clone = world.clone("main")
        world.record("main", clone)
        world.workspace("devlaunch-main", world.repo_dir / "main")

        status, _ = reconcile(world, "-y")

        assert status == 0
        assert world.sourced_at("devlaunch-main") == str(clone)

    def test_the_record_learns_the_devpod_workspace_id_it_was_missing(self, world):
        """Adoption writes the second copy of the id that never existed.

        Without this the repair holds only until the derivation next moves, and
        the whole ticket would have to be worked again.
        """
        clone = world.clone("main")
        world.record("main", clone)
        world.workspace("devlaunch-main", world.repo_dir / "main")

        reconcile(world, "-y")

        assert world.stored_id("main") == "devlaunch-main"

    def test_a_config_only_stub_is_adopted_like_a_missing_folder(self, world):
        """The 36th workspace on the reporting host: the folder is *there*.

        devpod reconstitutes a source folder from its cached workspace result
        when it finds the recorded one absent, leaving a directory holding one
        `devcontainer.json` and no `.git`. It looks healthy to anything that
        checks for existence, and it is the same orphan.
        """
        clone = world.clone("main")
        world.record("main", clone)
        stub = world.repo_dir / "main"
        (stub / ".devcontainer").mkdir(parents=True)
        (stub / ".devcontainer" / "devcontainer.json").write_text("{}")
        world.workspace("devlaunch-main", stub)

        reconcile(world, "-y")

        assert world.sourced_at("devlaunch-main") == str(clone)

    def test_a_branch_whose_old_directory_name_was_flattened_is_matched(self, world):
        """`feature/x` was a directory called `feature-x` under the old scheme.

        The leaf is the only thing carrying the branch, so the reconciler has to
        know the naming `dl` itself used to write -- which is history, not a
        guess about somebody else's format.
        """
        clone = world.clone("feature/x")
        world.record("feature/x", clone)
        world.workspace("devlaunch-feature-x", world.repo_dir / "feature-x")

        reconcile(world, "-y")

        assert world.sourced_at("devlaunch-feature-x") == str(clone)


class TestRefusingToGuessAndRefusingToDelete:
    """An orphan with nothing to adopt is named, and left exactly where it is."""

    def test_an_orphan_with_no_clone_is_reported_and_kept(self, world, capsys):
        world.workspace("devlaunch-gone", world.repo_dir / "gone")

        status, devpod = reconcile(world, "-y")

        assert world.sourced_at("devlaunch-gone") == str(world.repo_dir / "gone")
        assert "devlaunch-gone" in capsys.readouterr().out
        assert "delete" not in devpod.subcommands()
        assert status == 0

    def test_a_clone_a_live_workspace_already_opens_is_not_taken_from_it(self, world):
        """Two records, one clone: adopting it would leave the live workspace
        sourced at a directory a second workspace now claims."""
        clone = world.clone("main")
        world.record("main", clone)
        world.workspace("devlaunch-main-live", clone)
        world.workspace("devlaunch-main", world.repo_dir / "main")

        reconcile(world, "-y")

        assert world.sourced_at("devlaunch-main") == str(world.repo_dir / "main")
        assert world.sourced_at("devlaunch-main-live") == str(clone)

    def test_two_orphans_matching_one_clone_are_both_left_alone(self, world):
        """Ambiguity is reported, not resolved by listing order.

        Two devpod workspaces registered against the same dead folder, which is
        what a re-registration under a second id leaves behind. One clone
        answers to it. Adopting either would be a coin flip, the loser would
        still be broken with nothing said about why, and the winner would be
        sharing its checkout with a workspace nobody chose.
        """
        clone = world.clone("main")
        world.record("main", clone)
        world.workspace("ws-a", world.repo_dir / "main")
        world.workspace("ws-b", world.repo_dir / "main")

        status, _ = reconcile(world, "-y")

        assert status == 0
        assert world.sourced_at("ws-a") == str(world.repo_dir / "main")
        assert world.sourced_at("ws-b") == str(world.repo_dir / "main")
        assert world.stored_id("main") is None

    def test_a_workspace_outside_the_cache_is_not_this_command_s_business(self, world):
        """`dl ./path` and every hand-made `devpod up` land here."""
        outside = world.devpod_home / "someones-project"
        outside.mkdir(parents=True)
        world.workspace("someones-project", outside)

        status, devpod = reconcile(world, "-y")

        assert world.sourced_at("someones-project") == str(outside)
        assert "delete" not in devpod.subcommands()
        assert status == 0


class TestRunningItTwiceChangesNothing:
    """Idempotence, which is what makes it safe to run when unsure."""

    def test_a_second_run_finds_nothing_to_do(self, world, capsys):
        clone = world.clone("main")
        world.record("main", clone)
        world.workspace("devlaunch-main", world.repo_dir / "main")

        reconcile(world, "-y")
        # devpod's listing is the world's, so the repaired source is what the
        # second run sees -- which is the point: the repair has to take the
        # workspace out of the orphan class, not merely out of this run's plan.
        world.listed[0]["source"]["localFolder"] = world.sourced_at("devlaunch-main")
        capsys.readouterr()

        status, devpod = reconcile(world, "-y")

        assert status == 0
        assert "Nothing to reconcile." in capsys.readouterr().out
        assert "delete" not in devpod.subcommands()
        assert world.sourced_at("devlaunch-main") == str(clone)
        assert world.stored_id("main") == "devlaunch-main"


class TestTheReportComesBeforeTheChange:
    """`--prune`'s shape: print the plan, confirm, `-y` to skip."""

    def test_without_consent_nothing_is_written(self, world):
        clone = world.clone("main")
        world.record("main", clone)
        world.workspace("devlaunch-main", world.repo_dir / "main")

        with patch("builtins.input", return_value="n"):
            status, _ = reconcile(world)

        assert status == 0
        assert world.sourced_at("devlaunch-main") == str(world.repo_dir / "main")

    def test_the_plan_names_the_rebuild_the_repair_costs(self, world, capsys):
        """The container is bind-mounted at the dead path, so a repaired
        workspace is not a reusable one. Saying so is part of the repair."""
        clone = world.clone("main")
        world.record("main", clone)
        world.workspace("devlaunch-main", world.repo_dir / "main")

        reconcile(world, "-y")

        assert "recreate" in capsys.readouterr().out
