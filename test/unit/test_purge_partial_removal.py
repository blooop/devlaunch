"""`dl --purge` removes what it is allowed to, and names what refused.

A container writes into its bind-mounted clone as its own user. In
`mcr.microsoft.com/devcontainers/base:ubuntu` that is `vscode`, uid 1000, and
the directories it creates are its own. Where the host user is uid 1000 too --
an ordinary single-developer box -- nothing goes wrong and this file's subject
never arises. Where they differ (CI, a shared machine, a container running as
root, devlaunch developed inside its own devcontainer) the host user cannot
empty those directories, and `shutil.rmtree` raises EACCES partway through.

devlaunch#131 measured what happened next: the purge stopped at the first
refusal and left *the whole cache* standing -- completion caches, metadata.json,
every other clone -- then exited 1 with an errno. Two separate faults, and the
second is the worse one: an exit code says "this did not work", and the user has
no way to learn that most of it did, or which path to go and look at.

So the purge now removes everything it is allowed to and reports the paths that
refused. The three outcomes #131 names -- removed everything, removed what it
was allowed to, removed nothing -- are two *decisions*: you are done, or these
paths need another pair of hands. "Removed nothing" is the degenerate case of
the second, where the list of refusals is the cache root itself, and it is told
apart by reading the report rather than by a third exit code.

No container is needed to test any of this. The failure is a filesystem
permission, so a directory this process cannot empty reproduces it exactly.
"""

import os
import pathlib
import subprocess
from typing import Iterator, List
from unittest.mock import patch

import pytest

from devlaunch.dl import purge_all_data, remove_tree

# Every assertion here rests on the process being refused by file permissions.
# root is refused by nothing, so under root these do not test a weaker version
# of the behaviour -- they test nothing at all, and would pass with the fix
# reverted. CI runs as an ordinary user; a container shell may not.
needs_an_unprivileged_user = pytest.mark.skipif(
    os.geteuid() == 0, reason="root can empty any directory, so nothing here can refuse"
)


def _sealed(directory: pathlib.Path) -> pathlib.Path:
    """Make *directory* one this process cannot empty, and return it.

    r-x: its contents stay listable, so a purge can walk in and discover that it
    cannot unlink anything. That is the container-written directory's shape --
    0755 owned by another uid -- reproduced by dropping write for ourselves
    instead of by owning it as someone else, which a test cannot do.
    """
    directory.chmod(0o500)
    return directory


class Cache:
    """A devlaunch cache directory, with a path in it that will refuse to go."""

    def __init__(self, root: pathlib.Path):
        self.root = root
        self.completions = root / "completions.json"
        self.metadata = root / "metadata.json"
        self.other_clone = root / "repos" / "blooop" / "bencher" / "bencher-main-kivagede"
        self.stuck = root / "repos" / "blooop" / "e2e-repo" / "e2e-purge-devlaunchs"

        self.other_clone.mkdir(parents=True)
        (self.other_clone / "README.md").write_text("a clone that will go\n")
        self.completions.write_text("{}")
        self.metadata.write_text("{}")

        # The clone the container wrote into: a file we will not be able to
        # unlink, because the directory holding it is not writable by us.
        self.stuck.mkdir(parents=True)
        (self.stuck / "pixi.lock").write_text("written by the container's user\n")

    def seal(self) -> None:
        _sealed(self.stuck)

    def unseal(self) -> None:
        """Let pytest's tmp_path teardown clean up after us."""
        for parent, dirs, _files in os.walk(self.root):
            for name in dirs:
                path = pathlib.Path(parent) / name
                if not path.is_symlink():
                    path.chmod(0o700)


@pytest.fixture(name="cache")
def fixture_cache(tmp_path) -> Iterator[Cache]:
    built = Cache(tmp_path / "devlaunch")
    try:
        yield built
    finally:
        if built.root.exists():
            built.unseal()


@pytest.fixture(name="purge")
def fixture_purge(cache):
    """`purge_all_data` pointed at *cache*, with devpod answering an empty list.

    No workspaces: this file is about the cache half of a purge, and #131 is
    explicit that the workspace half "did everything right".
    """
    empty_listing = subprocess.CompletedProcess([], 0, "[]", "")
    with patch("devlaunch.dl.subprocess.run", return_value=empty_listing):
        with patch("devlaunch.dl._get_cache_dir", return_value=cache.root):
            with patch("devlaunch.dl.update_cache_background"):
                yield purge_all_data


class TestARefusedPathDoesNotStopTheRest:
    """The fault #131 measured: one EACCES abandoned the entire cache."""

    @needs_an_unprivileged_user
    def test_everything_removable_is_removed(self, cache, purge):
        cache.seal()
        purge()
        assert not cache.completions.exists(), "a completion cache is removable and should go"
        assert not cache.metadata.exists(), "metadata.json is removable and should go"
        assert not cache.other_clone.exists(), "another clone is removable and should go"

    @needs_an_unprivileged_user
    def test_what_refused_is_still_there(self, cache, purge):
        """Not a consolation prize -- it is why the exit code is not 0.

        The purge must not report success while a clone it named is still on
        disk, and must not pretend to have removed something it did not.
        """
        cache.seal()
        assert purge() == 1
        assert (cache.stuck / "pixi.lock").exists()

    def test_a_cache_nothing_refuses_goes_completely(self, cache, purge):
        assert purge() == 0
        assert not cache.root.exists()


class TestTheReportIsActionable:
    """An errno is a fact about a syscall, not an answer to "now what"."""

    @needs_an_unprivileged_user
    def test_the_refused_path_is_named(self, cache, purge, capsys):
        cache.seal()
        purge()
        assert str(cache.stuck) in capsys.readouterr().out

    @needs_an_unprivileged_user
    def test_the_command_that_finishes_the_job_is_given(self, cache, purge, capsys):
        """The user is told the one thing that works, not left to compose it.

        `sudo` is what clears a directory owned by another uid, and the cache
        root is the argument -- naming the individual refusals there would leave
        the empty parents behind.
        """
        cache.seal()
        purge()
        printed = capsys.readouterr().out
        assert f"sudo rm -rf {cache.root}" in printed

    @needs_an_unprivileged_user
    def test_what_did_go_is_said_too(self, cache, purge, capsys):
        """ "Could not remove X" alone reads as "removed nothing"."""
        cache.seal()
        purge()
        printed = capsys.readouterr().out
        assert "Removed what was permitted" in printed


class TestOnlyTheObstructionIsNamed:
    """`remove_tree` on its own, where the reporting rule is decided."""

    @needs_an_unprivileged_user
    def test_ancestors_that_only_failed_because_of_it_are_not_listed(self, cache):
        cache.seal()
        refused = remove_tree(cache.root)
        assert refused == (cache.stuck / "pixi.lock",), (
            "every directory from the cache root down to the sealed one also fails "
            f"to go, and saying so five times buries the one fact: {refused}"
        )

    @needs_an_unprivileged_user
    def test_two_separate_obstructions_are_both_listed(self, cache):
        """Suppressing ancestors must not suppress siblings."""
        second = cache.root / "repos" / "blooop" / "other" / "clone"
        second.mkdir(parents=True)
        (second / "held").write_text("also stuck\n")
        _sealed(second)
        cache.seal()
        refused = remove_tree(cache.root)
        assert sorted(refused) == sorted([cache.stuck / "pixi.lock", second / "held"])

    def test_a_tree_that_goes_completely_refuses_nothing(self, cache):
        assert remove_tree(cache.root) == ()
        assert not cache.root.exists()

    def test_a_tree_that_is_not_there_refuses_nothing(self, tmp_path):
        """A purge run twice is not an error the second time."""
        assert remove_tree(tmp_path / "never-existed") == ()

    @needs_an_unprivileged_user
    def test_an_unreadable_directory_is_reported_rather_than_skipped(self, cache):
        """A directory that cannot even be listed must not pass for empty.

        `os.walk` reports a failure to scan through a callback and otherwise
        carries on silently, so without that callback the tree would be walked
        as though the directory held nothing, `rmdir` would fail on it, and the
        contents would be neither removed nor mentioned.
        """
        opaque = cache.root / "repos" / "blooop" / "opaque"
        opaque.mkdir(parents=True)
        (opaque / "inside").write_text("unreadable\n")
        opaque.chmod(0o300)  # -wx: enterable, not listable
        refused = remove_tree(cache.root)
        try:
            assert opaque in refused, f"the directory it could not read must be named: {refused}"
        finally:
            opaque.chmod(0o700)


class TestNothingToPurge:
    """The existing empty-cache path, kept honest while the rest changed."""

    def test_a_cache_that_was_never_made_reports_nothing_to_purge(self, cache, purge, capsys):
        cache.unseal()
        remove_tree(cache.root)
        assert purge() == 0
        assert "No data to purge." in capsys.readouterr().out


class TestRefusalsAreNotInvented:
    """The suppression rule must not swallow a real refusal.

    `remove_tree` decides not to mention a path when something below it already
    refused. A bug there is silent in exactly the direction that matters -- a
    purge reporting success it did not have -- so the rule is checked against a
    list built independently of it.
    """

    @needs_an_unprivileged_user
    def test_every_refused_path_is_still_on_disk_afterwards(self, cache):
        cache.seal()
        refused = remove_tree(cache.root)
        assert refused, "the sealed directory must produce at least one refusal"
        for path in refused:
            assert path.exists(), f"{path} was reported as refused but is gone"

    @needs_an_unprivileged_user
    def test_nothing_reported_as_removed_survives(self, cache):
        cache.seal()
        survivors: List[pathlib.Path] = []
        refused = remove_tree(cache.root)
        for parent, dirs, files in os.walk(cache.root):
            for name in files:
                survivors.append(pathlib.Path(parent) / name)
            if not dirs and not files:
                survivors.append(pathlib.Path(parent))
        unexplained = [
            path for path in survivors if not any(path.is_relative_to(r.parent) for r in refused)
        ]
        assert not unexplained, f"still on disk and not explained by any refusal: {unexplained}"
