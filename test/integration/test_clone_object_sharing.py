"""The workspace clone shares its pack files with the bare cache.

Every workspace is a full clone of the bare cache next to it, and the reason
that is affordable is that git's default local transport *hardlinks* the pack
files instead of copying them: the second workspace of a repo costs its working
tree and its refs, not another copy of the history. Nothing in the code says so
-- the sharing is what `git clone <path> <path>` does when nobody asks it for
anything else -- so this file is where it is said, and where a change that
forfeits it goes red.

Measured on blooop/devlaunch (`du -sc` over the cache and each clone's `.git`,
ext4, git 2.55.0): cache plus one workspace is 2400 KB shared against 4472 KB
with `--no-hardlinks`, and each further workspace's `.git` costs 196 KB instead
of 2268 KB. devlaunch#154 decided this on the same measurement over a smaller
history (2044 KB / 3788 KB, 180 KB / 1924 KB); the ratio is the durable part.

Deliberately no clone flag guards this instead. `--local` is already the
default and does not even error on a `file://` source, so it would pin nothing;
`--shared` and `--reference` were measured to leave an fsck-broken workspace
once the bare's force-refspec fetch and gc have run, for a 2 KB saving. An
assertion is the guard, and it is this one.
"""
# pylint: disable=redefined-outer-name

import subprocess
from pathlib import Path

import pytest


def _packed_bare_cache(clone_manager, remote_url: str) -> Path:
    """Build the bare cache for test/repo and leave its objects in a pack file.

    The repack is not decoration, it is what makes the assertion capable of
    failing. The suite's fixture repository is three objects, which is far under
    git's `transfer.unpackLimit`, so a push explodes into loose objects and the
    bare cache ends up with no `objects/pack` directory at all -- against which
    "every pack file is shared" is a statement about the empty set. A cache
    cloned from a real forge arrives packed, so packing it here is also the
    honest starting state rather than a contrivance.

    It happens before the workspace is cloned on purpose: repacking a bare that
    already has clones is the *other* thing this file has an opinion about, and
    it is covered separately below.
    """
    clone_manager.repo_manager.ensure_repo("test", "repo", remote_url)
    bare_path = clone_manager.repo_manager.get_bare_path("test", "repo")
    subprocess.run(["git", "repack", "-a", "-d"], cwd=bare_path, check=True, capture_output=True)
    return bare_path


def _packs(objects_dir: Path) -> dict[str, Path]:
    """The pack files under an objects directory, keyed by name."""
    return {p.name: p for p in sorted((objects_dir / "pack").glob("*.pack"))}


@pytest.mark.integration
class TestCloneSharesObjectsWithTheCache:
    """What a workspace costs on disk, and why it is not a copy of the history."""

    def test_a_workspace_clone_shares_the_caches_pack_files_rather_than_copying_them(
        self, clone_manager, local_git_repo
    ):
        """Each pack in the workspace is the *same file* as the cache's.

        Same file identity — `(st_dev, st_ino)`, the pair this repo already
        counts hardlinks by in `disk_usage.py` — and a link count that proves at
        least two directory entries point at it. Both halves are needed: an
        inode number is only unique within its filesystem, so on its own it is
        satisfied by a copy that landed on another device and happened to reuse
        the number; and the link count alone says two entries exist somewhere
        without saying they are these two.

        This is the assertion that goes red on a `file://` URL, an intermediate
        copy, or an explicit `--no-hardlinks` -- each of which is a silent
        change, costing ~11x the `.git` per workspace with nothing failing.
        """
        remote_url = local_git_repo["remote_url"]
        bare_path = _packed_bare_cache(clone_manager, remote_url)

        ws_path = clone_manager.prepare_cold("test", "repo", "main", remote_url)

        cache_packs = _packs(bare_path / "objects")
        assert cache_packs, "the cache holds no pack file, so nothing below is being checked"

        workspace_packs = _packs(ws_path / ".git" / "objects")
        assert set(workspace_packs) == set(cache_packs)
        for name, cache_pack in cache_packs.items():
            cache_stat = cache_pack.stat()
            workspace_stat = workspace_packs[name].stat()
            assert (workspace_stat.st_dev, workspace_stat.st_ino) == (
                cache_stat.st_dev,
                cache_stat.st_ino,
            ), f"{name} is a copy, not a shared object file"
            assert workspace_stat.st_nlink >= 2, (
                f"{name} has {workspace_stat.st_nlink} link(s); a shared pack has at least 2"
            )

    def test_repacking_the_cache_leaves_an_existing_workspace_its_own_copy(
        self, clone_manager, local_git_repo
    ):
        """The cache can repack under a live workspace without breaking it.

        This is the safety property that makes `--shared`/`--reference`
        unnecessary rather than merely more fragile. Sharing is a hardlink and
        not a pointer, so a repack of the cache -- which unlinks the old pack
        and writes a new one -- drops the workspace's pack to a link count of
        one and leaves it as a private, complete copy. The workspace stops being
        cheap and never stops being valid, which is the opposite of what an
        alternates-based workspace does in the same situation.
        """
        remote_url = local_git_repo["remote_url"]
        bare_path = _packed_bare_cache(clone_manager, remote_url)
        ws_path = clone_manager.prepare_cold("test", "repo", "main", remote_url)
        shared_before = [p.stat().st_nlink for p in _packs(ws_path / ".git" / "objects").values()]
        assert shared_before and all(n >= 2 for n in shared_before)

        subprocess.run(
            ["git", "repack", "-a", "-d"], cwd=bare_path, check=True, capture_output=True
        )

        assert [p.stat().st_nlink for p in _packs(ws_path / ".git" / "objects").values()] == [
            1
        ] * len(shared_before)
        subprocess.run(["git", "fsck"], cwd=ws_path, check=True, capture_output=True)
