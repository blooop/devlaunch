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
import random
import subprocess
from typing import Iterator, List
from unittest.mock import patch

import pytest

from devlaunch.dl import purge_all_data, remove_tree


def refused_paths(tree) -> list:
    """`remove_tree` reports a path *and* what the system said about it; most
    assertions here are about which paths, so this drops the reasons."""
    return [refusal.path for refusal in remove_tree(tree)]


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
    def test_the_refused_path_is_named_with_what_the_system_said(self, cache, purge, capsys):
        """The reason is carried, not reconstructed from the path.

        A container writing as another user is the common cause and the one
        #131 is about, but a read-only mount, `chattr +i` and a busy mountpoint
        reach the same report -- and for the last two the advice below does not
        work. Printing the errno keeps the report honest about which it is.
        """
        cache.seal()
        purge()
        assert f"  - {cache.stuck}: Permission denied" in capsys.readouterr().out.splitlines()

    @needs_an_unprivileged_user
    def test_the_command_that_finishes_the_job_is_given(self, cache, purge, capsys):
        """The user is told the one thing that usually works, not left to
        compose it. The cache root is the argument -- naming the individual
        refusals there would leave the empty parents behind."""
        cache.seal()
        purge()
        printed = capsys.readouterr().out
        assert f"sudo rm -rf {cache.root}" in printed

    def test_the_path_in_that_command_is_shell_quoted(self, tmp_path, capsys):
        """It is handed to a person to paste into `sudo rm -rf`.

        $XDG_CACHE_HOME and $HOME are the user's to choose and a space in
        either is legal. Unquoted, `sudo rm -rf /home/ada/My Cache/devlaunch`
        pastes as two targets, and the first of them is a directory nobody
        asked to remove. This is the one place quoting is not cosmetic.
        """
        root = tmp_path / "My Cache" / "devlaunch"
        stuck = root / "repos" / "held"
        stuck.mkdir(parents=True)
        (stuck / "file").write_text("x")
        stuck.chmod(0o500)
        listing = subprocess.CompletedProcess([], 0, "[]", "")
        try:
            with patch("devlaunch.dl.subprocess.run", return_value=listing):
                with patch("devlaunch.dl._get_cache_dir", return_value=root):
                    with patch("devlaunch.dl.update_cache_background"):
                        purge_all_data()
            printed = capsys.readouterr().out
            assert f"sudo rm -rf '{root}'" in printed, printed
        finally:
            stuck.chmod(0o700)

    @needs_an_unprivileged_user
    def test_what_did_go_is_said_too(self, cache, purge, capsys):
        """ "Could not remove X" alone reads as "removed nothing"."""
        cache.seal()
        purge()
        printed = capsys.readouterr().out
        assert "Removed what was permitted" in printed

    @needs_an_unprivileged_user
    def test_the_cause_is_offered_rather_than_asserted(self, cache, purge, capsys):
        """The code never looks at the errno it caught, so it cannot know.

        It used to say "Written by a container running as a different user."
        flatly, which is wrong for every non-permission cause and for a
        directory the user sealed themselves.
        """
        cache.seal()
        purge()
        printed = capsys.readouterr().out
        assert "Usually this means" in printed
        assert "does not fix all of them" in printed


class TestOnlyTheObstructionIsNamed:
    """`remove_tree` on its own, where the reporting rule is decided."""

    @needs_an_unprivileged_user
    def test_ancestors_that_only_failed_because_of_it_are_not_listed(self, cache):
        cache.seal()
        refused = refused_paths(cache.root)
        assert refused == [cache.stuck], (
            "every directory from the cache root down to the sealed one also fails "
            f"to go, and saying so five times buries the one fact: {refused}"
        )

    @needs_an_unprivileged_user
    def test_the_directory_is_blamed_rather_than_each_file_in_it(self, cache):
        """The shape a real workspace has, and the reason this rule exists.

        Unlinking needs write permission on the *directory*, not on the file, so
        a clone owned by the container's user refuses every one of its children
        separately -- and none of them is an ancestor of another, so ancestor
        suppression alone catches none of them. On a real e2e workspace that was
        forty-odd `.git/objects` entries, hooks and a README reported one per
        line, all saying the same thing. The obstruction is the directory, which
        is also what the original errno named.
        """
        for name in ("README.md", "pyproject.toml", "config"):
            (cache.stuck / name).write_text("also written by the container\n")
        (cache.stuck / "objects").mkdir()
        cache.seal()
        assert refused_paths(cache.root) == [cache.stuck]

    @needs_an_unprivileged_user
    def test_two_separate_obstructions_are_both_listed(self, cache):
        """Suppressing ancestors must not suppress siblings."""
        second = cache.root / "repos" / "blooop" / "other" / "clone"
        second.mkdir(parents=True)
        (second / "held").write_text("also stuck\n")
        _sealed(second)
        cache.seal()
        refused = refused_paths(cache.root)
        assert sorted(refused) == sorted([cache.stuck, second])

    @needs_an_unprivileged_user
    def test_a_separately_sealed_ancestor_is_reported_as_well(self, cache):
        """Two sealed directories on one chain are two obstructions, not one.

        This is where "ancestors are not listed" stops being the right rule.
        The outer one does not fail *because* of the inner one -- clearing the
        inner would leave the outer exactly as stuck -- so each is a separate
        piece of work and a person told only about the inner one would fix it
        and find the purge still refusing.
        """
        outer = cache.root / "repos" / "outer"
        inner = outer / "middle" / "inner"
        inner.mkdir(parents=True)
        (inner / "file").write_text("x")
        _sealed(inner)
        _sealed(outer)
        try:
            assert sorted(refused_paths(cache.root)) == sorted([inner, outer])
        finally:
            outer.chmod(0o700)
            inner.chmod(0o700)

    @needs_an_unprivileged_user
    def test_a_path_whose_parent_is_writable_is_blamed_itself(self, cache):
        """Attribution walks up only as far as the permissions justify.

        Without this, a refusal in a perfectly writable directory would be
        blamed on an ancestor that has nothing wrong with it.
        """
        held = cache.root / "repos" / "blooop" / "held-open"
        held.mkdir(parents=True)
        (held / "inner").write_text("x\n")
        held.chmod(0o500)
        # `held`'s own parent is writable, so `held` is where the trail stops.
        assert refused_paths(cache.root) == [held]
        held.chmod(0o700)

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
        refused = refused_paths(cache.root)
        try:
            assert opaque in refused, f"the directory it could not read must be named: {refused}"
        finally:
            opaque.chmod(0o700)

    @needs_an_unprivileged_user
    def test_an_unlistable_but_empty_directory_is_not_reported(self, cache):
        """The other half of the case above, and it goes the opposite way.

        `os.walk` cannot scan this one either and says so -- but it is empty, so
        the `rmdir` afterwards succeeds and there is nothing left to report.
        Treating the scan failure as the refusal named a path that is not there,
        and, through the ancestor rule, could have silenced a genuine refusal
        above it. Found by the randomised trees in TestRefusalsAreNotInvented,
        not by reading the code.
        """
        opaque = cache.root / "repos" / "blooop" / "opaque"
        opaque.mkdir(parents=True)
        opaque.chmod(0o300)  # -wx and empty: unlistable, but it will go
        assert remove_tree(cache.root) == ()
        assert not cache.root.exists()


class TestNothingOutsideTheTreeIsTouched:
    """A purge removes the directory it was named. Not what it points at.

    `shutil.rmtree` refuses a symlinked root outright ("Cannot call rmtree on a
    symbolic link"), and losing that refusal is how a hand-rolled walk becomes
    dangerous rather than merely wrong: `os.walk`'s `followlinks=False` governs
    *subdirectories*, and the top is scanned whatever it is. So the version of
    this that only guarded inner symlinks descended a symlinked cache, emptied
    somebody's directory somewhere else entirely, and returned no refusals --
    a silent recursive delete outside the named tree, reported as a clean sweep.
    """

    def test_a_symlinked_root_is_unlinked_and_its_target_left_alone(self, tmp_path):
        target = tmp_path / "elsewhere"
        (target / "repos").mkdir(parents=True)
        (target / "metadata.json").write_text("somebody's cache")
        (target / "repos" / "work.txt").write_text("somebody's work")
        link = tmp_path / "cache" / "devlaunch"
        link.parent.mkdir()
        link.symlink_to(target)

        assert remove_tree(link) == ()
        assert not link.is_symlink(), "the link itself is devlaunch's to remove"
        assert (target / "metadata.json").read_text() == "somebody's cache"
        assert (target / "repos" / "work.txt").exists()

    def test_a_symlink_inside_the_tree_is_unlinked_not_followed(self, cache, tmp_path):
        outside = tmp_path / "outside"
        outside.mkdir()
        (outside / "precious.txt").write_text("not devlaunch's")
        (cache.root / "repos" / "link").symlink_to(outside)
        (cache.root / "repos" / "file-link").symlink_to(outside / "precious.txt")

        assert remove_tree(cache.root) == ()
        assert not cache.root.exists()
        assert (outside / "precious.txt").read_text() == "not devlaunch's"

    def test_a_dangling_symlink_is_removed_without_complaint(self, cache):
        (cache.root / "repos" / "broken").symlink_to(cache.root / "never-existed")
        assert remove_tree(cache.root) == ()
        assert not cache.root.exists()


class TestNothingToPurge:
    """The existing empty-cache path, kept honest while the rest changed."""

    def test_a_cache_that_was_never_made_reports_nothing_to_purge(self, cache, purge, capsys):
        cache.unseal()
        remove_tree(cache.root)
        assert purge() == 0
        assert "No data to purge." in capsys.readouterr().out

    @needs_an_unprivileged_user
    def test_a_cache_that_cannot_be_looked_at_is_not_mistaken_for_absent(self, tmp_path, capsys):
        """ "Cannot tell" is not "gone", and only one of them means success.

        `Path.exists()` answers False for both, so a cache whose *parent*
        cannot be traversed used to come out as "No data to purge." and exit 0
        with the cache fully intact -- a clean sweep reported over untouched
        data, which is the one failure this whole change exists to prevent.
        """
        home = tmp_path / "cachehome"
        root = home / "devlaunch"
        root.mkdir(parents=True)
        (root / "metadata.json").write_text("still here")
        home.chmod(0o600)  # rw-, not traversable: lstat on root raises EACCES
        listing = subprocess.CompletedProcess([], 0, "[]", "")
        try:
            assert not root.exists(), "the premise: exists() cannot see it"
            with patch("devlaunch.dl.subprocess.run", return_value=listing):
                with patch("devlaunch.dl._get_cache_dir", return_value=root):
                    with patch("devlaunch.dl.update_cache_background"):
                        code = purge_all_data()
            printed = capsys.readouterr().out
            assert code == 1, printed
            assert "No data to purge." not in printed, printed
            assert str(root) in printed, printed
        finally:
            home.chmod(0o700)
            assert (root / "metadata.json").read_text() == "still here"


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
        refused = refused_paths(cache.root)
        assert refused, "the sealed directory must produce at least one refusal"
        for path in refused:
            assert path.exists(), f"{path} was reported as refused but is gone"

    @needs_an_unprivileged_user
    def test_nothing_reported_as_removed_survives(self, cache):
        cache.seal()
        survivors: List[pathlib.Path] = []
        refused = refused_paths(cache.root)
        for parent, dirs, files in os.walk(cache.root):
            for name in files:
                survivors.append(pathlib.Path(parent) / name)
            if not dirs and not files:
                survivors.append(pathlib.Path(parent))
        unexplained = [
            path for path in survivors if not any(path.is_relative_to(r.parent) for r in refused)
        ]
        assert not unexplained, f"still on disk and not explained by any refusal: {unexplained}"

    @needs_an_unprivileged_user
    def test_the_two_invariants_hold_over_randomised_trees(self, tmp_path):
        """Hand-built cases check the shapes I thought of. This checks the rest.

        Two invariants, and between them they are the whole contract:

        - **nothing survives unsaid** -- a tree still on disk with an empty
          refusal list is a purge claiming a clean sweep it did not have, and is
          the only failure here that costs anybody anything;
        - **nothing is said that is not there** -- naming a path the user then
          cannot find is how a report stops being believed.

        This found a real defect that four hand-written cases missed: an
        unlistable *empty* directory made `os.walk` raise, and reporting at the
        point of raising named a path the following `rmdir` went on to remove.
        Fixed by deciding refusals from the disk once the walk is over, which
        also makes both invariants hold by construction rather than by care.

        Seeded, so a failure here is reproducible rather than a rumour.
        """
        rng = random.Random(20260808)
        for trial in range(60):
            root = tmp_path / f"tree{trial}"
            root.mkdir()
            made = [root]
            for _ in range(rng.randint(0, 10)):
                child = rng.choice(made) / f"d{rng.randrange(4)}"
                child.mkdir(exist_ok=True)
                if child not in made:
                    made.append(child)
            for directory in made:
                for _ in range(rng.randrange(3)):
                    (directory / f"f{rng.randrange(4)}").write_text("x")
            # Deepest first: sealing a parent would make sealing its child fail.
            for directory in sorted(made, key=lambda p: -len(p.parts)):
                if rng.random() < 0.25:
                    directory.chmod(rng.choice([0o000, 0o100, 0o300, 0o500]))

            refused = refused_paths(root)
            try:
                assert root.exists() == bool(refused), (
                    f"trial {trial}: tree survives={root.exists()} but refused={refused}"
                )
                for path in refused:
                    assert path.exists() or path.is_symlink(), (
                        f"trial {trial}: reported {path}, which is not there"
                    )
                assert len(set(refused)) == len(refused), f"trial {trial}: duplicates in {refused}"
            finally:
                subprocess.run(["chmod", "-R", "u+rwx", str(root)], check=False)
