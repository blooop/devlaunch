"""How a `devpod ssh` session ended, recovered from what devpod reports.

devpod means to pass a remote process's exit status through. Its top-level error
handler does:

    if sshExitErr, ok := err.(*ssh.ExitError); ok {
        os.Exit(sshExitErr.ExitStatus())
    }

But by the time the error reaches there it has been wrapped three times —
`ssh session: %w` in cmd/machine/ssh.go, then "run in container", then "tunnel to
container" — and a bare type assertion does not see through `%w`. So every
nonzero remote exit misses that branch and lands on devpod's generic failure
path instead, which prints

    error Try using the --debug flag to see a more verbose output    root.go:106
    fatal tunnel to container: run in container: ssh session: Process exited with status 130

and exits 1.

Nothing has gone wrong in that example. A login shell exits with the status of
its last command, so a single Ctrl-C before typing `exit` is enough to make a
perfectly ordinary session end 130. The session ran and it ended; devpod just has
no way left to say so.

Both of those lines are Error/Fatal level, which loft-sh/log sends to stderr
(Info-level progress goes to stdout, so reading stderr does not hold back the
"waiting for workspace" chatter). That makes the status recoverable: read
devpod's stderr, take the status out of the message it buried it in, and hold
back the two lines that only exist because devpod could not report it properly.

The distinction the rest of devlaunch needs is which process the resulting number
came from, so it is a type rather than a bare int — see SshOutcome.
"""

import re
from dataclasses import dataclass
from typing import Iterable, NoReturn, Optional, TextIO

# devpod prints this immediately before the fatal it belongs to, so it has to be
# held for one line to see which fatal that is.
DEBUG_HINT = "Try using the --debug flag to see a more verbose output"

# The status golang.org/x/crypto/ssh formatted into an *ssh.ExitError:
# "Process exited with status 130", optionally " from signal SIGINT" and
# ". Reason was: ...". Anchored on devpod's "fatal" tag as well so a remote
# program printing the same sentence on its own stderr (which reaches us only
# when there is no pty) cannot be mistaken for devpod's report.
#
# No \b before "fatal": devpod colours the tag, and the escape it emits ends in
# "m", so there is no word boundary in front of it.
REMOTE_EXIT_RE = re.compile(r"fatal\b.*\bssh session: Process exited with status (\d+)")


@dataclass(frozen=True)
class RemoteExit:
    """devpod ran the remote program, and it exited with `status`.

    Not a devlaunch failure, whatever `status` is: the shell or command the user
    asked for ran to completion. `status` belongs to that program.
    """

    status: int


@dataclass(frozen=True)
class DevpodFailed:
    """devpod never ran the remote program, or lost it partway.

    `exit_code` is devpod's own. devpod has already written its diagnostics to
    stderr by the time this is constructed, so it carries no message of its own —
    there is nothing devlaunch knows that the user has not already been told.
    """

    exit_code: int


SshOutcome = RemoteExit | DevpodFailed


def assert_never(value: NoReturn) -> NoReturn:
    """Fail loudly on an SshOutcome arm nobody handled.

    A runtime backstop, not a compile-time one: `ty`, the checker this project
    runs in CI, does not currently reject a `match` that drops an arm. It is
    still worth having, because a `match` with no fallthrough returns None, and
    `main` returning None makes `dl` exit 0 — a new outcome would otherwise go
    out as success.

    Stands in for typing.assert_never, which needs 3.11; this project is 3.10+.
    """
    raise AssertionError(f"unhandled outcome: {value!r}")


def filter_devpod_stderr(lines: Iterable[str], out: TextIO) -> Optional[int]:
    """Forward devpod's stderr, holding back its report of a remote exit status.

    Returns that status if devpod reported one. Everything else is passed through
    verbatim and unbuffered, so a genuine devpod failure still reads exactly as
    it does today — including the --debug hint, which is released ahead of the
    fatal it precedes rather than after it.
    """
    remote_status: Optional[int] = None
    held_hint: Optional[str] = None

    for line in lines:
        match = REMOTE_EXIT_RE.search(line)
        if match:
            remote_status = int(match.group(1))
            # The hint introduced this fatal, so it goes with it.
            held_hint = None
            continue
        if DEBUG_HINT in line:
            held_hint = line
            continue
        if held_hint is not None:
            out.write(held_hint)
            held_hint = None
        out.write(line)
        out.flush()

    if held_hint is not None:
        out.write(held_hint)
        out.flush()

    return remote_status


def interpret(devpod_exit_code: int, remote_status: Optional[int]) -> SshOutcome:
    """Decide what a finished `devpod ssh` actually reported.

    A recovered remote status wins over devpod's own exit code, because devpod
    reports 1 alongside it regardless of what the remote program returned.
    """
    if remote_status is not None:
        return RemoteExit(remote_status)
    if devpod_exit_code == 0:
        return RemoteExit(0)
    # No status to recover. Either devpod really did fail, or a future devpod
    # unwraps the error properly and exits with the remote status itself — in
    # which case this is still the right number to pass on, and devpod stayed
    # quiet, so nothing spurious is printed either way.
    return DevpodFailed(devpod_exit_code)
