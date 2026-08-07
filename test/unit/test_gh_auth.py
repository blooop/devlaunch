"""Tests for forwarding the host's GitHub CLI credentials into workspaces."""

import os
import pathlib
import stat
import subprocess
from unittest.mock import MagicMock, patch

import pytest

from devlaunch import gh_auth


@pytest.fixture(autouse=True)
def enable_forwarding(monkeypatch):
    """Undo the suite-wide opt-out; these tests are about the feature itself."""
    monkeypatch.delenv(gh_auth.DISABLE_VAR, raising=False)
    for var in gh_auth.HOST_TOKEN_VARS:
        monkeypatch.delenv(var, raising=False)
    gh_auth.resolve_token.cache_clear()
    yield
    gh_auth.resolve_token.cache_clear()


def gh_result(stdout: str, returncode: int = 0) -> MagicMock:
    return MagicMock(returncode=returncode, stdout=stdout, stderr="")


@pytest.mark.unit
class TestResolveToken:
    """Where the token comes from."""

    def test_host_env_wins_over_the_gh_cli(self, monkeypatch):
        """An exported token is already the answer gh would give, for free."""
        monkeypatch.setenv("GH_TOKEN", "gho_fromenv")
        with patch("devlaunch.gh_auth.subprocess.run") as mock_run:
            assert gh_auth.resolve_token() == "gho_fromenv"
        mock_run.assert_not_called()

    def test_github_token_is_accepted_too(self, monkeypatch):
        monkeypatch.setenv("GITHUB_TOKEN", "ghp_fromenv")
        assert gh_auth.resolve_token() == "ghp_fromenv"

    @patch("devlaunch.gh_auth.shutil.which", return_value="/usr/bin/gh")
    @patch("devlaunch.gh_auth.subprocess.run", return_value=gh_result("gho_fromcli\n"))
    def test_falls_back_to_the_gh_cli(self, mock_run, _mock_which):
        """gh sources the token whether the host stores it in a file or keyring."""
        assert gh_auth.resolve_token() == "gho_fromcli"
        assert mock_run.call_args[0][0] == ["gh", "auth", "token"]

    @patch("devlaunch.gh_auth.shutil.which", return_value=None)
    def test_no_gh_installed_is_not_an_error(self, _mock_which):
        assert gh_auth.resolve_token() is None

    @patch("devlaunch.gh_auth.shutil.which", return_value="/usr/bin/gh")
    @patch("devlaunch.gh_auth.subprocess.run", return_value=gh_result("", returncode=1))
    def test_logged_out_host_forwards_nothing(self, _mock_run, _mock_which):
        assert gh_auth.resolve_token() is None

    @patch("devlaunch.gh_auth.shutil.which", return_value="/usr/bin/gh")
    @patch("devlaunch.gh_auth.subprocess.run", return_value=gh_result("error: not logged in\n"))
    def test_prose_on_stdout_is_not_mistaken_for_a_token(self, _mock_run, _mock_which):
        """A wrapper that prints a message must not become the workspace's GH_TOKEN."""
        assert gh_auth.resolve_token() is None

    @patch("devlaunch.gh_auth.shutil.which", return_value="/usr/bin/gh")
    @patch(
        "devlaunch.gh_auth.subprocess.run",
        side_effect=subprocess.TimeoutExpired("gh", 10),
    )
    def test_a_hung_gh_does_not_hang_the_launch(self, _mock_run, _mock_which):
        """gh may block on a locked keyring; a workspace must still open."""
        assert gh_auth.resolve_token() is None

    @patch("devlaunch.gh_auth.shutil.which", return_value="/usr/bin/gh")
    @patch("devlaunch.gh_auth.subprocess.run", return_value=gh_result("gho_cached\n"))
    def test_gh_is_only_asked_once_per_process(self, mock_run, _mock_which):
        """Asking twice can mean unlocking a keyring twice in one `dl` run."""
        assert gh_auth.resolve_token() == "gho_cached"
        assert gh_auth.resolve_token() == "gho_cached"
        assert mock_run.call_count == 1


@pytest.mark.unit
class TestOptOut:
    """The DEVLAUNCH_NO_GH_TOKEN escape hatch."""

    def test_opt_out_beats_an_available_token(self, monkeypatch):
        monkeypatch.setenv("GH_TOKEN", "gho_fromenv")
        monkeypatch.setenv(gh_auth.DISABLE_VAR, "1")
        assert gh_auth.resolve_token() is None

    @pytest.mark.parametrize("value", ["", "0", "false", "no", " NO "])
    def test_falsey_values_leave_forwarding_on(self, monkeypatch, value):
        monkeypatch.setenv(gh_auth.DISABLE_VAR, value)
        assert gh_auth.forwarding_disabled() is False

    def test_unset_leaves_forwarding_on(self):
        assert gh_auth.forwarding_disabled() is False


@pytest.mark.unit
class TestUpArgs:
    """The flags handed to `devpod up`."""

    @patch("devlaunch.gh_auth.resolve_token", return_value="gho_secret")
    def test_token_travels_in_a_file_not_in_argv(self, _mock_token):
        """`devpod up` can run for minutes, and its argv is readable by anyone."""
        with gh_auth.up_args() as args:
            assert args[0] == "--workspace-env-file"
            assert "gho_secret" not in " ".join(args)
            path = pathlib.Path(args[1])
            assert path.read_text(encoding="utf-8") == "GH_TOKEN=gho_secret\n"

    @patch("devlaunch.gh_auth.resolve_token", return_value="gho_secret")
    def test_the_file_is_private_to_the_user(self, _mock_token):
        with gh_auth.up_args() as args:
            mode = stat.S_IMODE(os.stat(args[1]).st_mode)
        assert mode & (stat.S_IRWXG | stat.S_IRWXO) == 0

    @patch("devlaunch.gh_auth.resolve_token", return_value="gho_secret")
    def test_the_file_does_not_outlive_the_command(self, _mock_token):
        with gh_auth.up_args() as args:
            path = pathlib.Path(args[1])
        assert not path.exists()

    @patch("devlaunch.gh_auth.resolve_token", return_value="gho_secret")
    def test_the_file_is_removed_even_when_devpod_raises(self, _mock_token):
        path = None
        with pytest.raises(RuntimeError):
            with gh_auth.up_args() as args:
                path = pathlib.Path(args[1])
                raise RuntimeError("devpod blew up")
        assert not path.exists()

    @patch("devlaunch.gh_auth.resolve_token", return_value=None)
    def test_no_token_adds_no_flags(self, _mock_token):
        with gh_auth.up_args() as args:
            assert args == []

    @patch("devlaunch.gh_auth.resolve_token", return_value="gho_secret")
    @patch("devlaunch.gh_auth.tempfile.mkstemp", side_effect=OSError("No space left on device"))
    def test_an_unusable_temp_dir_costs_the_login_not_the_launch(self, _mock_mkstemp, _mock_token):
        """workspace_up has no exception handler above it on several paths."""
        with gh_auth.up_args() as args:
            assert args == []


@pytest.mark.unit
class TestSshArgs:
    """The flags handed to `devpod ssh`."""

    @patch("devlaunch.gh_auth.resolve_token", return_value="gho_secret")
    def test_only_the_variable_name_reaches_argv(self, _mock_token):
        args, env = gh_auth.ssh_args_and_env()
        assert args == ["--send-env", "GH_TOKEN"]
        assert env is not None
        assert env["GH_TOKEN"] == "gho_secret"

    @patch("devlaunch.gh_auth.resolve_token", return_value="gho_secret")
    def test_the_rest_of_the_environment_is_preserved(self, _mock_token, monkeypatch):
        """env replaces devpod's whole environment, so it must carry PATH along."""
        monkeypatch.setenv("DEVLAUNCH_CANARY", "kept")
        _, env = gh_auth.ssh_args_and_env()
        assert env is not None
        assert env["DEVLAUNCH_CANARY"] == "kept"
        assert env["PATH"] == os.environ["PATH"]

    @patch("devlaunch.gh_auth.resolve_token", return_value=None)
    def test_no_token_leaves_devpod_untouched(self, _mock_token):
        """A None env means devpod simply inherits the current one."""
        assert gh_auth.ssh_args_and_env() == ([], None)
