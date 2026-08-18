#!/usr/bin/env python3
"""Build the one world the `dl` binary's read-side tests are judged against.

Stdlib-only and implementation-blind, like `test/fixtures/devpod_shim.py`: it is
run by the Rust integration test *and* by the golden-capture harness that runs
the Python build against the same tree, so neither implementation can be the
thing that defines the fixture. It writes, under the root it is given:

- `cache/devlaunch/` — the devlaunch cache (`XDG_CACHE_HOME=<root>/cache`),
  holding `metadata.json` (schema 2) and the workspace clones under `repos/`.
- `bin/devpod` — the fake devpod, a two-line shell wrapper around the shim, with
  `DEVPOD_SHIM_STATE` pointing at `<root>/shim-state.json`.
- five workspaces in that state file, one per shape the listing has to describe:
  a devlaunch clone with a record and nothing to lose, a devlaunch clone with no
  record holding uncommitted work, a foreign local folder never opened, a git
  source, and a source devlaunch cannot read at all.
- a bare clone at `repos/blooop/devlaunch/.bare` so repo discovery and the
  local-branch half of the completion cache have something deterministic to find.

Every path it writes is under the root, so a test can substitute the root for a
placeholder and compare bytes against a golden captured on another machine.
"""

import json
import os
import pathlib
import subprocess
import sys

# Frozen so the `LAST USED` column and the `lastUsed` field are the same bytes on
# every machine. The empty one is devpod's answer for a workspace it has never
# opened, which the table spells `never`.
RECORDED = "2026-08-01T10:11:12+0000"
OLDER = "2026-07-30T09:08:07+0000"
NEVER = ""

# The ids, in the order `devpod list` answers them (the shim keeps insertion
# order, and so does the listing).
WITH_RECORD = "blooop-devlaunch-main-4f3a2b1c"
WITHOUT_RECORD = "blooop-other-feature-9e8d7c6b"
FOREIGN = "someones-project"
FROM_GIT = "devpod-upstream"
UNREADABLE = "an-image-workspace"


def git(cwd, *args):
    """Run git in *cwd*, insisting it worked."""
    done = subprocess.run(
        ["git", *args],
        cwd=str(cwd),
        capture_output=True,
        text=True,
        check=False,
        env={
            **os.environ,
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_SYSTEM": "/dev/null",
            "GIT_AUTHOR_NAME": "t",
            "GIT_AUTHOR_EMAIL": "t@t",
            "GIT_COMMITTER_NAME": "t",
            "GIT_COMMITTER_EMAIL": "t@t",
        },
    )
    if done.returncode != 0:
        raise SystemExit(f"git {args} in {cwd} failed: {done.stderr}")
    return done.stdout.strip()


def build(root: pathlib.Path, shim: pathlib.Path) -> None:
    cache = root / "cache" / "devlaunch"
    repos = cache / "repos"
    for path in (
        root / "config",
        root / "home",
        root / "devpod",
        root / "bin",
        repos,
        root / "foreign",
    ):
        path.mkdir(parents=True, exist_ok=True)

    # The fake devpod, on PATH under its real name.
    devpod = root / "bin" / "devpod"
    devpod.write_text(
        f'#!/bin/sh\nexec "{sys.executable}" "{shim}" "$@"\n',
        encoding="utf-8",
    )
    devpod.chmod(0o755)

    # A bare repository standing in for GitHub, with one commit on `main`.
    seed = root / "seed"
    seed.mkdir(parents=True, exist_ok=True)
    git(seed, "init", "-q", "-b", "main", ".")
    (seed / "README.md").write_text("seed\n", encoding="utf-8")
    git(seed, "add", "-A")
    git(seed, "commit", "-q", "-m", "seed")
    origin = root / "origin.git"
    git(root, "clone", "-q", "--bare", "seed", "origin.git")

    # The bare clone the cache keeps for blooop/devlaunch, which is what repo
    # discovery and the local-branch lookup read.
    bare = repos / "blooop" / "devlaunch" / ".bare"
    bare.parent.mkdir(parents=True, exist_ok=True)
    git(root, "clone", "-q", "--bare", str(origin), str(bare))

    # 1. A devlaunch clone with a record, clean, its branch pushed: nothing to
    #    lose, and the `checkedOut` branch is the recorded one.
    recorded_clone = repos / "blooop" / "devlaunch" / WITH_RECORD
    git(root, "clone", "-q", str(origin), str(recorded_clone))
    git(recorded_clone, "checkout", "-q", "-B", "main")

    # 2. A devlaunch clone with no record, holding an uncommitted file: dl's own,
    #    so it is measured and inspected, but with no repo/branch to report.
    unrecorded_clone = repos / "blooop" / "other" / WITHOUT_RECORD
    unrecorded_clone.parent.mkdir(parents=True, exist_ok=True)
    git(root, "clone", "-q", str(origin), str(unrecorded_clone))
    git(unrecorded_clone, "checkout", "-q", "-b", "feature")
    (unrecorded_clone / "scratch.txt").write_text("unsaved\n", encoding="utf-8")

    # 3. Somebody else's project directory, outside the cache.
    foreign = root / "foreign" / "proj"
    foreign.mkdir(parents=True, exist_ok=True)
    (foreign / "notes.txt").write_text("not devlaunch's\n", encoding="utf-8")

    # metadata.json, schema 2, with the one record.
    metadata = {
        "version": 2,
        "repositories": {},
        "worktrees": {
            "blooop/devlaunch/main": {
                "owner": "blooop",
                "repo": "devlaunch",
                "branch": "main",
                "local_path": str(recorded_clone),
                "workspace_id": WITH_RECORD,
                "created_at": "2026-07-01T00:00:00",
                "last_used": "2026-07-01T00:00:00",
                "devpod_workspace_id": WITH_RECORD,
            }
        },
    }
    (cache / "metadata.json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8"
    )

    # The fake devpod's workspaces, in listing order.
    state = {
        "providers": {"docker": {"config": {"name": "docker"}}},
        "workspaces": {
            WITH_RECORD: _workspace(
                WITH_RECORD, {"localFolder": str(recorded_clone)}, RECORDED, "Running"
            ),
            WITHOUT_RECORD: _workspace(
                WITHOUT_RECORD,
                {"localFolder": str(unrecorded_clone)},
                OLDER,
                "Stopped",
            ),
            FOREIGN: _workspace(
                FOREIGN, {"localFolder": str(foreign)}, NEVER, "Stopped"
            ),
            FROM_GIT: _workspace(
                FROM_GIT,
                {"gitRepository": "https://github.com/loft-sh/devpod.git"},
                RECORDED,
                "Running",
            ),
            UNREADABLE: _workspace(
                UNREADABLE, {"image": "ubuntu:22.04"}, OLDER, "Stopped"
            ),
        },
    }
    (root / "shim-state.json").write_text(
        json.dumps(state, indent=1), encoding="utf-8"
    )


def _workspace(workspace_id, source, last_used, state):
    return {
        "id": workspace_id,
        "source": source,
        "lastUsed": last_used,
        "provider": {"name": "docker"},
        "ide": {"name": "none"},
        "context": "default",
        "state": state,
    }


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: scenario.py <root> <devpod-shim.py>")
    build(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]).resolve())
