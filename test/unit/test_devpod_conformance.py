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

Both drivers also hold the corpus's invariants, rather than one holding them for
both: the roll call of row ids, the shape the named rows keep, and every row's
provenance line. Those checks lived on the Rust side alone, so a corpus edited
from here could lose a row or a provenance line with every suite still green.

A row is an id, seeded state, argv, expected exit code and the workspaces expected
afterwards. Not stdout: real devpod's own answer to a missing-and-ignored delete
is a timestamped, colourised log line no fake reproduces and nothing in this repo
parses, so pinning text would pin a fake's invention rather than reality. The
output *shapes* that are parsed -- `list --output json` carrying no state field
and the rest -- are pinned in `test_devpod_shim.py` against real recordings.
"""

# Requesting a fixture shadows its name; that is how pytest is written.
# pylint: disable=redefined-outer-name

import importlib.util
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest

FIXTURES = Path(__file__).parent.parent / "fixtures"
SHIM = FIXTURES / "devpod_shim.py"
CORPUS = FIXTURES / "devpod" / "conformance.json"
RUST_DRIVER = (
    Path(__file__).parent.parent.parent
    / "rust"
    / "devlaunch-test-support"
    / "src"
    / "devpod"
    / "conformance.rs"
)
RUST_FAKE = (
    Path(__file__).parent.parent.parent / "rust" / "devlaunch-test-support" / "src" / "devpod.rs"
)

#: Every row this driver expects to find in the corpus, by `id`.
#:
#: The roll call, and the answer to a corpus row being deleted with every suite
#: still green -- which is what the old guard allowed, because it asked only
#: whether certain flag *names* appeared somewhere across the rows. #310's own
#: regression row could go: its sibling still mentioned the flag. The Rust driver
#: holds the same list, so a row has to be removed from three files in two
#: languages before it stops being tested.
ROLL_CALL = [
    "delete-missing-with-ignore-not-found",
    "delete-missing-with-leading-ignore-not-found",
    "delete-missing-without-the-flag",
    "delete-removes-the-workspace",
    "delete-present-with-ignore-not-found",
    "stop-missing-workspace",
    "stop-running-workspace",
    "up-full-production-argv",
    "up-derives-id-with-trailing-flags",
    "up-init-env-before-source",
    "up-mount-before-source",
    "up-dotfiles-script-before-source",
    "up-dotfiles-script-env-before-source",
    "up-workspace-env-file-before-source",
    "up-restarts-a-stopped-workspace",
    "ssh-trailing-workdir-starts-workspace",
    "ssh-workdir-before-workspace",
    "ssh-workdir-and-command",
    "ssh-missing-workspace",
    "status-missing-workspace",
    "status-leaves-the-workspace-alone",
    "list-on-an-empty-machine",
    "list-leaves-workspaces-alone",
    "unknown-command",
]

#: The shape each named row has to keep: which subcommand, which flag, and which
#: side of the positional it sits on. Keyed by row id, so the guard is about
#: *that row* rather than about the flag appearing anywhere in the file. Cobra
#: takes a value flag on either side of the positional, and only the leading
#: position tells a value flag from a bare one -- read one as bare there and its
#: value becomes the workspace.
REQUIRED_SHAPES = [
    ("delete-missing-with-ignore-not-found", "delete", "--ignore-not-found", "after"),
    ("delete-missing-with-leading-ignore-not-found", "delete", "--ignore-not-found", "before"),
    ("up-init-env-before-source", "up", "--init-env", "before"),
    ("up-mount-before-source", "up", "--mount", "before"),
    ("up-dotfiles-script-before-source", "up", "--dotfiles-script", "before"),
    ("up-dotfiles-script-env-before-source", "up", "--dotfiles-script-env", "before"),
    ("up-workspace-env-file-before-source", "up", "--workspace-env-file", "before"),
    ("ssh-workdir-before-workspace", "ssh", "--workdir", "before"),
    ("ssh-trailing-workdir-starts-workspace", "ssh", "--workdir", "after"),
]

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
    """A row's `id` is what the roll call and a pytest failure both call it."""
    return [row["id"] for row in rows]


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
    assert CORPUS.exists()
    assert "test/fixtures/devpod/conformance.json" in RUST_DRIVER.read_text(encoding="utf-8")


def test_the_corpus_answers_the_roll_call():
    """A corpus row cannot leave without a test failing by its name."""
    found = [row["id"] for row in _rows()]

    missing = [row_id for row_id in ROLL_CALL if row_id not in found]
    assert not missing, f"the corpus has lost rows the roll call names: {missing}"

    unexpected = [row_id for row_id in found if row_id not in ROLL_CALL]
    assert not unexpected, (
        f"the corpus carries rows this driver does not name: {unexpected} -- "
        "a new row is added to the roll call in both drivers"
    )

    assert len(set(found)) == len(found), (
        "two corpus rows share an id, so one of them is not guarded by name"
    )


def test_the_rows_the_decision_named_keep_their_shape():
    """The minimum set #309 committed to, bound to the rows that carry it.

    The drift that motivated the corpus, every `up` value flag production sends
    that both fakes mis-parsed, and ssh's `--workdir`. Bound to row ids rather
    than to the file as a whole, so moving a flag out of the row supposed to
    carry it fails even when some other row still mentions it.
    """
    by_id = {row["id"]: row for row in _rows()}
    for row_id, verb, flag, side in REQUIRED_SHAPES:
        assert row_id in by_id, f"no corpus row with id {row_id!r}"
        argv = by_id[row_id]["argv"]
        assert argv[0] == verb, f"row {row_id!r} is supposed to exercise {verb}"
        # Both shapes are read off argv[1], the subcommand's first word: either
        # the flag leads and the positional follows its value, or the positional
        # leads and the flag trails. Deciding it that way keeps the guard clear of
        # the value-flag tables, which are the thing under test.
        if side == "before":
            assert argv[1] == flag, (
                f"row {row_id!r} must put {flag} ahead of {verb}'s positional, which "
                f"is the shape that tells a value flag from a bare one: {argv}"
            )
        else:
            assert not argv[1].startswith("-"), (
                f"row {row_id!r} must lead with {verb}'s positional: {argv}"
            )
            assert flag in argv[2:], (
                f"row {row_id!r} no longer passes {flag} after {verb}'s positional: {argv}"
            )


def test_every_row_says_how_it_was_verified():
    """Provenance travels with the row, enforced from this side too.

    A behaviour measured against the real binary and one inherited from the two
    fakes agreeing are different claims, and the drift got in by collapsing them.
    This ran on the Rust side alone, so a corpus edited from here could lose a
    provenance line with nothing to say so: an invariant enforced on one side of a
    file two suites edit is an invariant with a hole in it.
    """
    for row in _rows():
        assert row["verified"].lower().startswith(("measured", "unverified")), (
            f"row {row['name']!r} must open its `verified` with `measured` or "
            f"`unverified`, said {row['verified']!r}"
        )
        assert row["why"].strip(), f"row {row['name']!r} must say why it is here"
        assert row["name"].strip(), f"row {row['id']!r} must have a name to fail under"
        assert re.fullmatch(r"[a-z0-9-]+", row["id"]), (
            f"row id {row['id']!r} must be lowercase, digits and dashes, so it "
            "reads the same in both drivers"
        )


def _shim_module():
    """The shim imported as a module, for its own value-flag tables.

    Every other test here spawns it as a program, which is what it is; this one
    is about the constants inside it, and importing beats parsing its source.
    """
    spec = importlib.util.spec_from_file_location("devpod_shim_tables", SHIM)
    assert spec is not None and spec.loader is not None, f"{SHIM} is not importable"
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _rust_table(source: str, name: str) -> list:
    """The flag names in one of the Rust fake's tables, read out of its source.

    Reading text rather than linking Rust is the point, and it is the mirror of
    what the Rust driver does to this file: each side compares its own live
    values against the other side's source, so neither test can be fooled by
    misparsing the language it is written in.
    """
    match = re.search(
        rf"const {name}: &\[&str\] = &\[(.*?)\];",
        source,
        re.DOTALL,
    )
    assert match, f"the Rust fake no longer defines {name}"
    return re.findall(r'"([^"]+)"', match.group(1))


def test_the_two_fakes_agree_on_which_flags_take_a_value():
    """The tables deciding which flags consume the next argv element are one list.

    They are written out twice, once per fake, and nothing compared them: dropping
    a flag from one left every suite green, because only ~14 of the ~44 names have
    a corpus row to catch it behaviourally. The corpus proves the two fakes agree
    on the calls it covers; this proves they agree on the tables that decide the
    rest.
    """
    shim = _shim_module()
    rust = RUST_FAKE.read_text(encoding="utf-8")

    def ours(name):
        """The shim's copy of one table.

        Its tables are underscore-named because nothing was ever meant to import
        them; reading them is this test's whole job, and `getattr` says that
        without asking the shim to widen its surface for a test.
        """
        return getattr(shim, f"_{name}")

    for name, table in [
        ("GLOBAL_VALUE_FLAGS", "the globals"),
        ("UP_VALUE_FLAGS", "up"),
        ("SSH_VALUE_FLAGS", "ssh"),
        ("DELETE_VALUE_FLAGS", "delete"),
        ("STATUS_VALUE_FLAGS", "status"),
    ]:
        assert sorted(ours(name)) == sorted(_rust_table(rust, name)), (
            f"the two fakes disagree on which {table} flags take a value; a flag "
            "in one table and not the other is read as bare by one fake, which "
            "makes its value that call's positional"
        )

    # `stop` has no flags of its own, which no list of names can express.
    assert not ours("STOP_VALUE_FLAGS")
    assert "const STOP_VALUE_FLAGS: &[&str] = &[];" in rust, (
        "the Rust fake's stop table is no longer empty, and this one is"
    )
