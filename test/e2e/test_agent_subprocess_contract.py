"""`dl <ws> -- <command>` as a program sees it: pipes on every stream, no terminal.

`test_interactive_session.py` already runs one-shot commands through this workspace
and already asserts that a failing one comes back non-zero. It does all of it on a
**pty**, because that is the transport a developer gets, and dl reaches the container
two different ways depending on which it is: an OpenSSH invocation carrying `-F` for
the pty path, and `devpod ssh --command` for the piped one. So none of those
assertions cover the shape a script runs in, and the shape a script runs in is the one
`docs/agents-using-dl.md` publishes a contract about.

Hence a second file rather than four more cases over there. Everything here runs with
stdout, stderr and stdin as pipes, which is what `subprocess.run(capture_output=True)`
gives and what an agent harness gives, and each test is one clause of the published
contract. `test/test_agent_contract_doc.py` is the other half of the pairing: it holds
the page to still making the claims these tests are the evidence for.

The stderr clause is the interesting one and it is an `xfail`. See
`test_stderr_is_the_commands_output_verbatim`.
"""

# Requesting a fixture shadows its name; that is how pytest is written.
# pylint: disable=redefined-outer-name

from __future__ import annotations

import os
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, Iterator

import pytest

from fixtures.e2e_helpers import WorkspaceTracker, create_e2e_workspace, dl_command
from fixtures.git_fixtures import build_repo_with_devcontainer

WORKSPACE_ID = "e2e-test-agent-contract"

# A string no part of dl's own output could produce, so "is this line the command's"
# is decidable rather than a judgement about phrasing.
MARKER = "AGENT-CONTRACT-MARKER"


@dataclass
class PipedWorkspace:
    """One started workspace, addressed the way a script addresses it."""

    workspace_id: str
    cache_dir: Path

    def env(self) -> Dict[str, str]:
        """Read fresh per invocation, for the reason the interactive fixture does.

        This fixture is module-scoped and so is built before `test/conftest.py`'s
        function-scoped autouse fixtures have run once. A snapshot taken at setup
        would predate `no_gh_token_forwarding` and carry the developer's own
        environment into every dl in the file.
        """
        return {**os.environ, "XDG_CACHE_HOME": str(self.cache_dir)}

    def run(self, *command: str, stdin: str | None = None) -> subprocess.CompletedProcess:
        """`dl <ws> -- <command>` with every stream a pipe.

        No pty anywhere, deliberately: `capture_output` is what makes stdout and
        stderr pipes, and passing `input` (even the empty string) is what makes
        stdin one. A test here that let a terminal in would be testing the other
        file's transport.
        """
        return subprocess.run(
            [*dl_command(), self.workspace_id, "--", *command],
            env=self.env(),
            capture_output=True,
            text=True,
            input="" if stdin is None else stdin,
            timeout=180,
            check=False,
        )


@pytest.fixture(scope="module")
def piped(tmp_path_factory) -> Iterator[PipedWorkspace]:
    """One workspace for the file, built by this run and deleted after it.

    Module-scoped because a container build is the expensive part and every test
    below wants the same one. The tracker is this module's own, and `cleanup()`
    runs whether the tests passed, failed or never got that far: pytest will not
    hand a function-scoped `devpod_cleanup` to a module-scoped fixture, and the
    class behind both is the same one, so a leak is caught the same way.

    No ssh routing, unlike the interactive fixture. That scratch `HOME` exists so
    an OpenSSH lookup cannot succeed by accident against the developer's machine,
    and the piped transport makes no OpenSSH lookup: it is `devpod ssh --command`,
    which resolves the workspace out of this run's own `DEVPOD_HOME`.
    """
    root = tmp_path_factory.mktemp("agent-contract")
    cache_dir = root / "cache"
    source = build_repo_with_devcontainer(root / "repo")
    tracker = WorkspaceTracker()
    try:
        create_e2e_workspace(
            str(source),
            WORKSPACE_ID,
            cleanup=tracker,
            env={**os.environ, "XDG_CACHE_HOME": str(cache_dir)},
        )
        yield PipedWorkspace(workspace_id=WORKSPACE_ID, cache_dir=cache_dir)
    finally:
        tracker.cleanup()


def _shows(result: subprocess.CompletedProcess) -> str:
    """Both streams and the status, for a failure message worth reading.

    Every assertion in this file is about which stream something landed on, so a
    bare `assert x in stdout` failing without showing stderr hides the answer to
    the only question anyone will ask next.
    """
    return f"\nexit: {result.returncode}\nstdout: {result.stdout!r}\nstderr: {result.stderr!r}"


@pytest.mark.e2e
@pytest.mark.creates_workspace
def test_the_workspace_answers_at_all(piped):
    """The premise the rest of the file rests on.

    Without it, a container that never came up makes every assertion below fail
    for the same uninteresting reason, and the report says four things are broken
    when one is.
    """
    result = piped.run("sh", "-c", f"echo {MARKER}")
    assert result.returncode == 0, f"the workspace did not run a command{_shows(result)}"
    assert MARKER in result.stdout, _shows(result)


@pytest.mark.e2e
@pytest.mark.parametrize("status", [0, 1, 42, 127])
def test_the_exit_status_is_the_commands(piped, status):
    """Clause one. Several values, because 0 and 1 are also dl's own.

    `dl` exits 1 for a refusal and 127 for a missing devpod, so a transport that
    dropped the child's status on the floor and reported its own opinion would
    still look right for two of the four. 42 is the one that cannot be anything
    else, and 127 is the one that would be quietly wrong.
    """
    result = piped.run("sh", "-c", f"exit {status}")
    assert result.returncode == status, (
        f"`sh -c 'exit {status}'` came back as {result.returncode}{_shows(result)}"
    )


@pytest.mark.e2e
def test_stdout_carries_the_commands_output_and_nothing_of_dls(piped):
    """Clause two, and the one a parsing caller depends on completely.

    dl narrates a launch: whether the workspace was already running, the ssh
    invocation it is about to make. All of it belongs on stderr, and a single line
    of it on stdout turns `json.loads(result.stdout)` from working into failing at
    whichever call site happens to hit a cold workspace.

    Asserted as an exact match on the whole stream rather than as `MARKER in
    stdout`, because "the command's output" and "the command's output plus a
    banner" both satisfy the weaker form and only one of them is the contract.
    """
    result = piped.run("sh", "-c", f"echo {MARKER}")
    assert result.stdout == f"{MARKER}\n", (
        f"stdout must be the command's output alone; something else wrote to it{_shows(result)}"
    )


@pytest.mark.e2e
def test_stdin_reaches_the_command(piped):
    """Clause three.

    `cat` rather than `read`, so what is asserted is that the bytes arrived rather
    than that some shell builtin was willing to wait for them.
    """
    result = piped.run("cat", stdin=f"{MARKER}\n")
    assert result.returncode == 0, _shows(result)
    assert MARKER in result.stdout, f"piped stdin did not reach the command{_shows(result)}"


@pytest.mark.e2e
def test_no_terminal_is_required(piped):
    """Clause four, asked where it can actually fail.

    Every other test in this file already runs without a pty, so this one asks the
    container what it sees rather than re-asserting the same setup: a transport
    that quietly allocated a terminal anyway would pass all of them and still
    break a caller that checks `isatty` to decide whether to emit colour.
    """
    result = piped.run("sh", "-c", "test -t 1 && echo TTY || echo NO-TTY")
    assert result.returncode == 0, _shows(result)
    assert "NO-TTY" in result.stdout, (
        f"a piped `dl -- ` still gave the command a terminal on stdout{_shows(result)}"
    )


@pytest.mark.e2e
def test_merging_inside_the_container_is_a_working_recipe(piped):
    """The workaround `docs/agents-using-dl.md` tells a caller to use.

    It is documented, so it is tested: both lines arrive on stdout, in the right
    order, with nothing between them, and the exit status still belongs to the
    command. If this breaks, the page is telling callers to do something that does
    not work, which is worse than the mangling it routes around.
    """
    result = piped.run("sh", "-c", f"{{ echo {MARKER}-out; echo {MARKER}-err >&2; }} 2>&1; exit 3")
    assert result.returncode == 3, _shows(result)
    assert result.stdout == f"{MARKER}-out\n{MARKER}-err\n", (
        f"the documented `2>&1` recipe did not return both streams cleanly{_shows(result)}"
    )


@pytest.mark.e2e
@pytest.mark.xfail(
    strict=True,
    reason=(
        "devpod's stream logger reformats the command's stderr on the way out: it "
        "arrives timestamped, level-prefixed, ANSI-coloured and with a Go source "
        "location appended. docs/agents-using-dl.md documents the `2>&1` workaround "
        "instead. This xfail is strict so that fixing the transport turns the suite "
        "red and the page's workaround section gets removed in the same change."
    ),
)
def test_stderr_is_the_commands_output_verbatim(piped):
    """The clause the contract does not yet keep, written as the contract wants it.

    Deliberately not written as an assertion that the mangling happens. A test that
    pins current behaviour makes the bug part of the spec and quietly outlives the
    fix; a strict xfail of the *desired* behaviour fails the day it starts working,
    which is the day the documentation has to change.
    """
    result = piped.run("sh", "-c", f"echo {MARKER} >&2")
    assert result.returncode == 0, _shows(result)
    assert result.stderr.endswith(f"{MARKER}\n"), (
        f"stderr should be the command's, verbatim{_shows(result)}"
    )


@pytest.mark.e2e
def test_the_launch_narration_is_on_stderr_where_a_caller_can_ignore_it(piped):
    """The other side of clause two: the chatter exists, and it has a home.

    Worth its own case because "stdout is clean" has a cheap wrong way to pass,
    which is dl printing nothing at all anywhere. The narration is useful to a
    person watching a cold start, so the contract is that it is on stderr rather
    than that it is gone.
    """
    result = piped.run("sh", "-c", "true")
    assert result.returncode == 0, _shows(result)
    assert result.stderr.strip(), (
        "dl narrated nothing at all; the contract is that its output is on stderr, "
        f"not that there is none{_shows(result)}"
    )
