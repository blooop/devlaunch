"""What devlaunch installs into a workspace, and what it does when that fails."""

import io
import shlex
import subprocess
import tarfile
from typing import List, Optional
from unittest.mock import patch

import pytest

from devlaunch import tools
from devlaunch.tools import REQUIRED_TOOLS, Tool, ensure_tools, provision_script


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

    ensure_tools asks the real filesystem what the machine could lend; on a
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

    def test_the_profile_is_not_appended_to_twice(self):
        """Every profile edit is guarded, so relaunching cannot grow the file."""
        script = provision_script()
        profile_writes = [line for line in script.splitlines() if '>> "$PROFILE"' in line]
        assert profile_writes
        assert all(line.startswith("grep -q") for line in profile_writes)

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


@pytest.mark.usefixtures("no_host_payload")
class TestEnsureTools:
    """The three-trip flow: probe, host transfer, network install."""

    def test_a_provisioned_workspace_pays_one_probe_and_nothing_else(self):
        """The common path: every launch after the first is one round trip."""
        runner = Runner(returncodes=(0,))
        assert ensure_tools("myws", runner) is True
        assert len(runner.calls) == 1
        assert runner.calls[0][:2] == ["ssh", "myws"]
        # A non-login shell has no ~/.pixi/bin, so every tool would look absent.
        assert runner.script().startswith("bash -lc ")
        assert "command -v gh" in runner.script()
        assert "command -v claude" in runner.script()

    def test_with_nothing_to_lend_a_cold_workspace_gets_the_network_install(self):
        runner = Runner(returncodes=(1, 0))
        assert ensure_tools("myws", runner) is True
        assert len(runner.calls) == 2
        # shlex.split is the inverse of the quoting ensure_tools applies.
        argv = shlex.split(runner.script(1))
        assert argv[:2] == ["bash", "-lc"]
        assert argv[2] == provision_script()

    def test_a_host_with_the_tools_lends_them_instead_of_the_network(self, tmp_path):
        runner = Runner(returncodes=(1, 0))
        with patch.object(tools, "host_payload", return_value=fake_payload(tmp_path)):
            assert ensure_tools("myws", runner) is True
        assert len(runner.calls) == 2
        # The transfer is the tar stream: stdin on the trip, no stream elsewhere.
        assert [stream is not None for stream in runner.streams] == [False, True]
        assert "tar xf -" in runner.script(1)

    def test_a_transfer_the_container_rejects_falls_back_to_the_network(self, tmp_path):
        """The lent binaries may not run there (arch, libc); the gate at the
        end of the transfer script reports it, and the old path still runs."""
        runner = Runner(returncodes=(1, 1, 0))
        with patch.object(tools, "host_payload", return_value=fake_payload(tmp_path)):
            assert ensure_tools("myws", runner) is True
        assert len(runner.calls) == 3
        assert shlex.split(runner.script(2))[2] == provision_script()

    def test_the_install_output_reaches_the_user_but_the_probe_is_silent(self):
        """A cold install is tens of seconds; captured, it reads as a hung dl.
        The scripts' progress lines are the only thing on the terminal during
        it, so capturing them would be the same as having none.

        The probe is the exception: it reports nothing, and its everyday
        failure on a cold workspace reaches the terminal as a red devpod
        `fatal` describing the probe doing its job.
        """
        runner = Runner(returncodes=(1, 0))
        ensure_tools("myws", runner)
        assert runner.captured == [True, False]

    def test_a_failed_install_does_not_raise(self):
        """The workspace is up and the user asked for a session, not an install."""
        runner = Runner(returncodes=(1,), stderr="no network")
        assert ensure_tools("myws", runner) is False

    def test_a_missing_devpod_does_not_raise(self):
        def explode(*_args, **_kwargs):
            raise OSError("devpod vanished")

        assert ensure_tools("myws", explode) is False

    def test_the_opt_out_skips_devpod_entirely(self, monkeypatch):
        monkeypatch.setenv(tools.DISABLE_VAR, "1")
        runner = Runner()
        assert ensure_tools("myws", runner) is False
        assert runner.calls == []

    @pytest.mark.parametrize("value", ["", "0", "false", "no", "NO"])
    def test_falsey_opt_out_values_leave_provisioning_on(self, monkeypatch, value):
        monkeypatch.setenv(tools.DISABLE_VAR, value)
        runner = Runner()
        assert ensure_tools("myws", runner) is True


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

    def test_the_profile_edit_is_guarded_like_the_network_installs(self, tmp_path):
        script = self._script(tmp_path)
        profile_writes = [line for line in script.splitlines() if '>> "$PROFILE"' in line]
        assert profile_writes
        assert all(line.startswith("grep -q") for line in profile_writes)

    def test_the_stream_the_container_receives_is_that_tar(self, tmp_path):
        """End to end: what `tar xf -` reads on the other side has to be a
        readable archive of exactly the lent files, under the arcnames the
        link and the PATH edit were written against."""
        runner = Runner(returncodes=(1, 0))
        with patch.object(tools, "host_payload", return_value=fake_payload(tmp_path)):
            assert ensure_tools("myws", runner) is True
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
                with patch.object(dl.tools, "ensure_tools") as ensure:
                    dl.workspace_up("owner/repo", workspace_id="myws", workspace_identity="myws")
        return ensure

    def test_a_successful_up_installs_the_tools(self):
        ensure = self._up(returncode=0)
        ensure.assert_called_once()
        assert ensure.call_args.args[0] == "myws"

    def test_a_failed_up_does_not(self):
        """There is no container to install into."""
        assert self._up(returncode=1).call_count == 0


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
