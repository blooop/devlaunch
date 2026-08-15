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
paths need another pair of hands, and the exit status carries only that.

The three are still three, though, and collapsing them into the exit status is
not the same as collapsing them into the report. devlaunch#182 is what happened
when the removal's answer carried only the refusals: "one clone stayed behind"
and "not a byte of it moved" arrived at the caller as the same value, and the
second printed the first's sentence. The removal now answers with which of the
three happened and the purge has a sentence for each -- one exit status, three
headlines.

No container is needed to test any of this. The failure is a filesystem
permission, so a directory this process cannot empty reproduces it exactly.
"""

import contextlib
import os
import pathlib
import random
import subprocess
from typing import Iterator, List
from unittest.mock import patch

import pytest

from devlaunch.dl import (
    RemovedEverything,
    RemovedNothing,
    RemovedWhatItCould,
    purge_all_data,
    remove_tree,
)


def refusals(tree) -> tuple:
    """What *tree*'s removal refused, whatever arm it came back as.

    The arm is the subject of one test class and beside the point in the rest,
    which are about *which paths* are named and why -- so this reads the
    refusals out of either arm that has them, and calls a clean sweep an empty
    list rather than making every caller say `RemovedEverything()`.
    """
    outcome = remove_tree(tree)
    if isinstance(outcome, RemovedEverything):
        return ()
    return outcome.refused


def refused_paths(tree) -> list:
    """`remove_tree` reports a path *and* what the system said about it; most
    assertions here are about which paths, so this drops the reasons."""
    return [refusal.path for refusal in refusals(tree)]


def still_there(path: pathlib.Path) -> bool:
    """Whether *path* is on disk, for a test that seals directories.

    Not `Path.exists()`, for the same reason the code under test does not use
    it: with an unreadable parent it returns False on Python 3.14 and raises
    PermissionError on 3.13, and here both would be reporting "gone" about
    something present. Only FileNotFoundError means gone.
    """
    try:
        os.lstat(path)
    except FileNotFoundError:
        return False
    except OSError:
        return True
    return True


def _link(link: pathlib.Path, target: pathlib.Path) -> None:
    """Plant a symlink, tolerating a name the trial already used."""
    try:
        link.symlink_to(target)
    except OSError:
        pass


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


@pytest.fixture(name="sealed_root")
def fixture_sealed_root(tmp_path) -> Iterator[pathlib.Path]:
    """A cache where not one path can be removed, and it takes some care to build.

    Sealing the root is not on its own enough, which the first attempt at this
    fixture got wrong and the test caught: unlinking an entry needs write
    permission on the *directory holding it*, so a sealed root refuses its own
    entries while everything deeper down stays as removable as it ever was. A
    sealed root over a real clone tree therefore empties the clones and is an
    honest partial success -- pinned below, because that is the reading the
    third arm must not steal.

    So this is the cache that has none: the shape devlaunch writes before the
    first clone exists -- the completion caches, metadata.json and an empty
    `repos` -- under a root that will not let any of them go.
    """
    root = tmp_path / "devlaunch"
    (root / "repos").mkdir(parents=True)
    (root / "metadata.json").write_text("{}")
    (root / "completions.json").write_text("{}")
    root.chmod(0o500)
    try:
        yield root
    finally:
        root.chmod(0o700)


@contextlib.contextmanager
def purging(root: pathlib.Path):
    """`purge_all_data` pointed at *root*, with devpod answering an empty list.

    No workspaces: this file is about the cache half of a purge, and #131 is
    explicit that the workspace half "did everything right".
    """
    empty_listing = subprocess.CompletedProcess([], 0, "[]", "")
    with patch("devlaunch.dl.subprocess.run", return_value=empty_listing):
        with patch("devlaunch.dl._get_cache_dir", return_value=root):
            with patch("devlaunch.dl.update_cache_background"):
                yield purge_all_data


@pytest.fixture(name="purge")
def fixture_purge(cache):
    with purging(cache.root) as run:
        yield run


@pytest.fixture(name="purge_sealed_root")
def fixture_purge_sealed_root(sealed_root):
    """A whole purge of the cache where nothing at all can be removed."""
    with purging(sealed_root) as run:
        yield run


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


class TestThePurgeSaysWhichOfTheThreeHappened:
    """devlaunch#182: the headline claimed a partial success that never happened.

    One sentence per arm, and each of the three has to be false of the other
    two. The exit code deliberately stays two-valued -- zero means the cache is
    gone and nothing else does -- so the sentence is the only place the
    difference between "one clone stayed" and "nothing moved" is carried, and
    it is the part somebody reads before deciding whether to go and look.
    """

    @needs_an_unprivileged_user
    def test_a_purge_that_removed_nothing_says_nothing_was_removed(
        self, purge_sealed_root, sealed_root, capsys
    ):
        assert purge_sealed_root() == 1
        printed = capsys.readouterr().out
        assert f"Removed nothing under {sealed_root}." in printed, printed

    @needs_an_unprivileged_user
    def test_a_purge_that_removed_nothing_does_not_claim_a_partial_success(
        self, purge_sealed_root, capsys
    ):
        """The ticket's sentence, and the whole reason for the third arm."""
        purge_sealed_root()
        printed = capsys.readouterr().out
        assert "Removed what was permitted" not in printed, printed

    @needs_an_unprivileged_user
    def test_a_purge_that_removed_some_of_it_still_says_that(self, cache, purge, capsys):
        """The over-correction guard: two of these are refusals, not one.

        A fix that renamed the refusal sentence rather than splitting it would
        pass the test above and tell somebody whose completion caches, metadata
        and other clones all went that nothing was removed.
        """
        cache.seal()
        assert purge() == 1
        printed = capsys.readouterr().out
        assert f"Removed what was permitted under {cache.root}." in printed, printed
        assert "Removed nothing" not in printed, printed

    def test_a_clean_sweep_says_neither_of_the_refusal_sentences(self, cache, purge, capsys):
        assert purge() == 0
        printed = capsys.readouterr().out
        assert f"Removed: {cache.root}" in printed, printed
        assert "Removed nothing" not in printed, printed
        assert "Removed what was permitted" not in printed, printed

    @needs_an_unprivileged_user
    def test_removing_nothing_still_names_the_paths_and_the_way_out(
        self, purge_sealed_root, sealed_root, capsys
    ):
        """The third sentence replaces a headline, not the report under it.

        "Removed nothing" on its own is the errno-only report #131 removed,
        wearing different words: the paths and the command that usually clears
        them are what somebody acts on.
        """
        purge_sealed_root()
        printed = capsys.readouterr().out
        assert f"  - {sealed_root}: Permission denied" in printed.splitlines(), printed
        assert f"sudo rm -rf {sealed_root}" in printed, printed


class TestWhetherAnythingCameAwayIsPartOfTheAnswer:
    """Three arms, because a removal that may remove *some* of a tree has three.

    A flat list of refusals records what was refused and not whether anything
    went, so "one clone stayed behind" and "not a byte of it moved" arrive at a
    caller as the same value -- and devlaunch#182 is what that costs: a purge
    that removed nothing printed the sentence for a partial success.

    The distinction is decided here rather than re-derived at each call site,
    and it is a *type* rather than a `(removed_something, refused)` pair
    because a pair can say "removed everything, and here is what it refused".
    These arms cannot say it: the arm that means a clean sweep has nowhere to
    put a refusal.
    """

    def test_a_tree_that_goes_completely_says_so_and_carries_no_refusals(self, cache):
        assert remove_tree(cache.root) == RemovedEverything()

    @needs_an_unprivileged_user
    def test_a_tree_that_partly_went_is_told_apart_from_one_that_did_not(self, cache):
        """The distinction the exit code could not carry, at the seam that knows.

        Everything but the sealed clone is removable here, so this is a genuine
        partial success -- and the check that it went is what stops the arm
        being a label the code is free to get wrong.
        """
        cache.seal()
        outcome = remove_tree(cache.root)
        assert isinstance(outcome, RemovedWhatItCould), outcome
        assert [refusal.path for refusal in outcome.refused] == [cache.stuck]
        assert not cache.metadata.exists(), "the partial arm has to mean something went"
        assert not cache.other_clone.exists()

    @needs_an_unprivileged_user
    def test_a_cache_root_that_refuses_everything_reports_that_nothing_went(self, sealed_root):
        """devlaunch#182's case: the root itself is what will not let go.

        Nothing under it can be unlinked either, since unlinking an entry needs
        write permission on the directory holding it -- so the whole cache is
        standing afterwards and the honest answer names no removal at all.
        """
        outcome = remove_tree(sealed_root)
        assert isinstance(outcome, RemovedNothing), outcome
        assert [refusal.path for refusal in outcome.refused] == [sealed_root]
        assert (sealed_root / "metadata.json").read_text() == "{}"
        assert (sealed_root / "completions.json").exists()
        assert (sealed_root / "repos").is_dir()

    @needs_an_unprivileged_user
    def test_a_sealed_root_over_clones_that_did_go_is_still_a_partial_success(self, tmp_path):
        """The arm is decided by what moved, not by where the obstruction is.

        A sealed root refuses its own entries and nothing deeper: the clones
        under it are held in directories that are still writable, so they go.
        Reading "the root refused" as "nothing came away" would call this one
        removed-nothing and tell somebody their clones survived when they did
        not -- which is the same class of lie as devlaunch#182, pointed the
        other way.
        """
        root = tmp_path / "devlaunch"
        clone = root / "repos" / "blooop" / "bencher" / "bencher-main-kivagede"
        clone.mkdir(parents=True)
        (clone / "README.md").write_text("a clone that will go\n")
        root.chmod(0o500)
        try:
            outcome = remove_tree(root)
            assert isinstance(outcome, RemovedWhatItCould), outcome
            assert not clone.exists(), "the clones under a sealed root are still removable"
        finally:
            root.chmod(0o700)

    @needs_an_unprivileged_user
    def test_a_root_that_cannot_even_be_looked_at_removed_nothing(self, tmp_path):
        """ "Cannot tell" is not a partial success either.

        The lstat is refused before a single path is attempted, so there is
        nothing this could have removed -- and the arm that says so is the one
        that cannot be mistaken for progress.
        """
        home = tmp_path / "cachehome"
        root = home / "devlaunch"
        root.mkdir(parents=True)
        (root / "metadata.json").write_text("still here")
        home.chmod(0o600)  # rw-, not traversable: lstat on root raises EACCES
        try:
            outcome = remove_tree(root)
            assert isinstance(outcome, RemovedNothing), outcome
            assert [refusal.path for refusal in outcome.refused] == [root]
        finally:
            home.chmod(0o700)

    def test_a_symlinked_root_removed_nothing(self, tmp_path):
        """Refusing to follow a link is a refusal to remove, not a clean sweep.

        This one needs no permissions to reproduce, so it holds as root too --
        which matters, because it is the arm a container running as root would
        otherwise never exercise.
        """
        target = tmp_path / "elsewhere"
        target.mkdir()
        (target / "metadata.json").write_text("somebody's cache")
        link = tmp_path / "cache" / "devlaunch"
        link.parent.mkdir()
        link.symlink_to(target)

        outcome = remove_tree(link)
        assert isinstance(outcome, RemovedNothing), outcome
        assert [refusal.path for refusal in outcome.refused] == [link]

    def test_a_tree_that_was_never_there_is_a_clean_sweep_not_a_refusal(self, tmp_path):
        """A purge run twice is not a failure the second time, and is not a
        removal that refused nothing while removing nothing either: there is
        nothing left under that name, which is what the first arm means."""
        assert remove_tree(tmp_path / "never-existed") == RemovedEverything()


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
        assert refusals(cache.root) == ()
        assert not cache.root.exists()

    def test_a_tree_that_is_not_there_refuses_nothing(self, tmp_path):
        """A purge run twice is not an error the second time."""
        assert refusals(tmp_path / "never-existed") == ()

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
        assert refusals(cache.root) == ()
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

    def test_a_symlinked_root_is_refused_and_left_where_it_is(self, tmp_path):
        """Refused, not followed and not quietly unlinked.

        Unlinking only the link was the first attempt at this and it is the
        wrong answer for a reason worth writing down: it reports a clean sweep
        over clones that are still on disk on another volume, which is the
        failure direction this whole change exists to remove. A cache root is a
        symlink because somebody moved their cache, so following it and
        unlinking it cost them the same thing by opposite routes -- one deletes
        the workspaces, the other says they are gone.
        """
        target = tmp_path / "elsewhere"
        (target / "repos").mkdir(parents=True)
        (target / "metadata.json").write_text("somebody's cache")
        (target / "repos" / "work.txt").write_text("somebody's work")
        link = tmp_path / "cache" / "devlaunch"
        link.parent.mkdir()
        link.symlink_to(target)

        refused = refusals(link)
        assert [r.path for r in refused] == [link]
        assert link.is_symlink(), "the link is left where it is, not silently removed"
        assert (target / "metadata.json").read_text() == "somebody's cache"
        assert (target / "repos" / "work.txt").exists()
        # The advice is `sudo rm -rf <cache>`, which would remove the link and
        # nothing else, so the reason has to carry the real location.
        assert str(target) in refused[0].reason, refused[0].reason

    def test_a_purge_of_a_symlinked_cache_does_not_report_success(self, tmp_path, capsys):
        """The end-to-end half, because the exit code is the whole point."""
        target = tmp_path / "elsewhere"
        target.mkdir()
        (target / "metadata.json").write_text("somebody's cache")
        root = tmp_path / "cache" / "devlaunch"
        root.parent.mkdir()
        root.symlink_to(target)

        listing = subprocess.CompletedProcess([], 0, "[]", "")
        with patch("devlaunch.dl.subprocess.run", return_value=listing):
            with patch("devlaunch.dl._get_cache_dir", return_value=root):
                with patch("devlaunch.dl.update_cache_background"):
                    code = purge_all_data()
        printed = capsys.readouterr().out
        assert code == 1, printed
        assert "Removed:" not in printed, printed
        assert (target / "metadata.json").read_text() == "somebody's cache"

    def test_a_symlink_inside_the_tree_is_unlinked_not_followed(self, cache, tmp_path):
        outside = tmp_path / "outside"
        outside.mkdir()
        (outside / "precious.txt").write_text("not devlaunch's")
        (cache.root / "repos" / "link").symlink_to(outside)
        (cache.root / "repos" / "file-link").symlink_to(outside / "precious.txt")

        assert refusals(cache.root) == ()
        assert not cache.root.exists()
        assert (outside / "precious.txt").read_text() == "not devlaunch's"

    def test_a_dangling_symlink_is_removed_without_complaint(self, cache):
        (cache.root / "repos" / "broken").symlink_to(cache.root / "never-existed")
        assert refusals(cache.root) == ()
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

        A cache whose *parent* cannot be traversed used to come out as "No data
        to purge." and exit 0 with the cache fully intact -- a clean sweep
        reported over untouched data, which is the one failure this whole change
        exists to prevent.

        `Path.exists()` is what could not tell the two apart, and it is not even
        consistent about how it fails to: on Python 3.14 it returns False here,
        and on 3.13 it raises PermissionError. So the code this replaced
        answered wrongly on one version and crashed outright on the next, which
        CI found and my machine could not. The premise below accepts either,
        because the point is that neither is usable.
        """
        home = tmp_path / "cachehome"
        root = home / "devlaunch"
        root.mkdir(parents=True)
        (root / "metadata.json").write_text("still here")
        home.chmod(0o600)  # rw-, not traversable: lstat on root raises EACCES
        listing = subprocess.CompletedProcess([], 0, "[]", "")
        try:
            try:
                naive = root.exists()
            except OSError:
                naive = False
            assert not naive, "the premise: the naive check cannot see it"
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
            assert still_there(path), f"{path} was reported as refused but is gone"

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

        A third, which only symlinks can break: **nothing outside the tree is
        touched.** Every trial plants links to a canary directory alongside the
        tree, and the canary's contents are checked afterwards.

        This has found two real defects that hand-written cases missed. An
        unlistable *empty* directory made `os.walk` raise, and reporting at the
        point of raising named a path the following `rmdir` went on to remove --
        fixed by deciding refusals from the disk once the walk is over. And a
        symlinked tree root was followed rather than unlinked, which emptied the
        canary and reported a clean sweep.

        Seeded, so a failure here is reproducible rather than a rumour.
        """
        rng = random.Random(20260808)
        canary = tmp_path / "canary"
        canary.mkdir()
        (canary / "precious").write_text("outside the tree")

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
                roll = rng.random()
                if roll < 0.15:
                    _link(directory / f"l{rng.randrange(3)}", canary)
                elif roll < 0.25:
                    _link(directory / f"l{rng.randrange(3)}", canary / "precious")
                elif roll < 0.3:
                    _link(directory / f"l{rng.randrange(3)}", tmp_path / "nowhere")
            # Deepest first: sealing a parent would make sealing its child fail.
            for directory in sorted(made, key=lambda p: -len(p.parts)):
                if rng.random() < 0.25:
                    directory.chmod(rng.choice([0o000, 0o100, 0o300, 0o400, 0o500]))

            refused = refused_paths(root)
            try:
                assert still_there(root) == bool(refused), (
                    f"trial {trial}: tree survives={still_there(root)} but refused={refused}"
                )
                for path in refused:
                    assert still_there(path), f"trial {trial}: reported {path}, which is not there"
                assert len(set(refused)) == len(refused), f"trial {trial}: duplicates in {refused}"
                assert (canary / "precious").read_text() == "outside the tree", (
                    f"trial {trial}: a symlink was followed out of the tree"
                )
            finally:
                subprocess.run(["chmod", "-R", "u+rwx", str(root)], check=False)
