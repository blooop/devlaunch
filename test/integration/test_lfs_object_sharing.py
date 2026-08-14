"""The bare cache is the repo's git-lfs store, and workspaces hardlink from it.

The sibling of `test_clone_object_sharing.py`, one layer up the stack. That file
pins what `git clone <bare> <ws>` does for git's own objects — hardlinks the
packs, so a second workspace costs its worktree and not another copy of the
history. LFS objects are not git objects and the clone does not carry them at
all, so without the work this file covers, every workspace of an LFS repo paid a
**full download from the forge** and kept a **private copy** of the payload in
`.git/lfs/objects`. The cache made the history free and left the large files,
which are the expensive part, entirely unshared.

What the code does instead, and what is asserted below: fetch the payload once
into `<bare>/lfs` with the bare as cwd, then materialize each workspace with
`git lfs pull "file://<bare>"`. Measured on this suite's fixture repo (git-lfs
3.7.1, ext4): the workspace's object file is the **same `(st_dev, st_ino)`** as
the bare's, so the second workspace's store costs zero bytes, and the pull
succeeds with the remote **deleted from disk** — the whole point being that it
never reaches the network.

The remaining per-workspace cost is the worktree copy, which is 1x and
unavoidable: git-lfs writes real bytes into the working tree, and a devcontainer
build has to be able to read them. What is removed is the second copy and the
second download.

`file://` rather than an added remote or `-c lfs.storage`, and that restraint is
load-bearing rather than stylistic: the clone directory is bind-mounted into the
devcontainer while `.bare` is **not**, so anything host-specific persisted into
the clone's config breaks every in-container `git checkout` of an LFS repo.
`TestNothingHostSpecificIsPersisted` is where that is held.
"""
# pylint: disable=redefined-outer-name

import shutil
import subprocess
from pathlib import Path
from typing import Any, Dict

import pytest

# Big enough that a copy and a hardlink are visibly different things, small
# enough that the suite does not notice. The bytes are deterministic so a
# half-materialized worktree cannot pass by accident.
PAYLOAD = bytes(range(256)) * 512
assert len(PAYLOAD) == 131072

pytestmark = [
    pytest.mark.integration,
    pytest.mark.skipif(
        shutil.which("git-lfs") is None,
        reason="needs a real git-lfs; it is in the pixi test environment",
    ),
]


def git(cwd, *args, **kwargs):
    """Run a real git command, failing loudly."""
    return subprocess.run(
        ["git", *args], cwd=str(cwd), check=True, capture_output=True, text=True, **kwargs
    )


@pytest.fixture
def lfs_remote(tmp_path: Path, monkeypatch) -> Dict[str, Any]:
    """A local fixture "remote" whose payload really lives in git-lfs.

    Every git in the test — including the ones devlaunch spawns — is pointed at
    a global config of this test's own via `GIT_CONFIG_GLOBAL`. That is not
    tidiness: `git lfs pull` **fetches the objects but silently skips the
    checkout** unless the `filter.lfs` clean/smudge config is installed, so on a
    machine where nobody has run `git lfs install` the materialization assertions
    below would fail for a reason that has nothing to do with the cache. Scoping
    the config also keeps the developer's `commit.gpgsign` from failing every
    commit here, the way `test_lfs_probe_real.py` has to switch off per repo.

    Two branches, both naming the same LFS object: `main`, and a `feature/lfs`
    that adds one ordinary file. The second workspace is launched from that
    branch — a workspace is per branch, so two of them cannot share one — and
    the extra commit deliberately leaves `big.bin` untouched, so the checkout
    that switches branches has no reason to smudge and the only thing that can
    put real bytes on disk is the materialization under test.
    """
    config = tmp_path / "gitconfig"
    config.write_text("")
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(config))
    for key, value in (
        ("user.email", "test@example.com"),
        ("user.name", "Test"),
        ("commit.gpgsign", "false"),
        ("init.defaultBranch", "main"),
    ):
        subprocess.run(["git", "config", "--global", key, value], check=True, capture_output=True)
    subprocess.run(["git", "lfs", "install", "--skip-repo"], check=True, capture_output=True)

    remote = tmp_path / "lfs_remote.git"
    subprocess.run(
        ["git", "init", "--bare", "-q", "--initial-branch=main", str(remote)],
        check=True,
        capture_output=True,
    )

    work = tmp_path / "lfs_work"
    work.mkdir()
    git(work, "init", "-q")
    git(work, "lfs", "install", "--local")
    git(work, "lfs", "track", "*.bin")
    (work / "big.bin").write_bytes(PAYLOAD)
    (work / "README.md").write_text("# lfs fixture\n")
    git(work, "add", "-A")
    git(work, "commit", "-qm", "add an lfs payload")
    git(work, "remote", "add", "origin", str(remote))
    git(work, "push", "-q", "-u", "origin", "main")

    git(work, "checkout", "-q", "-b", "feature/lfs")
    (work / "notes.txt").write_text("an ordinary file, on the second branch\n")
    git(work, "add", "-A")
    git(work, "commit", "-qm", "add a plain file")
    git(work, "push", "-q", "-u", "origin", "feature/lfs")

    return {"remote_url": str(remote), "remote_path": remote, "work_dir": work}


def lfs_objects(store_root: Path) -> Dict[str, Path]:
    """The git-lfs object files under a store, keyed by oid."""
    objects = store_root / "objects"
    if not objects.is_dir():
        return {}
    return {p.name: p for p in objects.rglob("*") if p.is_file()}


def identity(path: Path) -> tuple:
    """The pair that says two directory entries are one file.

    `(st_dev, st_ino)`, the same pair `disk_usage.py` counts hardlinks by. An
    inode number is unique only within its filesystem, so on its own it would be
    satisfied by a copy that landed on another device and reused the number.
    """
    st = path.stat()
    return (st.st_dev, st.st_ino)


class TestTheBareCacheIsTheLfsStore:
    """One download for the repo, then every workspace hardlinks from it."""

    def test_a_second_workspace_materializes_with_the_remote_deleted(
        self, clone_manager, lfs_remote
    ):
        """The payload is fetched once, and the second workspace pays no network.

        The remote is **removed from disk** between the two launches, which is a
        harder condition than an unreachable URL: nothing can be silently
        re-fetched, so anything the second workspace ends up holding provably
        came out of the cache. That is the whole claim of devlaunch#154 —
        `<bare>/lfs` is the repo's store, and a workspace materializes out of it.

        Both halves are asserted because either alone is passable by accident: a
        workspace could hold the right bytes as its own private copy (correct
        and expensive, which is exactly the state before this change), or share
        an object file while leaving the worktree on a pointer.
        """
        remote_url = lfs_remote["remote_url"]

        first = clone_manager.prepare_cold("test", "repo", "main", remote_url)
        assert (first / "big.bin").read_bytes() == PAYLOAD

        bare_path = clone_manager.repo_manager.get_bare_path("test", "repo")
        cache_objects = lfs_objects(bare_path / "lfs")
        assert cache_objects, "the bare cache holds no LFS object, so it is not the store"

        shutil.rmtree(lfs_remote["remote_path"])

        second = clone_manager.prepare_cold("test", "repo", "feature/lfs", remote_url)

        assert (second / "big.bin").read_bytes() == PAYLOAD
        workspace_objects = lfs_objects(second / ".git" / "lfs")
        assert set(workspace_objects) == set(cache_objects)
        for oid, cache_object in cache_objects.items():
            assert identity(workspace_objects[oid]) == identity(cache_object), (
                f"{oid} is a copy in the workspace, not the cache's own file"
            )
            assert workspace_objects[oid].stat().st_nlink >= 2

    def test_the_first_workspace_leaves_the_payload_in_the_cache(self, clone_manager, lfs_remote):
        """The store lands in the bare, not only in the workspace that filled it.

        The direction matters: materializing the first workspace from origin and
        letting the cache stay empty would still give that workspace real
        content, and would leave every later one to download the payload again.
        The object being in `<bare>/lfs` *and* being the same file as the
        workspace's is what makes the second launch free.
        """
        first = clone_manager.prepare_cold("test", "repo", "main", lfs_remote["remote_url"])
        bare_path = clone_manager.repo_manager.get_bare_path("test", "repo")

        cache_objects = lfs_objects(bare_path / "lfs")
        workspace_objects = lfs_objects(first / ".git" / "lfs")

        assert set(cache_objects) == set(workspace_objects) != set()
        for oid, cache_object in cache_objects.items():
            assert identity(workspace_objects[oid]) == identity(cache_object)


class TestNothingHostSpecificIsPersisted:
    """What may not end up in the clone's config, and why it would hurt.

    `dl` hands the clone directory to `devpod up`, which bind-mounts *it* into
    the container. `.bare` is a sibling and is not mounted, so a host path
    written into the clone's config names a directory that does not exist inside
    — and git-lfs consults it on every checkout. The failure would not show up
    on the host at all, only in the container, on the repos this feature exists
    to make cheap.
    """

    def test_the_clone_records_no_lfs_storage_override(self, clone_manager, lfs_remote):
        """`lfs.storage` stays unset, however the objects got there.

        This is the assertion that goes red on the obvious shortcut. Pointing
        `lfs.storage` at the bare would share the objects with no copying at
        all, and it was rejected twice over: persisted, it breaks the container;
        passed as `-c`, it was measured to break against local-path remotes
        outright, because `GIT_CONFIG_PARAMETERS` is inherited by the
        remote-side git-lfs child.
        """
        ws_path = clone_manager.prepare_cold("test", "repo", "main", lfs_remote["remote_url"])

        result = subprocess.run(
            ["git", "config", "--local", "--get", "lfs.storage"],
            cwd=ws_path,
            capture_output=True,
            text=True,
            check=False,
        )

        assert result.stdout.strip() == ""

    def test_the_clone_keeps_exactly_one_remote_and_it_is_the_forge(
        self, clone_manager, lfs_remote
    ):
        """No `file://` remote is left behind pointing at the bare.

        Adding a remote for the cache is the other natural way to write this,
        and it persists the same host path in the same file. The URL is checked
        as well as the name: a remote called `origin` that had been repointed at
        the bare would satisfy a count and would still leave the container
        talking to a directory it cannot see — and would break `git push` on the
        host besides.
        """
        remote_url = lfs_remote["remote_url"]
        ws_path = clone_manager.prepare_cold("test", "repo", "main", remote_url)

        remotes = git(ws_path, "remote").stdout.split()
        origin_url = git(ws_path, "remote", "get-url", "origin").stdout.strip()

        assert remotes == ["origin"]
        assert origin_url == remote_url
