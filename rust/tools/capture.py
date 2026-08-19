#!/usr/bin/env python3
"""Capture what the frozen Python build prints for one launch, in a scratch world.

    capture.py [--fixture=--warm ...] -- <dl args...>

Reproduces rust/dl/tests/launch.rs's World: same scenario builder, same env, same
{ROOT} templating, same shim log filtering.
"""

import json
import os
import pathlib
import subprocess
import sys
import tempfile

REPO = pathlib.Path(__file__).resolve().parents[2]
PY = REPO / ".pixi/envs/default/bin/python"
SCENARIO = REPO / "rust/dl/tests/launch_scenario.py"
LIFECYCLE_SCENARIO = REPO / "rust/dl/tests/lifecycle_scenario.py"
SHIM = REPO / "test/fixtures/devpod_shim.py"


def main(argv):
    fixtures = []
    args = []
    seen = False
    scenario = SCENARIO
    module = "devlaunch.dl"
    for a in argv:
        if a == "--":
            seen = True
            continue
        if not seen:
            if a == "--lifecycle":
                scenario = LIFECYCLE_SCENARIO
                continue
            if a == "--aid":
                module = "devlaunch.aid"
                continue
            fixtures.append(a)
        else:
            args.append(a)

    root = pathlib.Path(tempfile.mkdtemp(prefix="dlt", dir="/tmp"))
    # Fixed-length scratch root, matching tempfile::Builder rand_bytes(6).
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
    }
    for extra in os.environ.get("CAPTURE_ENV", "").split(","):
        if extra:
            name, _, value = extra.partition("=")
            env[name] = value
    expanded = [a.replace("{ROOT}", str(root)) for a in args]
    done = subprocess.run(
        [str(PY), "-m", module, *expanded],
        cwd=str(REPO),
        env={**env, "PYTHONPATH": str(REPO)},
        capture_output=True,
        text=True,
        check=False,
    )

    def t(text):
        return text.replace(str(root), "{ROOT}")

    print("== exit:", done.returncode)
    print("== stdout:")
    for line in t(done.stdout).splitlines():
        print(f"  |{line}")
    print("== stderr:")
    for line in t(done.stderr).splitlines():
        print(f"  |{line}")
    print("== devpod calls:")
    log = root / "shim-log.jsonl"
    if log.exists():
        for line in log.read_text().splitlines():
            call = json.loads(line)
            words = []
            for word in call["argv"]:
                word = t(word)
                # 400, matching compare.py's WORD_LIMIT: long enough to keep the
                # `--command bash -lc <payload>` a launch sends (24 hid it).
                if len(word) > 400:
                    word = word.splitlines()[0][:400] + "…"
                words.append(word)
            print("  devpod " + " ".join(words))
    print("== root:", root)


if __name__ == "__main__":
    main(sys.argv[1:])
