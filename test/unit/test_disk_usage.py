"""What a directory costs, and what it would give back if it went away.

devlaunch shares git objects between a repo's workspaces on purpose: the bare
cache holds one copy and every workspace clone hardlinks out of it (devlaunch
#154), which is where most of the saving on a repo's second workspace comes
from. A number that walks one workspace and adds up file sizes reports that
saving as if it had never happened -- it bills every workspace for the whole
shared pool, so three workspaces of one repo read as three times the disk they
actually occupy.

So the number here is *exclusive* bytes: what deleting this directory would
free. A file counts only when every one of its links lies inside the tree being
measured. The tests below are the pin on that choice; swapping in an apparent
size fails the hardlink ones and nothing else.

The other property under test is that a measurement which could not be
completed never comes back as a number. A clone is written into by a container
running as another user, so a directory this process cannot read is the normal
case rather than the exotic one, and "at least N" is the truth about it.
"""

import contextlib
import os
from pathlib import Path
from unittest.mock import patch

import pytest

from devlaunch.disk_usage import (
    Measured,
    PartlyUnreadable,
    describe_usage,
    exclusive_usage,
    known_bytes,
    usage_as_json,
)

MIB = 1024 * 1024


def payload(path: Path, mib: int) -> None:
    """Write *mib* MiB of real bytes at *path*, making its parents."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"\0" * (mib * MIB))


def measured_bytes(usage) -> int:
    """The byte count of a complete measurement, failing if it was not one."""
    assert isinstance(usage, Measured), f"expected a complete measurement, got {usage!r}"
    return usage.exclusive_bytes


class TestWhatATreeHolds:
    def test_a_tree_of_known_content_reports_what_it_holds(self, tmp_path):
        for name in ("a.bin", "b.bin", "nested/c.bin", "nested/deep/d.bin"):
            payload(tmp_path / "tree" / name, 1)
        got = measured_bytes(exclusive_usage(tmp_path / "tree"))
        # Four MiB of payload, plus a handful of directory blocks -- and not
        # the eight MiB an implementation that counted each file twice reports.
        assert 4 * MIB <= got < 5 * MIB

    def test_a_directory_that_is_not_there_holds_nothing(self, tmp_path):
        assert exclusive_usage(tmp_path / "never-existed") == Measured(0)

    def test_an_empty_directory_costs_only_itself(self, tmp_path):
        (tmp_path / "empty").mkdir()
        assert measured_bytes(exclusive_usage(tmp_path / "empty")) < MIB

    def test_a_file_handed_in_instead_of_a_directory_is_worth_itself(self, tmp_path):
        # The caller that classifies clone directories walks whatever it found;
        # a stray file where a clone was expected is a measurement, not a crash.
        payload(tmp_path / "stray.bin", 1)
        assert MIB <= measured_bytes(exclusive_usage(tmp_path / "stray.bin")) < 2 * MIB

    def test_a_symlink_is_worth_its_own_size_not_its_targets(self, tmp_path):
        payload(tmp_path / "outside" / "big.bin", 4)
        (tmp_path / "tree").mkdir()
        (tmp_path / "tree" / "link").symlink_to(tmp_path / "outside" / "big.bin")
        assert measured_bytes(exclusive_usage(tmp_path / "tree")) < MIB


class TestSharingIsNotBilledTwice:
    """The exclusive-vs-apparent choice, pinned."""

    def test_a_file_hardlinked_from_outside_is_not_billed_to_the_tree(self, tmp_path):
        # The shape devlaunch actually creates: a bare cache holding the packs,
        # and a workspace clone whose pack file is the same inode.
        payload(tmp_path / "bare" / "shared.pack", 4)
        payload(tmp_path / "clone" / "own.bin", 1)
        os.link(tmp_path / "bare" / "shared.pack", tmp_path / "clone" / "shared.pack")

        got = measured_bytes(exclusive_usage(tmp_path / "clone"))
        # Deleting the clone frees its own MiB and nothing else: the bare keeps
        # the pack. An apparent size reports 5 MiB here.
        assert MIB <= got < 2 * MIB

    def test_the_holder_of_the_last_link_is_billed_for_it(self, tmp_path):
        # Same pool, measured from the side that is about to become the only
        # holder -- so the bytes are not simply lost from every report.
        payload(tmp_path / "bare" / "shared.pack", 4)
        (tmp_path / "clone").mkdir()
        os.link(tmp_path / "bare" / "shared.pack", tmp_path / "clone" / "shared.pack")
        (tmp_path / "clone" / "shared.pack").unlink()

        assert 4 * MIB <= measured_bytes(exclusive_usage(tmp_path / "bare")) < 5 * MIB

    def test_a_file_hardlinked_twice_within_the_tree_is_counted_once(self, tmp_path):
        payload(tmp_path / "tree" / "a.bin", 2)
        os.link(tmp_path / "tree" / "a.bin", tmp_path / "tree" / "b.bin")
        # Both links are inside, so the bytes are the tree's -- once.
        assert 2 * MIB <= measured_bytes(exclusive_usage(tmp_path / "tree")) < 3 * MIB


class TestWhatCouldNotBeRead:
    """No zero, and no wrong total, for a walk that hit a closed door."""

    @pytest.fixture(autouse=True)
    def _not_as_root(self):
        if os.geteuid() == 0:
            pytest.skip("root is refused by nothing, so the closed door would open")

    def test_an_unreadable_directory_makes_the_answer_a_floor(self, tmp_path):
        payload(tmp_path / "tree" / "readable.bin", 2)
        payload(tmp_path / "tree" / "locked" / "hidden.bin", 4)
        (tmp_path / "tree" / "locked").chmod(0o000)
        try:
            usage = exclusive_usage(tmp_path / "tree")
        finally:
            (tmp_path / "tree" / "locked").chmod(0o700)

        assert isinstance(usage, PartlyUnreadable)
        assert usage.unreadable == (tmp_path / "tree" / "locked",)
        # What was readable is still reported, as a floor: the hidden 4 MiB are
        # missing from it, which is exactly why it is not called a total.
        assert 2 * MIB <= usage.at_least_bytes < 3 * MIB

    def test_a_tree_that_cannot_be_opened_at_all_reports_no_total_either(self, tmp_path):
        (tmp_path / "tree").mkdir()
        (tmp_path / "tree").chmod(0o000)
        try:
            usage = exclusive_usage(tmp_path / "tree")
        finally:
            (tmp_path / "tree").chmod(0o700)
        assert isinstance(usage, PartlyUnreadable)
        assert usage.unreadable == (tmp_path / "tree",)

    def test_a_tree_behind_a_closed_parent_reports_no_total_either(self, tmp_path):
        # Not even the first `lstat` gets through -- that needs the parent to be
        # traversable -- so there is nothing to report but the closed door.
        payload(tmp_path / "outer" / "tree" / "file.bin", 1)
        (tmp_path / "outer").chmod(0o000)
        try:
            usage = exclusive_usage(tmp_path / "outer" / "tree")
        finally:
            (tmp_path / "outer").chmod(0o700)
        assert usage == PartlyUnreadable(0, (tmp_path / "outer" / "tree",))

    def test_entries_that_can_be_named_but_not_stat_ed_are_named(self, tmp_path):
        # Readable but not traversable: the names come back from the directory
        # itself, and every `stat` on them is refused. The bytes behind them are
        # unknown, so they are listed rather than quietly counted as nothing.
        payload(tmp_path / "tree" / "listed" / "hidden.bin", 2)
        (tmp_path / "tree" / "listed").chmod(0o444)
        try:
            usage = exclusive_usage(tmp_path / "tree")
        finally:
            (tmp_path / "tree" / "listed").chmod(0o700)
        assert isinstance(usage, PartlyUnreadable)
        assert tmp_path / "tree" / "listed" / "hidden.bin" in usage.unreadable


class TestWhatVanishesMidWalk:
    """A live cache changes under the walk, and that is not a closed door.

    git repacks, a container writes, a sibling command deletes: between reading
    a directory's names and weighing them, some of them stop existing. That is
    the ordinary weather on a cache being used, and a walk that reported a floor
    every time it happened would make `≥` the usual answer for no reason -- a
    thing that is not there frees nothing, which is a measurement, and it is the
    same answer the walk already gives for a clone that is not there at all.

    `os.scandir` is the only seam where the race can be staged, so it is where
    these tests stage it; the disappearing is real, only its timing is arranged.
    """

    def test_a_file_that_disappears_before_it_is_weighed_costs_nothing(self, tmp_path):
        payload(tmp_path / "tree" / "stays.bin", 2)
        payload(tmp_path / "tree" / "goes.bin", 4)
        real_scandir = os.scandir

        def racing(path):
            entries = list(real_scandir(path))
            (tmp_path / "tree" / "goes.bin").unlink(missing_ok=True)
            return contextlib.nullcontext(entries)

        with patch("devlaunch.disk_usage.os.scandir", racing):
            usage = exclusive_usage(tmp_path / "tree")

        # A total, not a floor -- and the 4 MiB that went away is not in it.
        assert 2 * MIB <= measured_bytes(usage) < 3 * MIB

    def test_a_directory_that_disappears_before_it_is_opened_costs_nothing(self, tmp_path):
        payload(tmp_path / "tree" / "stays.bin", 2)
        payload(tmp_path / "tree" / "goes" / "hidden.bin", 4)
        real_scandir = os.scandir
        gone = tmp_path / "tree" / "goes"

        def racing(path):
            if Path(path) == gone:
                raise FileNotFoundError(2, "No such file or directory", str(path))
            return real_scandir(path)

        with patch("devlaunch.disk_usage.os.scandir", racing):
            usage = exclusive_usage(tmp_path / "tree")

        assert 2 * MIB <= measured_bytes(usage) < 3 * MIB


class TestComparingUsagesWhateverArmTheyAre:
    """The accessor a caller that ranks or sums usages needs, so it does not
    reach into the arms and hand-roll an `else` a third arm would walk through.

    `dl --prune` (devlaunch#159) is the caller: "which of these is worth
    reclaiming first" is answerable from a floor as well as from a total, and it
    is the only question that is.
    """

    def test_a_measurement_offers_its_total(self):
        assert known_bytes(Measured(1536)) == 1536

    def test_a_floor_offers_the_floor(self):
        assert known_bytes(PartlyUnreadable(1536, (Path("/x"),))) == 1536

    def test_it_is_not_how_a_usage_is_reported(self):
        # The same number from either arm, which is exactly why the renderings
        # do not go through here: only they keep the floor visible as one.
        floor = PartlyUnreadable(1536, (Path("/x"),))
        assert known_bytes(floor) == known_bytes(Measured(1536))
        assert describe_usage(floor) != describe_usage(Measured(1536))


class TestHowAUsageReads:
    @pytest.mark.parametrize(
        "measured,expected",
        [
            (0, "0 B"),
            (512, "512 B"),
            (1536, "1.5 KiB"),
            (512_540_672, "488.8 MiB"),
            (1_824_649_216, "1.7 GiB"),
        ],
    )
    def test_a_measurement_reads_as_a_size(self, measured, expected):
        assert describe_usage(Measured(measured)) == expected

    def test_a_floor_reads_as_a_floor(self):
        assert describe_usage(PartlyUnreadable(1536, (Path("/x"),))) == "≥1.5 KiB"

    def test_a_measurement_is_json_as_the_bytes_it_is(self):
        assert usage_as_json(Measured(1536)) == {"exclusiveBytes": 1536}

    def test_a_floor_is_json_that_cannot_be_mistaken_for_a_total(self):
        assert usage_as_json(PartlyUnreadable(1536, (Path("/x"), Path("/y")))) == {
            "atLeastBytes": 1536,
            "unreadable": 2,
        }
