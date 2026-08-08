"""`dl --purge` deletes what devlaunch made, and names what it leaves.

`purge_all_data` used to iterate everything `devpod list` returned. devpod's
namespace is shared with every other way a person makes a workspace -- a hand-run
`devpod up`, another tool, an older devlaunch -- so "every workspace" meant other
people's work too.

The listing below is a recording, not an invention: it is the shape
`devpod list --output json` returned on the host this ticket was worked on, with
the home directory renamed. Four of the six are clones devlaunch made under its
own cache. `pythontemplate` is the workspace #103 measured an unscoped `--purge`
would have deleted, and `devlaunch` is a plain `devpod up` of a checkout. Neither
of those two can be recreated by devlaunch, and neither is touched by the cache
directory a purge removes.
"""

import json
import pathlib
import subprocess
from typing import Iterator, List, Sequence
from unittest.mock import patch

import pytest

from devlaunch.dl import (
    GitRepository,
    LocalFolder,
    UnrecognisedSource,
    Workspace,
    is_devlaunch_clone,
    main,
    purge_all_data,
    workspace_ownership,
)

# The leaf devlaunch's cache directory always has, under XDG_CACHE_HOME or ~/.cache.
CACHE_LEAF = "devlaunch"

CLONED_BY_DEVLAUNCH = [
    "bencher-test1-pipagito",
    "bencher-main-kivagede",
    "devlaunch-main-zovomobo",
    "devlaunch-t1-vebilote",
]
MADE_BY_SOMEONE_ELSE = ["devlaunch", "pythontemplate"]


def _entry(workspace_id: str, source) -> dict:
    """One `devpod list --output json` element."""
    return {
        "id": workspace_id,
        "source": {"localFolder": str(source)},
        "provider": {"name": "docker"},
        "ide": {"name": "none"},
        "lastUsed": "2026-08-08T11:43:27Z",
    }


def _recorded_listing(cache_dir: pathlib.Path) -> str:
    """The recorded six-workspace listing, rehomed under *cache_dir*.

    The two foreign workspaces are interleaved with the four clones rather than
    appended, because a split that happened to keep listing order would pass a
    test where they were not.
    """
    repos = cache_dir / "repos" / "blooop"
    return json.dumps(
        [
            _entry("bencher-test1-pipagito", repos / "bencher" / "bencher-test1-pipagito"),
            _entry("bencher-main-kivagede", repos / "bencher" / "bencher-main-kivagede"),
            _entry("devlaunch", "/home/dev/projects/devlaunch"),
            _entry("devlaunch-main-zovomobo", repos / "devlaunch" / "devlaunch-main-zovomobo"),
            _entry("devlaunch-t1-vebilote", repos / "devlaunch" / "devlaunch-t1-vebilote"),
            _entry("pythontemplate", "/home/dev/projects/python_template"),
        ]
    )


def _parsed_listing(cache_dir: pathlib.Path) -> Sequence[Workspace]:
    return [Workspace.from_json(entry) for entry in json.loads(_recorded_listing(cache_dir))]


def _local(source, workspace_id: str = "ws") -> Workspace:
    return Workspace(workspace_id, LocalFolder(str(source)), "", "docker", "none")


def _record(asked: List[Workspace]):
    """Stand in for is_devlaunch_clone, recording who it was asked about."""

    def predicate(workspace: Workspace, cache_dir: pathlib.Path) -> bool:
        asked.append(workspace)
        # The name imported into this module, not dl's attribute -- that one is
        # what is being patched, and calling it here would recurse.
        return is_devlaunch_clone(workspace, cache_dir)

    return predicate


class RecordedDevpod:
    """Stands in for the devpod process, answering `list` with a recording."""

    def __init__(self, listing: str):
        self.listing = listing
        self.deleted: List[str] = []

    def __call__(self, cmd, *_args, **_kwargs) -> subprocess.CompletedProcess:
        argv = list(cmd)
        if argv[1:2] == ["list"]:
            return subprocess.CompletedProcess(argv, 0, self.listing, "")
        if argv[1:2] == ["delete"]:
            self.deleted.append(argv[2])
        return subprocess.CompletedProcess(argv, 0, "", "")


@pytest.fixture(name="cache_dir")
def fixture_cache_dir(tmp_path) -> pathlib.Path:
    """A devlaunch cache directory with something in it worth removing."""
    path = tmp_path / CACHE_LEAF
    (path / "repos").mkdir(parents=True)
    (path / "completions.json").write_text("{}")
    return path


@pytest.fixture(name="devpod")
def fixture_devpod(cache_dir) -> Iterator[RecordedDevpod]:
    """devpod answering the recorded listing, with dl pointed at *cache_dir*."""
    recorder = RecordedDevpod(_recorded_listing(cache_dir))
    with patch("devlaunch.dl.subprocess.run", side_effect=recorder):
        with patch("devlaunch.dl._get_cache_dir", return_value=cache_dir):
            with patch("devlaunch.dl.update_cache_background"):
                yield recorder


class TestWhichWorkspacesAreDevlaunchs:
    """The predicate on its own, before any deleting happens."""

    def test_a_clone_under_the_cache_directory_is_devlaunchs(self, cache_dir):
        clone = cache_dir / "repos" / "blooop" / "bencher" / "bencher-main-kivagede"
        assert is_devlaunch_clone(_local(clone), cache_dir)

    def test_a_workspace_in_the_users_own_directory_is_not(self, cache_dir):
        assert not is_devlaunch_clone(_local("/home/dev/projects/python_template"), cache_dir)

    def test_a_git_source_is_not_ours_even_when_it_names_a_path_in_the_cache(self, cache_dir):
        """devlaunch always hands devpod a local path, so nothing else is ours.

        The source here is a path *inside* the cache directory on purpose. A
        real git URL is not a path at all, so a test using one passes whether or
        not this arm is refused -- containment rejects it either way, and the
        refusal could be deleted with every test still green. Only a source that
        would otherwise be recognised puts the refusal under test.

        The shape is reachable: `devpod up <path-to-bare-repo>` records a
        `gitRepository` source, and nothing stops that repo living in the cache.
        """
        inside = cache_dir / "repos" / "blooop" / "r" / "r-main-abcdefgh"
        assert is_devlaunch_clone(_local(inside), cache_dir), "the path itself is inside"
        not_ours = Workspace("r", GitRepository(str(inside)), "", "docker", "none")
        assert not is_devlaunch_clone(not_ours, cache_dir)

    def test_a_source_devlaunch_cannot_read_is_not_ours(self, cache_dir):
        """The other half of what used to be one parametrised case.

        It has stopped being the same test. When the source was a tag beside a
        parallel string, an unreadable source could hold a path in the cache and
        the only thing standing between it and deletion was a string comparison.
        That arm now has no path on it at all -- the nearest thing expressible is
        a payload devpod sent, which no containment test can be run against -- so
        this reads as a check that the arm is *answered*, rather than as a guard
        against a value that can no longer be built.
        """
        inside = cache_dir / "repos" / "blooop" / "r" / "r-main-abcdefgh"
        not_ours = Workspace(
            "r", UnrecognisedSource({"container": str(inside)}), "", "docker", "none"
        )
        assert not is_devlaunch_clone(not_ours, cache_dir)

    def test_a_sibling_directory_that_merely_shares_a_prefix_is_not(self, cache_dir):
        """`~/.cache/devlaunch-scratch` is not inside `~/.cache/devlaunch`.

        A string prefix test says it is, and would then delete it.
        """
        sibling = cache_dir.parent / f"{cache_dir.name}-scratch" / "ws"
        assert not is_devlaunch_clone(_local(sibling), cache_dir)

    def test_the_cache_directory_itself_is_not_a_clone_devlaunch_made(self, cache_dir):
        """Clones sit under repos/<owner>/<repo>/; nothing devlaunch makes *is*
        the cache root, so a workspace opened on it stays someone else's."""
        assert not is_devlaunch_clone(_local(cache_dir), cache_dir)

    def test_a_relative_source_is_not(self, cache_dir):
        """devpod records absolute local folders. Something that is not one
        cannot be judged against an absolute cache path, so it is not ours."""
        assert not is_devlaunch_clone(_local("repos/blooop/bencher/ws"), cache_dir)


class TestTheSplitIsAValue:
    """ "Recognised" and "unrecognised" are a distinction the code states, not
    one inferred at the point of deletion from a lookup that missed."""

    def test_the_split_keeps_every_workspace(self, cache_dir):
        listed = _parsed_listing(cache_dir)
        split = workspace_ownership(listed, cache_dir)
        assert sorted(ws.id for ws in split.mine + split.foreign) == sorted(ws.id for ws in listed)

    def test_the_split_puts_each_workspace_in_the_right_arm(self, cache_dir):
        split = workspace_ownership(_parsed_listing(cache_dir), cache_dir)
        assert [ws.id for ws in split.mine] == CLONED_BY_DEVLAUNCH
        assert [ws.id for ws in split.foreign] == MADE_BY_SOMEONE_ELSE

    def test_the_predicate_is_asked_once_per_workspace(self, cache_dir):
        """One pass, so the two arms cannot come from two different answers to
        the same question -- which is the whole reason the split is a value."""
        listed = _parsed_listing(cache_dir)
        asked = []
        with patch("devlaunch.dl.is_devlaunch_clone", side_effect=_record(asked)):
            workspace_ownership(listed, cache_dir)
        assert [ws.id for ws in asked] == [ws.id for ws in listed]


class TestPurgeDeletesOnlyWhatDevlaunchMade:
    def test_it_deletes_the_clones_devlaunch_made(self, devpod):
        assert purge_all_data() == 0
        assert devpod.deleted == CLONED_BY_DEVLAUNCH

    def test_it_leaves_the_workspaces_devlaunch_never_created(self, devpod):
        """The whole ticket: `pythontemplate` is still standing afterwards."""
        assert purge_all_data() == 0
        assert "pythontemplate" not in devpod.deleted
        assert "devlaunch" not in devpod.deleted

    def test_it_still_removes_the_cache_directory(self, devpod, cache_dir):
        """The cache goes whether or not anything was left standing in devpod."""
        assert purge_all_data() == 0
        assert devpod.deleted == CLONED_BY_DEVLAUNCH
        assert not cache_dir.exists()

    def test_a_purge_with_nothing_of_its_own_deletes_nothing(self, tmp_path, capsys):
        """Only other people's workspaces, and no cache: `--purge` is a no-op."""
        theirs = json.dumps([_entry("pythontemplate", "/home/dev/projects/python_template")])
        recorder = RecordedDevpod(theirs)
        with patch("devlaunch.dl.subprocess.run", side_effect=recorder):
            with patch("devlaunch.dl._get_cache_dir", return_value=tmp_path / "never-made"):
                assert purge_all_data() == 0
        assert recorder.deleted == []
        assert "No data to purge" in capsys.readouterr().out

    def test_pointing_the_cache_elsewhere_makes_a_purge_recognise_nothing(self, tmp_path):
        """The scratch-XDG recipe in AGENTS.md now protects `--purge` for real.

        It never did before: XDG_CACHE_HOME does not scope `devpod list`, so a
        scratch run still saw -- and deleted -- every real workspace. With the
        cache pointed elsewhere, every real workspace is now unrecognised.
        """
        real_cache = tmp_path / "real" / CACHE_LEAF
        real_cache.mkdir(parents=True)
        recorder = RecordedDevpod(_recorded_listing(real_cache))
        scratch_cache = tmp_path / "scratch" / CACHE_LEAF
        with patch("devlaunch.dl.subprocess.run", side_effect=recorder):
            with patch("devlaunch.dl._get_cache_dir", return_value=scratch_cache):
                assert purge_all_data() == 0
        assert recorder.deleted == []


class TestPurgeSaysWhatItIsLeavingBehind:
    def test_the_confirmation_counts_only_what_will_be_deleted(self, devpod, capsys):
        """The number the user approves is the number that dies.

        It used to say `6 DevPod workspace(s)` and mean six, two of them someone
        else's; it now says four and means four.
        """
        assert main(["--purge", "-y"]) == 0
        out = capsys.readouterr().out
        assert "4 DevPod workspace(s)" in out
        assert "6 DevPod workspace(s)" not in out
        assert devpod.deleted == CLONED_BY_DEVLAUNCH

    def test_it_names_the_workspaces_it_will_not_touch(self, devpod, capsys):
        """Silence here is the surprise the ticket names: a user who asked for a
        clean slate has to be told what survived, and which ones."""
        assert main(["--purge", "-y"]) == 0
        out = capsys.readouterr().out
        assert "did not create" in out
        for workspace_id in MADE_BY_SOMEONE_ELSE:
            assert workspace_id in out
        # Named because they survived, not named on the way past being deleted.
        assert devpod.deleted == CLONED_BY_DEVLAUNCH

    def test_it_says_nothing_about_others_when_there_are_none(self, tmp_path, capsys):
        """No standing line about foreign workspaces on a machine that has none."""
        cache = tmp_path / CACHE_LEAF
        cache.mkdir()
        clone = cache / "repos" / "blooop" / "r" / "r-main-abcdefgh"
        recorder = RecordedDevpod(json.dumps([_entry("r-main-abcdefgh", clone)]))
        with patch("devlaunch.dl.subprocess.run", side_effect=recorder):
            with patch("devlaunch.dl._get_cache_dir", return_value=cache):
                with patch("devlaunch.dl.update_cache_background"):
                    assert main(["--purge", "-y"]) == 0
        assert "did not create" not in capsys.readouterr().out
        assert recorder.deleted == ["r-main-abcdefgh"]

    def test_a_purge_the_user_declines_deletes_nothing_and_still_reported(
        self, devpod, cache_dir, capsys
    ):
        """The report is printed before the question, not after the answer --
        which is where a user deciding whether to say yes can read it."""
        with patch("builtins.input", return_value="n"):
            assert main(["--purge"]) == 0
        assert devpod.deleted == []
        assert cache_dir.exists()
        assert "pythontemplate" in capsys.readouterr().out
