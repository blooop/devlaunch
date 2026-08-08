"""`dl --purge` against a cache the filesystem will not let it empty.

A purge used to hand the whole cache directory to `shutil.rmtree`, which stops at
the first path it is refused. Everything it had not reached yet -- the completion
caches, the metadata, every other clone -- stayed on disk, so a purge that met one
unremovable directory did not do the ninety percent it was allowed to do.

Measured on CI rather than imagined: a devcontainer writes into its bind-mounted
clone as the container's own user, and the run that later purges is a different
uid, so a directory in the cache belongs to someone the purge cannot chmod.

The refusal is manufactured here without a second user, because the mismatched
uid is one cause of the condition and not the condition itself: a directory its
own owner has removed write permission from cannot have its entries unlinked
either. Root is exempt from that check, which is why these tests say so and skip.
"""

import os
import pathlib
import subprocess
from typing import Iterator
from unittest.mock import patch

import pytest

from devlaunch.dl import (
    RemovedEverything,
    RemovedNothing,
    RemovedWhatItCould,
    purge_all_data,
    remove_as_far_as_permitted,
)

pytestmark = pytest.mark.skipif(
    os.geteuid() == 0,
    reason="root ignores the directory permissions these tests manufacture the refusal with",
)


def make_unremovable(directory: pathlib.Path) -> pathlib.Path:
    """Fill *directory* with a file, then take away the right to unlink it.

    Stands in for a clone a container wrote as its own uid. Returned so a test
    can name the path it expects to still be standing afterwards.
    """
    directory.mkdir(parents=True)
    (directory / "config").write_text("written by someone else")
    directory.chmod(0o500)
    return directory


def restore_permissions(root: pathlib.Path) -> None:
    """Give *root* and every directory under it back its write bit.

    Without this the test's own leftovers outlive it: pytest's tmp_path cleanup
    is refused by exactly the permissions the test set up.
    """
    root.chmod(0o700)
    for parent, dirnames, _files in os.walk(root):
        for name in dirnames:
            pathlib.Path(parent, name).chmod(0o700)


@pytest.fixture(name="cache_dir")
def fixture_cache_dir(tmp_path) -> Iterator[pathlib.Path]:
    """A cache with the removable things a real one has, and clones under it."""
    cache_dir = tmp_path / "devlaunch"
    (cache_dir / "repos" / "blooop").mkdir(parents=True)
    (cache_dir / "completions.json").write_text("{}")
    (cache_dir / "metadata.json").write_text("{}")
    yield cache_dir
    if cache_dir.exists():
        restore_permissions(cache_dir)


@pytest.fixture(name="no_workspaces")
def fixture_no_workspaces(cache_dir) -> Iterator[None]:
    """devpod answering that there are no workspaces, dl pointed at *cache_dir*.

    The workspace half of a purge is another ticket's subject; these tests are
    about the cache directory, so devpod is given nothing to delete.
    """

    def devpod(cmd, *_args, **_kwargs) -> subprocess.CompletedProcess:
        return subprocess.CompletedProcess(list(cmd), 0, "[]", "")

    with patch("devlaunch.dl.subprocess.run", side_effect=devpod):
        with patch("devlaunch.dl._get_cache_dir", return_value=cache_dir):
            with patch("devlaunch.dl.update_cache_background"):
                yield


@pytest.mark.usefixtures("no_workspaces")
class TestAPurgeRemovesWhatItIsAllowedTo:
    def test_one_unremovable_clone_does_not_save_the_rest_of_the_cache(self, cache_dir):
        """The ticket's defect: the caches and the metadata were removable and
        did not go, because a directory somewhere else was not."""
        refused = make_unremovable(cache_dir / "repos" / "blooop" / "e2e-repo" / "clone")

        assert purge_all_data() == 1

        assert refused.exists(), "the directory the filesystem refused is still there"
        assert not (cache_dir / "completions.json").exists()
        assert not (cache_dir / "metadata.json").exists()

    def test_a_refusal_in_one_clone_does_not_save_another(self, cache_dir):
        """Two refusals in separate subtrees, and a third clone in neither."""
        blooop = cache_dir / "repos" / "blooop"
        first = make_unremovable(blooop / "repo-a" / "clone")
        second = make_unremovable(blooop / "repo-b" / "clone")
        removable = blooop / "repo-c" / "clone"
        removable.mkdir(parents=True)
        (removable / "README.md").write_text("removable")

        assert purge_all_data() == 1

        assert first.exists()
        assert second.exists()
        assert not removable.exists()


@pytest.mark.usefixtures("no_workspaces")
class TestAPurgeNamesWhatItWasNotAllowedToRemove:
    """Where the two implementations differ no matter what order the filesystem
    hands its entries over in.

    "Removed what it could" is hard to pin down from the outside -- a removal
    that stops at the first refusal has still removed everything it happened to
    reach first, and nothing decides what that is. What it cannot do is *name*
    the refusals it never reached, so the report is where the difference is
    stated, and it is the report a person acts on anyway.
    """

    def test_it_names_every_refusal_and_not_only_the_first(self, cache_dir, capsys):
        blooop = cache_dir / "repos" / "blooop"
        first = make_unremovable(blooop / "repo-a" / "clone")
        second = make_unremovable(blooop / "repo-b" / "clone")

        assert purge_all_data() == 1

        out = capsys.readouterr().out
        assert str(first) in out
        assert str(second) in out

    def test_it_says_what_would_finish_the_job(self, cache_dir, capsys):
        """An errno leaves the reader to work out both what happened and what to
        do about it. Only one command finishes a purge the owner cannot: the one
        run as somebody who may remove another user's files."""
        make_unremovable(cache_dir / "repos" / "blooop" / "e2e-repo" / "clone")

        assert purge_all_data() == 1

        assert f"sudo rm -rf {cache_dir}" in capsys.readouterr().out

    def test_a_refused_clone_is_named_once_not_file_by_file(self, cache_dir, capsys):
        """A clone a container wrote holds every file it ever built. They are all
        refused for one reason, and naming the reason once is the difference
        between a report and a wall of paths."""
        refused = cache_dir / "repos" / "blooop" / "e2e-repo" / "clone"
        refused.mkdir(parents=True)
        for name in ("a", "b", "c"):
            (refused / name).write_text("written by someone else")
        refused.chmod(0o500)

        assert purge_all_data() == 1

        out = capsys.readouterr().out
        assert str(refused) in out
        for name in ("a", "b", "c"):
            assert str(refused / name) not in out


@pytest.mark.usefixtures("no_workspaces")
class TestThePurgeSaysWhichOfTheThreeThingsHappened:
    """ "Removed everything", "removed what it was allowed to" and "removed
    nothing" are three outcomes, and a user acting on the second should not have
    to guess whether they are looking at the third."""

    def test_a_cache_it_could_not_touch_at_all_says_so(self, cache_dir, capsys):
        """Nothing under an unwritable cache directory can be unlinked, so this
        purge did not remove some of the cache -- it removed none of it."""
        cache_dir.chmod(0o500)

        assert purge_all_data() == 1

        out = capsys.readouterr().out
        assert f"Removed nothing from {cache_dir}" in out
        assert "Removed what it could" not in out

    def test_a_cache_whose_only_content_is_refused_removed_nothing(self, cache_dir, capsys):
        """The same answer reached the long way round, and the one a purge is
        most likely to give in earnest: every directory down to the clone is
        writable, so the removal walks the whole cache and still gets nowhere.

        Distinct from the case above, where the cache directory itself was
        unwritable and the walk never started. A removal that reports what it
        managed by counting its refusals cannot tell these two apart from a
        partial success -- both are "some paths were refused".
        """
        (cache_dir / "completions.json").unlink()
        (cache_dir / "metadata.json").unlink()
        refused = make_unremovable(cache_dir / "repos" / "blooop" / "e2e-repo" / "clone")

        assert purge_all_data() == 1

        out = capsys.readouterr().out
        assert f"Removed nothing from {cache_dir}" in out
        assert "Removed what it could" not in out
        assert str(refused) in out

    def test_a_cache_it_partly_emptied_says_that_instead(self, cache_dir, capsys):
        make_unremovable(cache_dir / "repos" / "blooop" / "e2e-repo" / "clone")

        assert purge_all_data() == 1

        out = capsys.readouterr().out
        assert f"Removed what it could from {cache_dir}" in out
        assert "Removed nothing" not in out

    def test_a_cache_that_went_entirely_still_reports_success(self, cache_dir, capsys):
        """The unchanged path, kept under test because it is now reached through
        a different removal: no refusals, no report about them, exit zero."""
        assert purge_all_data() == 0

        assert not cache_dir.exists()
        out = capsys.readouterr().out
        assert f"Removed: {cache_dir}" in out
        assert "Not permitted" not in out


class TestTheRemovalIsAValue:
    """The three outcomes as the removal itself states them, rather than as the
    exit status they are projected onto."""

    def test_a_tree_that_goes_entirely_reports_everything(self, tmp_path):
        tree = tmp_path / "tree"
        (tree / "nested").mkdir(parents=True)
        (tree / "nested" / "file").write_text("removable")

        assert remove_as_far_as_permitted(tree) == RemovedEverything()
        assert not tree.exists()

    def test_a_tree_nothing_can_be_removed_from_reports_nothing(self, tmp_path):
        tree = tmp_path / "tree"
        make_unremovable(tree)
        try:
            assert remove_as_far_as_permitted(tree) == RemovedNothing((tree,))
        finally:
            restore_permissions(tree)

    def test_a_tree_that_partly_goes_names_only_what_stayed(self, tmp_path):
        tree = tmp_path / "tree"
        tree.mkdir()
        (tree / "removable").write_text("goes")
        refused = make_unremovable(tree / "stuck")
        try:
            assert remove_as_far_as_permitted(tree) == RemovedWhatItCould((refused,))
            assert not (tree / "removable").exists()
        finally:
            restore_permissions(tree)

    def test_a_tree_it_emptied_but_cannot_remove_names_the_tree(self, tmp_path):
        """A directory's own removal is governed by its parent, so emptying one
        completely and still being refused it is a real outcome -- and it is the
        second arm, not the first: everything that was in it did go."""
        tree = tmp_path / "tree"
        tree.mkdir()
        (tree / "file").write_text("goes")
        tmp_path.chmod(0o500)
        try:
            assert remove_as_far_as_permitted(tree) == RemovedWhatItCould((tree,))
            assert not (tree / "file").exists()
        finally:
            tmp_path.chmod(0o700)

    def test_a_symlink_is_unlinked_and_not_walked_into(self, tmp_path):
        """A purge removes devlaunch's cache and nothing else. Following a link
        out of the cache would remove a checkout nobody asked about -- which is
        the one way this removal could be worse than the rmtree it replaces."""
        outside = tmp_path / "outside"
        outside.mkdir()
        (outside / "keep-me").write_text("not devlaunch's to remove")
        tree = tmp_path / "tree"
        tree.mkdir()
        (tree / "link").symlink_to(outside)

        assert remove_as_far_as_permitted(tree) == RemovedEverything()
        assert not tree.exists()
        assert (outside / "keep-me").exists()
