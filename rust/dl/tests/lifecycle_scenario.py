#!/usr/bin/env python3
"""Build the worlds the lifecycle commands are judged against.

Stdlib-only and implementation-blind, like `tests/scenario.py` and
`test/fixtures/devpod_shim.py`: it is run by the Rust integration test *and* by
the golden-capture harness that runs the frozen Python build against the same
tree, so neither implementation can be the thing that defines the fixture.

`tests/scenario.py` builds the read side's world and its `--ls` goldens are
measured from it, so it cannot grow a workspace without moving them. This is the
mutating side's world instead, and the fixtures each command needs are switched
on by name so that one command's golden does not have to explain another's
fixture:

    lifecycle_scenario.py <root> <devpod_shim.py> [--prunable] [--stale-record]
                                                  [--orphan] [--unplaceable]
                                                  [--unwritable] [--no-cache]
                                                  [--no-workspaces]
                                                  [--devcontainer-volumes]
                                                  [--agent-worktrees]

The base world, under the root it is given:

- `bin/devpod` — the fake devpod, a two-line shell wrapper around the shim.
- `bin/docker` — a fake docker that logs its argv to `docker-log` and exits 0.
  In every world, not just the one that asserts about it: `bin` is first on the
  test's PATH, and without it a delete would reach the real docker on whatever
  machine is running the suite.
- `cache/devlaunch/` — the devlaunch cache (`XDG_CACHE_HOME=<root>/cache`), with
  `metadata.json` at schema 2 and the clones under `repos/`.
- a **clean** clone at `repos/blooop/devlaunch/<CLEAN_LEAF>`, recorded for
  `blooop/devlaunch@main` with `devpod_workspace_id` naming a workspace id this
  build does *not* derive — devlaunch#88's shape, so `dl blooop/devlaunch@main
  stop` has to follow the record to reach it.
- a **dirty** clone at `repos/blooop/devlaunch/<DIRTY_LEAF>`, recorded for
  `blooop/devlaunch@dirty` and holding one untracked file, so `dl <id> rm` has
  something to refuse.
- a foreign workspace at `<root>/foreign/proj`, which is what `--purge` leaves
  standing and names.

Every path it writes is under the root, so a test can substitute the root for a
placeholder and compare bytes against a golden captured on another machine.
"""

import json
import os
import pathlib
import shutil
import subprocess
import sys

RECORDED = "2026-08-01T10:11:12+0000"
OLDER = "2026-07-30T09:08:07+0000"
NEVER = ""

# The clone directory leaves, and the workspace ids devpod knows them by.
#
# A clone's leaf *is* the workspace id its record carries — that is how `dl` names
# the directory — and the record's `devpod_workspace_id` is the id devpod answers
# to. Here the pair for `blooop/devlaunch@main` is deliberately not the id this
# build derives, which is devlaunch#88's shape: a workspace created under the old
# derivation, still addressable only through its record.
CLEAN_LEAF = "devlaunch-main-legacy"
CLEAN_WS = "devlaunch-main-legacy"
DIRTY_LEAF = "devlaunch-dirty-dofaraji"
DIRTY_WS = "devlaunch-dirty-dofaraji"
FOREIGN_WS = "someones-project"

# --prunable: two clone directories no workspace opens and no record names.
PRUNABLE_LEAF = "devlaunch-gone-nobody"
PRUNABLE_DIRTY_LEAF = "devlaunch-gone-dirty"

# --agent-worktrees: two real git worktrees an agent harness would have made
# inside the *clean* clone, which a live workspace opens. Registered from inside a
# container, so the paths git holds are container paths and git calls them
# prunable -- the shape devlaunch#426 measured 72 of. One is finished and
# collectable; the other holds an untracked note, so it is reported and kept.
AGENT_GONE = "agent-finished"
AGENT_HELD = "agent-unsaved"

# --stale-record: a record whose directory is definitively not there.
STALE_LEAF = "devlaunch-ancient-forgotten"

# --orphan: devpod records under a repository's clone tree at something that is
# not a clone. The first can be re-pointed (a record's clone answers to the
# legacy spelling of its branch); the second cannot, because nothing answers.
ORPHAN_WS = "other-feature-x-legacy"
ORPHAN_LEAF = "feature-x"
ORPHAN_CLONE_LEAF = "other-feature-x-repezebi"
UNADOPTABLE_WS = "other-nothing-here"
UNADOPTABLE_LEAF = "nothing-answers"

# --unplaceable: a live workspace whose `localFolder` devpod filled with something
# that is not a path at all. It could be opening *any* candidate, so both cleanup
# commands stop for it.
UNPLACEABLE_WS = "a-source-nobody-can-read"

# --unwritable: one directory inside a clone that this process may not write, so a
# removal gets part-way and says which path refused. Skipped under root, which can
# unlink anything: a caller that needs the refusal has to check for it.
UNWRITABLE_LEAF = "devlaunch-gone-locked"

# --unpushed: a recorded clone holding a commit no remote has.
UNPUSHED_LEAF = "devlaunch-unpushed-committed"
UNPUSHED_WS = "devlaunch-unpushed-committed"

# --sealed-cache: the cache root itself refuses, so a purge removes what it can and
# names what it could not. Skipped under root, which can unlink anything.

# --symlinked-cache: the cache root is a symbolic link (devlaunch#131). Following it
# would delete a tree nobody pointed at and unlinking it would report a clone as
# reclaimed while it sat on another volume, so a purge refuses and removes nothing.

# --not-a-clone: a recorded clone directory git cannot be asked about.
NOT_A_CLONE_LEAF = "devlaunch-opaque-nogit"
NOT_A_CLONE_WS = "devlaunch-opaque-nogit"


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


def _clone(root, origin, path, branch, dirty=False):
    path.parent.mkdir(parents=True, exist_ok=True)
    git(root, "clone", "-q", str(origin), str(path))
    git(path, "checkout", "-q", "-B", branch)
    if dirty:
        (path / "scratch.txt").write_text("unsaved\n", encoding="utf-8")
    return path


def build(root: pathlib.Path, shim: pathlib.Path, wanted: set) -> None:
    cache = root / "cache" / "devlaunch"
    repos = cache / "repos"
    for path in (root / "config", root / "home", root / "devpod", root / "bin", repos):
        path.mkdir(parents=True, exist_ok=True)

    devpod = root / "bin" / "devpod"
    devpod.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{shim}" "$@"\n', encoding="utf-8")
    devpod.chmod(0o755)

    # A fake docker, in *every* world and not only the one that asserts about it:
    # `bin` comes first on the test's PATH, so without this a delete would reach
    # whatever docker the machine running the suite has and remove volumes on it.
    # It logs one line per call and exits 0, which is what a removal that worked
    # looks like.
    docker = root / "bin" / "docker"
    docker.write_text(
        f'#!/bin/sh\necho "$@" >> "{root / "docker-log"}"\n',
        encoding="utf-8",
    )
    docker.chmod(0o755)

    # A bare repository standing in for GitHub, with one commit on `main`.
    seed = root / "seed"
    seed.mkdir(parents=True, exist_ok=True)
    git(seed, "init", "-q", "-b", "main", ".")
    (seed / "README.md").write_text("seed\n", encoding="utf-8")
    git(seed, "add", "-A")
    git(seed, "commit", "-q", "-m", "seed")
    origin = root / "origin.git"
    git(root, "clone", "-q", "--bare", "seed", "origin.git")

    # The bare clone the cache keeps for blooop/devlaunch. Never a prune
    # candidate: it is the copy every clone hardlinks its objects out of.
    bare = repos / "blooop" / "devlaunch" / ".bare"
    bare.parent.mkdir(parents=True, exist_ok=True)
    git(root, "clone", "-q", "--bare", str(origin), str(bare))

    if "v1-cache" in wanted:
        # A cache written by a *pre-#64* devlaunch: schema **1**, one clone under
        # the old flattened-branch leaf (`.../devlaunch/main`), and a record whose
        # `workspace_id` is the old `repo-branch` id (`devlaunch-main`). The first
        # command that builds the clone manager migrates it once — renaming the
        # directory onto the derived id and announcing what it did on stderr — so
        # this is the world the migration notices are compared against. devpod
        # lists nothing (the migration reads no devpod), keeping stdout
        # deterministic. Runs `dl --ls --json` (not the plain `--ls` table): as in
        # Python, only the paths that build the clone manager migrate, and the
        # table path is not one of them (dl.py:_get_clone_manager).
        old_clone = _clone(root, origin, repos / "blooop" / "devlaunch" / "main", "main")
        worktrees = {
            "blooop/devlaunch/main": _record(
                "blooop", "devlaunch", "main", old_clone, "devlaunch-main"
            ),
        }
        # `_record` names the workspace id after the clone's leaf; the old scheme's
        # is the flattened branch, so override it to the pre-#64 `repo-branch` id.
        worktrees["blooop/devlaunch/main"]["workspace_id"] = "devlaunch-main"
        (cache / "metadata.json").write_text(
            json.dumps(
                {"version": 1, "repositories": {}, "worktrees": worktrees},
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        (root / "shim-state.json").write_text(
            json.dumps(
                {"providers": {"docker": {"config": {"name": "docker"}}}, "workspaces": {}},
                indent=1,
            ),
            encoding="utf-8",
        )
        return

    clean = _clone(root, origin, repos / "blooop" / "devlaunch" / CLEAN_LEAF, "main")
    dirty = _clone(
        root,
        origin,
        repos / "blooop" / "devlaunch" / DIRTY_LEAF,
        "dirty",
        dirty=True,
    )
    foreign = root / "foreign" / "proj"
    foreign.mkdir(parents=True, exist_ok=True)
    (foreign / "notes.txt").write_text("not devlaunch's\n", encoding="utf-8")

    # The bare cache's own record, which is what the refresh child's fetch sweep
    # reads: an interval that elapsed long ago, so a sweep has one repository to
    # bring up to date and `last_fetched` to write back.
    repositories = {
        "blooop/devlaunch": {
            "owner": "blooop",
            "repo": "devlaunch",
            "remote_url": str(origin),
            "local_path": str(bare),
            "default_branch": "main",
            "last_fetched": "2020-01-01T00:00:00",
            "worktrees": [],
        }
    }
    worktrees = {
        "blooop/devlaunch/main": _record("blooop", "devlaunch", "main", clean, CLEAN_WS),
        "blooop/devlaunch/dirty": _record("blooop", "devlaunch", "dirty", dirty, DIRTY_WS),
    }
    workspaces = {
        CLEAN_WS: _workspace(CLEAN_WS, {"localFolder": str(clean)}, RECORDED, "Running"),
        DIRTY_WS: _workspace(DIRTY_WS, {"localFolder": str(dirty)}, OLDER, "Stopped"),
        FOREIGN_WS: _workspace(FOREIGN_WS, {"localFolder": str(foreign)}, NEVER, "Stopped"),
    }

    if "prunable" in wanted:
        # Nothing opens these and no record names them: the one arm `--prune`
        # removes, and the one it keeps until `--force` answers for it.
        _clone(root, origin, repos / "blooop" / "devlaunch" / PRUNABLE_LEAF, "gone")
        _clone(
            root,
            origin,
            repos / "blooop" / "devlaunch" / PRUNABLE_DIRTY_LEAF,
            "gone-dirty",
            dirty=True,
        )

    if "agent-worktrees" in wanted:
        # `git worktree add` from inside the clone, then the registrations rewritten
        # to the container paths they would really carry. Both steps matter: the
        # classification is git's own `worktree list`, and it is the container path
        # that makes a registration prunable on a host.
        trees = clean / ".claude" / "worktrees"
        trees.mkdir(parents=True, exist_ok=True)
        for leaf in (AGENT_GONE, AGENT_HELD):
            git(clean, "worktree", "add", "-q", "-b", leaf, str(trees / leaf))
            git(clean, "push", "-q", "origin", leaf)
        git(bare, "fetch", "-q", "origin", "+refs/heads/*:refs/heads/*", "--prune")
        (trees / AGENT_HELD / "notes.md").write_text("half a thought\n", encoding="utf-8")
        for leaf in (AGENT_GONE, AGENT_HELD):
            gitdir = clean / ".git" / "worktrees" / leaf / "gitdir"
            gitdir.write_text(
                gitdir.read_text(encoding="utf-8").replace(
                    str(clean), "/workspaces/devlaunch-main-legacy"
                ),
                encoding="utf-8",
            )

    if "unpushed" in wanted:
        # A commit that exists in this clone and nowhere else. Uncommitted work and
        # unpushed commits are different losses and the refusal names which.
        held = _clone(root, origin, repos / "blooop" / "devlaunch" / UNPUSHED_LEAF, "unpushed")
        (held / "kept.txt").write_text("committed, unpushed\n", encoding="utf-8")
        git(held, "add", "-A")
        git(held, "commit", "-q", "-m", "work nobody else has")
        worktrees["blooop/devlaunch/unpushed"] = _record(
            "blooop", "devlaunch", "unpushed", held, UNPUSHED_WS
        )
        workspaces[UNPUSHED_WS] = _workspace(
            UNPUSHED_WS, {"localFolder": str(held)}, OLDER, "Stopped"
        )

    if "not-a-clone" in wanted:
        # A recorded clone directory that is there and is not a repository git can
        # read — an interrupted delete, or a `.git` a container wrote as another
        # user. "Could not tell" refuses the delete for the same reason unpushed
        # work does: the files are still on disk (devlaunch#171).
        opaque = repos / "blooop" / "devlaunch" / NOT_A_CLONE_LEAF
        opaque.mkdir(parents=True, exist_ok=True)
        (opaque / "work.txt").write_text("who knows\n", encoding="utf-8")
        worktrees["blooop/devlaunch/opaque"] = _record(
            "blooop", "devlaunch", "opaque", opaque, NOT_A_CLONE_WS
        )
        workspaces[NOT_A_CLONE_WS] = _workspace(
            NOT_A_CLONE_WS, {"localFolder": str(opaque)}, OLDER, "Stopped"
        )

    if "unplaceable" in wanted:
        workspaces[UNPLACEABLE_WS] = _workspace(
            UNPLACEABLE_WS, {"localFolder": 42}, OLDER, "Stopped"
        )

    if "unwritable" in wanted:
        locked = _clone(root, origin, repos / "blooop" / "devlaunch" / UNWRITABLE_LEAF, "locked")
        held = locked / "held"
        held.mkdir()
        (held / "keep.txt").write_text("cannot be unlinked\n", encoding="utf-8")
        # No write bit on the directory, so its entries cannot be unlinked — the
        # shape a container writing as another user leaves behind.
        held.chmod(0o500)

    if "stale-record" in wanted:
        # A record for a directory that was removed by hand. metadata.json is
        # append-mostly and nothing has ever pruned it.
        worktrees["blooop/devlaunch/ancient"] = _record(
            "blooop",
            "devlaunch",
            "ancient",
            repos / "blooop" / "devlaunch" / STALE_LEAF,
            None,
        )

    if "orphan" in wanted:
        # devlaunch#88's shape: devpod's record names a folder under the
        # repository's clone tree that is not a clone, while the real checkout
        # sits beside it under a name the record's branch also answers to.
        adoptable = _clone(
            root,
            origin,
            repos / "blooop" / "other" / ORPHAN_CLONE_LEAF,
            "feature-x",
        )
        worktrees["blooop/other/feature/x"] = _record(
            "blooop", "other", "feature/x", adoptable, None
        )
        for workspace_id, leaf in (
            (ORPHAN_WS, ORPHAN_LEAF),
            (UNADOPTABLE_WS, UNADOPTABLE_LEAF),
        ):
            sourced_at = repos / "blooop" / "other" / leaf
            workspaces[workspace_id] = _workspace(
                workspace_id, {"localFolder": str(sourced_at)}, OLDER, "Stopped"
            )
            # devpod's own record, which is the file `--reconcile` rewrites.
            record = (
                root
                / "devpod"
                / "contexts"
                / "default"
                / "workspaces"
                / workspace_id
                / "workspace.json"
            )
            record.parent.mkdir(parents=True, exist_ok=True)
            record.write_text(
                json.dumps(
                    {
                        "id": workspace_id,
                        "uid": f"uid-{workspace_id}",
                        "source": {"localFolder": str(sourced_at)},
                        "provider": {"name": "docker"},
                    },
                    indent=2,
                ),
                encoding="utf-8",
            )

    (cache / "metadata.json").write_text(
        json.dumps(
            {"version": 2, "repositories": repositories, "worktrees": worktrees},
            indent=2,
        )
        + "\n",
        encoding="utf-8",
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
    if "devcontainer-volumes" in wanted:
        # devpod's own record of a *finished* create, which is where the names of
        # the volumes the devcontainer made are readable from: the folder it opened
        # (whose basename the `-pixi` mount is named after) and the devcontainer id
        # the `docker-in-docker` feature's volume is named after. Written only for
        # the clean workspace, so `rm` reaches it without `--force`.
        #
        # Both files, because devpod writes both: `workspace.json` on the way *in*
        # to an `up` and `workspace_result.json` on the way out of a finished one.
        # A result with no record beside it is a shape devpod never leaves, and dl
        # reads the record to settle which context a workspace is in before it
        # trusts the result.
        workspace_dir = root / "devpod" / "contexts" / "default" / "workspaces" / CLEAN_WS
        workspace_dir.mkdir(parents=True, exist_ok=True)
        (workspace_dir / "workspace.json").write_text(
            json.dumps(
                {
                    "id": CLEAN_WS,
                    "uid": f"uid-{CLEAN_WS}",
                    "source": {"localFolder": str(clean)},
                    "provider": {"name": "docker"},
                },
                indent=2,
            ),
            encoding="utf-8",
        )
        result = workspace_dir / "workspace_result.json"
        result.write_text(
            json.dumps(
                {
                    "ContainerDetails": {"Id": "abc123def456"},
                    "MergedConfig": {},
                    "SubstitutionContext": {
                        "LocalWorkspaceFolder": str(clean),
                        "ContainerWorkspaceFolder": "/workspaces/devlaunch",
                        "DevContainerID": "0f4b2c1d",
                    },
                },
                indent=2,
            ),
            encoding="utf-8",
        )

    if "sealed-cache" in wanted:
        # No write bit on the cache root, so nothing under it can be unlinked: the
        # purge arm where not one path came away.
        cache.chmod(0o500)

    if "symlinked-cache" in wanted:
        elsewhere = root / "elsewhere" / "devlaunch"
        elsewhere.parent.mkdir(parents=True, exist_ok=True)
        cache.rename(elsewhere)
        cache.symlink_to(elsewhere)

    if "no-cache" in wanted:
        # Everything devlaunch stores, removed by hand — the state a machine is in
        # after somebody deleted the cache themselves, or after a purge that got
        # interrupted. The workspaces devpod still lists are sourced under it.
        shutil.rmtree(cache)


if __name__ == "__main__":
    if len(sys.argv) < 3:
        raise SystemExit(
            "usage: lifecycle_scenario.py <root> <devpod_shim.py> [--prunable] "
            "[--stale-record] [--orphan] [--unplaceable] [--unwritable] "
            "[--no-cache] [--no-workspaces] [--not-a-clone] [--unpushed] "
            "[--sealed-cache] [--symlinked-cache] [--v1-cache] "
            "[--agent-worktrees] "
            "[--devcontainer-volumes]"
        )
    flags = {argument.lstrip("-") for argument in sys.argv[3:]}
    unknown = flags - {
        "prunable",
        "stale-record",
        "orphan",
        "unplaceable",
        "unwritable",
        "no-cache",
        "no-workspaces",
        "not-a-clone",
        "unpushed",
        "sealed-cache",
        "symlinked-cache",
        "v1-cache",
        "devcontainer-volumes",
        "agent-worktrees",
    }
    if unknown:
        raise SystemExit(f"lifecycle_scenario.py: unknown fixture(s): {sorted(unknown)}")
    build(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]).resolve(), flags)
