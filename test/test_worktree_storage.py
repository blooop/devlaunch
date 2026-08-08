"""Tests for worktree metadata storage."""
# pylint: disable=redefined-outer-name

import json
import os
import stat
import tempfile
from datetime import datetime
from pathlib import Path

import pytest

from devlaunch.worktree import storage as storage_module
from devlaunch.worktree.models import BaseRepository, WorktreeInfo
from devlaunch.worktree.storage import LEGACY_SCHEMA_VERSION, SCHEMA_VERSION, MetadataStorage


@pytest.fixture
def temp_storage():
    """Create a temporary storage instance."""
    with tempfile.TemporaryDirectory() as tmpdir:
        metadata_path = Path(tmpdir) / "metadata.json"
        storage = MetadataStorage(metadata_path)
        yield storage


def _repo_entry(owner="owner1", repo="repo1", **overrides):
    """Build a valid on-disk repository entry."""
    entry = {
        "owner": owner,
        "repo": repo,
        "remote_url": f"https://github.com/{owner}/{repo}.git",
        "local_path": f"/tmp/repos/{owner}/{repo}",
        "default_branch": "main",
        "last_fetched": None,
        "worktrees": [],
    }
    entry.update(overrides)
    return entry


def _worktree_entry(owner="owner1", repo="repo1", branch="branch1", **overrides):
    """Build a valid on-disk worktree entry."""
    entry = {
        "owner": owner,
        "repo": repo,
        "branch": branch,
        "local_path": f"/tmp/worktrees/{owner}/{repo}/{branch}",
        "workspace_id": branch,
        "created_at": "2024-01-01T10:00:00",
        "last_used": "2024-01-01T12:00:00",
        "devpod_workspace_id": None,
    }
    entry.update(overrides)
    return entry


def _write_metadata(path: Path, text: str) -> None:
    """Write raw metadata file content."""
    path.write_text(text, encoding="utf-8")


def _record_replacements(monkeypatch) -> list:
    """Record the source path of every Path.replace() performed from now on."""
    seen: list = []
    real_replace = Path.replace

    def recording_replace(self, target):
        seen.append(Path(self))
        return real_replace(self, target)

    monkeypatch.setattr(Path, "replace", recording_replace)
    return seen


class TestMetadataStorage:
    """Tests for MetadataStorage class."""

    def test_init_creates_parent_dir(self):
        """Test that initialization creates parent directory."""
        with tempfile.TemporaryDirectory() as tmpdir:
            metadata_path = Path(tmpdir) / "subdir" / "metadata.json"
            storage = MetadataStorage(metadata_path)
            assert metadata_path.parent.exists()
            assert storage.metadata_path == metadata_path

    def test_init_loads_empty_state(self, temp_storage):
        """Test that initialization creates empty repositories and worktrees."""
        assert temp_storage.repositories == {}
        assert temp_storage.worktrees == {}

    def test_add_repository(self, temp_storage):
        """Test adding a repository."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
            default_branch="main",
            last_fetched=datetime(2024, 1, 1, 12, 0),
            worktrees=[],
        )

        temp_storage.add_repository(repo)

        assert "test-owner/test-repo" in temp_storage.repositories
        assert temp_storage.repositories["test-owner/test-repo"] == repo

    def test_get_repository(self, temp_storage):
        """Test getting a repository."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
            default_branch="main",
        )

        temp_storage.add_repository(repo)
        retrieved = temp_storage.get_repository("test-owner", "test-repo")

        assert retrieved is not None
        assert retrieved.owner == "test-owner"
        assert retrieved.repo == "test-repo"

    def test_get_repository_not_found(self, temp_storage):
        """Test getting a non-existent repository."""
        retrieved = temp_storage.get_repository("nonexistent", "repo")
        assert retrieved is None

    def test_list_repositories(self, temp_storage):
        """Test listing repositories."""
        repo1 = BaseRepository(
            owner="owner1",
            repo="repo1",
            remote_url="https://github.com/owner1/repo1.git",
            local_path=Path("/tmp/repos/owner1/repo1"),
        )
        repo2 = BaseRepository(
            owner="owner2",
            repo="repo2",
            remote_url="https://github.com/owner2/repo2.git",
            local_path=Path("/tmp/repos/owner2/repo2"),
        )

        temp_storage.add_repository(repo1)
        temp_storage.add_repository(repo2)

        repos = temp_storage.list_repositories()
        assert len(repos) == 2
        assert repo1 in repos
        assert repo2 in repos

    def test_remove_repository(self, temp_storage):
        """Test removing a repository."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
        )

        temp_storage.add_repository(repo)
        temp_storage.remove_repository("test-owner", "test-repo")

        assert temp_storage.get_repository("test-owner", "test-repo") is None

    def test_remove_nonexistent_repository(self, temp_storage):
        """Test removing a non-existent repository doesn't raise."""
        temp_storage.remove_repository("nonexistent", "repo")

    def test_add_worktree(self, temp_storage):
        """Test adding a worktree."""
        # First add a repository
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
            worktrees=[],
        )
        temp_storage.add_repository(repo)

        worktree = WorktreeInfo(
            owner="test-owner",
            repo="test-repo",
            branch="feature-branch",
            local_path=Path("/tmp/worktrees/test-owner/test-repo/feature-branch"),
            workspace_id="feature-branch",
            created_at=datetime(2024, 1, 1, 10, 0),
            last_used=datetime(2024, 1, 1, 12, 0),
        )

        temp_storage.add_worktree(worktree)

        assert "test-owner/test-repo/feature-branch" in temp_storage.worktrees
        # Check that the repository's worktrees list was updated
        updated_repo = temp_storage.get_repository("test-owner", "test-repo")
        assert "feature-branch" in updated_repo.worktrees

    def test_get_worktree(self, temp_storage):
        """Test getting a worktree."""
        worktree = WorktreeInfo(
            owner="test-owner",
            repo="test-repo",
            branch="feature-branch",
            local_path=Path("/tmp/worktrees/test-owner/test-repo/feature-branch"),
            workspace_id="feature-branch",
        )

        temp_storage.add_worktree(worktree)
        retrieved = temp_storage.get_worktree("test-owner", "test-repo", "feature-branch")

        assert retrieved is not None
        assert retrieved.branch == "feature-branch"

    def test_get_worktree_not_found(self, temp_storage):
        """Test getting a non-existent worktree."""
        retrieved = temp_storage.get_worktree("nonexistent", "repo", "branch")
        assert retrieved is None

    def test_list_worktrees_all(self, temp_storage):
        """Test listing all worktrees."""
        wt1 = WorktreeInfo(
            owner="owner1",
            repo="repo1",
            branch="branch1",
            local_path=Path("/tmp/worktrees/owner1/repo1/branch1"),
            workspace_id="branch1",
        )
        wt2 = WorktreeInfo(
            owner="owner2",
            repo="repo2",
            branch="branch2",
            local_path=Path("/tmp/worktrees/owner2/repo2/branch2"),
            workspace_id="branch2",
        )

        temp_storage.add_worktree(wt1)
        temp_storage.add_worktree(wt2)

        worktrees = temp_storage.list_worktrees()
        assert len(worktrees) == 2

    def test_list_worktrees_filtered_by_owner_and_repo(self, temp_storage):
        """Test listing worktrees filtered by owner and repo."""
        wt1 = WorktreeInfo(
            owner="owner1",
            repo="repo1",
            branch="branch1",
            local_path=Path("/tmp/worktrees/owner1/repo1/branch1"),
            workspace_id="branch1",
        )
        wt2 = WorktreeInfo(
            owner="owner1",
            repo="repo1",
            branch="branch2",
            local_path=Path("/tmp/worktrees/owner1/repo1/branch2"),
            workspace_id="branch2",
        )
        wt3 = WorktreeInfo(
            owner="owner2",
            repo="repo2",
            branch="branch3",
            local_path=Path("/tmp/worktrees/owner2/repo2/branch3"),
            workspace_id="branch3",
        )

        temp_storage.add_worktree(wt1)
        temp_storage.add_worktree(wt2)
        temp_storage.add_worktree(wt3)

        worktrees = temp_storage.list_worktrees(owner="owner1", repo="repo1")
        assert len(worktrees) == 2
        branches = [wt.branch for wt in worktrees]
        assert "branch1" in branches
        assert "branch2" in branches

    def test_list_worktrees_filtered_by_owner_only(self, temp_storage):
        """Test listing worktrees filtered by owner only."""
        wt1 = WorktreeInfo(
            owner="owner1",
            repo="repo1",
            branch="branch1",
            local_path=Path("/tmp/worktrees/owner1/repo1/branch1"),
            workspace_id="branch1",
        )
        wt2 = WorktreeInfo(
            owner="owner2",
            repo="repo2",
            branch="branch2",
            local_path=Path("/tmp/worktrees/owner2/repo2/branch2"),
            workspace_id="branch2",
        )

        temp_storage.add_worktree(wt1)
        temp_storage.add_worktree(wt2)

        worktrees = temp_storage.list_worktrees(owner="owner1")
        assert len(worktrees) == 1
        assert worktrees[0].owner == "owner1"

    def test_get_worktree_by_workspace_id(self, temp_storage):
        """Test looking up a worktree by workspace ID."""
        wt = WorktreeInfo(
            owner="owner1",
            repo="repo1",
            branch="main",
            local_path=Path("/tmp/worktrees/owner1/repo1/main"),
            workspace_id="repo1-main",
        )
        temp_storage.add_worktree(wt)

        result = temp_storage.get_worktree_by_workspace_id("repo1-main")
        assert result is not None
        assert result.owner == "owner1"
        assert result.branch == "main"

    def test_get_worktree_by_workspace_id_not_found(self, temp_storage):
        """Test looking up a nonexistent workspace ID returns None."""
        result = temp_storage.get_worktree_by_workspace_id("nonexistent")
        assert result is None

    def test_remove_worktree(self, temp_storage):
        """Test removing a worktree."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
            worktrees=["feature-branch"],
        )
        temp_storage.add_repository(repo)

        worktree = WorktreeInfo(
            owner="test-owner",
            repo="test-repo",
            branch="feature-branch",
            local_path=Path("/tmp/worktrees/test-owner/test-repo/feature-branch"),
            workspace_id="feature-branch",
        )
        temp_storage.add_worktree(worktree)

        temp_storage.remove_worktree("test-owner", "test-repo", "feature-branch")

        assert temp_storage.get_worktree("test-owner", "test-repo", "feature-branch") is None
        # Check that the repository's worktrees list was updated
        updated_repo = temp_storage.get_repository("test-owner", "test-repo")
        assert "feature-branch" not in updated_repo.worktrees

    def test_remove_nonexistent_worktree(self, temp_storage):
        """Test removing a non-existent worktree doesn't raise."""
        temp_storage.remove_worktree("nonexistent", "repo", "branch")

    def test_persistence(self):
        """Test that data persists across storage instances."""
        with tempfile.TemporaryDirectory() as tmpdir:
            metadata_path = Path(tmpdir) / "metadata.json"

            # Create and populate first storage instance
            storage1 = MetadataStorage(metadata_path)
            repo = BaseRepository(
                owner="test-owner",
                repo="test-repo",
                remote_url="https://github.com/test-owner/test-repo.git",
                local_path=Path("/tmp/repos/test-owner/test-repo"),
            )
            storage1.add_repository(repo)

            worktree = WorktreeInfo(
                owner="test-owner",
                repo="test-repo",
                branch="feature-branch",
                local_path=Path("/tmp/worktrees/test-owner/test-repo/feature-branch"),
                workspace_id="feature-branch",
            )
            storage1.add_worktree(worktree)

            # Create second storage instance and verify data persists
            storage2 = MetadataStorage(metadata_path)
            assert storage2.get_repository("test-owner", "test-repo") is not None
            assert storage2.get_worktree("test-owner", "test-repo", "feature-branch") is not None

    def test_save_creates_valid_json(self, temp_storage):
        """Test that save creates valid JSON file."""
        repo = BaseRepository(
            owner="test-owner",
            repo="test-repo",
            remote_url="https://github.com/test-owner/test-repo.git",
            local_path=Path("/tmp/repos/test-owner/test-repo"),
        )
        temp_storage.add_repository(repo)

        # Read the file directly and verify it's valid JSON
        with open(temp_storage.metadata_path, "r", encoding="utf-8") as f:
            data = json.load(f)

        assert "repositories" in data
        assert "worktrees" in data
        assert "test-owner/test-repo" in data["repositories"]


class TestCorruptMetadataFile:
    """A corrupt metadata.json must never crash the CLI."""

    @pytest.mark.parametrize(
        "content",
        [
            pytest.param("{not json", id="invalid-json"),
            pytest.param('{"repositories": {"a": {"owner": "a"', id="truncated"),
            pytest.param("", id="empty-file"),
            pytest.param("[]", id="toplevel-list"),
            pytest.param('"x"', id="toplevel-string"),
        ],
    )
    def test_corrupt_file_loads_empty_and_is_quarantined(self, tmp_path, capsys, content):
        """A corrupt file yields empty state, is quarantined, and warns on stderr only."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(metadata_path, content)

        storage = MetadataStorage(metadata_path)

        assert storage.repositories == {}
        assert storage.worktrees == {}
        corrupt_path = tmp_path / "metadata.json.corrupt"
        assert corrupt_path.exists()
        assert corrupt_path.read_text(encoding="utf-8") == content
        assert not metadata_path.exists()

        captured = capsys.readouterr()
        assert captured.out == ""
        assert len(captured.err.strip().splitlines()) == 1
        assert str(corrupt_path) in captured.err

    @pytest.mark.parametrize(
        "content",
        [
            pytest.param(b'{"a": \x82\xff}', id="invalid-utf8"),
            pytest.param('{"repositories": {}, "worktrees": {}}'.encode("utf-16"), id="utf16"),
        ],
    )
    def test_undecodable_file_loads_empty_and_is_quarantined(self, tmp_path, capsys, content):
        """Bytes that are not valid UTF-8 are corruption, not a crash."""
        metadata_path = tmp_path / "metadata.json"
        metadata_path.write_bytes(content)

        storage = MetadataStorage(metadata_path)

        assert storage.repositories == {}
        assert storage.worktrees == {}
        corrupt_path = tmp_path / "metadata.json.corrupt"
        assert corrupt_path.read_bytes() == content
        assert not metadata_path.exists()

        captured = capsys.readouterr()
        assert captured.out == ""
        assert len(captured.err.strip().splitlines()) == 1
        assert str(corrupt_path) in captured.err

    def test_quarantine_uses_single_slot(self, tmp_path, capsys):
        """Repeated corruption overwrites the single .corrupt slot."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(metadata_path, "first-corruption")
        MetadataStorage(metadata_path)
        _write_metadata(metadata_path, "second-corruption")
        MetadataStorage(metadata_path)
        capsys.readouterr()

        corrupt_files = sorted(p.name for p in tmp_path.iterdir())
        assert corrupt_files == ["metadata.json.corrupt"]
        assert (tmp_path / "metadata.json.corrupt").read_text(
            encoding="utf-8"
        ) == "second-corruption"

    def test_storage_is_usable_after_quarantine(self, tmp_path, capsys):
        """After recovering from corruption the storage can still save and reload."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(metadata_path, "{broken")
        storage = MetadataStorage(metadata_path)
        capsys.readouterr()

        storage.add_repository(
            BaseRepository(
                owner="owner1",
                repo="repo1",
                remote_url="https://github.com/owner1/repo1.git",
                local_path=Path("/tmp/repos/owner1/repo1"),
            )
        )

        assert MetadataStorage(metadata_path).get_repository("owner1", "repo1") is not None


class TestMalformedEntries:
    """One bad entry must not cost the user the whole file."""

    @pytest.mark.parametrize(
        "bad_entry",
        [
            pytest.param(
                {k: v for k, v in _worktree_entry().items() if k != "local_path"},
                id="missing-local-path",
            ),
            pytest.param(_worktree_entry(created_at="not-a-timestamp"), id="unparsable-created-at"),
            pytest.param(_worktree_entry(local_path=None), id="null-local-path"),
            pytest.param("not-an-object", id="non-dict-entry"),
        ],
    )
    def test_bad_worktree_entry_is_skipped(self, tmp_path, capsys, bad_entry):
        """Good entries load, only the bad one is skipped, and the file is not quarantined."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps(
                {
                    "version": SCHEMA_VERSION,
                    "repositories": {"owner1/repo1": _repo_entry()},
                    "worktrees": {
                        "owner1/repo1/good": _worktree_entry(branch="good"),
                        "owner1/repo1/bad": bad_entry,
                    },
                }
            ),
        )

        storage = MetadataStorage(metadata_path)

        assert list(storage.worktrees) == ["owner1/repo1/good"]
        assert "owner1/repo1" in storage.repositories
        assert not (tmp_path / "metadata.json.corrupt").exists()
        assert metadata_path.exists()

        captured = capsys.readouterr()
        assert captured.out == ""
        # One line naming the skipped entry, one naming the backup of the original.
        assert len(captured.err.strip().splitlines()) == 2
        assert "owner1/repo1/bad" in captured.err
        assert str(tmp_path / "metadata.json.bak") in captured.err

    def test_skipped_entry_survives_the_next_write(self, tmp_path, capsys):
        """A rewrite drops the skipped entry, so the original must be preserved first."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps(
                {
                    "version": SCHEMA_VERSION,
                    "repositories": {"owner1/repo1": _repo_entry()},
                    "worktrees": {
                        "owner1/repo1/good": _worktree_entry(branch="good"),
                        "owner1/repo1/bad": _worktree_entry(
                            branch="bad", created_at="not-a-timestamp"
                        ),
                    },
                }
            ),
        )
        original_bytes = metadata_path.read_bytes()

        storage = MetadataStorage(metadata_path)
        backup_path = tmp_path / "metadata.json.bak"
        # The backup exists before anything can overwrite the file.
        assert backup_path.read_bytes() == original_bytes
        assert str(backup_path) in capsys.readouterr().err

        storage.add_worktree(
            WorktreeInfo(
                owner="owner1",
                repo="repo1",
                branch="new",
                local_path=Path("/tmp/worktrees/owner1/repo1/new"),
                workspace_id="new",
            )
        )

        on_disk = json.loads(metadata_path.read_text(encoding="utf-8"))
        assert sorted(on_disk["worktrees"]) == ["owner1/repo1/good", "owner1/repo1/new"]
        assert backup_path.read_bytes() == original_bytes
        assert "not-a-timestamp" in backup_path.read_text(encoding="utf-8")

    def test_a_newer_field_does_not_cost_the_entry(self, tmp_path, capsys):
        """A field only a newer build knows about is dropped, not the whole entry.

        Every stored entry has the same shape, so treating an unrecognized field
        as corruption would make one field added by a newer devlaunch wipe the
        entire worktree list at once. The entry loads; the original is preserved.
        """
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps(
                {
                    "version": SCHEMA_VERSION,
                    "repositories": {"owner1/repo1": _repo_entry(future_repo_field=1)},
                    "worktrees": {
                        "owner1/repo1/branch1": _worktree_entry(pinned_by_newer_build=True)
                    },
                }
            ),
        )
        original_bytes = metadata_path.read_bytes()

        storage = MetadataStorage(metadata_path)

        assert list(storage.worktrees) == ["owner1/repo1/branch1"]
        assert list(storage.repositories) == ["owner1/repo1"]
        assert storage.worktrees["owner1/repo1/branch1"].branch == "branch1"

        captured = capsys.readouterr()
        assert captured.out == ""
        # One line per entry naming the dropped field, one naming the backup.
        assert len(captured.err.strip().splitlines()) == 3
        assert "pinned_by_newer_build" in captured.err
        assert "future_repo_field" in captured.err
        assert not (tmp_path / "metadata.json.corrupt").exists()
        assert (tmp_path / "metadata.json.bak").read_bytes() == original_bytes

        storage.save()

        on_disk = json.loads(metadata_path.read_text(encoding="utf-8"))
        assert sorted(on_disk["worktrees"]) == ["owner1/repo1/branch1"]
        assert "pinned_by_newer_build" not in on_disk["worktrees"]["owner1/repo1/branch1"]
        assert (tmp_path / "metadata.json.bak").read_bytes() == original_bytes

    def test_unknown_top_level_section_survives_the_next_write(self, tmp_path, capsys):
        """A section this build cannot round-trip is preserved before it is dropped."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps(
                {
                    "version": SCHEMA_VERSION,
                    "repositories": {"owner1/repo1": _repo_entry()},
                    "worktrees": {},
                    "pinned_workspaces": {"owner1/repo1": True},
                }
            ),
        )
        original_bytes = metadata_path.read_bytes()

        storage = MetadataStorage(metadata_path)

        assert list(storage.repositories) == ["owner1/repo1"]
        captured = capsys.readouterr()
        assert captured.out == ""
        assert len(captured.err.strip().splitlines()) == 2
        assert "pinned_workspaces" in captured.err
        backup_path = tmp_path / "metadata.json.bak"
        assert str(backup_path) in captured.err
        assert backup_path.read_bytes() == original_bytes

        storage.save()

        assert "pinned_workspaces" not in json.loads(metadata_path.read_text(encoding="utf-8"))
        assert backup_path.read_bytes() == original_bytes

    def test_backup_uses_a_single_slot_distinct_from_quarantine(self, tmp_path, capsys):
        """Repeated lossy loads overwrite one .bak slot and never touch .corrupt."""
        metadata_path = tmp_path / "metadata.json"
        for marker in ("first", "second"):
            _write_metadata(
                metadata_path,
                json.dumps(
                    {
                        "version": SCHEMA_VERSION,
                        "repositories": {},
                        "worktrees": {f"owner1/repo1/{marker}": "not-an-object"},
                    }
                ),
            )
            MetadataStorage(metadata_path)
        capsys.readouterr()

        assert sorted(p.name for p in tmp_path.iterdir()) == [
            "metadata.json",
            "metadata.json.bak",
        ]
        assert "second" in (tmp_path / "metadata.json.bak").read_text(encoding="utf-8")

    def test_bad_repository_entry_is_skipped(self, tmp_path, capsys):
        """A malformed repository entry is skipped while good repositories load."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps(
                {
                    "version": SCHEMA_VERSION,
                    "repositories": {
                        "owner1/good": _repo_entry(repo="good"),
                        "owner1/bad": _repo_entry(repo="bad", local_path=None),
                    },
                    "worktrees": {},
                }
            ),
        )

        storage = MetadataStorage(metadata_path)

        assert list(storage.repositories) == ["owner1/good"]
        assert not (tmp_path / "metadata.json.corrupt").exists()
        captured = capsys.readouterr()
        assert captured.out == ""
        assert "owner1/bad" in captured.err
        assert (tmp_path / "metadata.json.bak").exists()

    def test_non_dict_section_is_skipped(self, tmp_path, capsys):
        """A section that is not an object is skipped without losing the other section."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps(
                {
                    "version": SCHEMA_VERSION,
                    "repositories": ["not", "an", "object"],
                    "worktrees": {"owner1/repo1/branch1": _worktree_entry()},
                }
            ),
        )

        storage = MetadataStorage(metadata_path)

        assert storage.repositories == {}
        assert list(storage.worktrees) == ["owner1/repo1/branch1"]
        assert not (tmp_path / "metadata.json.corrupt").exists()
        captured = capsys.readouterr()
        assert captured.out == ""
        assert "repositories" in captured.err
        assert (tmp_path / "metadata.json.bak").exists()


class TestAtomicSave:
    """save() must never leave a half-written metadata.json."""

    def test_save_writes_via_a_temp_file(self, temp_storage, monkeypatch):
        """save() renames a temp file into place rather than writing in place."""
        seen = _record_replacements(monkeypatch)
        temp_storage.save()

        assert len(seen) == 1
        assert seen[0].name != temp_storage.metadata_path.name
        assert seen[0].name.endswith(".tmp")

    def test_instances_in_one_process_do_not_share_a_temp_path(self, tmp_path, monkeypatch):
        """Two storage objects must not write through the same temp file."""
        metadata_path = tmp_path / "metadata.json"
        first = MetadataStorage(metadata_path)
        second = MetadataStorage(metadata_path)
        seen = _record_replacements(monkeypatch)

        first.save()
        second.save()

        assert len(seen) == 2
        assert seen[0] != seen[1]

    def test_interrupted_save_leaves_original_intact(self, temp_storage, monkeypatch):
        """A truncated temp file is discarded and the previous metadata stays readable."""
        temp_storage.add_repository(
            BaseRepository(
                owner="owner1",
                repo="repo1",
                remote_url="https://github.com/owner1/repo1.git",
                local_path=Path("/tmp/repos/owner1/repo1"),
            )
        )
        original = temp_storage.metadata_path.read_text(encoding="utf-8")
        parent = temp_storage.metadata_path.parent
        partial = {}

        def exploding_dump(_data, fp, **_kwargs):
            # Write real bytes first so a genuinely truncated temp file exists on
            # disk at the moment the write fails.
            fp.write('{\n  "version": 1,\n  "repositories": {"owner2/repo2"')
            fp.flush()
            partial.update(
                {p: p.read_text(encoding="utf-8") for p in parent.glob("*.tmp")},
            )
            raise OSError("disk full")

        monkeypatch.setattr(storage_module.json, "dump", exploding_dump)
        # save() must not swallow the write failure
        with pytest.raises(OSError):
            temp_storage.add_repository(
                BaseRepository(
                    owner="owner2",
                    repo="repo2",
                    remote_url="https://github.com/owner2/repo2.git",
                    local_path=Path("/tmp/repos/owner2/repo2"),
                )
            )

        monkeypatch.undo()
        # A single truncated temp file really existed mid-write.
        assert len(partial) == 1
        truncated = next(iter(partial.values()))
        assert truncated.endswith('{"owner2/repo2"')
        with pytest.raises(json.JSONDecodeError):
            json.loads(truncated)

        assert temp_storage.metadata_path.read_text(encoding="utf-8") == original
        reloaded = MetadataStorage(temp_storage.metadata_path)
        assert list(reloaded.repositories) == ["owner1/repo1"]
        # The lock sidecar is deliberate and permanent (see locks.py: unlinking
        # an flock'd file breaks the lock); only write debris counts as leftover.
        lock_name = temp_storage.metadata_path.name + ".lock"
        leftovers = [
            p.name
            for p in temp_storage.metadata_path.parent.iterdir()
            if p.name not in (temp_storage.metadata_path.name, lock_name)
        ]
        assert leftovers == []


class TestFilePermissions:
    """metadata.json holds repo owners and local paths: keep it private."""

    @staticmethod
    def _mode(path: Path) -> int:
        return stat.S_IMODE(path.stat().st_mode)

    def test_existing_mode_survives_the_atomic_replace(self, tmp_path):
        """A 0600 metadata.json is still 0600 after save()."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps({"version": SCHEMA_VERSION, "repositories": {}, "worktrees": {}}),
        )
        metadata_path.chmod(0o600)
        storage = MetadataStorage(metadata_path)

        previous_umask = os.umask(0o002)
        try:
            storage.save()
        finally:
            os.umask(previous_umask)

        assert self._mode(metadata_path) == 0o600

    def test_unusual_existing_mode_is_preserved(self, tmp_path):
        """save() copies whatever mode the user set, not just 0600."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps({"version": SCHEMA_VERSION, "repositories": {}, "worktrees": {}}),
        )
        metadata_path.chmod(0o640)
        MetadataStorage(metadata_path).save()

        assert self._mode(metadata_path) == 0o640

    def test_new_file_is_created_private(self, tmp_path):
        """A metadata.json created from scratch is 0600 regardless of umask."""
        metadata_path = tmp_path / "metadata.json"
        storage = MetadataStorage(metadata_path)

        previous_umask = os.umask(0o002)
        try:
            storage.save()
        finally:
            os.umask(previous_umask)

        assert self._mode(metadata_path) == 0o600


class TestSymlinkedMetadataFile:
    """A metadata.json symlinked elsewhere must keep working through the link."""

    @staticmethod
    def _linked(tmp_path):
        real_path = tmp_path / "synced" / "metadata.json"
        real_path.parent.mkdir()
        link_path = tmp_path / "metadata.json"
        link_path.symlink_to(real_path)
        return real_path, link_path

    def test_save_writes_through_the_symlink(self, tmp_path):
        """save() updates the link target and leaves the symlink itself in place."""
        real_path, link_path = self._linked(tmp_path)
        _write_metadata(
            real_path,
            json.dumps({"version": SCHEMA_VERSION, "repositories": {}, "worktrees": {}}),
        )

        storage = MetadataStorage(link_path)
        storage.add_repository(
            BaseRepository(
                owner="owner1",
                repo="repo1",
                remote_url="https://github.com/owner1/repo1.git",
                local_path=Path("/tmp/repos/owner1/repo1"),
            )
        )

        assert link_path.is_symlink()
        assert os.readlink(link_path) == str(real_path)
        on_disk = json.loads(real_path.read_text(encoding="utf-8"))
        assert "owner1/repo1" in on_disk["repositories"]
        assert list(MetadataStorage(link_path).repositories) == ["owner1/repo1"]

    def test_quarantine_moves_the_link_target(self, tmp_path, capsys):
        """Corruption behind a symlink quarantines the real file, not the link."""
        real_path, link_path = self._linked(tmp_path)
        _write_metadata(real_path, "{broken")

        MetadataStorage(link_path)

        assert link_path.is_symlink()
        assert not real_path.exists()
        corrupt_path = real_path.with_name("metadata.json.corrupt")
        assert corrupt_path.read_text(encoding="utf-8") == "{broken"
        assert str(corrupt_path) in capsys.readouterr().err


class TestSingleWrite:
    """Mutating helpers must write metadata.json exactly once."""

    @staticmethod
    def _count_saves(storage, monkeypatch):
        calls = []
        real_save = storage.save

        def counting_save():
            calls.append(1)
            real_save()

        monkeypatch.setattr(storage, "save", counting_save)
        return calls

    def _populated(self, storage):
        storage.add_repository(
            BaseRepository(
                owner="owner1",
                repo="repo1",
                remote_url="https://github.com/owner1/repo1.git",
                local_path=Path("/tmp/repos/owner1/repo1"),
                worktrees=[],
            )
        )
        return WorktreeInfo(
            owner="owner1",
            repo="repo1",
            branch="branch1",
            local_path=Path("/tmp/worktrees/owner1/repo1/branch1"),
            workspace_id="branch1",
        )

    def test_add_worktree_saves_once(self, temp_storage, monkeypatch):
        """add_worktree writes once even when it updates the repository's branch list."""
        worktree = self._populated(temp_storage)
        calls = self._count_saves(temp_storage, monkeypatch)

        temp_storage.add_worktree(worktree)

        assert len(calls) == 1
        on_disk = json.loads(temp_storage.metadata_path.read_text(encoding="utf-8"))
        assert on_disk["worktrees"]["owner1/repo1/branch1"]["branch"] == "branch1"
        assert on_disk["repositories"]["owner1/repo1"]["worktrees"] == ["branch1"]

    def test_remove_worktree_saves_once(self, temp_storage, monkeypatch):
        """remove_worktree writes once even when it updates the repository's branch list."""
        worktree = self._populated(temp_storage)
        temp_storage.add_worktree(worktree)
        calls = self._count_saves(temp_storage, monkeypatch)

        temp_storage.remove_worktree("owner1", "repo1", "branch1")

        assert len(calls) == 1
        on_disk = json.loads(temp_storage.metadata_path.read_text(encoding="utf-8"))
        assert on_disk["worktrees"] == {}
        assert on_disk["repositories"]["owner1/repo1"]["worktrees"] == []


class TestSchemaVersion:
    """The top-level version key gives future migrations a deterministic trigger."""

    def test_save_writes_current_version(self, temp_storage):
        """A round-trip writes and reads back the current version."""
        temp_storage.save()
        on_disk = json.loads(temp_storage.metadata_path.read_text(encoding="utf-8"))

        assert SCHEMA_VERSION == 2
        assert on_disk["version"] == 2
        assert MetadataStorage(temp_storage.metadata_path).schema_version == 2

    def test_legacy_file_without_version_loads_silently(self, tmp_path, capsys):
        """A pre-versioning file is the oldest shape, read with no warning.

        Not the *current* version: an absent header must still put the file below
        SCHEMA_VERSION, or the id-scheme migration keyed on that comparison would
        skip exactly the caches that predate versioning.
        """
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps(
                {
                    "repositories": {"owner1/repo1": _repo_entry()},
                    "worktrees": {"owner1/repo1/branch1": _worktree_entry()},
                }
            ),
        )

        storage = MetadataStorage(metadata_path)

        assert storage.schema_version == LEGACY_SCHEMA_VERSION
        assert LEGACY_SCHEMA_VERSION < SCHEMA_VERSION
        assert list(storage.repositories) == ["owner1/repo1"]
        assert list(storage.worktrees) == ["owner1/repo1/branch1"]
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""
        assert not (tmp_path / "metadata.json.corrupt").exists()

    def test_newer_version_warns_and_preserves_the_original(self, tmp_path, capsys):
        """A file from a newer devlaunch loads, and the original survives the rewrite."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps(
                {
                    "version": 99,
                    "repositories": {"owner1/repo1": _repo_entry()},
                    "worktrees": {"owner1/repo1/branch1": _worktree_entry()},
                }
            ),
        )
        original_bytes = metadata_path.read_bytes()

        storage = MetadataStorage(metadata_path)

        assert storage.schema_version == 99
        assert list(storage.repositories) == ["owner1/repo1"]
        assert list(storage.worktrees) == ["owner1/repo1/branch1"]
        assert metadata_path.exists()
        assert not (tmp_path / "metadata.json.corrupt").exists()
        captured = capsys.readouterr()
        assert captured.out == ""
        assert len(captured.err.strip().splitlines()) == 2
        assert "newer" in captured.err
        backup_path = tmp_path / "metadata.json.bak"
        assert str(backup_path) in captured.err
        assert backup_path.read_bytes() == original_bytes

        storage.save()

        # The promise is not immutability: the file is rewritten in this format.
        assert json.loads(metadata_path.read_text(encoding="utf-8"))["version"] == SCHEMA_VERSION
        assert backup_path.read_bytes() == original_bytes

    def test_older_version_loads_and_is_upgraded_on_write(self, tmp_path, capsys):
        """A lower version is an older shape: load it, report it, upgrade on write."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            json.dumps(
                {
                    "version": 0,
                    "repositories": {"owner1/repo1": _repo_entry()},
                    "worktrees": {"owner1/repo1/branch1": _worktree_entry()},
                }
            ),
        )

        storage = MetadataStorage(metadata_path)

        assert storage.schema_version == 0
        assert list(storage.repositories) == ["owner1/repo1"]
        assert list(storage.worktrees) == ["owner1/repo1/branch1"]
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err == ""
        assert not (tmp_path / "metadata.json.corrupt").exists()
        assert not (tmp_path / "metadata.json.bak").exists()

        storage.save()

        assert json.loads(metadata_path.read_text(encoding="utf-8"))["version"] == SCHEMA_VERSION

    @pytest.mark.parametrize(
        "version, normalized",
        [
            pytest.param("1.0", 1, id="integral-float"),
            pytest.param("2.0", 2, id="integral-float-current"),
            pytest.param("3.0", 3, id="integral-float-newer"),
            pytest.param("true", None, id="bool-true"),
            pytest.param("null", None, id="null"),
            pytest.param('"1"', None, id="string"),
            pytest.param("1.5", None, id="non-integral-float"),
            pytest.param("[1]", None, id="list"),
        ],
    )
    def test_odd_version_header_never_costs_the_entries(
        self, tmp_path, capsys, version, normalized
    ):
        """A cosmetically odd version header is not file-level corruption."""
        metadata_path = tmp_path / "metadata.json"
        _write_metadata(
            metadata_path,
            '{"version": '
            + version
            + ', "repositories": {"owner1/repo1": '
            + json.dumps(_repo_entry())
            + '}, "worktrees": {"owner1/repo1/branch1": '
            + json.dumps(_worktree_entry())
            + "}}",
        )
        original_bytes = metadata_path.read_bytes()

        storage = MetadataStorage(metadata_path)

        assert list(storage.repositories) == ["owner1/repo1"]
        assert list(storage.worktrees) == ["owner1/repo1/branch1"]
        assert metadata_path.exists()
        assert not (tmp_path / "metadata.json.corrupt").exists()
        backup_path = tmp_path / "metadata.json.bak"
        captured = capsys.readouterr()
        assert captured.out == ""

        if normalized is not None:
            # An integral number is that version; JSON has one number type.
            assert storage.schema_version == normalized
            if normalized <= SCHEMA_VERSION:
                assert captured.err == ""
                assert not backup_path.exists()
        else:
            # An unreadable header is read as the oldest shape, not the current
            # one, so a cache that needs migrating never claims to be current.
            assert storage.schema_version == LEGACY_SCHEMA_VERSION
            assert len(captured.err.strip().splitlines()) == 2
            assert str(backup_path) in captured.err
            assert backup_path.read_bytes() == original_bytes
