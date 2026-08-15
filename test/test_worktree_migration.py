"""Tests for the one-shot migration of a cache onto the current id scheme.

Every test here builds its own cache under ``tmp_path``. Nothing in this file may
read or write the real ``~/.cache/devlaunch``: the migration renames directories
that can hold uncommitted work.
"""
# pylint: disable=redefined-outer-name

import json
import subprocess
from pathlib import Path

import pytest

from devlaunch import dl
from devlaunch.workspace_id import WorkspaceId
from devlaunch.worktree.migration import (
    ORPHAN_LIST_NAME,
    UNMIGRATED_LIST_NAME,
    migrate_cache,
)
from devlaunch.worktree.storage import SCHEMA_VERSION, MetadataStorage


def _old_workspace_id(repo: str, branch: str) -> str:
    """The pre-#64 devpod id: repo and branch flattened, no identity suffix."""
    return f"{repo}-{branch}".replace("/", "-").replace("_", "-")


def _old_leaf(branch: str) -> str:
    """The pre-#64 clone-directory leaf: the flattened branch name alone."""
    return branch.replace("/", "-")


def _repo_entry(owner: str, repo: str, repo_root: Path, branches) -> dict:
    return {
        "owner": owner,
        "repo": repo,
        "remote_url": f"git@github.com:{owner}/{repo}.git",
        "local_path": str(repo_root / ".bare"),
        "default_branch": "main",
        "last_fetched": None,
        "worktrees": list(branches),
    }


def _worktree_entry(owner: str, repo: str, branch: str, local_path: Path, **overrides) -> dict:
    entry = {
        "owner": owner,
        "repo": repo,
        "branch": branch,
        "local_path": str(local_path),
        "workspace_id": _old_workspace_id(repo, branch),
        "created_at": "2024-01-01T10:00:00",
        "last_used": "2024-01-01T12:00:00",
        "devpod_workspace_id": None,
    }
    entry.update(overrides)
    return entry


def build_legacy_cache(
    root: Path,
    layout,
    version=1,
    unrecorded=(),
    make_dirs=True,
):
    """Build an old-scheme cache under *root* and return (metadata_path, repos_dir).

    ``layout`` maps ``(owner, repo)`` to a list of branch names, each of which gets
    a clone directory named the old way plus a matching metadata record.
    ``unrecorded`` is a list of ``(owner, repo, leaf)`` directories with no record.
    """
    devlaunch_dir = root / "devlaunch"
    repos_dir = devlaunch_dir / "repos"
    repositories = {}
    worktrees = {}

    for (owner, repo), branches in layout.items():
        repo_root = repos_dir / owner / repo
        (repo_root / ".bare").mkdir(parents=True, exist_ok=True)
        repositories[f"{owner}/{repo}"] = _repo_entry(owner, repo, repo_root, branches)
        for branch in branches:
            clone = repo_root / _old_leaf(branch)
            if make_dirs:
                (clone / ".git").mkdir(parents=True, exist_ok=True)
            worktrees[f"{owner}/{repo}/{branch}"] = _worktree_entry(owner, repo, branch, clone)

    for owner, repo, leaf in unrecorded:
        (repos_dir / owner / repo / leaf / ".git").mkdir(parents=True, exist_ok=True)

    data = {"repositories": repositories, "worktrees": worktrees}
    if version is not None:
        data["version"] = version
    devlaunch_dir.mkdir(parents=True, exist_ok=True)
    metadata_path = devlaunch_dir / "metadata.json"
    metadata_path.write_text(json.dumps(data, indent=2), encoding="utf-8")
    return metadata_path, repos_dir


def run_migration(metadata_path: Path, repos_dir: Path):
    """Load the cache from disk and migrate it, as a fresh dl invocation would."""
    storage = MetadataStorage(metadata_path)
    return migrate_cache(storage, repos_dir)


def on_disk(metadata_path: Path) -> dict:
    return json.loads(metadata_path.read_text(encoding="utf-8"))


def leaves(repo_root: Path):
    return sorted(p.name for p in repo_root.iterdir())


def new_leaf(owner: str, repo: str, branch: str) -> str:
    return WorkspaceId(owner, repo, branch).value


@pytest.fixture
def simple_cache(tmp_path):
    """One repo, three branches, no surprises."""
    metadata_path, repos_dir = build_legacy_cache(
        tmp_path, {("blooop", "devlaunch"): ["main", "feature/auth", "aid_auto_2"]}
    )
    return metadata_path, repos_dir


class TestRenaming:
    """Old-scheme clone directories are renamed to the derived id, once."""

    def test_dirs_are_renamed_to_the_derived_id(self, simple_cache):
        metadata_path, repos_dir = simple_cache
        repo_root = repos_dir / "blooop" / "devlaunch"

        report = run_migration(metadata_path, repos_dir)

        assert report is not None
        assert len(report.renamed) == 3
        assert leaves(repo_root) == sorted(
            [
                ".bare",
                new_leaf("blooop", "devlaunch", "main"),
                new_leaf("blooop", "devlaunch", "feature/auth"),
                new_leaf("blooop", "devlaunch", "aid_auto_2"),
            ]
        )

    def test_bare_is_never_renamed(self, simple_cache):
        metadata_path, repos_dir = simple_cache
        bare = repos_dir / "blooop" / "devlaunch" / ".bare"

        report = run_migration(metadata_path, repos_dir)

        assert bare.is_dir()
        assert all(src.name != ".bare" for src, _ in report.renamed)
        assert all(dest.name != ".bare" for _, dest in report.renamed)
        assert bare not in report.unmigrated

    def test_metadata_agrees_with_the_filesystem(self, simple_cache):
        metadata_path, repos_dir = simple_cache

        run_migration(metadata_path, repos_dir)

        stored = on_disk(metadata_path)
        assert stored["version"] == SCHEMA_VERSION
        for key, entry in stored["worktrees"].items():
            owner, repo, branch = key.split("/", 2)
            expected = repos_dir / owner / repo / new_leaf(owner, repo, branch)
            assert Path(entry["local_path"]) == expected
            assert Path(entry["local_path"]).is_dir()
            assert entry["workspace_id"] == new_leaf(owner, repo, branch)

    def test_second_run_is_a_no_op(self, simple_cache, capsys):
        metadata_path, repos_dir = simple_cache
        run_migration(metadata_path, repos_dir)
        capsys.readouterr()
        before = on_disk(metadata_path)
        listing = sorted(p.name for p in (repos_dir / "blooop" / "devlaunch").iterdir())

        assert run_migration(metadata_path, repos_dir) is None

        assert on_disk(metadata_path) == before
        assert sorted(p.name for p in (repos_dir / "blooop" / "devlaunch").iterdir()) == listing
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""

    def test_a_cache_already_on_the_new_scheme_only_gains_the_version(self, tmp_path):
        """Nothing to rename, but the header still has to move off version 1."""
        metadata_path, repos_dir = build_legacy_cache(tmp_path, {})
        report = run_migration(metadata_path, repos_dir)

        assert report is not None
        assert report.renamed == []
        assert on_disk(metadata_path)["version"] == SCHEMA_VERSION

    def test_an_absent_version_header_still_triggers_the_migration(self, tmp_path):
        """A pre-versioning file is version 1, not the current version."""
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path, {("blooop", "devlaunch"): ["main"]}, version=None
        )

        report = run_migration(metadata_path, repos_dir)

        assert report is not None
        assert len(report.renamed) == 1

    def test_a_newer_file_is_never_migrated(self, tmp_path):
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path, {("blooop", "devlaunch"): ["main"]}, version=SCHEMA_VERSION + 1
        )

        assert run_migration(metadata_path, repos_dir) is None
        assert (repos_dir / "blooop" / "devlaunch" / "main").is_dir()

    def test_the_stored_path_is_the_source_not_a_recomputed_one(self, tmp_path):
        """A record whose directory sits somewhere unexpected is still followed."""
        metadata_path, repos_dir = build_legacy_cache(tmp_path, {("blooop", "devlaunch"): ["main"]})
        repo_root = repos_dir / "blooop" / "devlaunch"
        odd = repo_root / "not-the-branch-name"
        (repo_root / "main").rename(odd)
        data = on_disk(metadata_path)
        data["worktrees"]["blooop/devlaunch/main"]["local_path"] = str(odd)
        metadata_path.write_text(json.dumps(data), encoding="utf-8")

        report = run_migration(metadata_path, repos_dir)

        assert report.renamed == [(odd, repo_root / new_leaf("blooop", "devlaunch", "main"))]
        assert not odd.exists()


class TestUncommittedWork:
    """The reason the strategy is rename-not-orphan."""

    @staticmethod
    def _git(cwd: Path, *args: str) -> str:
        result = subprocess.run(
            ["git", "-c", "user.email=t@t", "-c", "user.name=t", *args],
            cwd=cwd,
            capture_output=True,
            text=True,
            check=True,
        )
        return result.stdout

    def test_uncommitted_changes_survive_the_rename(self, tmp_path):
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path, {("blooop", "devlaunch"): ["main"]}, make_dirs=False
        )
        repo_root = repos_dir / "blooop" / "devlaunch"
        bare = repo_root / ".bare"
        subprocess.run(
            ["git", "init", "--bare", "-b", "main", str(bare)], check=True, capture_output=True
        )
        clone = repo_root / "main"
        subprocess.run(["git", "clone", str(bare), str(clone)], check=True, capture_output=True)
        (clone / "committed.txt").write_text("committed\n", encoding="utf-8")
        self._git(clone, "add", "committed.txt")
        self._git(clone, "commit", "-m", "first")
        self._git(clone, "push", "origin", "main")
        (clone / "work-in-progress.txt").write_text("do not lose me\n", encoding="utf-8")
        (clone / "committed.txt").write_text("edited but not committed\n", encoding="utf-8")
        assert self._git(clone, "status", "--porcelain") != ""

        run_migration(metadata_path, repos_dir)

        moved = repo_root / new_leaf("blooop", "devlaunch", "main")
        assert not clone.exists()
        assert (moved / "work-in-progress.txt").read_text(encoding="utf-8") == "do not lose me\n"
        assert (moved / "committed.txt").read_text(encoding="utf-8") == "edited but not committed\n"
        status = self._git(moved, "status", "--porcelain")
        assert " M committed.txt" in status
        assert "?? work-in-progress.txt" in status
        # The remote still points at .bare, which did not move, so the clone is
        # not just present but still functional.
        assert self._git(moved, "log", "--oneline").strip().endswith("first")
        assert self._git(moved, "fetch", "origin") == ""


class TestDirectoriesTheMigrationWillNotTouch:
    """Collisions, record-less directories and stale records."""

    def test_a_destination_collision_renames_nothing(self, tmp_path, capsys):
        metadata_path, repos_dir = build_legacy_cache(tmp_path, {("blooop", "devlaunch"): ["main"]})
        repo_root = repos_dir / "blooop" / "devlaunch"
        old = repo_root / "main"
        already = repo_root / new_leaf("blooop", "devlaunch", "main")
        (already / ".git").mkdir(parents=True)
        (already / "marker.txt").write_text("new scheme clone\n", encoding="utf-8")
        (old / "marker.txt").write_text("old scheme clone\n", encoding="utf-8")

        report = run_migration(metadata_path, repos_dir)

        assert report.renamed == []
        assert old.is_dir()
        assert (old / "marker.txt").read_text(encoding="utf-8") == "old scheme clone\n"
        assert (already / "marker.txt").read_text(encoding="utf-8") == "new scheme clone\n"
        assert old in report.unmigrated
        # The record follows the canonically named clone, so a later `dl ... rm`
        # deletes the directory devpod is actually using.
        entry = on_disk(metadata_path)["worktrees"]["blooop/devlaunch/main"]
        assert Path(entry["local_path"]) == already
        assert str(old) in (repos_dir.parent / UNMIGRATED_LIST_NAME).read_text(encoding="utf-8")
        assert "no metadata record" in capsys.readouterr().err

    def test_record_less_directories_are_left_and_listed(self, tmp_path, capsys):
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path,
            {("blooop", "devlaunch"): ["main"]},
            unrecorded=[("blooop", "devlaunch", "orphan-dir"), ("blooop", "bencher", "w1")],
        )

        report = run_migration(metadata_path, repos_dir)

        orphan = repos_dir / "blooop" / "devlaunch" / "orphan-dir"
        stray = repos_dir / "blooop" / "bencher" / "w1"
        assert orphan.is_dir() and stray.is_dir()
        assert sorted(report.unmigrated) == sorted([orphan, stray])
        listing = (repos_dir.parent / UNMIGRATED_LIST_NAME).read_text(encoding="utf-8")
        assert str(orphan) in listing
        assert str(stray) in listing
        err = capsys.readouterr().err
        assert "2 clone director" in err
        assert UNMIGRATED_LIST_NAME in err

    def test_no_listing_file_is_written_when_every_directory_migrated(self, simple_cache):
        metadata_path, repos_dir = simple_cache

        run_migration(metadata_path, repos_dir)

        assert not (repos_dir.parent / UNMIGRATED_LIST_NAME).exists()

    def test_a_record_whose_directory_is_gone_does_not_fail_the_run(self, tmp_path, capsys):
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path, {("blooop", "devlaunch"): ["main", "w1"]}
        )
        gone = repos_dir / "blooop" / "devlaunch" / "w1"
        (gone / ".git").rmdir()
        gone.rmdir()

        report = run_migration(metadata_path, repos_dir)

        assert report.missing == [gone]
        assert len(report.renamed) == 1
        entry = on_disk(metadata_path)["worktrees"]["blooop/devlaunch/w1"]
        assert Path(entry["local_path"]).name == new_leaf("blooop", "devlaunch", "w1")
        assert "no longer there" in capsys.readouterr().err

    def test_a_record_with_an_unusable_branch_is_left_alone(self, tmp_path, capsys):
        """The old derivation coerced bad refs, so a stored branch may not be one."""
        metadata_path, repos_dir = build_legacy_cache(tmp_path, {("blooop", "devlaunch"): ["main"]})
        repo_root = repos_dir / "blooop" / "devlaunch"
        bad = repo_root / "feature-auth"
        (bad / ".git").mkdir(parents=True)
        data = on_disk(metadata_path)
        data["worktrees"]["blooop/devlaunch/feature auth"] = _worktree_entry(
            "blooop", "devlaunch", "feature auth", bad
        )
        metadata_path.write_text(json.dumps(data), encoding="utf-8")

        report = run_migration(metadata_path, repos_dir)

        assert bad.is_dir()
        assert report.unusable == [(bad, "feature auth")]
        entry = on_disk(metadata_path)["worktrees"]["blooop/devlaunch/feature auth"]
        assert Path(entry["local_path"]) == bad
        assert entry["workspace_id"] == _old_workspace_id("devlaunch", "feature auth")
        assert "feature auth" in capsys.readouterr().err

    def test_a_branch_named_after_another_branchs_derived_id_is_refused(self, tmp_path, capsys):
        """#55's `foo-bexoza` case: never adopt a clone another record owns."""
        squatter = new_leaf("blooop", "devlaunch", "main")
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path, {("blooop", "devlaunch"): ["main", squatter]}
        )
        repo_root = repos_dir / "blooop" / "devlaunch"

        report = run_migration(metadata_path, repos_dir)

        assert report.blocked == [(repo_root / "main", repo_root / squatter)]
        assert (repo_root / "main").is_dir()
        main_entry = on_disk(metadata_path)["worktrees"]["blooop/devlaunch/main"]
        assert Path(main_entry["local_path"]) == repo_root / "main"
        assert main_entry["workspace_id"] == _old_workspace_id("devlaunch", "main")
        # The other record migrated normally, out of the way.
        other = on_disk(metadata_path)["worktrees"][f"blooop/devlaunch/{squatter}"]
        assert Path(other["local_path"]).name == new_leaf("blooop", "devlaunch", squatter)
        assert "already another workspace's clone directory" in capsys.readouterr().err


class TestInterruption:
    """An interrupted run leaves a resumable state, never stale metadata."""

    def test_a_crash_between_the_renames_and_the_save_is_resumable(self, tmp_path, capsys):
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path, {("blooop", "devlaunch"): ["main", "w1"]}
        )
        repo_root = repos_dir / "blooop" / "devlaunch"
        # What a crash after the first rename and before the single save leaves:
        # one directory on the new scheme, the file untouched at version 1.
        (repo_root / "main").rename(repo_root / new_leaf("blooop", "devlaunch", "main"))
        assert on_disk(metadata_path)["version"] == 1
        capsys.readouterr()

        report = run_migration(metadata_path, repos_dir)

        assert report.renamed == [
            (repo_root / "w1", repo_root / new_leaf("blooop", "devlaunch", "w1"))
        ]
        assert report.unmigrated == []
        assert report.missing == []
        stored = on_disk(metadata_path)
        assert stored["version"] == SCHEMA_VERSION
        for key, entry in stored["worktrees"].items():
            owner, repo, branch = key.split("/", 2)
            assert Path(entry["local_path"]) == repo_root / new_leaf(owner, repo, branch)
            assert Path(entry["local_path"]).is_dir()

    def test_the_version_is_written_in_the_same_save_as_the_paths(self, simple_cache, monkeypatch):
        """No intermediate save may claim the migration finished."""
        metadata_path, repos_dir = simple_cache
        saves = []
        real_save = MetadataStorage.save

        def counting_save(self):
            saves.append(on_disk(metadata_path)["version"])
            real_save(self)

        monkeypatch.setattr(MetadataStorage, "save", counting_save)

        run_migration(metadata_path, repos_dir)

        # One save, and it saw version 1 on disk: no earlier write bumped the
        # header while directories were still being renamed.
        assert saves == [1]


class TestOrphanedContainers:
    """Old containers keep their ids; dl says so and never deletes them."""

    def test_the_notice_names_the_count_and_a_cleanup_command(self, simple_cache, capsys):
        metadata_path, repos_dir = simple_cache

        report = run_migration(metadata_path, repos_dir)

        assert sorted(report.orphaned_ids) == sorted(
            [
                _old_workspace_id("devlaunch", "main"),
                _old_workspace_id("devlaunch", "feature/auth"),
                _old_workspace_id("devlaunch", "aid_auto_2"),
            ]
        )
        listing = repos_dir.parent / ORPHAN_LIST_NAME
        assert listing.read_text(encoding="utf-8").splitlines() == sorted(report.orphaned_ids)
        err = capsys.readouterr().err
        notice = [line for line in err.splitlines() if "orphan" in line]
        assert len(notice) == 1
        assert "3 devpod" in notice[0]
        assert f"xargs -r -n1 devpod delete < {listing}" in notice[0]

    def test_no_notice_when_no_id_changed(self, tmp_path, capsys):
        """Records already carrying the derived id have no orphaned container."""
        metadata_path, repos_dir = build_legacy_cache(tmp_path, {})
        repo_root = repos_dir / "blooop" / "devlaunch"
        leaf = new_leaf("blooop", "devlaunch", "main")
        (repo_root / ".bare").mkdir(parents=True)
        (repo_root / leaf / ".git").mkdir(parents=True)
        data = on_disk(metadata_path)
        data["worktrees"]["blooop/devlaunch/main"] = _worktree_entry(
            "blooop", "devlaunch", "main", repo_root / leaf, workspace_id=leaf
        )
        metadata_path.write_text(json.dumps(data), encoding="utf-8")

        report = run_migration(metadata_path, repos_dir)

        assert report.orphaned_ids == []
        assert not (repos_dir.parent / ORPHAN_LIST_NAME).exists()
        assert "orphan" not in capsys.readouterr().err

    def test_the_notice_costs_no_devpod_call(self, simple_cache, monkeypatch):
        """The orphan ids come from metadata, so migration spawns no devpod."""
        calls = []
        monkeypatch.setattr(dl, "run_devpod", lambda *a, **k: calls.append(a))

        metadata_path, repos_dir = simple_cache
        run_migration(metadata_path, repos_dir)

        assert calls == []

    def test_no_container_is_deleted(self, simple_cache, monkeypatch):
        monkeypatch.setattr(
            subprocess,
            "run",
            lambda *a, **k: pytest.fail(f"migration must not run a subprocess: {a}"),
        )
        metadata_path, repos_dir = simple_cache
        run_migration(metadata_path, repos_dir)


class TestRealisticCache:
    """A fixture shaped like the cache this migration was written for."""

    LAYOUT = {
        ("blooop", "bencher"): [
            "main",
            "w1",
            "w2",
            "w3",
            "asdf1",
            "asdf2",
            "test-dotfix",
            "rerun30",
            "update",
            "tmp",
        ],
        ("blooop", "devlaunch"): ["main", "aid", "aid_auto_2", "bugfix1", "slow_install"],
        ("blooop", "python_template"): ["main", "ws1", "ws3", "ws99", "prek", "wsnew"],
        ("blooop", "wayfinder"): ["main", "format", "devlaunch"],
        ("blooop", "rockerc"): ["main", "nb3"],
    }
    UNRECORDED = [("blooop", "bencher", "ws9"), ("blooop", "wayfinder", "leftover")]

    def test_end_to_end(self, tmp_path, capsys):
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path, self.LAYOUT, unrecorded=self.UNRECORDED
        )
        expected_renames = sum(len(v) for v in self.LAYOUT.values())

        report = run_migration(metadata_path, repos_dir)
        first_run_err = capsys.readouterr().err

        assert len(report.renamed) == expected_renames
        assert len(report.orphaned_ids) == expected_renames
        assert len(report.unmigrated) == len(self.UNRECORDED)
        assert report.missing == []
        assert report.unusable == []
        assert report.blocked == []
        assert report.failed == []

        # Every recorded directory is where metadata says it is, and every leaf is
        # globally unique now rather than unique only within its parent.
        stored = on_disk(metadata_path)
        assert stored["version"] == SCHEMA_VERSION
        all_leaves = []
        for key, entry in stored["worktrees"].items():
            owner, repo, branch = key.split("/", 2)
            path = Path(entry["local_path"])
            assert path.is_dir()
            assert path == repos_dir / owner / repo / new_leaf(owner, repo, branch)
            assert entry["workspace_id"] == path.name
            all_leaves.append(path.name)
        assert len(set(all_leaves)) == len(all_leaves) == expected_renames

        for owner, repo in self.LAYOUT:
            assert (repos_dir / owner / repo / ".bare").is_dir()
        for owner, repo, leaf in self.UNRECORDED:
            assert (repos_dir / owner / repo / leaf).is_dir()

        assert f"migrated {expected_renames} workspace clone" in first_run_err
        assert f"{expected_renames} devpod container" in first_run_err

        # Second run: nothing to do, nothing said.
        assert run_migration(metadata_path, repos_dir) is None
        assert capsys.readouterr().err == ""


class TestWiring:
    """Where the migration runs from."""

    @pytest.fixture
    def isolated_cache(self, tmp_path, monkeypatch):
        """Point every devlaunch path at tmp_path and forget any cached manager."""
        monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path))
        monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "config"))
        monkeypatch.setattr(dl, "_cache", {})
        return build_legacy_cache(tmp_path, {("blooop", "devlaunch"): ["main"]})

    def test_the_clone_manager_factory_migrates(self, isolated_cache):
        metadata_path, repos_dir = isolated_cache

        manager = dl._get_clone_manager()  # pylint: disable=protected-access

        assert (
            repos_dir / "blooop" / "devlaunch" / new_leaf("blooop", "devlaunch", "main")
        ).is_dir()
        assert on_disk(metadata_path)["version"] == SCHEMA_VERSION
        # The manager the caller gets back agrees with the migrated cache.
        assert manager.get_workspace_path("blooop", "devlaunch", "main").is_dir()

    @pytest.mark.usefixtures("isolated_cache")
    def test_the_factory_migrates_only_once_per_process(self, monkeypatch):
        calls = []
        real = dl.migrate_cache
        monkeypatch.setattr(
            dl, "migrate_cache", lambda *a, **k: (calls.append(a), real(*a, **k))[1]
        )

        dl._get_clone_manager()  # pylint: disable=protected-access
        dl._get_clone_manager()  # pylint: disable=protected-access

        assert len(calls) == 1

    @pytest.mark.parametrize("argv", [["--help"], ["-h"], ["--version"]])
    def test_help_and_version_never_migrate(self, isolated_cache, monkeypatch, argv):
        metadata_path, repos_dir = isolated_cache
        monkeypatch.setattr(
            dl, "migrate_cache", lambda *a, **k: pytest.fail("must not migrate on " + argv[0])
        )

        assert dl.main(argv) == 0

        assert (repos_dir / "blooop" / "devlaunch" / "main").is_dir()
        assert on_disk(metadata_path)["version"] == 1


class TestWhatTheFilesystemRefuses:
    """The three ``except OSError`` arms, driven by a filesystem that says no.

    Nothing here is exotic: a cache on a read-only mount, a directory whose
    permissions someone tightened, a disk with nothing left on it. What they
    have in common is that the migration is a *whole-cache* operation running
    before the command the user typed — so the standard it is held to is that
    one refusal costs the run one directory, never the run.

    Refusals are arranged with real permissions and real inode types rather
    than a patched ``os.rename``, because what is being tested is precisely
    that the arm catching them catches what the operating system actually
    raises. The permission ones go through ``refuses_writes``/``refuses_reads``,
    which *verify* the refusal and skip where the filesystem does not enforce
    modes — a `geteuid` check would leave contributors on a Docker Desktop
    bind mount with two red tests and no defect.
    """

    def test_a_rename_the_filesystem_refuses_costs_one_directory(
        self, simple_cache, capsys, refuses_writes
    ):
        metadata_path, repos_dir = simple_cache
        repo_root = repos_dir / "blooop" / "devlaunch"
        before = leaves(repo_root)
        refuses_writes(repo_root)

        report = run_migration(metadata_path, repos_dir)

        assert len(report.failed) == 3, "every rename in the locked directory was refused"
        assert not report.renamed
        for src, dest, exc in report.failed:
            assert isinstance(exc, OSError)
            assert src.parent == dest.parent == repo_root

        # The directories are still where they were, which is the whole point:
        # a clone that could not be renamed holds work, and the migration would
        # rather leave it under a name nothing looks for than lose track of it.
        assert leaves(repo_root) == before

        # And the record still points at the directory that is really there. A
        # record repointed at a name the rename did not produce would send the
        # next `dl ... rm` at a path with nothing in it.
        stored = on_disk(metadata_path)["worktrees"]
        for record in stored.values():
            assert Path(record["local_path"]).is_dir()

        said = capsys.readouterr().err
        assert said.count("could not rename") == 3
        assert "it was left where it is" in said

    def test_a_refused_rename_leaves_the_header_behind_until_the_retry_succeeds(
        self, tmp_path, capsys, refuses_writes
    ):
        # The consequence the test above stops one line short of. A refused
        # rename is not a crash, but it has to be survivable the same way: the
        # header is what the next run's version comparison reads, so a run that
        # left a directory under its old name must leave the header at 1 too.
        # Advancing it would strand those records on their pre-#64
        # `workspace_id` forever -- `remove_workspace_by_id` matches on exactly
        # the id dl derives today, so `dl acme/widgets@main rm` could never find
        # them again, and the clone plus its record would be silently orphaned
        # while the container went (blooop/devlaunch#180).
        #
        # Two repos, one of them locked: the refusal has to be *partial* to show
        # that the save still happens. The successful renames are recorded
        # immediately -- declining the save entirely would lose them.
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path,
            {("blooop", "devlaunch"): ["main"], ("acme", "widgets"): ["main", "release"]},
        )
        locked = repos_dir / "acme" / "widgets"
        refuses_writes(locked)

        report = run_migration(metadata_path, repos_dir)

        assert len(report.renamed) == 1
        assert len(report.failed) == 2
        refused = sorted(str(src) for src, _, _ in report.failed)

        cache = on_disk(metadata_path)
        assert cache["version"] == 1, "the header may not claim more than the filesystem has done"

        # The half that worked is on disk, not just in memory: the record moved
        # with its directory and carries the id dl derives today.
        done = cache["worktrees"]["blooop/devlaunch/main"]
        assert Path(done["local_path"]) == (
            repos_dir / "blooop" / "devlaunch" / new_leaf("blooop", "devlaunch", "main")
        )
        assert Path(done["local_path"]).is_dir()
        assert done["workspace_id"] == new_leaf("blooop", "devlaunch", "main")

        # The half that was refused still points at the directory that is really
        # there, under its old name and its old id.
        for key in ("acme/widgets/main", "acme/widgets/release"):
            left = cache["worktrees"][key]
            assert Path(left["local_path"]).is_dir()
            assert left["workspace_id"] == _old_workspace_id("widgets", left["branch"])

        # A second run picks the cache back up and retries *exactly* the refused
        # set: the already-renamed clone is the documented "destination present,
        # source gone" resume, which is caught up to without a second rename.
        capsys.readouterr()
        second = run_migration(metadata_path, repos_dir)

        assert second is not None, "the header at 1 is what lets the next run in"
        assert second.renamed == []
        assert sorted(str(src) for src, _, _ in second.failed) == refused

        # The notice repeats on every run until the refusal is fixed by hand --
        # the accepted cost of keeping the header honest (#180).
        assert capsys.readouterr().err.count("could not rename") == 2

        # And when the refusal lifts, the retry completes and only then does the
        # header advance -- the records were recoverable the whole time.
        locked.chmod(0o700)
        third = run_migration(metadata_path, repos_dir)

        assert len(third.renamed) == 2
        assert third.failed == []
        finished = on_disk(metadata_path)
        assert finished["version"] == SCHEMA_VERSION
        for key, entry in finished["worktrees"].items():
            owner, repo, branch = key.split("/", 2)
            assert entry["workspace_id"] == new_leaf(owner, repo, branch)
            assert Path(entry["local_path"]).is_dir()

    def test_an_unrelated_save_between_runs_does_not_advance_the_header(
        self, simple_cache, capsys, refuses_writes
    ):
        """Any save, not just the migration's, has to keep the header honest.

        The migration is not the only thing that writes ``metadata.json``:
        opening a workspace or reconciling saves too, through a storage object
        loaded fresh long after the migration ran. If ``save`` stamped the
        current version the way it used to, the very next such write would
        re-strand the records a refused rename had deliberately left behind,
        and gating only the migration's own save would miss it entirely.
        """
        metadata_path, repos_dir = simple_cache
        refuses_writes(repos_dir / "blooop" / "devlaunch")
        assert run_migration(metadata_path, repos_dir).failed
        capsys.readouterr()

        # An unrelated operation, mid-life: it loads the cache and writes it back
        # knowing nothing about migrations.
        MetadataStorage(metadata_path).save()

        assert on_disk(metadata_path)["version"] == 1
        assert run_migration(metadata_path, repos_dir) is not None, "still reachable"

    def test_a_corner_of_the_cache_that_cannot_be_scanned_costs_only_that_corner(
        self, tmp_path, capsys, refuses_reads
    ):
        # The scan for record-less directories runs after the renames and over
        # the *whole* cache, so it reaches owners this run has no business with.
        # One unreadable directory there must not cost the migration the work it
        # has already done -- the renames are on disk by this point and the
        # version header has not been written yet.
        #
        # The unreadable owner is named to sort **before** the real one, and
        # that is the entire point of the name. Owners are walked in sorted
        # order, and while the whole three-level walk sat under one `try` the
        # first refusal ended the scan for every owner after it: with the
        # unreadable directory sorting last the test passed against code that
        # silently abandoned every unmigrated clone in the cache.
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path,
            {("blooop", "devlaunch"): ["main"]},
            unrecorded=[("blooop", "devlaunch", "stray-clone")],
        )
        unreadable = repos_dir / "aaa-corp"
        unreadable.mkdir()
        refuses_reads(unreadable)

        report = run_migration(metadata_path, repos_dir)

        assert report is not None
        assert len(report.renamed) == 1, "the readable half migrated"
        assert (
            repos_dir / "blooop" / "devlaunch" / new_leaf("blooop", "devlaunch", "main")
        ).is_dir()
        assert on_disk(metadata_path)["version"] == SCHEMA_VERSION

        # The scan carried on past the refusal and still found what it was for.
        assert [p.name for p in report.unmigrated] == ["stray-clone"]
        said = capsys.readouterr().err
        assert f"could not scan {unreadable}" in said
        assert "1 clone directory could not be renamed" in said

    def test_a_listing_that_cannot_be_written_still_leaves_a_usable_notice(self, tmp_path, capsys):
        # The orphaned-id listing exists to turn "12 containers are orphaned"
        # into a command the user can paste. When it cannot be written the
        # notice has to degrade to an instruction rather than to a path that is
        # not there -- naming a file that does not exist is worse than naming
        # none, because the user runs the pasted command against nothing.
        #
        # The refusal is a directory sitting where the file goes, so this needs
        # no permissions at all and holds on every filesystem.
        metadata_path, repos_dir = build_legacy_cache(tmp_path, {("blooop", "devlaunch"): ["main"]})
        (metadata_path.parent / ORPHAN_LIST_NAME).mkdir()

        report = run_migration(metadata_path, repos_dir)

        assert report.orphaned_ids, "there was something to list"
        said = capsys.readouterr().err
        assert f"could not write {metadata_path.parent / ORPHAN_LIST_NAME}" in said
        assert "devpod delete <old-id>, one per workspace" in said
        assert f"xargs -r -n1 devpod delete < {metadata_path.parent}" not in said

    def test_an_unmigrated_listing_that_cannot_be_written_still_names_the_count(
        self, tmp_path, capsys
    ):
        # The same degradation on the other listing. Both notices interpolate
        # `listing` and both have to read as sentences without it.
        metadata_path, repos_dir = build_legacy_cache(
            tmp_path,
            {("blooop", "devlaunch"): ["main"]},
            unrecorded=[("blooop", "devlaunch", "stray-clone")],
        )
        (metadata_path.parent / UNMIGRATED_LIST_NAME).mkdir()

        report = run_migration(metadata_path, repos_dir)

        assert len(report.unmigrated) == 1
        said = capsys.readouterr().err
        assert f"could not write {metadata_path.parent / UNMIGRATED_LIST_NAME}" in said
        assert "1 clone directory could not be renamed" in said
        assert "listed in" not in said
