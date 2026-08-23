"""The PATH shim, driven over the shared devpod conformance corpus.

There are two hand-written fake devpods in this repo -- the shim program here
and the Rust `DevpodMachine` behind the fake runner -- and each was only ever
tested against itself, which is exactly how they drifted: one refused `delete
--ignore-not-found` where real devpod exits 0, for months after the other was
fixed for it.

`test/fixtures/devpod/conformance.json` holds the expectations once, as real
devpod v0.26.1's behaviour with per-row provenance, and both fakes are driven
over it: this module spawns the shim per row, and
`rust/devlaunch-test-support/src/devpod/conformance.rs` runs the in-process fake
over the same file. A row that only one of them honours now fails somewhere.

A row is seeded state, argv, expected exit code and the workspaces expected
afterwards. Not stdout: real devpod's own answer to a missing-and-ignored delete
is a timestamped, colourised log line no fake reproduces and nothing in this repo
parses, so pinning text would pin a fake's invention rather than reality. The
output *shapes* that are parsed -- `list --output json` carrying no state field
and the rest -- are pinned in `test_devpod_shim.py` against real recordings.
"""

# Requesting a fixture shadows its name; that is how pytest is written.
# pylint: disable=redefined-outer-name

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

FIXTURES = Path(__file__).parent.parent / "fixtures"
SHIM = FIXTURES / "devpod_shim.py"
CORPUS = FIXTURES / "devpod" / "conformance.json"

#: The default fields a shim state entry carries beyond what a corpus row names.
#: The corpus says only what a row's outcome depends on -- id, source, state --
#: so the rest is filled with devpod's own defaults here.
_STAMP = "2026-01-01T00:00:00+0000"


def _rows():
    return json.loads(CORPUS.read_text(encoding="utf-8"))["rows"]


def _source_object(source: str) -> dict:
    """What devpod records for a source: a path one way, a URL another."""
    looks_remote = "://" in source or source.startswith("git@")
    if not looks_remote and (source.startswith(("/", ".", "~")) or os.path.exists(source)):
        return {"localFolder": source}
    return {"gitRepository": source}


def _seeded_state(given) -> dict:
    return {
        "workspaces": {
            seed["id"]: {
                "id": seed["id"],
                "source": _source_object(seed["source"]),
                "lastUsed": _STAMP,
                "provider": {"name": "docker"},
                "ide": {"name": "none"},
                "context": "default",
                "state": seed["state"],
            }
            for seed in given
        },
        "providers": {"docker": {"config": {"name": "docker"}}},
    }


def _workspaces_now(state_file: Path):
    """Every workspace the shim left behind, as a corpus `then` list."""
    data = json.loads(state_file.read_text(encoding="utf-8"))
    return sorted(
        ({"id": ws["id"], "state": ws["state"]} for ws in data.get("workspaces", {}).values()),
        key=lambda entry: entry["id"],
    )


def _ids(rows):
    return [row["name"] for row in rows]


@pytest.mark.parametrize("row", _rows(), ids=_ids(_rows()))
def test_the_shim_honours_every_row_of_the_corpus(row, tmp_path):
    state_file = tmp_path / "shim-state.json"
    state_file.write_text(json.dumps(_seeded_state(row["given"])), encoding="utf-8")

    env = dict(os.environ)
    env["DEVPOD_SHIM_STATE"] = str(state_file)
    env.pop("DEVPOD_SHIM_CONFIG", None)
    env.pop("DEVPOD_SHIM_LOG", None)
    result = subprocess.run(
        [sys.executable, str(SHIM), *row["argv"]],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == row["exit"], (
        f"exit code for {row['name']!r} (argv {row['argv']})\n"
        f"  {row['verified']}\n"
        f"  stdout: {result.stdout!r}\n  stderr: {result.stderr!r}"
    )
    assert _workspaces_now(state_file) == row["then"], (
        f"workspaces after {row['name']!r} (argv {row['argv']})\n  {row['verified']}"
    )


def test_the_corpus_is_the_one_the_rust_side_reads():
    """Both fakes read the same file, so neither can be given a private copy.

    The Rust driver reaches this path with `include_str!`, which fails the build
    if it moves; this is the other half of that -- the Python side names the same
    file, and a second corpus alongside it would defeat the whole mechanism.
    """
    rust_driver = (
        Path(__file__).parent.parent.parent
        / "rust"
        / "devlaunch-test-support"
        / "src"
        / "devpod"
        / "conformance.rs"
    )
    assert CORPUS.exists()
    assert "test/fixtures/devpod/conformance.json" in rust_driver.read_text(encoding="utf-8")
