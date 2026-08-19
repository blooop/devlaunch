"""The pytest harness that puts the fake devpod on PATH (#252 §3).

`devpod_shim` is the Tier-1 fixture: a scratch bin directory holding an
executable named `devpod`, an environment that finds it first, and typed
access to the shim's three channels (state, response table, invocation log).
The last test is the point of the whole arrangement: the command under test —
whatever `dl_command()` names — runs against the shim end to end.
"""

import json
import subprocess

from fixtures.e2e_helpers import dl_command


class TestShimOnPath:
    def test_the_shim_answers_as_devpod(self, devpod_shim):
        result = subprocess.run(
            ["devpod", "version"],
            env=devpod_shim.env(),
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0
        assert "shim" in result.stdout

    def test_calls_are_recorded(self, devpod_shim):
        subprocess.run(
            ["devpod", "list", "--output", "json"],
            env=devpod_shim.env(),
            capture_output=True,
            check=False,
        )
        assert devpod_shim.calls() == [["list", "--output", "json"]]

    def test_seeded_workspaces_are_listed(self, devpod_shim):
        devpod_shim.seed_workspace("pre-existing", source="https://example.com/o/r.git")
        result = subprocess.run(
            ["devpod", "list", "--output", "json"],
            env=devpod_shim.env(),
            capture_output=True,
            text=True,
            check=False,
        )
        assert [ws["id"] for ws in json.loads(result.stdout)] == ["pre-existing"]

    def test_configured_failures_apply(self, devpod_shim):
        devpod_shim.set_responses([{"prefix": ["list"], "returncode": 1, "stderr": "boom"}])
        result = subprocess.run(
            ["devpod", "list", "--output", "json"],
            env=devpod_shim.env(),
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 1
        assert "boom" in result.stderr


class TestDlAgainstShim:
    """The command under test, judged with the shim as its whole devpod."""

    def test_ls_with_no_workspaces(self, devpod_shim):
        result = subprocess.run(
            [*dl_command(), "--ls"],
            env=devpod_shim.env(),
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0, result.stderr
        assert "No workspaces found" in result.stdout
        # And it really was the shim that answered.
        assert ["list", "--output", "json"] in devpod_shim.calls()

    def test_ls_shows_a_seeded_workspace(self, devpod_shim):
        devpod_shim.seed_workspace("seeded-ws", source="https://example.com/o/r.git")
        result = subprocess.run(
            [*dl_command(), "--ls"],
            env=devpod_shim.env(),
            capture_output=True,
            text=True,
            check=False,
        )
        assert result.returncode == 0, result.stderr
        assert "seeded-ws" in result.stdout
