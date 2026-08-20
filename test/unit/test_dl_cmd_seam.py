"""The DEVLAUNCH_DL_CMD / DEVLAUNCH_AID_CMD seams (#252 §1).

The acceptance harness judges *a binary*, and which binary is a run-time
parameter: unset, the suite tests the release binaries this checkout builds; set,
the same tests judge whatever command the variable names. These tests pin the
seam itself, so every spawn site that routes through it inherits the contract.

The seam outlived what it was built for. It existed so one suite could judge the
Python build and the Rust one alternately during the port (#252); the Python build
is gone (#267) and the seam stays, because "which build am I testing" is still a
question worth being able to answer -- a debug build, a `cargo run`, an installed
release, the wheel's binary.
"""

import subprocess
from pathlib import Path
from unittest.mock import patch

import pytest

from fixtures.e2e_helpers import (
    DLRunner,
    aid_command,
    dl_command,
)

REPO_ROOT = Path(__file__).resolve().parent.parent.parent

# `pytest.fail` raises this; named so the `raises` below reads as the outcome it
# is (a failed test) rather than as an exception type from nowhere.
Failed = pytest.fail.Exception


class TestTheDefaultIsTheReleaseBinary:
    """Unset, the seam names `cargo build --release`'s output and nothing else."""

    def test_default_is_the_release_binary(self, monkeypatch, tmp_path):
        monkeypatch.delenv("DEVLAUNCH_DL_CMD", raising=False)
        monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path))
        built = tmp_path / "release" / "dl"
        built.parent.mkdir(parents=True)
        built.touch()
        assert dl_command() == [str(built)]

    def test_a_redirected_target_dir_is_honoured(self, monkeypatch, tmp_path):
        """cargo honours CARGO_TARGET_DIR, so the harness must not look elsewhere."""
        monkeypatch.delenv("DEVLAUNCH_DL_CMD", raising=False)
        monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "elsewhere"))
        built = tmp_path / "elsewhere" / "release" / "aid"
        built.parent.mkdir(parents=True)
        built.touch()
        assert aid_command() == [str(built)]

    def test_a_missing_binary_fails_the_test_that_asked(self, monkeypatch, tmp_path):
        """Not skipped, and not built on the spot.

        Skipping would report nothing and pass. Building would put a compile inside
        whichever test asked first -- including the ones that measure time, and
        including several at once under `-n auto` -- and would leave the suite
        judging a binary it built from whatever the tree said at that moment.
        """
        monkeypatch.delenv("DEVLAUNCH_DL_CMD", raising=False)
        monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "never-built"))
        with pytest.raises(Failed) as refused:
            dl_command()
        assert "cargo build --release" in str(refused.value)


class TestTheOverride:
    def test_override_names_a_binary(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "/opt/devlaunch/target/release/dl")
        assert dl_command() == ["/opt/devlaunch/target/release/dl"]

    def test_override_may_carry_arguments(self, monkeypatch):
        """This is how a debug build is tested without building a release one."""
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "cargo run -q --bin dl --")
        assert dl_command() == ["cargo", "run", "-q", "--bin", "dl", "--"]

    def test_override_respects_shell_quoting(self, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "'/tmp/a dir/dl' --flag")
        assert dl_command() == ["/tmp/a dir/dl", "--flag"]

    def test_an_override_needs_no_binary_on_disk(self, monkeypatch, tmp_path):
        """The existence check is the default's, not the override's.

        An override may name something that is not a file at all -- `cargo run`,
        a wrapper script, a command on PATH -- so checking it here would refuse
        the shapes the seam exists to allow.
        """
        monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "never-built"))
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "dl-from-path")
        assert dl_command() == ["dl-from-path"]

    def test_empty_override_is_unset(self, monkeypatch, tmp_path):
        # An empty string is a shell artifact (`DEVLAUNCH_DL_CMD= pytest ...`),
        # not a request to run the empty command.
        monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path))
        built = tmp_path / "release" / "dl"
        built.parent.mkdir(parents=True)
        built.touch()
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "")
        assert dl_command() == [str(built)]

    def test_whitespace_only_override_is_unset(self, monkeypatch, tmp_path):
        monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path))
        built = tmp_path / "release" / "dl"
        built.parent.mkdir(parents=True)
        built.touch()
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "   ")
        assert dl_command() == [str(built)]


class TestTwoEntryPointsTwoSeams:
    def test_aid_ignores_the_dl_override(self, monkeypatch, tmp_path):
        """Pointing the harness at one binary must not silently redirect the other."""
        monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path))
        built = tmp_path / "release" / "aid"
        built.parent.mkdir(parents=True)
        built.touch()
        monkeypatch.setenv("DEVLAUNCH_DL_CMD", "/opt/rust/dl")
        monkeypatch.delenv("DEVLAUNCH_AID_CMD", raising=False)
        assert dl_command() == ["/opt/rust/dl"]
        assert aid_command() == [str(built)]


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
