"""Integration tests for the git-lfs pointer check against real git repositories.

Whether a workspace still holds unmaterialized LFS pointers is a question about
what git and git-lfs actually see, and the ways a repository can end up holding
pointers are not reproducible with a fake git. These tests therefore build real
repositories with real git and let the real thing answer.

Only the git-lfs executable is stubbed, and only where it is forked: what
`git lfs ls-files` reports for each of these repositories was verified against
git-lfs 3.7.1, and is recorded here so the suite does not require git-lfs to be
installed. Notably it reports a committed pointer file even when the repository
declares no `filter=lfs` attribute anywhere — `git check-attr` says
`filter: unspecified` for that same file — so "this repo declares LFS" is not a
usable stand-in for "this repo holds pointers".
"""

import subprocess
from unittest.mock import patch

import pytest

from devlaunch.worktree.workspace_clone import WorkspaceCloneManager

# A syntactically real pointer file, as `git lfs track` + commit would leave it
# in a clone made with GIT_LFS_SKIP_SMUDGE=1.
POINTER = b"version https://git-lfs.github.com/spec/v1\noid sha256:" + b"0" * 64 + b"\nsize 12\n"

# What materialization would leave behind: the twelve bytes POINTER declares.
REAL_CONTENT = b"real content"
assert len(REAL_CONTENT) == 12, "must match the size the pointer declares"

LFS_ATTRIBUTE_LINE = "*.bin filter=lfs diff=lfs merge=lfs -text\n"


def git(repo, *args):
    """Run a real git command in *repo*, failing loudly."""
    subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True)


def make_repo(path):
    """Create a real git repository these tests can commit to.

    Signing is switched off explicitly rather than left to whatever the machine
    running the suite has in its global config: with `commit.gpgsign = true` set
    there, every commit below fails for want of a key, and the suite reports a
    dozen failures about git-lfs that have nothing to do with git-lfs. Other
    integration tests in this repo still inherit that config; this one does not
    need to.
    """
    path.mkdir(parents=True, exist_ok=True)
    git(path, "init", "-q")
    git(path, "config", "user.email", "test@example.com")
    git(path, "config", "user.name", "Test")
    git(path, "config", "commit.gpgsign", "false")
    return path


def commit_pointer(path, name="big.bin"):
    """Commit *name* as an unmaterialized LFS pointer."""
    (path / name).write_bytes(POINTER)
    git(path, "add", "-A")
    git(path, "commit", "-qm", "add pointer")


def check_pointers(ws_path, lfs_reports=()):
    """Answer _has_lfs_pointers for a real repo, stubbing only the git-lfs fork.

    Returns (answer, commands_issued) so a test can pin both the answer and
    whether the git-lfs fork this gate exists to avoid was paid.
    """
    issued = []
    real_run = subprocess.run

    def spy(cmd, *args, **kwargs):
        issued.append(list(cmd))
        if list(cmd[:2]) == ["git", "lfs"]:
            return subprocess.CompletedProcess(cmd, 0, "".join(f"{n}\n" for n in lfs_reports), "")
        # Forwarded verbatim, `check` included: this stands in for subprocess.run
        # itself, so it must not impose a policy of its own.
        return real_run(cmd, *args, **kwargs)  # pylint: disable=subprocess-run-check

    with (
        patch("devlaunch.worktree.workspace_clone.shutil.which", return_value="/usr/bin/git-lfs"),
        patch("devlaunch.worktree.workspace_clone.subprocess.run", side_effect=spy),
    ):
        answer = WorkspaceCloneManager._has_lfs_pointers(ws_path)  # pylint: disable=protected-access
    return answer, issued


def forked_git_lfs(issued):
    """True if any issued command forked git-lfs."""
    return any(cmd[:2] == ["git", "lfs"] for cmd in issued)


@pytest.mark.integration
class TestPointerDetectionAgainstRealRepos:
    """A workspace holding pointers must be recognised however it got them."""

    def test_committed_pointer_without_gitattributes_is_detected(self, tmp_path):
        """A pointer committed with no gitattributes anywhere still needs pulling.

        Nothing stops a pointer file being committed into a repository that
        declares no LFS filter — a deleted .gitattributes, or a file added by a
        tool. git-lfs lists such a file; git's own `check-attr` does not call it
        LFS-filtered. A workspace holding one must still be materialized, or it
        is shipped to the user as a stub.
        """
        ws = make_repo(tmp_path / "ws")
        commit_pointer(ws)

        answer, issued = check_pointers(ws, lfs_reports=["big.bin"])

        assert answer is True
        assert forked_git_lfs(issued)

    def test_pointer_declared_by_out_of_clone_attributes_is_detected(self, tmp_path):
        """LFS declared through core.attributesFile still materializes.

        git honours attributes from outside the working tree — core.attributesFile
        here, /etc/gitattributes by the same mechanism — so a repository can be
        LFS-tracked in git's view with no gitattributes file of its own.
        """
        attributes = tmp_path / "global_gitattributes"
        attributes.write_text(LFS_ATTRIBUTE_LINE)
        ws = make_repo(tmp_path / "ws")
        commit_pointer(ws)
        git(ws, "config", "core.attributesFile", str(attributes))

        answer, issued = check_pointers(ws, lfs_reports=["big.bin"])

        assert answer is True
        assert forked_git_lfs(issued)

    def test_pointer_declared_by_local_info_attributes_is_detected(self, tmp_path):
        """LFS declared only in .git/info/attributes still materializes.

        This declaration is local and untracked, so it never appears in the
        index or the working tree.
        """
        ws = make_repo(tmp_path / "ws")
        commit_pointer(ws)
        info = ws / ".git" / "info"
        info.mkdir(parents=True, exist_ok=True)
        (info / "attributes").write_text(LFS_ATTRIBUTE_LINE)

        answer, issued = check_pointers(ws, lfs_reports=["big.bin"])

        assert answer is True
        assert forked_git_lfs(issued)

    def test_pointer_in_a_subdirectory_is_detected(self, tmp_path):
        """Pointers are found at any depth, not only at the top level."""
        ws = make_repo(tmp_path / "ws")
        (ws / "assets").mkdir()
        (ws / ".gitattributes").write_text(LFS_ATTRIBUTE_LINE)
        commit_pointer(ws, name="assets/big.bin")

        answer, issued = check_pointers(ws, lfs_reports=["assets/big.bin"])

        assert answer is True
        assert forked_git_lfs(issued)

    def test_tracked_paths_that_will_not_open_do_not_stop_the_scan(self, tmp_path):
        """Unreadable tracked paths are skipped, not fatal, and not the end.

        Deciding from the working tree means looking at every tracked path
        rather than the handful git-lfs names, and ordinary workspaces are full
        of tracked paths that will not open: a file the user deleted, a symlink
        whose target is gone, a submodule's directory. None of them is a
        pointer. Treating any of them as an error would break the launch of a
        perfectly normal workspace, and giving up at the first one would strand
        a real pointer sitting behind it — so the pointer here is named to sort
        last, behind all three.
        """
        ws = make_repo(tmp_path / "ws")
        (ws / "gone.txt").write_text("deleted from the working tree later\n")
        (ws / "dangling").symlink_to("no-such-target")
        submodule = make_repo(ws / "nested")
        (submodule / "f").write_text("x\n")
        git(submodule, "add", "-A")
        git(submodule, "commit", "-qm", "nested")
        commit_pointer(ws, name="zz_big.bin")
        (ws / "gone.txt").unlink()

        answer, issued = check_pointers(ws, lfs_reports=["zz_big.bin"])

        assert answer is True
        assert forked_git_lfs(issued)

    def test_pointer_is_detected_when_the_clone_has_no_index(self, tmp_path):
        """A clone left with no `.git/index` still gets its pointers materialized.

        An interrupted clone or checkout can leave the index missing entirely,
        and git answers `ls-files` for such a clone with *success and no output*
        — not with an error. A gate that asked only the index would read that as
        "nothing is tracked, so nothing can be a pointer" and skip, while
        git-lfs, which reads HEAD as well, still names the pointer. Nothing
        about that heals on its own: the materialization retry exists for
        exactly the interrupted operations that produce this state, so the skip
        would repeat on every later launch.
        """
        ws = make_repo(tmp_path / "ws")
        (ws / ".gitattributes").write_text(LFS_ATTRIBUTE_LINE)
        commit_pointer(ws)
        (ws / ".git" / "index").unlink()

        answer, issued = check_pointers(ws, lfs_reports=["big.bin"])

        assert answer is True
        assert forked_git_lfs(issued)

    def test_pointer_only_in_head_is_detected(self, tmp_path):
        """A pointer git-lfs names from HEAD counts even when the index drops it.

        `git lfs ls-files` reports the union of HEAD's tree and the index, so
        un-staging a tracked path leaves it named by git-lfs and absent from the
        index. The gate has to ask the same union, or it answers "no pointers"
        about a path the probe would have named.
        """
        ws = make_repo(tmp_path / "ws")
        (ws / ".gitattributes").write_text(LFS_ATTRIBUTE_LINE)
        commit_pointer(ws)
        git(ws, "rm", "-q", "--cached", "big.bin")

        answer, issued = check_pointers(ws, lfs_reports=["big.bin"])

        assert answer is True
        assert forked_git_lfs(issued)

    def test_unmaterialized_pointer_is_detected_on_every_launch(self, tmp_path):
        """A workspace left on pointers is retried, not written off.

        A failed `git lfs pull` leaves an existing workspace holding pointers.
        Deciding once that a repository needs no materialization would make that
        state permanent — every later launch would build against stubs.
        """
        ws = make_repo(tmp_path / "ws")
        commit_pointer(ws)

        first, _ = check_pointers(ws, lfs_reports=["big.bin"])
        second, _ = check_pointers(ws, lfs_reports=["big.bin"])

        assert first is True
        assert second is True

    def test_materialized_workspace_needs_no_further_pull(self, tmp_path):
        """Once the real content is on disk the workspace is done.

        The counterpart to the retry above: retrying must stop when it has
        worked, or every launch would re-pull.
        """
        ws = make_repo(tmp_path / "ws")
        (ws / ".gitattributes").write_text(LFS_ATTRIBUTE_LINE)
        commit_pointer(ws)
        (ws / "big.bin").write_bytes(REAL_CONTENT)

        answer, _ = check_pointers(ws, lfs_reports=["big.bin"])

        assert answer is False


@pytest.mark.integration
class TestProbeCostAgainstRealRepos:
    """The git-lfs fork is what this gate exists to avoid."""

    def test_ordinary_repo_never_forks_git_lfs(self, tmp_path):
        """A repository holding no pointer files must not pay a git-lfs fork.

        This is the common case by an enormous margin, and it pays the fork on
        every launch for an answer already visible in the working tree.
        """
        ws = make_repo(tmp_path / "ws")
        (ws / "main.py").write_text("print('hi')\n")
        (ws / "docs").mkdir()
        (ws / "docs" / "readme.md").write_text("# hi\n")
        git(ws, "add", "-A")
        git(ws, "commit", "-qm", "init")

        answer, issued = check_pointers(ws)

        assert answer is False
        assert not forked_git_lfs(issued)

    def test_materialized_lfs_repo_never_forks_git_lfs(self, tmp_path):
        """A fully materialized LFS repository has nothing left to ask git-lfs.

        Declaring LFS is not the question; holding an unmaterialized pointer is.
        A warm workspace whose LFS content is already on disk gets the same free
        answer as a repository that never used LFS.
        """
        ws = make_repo(tmp_path / "ws")
        (ws / ".gitattributes").write_text(LFS_ATTRIBUTE_LINE)
        commit_pointer(ws)
        (ws / "big.bin").write_bytes(REAL_CONTENT)

        answer, issued = check_pointers(ws, lfs_reports=["big.bin"])

        assert answer is False
        assert not forked_git_lfs(issued)

    def test_tracked_paths_that_will_not_open_are_not_read_as_pointers(self, tmp_path):
        """A path that cannot be opened is not evidence of a pointer.

        The counterpart to the scan-does-not-stop test above: every ordinary
        workspace has tracked paths that will not open — a file the user
        deleted, a dangling symlink, a submodule's directory — and none of them
        says anything about LFS. Reading "cannot open it" as "assume pointer"
        would reinstate the git-lfs fork on every launch of every such
        workspace, which is the entire cost this gate removes, and would do it
        with the answer unchanged.
        """
        ws = make_repo(tmp_path / "ws")
        (ws / "gone.txt").write_text("deleted from the working tree later\n")
        (ws / "dangling").symlink_to("no-such-target")
        submodule = make_repo(ws / "nested")
        (submodule / "f").write_text("x\n")
        git(submodule, "add", "-A")
        git(submodule, "commit", "-qm", "nested")
        git(ws, "add", "-A")
        git(ws, "commit", "-qm", "init")
        (ws / "gone.txt").unlink()

        answer, issued = check_pointers(ws)

        assert answer is False
        assert not forked_git_lfs(issued)

    def test_unreadable_index_still_probes(self, tmp_path):
        """When the tracked files cannot be listed, the probe runs anyway.

        The cheap check exists to save a fork, not to decide LFS is absent.
        Reading "cannot tell" as "no LFS here" would strand a workspace on
        pointer files — exactly the silent degradation the probe refuses.
        """
        ws = tmp_path / "not-a-repo"
        ws.mkdir()
        (ws / "big.bin").write_bytes(POINTER)

        answer, issued = check_pointers(ws, lfs_reports=["big.bin"])

        assert answer is True
        assert forked_git_lfs(issued)
