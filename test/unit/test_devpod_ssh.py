"""Tests for interpreting how a `devpod ssh` session ended.

The stderr fixtures here are devpod's real output, not an approximation of it:
devpod colours its log lines unconditionally (loft-sh/log calls ansi.Color
without checking whether the stream is a terminal), so the escape sequences are
part of what devlaunch actually has to read.
"""

import io

import pytest

from devlaunch import devpod_ssh
from devlaunch.devpod_ssh import DevpodFailed, RemoteExit


pytestmark = pytest.mark.unit


# The two lines a normal `exit` produced, verbatim from the report that prompted
# this. 130 is a shell exiting with the status of a Ctrl-C'd last command.
DEBUG_HINT_LINE = (
    "\x1b[97;1m20:41:27 \x1b[0m\x1b[91;1merror \x1b[0m"
    "Try using the --debug flag to see a more verbose output       root.go:106\n"
)
REMOTE_EXIT_LINE = (
    "\x1b[97;1m20:41:27 \x1b[0m\x1b[91;1mfatal \x1b[0m"
    "tunnel to container: run in container: ssh session: "
    "Process exited with status 130                                root.go:113\n"
)


def run_filter(lines):
    """Run the stderr filter, returning (recovered status, what reached stderr)."""
    out = io.StringIO()
    status = devpod_ssh.filter_devpod_stderr(lines, out)
    return status, out.getvalue()


class TestNormalSessionEnd:
    """A remote program exiting nonzero is not a devlaunch failure."""

    def test_exiting_a_shell_is_silent_and_keeps_the_shells_status(self):
        """The reported bug: `exit` printed an error and a fatal, and dl returned 1."""
        status, written = run_filter([DEBUG_HINT_LINE, REMOTE_EXIT_LINE])
        assert status == 130
        assert written == ""
        assert devpod_ssh.interpret(1, status) == RemoteExit(130)

    def test_a_signal_report_still_yields_the_status(self):
        """x/crypto appends the signal when the remote process was killed by one."""
        line = (
            "20:41:27 fatal tunnel to container: run in container: ssh session: "
            "Process exited with status 130 from signal SIGINT root.go:113\n"
        )
        status, written = run_filter([DEBUG_HINT_LINE, line])
        assert status == 130
        assert written == ""

    def test_a_clean_exit_reports_zero(self):
        assert devpod_ssh.interpret(0, None) == RemoteExit(0)

    def test_the_recovered_status_beats_devpods_own_exit_code(self):
        """devpod exits 1 next to every remote status, so 1 is never the answer."""
        assert devpod_ssh.interpret(1, 2) == RemoteExit(2)


class TestRealFailures:
    """Anything devpod says for its own sake still reaches the user, in order."""

    def test_a_genuine_failure_keeps_both_lines_and_their_order(self):
        """The hint is held to see what follows it, so it must not end up after."""
        fatal = "20:41:27 fatal tunnel to container: dial tcp: connection refused\n"
        status, written = run_filter([DEBUG_HINT_LINE, fatal])
        assert status is None
        assert written == DEBUG_HINT_LINE + fatal
        assert devpod_ssh.interpret(1, status) == DevpodFailed(1)

    def test_a_trailing_hint_with_nothing_after_it_is_still_released(self):
        status, written = run_filter([DEBUG_HINT_LINE])
        assert status is None
        assert written == DEBUG_HINT_LINE

    def test_unrelated_stderr_passes_through_untouched(self):
        lines = ["20:41:27 warn workspace is already running\n", "some remote stderr\n"]
        status, written = run_filter(lines)
        assert status is None
        assert written == "".join(lines)

    def test_a_lost_session_is_a_devpod_failure(self):
        """x/crypto's ExitMissingError carries no status, and is a real failure."""
        line = (
            "20:41:27 fatal tunnel to container: run in container: ssh session: "
            "wait: remote command exited without exit status or exit signal\n"
        )
        status, written = run_filter([line])
        assert status is None
        assert written == line

    def test_a_remote_program_printing_the_same_sentence_is_not_devpods_report(self):
        """Without a pty the remote program's own stderr arrives on this stream."""
        status, written = run_filter(["Process exited with status 7\n"])
        assert status is None
        assert written == "Process exited with status 7\n"
