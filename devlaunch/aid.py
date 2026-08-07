"""aid - AI Develop: `dl`, with a coding agent started for you.

aid is a shortcut, not a second launcher. It rewrites its own command line into
a `dl` one and hands that to :func:`devlaunch.dl.main`, so:

    aid owner/repo@branch fix the flaky test

is exactly

    dl owner/repo@branch -- claude 'fix the flaky test'

Everything that decides how a workspace is obtained — the bare repo cache, the
worktree clone, the workspace id, the devpod container, the fast attach to one
that is already running, the forwarded gh login — happens inside dl, once. There
is no container machinery in this module, deliberately: an aid that built its own
would drift from dl and start rebuilding containers dl would have reused.
"""

from __future__ import annotations

import logging
import os
import shlex
import sys
from dataclasses import dataclass, field
from typing import Dict, List, Optional

from . import dl

# Base command per agent. The prompt, when there is one, is appended as a single
# quoted argument; each of these CLIs takes an initial prompt that way and then
# drops into its interactive session.
AGENT_COMMANDS: Dict[str, List[str]] = {
    "claude": ["claude"],
    "codex": ["codex"],
    "gemini": ["gemini", "--prompt-interactive"],
}

# Flags that pick the agent, e.g. `aid --gemini owner/repo ...`.
AGENT_FLAGS: Dict[str, str] = {f"--{name}": name for name in AGENT_COMMANDS}

DEFAULT_AGENT = "claude"

# Overrides the default agent for people who do not want to type a flag every
# time. A --flag on the command line still wins.
AGENT_ENV_VAR = "DEVLAUNCH_AID_AGENT"


class UsageError(Exception):
    """The aid command line could not be understood."""


@dataclass(frozen=True)
class AidArgs:
    """An aid command line, split into the pieces the dl one is built from.

    Only ever built by parse_aid_args, which refuses a command line without a
    workspace, so spec is a str here and every field is ready to use.
    """

    spec: str
    agent: str = DEFAULT_AGENT
    # dl options seen before the spec (`--devcontainer x`), passed through as-is.
    dl_options: List[str] = field(default_factory=list)
    prompt: str = ""


def default_agent(env: Optional[Dict[str, str]] = None) -> str:
    """Return the agent to use when no flag picks one."""
    environ = os.environ if env is None else env
    name = environ.get(AGENT_ENV_VAR, "").strip()
    if not name:
        return DEFAULT_AGENT
    if name not in AGENT_COMMANDS:
        raise UsageError(
            f"{AGENT_ENV_VAR}={name!r} is not a known agent. "
            f"Choose one of: {', '.join(sorted(AGENT_COMMANDS))}."
        )
    return name


def parse_aid_args(argv: List[str], env: Optional[Dict[str, str]] = None) -> AidArgs:
    """Split an aid command line into agent, dl options, workspace spec and prompt.

    The first argument that is neither an agent flag nor a dl option is the
    workspace spec; everything after it is the prompt, flags and all, so a
    prompt never has to be quoted to protect it from aid's own parsing.
    """
    agent = default_agent(env)
    dl_options: List[str] = []
    spec: Optional[str] = None
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg in AGENT_FLAGS:
            agent = AGENT_FLAGS[arg]
            i += 1
            continue
        if arg in dl.DL_VALUE_OPTIONS:
            # Take the value with it; dl reports a missing one.
            dl_options.extend(argv[i : i + 2])
            i += 2
            continue
        if arg.startswith("-"):
            dl_options.append(arg)
            i += 1
            continue
        spec = arg
        i += 1
        break

    if spec is None:
        raise UsageError("aid needs a workspace: aid <user/repo>[@branch] [prompt]")

    return AidArgs(spec=spec, agent=agent, dl_options=dl_options, prompt=" ".join(argv[i:]))


def build_agent_command(agent: str, prompt: str = "") -> str:
    """Build the shell command that starts the agent inside the workspace.

    Returned as one shell string because that is what dl's `-- <command>` form
    takes. The prompt is quoted here rather than reassembled by the caller, so
    the words the user typed reach the agent as the single argument they meant.
    """
    try:
        command = list(AGENT_COMMANDS[agent])
    except KeyError:
        raise UsageError(
            f"Unknown agent {agent!r}. Choose one of: {', '.join(sorted(AGENT_COMMANDS))}."
        ) from None
    if not prompt:
        # No prompt to be interactive about: start the agent's plain session.
        # gemini's --prompt-interactive would be a syntax error without one.
        return shlex.quote(command[0])
    command.append(prompt)
    return shlex.join(command)


def build_dl_args(parsed: AidArgs) -> List[str]:
    """Turn a parsed aid command line into the dl one that does the work."""
    return [
        *parsed.dl_options,
        parsed.spec,
        "--",
        build_agent_command(parsed.agent, parsed.prompt),
    ]


def print_help() -> None:
    """Print usage help."""
    agents = ", ".join(f"--{name}" for name in sorted(AGENT_COMMANDS))
    print(
        f"""aid - AI Develop: start a coding agent in a devlaunch workspace

aid is a shortcut for `dl <workspace> -- <agent> '<prompt>'`. The workspace is
opened by dl itself, so it is the same workspace, container and clone that
`dl <workspace>` gives you — started if it is stopped, attached to if it is
already running, and never rebuilt just because aid asked for it.

Usage:
    aid <user/repo>[@branch] [prompt...]   Open the workspace and start the agent
    aid <workspace> [prompt...]            Same, for an existing workspace or ./path

Options:
    {agents}
                                     Pick the agent (default: {DEFAULT_AGENT})
    --devcontainer <variant|path>    Passed through to dl
    --help, -h                       Show this help
    --version                        Show version

Environment:
    {AGENT_ENV_VAR}=<agent>       Change the default agent

Examples:
    aid blooop/devlaunch                       # Start {DEFAULT_AGENT} in the workspace
    aid blooop/devlaunch@fix/42 fix the bug    # Open the branch, hand over the prompt
    aid --gemini ./my-project explain this     # Pick a different agent

Everything else — listing, stopping, deleting, VS Code — is dl's job:
    dl --help
"""
    )


def main(argv: Optional[List[str]] = None) -> int:
    """Entry point for the aid command."""
    args = sys.argv[1:] if argv is None else list(argv)

    if not args or args[0] in ("--help", "-h"):
        print_help()
        return 0 if args else 1

    if args[0] == "--version":
        print(f"aid {dl.get_version()}")
        return 0

    try:
        parsed = parse_aid_args(args)
        dl_args = build_dl_args(parsed)
    except UsageError as e:
        logging.error(str(e))
        return 1

    logging.info("aid -> dl %s", shlex.join(dl_args))
    return dl.main(dl_args)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
