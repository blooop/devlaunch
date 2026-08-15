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
import os
from pathlib import Path
from typing import List, Optional
from unittest.mock import patch

import pytest

from devlaunch import dl
from devlaunch.workspace_id import WorkspaceId
from devlaunch.worktree.migration import MigrationReport, migrate_cache
from devlaunch.xdg import devlaunch_cache

# The migration suite's own builder for a pre-#64 cache, rather than a second
# idea of what one looks like. What TestTheRepairTheMigrationsNoticePromises
# asks is whether the artifact that migration produces is one this command can
# adopt, and two descriptions of the input would let the answer be yes about a
# cache nobody's dl ever wrote. `test/` is on sys.path (see test/conftest.py),
# which is how test_claude_code_feature_mounts.py already reaches
# test_lending_doc.
from test_worktree_migration import build_legacy_cache, _old_leaf

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

    def old_scheme_clone(self, branch: str) -> Path:
        """*branch*'s clone and record as a pre-#64 build left them, un-migrated.

        Returned path is the directory under its **old** leaf, which has to be
        read before :meth:`migrate` runs because that is the name the rename
        takes away -- and the name devpod's record is still holding.
        """
        build_legacy_cache(self.cache.parent, {(OWNER, REPO): [branch]})
        return self.repo_dir / _old_leaf(branch)

    def migrate(self) -> MigrationReport:
        """Run dl's real one-shot migration over this cache, once.

        The migration itself and not a hand-made imitation of its result: the
        renamed leaf, the repointed record and the list of orphaned container
        ids all come out of the code that ships, so a change to any of them
        arrives here rather than being described twice.
        """
        from devlaunch.worktree.storage import (  # pylint: disable=import-outside-toplevel
            MetadataStorage,
        )

        report = migrate_cache(MetadataStorage(self.cache / "metadata.json"), self.cache / "repos")
        assert report is not None, "the cache under test was not on the old scheme"
        return report

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

    def remote_workspace(self, workspace_id: str, url: str) -> None:
        """A workspace devpod sources from a git remote rather than a folder.

        Listed only, with no `workspace.json` written for it, and that absence is
        an assertion: this command re-points a record by rewriting that file, so
        a run that decided to adopt this workspace would have nothing to rewrite
        and would say so. It stands in for the workspaces `devpod up <url>` and
        every other tool leave in the shared namespace.
        """
        self.listed.append(
            {
                "id": workspace_id,
                "source": {"gitRepository": url},
                "provider": {"name": "docker"},
                "ide": {"name": "none"},
                "context": "default",
                "lastUsed": "2026-03-01T18:39:40Z",
            }
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


class TestTheRepairTheMigrationsNoticePromises:
    """The id-scheme migration tells the user this command will adopt what it orphaned.

    That notice is written in migration.py and its truth is owed here, across a
    module boundary nothing else spans: the notice's own tests pin the words and
    the ordering, so if the join in :func:`_orphaned_workspaces` or the spellings
    in :func:`_leaf_spellings` ever narrowed, the sentence would start lying with
    every test in both files still green. One case, built out of the migration's
    real machinery, is what closes that.
    """

    def test_a_container_the_migration_orphans_is_one_this_command_adopts(self, world):
        """The migration's exact leavings: old source path, renamed clone.

        Nothing here is described twice. The clone and the record are the old
        cache the migration suite builds, the rename is the shipped
        `migrate_cache`, the orphaned container's id is read off the report's own
        list -- the list the notice counts and writes to disk for the user -- and
        the clone the adoption must land on is derived with `WorkspaceId` rather
        than spelled out, so a moved derivation moves the expectation with it.

        `feature/auth` rather than `main` because it is the branch whose old
        directory name is not its own name: the migration wrote `feature-auth`,
        so the join has to come through the flattened spelling rather than
        through the two that coincide when a branch needs no flattening.
        """
        old_source = world.old_scheme_clone("feature/auth")
        report = world.migrate()
        (orphaned_id,) = report.orphaned_ids
        # devpod learned nothing from the rename, because nothing told it: its
        # record still sources the container at the directory that has moved.
        world.workspace(orphaned_id, old_source)

        status, _ = reconcile(world, "-y")

        assert status == 0
        renamed = world.repo_dir / WorkspaceId(OWNER, REPO, "feature/auth").value
        assert world.sourced_at(orphaned_id) == str(renamed)
        # The other half of adoption, and the half the migration deliberately did
        # not do: it leaves `devpod_workspace_id` alone rather than giving the
        # field a second meaning, so this is the only writer that can join the
        # renamed record back to the container the notice named.
        assert world.stored_id("feature/auth") == orphaned_id


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

    def test_a_clone_whose_subdirectory_a_live_workspace_opens_is_not_taken_either(self, world):
        """`devpod up <clone>/subproject` holds the clone, and holds it whole.

        The check has to be containment and not equality, which is the mistake
        `WorkspaceLocations.holder` was carved out for on the deletion side
        ("Equality answered no and deleted the parent"). Here equality answers
        no and hands the directory to a dead record instead: the live workspace
        keeps opening a subdirectory of a checkout a second workspace now claims
        as its source, which is the collision this refusal exists to prevent.
        """
        clone = world.clone("main")
        world.record("main", clone)
        (clone / "subproject").mkdir()
        world.workspace("devlaunch-main-live", clone / "subproject")
        world.workspace("devlaunch-main", world.repo_dir / "main")

        status, _ = reconcile(world, "-y")

        assert status == 0
        assert world.sourced_at("devlaunch-main") == str(world.repo_dir / "main")
        assert world.sourced_at("devlaunch-main-live") == str(clone / "subproject")
        assert world.stored_id("main") is None

    def test_two_clones_answering_to_one_old_name_adopt_neither(self, world, capsys):
        """The old flattened leaf is not injective, so one name can mean two branches.

        `feature/auth` and `feature-auth` are both branches a repository can
        have, and under the pre-#81 scheme both were the directory
        `feature-auth` -- five verified preimages of that exact leaf are on
        record (5db5c42). A devpod record sourced there names both clones and
        chooses neither: resolving it would be dict insertion order deciding
        which branch's checkout a workspace reopens, and the wrong answer is
        indistinguishable from the right one afterwards.
        """
        flattened = world.clone("feature/auth")
        literal = world.clone("feature-auth")
        world.record("feature/auth", flattened)
        world.record("feature-auth", literal)
        world.workspace("devlaunch-feature-auth", world.repo_dir / "feature-auth")

        status, _ = reconcile(world, "-y")
        out = capsys.readouterr().out

        assert status == 0
        assert world.sourced_at("devlaunch-feature-auth") == str(world.repo_dir / "feature-auth")
        assert world.stored_id("feature/auth") is None
        assert world.stored_id("feature-auth") is None
        # Both sides of the ambiguity are named, because a report that said only
        # "ambiguous" would leave the user unable to settle it by hand.
        assert "devlaunch-feature-auth" in out
        assert str(flattened) in out
        assert str(literal) in out

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


class TestTheReportDoesNotDependOnWhereItWasRun:
    """devlaunch#224, at the surface it was found on.

    Run from inside `<root>/<owner>/<repo>/`, this command listed every
    git-URL-sourced workspace on the machine as that repository's orphan: a
    remote is relative-looking text, so resolving it as a path resolved it
    against the current directory and landed it in the tree the user happened to
    be standing in. Nothing was ever re-pointed -- no clone's directory name can
    equal a URL -- so the whole of it was a report about workspaces that had
    nothing to do with the repository named.
    """

    def test_a_workspace_devpod_sources_from_a_remote_is_no_repositorys_orphan(
        self, world, capsys, monkeypatch
    ):
        world.remote_workspace("wayfinder", "git@github.com:blooop/wayfinder.git")
        monkeypatch.chdir(world.repo_dir)

        status, devpod = reconcile(world, "-y")

        assert status == 0
        assert "Nothing to reconcile." in capsys.readouterr().out
        assert "delete" not in devpod.subcommands()

    def test_standing_in_a_repository_does_not_change_what_is_reported(
        self, world, capsys, monkeypatch
    ):
        """The property, rather than one instance of it: same workspaces, two
        directories, one report.

        A real orphan stands beside the remote so the comparison is between two
        reports with something in them, not between two empty ones -- an
        equality that holds because both runs found nothing would hold with the
        bug fully back.
        """
        world.workspace("devlaunch-gone", world.repo_dir / "gone")
        world.remote_workspace("wayfinder", "https://github.com/blooop/wayfinder.git")
        monkeypatch.chdir(world.devpod_home)

        reconcile(world, "-y")
        outside = capsys.readouterr().out
        monkeypatch.chdir(world.repo_dir)
        reconcile(world, "-y")

        assert "devlaunch-gone" in outside
        assert capsys.readouterr().out == outside


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


# root is refused by nothing, so under root these would pass with the behaviour
# they guard fully reverted. Same reasoning as test_prune_orphaned_clones.py's.
needs_an_unprivileged_user = pytest.mark.skipif(
    os.geteuid() == 0, reason="root can write any directory, so nothing here can refuse"
)


class TestARepairThatCannotBeMadeIsNotHalfMade:
    """The record is only told about a workspace devpod's record now agrees with.

    devpod's file is rewritten first and metadata's id second, so a failure
    stops before the second. The other order would leave `dl` following a record
    to a workspace still sourced at a dead path -- which is the fault this
    command exists to clear, reintroduced by the command itself.
    """

    def test_a_devpod_record_that_is_not_there_is_reported_and_nothing_is_claimed(self, world):
        """devpod lists the workspace; its own file for it is gone.

        A partially-removed devpod home, which is exactly the kind of state a
        machine needing this command is already in.
        """
        clone = world.clone("main")
        world.record("main", clone)
        path = world.workspace("devlaunch-main", world.repo_dir / "main")
        path.unlink()

        status, _ = reconcile(world, "-y")

        assert status == 1
        assert world.stored_id("main") is None

    def test_a_record_shaped_in_a_way_dl_cannot_read_is_left_alone(self, world):
        """A `source` that is not an object is not one dl can safely replace."""
        clone = world.clone("main")
        world.record("main", clone)
        path = world.workspace("devlaunch-main", world.repo_dir / "main")
        path.write_text(json.dumps({"id": "devlaunch-main", "source": "somewhere"}))

        status, _ = reconcile(world, "-y")

        assert status == 1
        assert json.loads(path.read_text())["source"] == "somewhere"
        assert world.stored_id("main") is None

    @needs_an_unprivileged_user
    def test_a_write_that_is_refused_leaves_devpods_record_whole(self, world, refuses_writes):
        """The rename is over a temp file in the same directory, so a refusal
        cannot truncate what devpod had."""
        clone = world.clone("main")
        world.record("main", clone)
        path = world.workspace("devlaunch-main", world.repo_dir / "main")
        before = path.read_text()
        refuses_writes(path.parent)

        status, _ = reconcile(world, "-y")

        assert status == 1
        assert path.read_text() == before
        assert world.stored_id("main") is None


class TestSourcesThisCommandCannotFollow:
    """A live workspace dl cannot place stops the command, exactly as in `--prune`.

    The two commands ask one question of an unfollowable source -- which clone
    might this be holding? -- and get one answer: any of them. `--prune` stops
    because it cannot call a clone unreferenced; this stops because it cannot
    call one free to give away, and handing a dead record a directory a working
    workspace turns out to open is the collision the claimed-clone check exists
    to prevent, reached by not knowing instead of by guessing.
    """

    def test_a_local_folder_devpod_filled_with_an_object_stops_the_command(self, world):
        clone = world.clone("main")
        world.record("main", clone)
        world.workspace("devlaunch-main", world.repo_dir / "main")
        world.listed.append({"id": "unreadable", "source": {"localFolder": {"nested": True}}})

        status, devpod = reconcile(world, "-y")

        assert status == 1
        assert world.sourced_at("devlaunch-main") == str(world.repo_dir / "main")
        assert world.stored_id("main") is None
        assert "delete" not in devpod.subcommands()

    def test_the_refusal_is_the_report_prune_gives_named_for_this_command(self, world, capsys):
        """Same shape, same sentence, this command's name and this command's
        outcome -- so that having read it once is having read it for both."""
        world.listed.append({"id": "unreadable", "source": {"localFolder": {"nested": True}}})

        status, _ = reconcile(world, "-y")
        out = capsys.readouterr().out

        assert status == 1
        assert "dl --reconcile cannot follow these live workspaces' sources:" in out
        assert "unreadable" in out
        assert "Nothing was re-pointed" in out

    def test_a_source_no_filesystem_call_will_accept_stops_the_command(self, world):
        clone = world.clone("main")
        world.record("main", clone)
        world.workspace("devlaunch-main", world.repo_dir / "main")
        world.listed.append({"id": "unusable", "source": {"localFolder": "/no\0such/path"}})

        status, _ = reconcile(world, "-y")

        assert status == 1
        assert world.sourced_at("devlaunch-main") == str(world.repo_dir / "main")
        assert world.stored_id("main") is None

    def test_a_record_dl_cannot_name_a_directory_for_is_no_candidate(self, world, capsys):
        """An empty path and a branch the derivation refuses: no directory at
        all, so nothing can be adopted into it."""
        from devlaunch.worktree.models import WorktreeInfo  # pylint: disable=import-outside-toplevel
        from devlaunch.worktree.storage import (  # pylint: disable=import-outside-toplevel
            MetadataStorage,
        )

        MetadataStorage(world.cache / "metadata.json").add_worktree(
            WorktreeInfo(
                owner=OWNER,
                repo=REPO,
                branch="--evil",
                local_path=Path(""),
                workspace_id="whatever",
            )
        )
        world.workspace("devlaunch-main", world.repo_dir / "main")

        status, _ = reconcile(world, "-y")

        assert status == 0
        assert "devlaunch-main" in capsys.readouterr().out


class TestOptionsAreRefusedRatherThanIgnored:
    """`--prune`'s rule: an option that reads as a rehearsal must not act."""

    def test_an_unknown_option_stops_the_command(self, world):
        clone = world.clone("main")
        world.record("main", clone)
        world.workspace("devlaunch-main", world.repo_dir / "main")

        status, devpod = reconcile(world, "--dry-run", "-y")

        assert status == 1
        assert devpod.subcommands() == []
        assert world.sourced_at("devlaunch-main") == str(world.repo_dir / "main")
