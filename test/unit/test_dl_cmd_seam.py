"""The DEVLAUNCH_DL_CMD / DEVLAUNCH_AID_CMD seams (#252 §1).

The acceptance harness judges *a binary*, and which binary is a run-time
parameter: unset, the suite tests the Python implementation it always tested;
set, the same tests judge whatever command the variable names — the Rust `dl`
during the port. These tests pin the seam itself, so every spawn site that
routes through it inherits the contract.
"""

import subprocess
import sys
from unittest.mock import patch

from fixtures.e2e_helpers import (
    DLRunner,
    aid_command,
    aid_is_python,
    dl_command,
    dl_is_python,
)


class TestDlCommand:
    def test_default_is_this_interpreters_devlaunch(self, monkeypatch):
        monkeypatch.delenv("DEVLAUNCH_DL_CMD", raising=False)
        assert dl_command() == [sys.executable, "-m", "devlaunch.dl"]

    def test_override_names_a_binary(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "/opt/devlaunch/target/release/dl")
        assert dl_command() == ["/opt/devlaunch/target/release/dl"]

    def test_override_may_carry_arguments(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "cargo run -q --bin dl --")
        assert dl_command() == ["cargo", "run", "-q", "--bin", "dl", "--"]

    def test_override_respects_shell_quoting(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "'/tmp/a dir/dl' --flag")
        assert dl_command() == ["/tmp/a dir/dl", "--flag"]

    def test_empty_override_is_unset(self, monkeypatch):
        # An empty string is a shell artifact (`DEVLAUNCH_DL_CMD= pytest ...`),
        # not a request to run the empty command.
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "")
        assert dl_command() == [sys.executable, "-m", "devlaunch.dl"]

    def test_whitespace_only_override_is_unset(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "   ")
        assert dl_command() == [sys.executable, "-m", "devlaunch.dl"]


class TestAidCommand:
    def test_default_is_this_interpreters_aid(self, monkeypatch):
        monkeypatch.delenv("DEVLAUNCH_AID_CMD", raising=False)
        assert aid_command() == [sys.executable, "-m", "devlaunch.aid"]

    def test_override(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_AID_CMD", "/usr/local/bin/aid")
        assert aid_command() == ["/usr/local/bin/aid"]

    def test_aid_ignores_the_dl_override(self, monkeypatch):
        # Two entry points, two seams: pointing the harness at a Rust dl must
        # not silently redirect aid, which may not exist yet on that side.
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "/opt/rust/dl")
        monkeypatch.delenv("DEVLAUNCH_AID_CMD", raising=False)
        assert aid_command() == [sys.executable, "-m", "devlaunch.aid"]


class TestWhichImplementationIsUnderTest:
    """The one switch a divergence-row branch is allowed to read.

    Derived from the same variable as `dl_command`, so a test cannot be told one
    thing by the seam and another by the switch.
    """

    def test_unset_means_python(self, monkeypatch):
        monkeypatch.delenv("DEVLAUNCH_DL_CMD", raising=False)
        assert dl_is_python() is True

    def test_a_named_binary_is_not_python(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "/opt/devlaunch/target/release/dl")
        assert dl_is_python() is False

    def test_blank_is_unset_here_too(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "   ")
        assert dl_is_python() is True

    def test_aid_has_its_own_switch(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "/opt/rust/dl")
        monkeypatch.delenv("DEVLAUNCH_AID_CMD", raising=False)
        assert dl_is_python() is False
        assert aid_is_python() is True


class TestDLRunnerRoutesThroughSeam:
    def test_runner_spawns_the_seam_command(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "/somewhere/else/dl --quiet")
        recorded = {}

        def record(cmd, **_kwargs):
            recorded["cmd"] = cmd
            return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

        with patch("fixtures.e2e_helpers.subprocess.run", side_effect=record):
            DLRunner().run("--ls")

        assert recorded["cmd"] == ["/somewhere/else/dl", "--quiet", "--ls"]
