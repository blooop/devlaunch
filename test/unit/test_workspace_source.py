"""A workspace's source is one value, and it is the value that source has.

`Workspace` used to carry a `source_type` string beside a parallel `source`
string. Two of the three tags described what `source` held -- a path for
`"local"`, a URL for `"git"` -- and the third, `"unknown"`, did not: it put
`str(the raw devpod object)` into the same field. So the field's type was honest
for two values of the tag and a debug repr for the third, and nothing stopped a
`Workspace("ws", "git", "/home/dev/project", ...)` from being constructed and
believed.

The replacement is a sum type. Each arm carries only what that arm actually has,
so the tag and the value are the same fact rather than two facts that can
disagree, and the arm devlaunch cannot read carries the payload devpod sent
rather than a rendering of it.

The unreadable arm is reachable, not defensive: devpod's own workspace source
carries `image` and `container` fields alongside `localFolder` and
`gitRepository` -- read off the shipped binary's JSON struct tags -- so
`devpod up ubuntu:24.04` lands there. Every case below that uses an `image`
source is that shape, not an invented one.
"""

import json
import logging
from unittest.mock import patch

import pytest

from devlaunch.dl import (
    GitRepository,
    LocalFolder,
    UnreadableWorkspaceList,
    UnrecognisedSource,
    Workspace,
    describe_source,
    discover_repos_from_workspaces,
    fuzzy_select_workspace,
    parse_workspaces,
)


def _listed(source, workspace_id: str = "ws") -> Workspace:
    """One `devpod list --output json` element, parsed."""
    return Workspace.from_json(
        {
            "id": workspace_id,
            "source": source,
            "lastUsed": "2026-08-08T11:43:27Z",
            "provider": {"name": "docker"},
            "ide": {"name": "none"},
        }
    )


class TestEachArmCarriesOnlyWhatItHas:
    def test_a_local_folder_source_is_a_path(self):
        ws = _listed({"localFolder": "/home/dev/myproject"})
        assert ws.source == LocalFolder("/home/dev/myproject")

    def test_a_git_source_is_a_url(self):
        ws = _listed({"gitRepository": "github.com/blooop/devlaunch"})
        assert ws.source == GitRepository("github.com/blooop/devlaunch")

    def test_a_source_devlaunch_cannot_read_keeps_what_devpod_sent(self):
        """The old shape stored `str(source)` here, which is a debug repr: the
        `image` below survived only as characters inside a rendering of a dict.
        Indexing it is the assertion, because that is what a string cannot do."""
        ws = _listed({"image": "ubuntu:24.04"})
        assert isinstance(ws.source, UnrecognisedSource)
        assert ws.source.payload["image"] == "ubuntu:24.04"

    def test_a_workspace_devpod_listed_with_no_source_at_all_is_unreadable(self):
        """`{"id": "minimal"}` -- every other key absent. Previously this became
        the tag `"unknown"` beside the two-character string `"{}"`."""
        ws = Workspace.from_json({"id": "minimal"})
        assert ws.id == "minimal"
        assert ws.source == UnrecognisedSource({})

    def test_the_unreadable_arm_holds_no_path_and_no_url(self):
        """The sentinel is gone by construction, not by convention: there is no
        field on this arm for a path or a URL to be smuggled into."""
        source = _listed({"image": "ubuntu:24.04"}).source
        assert not hasattr(source, "path")
        assert not hasattr(source, "url")


class TestARepoDevlaunchCannotReadIsReportedRatherThanSkipped:
    """Repo discovery used to be an `if`/`elif` with no `else`.

    A workspace whose source it could not read fell off the end and left no
    trace -- the same outcome as a workspace it read fine and found no repo in.
    Discovery cannot invent an owner/repo for an image reference, so the fix is
    not a third answer; it is that the skip is *stated*.
    """

    def test_it_says_which_workspace_and_what_it_saw(self, caplog):
        ws = _listed({"image": "ubuntu:24.04"}, workspace_id="from-an-image")
        with caplog.at_level(logging.WARNING):
            repos = discover_repos_from_workspaces([ws])
        assert repos == {}
        assert "from-an-image" in caplog.text
        assert "image" in caplog.text

    def test_it_is_a_warning_and_not_a_debug_line_nothing_can_turn_on(self, caplog):
        """dl pins logging at INFO with no verbosity flag, so a debug line here
        would be unreachable -- which is the silence this ticket is about."""
        ws = _listed({"image": "ubuntu:24.04"}, workspace_id="from-an-image")
        with caplog.at_level(logging.WARNING):
            discover_repos_from_workspaces([ws])
        assert [r.levelno for r in caplog.records] == [logging.WARNING]

    def test_the_readable_workspaces_beside_it_are_still_discovered(self):
        """Reporting the one it cannot read does not cost the ones it can."""
        workspaces = [
            _listed({"image": "ubuntu:24.04"}, workspace_id="from-an-image"),
            _listed({"gitRepository": "github.com/blooop/devlaunch"}, workspace_id="dl"),
        ]
        assert discover_repos_from_workspaces(workspaces) == {"blooop": ["devlaunch"]}

    def test_a_git_source_with_no_owner_repo_in_it_stays_quiet(self, caplog):
        """Not every skip is a defect. Discovery's job is finding GitHub
        owner/repo pairs; a source it read perfectly well and found none in is
        an ordinary answer, and warning about it would train users past the
        warning that matters."""
        ws = _listed({"gitRepository": "https://gitlab.com/group/sub/proj.git"})
        with caplog.at_level(logging.WARNING):
            assert discover_repos_from_workspaces([ws]) == {}
        assert caplog.records == []


class TestTheListingDescribesEverySource:
    """`dl --ls` and the fzf picker are the two call sites that only ever wanted
    the tag. They ask one function for both columns, so the kind shown and the
    detail shown cannot come from two different readings of the same source."""

    @pytest.mark.parametrize(
        "source, kind, detail",
        [
            ({"localFolder": "/home/dev/myproject"}, "local", "/home/dev/myproject"),
            (
                {"gitRepository": "github.com/blooop/devlaunch"},
                "git",
                "github.com/blooop/devlaunch",
            ),
            ({"image": "ubuntu:24.04"}, "unknown", json.dumps({"image": "ubuntu:24.04"})),
            ({}, "unknown", "{}"),
        ],
    )
    def test_it_names_the_kind_and_shows_the_detail(self, source, kind, detail):
        assert describe_source(_listed(source).source) == (kind, detail)

    def test_every_arm_gets_a_non_empty_kind_and_detail(self):
        """A column that renders empty for one arm is the listing's own version
        of dropping it: the row is there and says nothing."""
        for source in ({"localFolder": "/p"}, {"gitRepository": "u"}, {"image": "i"}, {}):
            kind, detail = describe_source(_listed(source).source)
            assert kind and detail


class TestTheFuzzyPickerOffersEverySource:
    """The other of the two display call sites, and the one with no tests at all
    before this change -- so the arm the ticket is about could have been dropped
    from the picker too and nothing would have said."""

    def test_a_workspace_devlaunch_cannot_read_is_still_offered_and_selectable(self):
        workspaces = [
            _listed({"localFolder": "/home/dev/myproject"}, workspace_id="mine"),
            _listed({"image": "ubuntu:24.04"}, workspace_id="from-an-image"),
        ]
        offered = []

        def fake_iterfzf(options, multi=False):  # noqa: ARG001 - matches iterfzf's signature
            offered.extend(options)
            return offered[1]

        with patch("devlaunch.dl.list_workspaces", return_value=workspaces):
            with patch("iterfzf.iterfzf", fake_iterfzf):
                selected = fuzzy_select_workspace()

        assert offered == [
            "mine | local | /home/dev/myproject",
            'from-an-image | unknown | {"image": "ubuntu:24.04"}',
        ]
        # Picking the row maps back to the workspace, which is what makes it an
        # offer rather than a line of text.
        assert selected == "from-an-image"


class TestASourceThatIsNotAnObjectAtAllIsAnUnreadableListing:
    """The arms are total over the source *object* devpod documents.

    Something that is not an object is not a source devlaunch cannot read; it
    is a listing devlaunch cannot read, which the listing parser already has an
    answer for. Refusing it there keeps the unreadable arm honest -- it holds
    what devpod sent, and it can only do that if what devpod sent was an object.
    """

    @pytest.mark.parametrize("source", ["/home/dev/project", 7, ["localFolder"]])
    def test_the_listing_is_refused_rather_than_guessed_at(self, source):
        listing = json.dumps([{"id": "odd", "source": source}])
        with pytest.raises(UnreadableWorkspaceList) as raised:
            parse_workspaces(listing)
        assert "odd" in str(raised.value)

    def test_a_string_source_is_not_read_as_a_local_folder_by_accident(self):
        """`"localFolder" in some_string` is a substring test, so a string
        source mentioning the key at all used to be one indexing error away
        from being taken for a folder."""
        listing = json.dumps([{"id": "odd", "source": "/srv/localFolder/x"}])
        with pytest.raises(UnreadableWorkspaceList):
            parse_workspaces(listing)
