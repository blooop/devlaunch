#!/usr/bin/env python3
"""The rust-parity gate: spec-ledger lint and the two-way expected-failure ratchet.

Stdlib only, so CI can run the lint with a bare python3 before any environment
exists. Three subcommands:

  lint       spec-ledger coverage + disposition rules + the shrink-only
             `pending` ratchet against rust/pending-count.txt, plus a manifest
             syntax check.
  self-test  the unit tests in rust/test_parity.py.
  run        build the release `dl`, run both pytest tiers against it with
             DEVLAUNCH_DL_CMD, and compare failures against
             rust/parity-manifest.txt: any failure outside the manifest fails
             (regression), any manifest entry that passed fails (stale entry —
             shrink the manifest in the same PR). While the manifest is the
             day-one `ALL` sentinel the pytest run is skipped: the binary
             cannot pass anything yet, and the scaffold checks (cargo
             build/test/clippy/fmt + lint + self-test) are the whole gate.

The manifest's git history is the parity dashboard; docs/rust-rewrite-plan.md
owns the surrounding policy.
"""

from __future__ import annotations

import argparse
import fnmatch
import os
import re
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path

RUST_DIR = Path(__file__).resolve().parent
REPO_ROOT = RUST_DIR.parent
LEDGER = RUST_DIR / "spec-ledger.md"
MANIFEST = RUST_DIR / "parity-manifest.txt"
PENDING_COUNT = RUST_DIR / "pending-count.txt"

# The exact disposition vocabulary from docs/rust-rewrite-plan.md ("The spec
# ledger"). `out of port scope` may carry a parenthetical qualifier; `covered
# by divergence row #N` names a row of the plan's Grade-C table.
_DISPOSITION_RES = [
    re.compile(r"^pending$"),
    re.compile(r"^re-expressed at boundary$"),
    re.compile(r"^re-pinned in Rust$"),
    re.compile(r"^covered by divergence row #\d+$"),
    re.compile(r"^out of port scope( \([^)]+\))?$"),
]


class ManifestError(Exception):
    pass


@dataclass
class Manifest:
    sentinel_all: bool
    patterns: list[str]


@dataclass
class LedgerRow:
    file: str
    disposition: str
    notes: str


def parse_manifest(text: str) -> Manifest:
    lines = [
        line.strip()
        for line in text.splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]
    sentinel = "ALL" in lines
    patterns = [line for line in lines if line != "ALL"]
    if sentinel and patterns:
        raise ManifestError(
            "parity-manifest.txt mixes the ALL sentinel with per-test entries; "
            "once the first test passes, replace ALL with the list of tests "
            "still expected to fail"
        )
    return Manifest(sentinel_all=sentinel, patterns=patterns)


def parse_ledger(text: str) -> list[LedgerRow]:
    rows = []
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("|"):
            continue
        cells = [c.strip() for c in stripped.strip("|").split("|")]
        if len(cells) < 2:
            continue
        file_cell = cells[0].strip("`").strip()
        if file_cell in ("file", "") or set(file_cell) <= {"-", ":"}:
            continue  # header or separator row
        rows.append(
            LedgerRow(
                file=file_cell,
                disposition=cells[1],
                notes=cells[2] if len(cells) > 2 else "",
            )
        )
    return rows


def base_disposition(disposition: str) -> str:
    return re.sub(r" \([^)]+\)$", "", disposition)


def lint_dispositions(rows: list[LedgerRow]) -> list[str]:
    errors = []
    for row in rows:
        if not any(r.match(row.disposition) for r in _DISPOSITION_RES):
            errors.append(
                f"{row.file}: disposition {row.disposition!r} is not in the "
                "allowed set (see docs/rust-rewrite-plan.md)"
            )
    return errors


def lint_coverage(rows: list[LedgerRow], test_files: list[str]) -> list[str]:
    errors = []
    seen: set[str] = set()
    for row in rows:
        if row.file in seen:
            errors.append(f"{row.file}: duplicate ledger row")
        seen.add(row.file)
    missing = sorted(set(test_files) - seen)
    stale = sorted(seen - set(test_files))
    errors.extend(
        f"{f}: file under test/ has no spec-ledger row — every new Python "
        "test file needs a disposition decision (docs/rust-rewrite-plan.md)"
        for f in missing
    )
    errors.extend(f"{f}: ledger row for a file that no longer exists" for f in stale)
    return errors


def lint_pending_ratchet(current: int, recorded: int) -> list[str]:
    if current > recorded:
        return [
            f"pending ledger rows grew {recorded} -> {current}; the pending "
            "count only shrinks — give the new file a real disposition"
        ]
    if current < recorded:
        return [
            f"pending ledger rows shrank {recorded} -> {current}; record it by "
            f"updating rust/pending-count.txt to {current} in this PR"
        ]
    return []


def compare_failures(failed: list[str], patterns: list[str]) -> tuple[list[str], list[str]]:
    """Two-way ratchet: (unexpected failures, stale manifest entries).

    A pattern matches a node id exactly, as an fnmatch glob, or as a bare
    test id matching any of its parametrized cases.
    """

    def matches(pattern: str, node_id: str) -> bool:
        return (
            node_id == pattern
            or fnmatch.fnmatchcase(node_id, pattern)
            or fnmatch.fnmatchcase(node_id, pattern + "[[]*[]]")
        )

    unexpected = [f for f in failed if not any(matches(p, f) for p in patterns)]
    stale = [p for p in patterns if not any(matches(p, f) for f in failed)]
    return unexpected, stale


def discover_test_files() -> list[str]:
    # The filesystem, not `git ls-files`: an uncommitted test file must not
    # evade the every-file-has-a-row rule.
    return sorted(
        str(p.relative_to(REPO_ROOT))
        for p in (REPO_ROOT / "test").rglob("*.py")
        if "__pycache__" not in p.parts
    )


def cmd_lint() -> int:
    errors: list[str] = []
    try:
        parse_manifest(MANIFEST.read_text())
    except ManifestError as e:
        errors.append(str(e))
    rows = parse_ledger(LEDGER.read_text())
    errors.extend(lint_dispositions(rows))
    errors.extend(lint_coverage(rows, discover_test_files()))
    current_pending = sum(1 for r in rows if r.disposition == "pending")
    recorded = int(PENDING_COUNT.read_text().split()[0])
    errors.extend(lint_pending_ratchet(current_pending, recorded))
    for err in errors:
        print(f"parity lint: {err}", file=sys.stderr)
    if not errors:
        print(f"parity lint: ok ({len(rows)} ledger rows, {current_pending} pending)")
    return 1 if errors else 0


def cmd_self_test() -> int:
    sys.path.insert(0, str(RUST_DIR))
    suite = unittest.defaultTestLoader.discover(str(RUST_DIR), pattern="test_parity.py")
    result = unittest.TextTestRunner(verbosity=1).run(suite)
    return 0 if result.wasSuccessful() else 1


def _run_tier(name: str, marker: str, dl_cmd: str, aid_cmd: str) -> list[str]:
    """Run one pytest tier against the Rust binaries; return failed node ids.

    Both seams are exported, not just `dl`'s: `aid` is a second binary on the
    Rust side (#252 §1) and a tier that redirected only `dl` would judge the
    Python `aid` against the Rust `dl` -- a pair that ships together and would
    then never be tested together.
    """
    junit = Path(tempfile.mkstemp(prefix=f"parity-{name}-", suffix=".xml")[1])
    env = dict(os.environ, DEVLAUNCH_DL_CMD=dl_cmd, DEVLAUNCH_AID_CMD=aid_cmd)
    # Through pixi so the tier runs in the project environment CI already
    # restores; parity failures are read from junit, not the exit code.
    subprocess.run(
        ["pixi", "run", "pytest", "-m", marker, "-p", "no:cacheprovider",
         "--junit-xml", str(junit), "-q"],
        cwd=REPO_ROOT,
        env=env,
        check=False,
    )
    # A tier that never got as far as writing a report has not passed and has
    # not failed a listed test either: it did not run. Saying so beats an
    # ElementTree traceback, and beats an empty failure list read as success --
    # which would take the manifest's stale-entry half down with it.
    if not junit.exists() or junit.stat().st_size == 0:
        raise ManifestError(
            f"tier {name}: pytest wrote no junit report, so the tier's result is "
            "unknown (a collection error, or an environment that could not start "
            "pytest at all)"
        )
    failed = []
    root = ET.parse(junit).getroot()
    for case in root.iter("testcase"):
        if case.find("failure") is not None or case.find("error") is not None:
            classname = case.get("classname", "").replace(".", "/")
            failed.append(f"{classname}.py::{case.get('name')}")
    junit.unlink(missing_ok=True)
    return failed


def cmd_run() -> int:
    manifest = parse_manifest(MANIFEST.read_text())
    if manifest.sentinel_all:
        print(
            "parity run: manifest is the day-one ALL sentinel — the Rust "
            "binary is not expected to pass anything yet; skipping the pytest "
            "tiers (scaffold checks are the gate)"
        )
        return 0
    subprocess.run(
        ["cargo", "build", "--release", "--locked", "-p", "dl", "-p", "aid"],
        cwd=RUST_DIR,
        check=True,
    )
    release = RUST_DIR / "target" / "release"
    dl_cmd, aid_cmd = str(release / "dl"), str(release / "aid")
    failed = _run_tier("tier1", "not e2e", dl_cmd, aid_cmd) + _run_tier(
        "tier2", "e2e", dl_cmd, aid_cmd
    )
    unexpected, stale = compare_failures(failed, manifest.patterns)
    for f in unexpected:
        print(f"parity run: UNEXPECTED FAILURE {f} (not in the manifest)", file=sys.stderr)
    for p in stale:
        print(
            f"parity run: STALE MANIFEST ENTRY {p} (now passes — shrink the "
            "manifest in this PR)",
            file=sys.stderr,
        )
    if not unexpected and not stale:
        print(f"parity run: ok ({len(failed)} expected failures remain)")
    return 1 if (unexpected or stale) else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["lint", "self-test", "run"])
    args = parser.parse_args()
    try:
        return {"lint": cmd_lint, "self-test": cmd_self_test, "run": cmd_run}[args.command]()
    except ManifestError as error:
        # The gate's own complaints are sentences, not tracebacks: a manifest
        # that mixes the ALL sentinel with entries, or a tier that never wrote a
        # report, is a thing to read and act on rather than a stack to decode.
        print(f"parity {args.command}: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
