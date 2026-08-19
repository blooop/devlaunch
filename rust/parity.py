#!/usr/bin/env python3
"""The rust-parity gate: spec-ledger lint and the two-way expected-failure ratchet.

Stdlib only, so CI can run the lint with a bare python3 before any environment
exists. Three subcommands:

  lint       spec-ledger coverage + disposition rules + the `pending` count
             consistency check against rust/pending-count.txt (the count is
             reviewed in the PR, not ratcheted against main), plus a manifest
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

# pytest's exit code for "collected no tests". The ratchet reads failures from
# the junit report and would otherwise score a run that collected nothing -- a
# bad marker expression, a broken conftest, a wrong cwd -- as zero failures, i.e.
# a pass. See `check_tier_ran`.
PYTEST_NO_TESTS_RC = 5

# Loose backstops, not a census. Their only job is to catch a tier that
# collapsed to nothing (or nearly), which is the shape rc==5 and a crashed
# collection take; they are set well below the real counts (tier 1 is ~1874
# tests, tier 2 ~a dozen e2e) precisely so ordinary test churn never trips them.
# A tier that dips below its floor did not run enough to trust its silence.
TIER1_FLOOR = 1000
TIER2_FLOOR = 3

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


def lint_pending_count(current: int, recorded: int) -> list[str]:
    """rust/pending-count.txt must equal the ledger's live `pending` row count.

    This is a consistency check reviewed in the PR, not a ratchet: `recorded` is
    read from the working tree (the same PR that changes the ledger changes this
    number), so it cannot by itself stop the count growing across PRs -- a
    reviewer reading the diff does that. What it does catch mechanically is the
    file drifting out of step with the ledger it summarises, in either direction.
    """
    if current > recorded:
        return [
            f"pending ledger rows grew {recorded} -> {current}; a new file "
            "landed without a real disposition. If that is intended, record it "
            f"by updating rust/pending-count.txt to {current} in this PR (the "
            "count is reviewed in the PR, so a rise has to be a deliberate edit)"
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


def reconstruct_node_id(classname: str, name: str, is_test_file) -> str:
    """Rebuild a pytest node id (`test/path.py::[Class::]name`) from a junit
    `<testcase>`'s `classname` and `name`.

    junit's `classname` is the dotted import path of the test's module with any
    enclosing class names appended: a module-level `test/test_dl.py::test_x` is
    `test.test_dl`, and `test/test_x.py::TestThing::test_y` is
    `test.test_x.TestThing`. Where the module ends and the class nesting begins
    cannot be read off the dotted string -- a dot is a directory separator or a
    class boundary and the name does not say which. So the longest dotted prefix
    that names a real test file wins, and whatever dots trail it are classes.

    This is the fix for the old `classname.replace(".", "/") + ".py::" + name`,
    which turned `test.test_x.TestThing` into `test/test_x/TestThing.py::test_y`
    -- an id that matched no manifest entry and no real file, so every
    class-based failure (the bulk of the suite) was unmatchable.

    `is_test_file(rel_path)` answers whether a repo-relative path is a test file;
    it is injected so the logic is unit-testable without touching the disk.
    """
    parts = classname.split(".") if classname else []
    for i in range(len(parts), 0, -1):
        candidate = "/".join(parts[:i]) + ".py"
        if is_test_file(candidate):
            segments = [*parts[i:], name]
            return f"{candidate}::{'::'.join(segments)}"
    # No prefix names a file we know: keep the failure legible and surfaced (a
    # dropped id is a failure the ratchet cannot see) rather than inventing a
    # path. This is the best-effort fallback, not the expected path.
    base = "/".join(parts)
    return f"{base}.py::{name}" if base else name


def check_tier_ran(name: str, rc: int, total: int, floor: int, n_failures: int) -> None:
    """Refuse to trust a tier's silence unless it actually ran.

    The ratchet reads failures from junit and ignores the exit code, so three
    shapes score as a clean pass without having tested anything: a run that
    collected nothing (rc 5), a run below its testcase floor, and a run pytest
    reports as failed (rc != 0) while its junit lists no failures -- a crash, an
    internal or usage error, or a session-floor guard that travels only by exit
    code (test/e2e/conftest.py's `pytest_sessionfinish`). Each raises here.

    Split out from `_run_tier` so it is exercised by the self-test rather than
    only by a full CI run.
    """
    if rc == PYTEST_NO_TESTS_RC:
        raise ManifestError(
            f"tier {name}: pytest collected no tests (exit {PYTEST_NO_TESTS_RC}) "
            "— a marker expression that matched nothing, a broken conftest, or a "
            "wrong working directory. An empty run is not a passing run"
        )
    if total < floor:
        raise ManifestError(
            f"tier {name}: the junit report holds {total} testcases, below the "
            f"floor of {floor} — the tier collapsed to almost nothing, so its "
            "empty failure list cannot be read as a pass"
        )
    if rc != 0 and n_failures == 0:
        raise ManifestError(
            f"tier {name}: pytest exited {rc} but its junit report lists no "
            "failures — a crash, an internal or usage error, or a guard that "
            "sets the exit code without filing a testcase. The gate reads "
            "failures from junit, so this would otherwise score as a pass"
        )


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
    errors.extend(lint_pending_count(current_pending, recorded))
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


def _run_tier(name: str, marker: str, dl_cmd: str, aid_cmd: str, floor: int) -> list[str]:
    """Run one pytest tier against the Rust binaries; return failed node ids.

    Both seams are exported, not just `dl`'s: `aid` is a second binary on the
    Rust side (#252 §1) and a tier that redirected only `dl` would judge the
    Python `aid` against the Rust `dl` -- a pair that ships together and would
    then never be tested together.

    The exit code is not thrown away. A tier that never wrote a report, that
    collected nothing, that ran too little to trust, or that pytest reports as
    failed while its junit lists no failures is not a pass -- `check_tier_ran`
    turns each of those into a spoken error instead of an empty failure list
    read as success, which would take the manifest's stale-entry half down with
    it.
    """
    junit = Path(tempfile.mkstemp(prefix=f"parity-{name}-", suffix=".xml")[1])
    env = dict(os.environ, DEVLAUNCH_DL_CMD=dl_cmd, DEVLAUNCH_AID_CMD=aid_cmd)
    # Through pixi so the tier runs in the project environment CI already
    # restores.
    proc = subprocess.run(
        [
            "pixi",
            "run",
            "pytest",
            "-m",
            marker,
            "-p",
            "no:cacheprovider",
            "--junit-xml",
            str(junit),
            "-q",
        ],
        cwd=REPO_ROOT,
        env=env,
        check=False,
    )
    if not junit.exists() or junit.stat().st_size == 0:
        raise ManifestError(
            f"tier {name}: pytest wrote no junit report, so the tier's result is "
            "unknown (a collection error, or an environment that could not start "
            "pytest at all)"
        )
    cases = list(ET.parse(junit).getroot().iter("testcase"))
    junit.unlink(missing_ok=True)
    failed = [
        reconstruct_node_id(case.get("classname", ""), case.get("name", ""), _repo_has_test_file)
        for case in cases
        if case.find("failure") is not None or case.find("error") is not None
    ]
    check_tier_ran(name, proc.returncode, len(cases), floor, len(failed))
    return failed


def _repo_has_test_file(rel_path: str) -> bool:
    return (REPO_ROOT / rel_path).is_file()


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
    failed = _run_tier("tier1", "not e2e", dl_cmd, aid_cmd, TIER1_FLOOR) + _run_tier(
        "tier2", "e2e", dl_cmd, aid_cmd, TIER2_FLOOR
    )
    unexpected, stale = compare_failures(failed, manifest.patterns)
    for f in unexpected:
        print(f"parity run: UNEXPECTED FAILURE {f} (not in the manifest)", file=sys.stderr)
    for p in stale:
        print(
            f"parity run: STALE MANIFEST ENTRY {p} (now passes — shrink the manifest in this PR)",
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
