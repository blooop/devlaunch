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


def a_remote_headed_at(tmp_path: Path, branch: str, name: str) -> str:
    """A bare repository whose HEAD is *branch*, with one commit on it."""
    remote = tmp_path / f"{name}.git"
    subprocess.run(
        ["git", "init", "--bare", f"--initial-branch={branch}", str(remote)],
        check=True,
        capture_output=True,
    )
    work = tmp_path / name
    subprocess.run(["git", "clone", str(remote), str(work)], check=True, capture_output=True)
    for key, value in (("user.email", "t@example.com"), ("user.name", "T")):
        subprocess.run(["git", "config", key, value], cwd=work, check=True, capture_output=True)
    (work / "README.md").write_text("hello\n", encoding="utf-8")
    subprocess.run(["git", "add", "-A"], cwd=work, check=True, capture_output=True)
    subprocess.run(["git", "commit", "-m", "first"], cwd=work, check=True, capture_output=True)
    subprocess.run(
        ["git", "push", "-u", "origin", branch], cwd=work, check=True, capture_output=True
    )
    return str(remote)


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

    def test_the_clone_on_disk_is_adopted_rather_than_cloned_over(self, real_managers, tmp_path):
        manager: RepositoryManager = real_managers["repo_manager"]
        storage = real_managers["storage"]
        # Headed at `master`, not `main`. The rebuilt record has to carry a
        # branch read off the adopted clone, and `main` is what
        # `_get_default_branch` returns when it could read nothing at all --
        # so against a main-headed remote the assertion below is satisfied by
        # the fallback and pins nothing about the adoption path.
        remote_url = a_remote_headed_at(tmp_path, "master", "adopted")

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

        adopted = manager.ensure_repo("test", "repo", remote_url)

        assert refs_of(bare_path) == before, "the clone was rebuilt instead of adopted"
        assert adopted.local_path == bare_path
        assert adopted.remote_url == remote_url
        assert adopted.default_branch == "master", (
            "the rebuilt record fell back to `main` instead of reading the clone"
        )
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

        cloned = manager.ensure_repo("test", "repo", local_git_repo["remote_url"])

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
            manager.ensure_repo("test", "repo", nowhere)

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
            manager.ensure_repo("test", "repo", str(tmp_path / "gone.git"))

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
            manager.ensure_repo("test", "repo", str(tmp_path / "gone.git"))

        recovered = manager.ensure_repo("test", "repo", local_git_repo["remote_url"])
        assert recovered.default_branch == "main"
        assert manager.repo_exists("test", "repo")


@pytest.mark.integration
class TestTheDefaultBranchIsReadOffTheClone:
    """``main`` is the fallback, not the assumption."""

    # `master` because a repo whose default really is master and a repo the
    # function could not read otherwise give the same answer -- there are three
    # layers of `except` above a literal `return "main"`.
    #
    # `release/1.0` because a branch name may contain slashes and the reading
    # used to take the segment after the last one, so a real default branch was
    # silently recorded as a ref the repository does not have. `master` cannot
    # catch that: it has no slash and passes either way.
    @pytest.mark.parametrize("branch", ["master", "release/1.0", "feature/auth"])
    def test_the_recorded_branch_is_the_one_the_remote_actually_heads_at(
        self, real_managers, tmp_path, branch
    ):
        remote = a_remote_headed_at(tmp_path, branch, branch.replace("/", "-"))

        cloned = real_managers["repo_manager"].ensure_repo("test", "headed", remote)

        assert cloned.default_branch == branch
        # And it names a ref that is really there, which is the property the
        # equality above is a proxy for.
        bare = real_managers["repo_manager"].get_bare_path("test", "headed")
        assert f"refs/heads/{branch}" in refs_of(bare)


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

        recovered = manager.ensure_repo("test", "repo", local_git_repo["remote_url"])
        assert manager.repo_exists("test", "repo")
        assert recovered.default_branch == "main"


@pytest.mark.integration
class TestACacheTheUserCannotWriteTo:
    """A read-only mount, a full disk, a directory someone tightened.

    The first version of this test made the whole `repos_dir` unwritable, and
    that never reached the code it was about: `ensure_repo` takes the repo lock
    first, and `hold_lock` opens with `lock_path.parent.mkdir(...)`, so the call
    died there with the clone, its cleanup and every storage write unexecuted.
    Both of its post-assertions were then true before the call as well as after.

    So the directories the lock needs are made first and only the *clone* is
    blocked, which puts the failure in `clone_repo` where the claim lives.
    """

    def test_a_clone_that_cannot_be_written_leaves_no_record_behind(
        self, real_managers, local_git_repo, refuses_writes
    ):
        manager: RepositoryManager = real_managers["repo_manager"]
        # Everything up to the clone exists and is writable...
        repo_dir = manager.get_repo_path("test", "repo")
        repo_dir.mkdir(parents=True)
        manager.lock_path("test", "repo").touch()
        # ...and then the directory the clone must create `.bare` in is not.
        refuses_writes(repo_dir)

        with pytest.raises(RuntimeError, match="Failed to clone repository"):
            manager.ensure_repo("test", "repo", local_git_repo["remote_url"])

        # The residue that would actually hurt: every caller reads a returned
        # record as "the cache is ready", so a record for a clone that did not
        # materialize sends the next launch to an empty path.
        assert real_managers["storage"].get_repository("test", "repo") is None, (
            "a record was written for a clone that does not exist"
        )
        assert not manager.get_bare_path("test", "repo").exists()
        assert manager.lock_path("test", "repo").exists()
