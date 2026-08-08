"""The e2e suite's workspace-creation step, tested without a daemon.

`test_purge_deletes_workspaces` used to call `devpod up ... --id e2e-test-purge`
without `--ide none`, unlike every sibling. devpod created the workspace, then
tried to open an editor, could not find `xdg-open`, and exited 1. The test read
that non-zero rc as "could not create test workspace" and skipped -- on any
headless machine, container or not -- while the container it had just
successfully made stayed behind, tracked by nothing.

Two failures compounding, and the second is the worse one: a create that failed
after creating something is not a create that did nothing, and a step the suite
depends on is not something to quietly opt out of. Both are properties of the
creation step itself, so they are checked here against a fake process boundary
rather than by running the e2e suite.
"""

import subprocess

import pytest

from fixtures.e2e_guard import LEDGER
from fixtures.e2e_helpers import create_e2e_workspace

SOURCE = "/tmp/some-local-repo"
WORKSPACE_ID = "e2e-test-example"


class RecordingTracker:
    """Stand-in for the devpod_cleanup fixture."""

    def __init__(self):
        self.workspaces: list[str] = []

    def track(self, workspace_id: str) -> None:
        self.workspaces.append(workspace_id)


class RecordingDevpod:
    """Stand-in for the devpod process. Records what the tracker knew at the
    moment it was asked to create a workspace."""

    def __init__(self, tracker, rc: int = 0):
        self.tracker = tracker
        self.rc = rc
        self.commands: list[list[str]] = []
        self.tracked_at_call_time: list[str] = []

    def __call__(self, cmd, **kwargs):  # noqa: ARG002
        self.commands.append(list(cmd))
        self.tracked_at_call_time = list(self.tracker.workspaces)
        return subprocess.CompletedProcess(cmd, self.rc, stdout="", stderr="no xdg-open")


def test_workspace_is_created_without_an_ide():
    tracker = RecordingTracker()
    devpod = RecordingDevpod(tracker)

    create_e2e_workspace(SOURCE, WORKSPACE_ID, cleanup=tracker, run=devpod)

    cmd = devpod.commands[0]
    assert cmd[:2] == ["devpod", "up"]
    assert cmd[cmd.index("--ide") + 1] == "none"


def test_workspace_is_registered_for_cleanup_before_it_is_created():
    """Registering afterwards is registering only on the happy path -- a create
    that dies partway through has still made something."""
    tracker = RecordingTracker()
    devpod = RecordingDevpod(tracker)

    create_e2e_workspace(SOURCE, WORKSPACE_ID, cleanup=tracker, run=devpod)

    assert devpod.tracked_at_call_time == [WORKSPACE_ID]


def test_failed_creation_fails_the_test_rather_than_skipping_it():
    tracker = RecordingTracker()
    devpod = RecordingDevpod(tracker, rc=1)

    with pytest.raises(pytest.fail.Exception) as raised:
        create_e2e_workspace(SOURCE, WORKSPACE_ID, cleanup=tracker, run=devpod)

    assert not isinstance(raised.value, pytest.skip.Exception)
    assert WORKSPACE_ID in str(raised.value)


def test_failed_creation_leaves_the_workspace_registered_for_cleanup():
    """The leak, stated as a property: whatever devpod did or did not manage to
    build, someone is still going to delete it."""
    tracker = RecordingTracker()
    devpod = RecordingDevpod(tracker, rc=1)

    with pytest.raises(pytest.fail.Exception):
        create_e2e_workspace(SOURCE, WORKSPACE_ID, cleanup=tracker, run=devpod)

    assert tracker.workspaces == [WORKSPACE_ID]


def test_a_stubbed_creation_does_not_credit_the_session_ledger():
    """The one way the ledger could be made to lie, closed.

    The ledger's whole claim is that a workspace in it is a container that
    exists, which is why it is written at the only place that builds one. But
    these very tests call that place with a stub -- a `devpod up` that returns
    rc 0 having done nothing -- and a session that ran both kinds of test at
    once (`pytest -m ""`) would find three phantom workspaces credited and its
    floor cleared by them.
    """
    tracker = RecordingTracker()
    devpod = RecordingDevpod(tracker)
    before = list(LEDGER.workspaces_created)

    create_e2e_workspace(SOURCE, WORKSPACE_ID, cleanup=tracker, run=devpod)

    assert LEDGER.workspaces_created == before


def test_creation_reports_the_source_and_id_it_was_given():
    tracker = RecordingTracker()
    devpod = RecordingDevpod(tracker)

    create_e2e_workspace(SOURCE, WORKSPACE_ID, cleanup=tracker, run=devpod)

    cmd = devpod.commands[0]
    assert SOURCE in cmd
    assert cmd[cmd.index("--id") + 1] == WORKSPACE_ID
