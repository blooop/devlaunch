"""Carry a terminal into the workspace for commands that need one.

`devpod ssh --command` never asks its ssh session for a pty. It requests one
only for a bare interactive attach, so anything started through --command runs
with stdin, stdout and stderr on pipes and TERM=dumb, and there is no devpod
flag that forces the matter.

One-shot commands don't care. Interactive ones do, and they don't fail in a way
that looks like a missing terminal: `claude` reads the pipe as "invoked
non-interactively", switches to --print mode, and exits -- so `aid <repo>`
returned to the shell instead of leaving a session behind, and `dl <ws> --
claude 'fix it'` printed one answer and stopped.

devpod already publishes the way out. Every `devpod up` writes an ssh host alias
`<workspace>.devpod` into ~/.ssh/config whose ProxyCommand tunnels through
`devpod ssh --stdio`, so OpenSSH can open the same session devpod would and, with
-t, ask it for a pty. That gives the payload a real terminal, and gives the user
OpenSSH's terminal handling -- raw mode, window size, SIGWINCH -- which is what a
TUI needs and what a hand-rolled pty proxy in dl would have to reimplement.

This module only decides and composes; dl.workspace_ssh does the spawning. The
decision is deliberately the one ssh itself makes: use a terminal when there is
a terminal to use. A redirected `dl <ws> -- ls > out.txt` keeps the old
transport and so keeps its output free of escape sequences.
"""

from __future__ import annotations

import os
import pathlib
import shlex
import sys
from typing import Any, Iterable, List, Optional

# Where devpod writes its host aliases. A module-level path rather than a call
# to Path.home() per lookup so tests can point it at a fixture.
SSH_CONFIG_PATH = pathlib.Path.home() / ".ssh" / "config"

# devpod names each alias after the workspace, and brackets the block with
# markers it recognises again on the next `up`.
HOST_SUFFIX = ".devpod"
MARKER_PREFIX = "# DevPod Start "

# Set this to keep every command on the devpod transport, whatever the terminal
# says. An escape hatch for a machine where the ssh alias is stale or the
# tunnel misbehaves, matching DEVLAUNCH_NO_GH_TOKEN in spirit.
DISABLE_VAR = "DEVLAUNCH_NO_TTY"

_FALSEY = ("", "0", "false", "no")


def disabled() -> bool:
    """Whether the user opted this machine out of the pty transport."""
    return os.environ.get(DISABLE_VAR, "").strip().lower() not in _FALSEY


def host_alias(workspace_id: str) -> str:
    """The ssh host name devpod publishes for a workspace."""
    return f"{workspace_id}{HOST_SUFFIX}"


def have_terminal(stdin: Any = None, stdout: Any = None) -> bool:
    """Whether dl was run from a terminal it can hand to the workspace.

    Both directions have to be a terminal: a pty on stdout with stdin redirected
    would give a TUI a screen it cannot receive keystrokes on, and a pty on
    stdin with stdout redirected would fill the redirect with escape sequences.

    Anything that isn't a real stream -- pytest's capture object, a closed file
    -- counts as no terminal, because that is what it behaves like.
    """
    if disabled():
        return False
    streams = (
        sys.stdin if stdin is None else stdin,
        sys.stdout if stdout is None else stdout,
    )
    for stream in streams:
        try:
            if not stream.isatty():
                return False
        except (AttributeError, ValueError, OSError):
            return False
    return True


def devpod_host_configured(workspace_id: str, config_path: Optional[pathlib.Path] = None) -> bool:
    """Whether devpod has published an ssh alias for this workspace.

    Matched on devpod's own start marker as a whole line, not as a substring:
    workspace ids share prefixes by construction (`devlaunch-main-abcdefgh` and
    `devlaunch-main-ijklmnop`), so a substring test would route a command at a
    host alias belonging to a different container.
    """
    path = SSH_CONFIG_PATH if config_path is None else config_path
    try:
        text = pathlib.Path(path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        # No config, no permission, no alias -- all mean "fall back", never
        # "fail the launch".
        return False
    marker = f"{MARKER_PREFIX}{host_alias(workspace_id)}"
    return any(line.strip() == marker for line in text.splitlines())


def ssh_command_args(
    workspace_id: str,
    command: str,
    send_env: Iterable[str] = (),
    workdir: Optional[str] = None,
) -> List[str]:
    """Build the OpenSSH invocation that runs `command` under a pty.

    -t is what the whole module exists for: without it ssh runs a command with
    no terminal, which is the situation being escaped.

    send_env names variables only. OpenSSH reads their values from its own
    environment, so a forwarded token never appears in argv where `ps` would
    show it to every other user on the host -- the same discipline gh_auth
    applies to the devpod transport.
    """
    args = ["ssh", "-t"]
    for name in send_env:
        args.extend(["-o", f"SendEnv={name}"])
    args.append(host_alias(workspace_id))
    # ssh has no --workdir, so a directory has to travel inside the command.
    payload = command
    if workdir:
        payload = f"cd {shlex.quote(workdir)} && {command}"
    args.append(payload)
    return args
