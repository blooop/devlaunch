#!/usr/bin/env python3
"""A fake `devpod` for the acceptance harness's Tier 1 (#252 §3).

Stands on PATH under the name `devpod`, in front of *any* dl implementation —
it must know nothing about which one is calling, so it is a standalone,
stdlib-only program: no devlaunch imports, no pytest, no test helpers. The one
file it reads from the tree is `devpod/value_flags.json` beside it, which says
which flags consume a value and is the other fake's copy too; the harness runs
this program from where it lives rather than copying it, so the path holds, and
a run that cannot read it exits 78 rather than parsing every call wrong. It
re-homes the proven design of the Python `devpod_mock` fixture (retired with
that tree in #267) as a separate process:

- **workspace state machine**, persisted to the JSON file named by
  ``DEVPOD_SHIM_STATE`` so it survives across the many short-lived processes a
  test spawns. `up` creates or restarts, `stop` stops, `delete` removes,
  `list`/`status` answer from it — in the shapes dl actually parses, which are
  real devpod's shapes: `list --output json` is an array of workspace objects
  whose entries carry no state (real devpod answers state only to `status`).
- **argv→response table**, loaded from the JSON file named by
  ``DEVPOD_SHIM_CONFIG``: ``{"responses": [{"prefix": [...], "returncode": N,
  "stdout": "...", "stderr": "..."}]}``. The first entry whose ``prefix``
  matches the call's leading argv wins, and short-circuits the state machine —
  this is the failure-injection channel (mid-provision failures, malformed
  output, provider errors).
- **invocation log**, appended to the file named by ``DEVPOD_SHIM_LOG`` as one
  JSON object per line ``{"argv": [...]}``, so tests can assert the exact
  sequence of devpod commands an implementation issued.

Where real devpod would refuse — an id it has no workspace for, a command it
does not know — the shim exits non-zero with the refusal on stderr, because
Tier 2 exists to prove this program never drifts from the real one.

It is not the only fake devpod here, and that is what `test/fixtures/devpod/
conformance.json` is for: an argv→outcome table this program and the Rust
`DevpodMachine` are both driven over, so neither can quietly become stricter
than devpod again. Any behaviour change in here belongs in a corpus row first —
`test/unit/test_devpod_conformance.py` is this side of that bargain.

A missing ``DEVPOD_SHIM_STATE`` is different: that is a broken harness, not an
empty machine, and it exits 78 (EX_CONFIG) naming the variable rather than
letting a misconfigured run read as "no workspaces".
"""

import fcntl
import json
import os
import re
import sys
import time

EX_CONFIG = 78

#: Which flags consume the next argv element, read from the file the other fake
#: reads too.
#:
#: These lists used to be written out here *and* in the Rust fake, which is the
#: same two-copies-of-one-fact shape that let `delete --ignore-not-found` drift:
#: one side dropping a flag reads its value as a positional, and no test of
#: either fake against itself can see it. The file says what each list is for and
#: where it came from. Read by path rather than imported, because this program is
#: materialized on PATH as `devpod` and has no package to import from.
_VALUE_FLAGS_FILE = os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "devpod", "value_flags.json"
)

try:
    with open(_VALUE_FLAGS_FILE, encoding="utf-8") as _f:
        _TABLES = {name: frozenset(flags) for name, flags in json.load(_f)["tables"].items()}
except (OSError, ValueError, KeyError) as _error:
    # The same call as a missing DEVPOD_SHIM_STATE: a broken harness, not a
    # devpod that happens to parse nothing. Without the tables this program
    # reads every value flag as bare and makes its value the workspace, which
    # is the exact failure the shared file exists to prevent -- so it refuses
    # to run at all rather than run wrong.
    print(f"devpod-shim: cannot read {_VALUE_FLAGS_FILE}: {_error}", file=sys.stderr)
    sys.exit(EX_CONFIG)

_GLOBAL_VALUE_FLAGS = _TABLES["global"]
_UP_VALUE_FLAGS = _TABLES["up"]
_SSH_VALUE_FLAGS = _TABLES["ssh"]
_DELETE_VALUE_FLAGS = _TABLES["delete"]
_STATUS_VALUE_FLAGS = _TABLES["status"]
_STOP_VALUE_FLAGS = _TABLES["stop"]


def _log(argv):
    path = os.environ.get("DEVPOD_SHIM_LOG")
    if not path:
        return
    with open(path, "a", encoding="utf-8") as f:
        fcntl.flock(f.fileno(), fcntl.LOCK_EX)
        f.write(json.dumps({"argv": argv}) + "\n")


def _configured_response(argv):
    path = os.environ.get("DEVPOD_SHIM_CONFIG")
    if not path or not os.path.exists(path):
        return None
    with open(path, encoding="utf-8") as f:
        config = json.load(f)
    for entry in config.get("responses", []):
        prefix = entry.get("prefix", [])
        if argv[: len(prefix)] == prefix:
            return entry
    return None


class State:
    """The machine's workspaces and providers, kept in one flocked JSON file."""

    def __init__(self, path):
        self.path = path
        self.data = {"workspaces": {}, "providers": {"docker": {"config": {"name": "docker"}}}}
        if os.path.exists(path):
            with open(path, encoding="utf-8") as f:
                self.data = json.load(f)

    def save(self):
        tmp = self.path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(self.data, f, indent=1)
        os.replace(tmp, self.path)

    @property
    def workspaces(self):
        return self.data.setdefault("workspaces", {})

    @property
    def providers(self):
        return self.data.setdefault("providers", {})


def _state():
    path = os.environ.get("DEVPOD_SHIM_STATE")
    if not path:
        print(
            "devpod-shim: DEVPOD_SHIM_STATE is not set; this fake devpod has "
            "nowhere to keep its workspace state. The harness that put it on "
            "PATH must point that variable at a scratch file.",
            file=sys.stderr,
        )
        sys.exit(EX_CONFIG)
    return State(path)


def _now():
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def _refuse(verb, workspace_id):
    print(f"devpod-shim: {verb}: couldn't find workspace {workspace_id}", file=sys.stderr)
    return 1


def _source_object(source):
    """A `devpod list` source object for what `up` was handed.

    Real devpod records a path source as `localFolder` and a URL as
    `gitRepository`; dl's own launches always hand it a clone path.
    """
    looks_remote = "://" in source or source.startswith("git@")
    if not looks_remote and (source.startswith(("/", ".", "~")) or os.path.exists(source)):
        return {"localFolder": source}
    return {"gitRepository": source}


def _derive_id(source):
    """The id devpod would make up when `up` is given no --id: the source's
    last path-ish segment, lowercased, squeezed to [a-z0-9-]."""
    tail = re.sub(r"\.git$", "", source.rstrip("/")).split("/")[-1]
    derived = re.sub(r"[^a-z0-9]+", "-", tail.lower()).strip("-")
    return derived or "workspace"


def _positional_and_flags(args, value_flags):
    """The positionals and flags of one subcommand's argv.

    `value_flags` is the subcommand's own set; the globals are added here,
    because real devpod inherits them everywhere.
    """
    value_flags = set(value_flags) | _GLOBAL_VALUE_FLAGS
    positionals = []
    flags = {}
    i = 0
    while i < len(args):
        arg = args[i]
        if arg in value_flags and i + 1 < len(args):
            flags[arg] = args[i + 1]
            i += 2
        elif arg.startswith("-"):
            eq = arg.split("=", 1)
            if len(eq) == 2:
                flags[eq[0]] = eq[1]
            else:
                flags[arg] = True
            i += 1
        else:
            positionals.append(arg)
            i += 1
    return positionals, flags


def cmd_up(args):
    positionals, flags = _positional_and_flags(args, _UP_VALUE_FLAGS)
    if not positionals:
        print("devpod-shim: up: no workspace source given", file=sys.stderr)
        return 1
    source = positionals[0]
    state = _state()
    workspace_id = flags.get("--id")
    if workspace_id is None:
        # `dl <existing-id> up`-style restarts address the workspace by id.
        if source in state.workspaces:
            workspace_id = source
        else:
            workspace_id = _derive_id(source)
    existing = state.workspaces.get(workspace_id)
    if existing is None:
        state.workspaces[workspace_id] = {
            "id": workspace_id,
            "source": _source_object(source),
            "lastUsed": _now(),
            "provider": {"name": "docker"},
            "ide": {"name": flags.get("--ide", "none")},
            "context": "default",
            "state": "Running",
        }
    else:
        existing["state"] = "Running"
        existing["lastUsed"] = _now()
    state.save()
    print(f"Workspace {workspace_id} is ready")
    return 0


def cmd_stop(args):
    positionals, _ = _positional_and_flags(args, _STOP_VALUE_FLAGS)
    if not positionals:
        print("devpod-shim: stop: no workspace given", file=sys.stderr)
        return 1
    state = _state()
    workspace = state.workspaces.get(positionals[0])
    if workspace is None:
        return _refuse("stop", positionals[0])
    workspace["state"] = "Stopped"
    state.save()
    return 0


def cmd_delete(args):
    positionals, flags = _positional_and_flags(args, _DELETE_VALUE_FLAGS)
    if not positionals:
        print("devpod-shim: delete: no workspace given", file=sys.stderr)
        return 1
    state = _state()
    if positionals[0] not in state.workspaces:
        # `--ignore-not-found` makes a delete mean "ensure absent", the way
        # `rm -f` does, and real devpod v0.26.1 exits 0 for it with nothing to
        # say. Without this the shim refused, which is the one thing a fake
        # devpod must not do: `dl <ws> rm --force` passes the flag on every
        # forced remove, so a run against a workspace that was already gone
        # failed here and succeeded against the real thing. Nothing is printed
        # because there was nothing to delete and inventing a line the real
        # tool does not print is the same class of infidelity.
        if "--ignore-not-found" in flags:
            return 0
        return _refuse("delete", positionals[0])
    del state.workspaces[positionals[0]]
    state.save()
    print(f"Successfully deleted workspace {positionals[0]}")
    return 0


def _listed(workspace):
    """One `devpod list` entry: the record without the state key, which real
    devpod answers only to `status`."""
    return {key: value for key, value in workspace.items() if key != "state"}


def cmd_list(args):
    state = _state()
    entries = [_listed(ws) for ws in state.workspaces.values()]
    if "--output" in args and "json" in args:
        print(json.dumps(entries))
    else:
        for entry in entries:
            print(f"{entry['id']}  docker  {entry['lastUsed']}")
    return 0


def cmd_status(args):
    positionals, _ = _positional_and_flags(args, _STATUS_VALUE_FLAGS)
    if not positionals:
        print("devpod-shim: status: no workspace given", file=sys.stderr)
        return 1
    state = _state()
    workspace = state.workspaces.get(positionals[0])
    if workspace is None:
        return _refuse("status", positionals[0])
    payload = {
        "id": workspace["id"],
        "context": workspace.get("context", "default"),
        "provider": workspace.get("provider", {}).get("name", "docker"),
        "state": workspace.get("state", "Stopped"),
    }
    if "--output" in args and "json" in args:
        print(json.dumps(payload))
    else:
        print(f"Workspace {payload['id']} is {payload['state']}")
    return 0


def cmd_ssh(args):
    positionals, _ = _positional_and_flags(args, _SSH_VALUE_FLAGS)
    if not positionals:
        print("devpod-shim: ssh: no workspace given", file=sys.stderr)
        return 1
    state = _state()
    workspace = state.workspaces.get(positionals[0])
    if workspace is None:
        return _refuse("ssh", positionals[0])
    # Real devpod starts a stopped workspace to ssh into it.
    if workspace.get("state") != "Running":
        workspace["state"] = "Running"
        state.save()
    return 0


def cmd_provider(args):
    if not args:
        print("devpod-shim: provider: missing subcommand", file=sys.stderr)
        return 1
    state = _state()
    if args[0] == "list":
        if "--output" in args and "json" in args:
            print(json.dumps(state.providers))
        else:
            for name in state.providers:
                print(name)
        return 0
    if args[0] == "add" and len(args) > 1:
        state.providers[args[1]] = {"config": {"name": args[1]}}
        state.save()
        return 0
    if args[0] == "use" and len(args) > 1:
        return 0 if args[1] in state.providers else 1
    print(f"devpod-shim: provider: unknown subcommand {args[0]!r}", file=sys.stderr)
    return 1


def cmd_context(args):
    if args[:1] == ["options"]:
        print(json.dumps({}))
        return 0
    print(f"devpod-shim: context: unknown subcommand {args[:1]!r}", file=sys.stderr)
    return 1


def main(argv):
    _log(argv)

    configured = _configured_response(argv)
    if configured is not None:
        sys.stdout.write(configured.get("stdout", ""))
        sys.stderr.write(configured.get("stderr", ""))
        return int(configured.get("returncode", 0))

    if not argv:
        print("devpod-shim: no command given", file=sys.stderr)
        return 1
    command, args = argv[0], argv[1:]
    if command == "version":
        print("devpod version v0.26.1-shim")
        return 0
    handlers = {
        "up": cmd_up,
        "stop": cmd_stop,
        "delete": cmd_delete,
        "list": cmd_list,
        "status": cmd_status,
        "ssh": cmd_ssh,
        "provider": cmd_provider,
        "context": cmd_context,
    }
    handler = handlers.get(command)
    if handler is None:
        print(f'devpod-shim: unknown command "{command}" for "devpod"', file=sys.stderr)
        return 1
    return handler(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
