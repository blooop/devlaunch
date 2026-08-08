"""What devlaunch installs into a workspace, and what it does when that fails."""

import shlex
import subprocess
from typing import List
from unittest.mock import patch

import pytest

from devlaunch import tools
from devlaunch.tools import REQUIRED_TOOLS, Tool, ensure_tools, provision_script


class Runner:
    """Stands in for dl.run_devpod, recording what was asked of devpod."""

    def __init__(self, returncode: int = 0, stdout: str = "", stderr: str = ""):
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr
        self.calls: List[List[str]] = []

    def __call__(self, args, capture=False, env=None) -> subprocess.CompletedProcess:
        self.calls.append(list(args))
        return subprocess.CompletedProcess(
            args=list(args), returncode=self.returncode, stdout=self.stdout, stderr=self.stderr
        )

    @property
    def script(self) -> str:
        """The payload of the single ssh --command that was sent."""
        return self.calls[0][self.calls[0].index("--command") + 1]


@pytest.fixture(autouse=True)
def _forwarding_enabled(monkeypatch):
    """The opt-out must not leak in from the machine running the tests."""
    monkeypatch.delenv(tools.DISABLE_VAR, raising=False)


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

    def test_the_profile_is_not_appended_to_twice(self):
        """Every profile edit is guarded, so relaunching cannot grow the file."""
        script = provision_script()
        profile_writes = [line for line in script.splitlines() if '>> "$PROFILE"' in line]
        assert profile_writes
        assert all(line.startswith("grep -q") for line in profile_writes)

    def test_pixi_is_installed_when_the_image_has_none(self):
        """An arbitrary repo's container need not carry pixi."""
        assert "command -v pixi" in provision_script()

    def test_a_custom_tool_set_is_honoured(self):
        script = provision_script([Tool(command="jq", package="jq")])
        assert "command -v jq" in script
        assert "command -v gh" not in script


class TestEnsureTools:
    """How the script is delivered, and what happens when it does not work."""

    def test_it_sends_one_ssh_command_through_a_login_shell(self):
        """A non-login shell has no ~/.pixi/bin, so every tool would look absent."""
        runner = Runner()
        assert ensure_tools("myws", runner) is True
        assert len(runner.calls) == 1
        assert runner.calls[0][:2] == ["ssh", "myws"]
        assert runner.script.startswith("bash -lc ")

    def test_the_script_survives_shell_quoting(self):
        """The payload is full of quotes; it must reach the container intact."""
        runner = Runner()
        ensure_tools("myws", runner)
        # shlex.split is the inverse of the quoting ensure_tools applies.
        argv = shlex.split(runner.script)
        assert argv[:2] == ["bash", "-lc"]
        assert argv[2] == provision_script()

    def test_a_failed_install_does_not_raise(self):
        """The workspace is up and the user asked for a session, not an install."""
        runner = Runner(returncode=1, stderr="no network")
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
