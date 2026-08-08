"""Tests for `dl <ws> -- <command>` keeping a terminal when the user has one.

`devpod ssh --command` never asks its ssh session for a pty, so anything it
starts runs with stdin, stdout and stderr on pipes and TERM=dumb. One-shot
commands don't care. Interactive programs do: `claude` sees a pipe, decides it
was invoked non-interactively, switches to --print mode and exits -- which is
`aid <repo>` failing to leave a session behind.

The fix is a second transport, not a second launcher: when dl itself is on a
terminal it hands the command to OpenSSH through the host alias `devpod up`
already wrote, with -t to force a pty. These tests pin the routing decision and
the shape of both invocations; test/e2e/test_interactive_session.py proves the
resulting session is real and stays up.
"""

# Requesting a fixture shadows its name and often ignores its value -- both are
# how pytest is written, and neither is a defect. Same suppression as
# test/fixtures/git_fixtures.py.
# pylint: disable=redefined-outer-name,unused-argument

import pathlib
from unittest.mock import MagicMock, patch

import pytest

from devlaunch import dl as dl_module
from devlaunch import gh_auth, tty_session
from devlaunch.devpod_ssh import RemoteExit
from devlaunch.dl import workspace_ssh


@pytest.fixture
def ssh_config(tmp_path) -> pathlib.Path:
    """An ssh config carrying the alias `devpod up` writes for `myws`."""
    config = tmp_path / "config"
    config.write_text(
        "# DevPod Start myws.devpod\n"
        "Host myws.devpod\n"
        '  ProxyCommand "devpod" ssh --stdio --context default --user vscode myws\n'
        "  User vscode\n"
        "# DevPod End myws.devpod\n"
    )
    return config


@pytest.fixture
def on_a_terminal(monkeypatch, ssh_config):
    """Put dl in the situation a developer is in: a real terminal, alias present."""
    monkeypatch.setattr(tty_session, "have_terminal", lambda *a, **k: True)
    monkeypatch.setattr(tty_session, "SSH_CONFIG_PATH", ssh_config)


@pytest.fixture
def no_terminal(monkeypatch):
    """Put dl in the situation CI is in: output redirected, no terminal."""
    monkeypatch.setattr(tty_session, "have_terminal", lambda *a, **k: False)


def ssh_args(mock_run_ssh) -> list:
    """The argv of the single OpenSSH invocation dl made."""
    assert mock_run_ssh.call_count == 1, f"expected one ssh call, got {mock_run_ssh.call_count}"
    return mock_run_ssh.call_args[0][0]


class TestTransportRouting:
    """Which of the two transports carries the command."""

    @patch("devlaunch.dl.run_ssh")
    @patch("devlaunch.dl.run_devpod_session")
    def test_terminal_uses_openssh_with_a_pty(self, mock_session, mock_ssh, on_a_terminal):
        """The regression: an interactive payload must not go through --command."""
        mock_ssh.return_value = MagicMock(returncode=0)
        workspace_ssh("myws", command="claude")

        args = ssh_args(mock_ssh)
        assert args[0] == "ssh"
        assert "-t" in args
        assert "myws.devpod" in args
        mock_session.assert_not_called()

    @patch("devlaunch.dl.run_ssh")
    @patch("devlaunch.dl.run_devpod_session")
    def test_no_terminal_keeps_the_devpod_transport(self, mock_session, mock_ssh, no_terminal):
        """Piped output must stay clean, so no pty and no escape sequences."""
        mock_session.return_value = RemoteExit(0)
        workspace_ssh("myws", command="make test")

        mock_ssh.assert_not_called()
        args = mock_session.call_args[0][0]
        assert args[:2] == ["ssh", "myws"]
        assert "--command" in args

    @patch("devlaunch.dl.run_ssh")
    @patch("devlaunch.dl.run_devpod_session")
    def test_missing_host_alias_falls_back_and_warns(
        self, mock_session, mock_ssh, monkeypatch, tmp_path, caplog
    ):
        """A workspace devpod never wrote an alias for still has to run the command."""
        monkeypatch.setattr(tty_session, "have_terminal", lambda *a, **k: True)
        monkeypatch.setattr(tty_session, "SSH_CONFIG_PATH", tmp_path / "absent")
        mock_session.return_value = RemoteExit(0)

        workspace_ssh("myws", command="claude")

        mock_ssh.assert_not_called()
        assert "--command" in mock_session.call_args[0][0]
        assert "myws" in caplog.text

    @patch("devlaunch.dl.run_ssh")
    @patch("devlaunch.dl.run_devpod_session")
    def test_interactive_attach_is_untouched(self, mock_session, mock_ssh, on_a_terminal):
        """`dl <ws>` with no command already gets a pty from devpod; leave it alone."""
        mock_session.return_value = RemoteExit(0)
        workspace_ssh("myws")

        mock_ssh.assert_not_called()
        args = mock_session.call_args[0][0]
        assert args == ["ssh", "myws"]


class TestMissingSsh:
    """A host without OpenSSH has to be told what is missing, and what to do."""

    @patch("devlaunch.dl.subprocess.run", side_effect=FileNotFoundError())
    def test_missing_ssh_is_its_own_error(self, _run, on_a_terminal):
        """Not DevpodNotInstalled: devpod is present, so that message misleads."""
        with pytest.raises(dl_module.SshNotInstalled) as excinfo:
            workspace_ssh("myws", command="claude")
        assert "DEVLAUNCH_NO_TTY=1" in str(excinfo.value), "the message must name the way out"

    def test_both_missing_binary_errors_share_a_base(self):
        """main() catches one type, so a new spawn helper cannot slip past it."""
        assert issubclass(dl_module.SshNotInstalled, dl_module.MissingBinary)
        assert issubclass(dl_module.DevpodNotInstalled, dl_module.MissingBinary)

    @patch("devlaunch.dl.get_workspace_ids", return_value=["myws"])
    @patch("devlaunch.dl.get_workspace_state", return_value="Running")
    @patch("devlaunch.dl.subprocess.run", side_effect=FileNotFoundError())
    def test_main_reports_it_rather_than_crashing(self, _run, _state, _ids, on_a_terminal, capsys):
        assert dl_module.main(["myws", "--", "claude"]) == dl_module.DEVPOD_MISSING_EXIT_CODE
        assert "ssh not found" in capsys.readouterr().err


class TestPayloadParity:
    """Both transports must deliver the same command to the same shell."""

    @patch("devlaunch.dl.run_ssh")
    def test_login_shell_wrapper_survives(self, mock_ssh, on_a_terminal):
        """PATH entries from ~/.profile matter just as much over ssh."""
        mock_ssh.return_value = MagicMock(returncode=0)
        workspace_ssh("myws", command="claude")
        assert ssh_args(mock_ssh)[-1] == "bash -lc claude"

    @patch("devlaunch.dl.run_ssh")
    def test_quoted_prompt_reaches_the_agent_intact(self, mock_ssh, on_a_terminal):
        """`aid repo fix the bug` becomes one quoted argument; it must stay one."""
        mock_ssh.return_value = MagicMock(returncode=0)
        workspace_ssh("myws", command="claude 'fix the bug'")
        assert ssh_args(mock_ssh)[-1] == "bash -lc 'claude '\"'\"'fix the bug'\"'\"''"

    @patch("devlaunch.dl.run_ssh")
    def test_workdir_travels_as_a_cd(self, mock_ssh, on_a_terminal):
        mock_ssh.return_value = MagicMock(returncode=0)
        workspace_ssh("myws", command="make", workdir="/workspaces/myws")
        assert ssh_args(mock_ssh)[-1] == "cd /workspaces/myws && bash -lc make"

    @patch("devlaunch.dl.run_ssh")
    @pytest.mark.parametrize("code", [0, 1, 42, 130])
    def test_exit_code_propagates(self, mock_ssh, on_a_terminal, code):
        """A failing command in the workspace has to fail `dl` too."""
        mock_ssh.return_value = MagicMock(returncode=code)
        assert workspace_ssh("myws", command="false") == code


class TestTokenForwarding:
    """The gh login has to survive the change of transport."""

    @pytest.fixture
    def host_is_logged_in(self, monkeypatch):
        """Opt back in to forwarding, the way the gh_auth tests do."""
        monkeypatch.delenv(gh_auth.DISABLE_VAR, raising=False)
        monkeypatch.setenv("GH_TOKEN", "gho_secretvalue")
        gh_auth.resolve_token.cache_clear()
        yield
        gh_auth.resolve_token.cache_clear()

    @patch("devlaunch.dl.run_ssh")
    def test_token_named_in_argv_and_carried_in_the_environment(
        self, mock_ssh, on_a_terminal, host_is_logged_in
    ):
        """ps must never show the token, so only the variable name is an argument."""
        mock_ssh.return_value = MagicMock(returncode=0)

        workspace_ssh("myws", command="claude")

        args = ssh_args(mock_ssh)
        assert "SendEnv=GH_TOKEN" in args
        assert not any("gho_secretvalue" in arg for arg in args)

        env = mock_ssh.call_args[1]["env"]
        assert env["GH_TOKEN"] == "gho_secretvalue"

    @patch("devlaunch.dl.run_ssh")
    def test_no_token_means_no_send_env(self, mock_ssh, on_a_terminal):
        """The autouse fixture disables forwarding, so there is nothing to send."""
        mock_ssh.return_value = MagicMock(returncode=0)

        workspace_ssh("myws", command="claude")

        assert not any("SendEnv" in arg for arg in ssh_args(mock_ssh))


class TestAidReachesTheTtyTransport:
    """aid is only a command line rewrite, so it must land on the same path."""

    @patch("devlaunch.dl.run_ssh")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.get_workspace_state", return_value="Running")
    @patch("devlaunch.dl.get_workspace_ids", return_value=["myws"])
    def test_aid_with_no_prompt_starts_an_interactive_agent(
        self, _ids, _state, _up, mock_ssh, on_a_terminal
    ):
        from devlaunch import aid

        mock_ssh.return_value = MagicMock(returncode=0)
        aid.main(["myws"])

        args = ssh_args(mock_ssh)
        assert "-t" in args
        assert args[-1] == "bash -lc 'claude --dangerously-skip-permissions'"

    @patch("devlaunch.dl.run_ssh")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.get_workspace_state", return_value="Running")
    @patch("devlaunch.dl.get_workspace_ids", return_value=["myws"])
    def test_aid_with_a_prompt_still_gets_a_terminal(
        self, _ids, _state, _up, mock_ssh, on_a_terminal
    ):
        """The prompt seeds the session; the session still has to be interactive."""
        from devlaunch import aid

        mock_ssh.return_value = MagicMock(returncode=0)
        aid.main(["myws", "fix", "the", "bug"])

        args = ssh_args(mock_ssh)
        assert "-t" in args
        assert "fix the bug" in args[-1]
