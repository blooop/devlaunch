"""Put the tools a session always needs into every workspace devlaunch opens.

`gh` and `claude` are not optional extras for the way these workspaces get used:
`dl` already forwards the host's GitHub login into every container, which is
worth nothing when the container has no `gh` to spend it, and `aid` exists to
run `claude` in there. Both currently arrive only when the repo's own
devcontainer.json arranges them -- this repo does, through
`.devcontainer/claude-code/`, which is why `claude` is present in its workspaces
and `gh` (a project pixi dependency, reachable only as `pixi run gh`) is not.

A guarantee that depends on the repo is not a guarantee. `dl` launches arbitrary
repos, so the tools have to come from the invocation, the same argument
gh_auth makes for the token and workspace_ssh's login shell makes for PATH.

Installing them costs a devpod round-trip on `up`, which already runs for
seconds, and nothing at all on the attach paths -- the script exits before doing
any work when both tools are already there, and `up` is the only caller.

Two consequences worth knowing:

- A workspace that is already running when `dl` reaches it skips `up` entirely
  (the fast-attach path), so it is not topped up. That covers workspaces started
  by something other than `dl`, and ones created before this existed; both get
  the tools on their next `dl <ws> restart` or `up`.
- Provisioning is a convenience, so a failed install costs the workspace its
  tools and not its launch. Every failure here is logged and swallowed.
"""

import logging
import os
import shlex
from dataclasses import dataclass
from typing import List, Optional, Sequence

# Set this to opt a machine out of installing tools into workspaces.
DISABLE_VAR = "DEVLAUNCH_NO_TOOLS"

_FALSEY = ("", "0", "false", "no")

# The claude package lives in a personal channel rather than conda-forge.
BLOOOP_CHANNEL = "https://prefix.dev/blooop"


@dataclass(frozen=True)
class Tool:
    """A binary a session must be able to run, and the pixi package providing it.

    `command` is what a shell has to find on PATH, which is not always the
    package name -- `claude` ships in `claude-shim` -- so both are recorded
    rather than one being derived from the other.
    """

    command: str
    package: str
    channel: Optional[str] = None

    @property
    def install_args(self) -> List[str]:
        """The `pixi global install` arguments that provide this tool."""
        if self.channel:
            return ["--channel", self.channel, self.package]
        return [self.package]


REQUIRED_TOOLS: Sequence[Tool] = (
    Tool(command="gh", package="gh"),
    Tool(command="claude", package="claude-shim", channel=BLOOOP_CHANNEL),
)


def provisioning_disabled() -> bool:
    """Whether the user opted this machine out of installing tools."""
    return os.environ.get(DISABLE_VAR, "").strip().lower() not in _FALSEY


def _install_line(tool: Tool) -> str:
    args = " ".join(shlex.quote(arg) for arg in tool.install_args)
    return (
        f"if ! command -v {shlex.quote(tool.command)} >/dev/null 2>&1; then\n"
        f'  echo "devlaunch: installing {tool.command}"\n'
        f"  pixi global install {args} || failed=1\n"
        f"fi"
    )


def provision_script(tools: Sequence[Tool] = REQUIRED_TOOLS) -> str:
    """The shell script that makes `tools` available in a workspace.

    Idempotent and cheap on the common path: every tool already on PATH is
    skipped, so a workspace that has been provisioned before does nothing but
    answer. It runs under a login shell (see ensure_tools), which is what puts
    an earlier run's ~/.pixi/bin on PATH -- checked from a non-login shell every
    tool would look missing and be reinstalled on every launch.

    Exits 0 unless an install actually failed, so "nothing to do" and "all
    installs worked" are the same answer to the caller.
    """
    all_present = " && ".join(
        f"command -v {shlex.quote(tool.command)} >/dev/null 2>&1" for tool in tools
    )
    installs = "\n".join(_install_line(tool) for tool in tools)
    # The trampoline pixi writes into ~/.pixi/bin does not work for packages
    # that ship a shell script, which is why the env's own bin directory is
    # added too -- the same workaround .devcontainer/claude-code/install.sh
    # carries, for the same package.
    profile_lines = "\n".join(
        [
            'PROFILE="$HOME/.profile"',
            'grep -q "\\.pixi/bin" "$PROFILE" 2>/dev/null || '
            'echo \'export PATH="$HOME/.pixi/bin:$PATH"\' >> "$PROFILE"',
            'grep -q "pixi/envs/claude-shim" "$PROFILE" 2>/dev/null || '
            'echo \'[ -d "$HOME/.pixi/envs/claude-shim/bin" ] && '
            'export PATH="$HOME/.pixi/envs/claude-shim/bin:$PATH"\' >> "$PROFILE"',
        ]
    )
    return "\n".join(
        [
            "set -u",
            "failed=0",
            # Everything already there: leave without touching pixi, the
            # profile, or the network. Every launch after the first takes this.
            f"if {all_present}; then exit 0; fi",
            _pixi_bootstrap(),
            installs,
            profile_lines,
            'exit "$failed"',
        ]
    )


def _pixi_bootstrap() -> str:
    """Install pixi if the image has none, since every tool here comes from it.

    An arbitrary repo's container is not required to carry pixi, and without it
    the guarantee this module makes would hold only for images that happen to
    have it. Failure is left to the install steps to report: they will fail for
    a reason the log can name.
    """
    return "\n".join(
        [
            "if ! command -v pixi >/dev/null 2>&1; then",
            '  echo "devlaunch: installing pixi"',
            "  curl -fsSL https://pixi.sh/install.sh | bash >/dev/null 2>&1 || true",
            '  export PATH="$HOME/.pixi/bin:$PATH"',
            "fi",
        ]
    )


def ensure_tools(workspace: str, runner, tools: Sequence[Tool] = REQUIRED_TOOLS) -> bool:
    """Make `tools` available in `workspace`. Returns whether they now are.

    `runner` is dl.run_devpod, passed in rather than imported to keep this
    module off dl's import cycle and testable without a devpod.

    The payload goes through `bash -lc` for the same reason workspace_ssh wraps
    its own: devpod runs --command under a shell that sources no profile, so
    PATH would be missing the pixi directory this module itself installs into.
    """
    if provisioning_disabled():
        logging.debug("%s is set; not installing tools into %s", DISABLE_VAR, workspace)
        return False

    script = provision_script(tools)
    command = f"bash -lc {shlex.quote(script)}"
    try:
        result = runner(["ssh", workspace, "--command", command], capture=True)
    except OSError as e:
        logging.debug("Could not install tools into %s: %s", workspace, e)
        return False

    if result.returncode != 0:
        # Named, not raised: the workspace is up and the user asked for a
        # session, not for an install.
        logging.warning(
            "Could not install %s into %s; the session will start without them.",
            " and ".join(tool.command for tool in tools),
            workspace,
        )
        logging.debug("tool install output: %s", (result.stdout or "") + (result.stderr or ""))
        return False
    return True
