"""E2E: `dl <ws> -- <command>` must leave a live interactive session behind.

These are the tests that would have caught the bug. Everything else about the
pty transport can be checked by inspecting argv, but "the process is still
there a moment later, and still listening" only means anything against a real
container -- the failure was that devpod's --command transport starts the
process with no terminal, and the process notices and leaves.

The payload is a shell loop rather than `claude`: same shape (announce, then
block on input forever), no install, no network, no model. A test that depended
on `claude` would be testing Anthropic's uptime.

These require a real devpod and are excluded from the default run. To run them:

    pixi run pytest -m e2e test/e2e/test_interactive_session.py
"""

# Requesting a fixture shadows its name; that is how pytest is written.
# pylint: disable=redefined-outer-name

import os
import re
import shlex
import subprocess

import pytest

from devlaunch import tty_session
from fixtures.pty_helpers import PtySession

# Announce readiness, then block on stdin with no exit of its own.
LONG_RUNNING = (
    "echo READY-$$; "
    "while IFS= read -r line; do "
    '[ "$line" = quit ] && exit 0; '
    'echo "ECHO:$line"; '
    "done"
)

# The reported bug in one payload: refuse to start without a terminal, and
# otherwise never exit. A shell loop alone does not discriminate between the two
# transports -- `while read` is perfectly happy on a pipe -- so it takes the tty
# check in front of it to reproduce what `claude` does, without needing claude.
NEEDS_A_TTY_AND_STAYS = (
    "if [ -t 0 ]; then S=HAVE; else S=MISSING; fi; "
    'echo "TTY-STATUS:$S"; '
    '[ "$S" = HAVE ] || exit 42; ' + LONG_RUNNING
)

# The check the old transport failed: report whether stdin is a terminal, the
# way an interactive agent decides whether to start at all.
#
# dl logs the command it is about to run, so the payload itself lands in the pty
# buffer before any output does. Every marker here is therefore assembled at
# runtime: `TTY-STATUS:HAVE` cannot appear in the echoed command, so matching it
# can only mean the command actually ran and printed it.
NEEDS_A_TTY = 'if [ -t 0 ]; then S=HAVE; else S=MISSING; fi; echo "TTY-STATUS:$S"; tty'

# Same trick: the literal `SHELL-PATH:` exists only in the output.
LOGIN_SHELL_PROBE = 'P=PATH; echo "SHELL-${P}:$PATH"'


def devpod_available() -> bool:
    try:
        return (
            subprocess.run(["devpod", "version"], capture_output=True, check=False).returncode == 0
        )
    except FileNotFoundError:
        return False


@pytest.fixture(scope="module")
def running_workspace() -> str:
    """A started workspace to attach to, reused by every test in the module.

    Creating one costs an image pull and a container build, so it is worth
    doing once. The id is read back from devpod rather than assumed.
    """
    if not devpod_available():
        pytest.skip("devpod not available")

    workspace_id = os.environ.get("DEVLAUNCH_E2E_WORKSPACE")
    if not workspace_id:
        pytest.skip("set DEVLAUNCH_E2E_WORKSPACE to a started devpod workspace to run these tests")

    state = subprocess.run(
        ["devpod", "status", workspace_id, "--output", "json"],
        capture_output=True,
        text=True,
        check=False,
    )
    if state.returncode != 0:
        pytest.skip(f"workspace {workspace_id} is not usable: {state.stderr.strip()}")
    return workspace_id


def dl(workspace_id: str, *args: str) -> PtySession:
    """`dl` on a pty, the way a developer runs it."""
    return PtySession(["python", "-m", "devlaunch.dl", workspace_id, *args], timeout=120)


def reported_cwd(output: str) -> str:
    """Pull the directory the probe printed, failing loudly if it printed none."""
    match = re.search(r"IN-PWD:(\S+)", output)
    assert match, f"no working directory in output:\n{output}"
    return match.group(1)


@pytest.mark.e2e
class TestCommandGetsATerminal:
    """The half the user reported working -- kept working, now with a terminal."""

    def test_host_alias_exists_for_the_workspace(self, running_workspace):
        """The pty transport reads devpod's own alias; without it dl falls back."""
        assert tty_session.devpod_host_configured(running_workspace), (
            "devpod wrote no ssh host alias for this workspace, so dl will fall "
            "back to the transport that has no terminal"
        )

    def test_one_shot_command_still_runs_and_reports_its_output(self, running_workspace):
        with dl(running_workspace, "--", "echo one-shot-worked") as s:
            s.expect("one-shot-worked")
            assert s.wait(timeout=30) == 0

    def test_one_shot_command_propagates_failure(self, running_workspace):
        with dl(running_workspace, "--", "exit 7") as s:
            assert s.wait(timeout=60) == 7

    def test_command_runs_under_a_login_shell(self, running_workspace):
        """The reason the payload is wrapped in bash -lc; it must survive."""
        with dl(running_workspace, "--", LOGIN_SHELL_PROBE) as s:
            s.expect("SHELL-PATH:")
            assert "/.pixi/bin" in s.text or "/usr/local" in s.text

    def test_command_gets_a_controlling_terminal(self, running_workspace):
        """The root cause, asserted directly."""
        with dl(running_workspace, "--", NEEDS_A_TTY) as s:
            s.expect("TTY-STATUS:HAVE")
            s.expect(r"/dev/pts/\d+")
            assert "TTY-STATUS:MISSING" not in s.text
            assert s.wait(timeout=30) == 0

    def test_term_is_usable_inside_the_workspace(self, running_workspace):
        """The old transport set TERM=dumb, which TUIs treat as no terminal."""
        with dl(running_workspace, "--", 'T=TERM; echo "IS-${T}:$TERM"') as s:
            s.expect("IS-TERM:")
            assert "IS-TERM:dumb" not in s.text

    def test_lands_in_the_same_directory_as_the_devpod_transport(self, running_workspace):
        """Changing transport must not quietly change where the command runs.

        Neither invocation passes a working directory -- devpod's ssh server
        picks the workspaceFolder on its own, over the tunnel as well as
        directly -- and a command that started running in $HOME instead would be
        a silent, confusing regression rather than a visible failure.
        """
        probe = 'D=PWD; echo "IN-${D}:$PWD"'
        with dl(running_workspace, "--", probe) as s:
            s.expect(r"IN-PWD:\S+")
            through_ssh = reported_cwd(s.text)

        direct = subprocess.run(
            ["devpod", "ssh", running_workspace, "--command", f"bash -lc {shlex.quote(probe)}"],
            capture_output=True,
            text=True,
            check=False,
        )
        through_devpod = reported_cwd(direct.stdout)

        assert through_ssh == through_devpod


@pytest.mark.e2e
class TestLongRunningSession:
    """The half the user reported broken: a command that is meant not to exit."""

    def test_session_stays_up_instead_of_exiting_immediately(self, running_workspace):
        with dl(running_workspace, "--", LONG_RUNNING) as s:
            s.expect("READY-")
            s.assert_running(grace=5.0)

    def test_session_still_accepts_input_after_starting(self, running_workspace):
        """Alive is not the claim -- interactive is."""
        with dl(running_workspace, "--", LONG_RUNNING) as s:
            s.expect("READY-")
            s.send("hello")
            s.expect("ECHO:hello")

    def test_session_survives_several_round_trips(self, running_workspace):
        with dl(running_workspace, "--", LONG_RUNNING) as s:
            s.expect("READY-")
            for n in range(3):
                s.send(f"msg{n}")
                s.expect(f"ECHO:msg{n}")
            s.assert_running(grace=2.0)

    def test_session_exits_cleanly_when_the_command_ends(self, running_workspace):
        """It must not exit on its own, and must exit when told."""
        with dl(running_workspace, "--", LONG_RUNNING) as s:
            s.expect("READY-")
            s.assert_running(grace=2.0)
            s.send("quit")
            assert s.wait(timeout=60) == 0

    def test_agent_shaped_payload_starts_and_stays(self, running_workspace):
        """The reported bug, reproduced without depending on a coding agent.

        A payload that needs a terminal to start and then never exits is what
        `aid <repo>` runs. On the transport that has no pty this stops at
        TTY-STATUS:MISSING and exits 42 before READY is ever printed.
        """
        with dl(running_workspace, "--", NEEDS_A_TTY_AND_STAYS) as s:
            s.expect("TTY-STATUS:HAVE")
            s.expect("READY-")
            s.assert_running(grace=5.0)
            s.send("still-there")
            s.expect("ECHO:still-there")
            s.send("quit")
            assert s.wait(timeout=60) == 0


@pytest.mark.e2e
class TestAidStartsAnAgent:
    """`aid <ws>` is the reported failure, end to end."""

    def test_aid_leaves_an_interactive_agent_running(self, running_workspace):
        """Skipped unless the workspace actually has claude, since that is the
        one thing about this path that is not dl's to guarantee."""
        probe = PtySession(
            ["python", "-m", "devlaunch.dl", running_workspace, "--", "command -v claude"],
            timeout=90,
        )
        with probe:
            probe.wait(timeout=90)
            if "claude" not in probe.text:
                pytest.skip("no claude in this workspace")

        session = PtySession(["python", "-m", "devlaunch.aid", running_workspace], timeout=180)
        with session:
            # Claude Code prints its banner once the TUI is up; without a
            # terminal it exits before ever getting there.
            session.expect(r"Claude Code|Welcome to Claude")
            session.assert_running(grace=5.0)
