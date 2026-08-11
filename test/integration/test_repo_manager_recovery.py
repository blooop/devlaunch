"""What ``RepositoryManager`` does when it finds the cache mid-disaster.

Every arm here is reached by a state some *other* run left behind — a process
that died between cloning and saving its metadata, a clone `git` refused
halfway, a metadata file restored from a backup older than the directories it
describes. They are the arms with the most to lose, because the alternative to
recovering is deleting a bare clone that another launch is using or re-cloning
one that holds a ref nothing else has.

They are also the arms the unit tests cannot reach: what distinguishes them is
the state of a real directory (does ``.bare`` have a ``HEAD``?) and the exit
status of a real ``git clone``, so a mocked ``subprocess`` decides the answer
before the code does. So these run real git against real directories, with no
network: the "remote" is a bare repository in the same ``tmp_path``.
"""

import os
import shutil
import subprocess
from pathlib import Path

import pytest

from devlaunch.worktree.repo_manager import RepositoryManager


def head_of(bare_path: Path) -> str:
    """The commit a bare clone's HEAD resolves to."""
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=bare_path,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def refs_of(bare_path: Path) -> list:
    """Every ref name in a bare clone, sorted."""
    listed = subprocess.run(
        ["git", "for-each-ref", "--format=%(refname)"],
        cwd=bare_path,
        capture_output=True,
        text=True,
        check=True,
    )
    return sorted(listed.stdout.split())


@pytest.mark.integration
class TestABareCloneWithNoRecord:
    """The clone is on disk and ``metadata.json`` has never heard of it.

    Two ways in, and they are indistinguishable from the inside: another
    process cloned it just now and this process loaded its metadata before that
    one saved, or a run died between the clone and the save. Both leave the same
    two facts — a working ``.bare``, no record — and in both the clone is the
    authority. Deleting it is not an option: it may be the cache a live launch
    is running out of.
    """

    def test_the_clone_on_disk_is_adopted_rather_than_cloned_over(
        self, real_managers, local_git_repo
    ):
        manager: RepositoryManager = real_managers["repo_manager"]
        storage = real_managers["storage"]
        remote_url = local_git_repo["remote_url"]

        manager.clone_repo("test", "repo", remote_url)
        bare_path = manager.get_bare_path("test", "repo")

        # A ref the remote does not have, so "the same clone" is a fact about
        # this directory's contents rather than about its mtime: a re-clone
        # would rebuild the refs from the remote and this one would be gone.
        subprocess.run(
            ["git", "update-ref", "refs/heads/only-here", head_of(bare_path)],
            cwd=bare_path,
            check=True,
            capture_output=True,
        )
        before = refs_of(bare_path)
        assert "refs/heads/only-here" in before

        # The record disappears; the directory does not.
        storage.remove_repository("test", "repo")
        assert storage.get_repository("test", "repo") is None

        adopted = manager.ensure_repo("test", "repo", remote_url, auto_fetch=False)

        assert refs_of(bare_path) == before, "the clone was rebuilt instead of adopted"
        assert adopted.local_path == bare_path
        assert adopted.remote_url == remote_url
        assert adopted.default_branch == "main", "the branch was read off the clone"
        assert storage.get_repository("test", "repo") is not None, "the record was rebuilt"

    def test_a_partial_clone_with_no_head_is_cleared_and_replaced(
        self, real_managers, local_git_repo
    ):
        # The other side of the same check, and the reason it is `HEAD` and not
        # `exists()`: a directory `git clone` was killed halfway through is not
        # a clone, and nothing can be recovered from it. Holding the repo lock
        # means no live process owns it, so it goes.
        manager: RepositoryManager = real_managers["repo_manager"]
        bare_path = manager.get_bare_path("test", "repo")
        bare_path.mkdir(parents=True)
        (bare_path / "objects").mkdir()
        (bare_path / "half-written").write_text("nothing usable\n", encoding="utf-8")

        cloned = manager.ensure_repo("test", "repo", local_git_repo["remote_url"], auto_fetch=False)

        assert not (bare_path / "half-written").exists(), "the wreckage was left in place"
        assert (bare_path / "HEAD").exists()
        assert cloned.default_branch == "main"
        assert "refs/heads/main" in refs_of(bare_path)


@pytest.mark.integration
class TestACloneThatFails:
    """``git clone`` refused, and the question is what is left behind.

    Worth knowing before reading these: for every failure reachable from here —
    a remote that does not exist, one that is not a repository, one that is a
    plain file — ``git`` removes the destination it created before exiting, so
    the ``rmtree`` in ``clone_repo``'s ``except`` is not what makes the
    directory go away. It is a backstop for the failures that are *not*
    reachable from a test: a clone killed by a signal, or one that ran the disk
    out mid-write.

    So none of these pin that ``rmtree`` firing. What they pin is the state the
    cache is left in whoever did the removing — no directory, **no metadata
    record**, and a lock file still where the next process will look for it —
    and that the cleanup cannot widen into the directory above it.
    """

    def test_nothing_is_left_where_the_clone_would_have_gone(self, real_managers, tmp_path):
        manager: RepositoryManager = real_managers["repo_manager"]
        nowhere = str(tmp_path / "no-such-repo.git")

        with pytest.raises(RuntimeError, match="Failed to clone repository"):
            manager.ensure_repo("test", "repo", nowhere, auto_fetch=False)

        assert not manager.get_bare_path("test", "repo").exists()
        # The residue that would actually hurt. A record is every caller's
        # answer to "is the cache ready", and one naming a directory that is
        # not there sends the next launch to a path with nothing in it.
        assert real_managers["storage"].get_repository("test", "repo") is None

    def test_the_lock_file_survives_the_cleanup(self, real_managers, tmp_path):
        # The cleanup deletes `.bare` and must not widen to the directory above
        # it, because that is where the lock this call is *currently holding*
        # lives. Unlinking an flock'd file is the classic self-defeating move:
        # the holder still holds an inode nobody else can see, and the next
        # arrival locks a fresh file and walks straight past it. So a failed
        # clone racing a waiting sibling would hand both of them the lock.
        #
        # Stated here rather than in `locks.py`'s own tests: the rule is about
        # the file, and this is the one code path that deletes near it.
        manager: RepositoryManager = real_managers["repo_manager"]
        lock_path = manager.lock_path("test", "repo")

        with pytest.raises(RuntimeError):
            manager.ensure_repo("test", "repo", str(tmp_path / "gone.git"), auto_fetch=False)

        assert lock_path.exists(), "a failed clone took the repo lock file with it"
        assert lock_path.parent.exists()

    def test_a_second_attempt_after_a_failure_succeeds(
        self, real_managers, tmp_path, local_git_repo
    ):
        # What the cleanup is for, as an outcome rather than a directory
        # listing: the run that failed left the cache in a state the next one
        # can clone into.
        manager: RepositoryManager = real_managers["repo_manager"]
        with pytest.raises(RuntimeError):
            manager.ensure_repo("test", "repo", str(tmp_path / "gone.git"), auto_fetch=False)

        recovered = manager.ensure_repo(
            "test", "repo", local_git_repo["remote_url"], auto_fetch=False
        )
        assert recovered.default_branch == "main"
        assert manager.repo_exists("test", "repo")


@pytest.mark.integration
class TestTheDefaultBranchIsReadOffTheClone:
    """``main`` is the fallback, not the assumption."""

    def test_a_master_headed_remote_is_recorded_as_master(self, real_managers, tmp_path):
        # `_get_default_branch` falls back to "main" through three layers of
        # `except`, so a repo whose default really is `master` and a repo the
        # function could not read produce the same answer. Only a real clone of
        # a real master-headed remote tells the two apart.
        remote = tmp_path / "old_style.git"
        subprocess.run(
            ["git", "init", "--bare", "--initial-branch=master", str(remote)],
            check=True,
            capture_output=True,
        )
        work = tmp_path / "old_style"
        subprocess.run(["git", "clone", str(remote), str(work)], check=True, capture_output=True)
        for key, value in (("user.email", "t@example.com"), ("user.name", "T")):
            subprocess.run(["git", "config", key, value], cwd=work, check=True, capture_output=True)
        (work / "README.md").write_text("old\n", encoding="utf-8")
        subprocess.run(["git", "add", "-A"], cwd=work, check=True, capture_output=True)
        subprocess.run(["git", "commit", "-m", "first"], cwd=work, check=True, capture_output=True)
        subprocess.run(
            ["git", "push", "-u", "origin", "master"], cwd=work, check=True, capture_output=True
        )

        cloned = real_managers["repo_manager"].ensure_repo(
            "test", "old-style", str(remote), auto_fetch=False
        )
        assert cloned.default_branch == "master"


@pytest.mark.integration
class TestARecordWhoseCloneIsGone:
    """``metadata.json`` outlived the directory it describes.

    A restored backup, a hand-deleted cache, a half-finished ``dl --purge``.
    The record is the stale one here, which is the opposite of the adoption
    case above, and the resolution is the same principle: the filesystem wins.
    """

    def test_the_record_is_not_reported_as_a_present_repository(
        self, real_managers, local_git_repo
    ):
        manager: RepositoryManager = real_managers["repo_manager"]
        manager.clone_repo("test", "repo", local_git_repo["remote_url"])
        assert manager.get_repo("test", "repo") is not None

        # Only the directory goes. The record stays exactly as it was.
        shutil.rmtree(manager.get_bare_path("test", "repo"))

        assert real_managers["storage"].get_repository("test", "repo") is not None
        assert manager.get_repo("test", "repo") is None, "a record alone is not a repository"
        assert not manager.repo_exists("test", "repo")

    def test_the_next_ensure_clones_it_back(self, real_managers, local_git_repo):
        manager: RepositoryManager = real_managers["repo_manager"]
        manager.clone_repo("test", "repo", local_git_repo["remote_url"])
        shutil.rmtree(manager.get_bare_path("test", "repo"))

        recovered = manager.ensure_repo(
            "test", "repo", local_git_repo["remote_url"], auto_fetch=False
        )
        assert manager.repo_exists("test", "repo")
        assert recovered.default_branch == "main"


@pytest.mark.integration
@pytest.mark.skipif(os.geteuid() == 0, reason="root is not refused by directory permissions")
class TestAnUnwritableCache:
    """The cache directory the user cannot write to."""

    def test_a_clone_into_an_unwritable_repos_dir_raises_rather_than_half_succeeding(
        self, real_managers, local_git_repo, tmp_path
    ):
        # A full disk and a read-only mount both arrive here, and the arm that
        # handles them is the same `except OSError` the failed-clone cleanup
        # runs under. What must not happen is a `BaseRepository` handed back
        # for a clone that is not there: every caller treats a returned record
        # as "the cache is ready".
        locked = tmp_path / "locked-cache"
        locked.mkdir()
        blocked = RepositoryManager(
            repos_dir=locked,
            storage=real_managers["storage"],
            config=real_managers["config"],
        )
        locked.chmod(0o500)
        try:
            with pytest.raises((RuntimeError, OSError, PermissionError)):
                blocked.ensure_repo("test", "repo", local_git_repo["remote_url"], auto_fetch=False)
        finally:
            locked.chmod(0o700)

        assert real_managers["storage"].get_repository("test", "repo") is None, (
            "a record was written for a clone that does not exist"
        )
        assert not (locked / "test" / "repo" / ".bare" / "HEAD").exists()
