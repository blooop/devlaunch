"""Tests for picking a transport that can carry a terminal into the workspace.

The behaviour under test is a routing decision, not a container one: given
whether the user has a terminal and whether devpod has written an ssh host
alias for the workspace, which of the two transports should carry the command.
Everything here is pure logic over a config file and two isatty answers, so it
runs without devpod, without ssh and without a container.
"""

import io
import pathlib

import pytest

from devlaunch import tty_session


class FakeStream:
    """A stdin/stdout stand-in whose isatty answer the test chooses."""

    def __init__(self, tty: bool):
        self._tty = tty

    def isatty(self) -> bool:
        return self._tty


def write_devpod_entry(path: pathlib.Path, *workspace_ids: str) -> None:
    """Write an ssh config in the exact shape `devpod up` leaves behind."""
    blocks = []
    for workspace_id in workspace_ids:
        host = f"{workspace_id}.devpod"
        blocks.append(
            f"# DevPod Start {host}\n"
            f"Host {host}\n"
            f"  ForwardAgent yes\n"
            f"  StrictHostKeyChecking no\n"
            f'  ProxyCommand "devpod" ssh --stdio --context default --user vscode {workspace_id}\n'
            f"  User vscode\n"
            f"# DevPod End {host}\n"
        )
    path.write_text("".join(blocks))


class TestHaveTerminal:
    """have_terminal is the same question ssh asks before allocating a pty."""

    def test_both_streams_a_terminal(self):
        assert tty_session.have_terminal(FakeStream(True), FakeStream(True)) is True

    def test_piped_stdout_is_not_a_terminal(self):
        """`dl ws -- ls > out.txt` must not get a pty, or out.txt fills with escapes."""
        assert tty_session.have_terminal(FakeStream(True), FakeStream(False)) is False

    def test_piped_stdin_is_not_a_terminal(self):
        assert tty_session.have_terminal(FakeStream(False), FakeStream(True)) is False

    def test_stream_without_isatty_is_not_a_terminal(self):
        """pytest's captured stdout is a plain object; it must not read as a tty."""
        assert tty_session.have_terminal(object(), object()) is False

    def test_closed_stream_is_not_a_terminal(self):
        closed = io.StringIO()
        closed.close()
        assert tty_session.have_terminal(closed, closed) is False

    def test_opt_out_env_var_forces_no_terminal(self, monkeypatch):
        """An escape hatch for a machine where the ssh transport misbehaves."""
        monkeypatch.setenv(tty_session.DISABLE_VAR, "1")
        assert tty_session.have_terminal(FakeStream(True), FakeStream(True)) is False

    @pytest.mark.parametrize("value", ["", "0", "false", "no"])
    def test_falsey_opt_out_still_allows_a_terminal(self, monkeypatch, value):
        monkeypatch.setenv(tty_session.DISABLE_VAR, value)
        assert tty_session.have_terminal(FakeStream(True), FakeStream(True)) is True


class TestDevpodHostConfigured:
    """The ssh alias is devpod's own; dl reads it rather than reinventing it."""

    def test_finds_the_entry_devpod_wrote(self, tmp_path):
        config = tmp_path / "config"
        write_devpod_entry(config, "devlaunch-main-abcdefgh")
        assert tty_session.devpod_host_configured("devlaunch-main-abcdefgh", config) is True

    def test_absent_workspace_is_not_configured(self, tmp_path):
        config = tmp_path / "config"
        write_devpod_entry(config, "some-other-workspace")
        assert tty_session.devpod_host_configured("devlaunch-main-abcdefgh", config) is False

    def test_prefix_of_another_workspace_does_not_count(self, tmp_path):
        """`dl` names workspaces by prefix + suffix, so substring matching would lie."""
        config = tmp_path / "config"
        write_devpod_entry(config, "devlaunch-main-abcdefgh")
        assert tty_session.devpod_host_configured("devlaunch-main", config) is False

    def test_missing_config_file_is_not_configured(self, tmp_path):
        assert tty_session.devpod_host_configured("anything", tmp_path / "nope") is False

    def test_unreadable_config_is_not_configured(self, tmp_path):
        """A config dl cannot read is a fallback, never a crash mid-launch."""
        config = tmp_path / "config"
        write_devpod_entry(config, "devlaunch-main-abcdefgh")
        config.chmod(0o000)
        try:
            assert tty_session.devpod_host_configured("devlaunch-main-abcdefgh", config) is False
        finally:
            config.chmod(0o600)

    def test_one_of_several_entries(self, tmp_path):
        config = tmp_path / "config"
        write_devpod_entry(config, "ws-one", "ws-two", "ws-three")
        assert tty_session.devpod_host_configured("ws-two", config) is True


class TestSshCommandArgs:
    """The OpenSSH invocation that actually carries a terminal."""

    def test_forces_a_pty(self):
        args = tty_session.ssh_command_args("myws", "bash -lc claude")
        assert args[0] == "ssh"
        assert "-t" in args, "without -t ssh runs the command with no terminal"

    def test_targets_the_devpod_host_alias(self):
        args = tty_session.ssh_command_args("myws", "bash -lc claude")
        assert "myws.devpod" in args

    def test_payload_is_the_final_argument(self):
        """One argument, so the remote shell sees the command dl composed."""
        args = tty_session.ssh_command_args("myws", "bash -lc 'claude do the thing'")
        assert args[-1] == "bash -lc 'claude do the thing'"

    def test_host_comes_before_the_payload(self):
        args = tty_session.ssh_command_args("myws", "bash -lc claude")
        assert args.index("myws.devpod") < len(args) - 1

    def test_send_env_names_variables_without_their_values(self):
        """The token must reach the container through the environment, not argv."""
        args = tty_session.ssh_command_args("myws", "bash -lc claude", send_env=["GH_TOKEN"])
        assert "SendEnv=GH_TOKEN" in args
        assert not any("secret" in arg for arg in args)

    def test_no_send_env_when_nothing_to_forward(self):
        args = tty_session.ssh_command_args("myws", "bash -lc claude")
        assert not any(arg.startswith("SendEnv") for arg in args)

    def test_workdir_becomes_a_cd_in_the_payload(self):
        """ssh has no --workdir, so the directory has to travel in the command."""
        args = tty_session.ssh_command_args("myws", "bash -lc make", workdir="/workspaces/myws")
        assert args[-1].startswith("cd /workspaces/myws && ")
        assert args[-1].endswith("bash -lc make")

    def test_workdir_with_spaces_is_quoted(self):
        args = tty_session.ssh_command_args("myws", "bash -lc make", workdir="/a dir/with space")
        assert "'/a dir/with space'" in args[-1]
