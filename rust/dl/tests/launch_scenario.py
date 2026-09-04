#!/usr/bin/env python3
"""Build the worlds the launch verbs are judged against.

Stdlib-only and implementation-blind, like `tests/scenario.py`,
`tests/lifecycle_scenario.py` and `test/fixtures/devpod_shim.py`: it is run by the
Rust integration test *and* by the golden-capture harness that runs the frozen
Python build against the same tree, so neither implementation can be the thing
that defines the fixture.

    launch_scenario.py <root> <devpod_shim.py> [--warm] [--stopped] [--gh]
                                              [--no-devpod] [--no-workspaces]
                                              [--stale-checkout] [--dotfiles]

The base world, under the root it is given:

- `bin/devpod` — the fake devpod, a two-line shell wrapper around the shim.
- `cache/devlaunch/` — the devlaunch cache (`XDG_CACHE_HOME=<root>/cache`), with
  `metadata.json` at schema 2 and the clones under `repos/`.
- a bare clone at `repos/blooop/devlaunch/.bare` whose **recorded remote is a
  local `origin.git`**, so every git call a cold launch makes — the fetch, the
  branch, the workspace clone — happens offline and answers the same on every
  machine. `origin.git` carries two branches, `main` and `cold`.
- **no workspace clone and no worktree record for `cold`**, which is what makes
  `dl blooop/devlaunch@cold` the cold launch: devpod is asked about the derived
  id, denies it, and the host prepares the clone before `devpod up` ever runs.

The fixtures, switched on by name so that one verb's golden does not have to
explain another's world:

- `--warm`: `blooop/devlaunch@main` exists as a clone, as a record, and as a
  **Running** devpod workspace under the id this build derives. This is the
  fast-attach fixture, and the one `dl <workspace-id>` addresses by bare name.
- `--stopped`: the same workspace, **Stopped**. Every verb that has to bring a
  container up before it can do anything uses this.
- `--stale-checkout`: two further commits on `origin.git`'s `main`, fetched into
  the warm workspace's clone and *not* checked out. Only usable beside `--warm` or
  `--stopped`, whose clone it is. This is the world blooop/devlaunch#560 reports:
  the clone's `HEAD` is behind its own `refs/remotes/origin/main`, which is a fact
  the clone holds and no launch of a workspace devpod already knows goes looking
  for.
- `--dotfiles`: `devpod context options` names a dotfiles repository, which is the
  only place dl reads one from. Without it the fake devpod answers `{}`, so a
  launch forwards no `--dotfiles` flag -- which is the state a fortnight of #560
  was spent being unable to observe.
- `--gh`: a fake `gh` in `gh-bin/`, printing a token on `gh auth token`. It is a
  directory of its own so a test decides by PATH whether this host has a GitHub
  login at all — an absent `gh` is a choice and not a failure, and the two answers
  are different launches.
- `--no-devpod`: no `bin/devpod` at all, for the exit-127 line.
- `--no-workspaces`: devpod lists nothing, whatever the other fixtures recorded.
- `--fail-up` / `--fail-stop`: the fake devpod refuses that subcommand, with a
  status of its own (7 and 9), which is the number dl has to hand back.
- `--symlinked-path`: `foreign/link` -> `foreign/real`, the one input that tells
  divergence row 20's lexical naming from Python's `Path.resolve()`.
- `--fail-session`: `devpod ssh` refuses with 3, which is devpod's own ending.
- `--remote-exit`: `devpod ssh` exits 1 with the fatal devpod buries a remote exit
  status in, so what dl hands back is the remote program's 130 and not devpod's 1.
  Both are only usable beside `--warm`, where the fast attach makes no other ssh
  trip for the response table to answer.

Every path it writes is under the root, so a test can substitute the root for a
placeholder and compare bytes against a golden captured on another machine.
"""

import json
import os
import pathlib
import subprocess
import sys

RECORDED = "2026-08-01T10:11:12+0000"

# The workspace id this build derives for `blooop/devlaunch@main`. Written down
# rather than computed, because a fixture that derived it the way dl does could
# not tell a change in the derivation from a change in the world.
MAIN_WS = "devlaunch-main-3j1t"
MAIN_LEAF = MAIN_WS

# The branch with no clone and no record: the cold launch's.
COLD_BRANCH = "cold"

# A token of the shape every GitHub token has, so the fake `gh` is believed.
TOKEN = "gho_devlaunchtesttoken0123456789"

# What `--dotfiles` makes `devpod context options` name. Never cloned by anything
# here: the fake devpod is asked for the option and the flag is what is asserted.
DOTFILES_URL = "https://github.com/blooop/dotfiles"


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


def _executable(path, script):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(script, encoding="utf-8")
    path.chmod(0o755)


def _workspace(workspace_id, source, state):
    return {
        "id": workspace_id,
        "source": {"localFolder": str(source)},
        "lastUsed": RECORDED,
        "provider": {"name": "docker"},
        "ide": {"name": "none"},
        "context": "default",
        "state": state,
    }


def _record(owner, repo, branch, clone, devpod_id):
    return {
        "owner": owner,
        "repo": repo,
        "branch": branch,
        "local_path": str(clone),
        "workspace_id": pathlib.Path(clone).name,
        "created_at": "2026-07-01T00:00:00",
        "last_used": "2026-07-01T00:00:00",
        "devpod_workspace_id": devpod_id,
    }


def build(root: pathlib.Path, shim: pathlib.Path, wanted: set) -> None:
    cache = root / "cache" / "devlaunch"
    repos = cache / "repos"
    for path in (root / "config", root / "home", root / "devpod", root / "bin", repos):
        path.mkdir(parents=True, exist_ok=True)

    if "no-devpod" not in wanted:
        _executable(
            root / "bin" / "devpod",
            f'#!/bin/sh\nexec "{sys.executable}" "{shim}" "$@"\n',
        )

    if "gh" in wanted:
        # Only `auth token` answers; anything else exits non-zero, as a gh that
        # does not know a subcommand does.
        _executable(
            root / "gh-bin" / "gh",
            '#!/bin/sh\nif [ "$1" = "auth" ] && [ "$2" = "token" ]; then\n'
            f'  echo "{TOKEN}"\n  exit 0\nfi\n'
            'echo "gh-fake: unknown command" >&2\nexit 1\n',
        )

    if "symlinked-path" in wanted:
        # A directory reached through a symbolic link, which is the input
        # **divergence row 20** is about: `Path.resolve()` follows the link and
        # names the workspace after its target, where a lexical normalisation names
        # it after the link the user typed.
        real = root / "foreign" / "real"
        real.mkdir(parents=True, exist_ok=True)
        (real / "notes.txt").write_text("a project\n", encoding="utf-8")
        (root / "foreign" / "link").symlink_to(real)

    # A bare repository standing in for GitHub, with `main` and `cold` on it.
    seed = root / "seed"
    seed.mkdir(parents=True, exist_ok=True)
    git(seed, "init", "-q", "-b", "main", ".")
    (seed / "README.md").write_text("seed\n", encoding="utf-8")
    git(seed, "add", "-A")
    git(seed, "commit", "-q", "-m", "seed")
    git(seed, "branch", COLD_BRANCH)
    origin = root / "origin.git"
    git(root, "clone", "-q", "--bare", "seed", "origin.git")

    bare = repos / "blooop" / "devlaunch" / ".bare"
    bare.parent.mkdir(parents=True, exist_ok=True)
    git(root, "clone", "-q", "--bare", str(origin), str(bare))

    # The remote every git call resolves through is the local `origin.git`, so a
    # cold launch's fetch and clone are offline. `last_fetched` is recent enough
    # that no launch is also a fetch.
    repositories = {
        "blooop/devlaunch": {
            "owner": "blooop",
            "repo": "devlaunch",
            "remote_url": str(origin),
            "local_path": str(bare),
            "default_branch": "main",
            "last_fetched": "2026-08-01T00:00:00",
            "worktrees": [],
        }
    }
    worktrees = {}
    workspaces = {}

    if "warm" in wanted or "stopped" in wanted:
        clone = repos / "blooop" / "devlaunch" / MAIN_LEAF
        git(root, "clone", "-q", str(origin), str(clone))
        git(clone, "checkout", "-q", "-B", "main")
        if "stale-checkout" in wanted:
            # The branch moves on the remote and the *clone* is told about it, then
            # nothing checks it out. Two commits so the count is unmistakably a
            # count and not a boolean rendered as one.
            for nth in ("second", "third"):
                (seed / f"{nth}.md").write_text(f"{nth}\n", encoding="utf-8")
                git(seed, "add", "-A")
                git(seed, "commit", "-q", "-m", nth)
            git(seed, "push", "-q", str(origin), "main")
            git(clone, "fetch", "-q", "origin", "main")
        worktrees["blooop/devlaunch/main"] = _record("blooop", "devlaunch", "main", clone, MAIN_WS)
        state = "Running" if "warm" in wanted else "Stopped"
        workspaces[MAIN_WS] = _workspace(MAIN_WS, clone, state)

    (cache / "metadata.json").write_text(
        json.dumps(
            {"version": 3, "repositories": repositories, "worktrees": worktrees},
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    # The failure-injection channel: the first entry whose `prefix` matches the
    # call's leading argv wins and short-circuits the shim's state machine.
    refusals = []
    if "dotfiles" in wanted:
        # devpod's own shape, which is nested: `{"NAME": {"value": …}}`. A flat map
        # is what dl's *cache* file holds, and hand-writing devpod's shape into that
        # file sets nothing -- so a fixture that answered flat here would be testing
        # a document dl never reads (#560).
        refusals.append(
            {
                "prefix": ["context", "options"],
                "returncode": 0,
                "stdout": json.dumps({"DOTFILES_URL": {"value": DOTFILES_URL}}) + "\n",
            }
        )
    if "fail-up" in wanted:
        refusals.append(
            {"prefix": ["up"], "returncode": 7, "stderr": "devpod: image pull failed\n"}
        )
    if "fail-stop" in wanted:
        refusals.append(
            {"prefix": ["stop"], "returncode": 9, "stderr": "devpod: provider is gone\n"}
        )
    if "fail-session" in wanted:
        refusals.append(
            {
                "prefix": ["ssh"],
                "returncode": 3,
                "stderr": "devpod: connection refused\n",
            }
        )
    if "remote-exit" in wanted:
        # What devpod writes when the *remote* program exited non-zero: it buries
        # the status in a fatal of its own and still exits 1. The status is the
        # session's, the 1 is devpod's, and telling them apart is the whole of
        # `devpod_ssh`.
        refusals.append(
            {
                "prefix": ["ssh"],
                "returncode": 1,
                "stderr": (
                    "Try using the --debug flag to see a more verbose output\n"
                    "10:11:12 fatal ssh session: Process exited with status 130\n"
                ),
            }
        )
    if refusals:
        (root / "shim-config.json").write_text(
            json.dumps({"responses": refusals}), encoding="utf-8"
        )

    (root / "shim-state.json").write_text(
        json.dumps(
            {
                "providers": {"docker": {"config": {"name": "docker"}}},
                "workspaces": {} if "no-workspaces" in wanted else workspaces,
            },
            indent=1,
        ),
        encoding="utf-8",
    )


FIXTURES = {
    "warm",
    "stopped",
    "stale-checkout",
    "dotfiles",
    "gh",
    "no-devpod",
    "no-workspaces",
    "fail-up",
    "fail-stop",
    "symlinked-path",
    "fail-session",
    "remote-exit",
}

if __name__ == "__main__":
    if len(sys.argv) < 3:
        raise SystemExit(
            "usage: launch_scenario.py <root> <devpod_shim.py> "
            f"[{'] ['.join('--' + name for name in sorted(FIXTURES))}]"
        )
    flags = {argument.lstrip("-") for argument in sys.argv[3:]}
    unknown = flags - FIXTURES
    if unknown:
        raise SystemExit(f"launch_scenario.py: unknown fixture(s): {sorted(unknown)}")
    build(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]).resolve(), flags)
