#!/usr/bin/env python3
"""Run the frozen Python dl and the Rust dl in identical worlds and diff them.

Single case:

    compare.py [--lifecycle|--read] [--aid] [--release] [--fingerprint] \
        [fixtures...] -- <args...>

Only the *first* `--` separates fixtures from the dl argv; any later `--` is part
of the command being tested (`compare.py -- <ws> -- make test` is `dl <ws> -- make
test`), which the old "every `--` is a separator" made impossible to compare.

Case list (the CI `rust-parity` compare step drives this):

    compare.py --cases rust/tools/parity_cases.txt [--release] [--fingerprint]

Each non-comment line of the file is one case's argv, optionally followed by
`## same` (the default), `## diff: <divergence row / reason>` for a case that is
allowed to differ, or `## skip: <reason>`. The run fails on any case that differs
without an annotation citing why -- see the file's own header.
"""

import fnmatch
import hashlib
import json
import os
import pathlib
import shlex
import shutil
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

# Files under the cache that the detached refresh child races or that carry a
# per-run nonce, so they are never part of the state two runs must agree on.
VOLATILE = ("completions.*", "*.lock", "*.tmp", "last_fetched")

# The same refresh child also stamps a wall-clock into metadata.json under this
# key (a stale fixture value invites the refresh, which rewrites it to "now" if
# it wins the race before the run exits). It is normalised out of the fingerprint
# for the same reason the `last_fetched` sidecar is: it is the timestamp of the
# fetch, not a fact about the records.
_VOLATILE_JSON_KEYS = {"last_fetched"}

# Shim-argv words are trimmed only past this, long enough to keep the
# `--command bash -lc <payload>` a launch actually sends (the old 24 hid it).
WORD_LIMIT = 400


def _fingerprint(root: pathlib.Path) -> list[str]:
    """A stable picture of the cache the run left behind.

    Relative paths for every directory and file under `<root>/cache`, each file
    tagged with a hash of its contents with the scratch root templated out, so
    two runs in two different scratch roots compare equal. Volatile files are
    dropped -- their presence or contents is not a divergence. This is what
    catches the on-disk shapes a stdout/stderr diff cannot: an orphaned clone
    directory, a stale record, a metadata.json a migration corrupted.
    """
    cache = root / "cache"
    if not cache.exists():
        return ["(no cache)"]
    entries = []
    for dirpath, dirnames, filenames in os.walk(cache):
        here = pathlib.Path(dirpath)
        entries.append(f"{here.relative_to(root)}/")
        # A clone's own git store (`.git`/`.bare`) is recorded by presence and
        # not descended into. Its object store shards by commit SHA, and the
        # scenario builder stamps each world's commits with the wall-clock, so
        # two separate builds produce different SHAs and different sharding --
        # nondeterminism, not a divergence. What the on-disk parity is actually
        # about -- an orphaned clone directory, a stale record, a migration that
        # corrupted metadata.json, uncommitted work left in a worktree -- lives
        # in the tree shape and the files *outside* those stores, which are
        # content-hashed below (root templated out so two scratch roots match).
        for store in sorted(n for n in dirnames if n in (".git", ".bare")):
            entries.append(f"{(here / store).relative_to(root)}/ (git store, contents omitted)")
        dirnames[:] = [n for n in dirnames if n not in (".git", ".bare")]
        for name in filenames:
            path = here / name
            rel = path.relative_to(root)
            if path.is_symlink():
                entries.append(f"{rel} -> symlink")
                continue
            if any(fnmatch.fnmatch(name, pat) for pat in VOLATILE):
                continue
            blob = path.read_bytes().replace(str(root).encode(), b"{ROOT}")
            if name == "metadata.json":
                blob = _normalize_metadata(blob)
            entries.append(f"{rel} {hashlib.sha256(blob).hexdigest()[:16]}")
    return sorted(entries)


def _scrub(node):
    if isinstance(node, dict):
        return {
            k: ("<volatile>" if k in _VOLATILE_JSON_KEYS else _scrub(v)) for k, v in node.items()
        }
    if isinstance(node, list):
        return [_scrub(v) for v in node]
    return node


def _normalize_metadata(blob: bytes) -> bytes:
    """Blank the refresh child's timestamp out of metadata.json before hashing.

    Everything else in the file -- the records, their local_path, the worktrees,
    a corruption a bad migration left -- is what the fingerprint is for and is
    kept. A file that will not parse (a fixture that corrupts it on purpose) is
    hashed as-is: both builds see the same bytes, and the malformed-ness is
    itself the state worth comparing.
    """
    try:
        return json.dumps(_scrub(json.loads(blob)), sort_keys=True).encode()
    except (ValueError, UnicodeDecodeError):
        return blob


def one_run(cmd, scenario, fixtures, args, extra_env, fingerprint):
    root = pathlib.Path(tempfile.mkdtemp(prefix="dlt", dir="/tmp"))
    try:
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
                    if len(word) > WORD_LIMIT:
                        word = word.splitlines()[0][:WORD_LIMIT] + "…"
                    words.append(word)
                calls.append("devpod " + " ".join(words))
        result = {
            "exit": done.returncode,
            "stdout": t(done.stdout).splitlines(),
            "stderr": t(done.stderr).splitlines(),
            "calls": calls,
        }
        if fingerprint:
            result["state"] = _fingerprint(root)
        return result
    finally:
        # The sweep runs hundreds of these; leaving a world per run behind is
        # what filled the disk.
        shutil.rmtree(root, ignore_errors=True)


def parse_case(argv, release):
    """Split one case's argv into (module, rust_binary, scenario, fixtures, args)."""
    fixtures, args = [], []
    seen = False
    scenario = SCENARIOS["launch"]
    aid = False
    for a in argv:
        if a == "--" and not seen:
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
    profile = "release" if release else "debug"
    rust = REPO / "rust/target" / profile / ("aid" if aid else "dl")
    return module, rust, scenario, fixtures, args


def compare_one(argv, release, fingerprint):
    """Run one case both ways; return (same, report_lines)."""
    module, rust, scenario, fixtures, args = parse_case(argv, release)
    extra = {}
    for pair in os.environ.get("CAPTURE_ENV", "").split(","):
        if pair:
            name, _, value = pair.partition("=")
            extra[name] = value
    python = one_run([str(PY), "-m", module], scenario, fixtures, args, extra, fingerprint)
    ported = one_run([str(rust)], scenario, fixtures, args, extra, fingerprint)
    same = python == ported
    lines = [("SAME  " if same else "DIFF  ") + " ".join(fixtures + ["--"] + args)]
    if not same:
        for key in ("exit", "stdout", "stderr", "calls", "state"):
            if python.get(key) != ported.get(key):
                lines.append(f"  {key}:")
                lines.append(f"    python: {python.get(key)}")
                lines.append(f"    rust  : {ported.get(key)}")
    return same, lines


def run_cases(path, release, fingerprint):
    """Drive a checked-in case list; fail on any un-annotated divergence."""
    failures = 0
    for raw in pathlib.Path(path).read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        case, _, annotation = line.partition("##")
        policy, _, reason = annotation.strip().partition(":")
        policy = policy.strip() or "same"
        reason = reason.strip()
        argv = shlex.split(case)
        if policy == "skip":
            print(f"SKIP  {case.strip()}  ({reason})")
            continue
        same, lines = compare_one(argv, release, fingerprint)
        if policy == "diff":
            # Allowed to differ, but it must cite a reason, and it must actually
            # still differ -- a diff-case gone SAME is an allowance to remove.
            if not reason:
                print("\n".join(lines))
                print("  ^ marked `diff` with no divergence row / reason cited")
                failures += 1
            elif same:
                print(f"NOTE  {case.strip()} now SAME; drop the `## diff: {reason}` allowance")
            else:
                print("\n".join(lines))
                print(f"  ^ allowed: {reason}")
            continue
        # policy == "same": any divergence is a regression.
        print("\n".join(lines))
        if not same:
            failures += 1
    if failures:
        print(f"\ncompare: {failures} case(s) diverged without an allowance", file=sys.stderr)
    return 1 if failures else 0


def main(argv):
    release = "--release" in argv
    fingerprint = "--fingerprint" in argv
    argv = [a for a in argv if a not in ("--release", "--fingerprint")]
    if argv and argv[0] == "--cases":
        return run_cases(argv[1], release, fingerprint)
    same, lines = compare_one(argv, release, fingerprint)
    print("\n".join(lines))
    return 0 if same else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
