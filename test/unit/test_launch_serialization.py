# pylint: disable=redefined-outer-name,protected-access
"""Two `up`s of one workspace, and what the second one does about the first.

wayfinder fires `dl <ws> up` in the background the moment a launch is staged,
then runs the real launch seconds later when the human hits enter again. That
makes concurrent `up`s of a single workspace an everyday event rather than an
edge case, so dl serializes them on a per-workspace lock — and the launch that
had to wait re-checks the state rather than re-walking a container lifecycle
the sibling just finished.
"""

import subprocess
from contextlib import contextmanager
from unittest.mock import patch

import pytest

from devlaunch import dl


@contextmanager
def _lock(contended: bool):
    """Stand in for hold_lock, reporting whatever contention a test wants."""
    yield contended


@pytest.fixture
def devpod():
    """Record devpod spawns; nothing here needs a real one."""
    calls = []

    def run(args, **_kwargs):
        calls.append(list(args))
        # `context options` is read on the way through and must parse.
        stdout = "{}" if args[:2] == ["context", "options"] else ""
        return subprocess.CompletedProcess(args=list(args), returncode=0, stdout=stdout)

    with patch.object(dl, "run_devpod", side_effect=run):
        with patch.object(dl, "invalidate_workspace_list_cache"):
            with patch.object(dl.tools, "provision_tools") as provision:
                yield calls, provision


def _up(devpod, *, contended: bool, state: str, **kwargs):
    """Run workspace_up under a lock with the given contention and state."""
    calls, provision = devpod
    with patch.object(dl, "hold_lock", lambda *_a, **_k: _lock(contended)):
        with patch.object(dl, "get_workspace_state", return_value=state):
            result = dl.workspace_up(
                "owner/repo", workspace_id="myws", workspace_identity="myws", **kwargs
            )
    return result, [c for c in calls if c[:1] == ["up"]], provision


class TestAContendedUp:
    """The launch that waited: the sibling it waited on may have done the job."""

    def test_a_running_workspace_is_not_brought_up_again(self, devpod):
        """The prewarm won the race, so the launch has nothing left to do but
        succeed — `devpod up` here would re-walk a whole container lifecycle
        to arrive where the workspace already is."""
        result, ups, _provision = _up(devpod, contended=True, state="Running")
        assert result.returncode == 0
        assert ups == []

    def test_the_skipped_up_still_makes_sure_the_tools_are_there(self, devpod):
        """ "Running" says the sibling's `devpod up` returned, not that its
        install did.

        The sibling can be interrupted between the two (the flock dies with
        the process), its `up` can fail after the container has started, or it
        can have run with DEVLAUNCH_NO_TOOLS set where this one did not. Each
        leaves a running workspace with no tools, and trusting the state would
        hand the user a session without them. The probe is one round trip and
        silent when there is nothing to do.
        """
        _result, ups, provision = _up(devpod, contended=True, state="Running")
        assert ups == []
        provision.assert_called_once()
        assert provision.call_args.args[0] == "myws"

    def test_a_stopped_workspace_is_still_brought_up(self, devpod):
        """Waiting is not evidence the sibling succeeded: a prewarm that
        failed, or that only got as far as creating a stopped workspace,
        leaves the launch exactly the work it came to do."""
        _result, ups, provision = _up(devpod, contended=True, state="Stopped")
        assert len(ups) == 1
        assert provision.call_count == 1

    @pytest.mark.parametrize(
        "kwargs",
        [
            {"ide": "vscode"},
            {"recreate": True},
            {"reset": True},
            {"devcontainer": ".devcontainer/robot/devcontainer.json"},
        ],
    )
    def test_a_side_effect_the_sibling_cannot_have_had_is_never_skipped(self, devpod, kwargs):
        """An IDE to open, a container to rebuild from scratch, a different
        devcontainer variant: a running workspace is not the answer to any of
        these, so the skip does not apply however contended the lock was.

        The variant is the one that would be silent about it. A prewarm brings
        up the default container; a human then asks for `--devcontainer robot`
        and waits on the lock. Skipping there would attach them to the default
        and never say so.
        """
        _result, ups, _provision = _up(devpod, contended=True, state="Running", **kwargs)
        assert len(ups) == 1


class TestALockThatCannotBeTaken:
    """An unwritable cache must not cost the user a workspace."""

    def test_the_up_still_happens(self, devpod):
        """A container writing as another uid is a documented occurrence in
        this cache, and a full or read-only disk lands here too. Serialization
        guards a race that may not be happening; an errno traceback in front
        of a `devpod up` that would have worked is the worse answer."""
        calls, provision = devpod
        with patch.object(dl, "hold_lock", side_effect=PermissionError(13, "Permission denied")):
            result = dl.workspace_up("owner/repo", workspace_id="myws", workspace_identity="myws")
        assert result.returncode == 0
        assert [c for c in calls if c[:1] == ["up"]]
        assert provision.call_count == 1


class TestAnUncontendedUp:
    """The everyday case pays the flock and nothing else."""

    def test_it_never_asks_for_the_state(self, devpod):
        """No sibling ran, so nothing can have changed under this process —
        the re-check would be a round trip bought with no question to answer."""
        calls, _provision = devpod
        with patch.object(dl, "hold_lock", lambda *_a, **_k: _lock(False)):
            with patch.object(dl, "get_workspace_state") as state:
                dl.workspace_up("owner/repo", workspace_id="myws", workspace_identity="myws")
        assert state.call_count == 0
        assert [c for c in calls if c[:1] == ["up"]]


class TestTheLockItself:
    """What the lock is keyed on, and when it is taken at all."""

    def test_it_is_keyed_by_workspace_not_by_repo(self):
        """Two nodes of one repo launch at once by design (one branch, one
        container each) — a repo-keyed lock would serialize them for nothing."""
        one = dl._launch_lock_path("wayfinder-16-abc")
        two = dl._launch_lock_path("wayfinder-17-def")
        assert one != two
        assert one.parent == two.parent

    def test_it_lives_outside_the_repo_cache(self):
        """The cache's walkers read every directory under repos/ as a repo,
        and these locks are also taken for workspaces that have no clone
        there at all (paths, URLs)."""
        path = dl._launch_lock_path("myws")
        assert "repos" not in path.parts
        assert path.is_relative_to(dl._get_cache_dir())

    def test_an_up_with_no_identity_takes_no_lock(self, devpod):
        """Nothing to key it on. The caller shapes that reach here are not the
        concurrent-launch ones."""
        calls, _provision = devpod
        with patch.object(dl, "hold_lock", side_effect=AssertionError("locked anyway")):
            dl.workspace_up("owner/repo")
        assert [c for c in calls if c[:1] == ["up"]]
