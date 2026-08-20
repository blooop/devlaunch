#!/usr/bin/env python3
"""Check a built devlaunch wheel before anything installs or publishes it.

The wheel is not a Python package any more: since 0.1.0 it is a container for two
compiled binaries, built by maturin from ``packaging/wheel/pyproject.toml``. That
makes two things worth asserting mechanically, both of which are silent failures
otherwise:

* **Both binaries are in there.** They come from *one* cargo package (`rust/dl`,
  whose second bin target sits behind the ``wheel`` feature), so a lost feature
  flag or a renamed target ships a wheel with `dl` and no `aid` -- a wheel that
  installs cleanly and leaves half the tool missing.
* **The version in the wheel is the version being released.** It is read from
  ``rust/Cargo.toml`` by maturin, by the conda recipe and by hatchling
  independently; a mismatch means one of those readers moved.

Nothing here needs the wheel installed, so it runs before the venv smoke test in
``.github/workflows/publish.yml`` and on every pull request from
``.github/workflows/ci.yml``.

Usage:

    python3 scripts/check_wheel.py <wheel> --version <expected>
"""

from __future__ import annotations

import argparse
import sys
import zipfile
from pathlib import Path

# What the wheel must contain, and all it may contain beyond `.dist-info`: the two
# binaries, as `.data/scripts/` entries that pip drops into the environment's bin/.
BINARIES = ("dl", "aid")


def check(wheel: Path, version: str) -> list[str]:
    """Return the problems with `wheel`, empty when there are none."""
    problems: list[str] = []
    with zipfile.ZipFile(wheel) as archive:
        names = archive.namelist()

    expected_name = f"devlaunch-{version}-"
    if not wheel.name.startswith(expected_name):
        problems.append(f"{wheel.name} is not named for version {version}")

    for binary in BINARIES:
        member = f"devlaunch-{version}.data/scripts/{binary}"
        if member not in names:
            problems.append(f"the wheel has no {member}")

    # A Python module in here would mean something got packaged alongside the
    # binaries that should not be. The Python build this used to guard against is
    # gone entirely (#267), so today this is a guard against a future maturin or
    # recipe change quietly widening the payload.
    modules = [name for name in names if name.startswith("devlaunch/")]
    if modules:
        problems.append(f"the wheel ships Python modules it should not: {modules}")

    if not problems:
        print("\n".join(sorted(names)))
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wheel", type=Path, help="the .whl to check")
    parser.add_argument("--version", required=True, help="the version the release is being cut at")
    args = parser.parse_args(argv)

    problems = check(args.wheel, args.version)
    for problem in problems:
        print(f"error: {problem}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
