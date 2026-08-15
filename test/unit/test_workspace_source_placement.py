# pylint: disable=redefined-outer-name
"""Where a workspace's source sits on this disk cannot depend on where `dl` ran.

devlaunch#224, found while closing devlaunch#88's manual step: `dl --reconcile`
run from inside `<root>/<owner>/<repo>/` listed every git-URL-sourced devpod
workspace on the machine as an orphan of *that* repository, at paths like
`<root>/blooop/devlaunch/git@github.com:blooop/wayfinder.git`. The same command
from a neutral directory listed none of them.

A remote URL is relative-looking text, so resolving it as a path resolves it
against the current directory -- the same hazard the empty-string guard on
`localFolder` names, arriving through the other arm: a workspace credited with
whatever repository the person running `dl` happened to be standing in. So the
property under test is not "a URL is skipped" but the stronger one the report
has to have: the same workspaces, classified from two directories, are one
answer.

**The arm still counts, and that is half of what is pinned here.** `devpod up
<path-to-a-repo>` records the `gitRepository` arm with a local path in it, and a
path the placement pass does not return is a directory `--prune` will call
unreferenced -- loss, where a misread URL is only a misreport. So the cases
below that keep a path are as load-bearing as the ones that drop a URL, and a
fix that simply stopped reading the arm would fail them.
"""

import pytest

from devlaunch.dl import GitRepository, Workspace, workspace_locations

OWNER, REPO = "blooop", "devlaunch"

#: Every spelling devpod can record a remote in. Each names a repository on
#: another machine and no directory on this one.
REMOTE_URLS = [
    "git@github.com:blooop/wayfinder.git",
    "https://github.com/blooop/wayfinder.git",
    "ssh://git@github.com/blooop/wayfinder.git",
]


def _sourced(source, workspace_id: str = "w1") -> Workspace:
    """One live devpod workspace, opening *source*."""
    return Workspace(workspace_id, source, "2026-08-08T11:43:27Z", "docker", "none")


@pytest.fixture
def root(tmp_path):
    """A clone tree holding one repository, and that repository holding no clone.

    The repository directory has to exist for a cwd to be inside it, which is
    the whole reproduction: the misclassification needs somewhere to stand.
    """
    repos = tmp_path / "repos"
    (repos / OWNER / REPO).mkdir(parents=True)
    return repos


class TestTheAnswerIsTheSameFromEveryDirectory:
    """Same workspaces, two directories, one classification."""

    @pytest.mark.parametrize("url", REMOTE_URLS)
    def test_a_remote_url_places_the_same_inside_a_repositorys_tree_as_outside_it(
        self, root, url, tmp_path, monkeypatch
    ):
        workspaces = [_sourced(GitRepository(url))]

        monkeypatch.chdir(tmp_path)
        outside = workspace_locations(workspaces, root)
        monkeypatch.chdir(root / OWNER / REPO)
        inside = workspace_locations(workspaces, root)

        assert inside == outside

    @pytest.mark.parametrize("url", REMOTE_URLS)
    def test_a_remote_url_is_no_repositorys_orphan_and_holds_no_clone(self, root, url, monkeypatch):
        """The three ways a workspace can appear in this answer, all empty.

        Equality with the neutral run above would also be satisfied by both runs
        being wrong in the same way, so what the right answer *is* gets said
        here: a repository on another machine disputes nothing here, holds
        nothing here, and is not a source dl failed to follow either.
        """
        monkeypatch.chdir(root / OWNER / REPO)

        located = workspace_locations([_sourced(GitRepository(url))], root)

        assert located.by_path == {}
        assert located.misplaced == {}
        assert located.unlocatable == ()


class TestAGitSourceCarryingALocalPathStillCounts:
    """`devpod up <path-to-a-repo>` records this arm, and prune reads it."""

    def test_a_clone_it_names_is_held_by_that_workspace(self, root, tmp_path, monkeypatch):
        clone = root / OWNER / REPO / "blooop-devlaunch-main"
        clone.mkdir()
        (clone / ".git").mkdir()
        monkeypatch.chdir(tmp_path)

        located = workspace_locations([_sourced(GitRepository(str(clone)))], root)

        assert located.holder(clone) == "w1"

    def test_a_gone_directory_it_names_still_disputes_that_repositorys_clones(
        self, root, tmp_path, monkeypatch
    ):
        """The `Misplaced` arm, reached through this source arm.

        A path under `<root>/<owner>/<repo>/` with no `.git` in it is a
        workspace that could be opening any of that repository's clones, and
        that stays true whether devpod recorded it as a folder or as a repo.
        """
        gone = root / OWNER / REPO / "main"
        monkeypatch.chdir(tmp_path)

        located = workspace_locations([_sourced(GitRepository(str(gone)))], root)

        assert (OWNER, REPO) in located.misplaced
