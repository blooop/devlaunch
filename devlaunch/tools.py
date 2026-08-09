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

What "lacks them" means is the probe's job, and it has three answers rather
than two, because a `claude` on PATH is not necessarily a `claude` worth
keeping: the shim this repo's own devcontainer feature bakes satisfies
`command -v` while still owing the GCS download on first use. So the probe
answers `provisioned` only for the official install layout, `lendable` for a
container whose claude is something else, and `absent` when a tool is really
missing -- and a lendable container is quietly upgraded to the host's real
binary the next time it goes through `up`. The container reports what it
found and the host says what that means, so "the official install layout" is
one definition asked from both ends rather than one per side.

The round trips this costs, by path: a provisioned workspace pays one (the
probe, which was always paid); a lendable or cold one pays two (probe, then
transfer), or three when the transfer cannot help a genuinely empty container
and the network fallback runs -- against a `devpod up` that already ran for
seconds to minutes.

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

import enum
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

# Where the official claude installer keeps one binary per version, relative to
# a home directory. The host reads it to decide what it may lend
# (`_claude_source`), writes into it when it lends (`transfer_script`), and the
# container is asked about it to decide whether it needs anything
# (`probe_script`). The *relation* that makes a path the official layout lives
# in `_is_official_claude`, for the same reason this string lives here: two
# copies of a definition are two probes with different opinions.
CLAUDE_VERSIONS_RELPATH = ".local/share/claude/versions"

# What every line of a probe's report starts with, so the report survives being
# printed into the same stdout as a container login profile's banner.
PROBE_MARK = "devlaunch-probe"

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

# What devlaunch writes above every PATH line it appends to a container's login
# profile, and the only thing its "have I already done this?" guard looks for.
# The mark exists so the guard can ask about devlaunch's own work instead of
# about a directory name. Asking about the directory made the answer depend on
# a file devlaunch does not own: Ubuntu's stock ~/.profile prepends
# ~/.local/bin itself, so on this repo's own base image
# (mcr.microsoft.com/devcontainers/base:ubuntu-24.04) the transfer's guard read
# the image's block as its own, skipped the prepend, and left the claude shim
# in front of the binary just lent -- which made every `devpod up` re-pay a
# multi-hundred-megabyte transfer for the life of the workspace. The same
# lesson devpod_provider learned from grepping devpod's rendered table: a guard
# that reads someone else's artifact is only correct until they change it.
PROFILE_MARK = "# devlaunch:"


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


def _all_present(tools: Sequence[Tool]) -> str:
    """The bash test for "every one of these already answers on PATH"."""
    return " && ".join(f"command -v {shlex.quote(tool.command)} >/dev/null 2>&1" for tool in tools)


def _is_official_claude(versions_dir: str, claude_binary: str) -> bool:
    """Whether a resolved `claude` is a binary of the official install.

    The one definition of "the official layout", and both sides of the pipe
    ask it: the host asks it of its own filesystem to decide what it may lend,
    and the container's probe reports its two resolved paths so it can be asked
    of those here. Written down once because a container-side copy of this
    relation and a host-side copy are two probes with different opinions --
    which is how a downloader parked at `versions/latest/bin/claude` came to be
    trusted by one while the other refused to lend the very same tree.

    A *direct child*, because that is what the installer creates: one binary
    per version, named for the version. Anything deeper is somebody else's
    tree that merely starts with the official path, and a downloader is free
    to choose such a path.

    Both arguments must arrive resolved -- `readlink -f` in the container,
    `Path.resolve` on the host -- because either home may be reached through a
    symlink, and comparing a resolved binary against an unresolved home makes
    a genuine install look like a shim forever. Anything that is not an
    absolute path (the empty answer a container with no `readlink` gives, a
    truncated line) is not the official layout: of the two ways to be wrong,
    lending again costs seconds and trusting a shim ships a container that
    still owes the download.
    """
    if not versions_dir.startswith("/") or not claude_binary.startswith("/"):
        return False
    return pathlib.PurePosixPath(claude_binary).parent == pathlib.PurePosixPath(versions_dir)


def _profile_prepend(tag: str, line: str, on_failure: str = "") -> str:
    """One PATH line appended to `$PROFILE` at most once, ever.

    The line is written under a `PROFILE_MARK` comment naming `tag`, and the
    guard is an exact-line match on that comment -- so what decides whether the
    edit has already been made is a line only these scripts ever write.
    Substring-matching the directory being added cannot do that job: a base
    image is free to mention, or even prepend, the same directory for its own
    reasons, and then the guard skips an append the workspace still needs.

    Exact-line (`-x`) and fixed-string (`-F`) rather than a pattern, so nothing
    in the mark is read as a regex and a longer line that merely contains it
    does not count as a match.

    Appending under a *new* mark to a profile some older devlaunch already
    edited duplicates one PATH entry, once: harmless (a directory twice on PATH
    resolves the same), self-limiting (the mark is there from then on), and the
    price of the guard no longer being able to lie.
    """
    mark = f"{PROFILE_MARK} {tag}"
    tail = f" || {on_failure}" if on_failure else ""
    return (
        f'grep -qxF {shlex.quote(mark)} "$PROFILE" 2>/dev/null || '
        f"printf '%s\\n' {shlex.quote(mark)} {shlex.quote(line)} >> \"$PROFILE\"{tail}"
    )


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
    all_present = _all_present(tools)
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
            _profile_prepend(
                "pixi-bin", 'export PATH="$HOME/.pixi/bin:$PATH"', on_failure="failed=1"
            ),
            _profile_prepend(
                "claude-shim-bin",
                '[ -d "$HOME/.pixi/envs/claude-shim/bin" ] && '
                'export PATH="$HOME/.pixi/envs/claude-shim/bin:$PATH"',
                on_failure="failed=1",
            ),
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


class ProbeResult(enum.Enum):
    """What one probe found in a workspace -- the whole answer, in one value.

    Three states rather than a boolean, because "there is a claude" and "there
    is a claude worth keeping" are different questions, and collapsing them is
    the bug this replaced: a container carrying the shim answered yes to the
    first and was left owing the ~285MB download the lending exists to avoid.
    A boolean plus a flag would have been the same two questions with a fourth,
    meaningless combination available; three named states have no illegal one.

    Which state a report means is decided here and nowhere else. The container
    reports what it found and never names a state, so there is no container-
    side copy of "the official layout" to drift away from the host's.
    """

    PROVISIONED = "provisioned"
    LENDABLE = "lendable"
    ABSENT = "absent"

    @classmethod
    def parse(cls, report: str) -> "ProbeResult":
        """Read a probe's report. Total: anything unreadable is ABSENT.

        The report is marked `key value` lines rather than one token because a
        token would mean the container had already decided -- with its own copy
        of the relation `_is_official_claude` states once, for both sides.
        What crosses the pipe is therefore two resolved paths only the
        container can know, and what they mean is settled here.

        Marked lines, and read from anywhere in the output, because the probe
        runs under `bash -lc`: the container's login profile is sourced first,
        so an image whose profile prints a banner puts it on the same stdout.

        A garbled, empty or truncated report has to mean something, and ABSENT
        is the only state whose worst case is harmless -- provisioning is
        idempotent, so reading it wrongly costs a redundant round trip, where
        a wrong PROVISIONED would silently skip the work the probe exists to
        schedule.
        """
        found = {}
        for line in report.splitlines():
            mark, _, rest = line.strip().partition(" ")
            if mark != PROBE_MARK:
                continue
            key, _, value = rest.partition(" ")
            found[key] = value.strip()
        if found.get("tools") != "present":
            return cls.ABSENT
        if _is_official_claude(found.get("versions", ""), found.get("claude", "")):
            return cls.PROVISIONED
        return cls.LENDABLE


def probe_script() -> str:
    """The shell script that reports what a workspace already has.

    It reports; it does not decide. Whether a container counts as provisioned
    turns on two facts only the container can know -- where its `claude`
    resolves to, and where the official versions directory in its home
    resolves to -- so those are what it prints, and `ProbeResult.parse` says
    what they mean. That split is the point: the relation between the two
    paths is stated once, in `_is_official_claude`, instead of once per
    language, which is what let a shim be trusted by one side and refused by
    the other.

    Exits 0 in every state: "this container has nothing" is an answer, not a
    failure, and a probe that exits non-zero paints a red devpod
    `fatal ... Process exited with status 1` on the terminal of every cold
    launch, describing the probe working exactly as intended. Nothing here can
    fail -- `$HOME` is expanded defensively because an image that never set it
    would otherwise abort the script under `set -u`, and a path that will not
    resolve is reported empty, which reads as `lendable`.

    It asks about the fixed pair this module lends rather than an arbitrary
    tool set, for the same reason `host_payload` does: what can be lent is
    `gh` and `claude`, so those are what there is a three-state answer for.
    """
    return "\n".join(
        [
            "set -u",
            f"if ! {{ {_all_present(REQUIRED_TOOLS)} ; }}; then",
            f'  echo "{PROBE_MARK} tools missing"',
            "  exit 0",
            "fi",
            f'echo "{PROBE_MARK} tools present"',
            # Both paths fully resolved, because the comparison they are for is
            # an equality: a home reached through a symlink resolves one side
            # and not the other, and a real install then reads `lendable` on
            # every launch forever.
            f'echo "{PROBE_MARK} versions '
            f'$(readlink -f "${{HOME-}}/{CLAUDE_VERSIONS_RELPATH}" 2>/dev/null || true)"',
            # Resolved, never run. The shim on PATH answers `command -v` just
            # as the real binary does, and telling them apart by asking either
            # one for its version would trigger, on the shim, the very ~285MB
            # download this answer exists to avoid. Where the name resolves to
            # is the whole test. `readlink` failing (or missing) leaves this
            # empty, which reads as `lendable` -- the answer that does more
            # work, never the one that skips it.
            f'echo "{PROBE_MARK} claude $(readlink -f "$(command -v claude)" 2>/dev/null || true)"',
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
    a wrapper script, a downloader that parked itself deeper inside that same
    directory -- is exactly the kind of downloader this transfer exists to
    skip, so only the official layout counts, and what counts as the official
    layout is `_is_official_claude`: the same relation the container's report
    is read through, so the two sides cannot come to different answers about
    one tree.
    """
    home = pathlib.Path.home()
    link = home / ".local/bin/claude"
    try:
        target = link.resolve(strict=True)
    except OSError:
        return None
    versions_dir = (home / CLAUDE_VERSIONS_RELPATH).resolve()
    if not _is_official_claude(str(versions_dir), str(target)):
        return None
    if not os.access(target, os.X_OK):
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
            (claude_binary, f"{CLAUDE_VERSIONS_RELPATH}/{version}"),
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
    claude_rel = f"{CLAUDE_VERSIONS_RELPATH}/{version}"
    # Prepended, and appended *last*, because winning is the whole point: the
    # container this lend exists for already has a shim earlier on PATH, put
    # there by a line further up this same file. The lent binary only becomes
    # the `claude` a session finds -- and the next probe only reads
    # `provisioned` -- because ~/.local/bin goes in front of it.
    profile_lines = "\n".join(
        [
            _PROFILE_RESOLUTION,
            _profile_prepend("local-bin", 'export PATH="$HOME/.local/bin:$PATH"'),
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
            f'mkdir -p "$HOME/.local/bin" "$HOME/{CLAUDE_VERSIONS_RELPATH}"',
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


def _probe(workspace: str, runner) -> ProbeResult:
    """One round trip: what does this workspace still need?

    Captured, unlike the two trips that may follow -- here the output *is* the
    answer the caller branches on, rather than progress a user needs to watch.

    A trip that fails is not an answer: the script exits 0 in all three states,
    so a non-zero status means the ssh itself did not get through, and the
    reading that costs a redundant trip is preferred to the one that skips the
    work.
    """
    result = runner(
        ["ssh", workspace, "--command", f"bash -lc {shlex.quote(probe_script())}"], capture=True
    )
    if result.returncode != 0:
        return ProbeResult.ABSENT
    return ProbeResult.parse(result.stdout)


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


def ensure_tools(workspace: str, runner) -> bool:
    """Make REQUIRED_TOOLS available in `workspace`. Returns whether they now are.

    Not parametric over a tool set, because the probe is not: it asks about the
    fixed pair this module can lend, so a caller-supplied set would be probed
    for one thing and installed for another.

    `runner` is dl.run_devpod, passed in rather than imported to keep this
    module off dl's import cycle and testable without a devpod.

    Three trips at most, each earning the next: a probe (the only trip a
    provisioned workspace ever pays), then the host lending its own binaries
    (see the module docstring for why that is the fast path), then the network
    install for a host with nothing to lend or a container the lent binaries
    cannot run in. Which of them run is decided by the probe's three-state
    answer, and only a genuinely `absent` container ever reaches the third.

    Version drift is deliberately not handled: a real claude already in the
    container is left alone whatever its version. Keeping versions in sync
    would make this a package manager, which the module docstring's "the host
    first, the network second" is explicitly not.

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
        found = _probe(workspace, runner)
        if found is ProbeResult.PROVISIONED:
            return True

        payload = host_payload()
        if payload is not None and _transfer(workspace, runner, payload):
            return True

        if found is ProbeResult.LENDABLE:
            # Both tools already answer here; only the claude was the wrong
            # one, and the lend that would have replaced it either had nothing
            # to send or would not run in this container. The network fallback
            # cannot help either way: it decides what to install with its own
            # `command -v` guards (see provision_script), which both tools
            # already satisfy, so the third trip would install nothing. Stop
            # with what is there.
            #
            # Accepted residual: a container that keeps rejecting the lend --
            # a different libc or architecture -- while carrying a shim
            # re-attempts one failing transfer on every `devpod up`, paying for
            # the tar and the stream before the arch/libc gate refuses the
            # binaries. What that costs end to end has not been measured; the
            # figure this comment used to carry was inherited, not taken. The
            # one part of it that has been measured is staging the tar (#158:
            # ~0.13s warm, ~2.6s cold), which is a fraction of it. Breaking the
            # loop needs per-container retry state persisted somewhere, which is
            # more machinery than the case is worth, and ensure_tools never runs
            # on the fast-attach path: this only ever rides an `up` that already
            # took seconds to minutes.
            return True

        # ProbeResult.ABSENT: the cold flow, with a tool genuinely missing.
        script = provision_script()
        result = runner(["ssh", workspace, "--command", f"bash -lc {shlex.quote(script)}"])
    except OSError as e:
        logging.debug("Could not install tools into %s: %s", workspace, e)
        return False

    if result.returncode != 0:
        # Named, not raised: the workspace is up and the user asked for a
        # session, not for an install.
        logging.warning(
            "Could not install %s into %s; the session will start without them.",
            " and ".join(tool.command for tool in REQUIRED_TOOLS),
            workspace,
        )
        return False
    return True
