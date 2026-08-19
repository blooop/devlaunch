"""Pytest harness for the fake `devpod` PATH shim (#252 §3).

The program itself is `test/fixtures/devpod_shim.py` — standalone and
implementation-blind. This module is the pytest side: a fixture that
materializes it as an executable named `devpod` in a scratch directory, an
environment that resolves it first, and typed access to its three channels.
"""

import json
import os
import stat
import sys
from pathlib import Path
from typing import Dict, List, Optional

import pytest

SHIM_PROGRAM = Path(__file__).parent / "devpod_shim.py"


class ShimHarness:
    """One test's fake devpod: where it lives and how to talk to it."""

    def __init__(self, root: Path):
        self.bin_dir = root / "bin"
        self.state_file = root / "devpod-state.json"
        self.log_file = root / "devpod-log.jsonl"
        self.config_file = root / "devpod-config.json"
        self.bin_dir.mkdir(parents=True)
        executable = self.bin_dir / "devpod"
        # A wrapper rather than a copy, so the program under test is the one in
        # the tree; the interpreter is pinned to this suite's own, so the shim
        # never depends on what `python3` means on the machine's PATH.
        executable.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{SHIM_PROGRAM}" "$@"\n')
        executable.chmod(executable.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    def env(self, base: Optional[Dict[str, str]] = None) -> Dict[str, str]:
        """An environment in which `devpod` is the shim.

        Built on the caller's (or the current) environment so the isolation
        the suite already establishes — XDG_CACHE_HOME, DEVPOD_HOME — rides
        along; only PATH and the shim's channels are added.
        """
        env = dict(os.environ if base is None else base)
        env["PATH"] = f"{self.bin_dir}{os.pathsep}{env.get('PATH', '')}"
        env["DEVPOD_SHIM_STATE"] = str(self.state_file)
        env["DEVPOD_SHIM_LOG"] = str(self.log_file)
        env["DEVPOD_SHIM_CONFIG"] = str(self.config_file)
        return env

    def calls(self) -> List[List[str]]:
        """Every devpod invocation so far, oldest first, as argv lists."""
        if not self.log_file.exists():
            return []
        return [json.loads(line)["argv"] for line in self.log_file.read_text().splitlines()]

    def set_responses(self, responses: List[dict]) -> None:
        """Install the argv→response table (first matching prefix wins)."""
        self.config_file.write_text(json.dumps({"responses": responses}))

    def seed_workspace(
        self,
        workspace_id: str,
        *,
        source: str,
        state: str = "Running",
        provider: str = "docker",
    ) -> None:
        """Put a workspace into the machine as if an earlier `up` made it."""
        data = {"workspaces": {}, "providers": {"docker": {"config": {"name": "docker"}}}}
        if self.state_file.exists():
            data = json.loads(self.state_file.read_text())
        source_object = (
            {"gitRepository": source}
            if "://" in source or source.startswith("git@")
            else {"localFolder": source}
        )
        data.setdefault("workspaces", {})[workspace_id] = {
            "id": workspace_id,
            "source": source_object,
            "lastUsed": "2026-01-01T00:00:00+0000",
            "provider": {"name": provider},
            "ide": {"name": "none"},
            "context": "default",
            "state": state,
        }
        self.state_file.write_text(json.dumps(data))


@pytest.fixture
def devpod_shim(tmp_path) -> ShimHarness:
    """A fake devpod on PATH, with its state, log, and response table."""
    return ShimHarness(tmp_path / "devpod-shim")
