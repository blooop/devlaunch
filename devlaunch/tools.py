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

Where the tools come from is a cost question, and the answer is **the host
first, the network second**. The machine running `dl` almost always has both
tools already -- `claude` as the official install (`~/.local/bin/claude`, a
single ~300MB binary the shim would otherwise re-download from GCS inside
every fresh container) and `gh` as a single static binary -- and the container
sits on the same disk, one pipe away. So a container that lacks them is lent
the host's own copies through a tar stream over the ssh channel dl already
holds, which turns the slowest part of a cold launch (minutes of in-container
downloads) into a local copy. Only when the host has nothing to lend, or the
lent binaries do not run there (a different arch or libc), does the old
network path run: bootstrap pixi, `pixi global install` each tool.

The round trips this costs, by path: a warm workspace pays one (the probe,
which was always paid); a cold one pays two (probe, then transfer), or three
when the transfer cannot help and the network fallback runs -- against a
`devpod up` that already ran for seconds to minutes.

Two consequences worth knowing:

- A workspace that is already running when `dl` reaches it skips `up` entirely
  (the fast-attach path), so it is not topped up. That covers workspaces started
  by something other than `dl`, and ones created before this existed; both get
  the tools on their next `dl <ws> restart` or `up`.
- Provisioning is a convenience, so a failed install costs the workspace its
  tools and not its launch: an install that fails is logged and the session
  starts anyway. The exception is a devpod that has gone missing between `up`
  and here, which dl treats as fatal everywhere else and which this does not
  make an exception of.
"""

import json
import logging
import os
import pathlib
import shlex
import shutil
import tarfile
import tempfile
from dataclasses import dataclass
from typing import List, Optional, Sequence, Tuple

# Set this to opt a machine out of installing tools into workspaces.
DISABLE_VAR = "DEVLAUNCH_NO_TOOLS"

_FALSEY = ("", "0", "false", "no")

# The claude package lives in a personal channel rather than conda-forge.
BLOOOP_CHANNEL = "https://prefix.dev/blooop"

# Which file a bash login shell will actually read. bash tries ~/.bash_profile,
# ~/.bash_login and ~/.profile in that order and sources only the first that
# exists, so appending to ~/.profile in an image that ships a ~/.bash_profile
# writes to a file nothing reads.
_PROFILE_RESOLUTION = "\n".join(
    [
        'if [ -f "$HOME/.bash_profile" ]; then PROFILE="$HOME/.bash_profile"',
        'elif [ -f "$HOME/.bash_login" ]; then PROFILE="$HOME/.bash_login"',
        'else PROFILE="$HOME/.profile"',
        "fi",
    ]
)


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
            # bash reads exactly one of these on login, in this order, and
            # stops at the first that exists -- so an image shipping a
            # ~/.bash_profile means ~/.profile is never sourced at all. Writing
            # to the wrong one leaves the tools installed and unreachable, and
            # (since the check above is `command -v`) reinstalled from scratch
            # on every single launch.
            _PROFILE_RESOLUTION,
            'grep -q "\\.pixi/bin" "$PROFILE" 2>/dev/null || '
            'echo \'export PATH="$HOME/.pixi/bin:$PATH"\' >> "$PROFILE" || failed=1',
            'grep -q "pixi/envs/claude-shim" "$PROFILE" 2>/dev/null || '
            'echo \'[ -d "$HOME/.pixi/envs/claude-shim/bin" ] && '
            'export PATH="$HOME/.pixi/envs/claude-shim/bin:$PATH"\' >> "$PROFILE" || failed=1',
        ]
    )
    return "\n".join(
        [
            "set -u",
            # Everything this script prints is progress, and progress is not
            # the answer to anything: `dl <ws> -- cmd > file` on a workspace
            # that needs provisioning must put the command's output in the
            # file and nothing else. pixi writes to stdout too, so redirect
            # once here rather than per line.
            "exec >&2",
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


@dataclass(frozen=True)
class HostPayload:
    """The host's own tool binaries, ready to lend to a container.

    `members` maps host files to where they land under the container's $HOME
    -- tar arcnames, so the same payload works whatever the container's
    username is. `claude_version` is carried because the transfer script has
    to create the `~/.local/bin/claude` symlink itself: the host's symlink
    points through the host's absolute home and would dangle anywhere else.
    """

    claude_version: str
    members: Tuple[Tuple[pathlib.Path, str], ...]


def _claude_source() -> Optional[Tuple[str, pathlib.Path]]:
    """The host's official claude install: (version, binary), or None.

    The official installer keeps one binary per version under
    `~/.local/share/claude/versions/` and points `~/.local/bin/claude` at the
    current one. Anything else on PATH answering to `claude` -- the pixi shim,
    a wrapper script -- is exactly the kind of downloader this transfer exists
    to skip, so only the official layout counts.
    """
    link = pathlib.Path.home() / ".local/bin/claude"
    try:
        target = link.resolve(strict=True)
    except OSError:
        return None
    versions_dir = (pathlib.Path.home() / ".local/share/claude/versions").resolve()
    if target.parent != versions_dir or not os.access(target, os.X_OK):
        return None
    return target.name, target


def _gh_source() -> Optional[pathlib.Path]:
    """The host's real gh binary, or None.

    `which` can answer with a pixi trampoline -- a small launcher that re-execs
    the env's binary named in a JSON file beside it -- and copying the
    trampoline without its configuration copies nothing that runs. When the
    sidecar is there, the answer is the binary it names; a sidecar that cannot
    be read makes the whole source None rather than shipping a launcher that
    will fail inside the container.
    """
    found = shutil.which("gh")
    if not found:
        return None
    path = pathlib.Path(found)
    sidecar = path.parent / "trampoline_configuration" / "gh.json"
    if sidecar.is_file():
        try:
            path = pathlib.Path(json.loads(sidecar.read_text(encoding="utf-8"))["exe"])
        except (OSError, ValueError, KeyError):
            return None
    if path.is_file() and os.access(path, os.X_OK):
        return path
    return None


def host_payload() -> Optional[HostPayload]:
    """What this host can lend a fresh container, or None.

    All or nothing: a host missing either tool falls back to the network path
    for both, rather than growing a per-tool matrix of half-lent states that
    the fallback script would then have to reason about.
    """
    claude = _claude_source()
    gh = _gh_source()
    if not claude or not gh:
        return None
    version, claude_binary = claude
    return HostPayload(
        claude_version=version,
        members=(
            (claude_binary, f".local/share/claude/versions/{version}"),
            (gh, ".local/bin/gh"),
        ),
    )


def transfer_script(payload: HostPayload) -> str:
    """The shell script that receives the tar stream and wires the tools up.

    Plain `bash -c` with explicit paths, not `-lc`: nothing here depends on a
    profile, and the profile is being edited by this very script. The two
    version checks at the end are the arch/libc gate -- a binary lent to a
    container that cannot run it fails here, the trip reports failure, and
    the caller falls back to the network install.
    """
    version = payload.claude_version
    claude_rel = f".local/share/claude/versions/{version}"
    profile_lines = "\n".join(
        [
            _PROFILE_RESOLUTION,
            'grep -q "\\.local/bin" "$PROFILE" 2>/dev/null || '
            'echo \'export PATH="$HOME/.local/bin:$PATH"\' >> "$PROFILE"',
        ]
    )
    return "\n".join(
        [
            "set -eu",
            # Progress belongs on stderr for the same reason provision_script
            # sends it there: stdout may be a `dl <ws> -- cmd > file`.
            "exec >&2",
            f'echo "devlaunch: lending claude {version} and gh from the host"',
            # Everything lands in a staging directory first, and the container
            # is only changed once the lent binaries have proved they run in
            # it. Unpacking straight into $HOME cost more than a failed
            # transfer should: the PATH edit and the `claude` symlink survived
            # a failing gate, and the network fallback that follows decides
            # what to install with `command -v` -- which a broken binary
            # satisfies. So a container that could not run the lent claude
            # ended up with a permanently broken one, the fallback installing
            # nothing, and every later probe reporting success.
            'STAGE="$HOME/.devlaunch-lend"',
            "trap 'rm -rf \"$STAGE\"' EXIT",
            'rm -rf "$STAGE"',
            'mkdir -p "$STAGE"',
            'tar xf - -C "$STAGE"',
            # The gate: prove the lent binaries actually run here, while
            # nothing outside the staging directory has been touched.
            f'"$STAGE/{claude_rel}" --version >/dev/null',
            '"$STAGE/.local/bin/gh" --version >/dev/null',
            # Proven. Now they can be moved into place.
            'mkdir -p "$HOME/.local/bin" "$HOME/.local/share/claude/versions"',
            f'mv -f "$STAGE/{claude_rel}" "$HOME/{claude_rel}"',
            'mv -f "$STAGE/.local/bin/gh" "$HOME/.local/bin/gh"',
            # The host's own symlink points through the host's home, so the
            # link is made here, against this container's $HOME.
            f'ln -sfn "$HOME/{claude_rel}" "$HOME/.local/bin/claude"',
            profile_lines,
        ]
    )


def _write_payload_tar(payload: HostPayload, out: pathlib.Path) -> None:
    """Write the payload as a plain (uncompressed) tar at *out*.

    Uncompressed on purpose: the stream crosses a local pipe into a container
    on the same disk, where gzip would cost seconds of CPU to save transfer
    time nobody is paying.
    """
    with tarfile.open(out, mode="w") as tar:
        for source, arcname in payload.members:
            tar.add(source, arcname=arcname)


def _probe(workspace: str, runner, tools: Sequence[Tool]) -> bool:
    """One round trip: are all `tools` already on the workspace's PATH?

    Captured, unlike the two trips that may follow. A probe has no progress to
    report -- it is a yes/no question -- and "no" is the everyday answer on a
    cold workspace, which devpod renders on the terminal as a red
    `fatal ... Process exited with status 1`. That line describes the probe
    working exactly as intended, so it is not shown.
    """
    checks = " && ".join(
        f"command -v {shlex.quote(tool.command)} >/dev/null 2>&1" for tool in tools
    )
    result = runner(
        ["ssh", workspace, "--command", f"bash -lc {shlex.quote(checks)}"], capture=True
    )
    return result.returncode == 0


def _transfer(workspace: str, runner, payload: HostPayload) -> bool:
    """Stream the host's binaries into the workspace. One round trip."""
    command = f"bash -c {shlex.quote(transfer_script(payload))}"
    # A real file rather than a pipe, so the stream stays on run_devpod --
    # dl's single devpod spawn point -- and a failed trip can be retried by
    # the fallback without a half-consumed generator in hand.
    with tempfile.TemporaryDirectory(prefix="devlaunch-tools-") as staging:
        bundle = pathlib.Path(staging) / "tools.tar"
        try:
            _write_payload_tar(payload, bundle)
        # Not just OSError: tarfile raises TarError (and ValueError for a
        # member it cannot represent), neither of which is an OSError, and
        # this runs *after* a successful `devpod up` -- so letting one out
        # would cost the user the workspace they just built over a
        # convenience that is allowed to fail.
        except (OSError, tarfile.TarError, ValueError) as e:
            logging.debug("Could not bundle host tools: %s", e)
            return False
        with open(bundle, "rb") as stream:
            result = runner(["ssh", workspace, "--command", command], stdin_file=stream)
    return result.returncode == 0


def ensure_tools(workspace: str, runner, tools: Sequence[Tool] = REQUIRED_TOOLS) -> bool:
    """Make `tools` available in `workspace`. Returns whether they now are.

    `runner` is dl.run_devpod, passed in rather than imported to keep this
    module off dl's import cycle and testable without a devpod.

    Three trips at most, each earning the next: a probe (the only trip a
    provisioned workspace ever pays), then the host lending its own binaries
    (see the module docstring for why that is the fast path), then the network
    install for a host with nothing to lend or a container the lent binaries
    cannot run in.

    The network payload goes through `bash -lc` for the same reason
    workspace_ssh wraps its own: devpod runs --command under a shell that
    sources no profile, so PATH would be missing the pixi directory this
    module itself installs into.

    Output is not captured. A cold install streams a ~300MB binary or
    downloads pixi and two packages, which with nothing on the terminal reads
    as a hung `dl`; the scripts' own progress lines are the answer to that,
    and they are worth nothing in a buffer. A workspace that needs no work
    stays silent because the probe prints nothing.

    Not every failure is swallowed: DevpodNotInstalled is deliberately not an
    OSError (see dl.DevpodNotInstalled) so that it is never mistaken for a
    failure of the thing being attempted, and it keeps that meaning here.
    """
    if provisioning_disabled():
        logging.debug("%s is set; not installing tools into %s", DISABLE_VAR, workspace)
        return False

    try:
        if _probe(workspace, runner, tools):
            return True

        payload = host_payload()
        if payload is not None and _transfer(workspace, runner, payload):
            return True

        script = provision_script(tools)
        result = runner(["ssh", workspace, "--command", f"bash -lc {shlex.quote(script)}"])
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
        return False
    return True
