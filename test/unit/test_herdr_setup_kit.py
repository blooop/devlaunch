"""`dl --herdr-setup` on a new machine, judged through the released binary.

The Rust suites pin the merge rules (`rust/dl/src/herdr_kit/`) and each step
(`rust/dl/tests/herdr_environment.rs`). This is the one check from outside:
a home with Claude Code and kitty configured and nothing of devlaunch's, a
fake herdr on PATH that writes down every call, and the binary that ships.

Everything the run could touch is scoped to `tmp_path`: HOME, all four XDG
directories, and a PATH of the fake herdr plus the system directories, so no
real herdr, chezmoi or kitty is reachable. `CLAUDE_CONFIG_DIR` and the
`HERDR_*` variables are left out on purpose, because both point a real session
at the files a test must never write.
"""

import json
import os
import subprocess
import tomllib
from pathlib import Path

import pytest

from fixtures.e2e_helpers import dl_command

# The same contract as the stub in rust/dl/tests/herdr_environment.rs: record
# every call, remember a link and an install so a second run sees them, and
# print a skill.
FAKE_HERDR = r"""#!/bin/sh
printf '%s\n' "$*" >> "$HOME/herdr.log"
case "$1 $2" in
  "plugin list")
    if [ -f "$HOME/linked" ]; then
      printf '{"result":{"plugins":[{"plugin_id":"local.agent-queue","plugin_root":"%s"}]}}\n' "$(cat "$HOME/linked")"
    else
      printf '{"result":{"plugins":[]}}\n'
    fi ;;
  "plugin link") printf '%s' "$3" > "$HOME/linked" ;;
  "integration status")
    if [ -f "$HOME/integrated" ]; then echo 'claude: current (v10) (x)'; else echo 'claude: not installed (x)'; fi ;;
  "integration install") touch "$HOME/integrated" ;;
  "--skill ") echo '# herdr skill' ;;
  *) echo '{}' ;;
esac
"""

BOOKKEEPING = {"herdr.log", "linked", "integrated"}


@pytest.fixture
def new_machine(tmp_path: Path) -> dict:
    home = tmp_path / "home"
    (home / ".claude").mkdir(parents=True)
    (home / ".config" / "kitty").mkdir(parents=True)
    tools = tmp_path / "tools"
    tools.mkdir()
    herdr = tools / "herdr"
    herdr.write_text(FAKE_HERDR)
    herdr.chmod(0o755)
    return {
        "HOME": str(home),
        "PATH": f"{tools}:/usr/bin:/bin",
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_DATA_HOME": str(home / ".local" / "share"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "XDG_STATE_HOME": str(home / ".local" / "state"),
        "LANG": os.environ.get("LANG", "C.UTF-8"),
    }


def setup(env: dict, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [*dl_command(), "--herdr-setup", *args],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


def snapshot(home: Path) -> dict:
    files = {}
    for path in sorted(home.rglob("*")):
        name = str(path.relative_to(home))
        if name in BOOKKEEPING:
            continue
        if path.is_symlink():
            files[name] = f"-> {os.readlink(path)}"
        elif path.is_file():
            files[name] = (path.stat().st_mode & 0o777, path.read_bytes())
    return files


def test_one_command_configures_a_new_machine_and_a_second_changes_nothing(new_machine):
    home = Path(new_machine["HOME"])

    first = setup(new_machine)

    assert first.returncode == 0, first.stderr
    assert "failed" not in first.stderr, first.stderr
    kit = home / ".local/share/devlaunch/herdr"
    config = tomllib.loads((home / ".config/herdr/config.toml").read_text())
    assert config["terminal"]["default_shell"] == str(home / ".local/bin/dl-herdr-shell")
    assert config["ui"]["tab_bar_right"][0]["command"] == str(kit / "status.sh")
    assert config["ui"]["window_title"].endswith(" · herdr")
    assert any(
        entry["key"] == "prefix+a" and entry["command"] == "local.agent-queue.toggle"
        for entry in config["keys"]["command"]
    )
    assert os.access(kit / "plugins/agent-queue/view.sh", os.X_OK)
    calls = (home / "herdr.log").read_text()
    assert f"plugin link {kit / 'plugins/agent-queue'}" in calls, calls
    assert "integration install claude" in calls, calls
    settings = json.loads((home / ".claude/settings.json").read_text())
    [stop] = settings["hooks"]["Stop"]
    assert "devlaunch-herdr-tab-title.sh" in stop["hooks"][0]["command"]
    assert (home / ".claude/skills/herdr/SKILL.md").read_text() == "# herdr skill\n"
    assert "include devlaunch-herdr.conf" in (home / ".config/kitty/kitty.conf").read_text()
    assert "herdr server reload-config" in first.stderr
    assert "DEVLAUNCH_HERDR=1" in first.stderr

    before = snapshot(home)
    (home / "herdr.log").unlink()
    second = setup(new_machine)

    assert second.returncode == 0, second.stderr
    assert snapshot(home) == before
    assert "changed" not in second.stderr, second.stderr
    calls = (home / "herdr.log").read_text()
    assert "plugin link" not in calls and "integration install" not in calls, calls


def test_a_dry_run_changes_nothing_on_disk(new_machine):
    home = Path(new_machine["HOME"])
    before = snapshot(home)

    result = setup(new_machine, "--dry-run")

    assert result.returncode == 0, result.stderr
    assert snapshot(home) == before
    assert "planned  herdr config" in result.stderr, result.stderr
    assert "nothing was written" in result.stderr
