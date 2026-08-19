"""The fake `devpod` PATH shim (#252 §3), tested as the program it is.

The shim re-homes `test/fixtures/devpod_mock.py`'s design — call recorder,
argv→response table, workspace state machine — as a standalone executable, so
it can stand on PATH in front of *any* dl implementation. Every test here
spawns it as a separate process, because that is the only way it is ever used.

What the shim must be faithful to is real devpod, not the old in-process mock:
Tier 2 (real-devpod e2e) exists to prove the shim never drifts, and these
tests pin the shim's side of that bargain — output shapes dl actually parses
(`list --output json`, `status --output json`, `provider list --output json`)
and honest non-zero exits for workspaces devpod would not know.
"""

# Requesting a fixture shadows its name; that is how pytest is written.
# pylint: disable=redefined-outer-name

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

SHIM = Path(__file__).parent.parent / "fixtures" / "devpod_shim.py"


class ShimProc:
    """Run the shim program once, with its three env-var channels wired."""

    def __init__(self, tmp_path: Path):
        self.state_file = tmp_path / "shim-state.json"
        self.log_file = tmp_path / "shim-log.jsonl"
        self.config_file = tmp_path / "shim-config.json"

    def env(self) -> dict:
        env = dict(os.environ)
        env["DEVPOD_SHIM_STATE"] = str(self.state_file)
        env["DEVPOD_SHIM_LOG"] = str(self.log_file)
        if self.config_file.exists():
            env["DEVPOD_SHIM_CONFIG"] = str(self.config_file)
        return env

    def run(self, *args: str, env=None) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, str(SHIM), *args],
            env=self.env() if env is None else env,
            capture_output=True,
            text=True,
            check=False,
        )

    def configure(self, responses) -> None:
        self.config_file.write_text(json.dumps({"responses": responses}))

    def calls(self):
        if not self.log_file.exists():
            return []
        return [json.loads(line)["argv"] for line in self.log_file.read_text().splitlines()]


@pytest.fixture
def shim(tmp_path) -> ShimProc:
    return ShimProc(tmp_path)


class TestBasics:
    def test_version_runs_without_state(self):
        env = dict(os.environ)
        env.pop("DEVPOD_SHIM_STATE", None)
        result = subprocess.run(
            [sys.executable, str(SHIM), "version"],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0
        assert result.stdout.strip()

    def test_stateful_command_without_state_file_fails_loud(self, shim):
        env = shim.env()
        del env["DEVPOD_SHIM_STATE"]
        result = shim.run("list", "--output", "json", env=env)
        assert result.returncode == 78  # EX_CONFIG: broken harness, not empty machine
        assert "DEVPOD_SHIM_STATE" in result.stderr

    def test_unknown_command_fails(self, shim):
        result = shim.run("frobnicate")
        assert result.returncode != 0
        assert "frobnicate" in result.stderr


class TestStateMachine:
    def test_empty_machine_lists_empty_json_array(self, shim):
        result = shim.run("list", "--output", "json")
        assert result.returncode == 0
        assert json.loads(result.stdout) == []

    def test_up_creates_a_running_workspace(self, shim, tmp_path):
        src = tmp_path / "clone"
        src.mkdir()
        up = shim.run("up", str(src), "--id", "my-ws", "--ide", "none")
        assert up.returncode == 0

        listed = json.loads(shim.run("list", "--output", "json").stdout)
        assert [ws["id"] for ws in listed] == ["my-ws"]
        entry = listed[0]
        # The shape dl parses: source object, provider/ide objects, no state
        # key — real `devpod list` does not answer state, `devpod status` does.
        assert entry["source"] == {"localFolder": str(src)}
        assert entry["provider"]["name"]
        assert entry["ide"]["name"] == "none"
        assert entry["context"] == "default"
        assert entry["lastUsed"]
        assert "state" not in entry

        status = json.loads(shim.run("status", "my-ws", "--output", "json").stdout)
        assert status["id"] == "my-ws"
        assert status["state"] == "Running"

    def test_up_with_a_url_source_is_a_git_repository(self, shim):
        shim.run("up", "https://github.com/blooop/devlaunch.git", "--id", "w")
        listed = json.loads(shim.run("list", "--output", "json").stdout)
        assert listed[0]["source"] == {"gitRepository": "https://github.com/blooop/devlaunch.git"}

    def test_up_without_id_derives_one_from_the_source(self, shim):
        shim.run("up", "https://github.com/blooop/Dev.Launch.git")
        listed = json.loads(shim.run("list", "--output", "json").stdout)
        assert [ws["id"] for ws in listed] == ["dev-launch"]

    def test_stop_and_restart(self, shim):
        shim.run("up", "https://example.com/a/b.git", "--id", "w")
        assert shim.run("stop", "w").returncode == 0
        assert json.loads(shim.run("status", "w", "--output", "json").stdout)["state"] == "Stopped"
        assert shim.run("up", "https://example.com/a/b.git", "--id", "w").returncode == 0
        assert json.loads(shim.run("status", "w", "--output", "json").stdout)["state"] == "Running"

    def test_delete_removes_the_workspace(self, shim):
        shim.run("up", "https://example.com/a/b.git", "--id", "w")
        assert shim.run("delete", "w", "--force").returncode == 0
        assert json.loads(shim.run("list", "--output", "json").stdout) == []

    def test_unknown_workspaces_are_errors_like_real_devpod(self, shim):
        for argv in (["stop", "ghost"], ["delete", "ghost"], ["status", "ghost"], ["ssh", "ghost"]):
            result = shim.run(*argv)
            assert result.returncode == 1, argv
            assert "ghost" in result.stderr, argv

    def test_state_survives_across_processes(self, shim):
        shim.run("up", "https://example.com/a/b.git", "--id", "w")
        # A brand-new process, same state file: the machine remembers.
        listed = json.loads(shim.run("list", "--output", "json").stdout)
        assert [ws["id"] for ws in listed] == ["w"]

    def test_ssh_touches_a_known_workspace(self, shim):
        shim.run("up", "https://example.com/a/b.git", "--id", "w")
        shim.run("stop", "w")
        result = shim.run("ssh", "w", "--command", "true")
        assert result.returncode == 0
        # Real devpod starts a stopped workspace to ssh into it.
        assert json.loads(shim.run("status", "w", "--output", "json").stdout)["state"] == "Running"


class TestPlumbing:
    def test_every_call_lands_in_the_log_in_order(self, shim):
        shim.run("version")
        shim.run("up", "https://example.com/a/b.git", "--id", "w")
        shim.run("list", "--output", "json")
        calls = shim.calls()
        assert calls[0] == ["version"]
        assert calls[1][0] == "up"
        assert calls[2] == ["list", "--output", "json"]

    def test_response_table_beats_the_state_machine(self, shim):
        shim.configure(
            [
                {
                    "prefix": ["list", "--output", "json"],
                    "returncode": 0,
                    "stdout": "this is not json",
                }
            ]
        )
        result = shim.run("list", "--output", "json")
        assert result.returncode == 0
        assert result.stdout == "this is not json"

    def test_response_table_injects_failures(self, shim):
        shim.configure([{"prefix": ["up"], "returncode": 1, "stderr": "provider docker not found"}])
        result = shim.run("up", "https://example.com/a/b.git", "--id", "w")
        assert result.returncode == 1
        assert "provider docker not found" in result.stderr
        # And the failure changed nothing: the machine never saw the up.
        assert json.loads(shim.run("list", "--output", "json").stdout) == []

    def test_configured_misses_fall_through_to_the_machine(self, shim):
        shim.configure([{"prefix": ["stop"], "returncode": 1, "stderr": "no"}])
        assert shim.run("list", "--output", "json").returncode == 0

    def test_provider_list_answers_by_name(self, shim):
        result = shim.run("provider", "list", "--output", "json")
        assert result.returncode == 0
        assert "docker" in json.loads(result.stdout)

    def test_provider_add_registers(self, shim):
        assert shim.run("provider", "add", "kubernetes").returncode == 0
        assert "kubernetes" in json.loads(shim.run("provider", "list", "--output", "json").stdout)

    def test_context_options_answer_is_an_object(self, shim):
        result = shim.run("context", "options", "--output", "json")
        assert result.returncode == 0
        assert isinstance(json.loads(result.stdout), dict)

    def test_plain_list_is_human_text(self, shim):
        shim.run("up", "https://example.com/a/b.git", "--id", "w")
        result = shim.run("list")
        assert result.returncode == 0
        assert "w" in result.stdout
