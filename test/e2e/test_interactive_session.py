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

Everything they need is built by the module. That is a change from the version
that asked for `DEVLAUNCH_E2E_WORKSPACE` to name a workspace somebody had
already started, which could not work and had opted out of all thirteen tests on
every run since the suite began scoping `DEVPOD_HOME` to a fresh directory: a
workspace outside that directory is one this run's devpod cannot describe,
attach to, or find an ssh alias for. So the variable is gone rather than fixed --
an opt-in that cannot be taken is worse than no opt-in, and it was buying a
saved image pull at the price of thirteen invisible tests.
"""

# Requesting a fixture shadows its name; that is how pytest is written.
# pylint: disable=redefined-outer-name

from __future__ import annotations

import os
import re
import shlex
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, Iterator

import pytest

from fixtures.e2e_guard import opt_out
from fixtures.e2e_helpers import (
    ScopedSsh,
    WorkspaceTracker,
    aid_command,
    create_e2e_workspace,
    dl_command,
    route_ssh_through,
    scoped_ssh_config,
)
from fixtures.git_fixtures import build_repo_with_devcontainer
from fixtures.pty_helpers import PtySession, devpod_host_configured

# One id for the module, private to the run: `DEVPOD_HOME` is a fresh directory
# every session, so two concurrent runs cannot collide on it. No other e2e
# workspace id contains this one as a substring, which is a property the purge
# test's assertions depend on across the suite.
WORKSPACE_ID = "e2e-test-interactive"

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

# Whether the workspace has a coding agent in it, and the trick again -- for the
# one probe where getting it wrong is invisible rather than loud. `command -v
# claude` names claude in the payload, and dl echoes the payload it is about to
# run, so a test that searched the buffer for "claude" found it in dl's own log
# line and concluded the agent was installed. Assembled at runtime, `AGENT-YES`
# exists only in the output.
CLAUDE_PROBE = (
    'if command -v claude >/dev/null 2>&1; then S=YES; else S=NO; fi; A=AGENT; echo "${A}-$S"'
)


@dataclass(frozen=True)
class InteractiveWorkspace:
    """A started workspace, and an environment from which dl can reach it."""

    workspace_id: str
    routing: ScopedSsh
    cache_dir: Path

    @property
    def ssh_config(self) -> Path:
        """The ssh config both dl and OpenSSH resolve in this environment."""
        return self.routing.config

    def env(self) -> Dict[str, str]:
        """The environment for one invocation, read fresh every time.

        Fresh rather than captured at fixture setup, and the difference is not
        cosmetic: this fixture is module-scoped, so it is built *before* the
        function-scoped autouse fixtures in `test/conftest.py` have run for the
        first test. A snapshot taken there predates
        `no_gh_token_forwarding` and would carry the developer's own environment
        into every dl in the file -- which is exactly what it did, until the run
        that printed `gh auth token exited 1` said so.
        """
        return self.routing.env({**os.environ, "XDG_CACHE_HOME": str(self.cache_dir)})

    def dl(self, *args: str, timeout: float = 120) -> PtySession:
        """`dl` on a pty, the way a developer runs it."""
        return PtySession(
            [*dl_command(), self.workspace_id, *args], env=self.env(), timeout=timeout
        )

    def aid(self, *args: str, timeout: float = 180) -> PtySession:
        """`aid` on a pty. Its own seam, because it is its own binary (#252 §1)."""
        return PtySession(
            [*aid_command(), self.workspace_id, *args], env=self.env(), timeout=timeout
        )


@pytest.fixture(scope="module")
def workspace(tmp_path_factory) -> Iterator[InteractiveWorkspace]:
    """One started workspace, built by this run, reused by every test here.

    Module-scoped because creating one costs an image pull and a container
    build, and every test in the file wants the same thing from it. Built rather
    than adopted: a workspace this run did not create is not in this run's
    `DEVPOD_HOME` and so cannot be reached at all -- see the module docstring.

    Nothing beyond `devpod up` is needed before dl's *pty* transport can be
    exercised, and that is a change worth naming. `route_ssh_through` supplies one
    thing and it is a negative: a scratch `HOME` with no `.ssh/config` in it, so
    neither lookup can succeed by accident against the developer's machine. Both
    of them are dl's own work. dl resolves `DEVPOD_SSH_CONFIG` to decide the alias
    exists, and dl passes that same file to OpenSSH as `-F` because OpenSSH reads
    neither that variable nor `$HOME`. So every pty assertion below is also an
    assertion about devlaunch#421: reaching a container at all means dl looked
    where devpod writes *and* told ssh to look there too.

    This used to be propped up by an `ssh` shim on `PATH` that prepended the `-F`
    itself, which is why the second half of #421 shipped to review with a green
    e2e run. It is gone.

    The tracker is this module's own, and `cleanup()` runs whether the tests
    passed, failed, or never got that far. `devpod_cleanup` is function-scoped
    and pytest will not hand a function-scoped fixture to a module-scoped one;
    the class behind both is the same, so a leak is caught the same way.

    Whether devpod runs at all is settled once, in the directory's conftest.
    """
    root = tmp_path_factory.mktemp("interactive-session")
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
        yield InteractiveWorkspace(
            workspace_id=WORKSPACE_ID,
            routing=route_ssh_through(scoped_ssh_config(), root / "ssh"),
            cache_dir=cache_dir,
        )
    finally:
        tracker.cleanup()


def reported_cwd(output: str) -> str:
    """Pull the directory the probe printed, failing loudly if it printed none."""
    match = re.search(r"IN-PWD:(\S+)", output)
    assert match, f"no working directory in output:\n{output}"
    return match.group(1)


@pytest.mark.e2e
@pytest.mark.creates_workspace
class TestCommandGetsATerminal:
    """The half the user reported working -- kept working, now with a terminal."""

    def test_host_alias_exists_for_the_workspace(self, workspace):
        """The pty transport reads devpod's own alias; without it dl falls back.

        Asked of the config this run's `devpod up` writes to, which is the file
        dl reads in this environment -- `DEVPOD_SSH_CONFIG` names it, and dl
        resolves that ahead of `~/.ssh/config`. Asking about the developer's real
        config instead would be a question about their machine.
        """
        assert devpod_host_configured(workspace.workspace_id, config_path=workspace.ssh_config), (
            "devpod wrote no ssh host alias for this workspace, so dl will fall "
            "back to the transport that has no terminal"
        )

    def test_openssh_is_told_to_read_the_config_devpod_published_into(self, workspace):
        """The second half of devlaunch#421, and the only test here that pins it.

        The first half taught dl to *read* `DEVPOD_SSH_CONFIG`. OpenSSH reads
        neither that variable nor `$HOME` -- it resolves the default user config
        through `getpwuid(getuid())` -- so a dl that decided it had a terminal and
        then ran a bare `ssh -t <alias>` sent OpenSSH after a host it could not
        resolve. Measured against this very workspace, with the shim that used to
        hide it removed: `ssh: Could not resolve hostname
        e2e-test-interactive.devpod`, exit 255, and `dl <ws> -- <cmd>` did not run
        at all. Not "ran without a terminal" -- did not run.

        Two assertions, and both halves are needed. The invocation dl logs has to
        name the config (a fact about argv, which is cheap and could be wrong
        about reality), and the command has to come back with output from inside
        the container (a fact about reality, which is what says the argv works).
        The environment is what makes the pair meaningful: the alias exists only
        in `DEVPOD_SSH_CONFIG`, and `HOME` holds no `.ssh/config`, so there is no
        second path by which OpenSSH could have resolved it.
        """
        config = str(workspace.ssh_config)
        assert not (workspace.routing.home / ".ssh" / "config").exists(), (
            "the assertion below only means something while the run's config is "
            "the sole place the alias can be found"
        )

        with workspace.dl("--", "echo reached-through-$(hostname)") as s:
            s.expect(r"reached-through-\S+")
            assert f"-F {config}" in s.text, (
                "dl must hand OpenSSH the config it read the alias out of; "
                f"invocation was:\n{s.text}"
            )
            assert "Could not resolve hostname" not in s.text
            assert s.wait(timeout=30) == 0

    def test_one_shot_command_still_runs_and_reports_its_output(self, workspace):
        with workspace.dl("--", "echo one-shot-worked") as s:
            s.expect("one-shot-worked")
            assert s.wait(timeout=30) == 0

    def test_one_shot_command_propagates_failure(self, workspace):
        with workspace.dl("--", "exit 7") as s:
            assert s.wait(timeout=60) == 7

    def test_command_runs_under_a_login_shell(self, workspace):
        """The reason the payload is wrapped in bash -lc; it must survive."""
        with workspace.dl("--", LOGIN_SHELL_PROBE) as s:
            s.expect("SHELL-PATH:")
            assert "/.pixi/bin" in s.text or "/usr/local" in s.text

    def test_command_gets_a_controlling_terminal(self, workspace):
        """The root cause, asserted directly."""
        with workspace.dl("--", NEEDS_A_TTY) as s:
            s.expect("TTY-STATUS:HAVE")
            s.expect(r"/dev/pts/\d+")
            assert "TTY-STATUS:MISSING" not in s.text
            assert s.wait(timeout=30) == 0

    def test_term_is_usable_inside_the_workspace(self, workspace):
        """The old transport set TERM=dumb, which TUIs treat as no terminal."""
        with workspace.dl("--", 'T=TERM; echo "IS-${T}:$TERM"') as s:
            s.expect("IS-TERM:")
            assert "IS-TERM:dumb" not in s.text

    def test_lands_in_the_same_directory_as_the_devpod_transport(self, workspace):
        """Changing transport must not quietly change where the command runs.

        Neither invocation passes a working directory -- devpod's ssh server
        picks the workspaceFolder on its own, over the tunnel as well as
        directly -- and a command that started running in $HOME instead would be
        a silent, confusing regression rather than a visible failure.
        """
        probe = 'D=PWD; echo "IN-${D}:$PWD"'
        with workspace.dl("--", probe) as s:
            s.expect(r"IN-PWD:\S+")
            through_ssh = reported_cwd(s.text)

        direct = subprocess.run(
            [
                "devpod",
                "ssh",
                workspace.workspace_id,
                "--command",
                f"bash -lc {shlex.quote(probe)}",
            ],
            capture_output=True,
            text=True,
            check=False,
        )
        through_devpod = reported_cwd(direct.stdout)

        assert through_ssh == through_devpod


@pytest.mark.e2e
@pytest.mark.creates_workspace
class TestLongRunningSession:
    """The half the user reported broken: a command that is meant not to exit."""

    def test_session_stays_up_instead_of_exiting_immediately(self, workspace):
        with workspace.dl("--", LONG_RUNNING) as s:
            s.expect("READY-")
            s.assert_running(grace=5.0)

    def test_session_still_accepts_input_after_starting(self, workspace):
        """Alive is not the claim -- interactive is."""
        with workspace.dl("--", LONG_RUNNING) as s:
            s.expect("READY-")
            s.send("hello")
            s.expect("ECHO:hello")

    def test_session_survives_several_round_trips(self, workspace):
        with workspace.dl("--", LONG_RUNNING) as s:
            s.expect("READY-")
            for n in range(3):
                s.send(f"msg{n}")
                s.expect(f"ECHO:msg{n}")
            s.assert_running(grace=2.0)

    def test_session_exits_cleanly_when_the_command_ends(self, workspace):
        """It must not exit on its own, and must exit when told."""
        with workspace.dl("--", LONG_RUNNING) as s:
            s.expect("READY-")
            s.assert_running(grace=2.0)
            s.send("quit")
            assert s.wait(timeout=60) == 0

    def test_agent_shaped_payload_starts_and_stays(self, workspace):
        """The reported bug, reproduced without depending on a coding agent.

        A payload that needs a terminal to start and then never exits is what
        `aid <repo>` runs. On the transport that has no pty this stops at
        TTY-STATUS:MISSING and exits 42 before READY is ever printed.
        """
        with workspace.dl("--", NEEDS_A_TTY_AND_STAYS) as s:
            s.expect("TTY-STATUS:HAVE")
            s.expect("READY-")
            s.assert_running(grace=5.0)
            s.send("still-there")
            s.expect("ECHO:still-there")
            s.send("quit")
            assert s.wait(timeout=60) == 0


@pytest.mark.e2e
@pytest.mark.creates_workspace
class TestAidStartsAnAgent:
    """`aid <ws>` is the reported failure, end to end."""

    def test_aid_leaves_an_interactive_agent_running(self, workspace):
        """Skipped unless the workspace actually has claude, since that is the
        one thing about this path that is not dl's to guarantee.

        Which is what happens on every machine that runs this suite as written:
        the fixture's devcontainer is `devcontainers/base:ubuntu` and carries no
        coding agent. Installing one would put an npm registry and an
        Anthropic-shaped download inside an e2e run, which is the dependency the
        payloads above were built to avoid -- so this is the one test in the file
        that declines rather than runs, and it says so in its own words. What it
        would add over `test_agent_shaped_payload_starts_and_stays` is the `aid`
        entry point rather than the transport, and the transport is the subject.
        """
        probe = workspace.dl("--", CLAUDE_PROBE, timeout=90)
        with probe:
            probe.expect("AGENT-(YES|NO)")
            if "AGENT-YES" not in probe.text:
                opt_out("no claude in this workspace")

        session = workspace.aid()
        with session:
            # On a terminal a promptless `aid` asks for the prompt while the
            # workspace boots; an empty Enter is the plain session this test has
            # always been about.
            session.expect(r"press Enter")
            session.send("")
            # Claude Code prints its banner once the TUI is up; without a
            # terminal it exits before ever getting there.
            session.expect(r"Claude Code|Welcome to Claude")
            session.assert_running(grace=5.0)
