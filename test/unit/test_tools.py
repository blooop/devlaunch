"""What devlaunch installs into a workspace, and what it does when that fails."""

import io
import logging
import os
import pathlib
import shlex
import shutil
import subprocess
import tarfile
from typing import List, Optional
from unittest.mock import patch

import pytest

from devlaunch import tools
from devlaunch.tools import REQUIRED_TOOLS, Tool, provision_script, provision_tools


class Runner:
    """Stands in for dl.run_devpod, recording what was asked of devpod.

    `returncodes` is consumed one per call, the last repeating -- the
    three-trip flow (probe, transfer, install) needs different answers to
    different trips, and a single number could only play one of them.
    """

    def __init__(self, returncodes=(0,), stdout: str = "", stderr: str = ""):
        self.returncodes = list(returncodes)
        self.stdout = stdout
        self.stderr = stderr
        self.calls: List[List[str]] = []
        self.captured: List[bool] = []
        # What each trip put on the command's stdin, read as devpod would.
        # None for the trips that stream nothing.
        self.streams: List[Optional[bytes]] = []

    def __call__(
        self, args, capture=False, env=None, stdin_file=None
    ) -> subprocess.CompletedProcess:
        self.calls.append(list(args))
        self.captured.append(capture)
        self.streams.append(stdin_file.read() if stdin_file is not None else None)
        returncode = self.returncodes[min(len(self.calls) - 1, len(self.returncodes) - 1)]
        return subprocess.CompletedProcess(
            args=list(args), returncode=returncode, stdout=self.stdout, stderr=self.stderr
        )

    def script(self, call: int = 0) -> str:
        """The payload of the `call`-th ssh --command that was sent."""
        return self.calls[call][self.calls[call].index("--command") + 1]


@pytest.fixture(autouse=True)
def _forwarding_enabled(monkeypatch):
    """The opt-out must not leak in from the machine running the tests."""
    monkeypatch.delenv(tools.DISABLE_VAR, raising=False)


@pytest.fixture
def no_host_payload():
    """This host must not lend its own binaries to a test of the flow.

    provision_tools asks the real filesystem what the machine could lend; on a
    developer machine that is a real claude and gh, on CI nothing, and a test
    of "what happens with nothing to lend" cannot depend on which. The
    resolvers themselves are tested against scratch homes in TestHostPayload,
    which is why this is scoped rather than autouse.
    """
    with patch.object(tools, "host_payload", return_value=None):
        yield


class TestRequiredTools:
    """The set is the point of the module, so it is pinned."""

    def test_gh_and_claude_are_both_required(self):
        assert {tool.command for tool in REQUIRED_TOOLS} == {"gh", "claude"}

    def test_claude_comes_from_the_shim_package(self):
        """`claude` is not a package name; installing `claude` would fail."""
        claude = next(t for t in REQUIRED_TOOLS if t.command == "claude")
        assert claude.package == "claude-shim"
        assert claude.install_args == ["--channel", tools.BLOOOP_CHANNEL, "claude-shim"]

    def test_a_channelless_tool_installs_by_bare_name(self):
        assert Tool(command="gh", package="gh").install_args == ["gh"]


class TestProvisionScript:
    """The script runs in someone else's container, so it is checked closely."""

    def test_it_does_nothing_when_every_tool_is_present(self):
        """The common path: the check has to come before any install."""
        script = provision_script()
        exit_early = script.index("exit 0")
        assert exit_early < script.index("pixi global install")
        assert "command -v gh" in script
        assert "command -v claude" in script

    def test_the_early_exit_requires_all_tools_not_any(self):
        """`&&`, not `||` -- one tool present is not the guarantee."""
        script = provision_script()
        guard = next(line for line in script.splitlines() if line.startswith("if command -v"))
        assert "&&" in guard
        assert "||" not in guard

    def test_each_tool_is_installed_only_when_missing(self):
        script = provision_script()
        for tool in REQUIRED_TOOLS:
            assert f"if ! command -v {tool.command} >/dev/null 2>&1; then" in script

    def test_a_failed_install_is_reported_through_the_exit_status(self):
        """A tool that would not install must not look like a success."""
        script = provision_script()
        assert "failed=1" in script
        assert 'exit "$failed"' in script

    def test_it_puts_the_pixi_bin_directory_on_the_login_path(self):
        """Without this the next launch reinstalls everything, forever."""
        script = provision_script()
        assert ".pixi/bin" in script
        assert ".profile" in script

    def test_it_writes_to_the_profile_bash_actually_reads(self):
        """bash sources the FIRST of bash_profile/bash_login/profile that exists.

        An image shipping a ~/.bash_profile therefore never reads ~/.profile,
        so writing there leaves the tools installed and unreachable -- and,
        because the presence check is `command -v`, reinstalled on every launch.
        """
        script = provision_script()
        assert '[ -f "$HOME/.bash_profile" ]' in script
        assert '[ -f "$HOME/.bash_login" ]' in script
        # The order is the whole point: bash_profile wins over profile.
        assert script.index(".bash_profile") < script.index(".bash_login")
        assert script.index(".bash_login") < script.index('PROFILE="$HOME/.profile"')

    def test_a_profile_that_cannot_be_written_is_a_failure(self):
        """Installed but not on PATH is not the guarantee this module makes."""
        script = provision_script()
        profile_writes = [line for line in script.splitlines() if '>> "$PROFILE"' in line]
        assert all(line.endswith("|| failed=1") for line in profile_writes)

    def test_pixi_is_installed_when_the_image_has_none(self):
        """An arbitrary repo's container need not carry pixi."""
        assert "command -v pixi" in provision_script()

    def test_progress_goes_to_stderr_not_stdout(self):
        """`dl <ws> -- cmd > file` must not get install chatter in the file.

        The provisioning ssh is a separate devpod call from the command's, but
        it shares dl's stdout, so an uncaptured first launch would write into
        whatever the caller redirected.
        """
        script = provision_script()
        assert "exec >&2" in script
        assert script.index("exec >&2") < script.index("echo")

    def test_a_custom_tool_set_is_honoured(self):
        script = provision_script([Tool(command="jq", package="jq")])
        assert "command -v jq" in script
        assert "command -v gh" not in script


# The devcontainer feature's installer edits the same file the scripts above
# do -- `$TARGET_HOME/.profile` is the *user's* login profile, not a file the
# feature owns -- so its guards are held to the same properties. It was
# excluded from the #164 sweep on the stated grounds that it guards its own
# line in its own file; that reason was wrong, which is why it is covered here
# rather than trusted.
FEATURE_INSTALLER = (
    pathlib.Path(__file__).resolve().parents[2] / ".devcontainer/claude-code/install.sh"
)


class TestProfileGuards:
    """Every question these scripts ask about the login profile, in every script.

    Two questions, both asked once here rather than per script, because each
    property is the same wherever it is asked: *which* file is the login
    profile, and *have we already edited it*. The first decides whether the
    edit is ever read, the second whether it stops being repeated.
    """

    SCRIPTS = ["provision", "transfer", "feature"]

    # The directory each script's profile edit unconditionally puts on a login
    # shell's PATH. Written out as literals rather than read back off the
    # script, because a test that derives what to expect from the thing under
    # test agrees with it however wrong it is.
    PREPENDED = {"provision": ".pixi/bin", "transfer": ".local/bin", "feature": ".pixi/bin"}

    # Every shape a container's home can have when a writer arrives. bash
    # sources the *first* of ~/.bash_profile, ~/.bash_login, ~/.profile that
    # exists and no other, so which of these files are already there is
    # exactly what decides where an edit has to land to be read.
    HOME_SHAPES = [
        (),
        (".profile",),
        (".bash_profile",),
        (".bash_login",),
        (".bash_profile", ".profile"),
        (".bash_login", ".profile"),
        (".bash_profile", ".bash_login", ".profile"),
    ]

    @staticmethod
    def _script(name, tmp_path):
        if name == "provision":
            return provision_script()
        if name == "transfer":
            return tools.transfer_script(fake_payload(tmp_path))
        return FEATURE_INSTALLER.read_text(encoding="utf-8")

    @staticmethod
    def _resolution(script):
        """The lines of `script` that decide which file `$PROFILE` names.

        Cut out by its own shape -- from the line that both names
        ~/.bash_profile and assigns `PROFILE` down to the `fi` that closes it
        -- and returned with the indentation of its surroundings taken off, so
        a block sitting inside a shell function compares against the same
        block rendered on its own.
        """
        lines = script.splitlines()
        start = next(
            i for i, line in enumerate(lines) if ".bash_profile" in line and "PROFILE=" in line
        )
        end = next(i for i in range(start, len(lines)) if lines[i].strip() == "fi")
        return "\n".join(line.strip() for line in lines[start : end + 1])

    @staticmethod
    def _profile_edit(script):
        """The part of `script` that picks a profile file and appends to it.

        Cut out of the script rather than rebuilt from its pieces so the block
        can be *run*: the resolution and the appends it feeds are lifted
        verbatim and executed, which is the only way to find out where the
        writes actually land.
        """
        appends = [line for line in script.splitlines() if '>> "$PROFILE"' in line]
        assert appends, "a script that never edits the profile has nothing to resolve"
        return "\n".join([TestProfileGuards._resolution(script)] + appends)

    @staticmethod
    def _guards(script):
        """The guard half of every line that appends to the login profile."""
        guards = [
            line.strip().partition("||")[0]
            for line in script.splitlines()
            if '>> "$PROFILE"' in line
        ]
        assert guards, "a script that never edits the profile has nothing to guard"
        return guards

    @pytest.mark.parametrize("name", SCRIPTS)
    @pytest.mark.parametrize("present", HOME_SHAPES, ids=lambda shape: "+".join(shape) or "empty")
    def test_a_profile_edit_lands_where_a_login_shell_will_read_it(self, name, present, tmp_path):
        """The edit has to go in the file bash *reads*, not the one named
        ~/.profile.

        bash sources exactly one login profile -- the first of ~/.bash_profile,
        ~/.bash_login, ~/.profile that exists -- so on an image shipping a
        ~/.bash_profile, appending to ~/.profile writes to a file nothing ever
        reads: the tools are installed and unreachable, and (since the reuse
        check is `command -v`) reinstalled from scratch on every launch. Two
        writers that answer this differently also dedupe against different
        files, so neither can see the other's marks.

        Asked of a real login bash rather than of a rule this test restates:
        the edit is run in a scratch home, then bash is started as a login
        shell there and asked what its PATH is. That makes bash the authority
        on both halves at once, and means all three writers agreeing is a
        consequence of each being right rather than a separate assertion.
        """
        home = tmp_path / "home"
        home.mkdir()
        for name_ in present:
            (home / name_).write_text("# stock\n", encoding="utf-8")
        bash = shutil.which("bash") or "/bin/bash"
        subprocess.run(
            [bash, "-c", self._profile_edit(self._script(name, tmp_path))],
            env={"PATH": os.environ["PATH"], "HOME": str(home), "TARGET_HOME": str(home)},
            check=True,
        )
        login_path = subprocess.run(
            [bash, "-l", "-c", 'printf "%s\\n" "$PATH"'],
            env={"PATH": os.environ["PATH"], "HOME": str(home)},
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        prepended = str(home / self.PREPENDED[name])
        assert prepended in login_path.strip().split(":"), (
            f"{name} appended to a file a login shell in a home with "
            f"{list(present) or 'no profile'} never reads"
        )

    @pytest.mark.parametrize("name", SCRIPTS)
    def test_a_guard_asks_about_devlaunchs_own_line_not_about_a_directory(self, name, tmp_path):
        """The guard means "have *we* already prepended our directory here", and
        the only evidence of that which devlaunch owns is the mark it writes.

        Asking instead whether the directory is mentioned anywhere in the file
        makes the answer a base image's to give. Ubuntu's stock ~/.profile
        prepends ~/.local/bin itself -- so on this repo's own base image that
        question answered "already done" about work nobody had done, the lent
        binary never reached the front of PATH, and every launch re-paid the
        transfer. A directory name may appear in what is appended; it may not
        appear in what decides whether to append.
        """
        script = self._script(name, tmp_path)
        for guard in self._guards(script):
            assert tools.PROFILE_MARK in guard
            for owned_by_the_image in (".local/bin", ".pixi/bin", "pixi/envs/claude-shim"):
                assert owned_by_the_image not in guard

    def test_every_line_lands_exactly_once_however_often_the_scripts_run(self, tmp_path):
        """Two different lines may never share a mark, and a rerun may never
        append again. Both are the same property -- the mark is what decides
        "already done" -- and both failure modes are silent: two lines under
        one mark drop whichever comes second (its guard finds the first
        line's mark and reads it as its own work), and a missed dedupe grows
        the profile on every launch. Nothing exits non-zero either way.

        Run for real rather than compared as text, and across *both* scripts
        into one profile, because that is where the lines meet: a provision
        and a lend edit the same file over a workspace's life, so a
        collision between their marks is just as fatal as one within either.
        """
        profile = tmp_path / "profile"
        appends = []
        for name in self.SCRIPTS:
            script = self._script(name, tmp_path)
            appends.extend(line.strip() for line in script.splitlines() if '>> "$PROFILE"' in line)
        assert len(appends) == 5, "a new PATH line belongs in this test's expectations"
        for _ in range(2):
            subprocess.run(
                [shutil.which("bash") or "/bin/bash", "-c", "\n".join(appends)],
                env={**os.environ, "PROFILE": str(profile)},
                check=True,
            )
        written = profile.read_text(encoding="utf-8").splitlines()
        for line in (
            'export PATH="$HOME/.pixi/bin:$PATH"',
            '[ -d "$HOME/.pixi/envs/claude-shim/bin" ] && '
            'export PATH="$HOME/.pixi/envs/claude-shim/bin:$PATH"',
            'export PATH="$HOME/.local/bin:$PATH"',
        ):
            assert written.count(line) == 1, f"{line!r} appended {written.count(line)} times"

    @pytest.mark.parametrize("name", SCRIPTS)
    def test_a_guard_matches_a_whole_line_not_a_fragment_of_one(self, name, tmp_path):
        """A mark is only devlaunch's if nothing longer can pass for it: an
        exact-line, fixed-string match, so neither a regex metacharacter nor a
        longer line that happens to contain the mark counts as a hit."""
        for guard in self._guards(self._script(name, tmp_path)):
            assert guard.startswith("grep -qxF ")

    def test_the_feature_installer_writes_the_very_fragments_devlaunch_renders(self):
        """The installer's two profile edits are `_profile_prepend`'s own
        output, verbatim -- same derived mark, same guard, same line.

        Pinned byte-for-byte rather than by shape, because the marks are
        content hashes a shell script cannot derive for itself: hardcoded,
        they drift the moment either side's line changes, and a drifted mark
        quietly costs a workspace one duplicate PATH entry per line -- the
        installer runs at image build, the provision script at `up`, and only
        matching marks let the second writer recognise the first's work.
        """
        installer = FEATURE_INSTALLER.read_text(encoding="utf-8")
        for line in (
            'export PATH="$HOME/.pixi/bin:$PATH"',
            '[ -d "$HOME/.pixi/envs/claude-shim/bin" ] && '
            'export PATH="$HOME/.pixi/envs/claude-shim/bin:$PATH"',
        ):
            fragment = tools._profile_prepend(line)  # pylint: disable=protected-access
            assert fragment in installer, (
                f"the feature installer no longer carries the rendered append for {line!r}; "
                "regenerate it with tools._profile_prepend"
            )

    @pytest.mark.parametrize("name", SCRIPTS)
    def test_every_writer_carries_the_one_resolution_the_module_renders(self, name, tmp_path):
        """There is one answer to "which file is the login profile", rendered
        in one place, and every writer carries that rendering.

        The test above proves each writer's edit reaches a login shell today.
        This one is why it stays true: two hand-maintained copies of the
        answer drift, and the drift is silent -- the writer that keeps the old
        answer still exits 0, still writes a file, and only stops being read.
        The rendering takes the home variable as an argument because the
        installer works on another user's home (`$TARGET_HOME`) while the
        scripts run in it (`$HOME`); that is the *only* difference allowed
        between them, which is what asserting equality here says.
        """
        home_var = "$TARGET_HOME" if name == "feature" else "$HOME"
        rendered = tools._profile_resolution(home_var)  # pylint: disable=protected-access
        assert self._resolution(self._script(name, tmp_path)) == rendered, (
            f"{name} no longer resolves the login profile the way the module does; "
            "regenerate it with tools._profile_resolution"
        )

    @pytest.mark.parametrize("name", SCRIPTS)
    def test_no_profile_edit_hides_from_these_tests_under_another_name(self, name, tmp_path):
        """Every append in these scripts goes to `$PROFILE`, spelled that way.

        The tests here find profile edits by matching that one spelling, so a
        writer that appended under any other name would be silently exempt
        from all of them -- unguarded, unresolved, and reported as covered.
        The filter used to also admit a lowercase `$profile` that nothing has
        ever written, which is precisely such an exemption sitting ready.
        """
        for line in self._script(name, tmp_path).splitlines():
            if ">>" in line:
                assert '>> "$PROFILE"' in line, (
                    f"{name} appends to something other than $PROFILE, which these "
                    f"tests do not see: {line.strip()!r}"
                )


def fake_payload(tmp_path) -> tools.HostPayload:
    """A payload made of scratch files, so no test ships a real 300MB binary."""
    claude = tmp_path / "claude-2.0.1"
    claude.write_bytes(b"#!/bin/sh\n")
    gh = tmp_path / "gh"
    gh.write_bytes(b"\x7fELF")
    return tools.HostPayload(
        claude_version="2.0.1",
        members=(
            (claude, ".local/share/claude/versions/2.0.1"),
            (gh, ".local/bin/gh"),
        ),
    )


# What a container in each of the three states reports back over the pipe,
# written out as the probe prints it rather than rebuilt from the module: a
# fixture composed the way the parser splits lines would agree with any format
# the probe ever drifted into. `/ws` stands in for a container's $HOME.
REPORT_ABSENT = "devlaunch-probe tools missing\n"
REPORT_PROVISIONED = (
    "devlaunch-probe tools present\n"
    "devlaunch-probe versions /ws/.local/share/claude/versions\n"
    "devlaunch-probe claude /ws/.local/share/claude/versions/2.0.1\n"
)
REPORT_LENDABLE = (
    "devlaunch-probe tools present\n"
    "devlaunch-probe versions /ws/.local/share/claude/versions\n"
    "devlaunch-probe claude /ws/.pixi/envs/claude-shim/bin/claude\n"
)

# The two outcome lines a stage of the setup pass can print, written out as the
# composer prints them for the same reason the reports above are: a fixture
# rebuilt from the module would agree with any format the pass drifted into.
# The third outcome has no line -- that is what makes it the third outcome.
STAGE_OK = "devlaunch-probe stage hostname ok\n"
STAGE_FAILED = "devlaunch-probe stage hostname failed 1\n"

# ~/.profile exactly as mcr.microsoft.com/devcontainers/base:ubuntu-24.04 ships
# it -- the base image .devcontainer/Dockerfile builds on -- copied out of the
# pulled image verbatim. The load-bearing part is the last block: Ubuntu's
# default profile puts ~/.local/bin on PATH itself, near the top of the file,
# long before anything devlaunch or the devcontainer feature appends. A guard
# that looks for the *directory* rather than for its own line reads that block
# as its own work and skips an append the workspace needs.
UBUNTU_STOCK_PROFILE = """\
# ~/.profile: executed by the command interpreter for login shells.
# This file is not read by bash(1), if ~/.bash_profile or ~/.bash_login
# exists.
# see /usr/share/doc/bash/examples/startup-files for examples.
# the files are located in the bash-doc package.

# the default umask is set in /etc/profile; for setting the umask
# for ssh logins, install and configure the libpam-umask package.
#umask 022

# if running bash
if [ -n "$BASH_VERSION" ]; then
    # include .bashrc if it exists
    if [ -f "$HOME/.bashrc" ]; then
\t. "$HOME/.bashrc"
    fi
fi

# set PATH so it includes user's private bin if it exists
if [ -d "$HOME/bin" ] ; then
    PATH="$HOME/bin:$PATH"
fi

# set PATH so it includes user's private bin if it exists
if [ -d "$HOME/.local/bin" ] ; then
    PATH="$HOME/.local/bin:$PATH"
fi
"""

# What .devcontainer/Dockerfile appends to that same file, verbatim, and in
# this order. The shim directory is prepended *last*, so it wins over
# everything above it -- including Ubuntu's own ~/.local/bin block.
DEVCONTAINER_PROFILE_LINES = """\
export PATH="$HOME/.pixi/bin:$PATH"
# Workaround: pixi trampoline fails for bash scripts, so add env bin directly
[ -d "$HOME/.pixi/envs/claude-shim/bin" ] && export PATH="$HOME/.pixi/envs/claude-shim/bin:$PATH"
"""


class TestProbeScript:
    """The probe runs in someone else's container and decides whether to lend.

    Checked as a string (what it may never do) and by running it for real
    against scratch `$HOME`s (what it answers), because both halves matter: an
    answer that is right for the wrong reason still costs a 285MB download.
    """

    @staticmethod
    def _home(tmp_path):
        home = tmp_path / "home"
        home.mkdir()
        return home

    @staticmethod
    def _bare_path(tmp_path):
        """A PATH carrying the coreutils the probe needs and nothing else.

        A test of "no claude here" must not find the developer's own claude,
        so the real system directories are kept off PATH entirely and the one
        external the probe uses is linked in by hand.
        """
        sysbin = tmp_path / "sysbin"
        sysbin.mkdir(exist_ok=True)
        for command in ("readlink",):
            found = shutil.which(command)
            assert found, f"the test host needs {command}"
            link = sysbin / command
            if not link.exists():
                link.symlink_to(found)
        return [sysbin]

    _KEEP_HOME = object()

    def _answer(self, tmp_path, home, path_dirs=(), *, home_env=_KEEP_HOME):
        """Run the probe for real and read its answer the way `dl` reads it.

        The script and the parser together, because that pair *is* the probe:
        the container reports what it found and this side says what that means,
        so a test of either half alone would not notice the two disagreeing.

        `home_env` replaces what `$HOME` is set to for the run, or removes the
        variable entirely when it is None.
        """
        env = {"PATH": ":".join(str(d) for d in [*path_dirs, *self._bare_path(tmp_path)])}
        home_value = str(home) if home_env is self._KEEP_HOME else home_env
        if home_value is not None:
            env["HOME"] = home_value
        # bash by absolute path: the stripped PATH the probe is handed cannot
        # also be the one that finds the shell running it.
        result = subprocess.run(
            [shutil.which("bash") or "/bin/bash", "-c", tools.probe_script()],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        # Exit 0 in every state: "no tools here" is an answer, not a failure,
        # and a non-zero probe paints a red devpod `fatal` on the terminal of
        # every cold launch.
        assert result.returncode == 0, result.stderr
        return tools.ProbeResult.parse(result.stdout)

    def _answer_after_login(self, tmp_path, home):
        """The same answer, but with PATH built by the home's own login profile.

        `_probe` runs the script under `bash -lc`, so in a real container every
        directory the probe searches was put there by the profile -- the base
        image's block, the devcontainer's appended lines and the transfer's
        prepend, in whatever order they ended up in the file. Handing the probe
        a PATH instead (as the other cases here do, to keep them hermetic)
        hides exactly that ordering, which is the thing a lend depends on.

        `$HOME/.profile` is sourced explicitly rather than using `bash -l`,
        because `-l` would also source the *test host's* /etc/profile and drag
        its system directories -- and whatever `claude` the developer has --
        into a run that is meant to see only this scratch home. The user
        profile is the file under test and the only one either script writes.
        """
        env = {
            "HOME": str(home),
            "PATH": ":".join(str(d) for d in self._bare_path(tmp_path)),
        }
        script = f'. "$HOME/.profile"\n{tools.probe_script()}'
        result = subprocess.run(
            [shutil.which("bash") or "/bin/bash", "-c", script],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0, result.stderr
        return tools.ProbeResult.parse(result.stdout)

    @staticmethod
    def _official_claude(home, version="2.0.1"):
        """What the official installer leaves behind: one binary per version
        under ~/.local/share/claude/versions, with ~/.local/bin/claude
        pointing at the current one."""
        binary = home / ".local/share/claude/versions" / version
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        binary.chmod(0o755)
        link = home / ".local/bin/claude"
        link.parent.mkdir(parents=True, exist_ok=True)
        link.symlink_to(binary)
        return link.parent

    @staticmethod
    def _claude_shim(home):
        """What .devcontainer/claude-code/install.sh bakes: something on PATH
        answering to `claude` that fetches the real binary on first run."""
        shim = home / ".local/bin/claude"
        shim.parent.mkdir(parents=True, exist_ok=True)
        shim.write_text("#!/bin/sh\necho 'downloading 285MB' >&2\n", encoding="utf-8")
        shim.chmod(0o755)
        return shim.parent

    @staticmethod
    def _gh(home):
        gh = home / ".local/bin/gh"
        gh.parent.mkdir(parents=True, exist_ok=True)
        gh.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        gh.chmod(0o755)
        return gh.parent

    def test_a_container_with_neither_tool_answers_absent(self, tmp_path):
        home = self._home(tmp_path)
        assert self._answer(tmp_path, home) is tools.ProbeResult.ABSENT

    def test_the_official_claude_layout_answers_provisioned(self, tmp_path):
        """The prebuilt-image jackpot: nothing to do, one round trip."""
        home = self._home(tmp_path)
        self._official_claude(home)
        assert self._answer(tmp_path, home, [self._gh(home)]) is tools.ProbeResult.PROVISIONED

    def test_a_baked_shim_answers_lendable_rather_than_provisioned(self, tmp_path):
        """The whole point of the three states. A `claude` that is a downloader
        satisfies `command -v` while still owing the ~285MB the lending exists
        to avoid, so it must not be mistaken for a provisioned workspace."""
        home = self._home(tmp_path)
        self._claude_shim(home)
        assert self._answer(tmp_path, home, [self._gh(home)]) is tools.ProbeResult.LENDABLE

    def test_a_real_claude_with_no_gh_is_absent_not_lendable(self, tmp_path):
        """`lendable` means "replace the claude"; a container missing gh
        outright needs the cold flow, which the network fallback can finish."""
        home = self._home(tmp_path)
        self._official_claude(home)
        assert self._answer(tmp_path, home, [home / ".local/bin"]) is tools.ProbeResult.ABSENT

    @staticmethod
    def _lend(tmp_path, home, version="2.0.1"):
        """Run the real transfer script against a scratch $HOME, for real.

        A host payload is built out of scratch binaries, tarred the way
        `_transfer` tars it, and streamed into `transfer_script` on stdin --
        so what lands in `home`, and what the script writes into that home's
        login profile, is what a real lend leaves behind.
        """
        source = tmp_path / f"host-{version}"
        source.mkdir(exist_ok=True)
        binaries = {}
        for name in (f"claude-{version}", "gh"):
            binary = source / name
            binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            binary.chmod(0o755)
            binaries[name] = binary
        payload = tools.HostPayload(
            claude_version=version,
            members=(
                (binaries[f"claude-{version}"], f".local/share/claude/versions/{version}"),
                (binaries["gh"], ".local/bin/gh"),
            ),
        )
        bundle = tmp_path / f"tools-{version}.tar"
        with tarfile.open(bundle, mode="w") as tar:
            for member_source, arcname in payload.members:
                tar.add(member_source, arcname=arcname)
        with open(bundle, "rb") as stream:
            lend = subprocess.run(
                [shutil.which("bash") or "/bin/bash", "-c", tools.transfer_script(payload)],
                stdin=stream,
                env={**os.environ, "HOME": str(home)},
                capture_output=True,
                text=True,
                check=False,
            )
        assert lend.returncode == 0, lend.stderr

    def test_the_layout_a_lend_leaves_behind_answers_provisioned(self, tmp_path):
        """Convergence, run for real end to end: the transfer script unpacks
        into a scratch $HOME, and the probe is then asked about that same home.

        The two scripts have to agree or the lending never terminates -- a lend
        the next probe does not recognise means every `up` for the rest of that
        workspace's life re-pays the transfer.
        """
        home = self._home(tmp_path)
        self._lend(tmp_path, home)
        assert self._answer(tmp_path, home, [home / ".local/bin"]) is tools.ProbeResult.PROVISIONED

    def test_a_lend_converges_on_the_image_this_repo_ships(self, tmp_path):
        """Convergence where it actually has to hold: a shim container whose
        login profile is Ubuntu's stock one plus the lines this repo's own
        devcontainer appends, with PATH decided by *sourcing that profile*
        rather than handed to the probe by the test.

        PATH order is the whole mechanism by which a lend takes effect -- the
        lent binary only wins because the transfer prepends `~/.local/bin` to
        the profile -- so a convergence test that builds PATH itself asserts a
        world in which the profile edit cannot be wrong. This one lets the
        profile decide, which is the only way the edit being skipped is
        visible: skipped, the shim stays in front, every `up` re-pays the tar,
        and the workspace never converges.
        """
        home = self._home(tmp_path)
        (home / ".profile").write_text(
            UBUNTU_STOCK_PROFILE + DEVCONTAINER_PROFILE_LINES, encoding="utf-8"
        )
        shim = home / ".pixi/envs/claude-shim/bin/claude"
        shim.parent.mkdir(parents=True)
        shim.write_text("#!/bin/sh\necho 'downloading 285MB' >&2\n", encoding="utf-8")
        shim.chmod(0o755)
        gh = home / ".pixi/bin/gh"
        gh.parent.mkdir(parents=True)
        gh.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        gh.chmod(0o755)

        assert self._answer_after_login(tmp_path, home) is tools.ProbeResult.LENDABLE
        self._lend(tmp_path, home)
        assert self._answer_after_login(tmp_path, home) is tools.ProbeResult.PROVISIONED
        # And the second lend never happens: converged means converged, not
        # converged-then-drifting-back.
        assert self._answer_after_login(tmp_path, home) is tools.ProbeResult.PROVISIONED

    @staticmethod
    def _nested_shim(home):
        """A downloader that parked itself *inside* the versions directory.

        The official installer puts one binary per version directly in that
        directory; anything deeper is somebody else's tree, and a downloader
        is free to choose a path that merely starts with the official one.
        """
        shim = home / ".local/share/claude/versions/latest/bin/claude"
        shim.parent.mkdir(parents=True, exist_ok=True)
        shim.write_text("#!/bin/sh\necho 'downloading 285MB' >&2\n", encoding="utf-8")
        shim.chmod(0o755)
        link = home / ".local/bin/claude"
        link.parent.mkdir(parents=True, exist_ok=True)
        link.symlink_to(shim)
        return link.parent

    def test_a_shim_hiding_inside_the_versions_directory_is_lendable(self, tmp_path):
        """`under the versions directory` is not the official layout -- being a
        direct child of it is. A downloader is free to park itself at
        `versions/latest/bin/claude`, and a probe that accepts any depth trusts
        it and leaves the container owing the download the lend exists to
        remove -- the exact illegal state the three-state probe replaced."""
        home = self._home(tmp_path)
        self._nested_shim(home)
        assert self._answer(tmp_path, home, [self._gh(home)]) is tools.ProbeResult.LENDABLE

    @pytest.mark.parametrize("layout", ["_official_claude", "_claude_shim", "_nested_shim"])
    def test_neither_side_of_the_pipe_can_disagree_about_one_tree(
        self, tmp_path, monkeypatch, layout
    ):
        """One definition of "the official install", asked from both ends.

        The container decides whether to keep what it has; the host decides
        what it may lend. Those are the same question about the same layout,
        and an answer of `provisioned` for a tree the host would refuse to lend
        is a shim nothing will ever replace -- so the two are pinned together
        rather than separately.
        """
        home = self._home(tmp_path)
        getattr(self, layout)(home)
        self._gh(home)
        container = self._answer(tmp_path, home, [home / ".local/bin"])
        monkeypatch.setattr(tools.pathlib.Path, "home", staticmethod(lambda: home))
        host = tools._claude_source()  # pylint: disable=protected-access
        assert (container is tools.ProbeResult.PROVISIONED) is (host is not None)

    def test_a_real_install_reached_through_a_symlinked_home_is_provisioned(self, tmp_path):
        """Images reach `$HOME` through a symlink often enough to matter, and
        resolving the claude while comparing against an unresolved `$HOME`
        makes a genuine install read `lendable` forever: the lend succeeds,
        changes nothing the next probe recognises, and is re-paid on every
        single `up` for the life of the workspace."""
        real = tmp_path / "real-home"
        real.mkdir()
        self._official_claude(real)
        self._gh(real)
        home = tmp_path / "home-link"
        home.symlink_to(real)
        answer = self._answer(tmp_path, home, [home / ".local/bin"])
        assert answer is tools.ProbeResult.PROVISIONED

    def test_a_trailing_slash_on_home_does_not_hide_the_official_install(self, tmp_path):
        """A `$HOME` written with a trailing slash is a legal value of the
        variable and names the same directory, so it must not turn a real
        install into a lend."""
        home = self._home(tmp_path)
        self._official_claude(home)
        self._gh(home)
        answer = self._answer(tmp_path, home, [home / ".local/bin"], home_env=f"{home}/")
        assert answer is tools.ProbeResult.PROVISIONED

    def test_a_container_with_no_home_set_still_answers(self, tmp_path):
        """`exits 0 in every state` has to survive an image that never set
        `HOME`: under `set -u` an unguarded expansion aborts the script, which
        is the red devpod `fatal` this probe was written to retire."""
        home = self._home(tmp_path)
        self._official_claude(home)
        assert (
            self._answer(tmp_path, home, [self._gh(home)], home_env=None)
            is tools.ProbeResult.LENDABLE
        )

    def test_it_resolves_the_link_rather_than_trusting_the_path_entry(self):
        """In the official layout `~/.local/bin/claude` is a symlink, so what
        it points at is the only thing that says which install it belongs to."""
        assert "readlink -f" in tools.probe_script()

    def test_it_never_runs_the_candidate_claude(self, tmp_path):
        """Shim-proofness, asked of a run rather than of the script's text:
        *any* invocation of the shim triggers the download the probe exists to
        detect, so the probe runs here against a claude that records being
        invoked, and the record must stay empty.

        The recorder is shell builtins only -- `:` and a redirection -- because
        the probe runs on a stripped PATH carrying nothing but `readlink`. A
        marker that needed an external binary (`touch`, `date`) would fail to
        record the very invocation it exists to catch, and this test would
        pass for the wrong reason.
        """
        home = self._home(tmp_path)
        shim = home / ".local/bin/claude"
        shim.parent.mkdir(parents=True, exist_ok=True)
        shim.write_text('#!/bin/sh\n: > "$HOME/shim-was-executed"\n', encoding="utf-8")
        shim.chmod(0o755)
        answer = self._answer(tmp_path, home, [self._gh(home)])
        # The answer proves the run reached the end of the script: a probe
        # that crashed before resolving the claude would leave the record
        # empty too, and this test would be guarding a script that never ran.
        assert answer is tools.ProbeResult.LENDABLE
        assert not (home / "shim-was-executed").exists()

    def test_the_script_text_confines_claude_to_the_known_lookups(self):
        """A complement to the behavioural test above, not the guard itself.

        The run above proves that one real probe executed nothing; this scrub
        confines where the name may appear at all, which is what catches an
        invocation parked on a branch that run does not take. Alone it is
        weaker than it reads -- an execution spelled `"$(command -v claude)"`
        contains no literal `claude` once the lookup is scrubbed, and walks
        straight past it -- which is why it no longer stands alone.
        """
        script = tools.probe_script()
        scrubbed = (
            script.replace("command -v claude", "")
            .replace(tools.CLAUDE_VERSIONS_RELPATH, "")
            .replace("devlaunch-probe claude", "")
        )
        assert "claude" not in scrubbed
        assert "--version" not in script

    def test_the_container_is_never_asked_to_name_a_state(self):
        """The container reports two resolved paths and no verdict.

        A token would mean it had decided, and deciding needs its own copy of
        "the official layout" -- the second opinion that let a shim parked
        deeper under the versions directory be trusted by the container while
        the host refused to lend the very same tree.
        """
        script = tools.probe_script()
        for state in tools.ProbeResult:
            assert state.value not in script

    def test_the_official_layout_is_defined_once_for_both_sides_of_the_pipe(self, tmp_path):
        """The container-side "is this the official install" and the host-side
        one it mirrors must not be able to drift apart, so there is one of it:
        both go through `_is_official_claude`, and the constant it is asked
        about is likewise stated once."""
        assert tools.CLAUDE_VERSIONS_RELPATH == ".local/share/claude/versions"
        assert tools.CLAUDE_VERSIONS_RELPATH in tools.probe_script()
        assert tools.CLAUDE_VERSIONS_RELPATH in tools.transfer_script(fake_payload(tmp_path))


class TestProbeResult:
    """The probe's answer is one value with three states, not a boolean."""

    def test_it_reads_the_state_out_of_what_the_container_reported(self):
        assert tools.ProbeResult.parse(REPORT_ABSENT) is tools.ProbeResult.ABSENT
        assert tools.ProbeResult.parse(REPORT_PROVISIONED) is tools.ProbeResult.PROVISIONED
        assert tools.ProbeResult.parse(REPORT_LENDABLE) is tools.ProbeResult.LENDABLE

    def test_a_claude_deeper_under_the_versions_directory_is_not_the_install(self):
        """`under` is not `in`. The installer writes one binary per version
        directly into that directory, so a path that merely starts with it is
        somebody else's -- a downloader's, in the case this ticket exists for."""
        report = (
            "devlaunch-probe tools present\n"
            "devlaunch-probe versions /ws/.local/share/claude/versions\n"
            "devlaunch-probe claude /ws/.local/share/claude/versions/latest/bin/claude\n"
        )
        assert tools.ProbeResult.parse(report) is tools.ProbeResult.LENDABLE

    def test_a_container_that_could_resolve_nothing_is_not_provisioned(self):
        """Two blanks are equal, and equality is the whole test -- so a
        container with no `readlink`, which resolves neither path, must not
        come out as the one perfect match."""
        report = "devlaunch-probe tools present\ndevlaunch-probe versions\ndevlaunch-probe claude\n"
        assert tools.ProbeResult.parse(report) is tools.ProbeResult.LENDABLE

    def test_a_chatty_login_profile_does_not_hide_the_answer(self):
        """The probe runs under `bash -lc`, so the container's profile is
        sourced first and an image whose profile prints a banner puts that on
        the same stdout. The marked lines are what the report is, which keeps
        such an image on the one-trip path instead of re-provisioning it
        forever."""
        report = "Welcome to this image!\n\n" + REPORT_PROVISIONED
        assert tools.ProbeResult.parse(report) is tools.ProbeResult.PROVISIONED

    @pytest.mark.parametrize(
        "report",
        [
            "",
            "bash: line 1: oh no",
            "provisioned",
            "devlaunch-probe tools\n",
            "tools present\nversions /ws/.local/share/claude/versions\n",
        ],
    )
    def test_a_report_it_cannot_read_means_absent(self, report):
        """Parsing is total, and it errs towards doing the work again:
        provisioning is idempotent, so a wrong `absent` costs a redundant trip
        where a wrong `provisioned` would silently skip the whole point."""
        assert tools.ProbeResult.parse(report) is tools.ProbeResult.ABSENT


class TestSetupPassScript:
    """The cold path's one setup pass: stages, then the probe, in one trip.

    Checked as a string (what the composition may never do to the probe it
    carries) and by running it for real against scratch `$HOME`s (that a stage
    failing does not cost the probe its answer), for the same reason
    TestProbeScript is checked both ways.
    """

    def test_it_carries_the_probe_script_verbatim(self):
        """Composed *from* the probe, never a re-expression of it.

        The relation the probe reports on is stated once, in
        `_is_official_claude`, and asked from both ends. A composition that
        rewrote or trimmed the probe would be the second copy — so the pass
        contains the shipped script character for character, and an edit to the
        probe that this pass did not inherit cannot happen.
        """
        assert tools.probe_script() in tools.setup_script("myws")

    def test_the_stages_run_in_front_of_the_probe(self):
        """Order, and it is not cosmetic: the probe exits early when a tool is
        missing, which is the commonest cold-path answer, so a stage placed
        behind it would report `not reached` on the very launches the fold
        exists for."""
        script = tools.setup_script("myws")
        for stage in tools.setup_stages("myws"):
            assert script.index(stage.command) < script.index(tools.probe_script())

    def test_no_set_e_spans_the_stages(self):
        """One stage's failure is contained to that stage. `-e` anywhere in the
        pass would make the first failing stage take the probe's answer with
        it, which is why the transfer — correctly `set -eu` for the
        all-or-nothing sequence it is — is not a stage in this pass."""
        assert "set -e" not in tools.setup_script("myws")

    def test_a_workspace_name_cannot_run_a_command_of_its_own(self):
        """The name is interpolated into a shell script, so it is quoted."""
        name = "myws; touch /tmp/pwned"
        script = tools.setup_script(name)
        assert f"hostname {shlex.quote(name)}" in script
        assert "hostname myws;" not in script

    @staticmethod
    def _run(tmp_path, workspace: str, sudo_exit: Optional[int] = None):
        """Run the whole pass for real, and read it the way the host reads it.

        `sudo_exit` puts a `sudo` on PATH that exits with that status; None
        leaves the stripped PATH with no `sudo` at all, which is what an image
        without it does. Never the host's real sudo — this must not be able to
        prompt, and must not be able to rename the machine running the tests.
        """
        home = tmp_path / "home"
        home.mkdir(exist_ok=True)
        sysbin = tmp_path / "sysbin"
        sysbin.mkdir(exist_ok=True)
        readlink = shutil.which("readlink")
        assert readlink, "the test host needs readlink"
        link = sysbin / "readlink"
        if not link.exists():
            link.symlink_to(readlink)
        if sudo_exit is not None:
            sudo = sysbin / "sudo"
            sudo.write_text(f"#!/bin/sh\nexit {sudo_exit}\n", encoding="utf-8")
            sudo.chmod(0o755)
        result = subprocess.run(
            [shutil.which("bash") or "/bin/bash", "-c", tools.setup_script(workspace)],
            env={"HOME": str(home), "PATH": str(sysbin)},
            capture_output=True,
            text=True,
            check=False,
        )
        # Exits 0 whatever the stages did: a non-zero `devpod ssh` has to keep
        # meaning the transport failed, which is the discrimination the probe
        # already relies on.
        assert result.returncode == 0, result.stderr
        return home, result.stdout

    def test_a_failing_stage_does_not_cost_the_probe_its_answer(self, tmp_path):
        """The legibility claim, run rather than argued: the stage fails, names
        itself with its status, and the probe still answers in the same trip."""
        home, report = self._run(tmp_path, "myws", sudo_exit=1)
        assert tools.ProbeResult.parse(report) is tools.ProbeResult.ABSENT
        assert tools.stage_outcomes(report, tools.setup_stages("myws")) == (
            tools.StageFailed(name=tools.HOSTNAME_STAGE, returncode=1),
        )
        assert home.exists()

    def test_a_stage_the_image_cannot_even_run_reports_its_status(self, tmp_path):
        """No `sudo` in the image at all: 127, not silence."""
        _, report = self._run(tmp_path, "myws")
        assert tools.stage_outcomes(report, tools.setup_stages("myws")) == (
            tools.StageFailed(name=tools.HOSTNAME_STAGE, returncode=127),
        )

    def test_a_stage_that_worked_says_so(self, tmp_path):
        """The privileged-image case, which is the one the user can see."""
        _, report = self._run(tmp_path, "myws", sudo_exit=0)
        assert tools.stage_outcomes(report, tools.setup_stages("myws")) == (
            tools.StageOk(name=tools.HOSTNAME_STAGE),
        )


class TestStageOutcomes:
    """A stage's outcome is three-valued, and the third value is an absence."""

    @staticmethod
    def _hostname(report: str):
        return tools.stage_outcomes(report, tools.setup_stages("myws"))

    def test_a_stage_that_worked_reads_ok(self):
        assert self._hostname(STAGE_OK) == (tools.StageOk(name=tools.HOSTNAME_STAGE),)

    def test_a_stage_that_failed_carries_its_status(self):
        assert self._hostname(STAGE_FAILED) == (
            tools.StageFailed(name=tools.HOSTNAME_STAGE, returncode=1),
        )

    def test_a_report_truncated_before_a_stage_reads_not_reached(self):
        """What a mid-script death or a cut-off report looks like. It must not
        read as `ok`: "never ran" and "ran fine" are the two states this whole
        value exists to keep apart."""
        assert self._hostname(REPORT_ABSENT) == (tools.StageNotReached(name=tools.HOSTNAME_STAGE),)

    @pytest.mark.parametrize(
        "report",
        [
            "devlaunch-probe stage hostname\n",
            "devlaunch-probe stage hostname failed\n",
            "devlaunch-probe stage hostname failed sideways\n",
            "devlaunch-probe stage hostname yes\n",
            "stage hostname ok\n",
        ],
    )
    def test_an_outcome_it_cannot_read_is_never_read_as_ok(self, report):
        """Total, and it errs the way the report is used: anything that is not
        a readable outcome is the absence of one, and the host names every
        outcome that is not `ok` — so an unreadable line is reported rather
        than passed over."""
        assert self._hostname(report) == (tools.StageNotReached(name=tools.HOSTNAME_STAGE),)

    def test_a_chatty_login_profile_does_not_hide_an_outcome(self):
        """The pass runs under `bash -lc` exactly as the probe does, so the
        container's banner lands on the same stdout."""
        report = "Welcome to this image!\n\n" + STAGE_OK
        assert self._hostname(report) == (tools.StageOk(name=tools.HOSTNAME_STAGE),)

    def test_the_probe_is_inert_to_the_outcome_lines(self):
        """The container gains stage outcomes and gains no opinions: the probe
        parser reads only the keys it always read, so the stage lines are the
        same shape of noise as a login banner."""
        assert tools.ProbeResult.parse(STAGE_OK + REPORT_PROVISIONED) is (
            tools.ProbeResult.PROVISIONED
        )
        assert tools.ProbeResult.parse(STAGE_FAILED + REPORT_ABSENT) is tools.ProbeResult.ABSENT


@pytest.mark.usefixtures("no_host_payload")
class TestProvisionTools:
    """The three-trip flow: probe, host transfer, network install."""

    def test_a_provisioned_workspace_pays_one_setup_pass_and_nothing_else(self):
        """The common path: every launch after the first is one round trip, and
        that trip is the whole setup pass -- stages and probe together."""
        runner = Runner(returncodes=(0,), stdout=REPORT_PROVISIONED)
        assert provision_tools("myws", runner) is True
        assert len(runner.calls) == 1
        assert runner.calls[0][:2] == ["ssh", "myws"]
        # A non-login shell has no ~/.pixi/bin, so every tool would look absent.
        argv = shlex.split(runner.script())
        assert argv[:2] == ["bash", "-lc"]
        assert argv[2] == tools.setup_script("myws")

    def test_the_container_is_never_named_by_a_trip_of_its_own(self):
        """The saving, stated as the thing that must not appear: on no probe
        answer does a trip go out whose whole payload is the hostname. Inside
        the pass's own script it is expected -- that is the fold."""
        for report in (REPORT_PROVISIONED, REPORT_LENDABLE, REPORT_ABSENT):
            runner = Runner(returncodes=(0, 0), stdout=report)
            provision_tools("myws", runner)
            assert not any("sudo hostname myws" in call for call in runner.calls)

    def test_with_nothing_to_lend_a_cold_workspace_gets_the_network_install(self):
        runner = Runner(returncodes=(0, 0), stdout=REPORT_ABSENT)
        assert provision_tools("myws", runner) is True
        assert len(runner.calls) == 2
        # shlex.split is the inverse of the quoting provision_tools applies.
        argv = shlex.split(runner.script(1))
        assert argv[:2] == ["bash", "-lc"]
        assert argv[2] == provision_script()

    def test_a_probe_trip_that_fails_outright_is_read_as_absent(self):
        """The script exits 0 in all three states, so a non-zero trip is the
        trip failing rather than an answer -- and the cold flow is the safe
        reading of no answer at all."""
        runner = Runner(returncodes=(1, 0), stdout=REPORT_PROVISIONED)
        assert provision_tools("myws", runner) is True
        assert len(runner.calls) == 2
        assert shlex.split(runner.script(1))[2] == provision_script()

    def test_a_host_with_the_tools_lends_them_instead_of_the_network(self, tmp_path):
        runner = Runner(returncodes=(0, 0), stdout=REPORT_ABSENT)
        with patch.object(tools, "host_payload", return_value=fake_payload(tmp_path)):
            assert provision_tools("myws", runner) is True
        assert len(runner.calls) == 2
        # The transfer is the tar stream: stdin on the trip, no stream elsewhere.
        assert [stream is not None for stream in runner.streams] == [False, True]
        assert "tar xf -" in runner.script(1)

    def test_a_transfer_the_container_rejects_falls_back_to_the_network(self, tmp_path):
        """The lent binaries may not run there (arch, libc); the gate at the
        end of the transfer script reports it, and the old path still runs."""
        runner = Runner(returncodes=(0, 1, 0), stdout=REPORT_ABSENT)
        with patch.object(tools, "host_payload", return_value=fake_payload(tmp_path)):
            assert provision_tools("myws", runner) is True
        assert len(runner.calls) == 3
        assert shlex.split(runner.script(2))[2] == provision_script()

    def test_a_shim_workspace_is_lent_the_hosts_real_claude(self, tmp_path):
        """`lendable` is the state this whole ticket exists for: both tools
        answer, but the claude is a downloader, so the host replaces it."""
        runner = Runner(returncodes=(0, 0), stdout=REPORT_LENDABLE)
        with patch.object(tools, "host_payload", return_value=fake_payload(tmp_path)):
            assert provision_tools("myws", runner) is True
        assert len(runner.calls) == 2
        assert [stream is not None for stream in runner.streams] == [False, True]
        assert "tar xf -" in runner.script(1)

    def test_a_shim_the_lend_could_not_replace_is_accepted_not_reinstalled(self, tmp_path):
        """The container could not run the lent binaries, but it does have a
        claude and a gh. The network fallback decides what to install with its
        own `command -v` guards, which both already satisfy, so a third trip
        would install nothing -- it is not taken."""
        runner = Runner(returncodes=(0, 1), stdout=REPORT_LENDABLE)
        with patch.object(tools, "host_payload", return_value=fake_payload(tmp_path)):
            assert provision_tools("myws", runner) is True
        assert len(runner.calls) == 2

    def test_a_shim_with_nothing_to_lend_stops_after_the_probe(self):
        """Nothing on the host to replace it with, and the network fallback
        would no-op, so the shim stands and the launch costs one trip."""
        runner = Runner(returncodes=(0,), stdout=REPORT_LENDABLE)
        assert provision_tools("myws", runner) is True
        assert len(runner.calls) == 1

    def test_the_install_output_reaches_the_user_but_the_probe_is_silent(self):
        """A cold install is tens of seconds; captured, it reads as a hung dl.
        The scripts' progress lines are the only thing on the terminal during
        it, so capturing them would be the same as having none.

        The probe is the exception: its output is not progress but the answer
        the caller branches on, which is exactly what has to be read back.
        """
        runner = Runner(returncodes=(0, 0), stdout=REPORT_ABSENT)
        provision_tools("myws", runner)
        assert runner.captured == [True, False]

    def test_a_failed_install_does_not_raise(self):
        """The workspace is up and the user asked for a session, not an install."""
        runner = Runner(returncodes=(1,), stderr="no network")
        assert provision_tools("myws", runner) is False

    def test_a_missing_devpod_does_not_raise(self):
        def explode(*_args, **_kwargs):
            raise OSError("devpod vanished")

        assert provision_tools("myws", explode) is False

    def test_the_opt_out_installs_nothing_and_still_names_the_container(self, monkeypatch):
        """Whether the pass runs and whether the tools work runs are two
        questions, and the opt-out answers only the second. Riding the hostname
        inside a gate called "no tools" would mean turning tools off silently
        turned container naming off with it -- a coupling nobody would find
        again for a year.
        """
        monkeypatch.setenv(tools.DISABLE_VAR, "1")
        runner = Runner(returncodes=(0,), stdout=REPORT_ABSENT)
        assert provision_tools("myws", runner) is False
        assert len(runner.calls) == 1
        assert shlex.split(runner.script())[2] == tools.setup_script("myws")

    @pytest.mark.parametrize("value", ["", "0", "false", "no", "NO"])
    def test_falsey_opt_out_values_leave_provisioning_on(self, monkeypatch, value):
        monkeypatch.setenv(tools.DISABLE_VAR, value)
        runner = Runner()
        assert provision_tools("myws", runner) is True


@pytest.mark.usefixtures("no_host_payload")
class TestTheHostNamesEveryStageThatIsNotOk:
    """The legibility that makes the fold allowed at all.

    Three trips gave three distinguishable errors only in principle: the
    hostname trip's failure was a boolean the attach threw away, so today one of
    the three is silence. One trip that names its stages is more legible than
    that, not less.
    """

    @staticmethod
    def _report(caplog, stdout: str, returncode: int = 0) -> List[tuple]:
        with caplog.at_level(logging.INFO, logger=""):
            provision_tools("myws", Runner(returncodes=(returncode, 0), stdout=stdout))
        return [
            (record.levelno, record.getMessage())
            for record in caplog.records
            if tools.HOSTNAME_STAGE in record.getMessage()
        ]

    def test_a_failing_stage_is_named_with_its_status(self, caplog):
        reported = self._report(caplog, STAGE_FAILED + REPORT_ABSENT)
        assert len(reported) == 1
        level, message = reported[0]
        assert "myws" in message
        assert "1" in message
        # Info, not warning, and only for this stage: `sudo hostname` cannot
        # succeed without CAP_SYS_ADMIN, which Docker drops by default, so a
        # warning here would fire on the majority of cold launches.
        assert level == logging.INFO

    def test_a_stage_that_worked_is_not_reported_at_all(self, caplog):
        assert self._report(caplog, STAGE_OK + REPORT_PROVISIONED) == []

    def test_a_stage_that_was_never_reached_is_named_too(self, caplog):
        """The state that used to be unrepresentable. A report with no line for
        the stage is not the stage working."""
        reported = self._report(caplog, REPORT_PROVISIONED)
        assert len(reported) == 1
        assert reported[0][0] == logging.INFO

    def test_a_trip_that_never_got_through_reports_no_stage_as_ok(self, caplog):
        """A non-zero `devpod ssh` is the transport, not a stage -- so nothing
        may be read as having run."""
        reported = self._report(caplog, "", returncode=1)
        assert len(reported) == 1

    def test_a_stage_warns_by_name_unless_it_asks_to_be_quieter(self):
        """Warning is the default a new stage gets: naming a stage that did not
        work is the whole point of the composition, and the hostname's info
        level is an exception it has to declare, measured (#167), rather than
        the rule."""
        assert tools.Stage(name="x", command="true").failure_level == logging.WARNING
        levels = {stage.name: stage.failure_level for stage in tools.setup_stages("myws")}
        assert levels == {tools.HOSTNAME_STAGE: logging.INFO}


class TestHostPayload:
    """What the host may lend, resolved from its real install layouts."""

    @staticmethod
    def _official_claude(home, version="2.0.1"):
        versions = home / ".local/share/claude/versions"
        versions.mkdir(parents=True)
        binary = versions / version
        binary.write_bytes(b"#!/bin/sh\n")
        binary.chmod(0o755)
        bin_dir = home / ".local/bin"
        bin_dir.mkdir(parents=True, exist_ok=True)
        (bin_dir / "claude").symlink_to(binary)
        return binary

    @staticmethod
    def _real_gh(home):
        """A gh binary reached directly, with no trampoline in front of it."""
        gh = home / "bin/gh"
        gh.parent.mkdir(parents=True, exist_ok=True)
        gh.write_bytes(b"\x7fELF")
        gh.chmod(0o755)
        return gh

    @staticmethod
    def _as_host(monkeypatch, home, gh):
        """Point the resolvers at a scratch home and a scratch gh on PATH."""
        monkeypatch.setattr(tools.pathlib.Path, "home", staticmethod(lambda: home))
        monkeypatch.setattr(tools.shutil, "which", lambda _: str(gh) if gh else None)

    def test_the_official_claude_layout_is_lent(self, tmp_path, monkeypatch):
        binary = self._official_claude(tmp_path)
        gh = self._real_gh(tmp_path)
        self._as_host(monkeypatch, tmp_path, gh)
        payload = tools.host_payload()
        assert payload is not None
        assert payload.claude_version == "2.0.1"
        assert payload.members == (
            (binary, ".local/share/claude/versions/2.0.1"),
            (gh, ".local/bin/gh"),
        )

    def test_the_lent_paths_are_all_home_relative(self, tmp_path, monkeypatch):
        """They are tar arcnames: an absolute one would unpack into the host's
        usernamed home, which does not exist in the container."""
        self._official_claude(tmp_path)
        self._as_host(monkeypatch, tmp_path, self._real_gh(tmp_path))
        payload = tools.host_payload()
        assert payload is not None
        assert all(not arcname.startswith("/") for _source, arcname in payload.members)

    def test_a_claude_that_is_not_the_official_install_is_not_lent(self, tmp_path, monkeypatch):
        """A shim or wrapper on PATH is the downloader this transfer exists to
        skip -- lending it would lend the download."""
        bin_dir = tmp_path / ".local/bin"
        bin_dir.mkdir(parents=True)
        rogue = bin_dir / "claude"
        rogue.write_bytes(b"#!/bin/sh\n")
        rogue.chmod(0o755)
        self._as_host(monkeypatch, tmp_path, self._real_gh(tmp_path))
        assert tools.host_payload() is None

    def test_a_pixi_trampoline_lends_the_binary_it_names(self, tmp_path, monkeypatch):
        """The trampoline is a launcher that re-execs the env's binary named in
        a JSON file beside it; copied alone it launches nothing."""
        real = tmp_path / "envs/gh/bin/gh"
        real.parent.mkdir(parents=True)
        real.write_bytes(b"\x7fELF")
        real.chmod(0o755)
        trampoline = tmp_path / "pixi-bin/gh"
        (trampoline.parent / "trampoline_configuration").mkdir(parents=True)
        trampoline.write_bytes(b"\x7fELF")
        trampoline.chmod(0o755)
        (trampoline.parent / "trampoline_configuration/gh.json").write_text(
            f'{{"exe": "{real}"}}', encoding="utf-8"
        )
        self._official_claude(tmp_path)
        self._as_host(monkeypatch, tmp_path, trampoline)
        payload = tools.host_payload()
        assert payload is not None
        assert (real, ".local/bin/gh") in payload.members

    def test_an_unreadable_trampoline_lends_nothing(self, tmp_path, monkeypatch):
        """Shipping the launcher without the binary it launches ships a break."""
        trampoline = tmp_path / "pixi-bin/gh"
        (trampoline.parent / "trampoline_configuration").mkdir(parents=True)
        trampoline.write_bytes(b"\x7fELF")
        trampoline.chmod(0o755)
        (trampoline.parent / "trampoline_configuration/gh.json").write_text(
            "not json", encoding="utf-8"
        )
        self._official_claude(tmp_path)
        self._as_host(monkeypatch, tmp_path, trampoline)
        assert tools.host_payload() is None

    def test_the_payload_is_all_or_nothing(self, tmp_path, monkeypatch):
        """A host missing either tool falls back to the network for both,
        rather than growing half-lent states the fallback must reason about."""
        self._official_claude(tmp_path)
        self._as_host(monkeypatch, tmp_path, None)
        assert tools.host_payload() is None


class TestTransferScript:
    """The receiving end runs in someone else's container; checked closely."""

    def _script(self, tmp_path) -> str:
        return tools.transfer_script(fake_payload(tmp_path))

    def test_it_unpacks_into_home_and_links_the_current_version(self, tmp_path):
        script = self._script(tmp_path)
        assert 'tar xf - -C "$STAGE"' in script
        # The host's own symlink would point through the host's home, so the
        # link is made in the container, against the container's $HOME.
        assert (
            'ln -sfn "$HOME/.local/share/claude/versions/2.0.1" "$HOME/.local/bin/claude"' in script
        )

    def test_nothing_leaves_the_staging_area_until_it_has_been_proved_to_run(self, tmp_path):
        """The arch/libc gate has to come before the container is changed, not
        after.

        Unpacking straight into $HOME meant a failed gate still left the PATH
        edit and the `claude` symlink behind — and the network fallback that
        follows decides what to install with `command -v`, which a broken
        binary satisfies. So a container that could not run the lent claude
        got a permanently broken one, the fallback installed nothing, and
        every later probe reported success. Order is the whole fix.
        """
        script = self._script(tmp_path)
        assert "set -eu" in script
        gate = script.index('"$STAGE/.local/share/claude/versions/2.0.1" --version')
        assert script.index('"$STAGE/.local/bin/gh" --version') < script.index("mv -f")
        assert gate < script.index("mv -f"), "proved before anything is moved into place"
        assert gate < script.index("ln -sfn"), "proved before the symlink is made"
        assert gate < script.index('>> "$PROFILE"'), "proved before PATH is edited"

    def test_a_failed_transfer_leaves_the_staging_area_behind_it(self, tmp_path):
        """`set -eu` aborts wherever it fails, so the cleanup has to be a trap
        rather than a last line — otherwise a gate failure strands a few
        hundred MB under $HOME that nothing ever collects."""
        script = self._script(tmp_path)
        assert "trap 'rm -rf \"$STAGE\"' EXIT" in script
        assert script.index("trap") < script.index("tar xf -")

    def test_progress_goes_to_stderr_not_stdout(self, tmp_path):
        script = self._script(tmp_path)
        assert "exec >&2" in script
        assert script.index("exec >&2") < script.index("echo")

    def test_the_stream_the_container_receives_is_that_tar(self, tmp_path):
        """End to end: what `tar xf -` reads on the other side has to be a
        readable archive of exactly the lent files, under the arcnames the
        link and the PATH edit were written against."""
        runner = Runner(returncodes=(1, 0))
        with patch.object(tools, "host_payload", return_value=fake_payload(tmp_path)):
            assert provision_tools("myws", runner) is True
        streamed = runner.streams[1]
        assert streamed is not None, "the transfer trip streamed nothing"
        with tarfile.open(fileobj=io.BytesIO(streamed)) as tar:
            names = tar.getnames()
        assert sorted(names) == [".local/bin/gh", ".local/share/claude/versions/2.0.1"]


class TestWorkspaceUpInstallsTools:
    """The wiring: every workspace dl opens goes through `up` at least once."""

    @staticmethod
    def _up(returncode: int):
        from devlaunch import dl

        with patch.object(dl, "run_devpod") as run_devpod:
            # stdout is read by the `context options` call on the way through.
            run_devpod.return_value = subprocess.CompletedProcess(
                [], returncode=returncode, stdout="{}"
            )
            with patch.object(dl, "invalidate_workspace_list_cache"):
                with patch.object(dl.tools, "provision_tools") as provision:
                    dl.workspace_up("owner/repo", workspace_id="myws", workspace_identity="myws")
        return provision

    def test_a_successful_up_installs_the_tools(self):
        provision = self._up(returncode=0)
        provision.assert_called_once()
        assert provision.call_args.args[0] == "myws"

    def test_a_failed_up_does_not(self):
        """There is no container to install into."""
        assert self._up(returncode=1).call_count == 0

    def test_up_against_a_container_already_running_still_tops_them_up(self):
        """`dl <ws> up` is one of the two verbs the module docstring names as
        how a workspace that missed provisioning gets it -- started by something
        other than dl, or created before this existed. That workspace is very
        often already running, which is the arm that returns without calling
        `devpod up` at all; skipping the tools there would make the documented
        recovery the one route that cannot recover."""
        from devlaunch import dl

        with (
            patch.object(dl, "get_workspace_state", return_value="Running"),
            patch.object(dl, "workspace_up") as workspace_up,
            patch.object(dl.tools, "provision_tools") as provision,
        ):
            assert dl.main(["myws", "up"]) == 0
        workspace_up.assert_not_called()
        provision.assert_called_once()
        assert provision.call_args.args[0] == "myws"


class TestNoRegressionInTheOptOutContract:
    """DEVLAUNCH_NO_TOOLS mirrors DEVLAUNCH_NO_GH_TOKEN, so it reads the same."""

    def test_disabled_matches_the_gh_auth_convention(self, monkeypatch):
        from devlaunch import gh_auth

        for value in ("1", "true", "yes", "anything"):
            monkeypatch.setenv(tools.DISABLE_VAR, value)
            monkeypatch.setenv(gh_auth.DISABLE_VAR, value)
            assert tools.provisioning_disabled() == gh_auth.forwarding_disabled()

    def test_unset_means_enabled(self, monkeypatch):
        monkeypatch.delenv(tools.DISABLE_VAR, raising=False)
        assert tools.provisioning_disabled() is False
