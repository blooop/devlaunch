"""Tests for the pty harness itself, against local processes.

The e2e tests that matter need a container, so they don't run in CI. This
module exercises the same harness against `bash` on the host, which makes the
one thing those tests depend on -- that a process which never exits is detected
as running, and one that dies early is detected as dead -- checkable everywhere
in under a second.

The payloads here are the same shape as the real one: print a marker, then
block on input forever.
"""

import time

import pytest

from fixtures.pty_helpers import PtySession, PtyTimeout, strip_ansi

# A process with no exit of its own: announce readiness, then echo input until
# told to stop. Standing in for `claude`, which is the same shape and far less
# convenient to install.
LONG_RUNNING = r"""
echo READY
while IFS= read -r line; do
    [ "$line" = quit ] && exit 0
    echo "ECHO:$line"
done
"""


def bash(script: str) -> PtySession:
    return PtySession(["bash", "-c", script], timeout=15)


class TestStripAnsi:
    def test_removes_colour(self):
        assert strip_ansi("\x1b[38;2;215;119;87mClaude\x1b[39m") == "Claude"

    def test_removes_cursor_and_screen_control(self):
        assert strip_ansi("\x1b[?25l\x1b[2J\x1b[Hhello") == "hello"

    def test_leaves_plain_text_alone(self):
        assert strip_ansi("plain text") == "plain text"


class TestTerminalIsReal:
    """The harness has to produce a terminal, or it tests nothing."""

    def test_child_sees_a_tty_on_all_three_streams(self):
        with bash('for fd in 0 1 2; do test -t $fd && echo "TTY$fd"; done; echo DONE') as s:
            s.expect("DONE")
            assert "TTY0" in s.text
            assert "TTY1" in s.text
            assert "TTY2" in s.text

    def test_child_gets_a_controlling_terminal(self):
        with bash("tty") as s:
            s.expect(r"/dev/pts/\d+")

    def test_term_is_not_dumb(self):
        """TERM=dumb is enough on its own to make some TUIs refuse to start."""
        with bash('echo "TERM=$TERM"') as s:
            s.expect("TERM=")
            assert "TERM=dumb" not in s.text


class TestLongRunningProcess:
    """The pattern the container tests use, proven against a local shell."""

    def test_readiness_is_a_marker_not_a_sleep(self):
        with bash(LONG_RUNNING) as s:
            started = time.monotonic()
            s.expect("READY")
            assert time.monotonic() - started < 10

    def test_process_that_never_exits_is_reported_running(self):
        with bash(LONG_RUNNING) as s:
            s.expect("READY")
            s.assert_running(grace=1.0)

    def test_session_round_trips_input(self):
        """Alive is not enough -- it has to still be listening."""
        with bash(LONG_RUNNING) as s:
            s.expect("READY")
            s.send("ping")
            s.expect("ECHO:ping")
            s.assert_running(grace=0.5)

    def test_session_exits_when_asked(self):
        with bash(LONG_RUNNING) as s:
            s.expect("READY")
            s.send("quit")
            assert s.wait(timeout=10) == 0

    def test_closing_kills_a_process_that_would_never_exit(self):
        session = bash(LONG_RUNNING)
        session.start()
        session.expect("READY")
        session.close()
        assert not session.is_running()


class TestFailureModes:
    """A broken transport must fail loudly, not pass quietly."""

    def test_process_that_exits_early_fails_assert_running(self):
        """This is the bug: it printed, looked fine, and was already gone."""
        with bash("echo READY; exit 1") as s:
            s.expect("READY")
            with pytest.raises(AssertionError, match="should still be running"):
                s.assert_running(grace=1.0)

    def test_missing_marker_fails_with_the_output_attached(self):
        with bash("echo something-else") as s:
            with pytest.raises(PtyTimeout, match="something-else"):
                s.expect("READY", timeout=3)

    def test_expect_reports_an_exit_before_the_marker(self):
        with bash("exit 3") as s:
            with pytest.raises(PtyTimeout, match="exited before"):
                s.expect("READY", timeout=5)

    def test_exit_code_is_reported(self):
        with bash("exit 42") as s:
            assert s.wait(timeout=10) == 42

    def test_signal_death_is_reported_as_128_plus_signal(self):
        with bash("kill -9 $$") as s:
            assert s.wait(timeout=10) == 128 + 9
