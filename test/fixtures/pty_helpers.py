"""Drive a process through a real terminal, and assert about one that never exits.

Every other runner in this suite is subprocess.run with capture_output=True,
which is exactly the situation the code under test is meant to detect as "no
terminal". A test that used it could never reach the pty transport, so this
module spawns through pty.fork() instead: the child gets a pty as its
controlling terminal, so isatty() is true on all three of its streams and it
behaves the way it does under a developer's shell.

The other half of the problem is that the processes worth testing here --
`claude`, a shell, anything dl is asked to leave running -- have no exit to wait
for. Waiting for one is the bug being tested, so PtySession never does. It
waits for *output* instead:

    with PtySession(argv) as session:
        session.expect("READY")        # readiness, not a sleep
        session.assert_running()       # the thing the old transport got wrong
        session.send("ping")
        session.expect("ECHO:ping")    # still listening, not just still alive
        session.send("quit")
        assert session.wait() == 0     # exits when asked, so teardown is clean

expect() blocks only until its marker arrives, so a passing test costs no more
than the process takes to start, and a broken one fails at its timeout instead
of hanging the suite. assert_running() is what distinguishes "started and
stayed" from "started and died": a process that exits early makes the pty
readable at EOF, so the failure surfaces as a timeout in expect() or a False
from is_running(), never as a silent pass.
"""

from __future__ import annotations

import errno
import os
import pty
import re
import select
import signal
import time
from pathlib import Path
from typing import Dict, List, Optional

# Long enough for `devpod ssh` to build a tunnel into a cold container on a
# loaded CI machine, short enough that a genuinely stuck session fails the test
# instead of the job.
DEFAULT_TIMEOUT = 90.0

# Once a marker has been seen, "is it still up?" needs a moment to be worth
# anything -- a process that dies on the first keystroke dies within this.
LIVENESS_GRACE = 2.0

_ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b[]()][^\x07\x1b]*(?:\x07|\x1b\\)?|\x1b[=>]")


def strip_ansi(text: str) -> str:
    """Drop escape sequences so assertions can name what a human would read.

    A pty makes programs emit colour, cursor moves and title sets that a pipe
    never would, so the raw buffer is a poor thing to assert against.
    """
    return _ANSI.sub("", text)


class PtyTimeout(AssertionError):
    """A marker never arrived. An AssertionError so pytest reports it as a failure."""


class PtySession:
    """A child process on the far end of a pty, addressed by what it prints."""

    def __init__(
        self,
        argv: List[str],
        env: Optional[Dict[str, str]] = None,
        timeout: float = DEFAULT_TIMEOUT,
    ):
        self.argv = list(argv)
        self.env = env
        self.timeout = timeout
        self.pid: Optional[int] = None
        self.fd: Optional[int] = None
        self.buffer = ""
        self._status: Optional[int] = None

    def __enter__(self) -> "PtySession":
        self.start()
        return self

    def __exit__(self, *_exc) -> None:
        self.close()

    def start(self) -> "PtySession":
        """Fork the child onto a new pty and hand back once it is running."""
        pid, fd = pty.fork()
        if pid == 0:  # pragma: no cover - the child never returns to pytest
            try:
                env = dict(os.environ) if self.env is None else self.env
                # A pty with TERM=dumb is what pytest's own environment leaves
                # behind, and some programs treat it as "no terminal after all".
                env.setdefault("TERM", "xterm-256color")
                os.execvpe(self.argv[0], self.argv, env)
            except BaseException:  # noqa: BLE001 - last chance before _exit
                pass
            os._exit(127)  # noqa: SLF001 - the only correct exit in a pty child
        self.pid, self.fd = pid, fd
        return self

    @property
    def pty_fd(self) -> int:
        """The master side of the pty, or a clear error if start() was skipped."""
        if self.fd is None:
            raise AssertionError("session not started: call start() or use it as a context manager")
        return self.fd

    # -- reading -----------------------------------------------------------

    def _read_once(self, deadline: float) -> bool:
        """Append whatever is readable. False once the pty is at EOF."""
        remaining = max(0.0, deadline - time.monotonic())
        readable, _, _ = select.select([self.pty_fd], [], [], min(0.25, remaining))
        if not readable:
            return True
        try:
            chunk = os.read(self.pty_fd, 65536)
        except OSError as e:
            # EIO is how a pty reports "the far end closed", i.e. the child exited.
            if e.errno in (errno.EIO, errno.EBADF):
                return False
            raise
        if not chunk:
            return False
        self.buffer += chunk.decode("utf-8", errors="replace")
        return True

    def expect(self, pattern: str, timeout: Optional[float] = None) -> str:
        """Read until `pattern` appears, then return everything read so far.

        Matching is against the ansi-stripped buffer, and the pattern is a
        regular expression so a test can pin structure without pinning layout.
        """
        deadline = time.monotonic() + (self.timeout if timeout is None else timeout)
        compiled = re.compile(pattern)
        while True:
            if compiled.search(strip_ansi(self.buffer)):
                return self.buffer
            if time.monotonic() >= deadline:
                raise PtyTimeout(
                    f"never saw {pattern!r} in {self.timeout:.0f}s.\n"
                    f"--- pty output ---\n{strip_ansi(self.buffer)}\n--- end ---"
                )
            if not self._read_once(deadline):
                # EOF: one last look, since the match may be in the final chunk.
                if compiled.search(strip_ansi(self.buffer)):
                    return self.buffer
                raise PtyTimeout(
                    f"process exited before {pattern!r} appeared "
                    f"(exit status {self.wait(timeout=5)}).\n"
                    f"--- pty output ---\n{strip_ansi(self.buffer)}\n--- end ---"
                )

    @property
    def text(self) -> str:
        """Everything read so far, readable."""
        return strip_ansi(self.buffer)

    # -- writing -----------------------------------------------------------

    def send(self, text: str, newline: bool = True) -> None:
        """Type into the session, the way a user at the terminal would."""
        payload = text + ("\n" if newline else "")
        os.write(self.pty_fd, payload.encode())

    # -- liveness ----------------------------------------------------------

    def is_running(self) -> bool:
        """Whether the child is still alive, without blocking on it."""
        if self.pid is None or self._status is not None:
            return False
        waited, status = os.waitpid(self.pid, os.WNOHANG)
        if waited == 0:
            return True
        self._status = status
        return False

    def assert_running(self, grace: float = LIVENESS_GRACE) -> None:
        """Fail unless the process is still up `grace` seconds from now.

        The grace period is the point: the transport bug this guards against
        produced a process that started, printed, and exited immediately, which
        an instantaneous check would happily call alive.
        """
        deadline = time.monotonic() + grace
        while time.monotonic() < deadline:
            if not self.is_running():
                raise AssertionError(
                    f"process exited (status {self._status}) but should still be running.\n"
                    f"--- pty output ---\n{self.text}\n--- end ---"
                )
            self._read_once(deadline)
        assert self.is_running(), "process exited during the liveness grace period"

    def wait(self, timeout: Optional[float] = None) -> int:
        """Block until the child exits and return its exit code."""
        deadline = time.monotonic() + (self.timeout if timeout is None else timeout)
        while time.monotonic() < deadline:
            if not self.is_running():
                return self._exit_code()
            self._read_once(deadline)
        raise PtyTimeout(
            f"process did not exit within the timeout.\n"
            f"--- pty output ---\n{self.text}\n--- end ---"
        )

    def _exit_code(self) -> int:
        if self._status is None:
            return -1
        if os.WIFEXITED(self._status):
            return os.WEXITSTATUS(self._status)
        if os.WIFSIGNALED(self._status):
            return 128 + os.WTERMSIG(self._status)
        return -1

    def close(self) -> None:
        """Kill the session and reap it, so no test leaks a live container shell."""
        if self.pid is not None and self._status is None:
            for sig in (signal.SIGTERM, signal.SIGKILL):
                try:
                    os.kill(self.pid, sig)
                except ProcessLookupError:
                    break
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and self.is_running():
                    time.sleep(0.05)
                if not self.is_running():
                    break
        if self.fd is not None:
            try:
                os.close(self.fd)
            except OSError:
                pass
            self.fd = None


# devpod's own spellings for the ssh alias it publishes, as
# `rust/devlaunch-core/src/clients/ssh.rs` declares them (HOST_SUFFIX and
# MARKER_PREFIX). Duplicated here rather than imported because the binary owns
# them now and a test process cannot import a Rust constant; `ssh.rs` has its own
# tests for the alias it builds, so what this file needs is only the ability to
# ask the same question of a config file.
HOST_SUFFIX = ".devpod"
MARKER_PREFIX = "# DevPod Start "


def devpod_host_configured(workspace_id: str, config_path: Path) -> bool:
    """Whether devpod has published an ssh alias for this workspace.

    Matched on devpod's own start marker as a whole line, not as a substring:
    workspace ids share prefixes by construction (`devlaunch-main-abcdefgh` and
    `devlaunch-main-ijklmnop`), so a substring test would answer about a host
    alias belonging to a different container.
    """
    try:
        text = Path(config_path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        # No config, no permission, no alias -- all mean "no alias", which is the
        # answer, not an error.
        return False
    marker = f"{MARKER_PREFIX}{workspace_id}{HOST_SUFFIX}"
    return any(line.strip() == marker for line in text.splitlines())
