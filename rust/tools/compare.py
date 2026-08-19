#!/usr/bin/env python3
"""Run the frozen Python dl and the Rust dl in identical worlds and diff them.

compare.py [--lifecycle] [--aid] [fixtures...] -- <args...>
"""

import json
import os
import pathlib
import subprocess
import sys
import tempfile

REPO = pathlib.Path(__file__).resolve().parents[2]
PY = REPO / ".pixi/envs/default/bin/python"
SCENARIOS = {
    "launch": REPO / "rust/dl/tests/launch_scenario.py",
    "lifecycle": REPO / "rust/dl/tests/lifecycle_scenario.py",
    "read": REPO / "rust/dl/tests/scenario.py",
}
SHIM = REPO / "test/fixtures/devpod_shim.py"


def one_run(cmd, scenario, fixtures, args, extra_env):
    root = pathlib.Path(tempfile.mkdtemp(prefix="dlt", dir="/tmp"))
    built = subprocess.run(
        ["python3", str(scenario), str(root), str(SHIM), *fixtures],
        capture_output=True,
        text=True,
        check=False,
    )
    if built.returncode != 0:
        raise SystemExit(f"scenario failed: {built.stderr}")
    env = {
        "PATH": f"{root}/bin:{root}/gh-bin:/usr/bin:/bin",
        "HOME": f"{root}/home",
        "XDG_CACHE_HOME": f"{root}/cache",
        "XDG_CONFIG_HOME": f"{root}/config",
        "DEVPOD_HOME": f"{root}/devpod",
        "DEVPOD_SHIM_STATE": f"{root}/shim-state.json",
        "DEVPOD_SHIM_LOG": f"{root}/shim-log.jsonl",
        "DEVPOD_SHIM_CONFIG": f"{root}/shim-config.json",
        "GIT_SSH_COMMAND": "false",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_SYSTEM": "/dev/null",
        **extra_env,
    }
    expanded = [a.replace("{ROOT}", str(root)) for a in args]
    done = subprocess.run(
        [*cmd, *expanded],
        cwd=str(REPO),
        env={**env, "PYTHONPATH": str(REPO)},
        capture_output=True,
        text=True,
        stdin=subprocess.DEVNULL,
        check=False,
    )

    def t(text):
        return text.replace(str(root), "{ROOT}")

    calls = []
    log = root / "shim-log.jsonl"
    if log.exists():
        for line in log.read_text().splitlines():
            argv = json.loads(line)["argv"]
            if argv and argv[0] == "list":
                continue  # the detached refresh child races with the parent
            words = []
            for word in argv:
                word = t(word)
                if len(word) > 24:
                    word = word.splitlines()[0][:24] + "…"
                words.append(word)
            calls.append("devpod " + " ".join(words))
    return {
        "exit": done.returncode,
        "stdout": t(done.stdout).splitlines(),
        "stderr": t(done.stderr).splitlines(),
        "calls": calls,
    }


def main(argv):
    fixtures, args = [], []
    seen = False
    scenario = SCENARIOS["launch"]
    aid = False
    for a in argv:
        if a == "--":
            seen = True
            continue
        if not seen:
            if a in ("--lifecycle", "--read"):
                scenario = SCENARIOS[a.lstrip("-")]
                continue
            if a == "--aid":
                aid = True
                continue
            fixtures.append(a)
        else:
            args.append(a)
    module = "devlaunch.aid" if aid else "devlaunch.dl"
    rust = REPO / "rust/target/debug" / ("aid" if aid else "dl")
    extra = {}
    for pair in os.environ.get("CAPTURE_ENV", "").split(","):
        if pair:
            name, _, value = pair.partition("=")
            extra[name] = value

    python = one_run([str(PY), "-m", module], scenario, fixtures, args, extra)
    ported = one_run([str(rust)], scenario, fixtures, args, extra)

    same = python == ported
    print(("SAME  " if same else "DIFF  ") + " ".join(fixtures + ["--"] + args))
    if not same:
        for key in ("exit", "stdout", "stderr", "calls"):
            if python[key] != ported[key]:
                print(f"  {key}:")
                print(f"    python: {python[key]}")
                print(f"    rust  : {ported[key]}")
    return 0 if same else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
