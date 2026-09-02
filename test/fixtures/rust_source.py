"""Constants the Python-side tests need, read out of the Rust source that owns them.

Since the Python implementation was retired (#267) the shipped tool is a binary,
and a test process cannot import its constants. Three ways out of that, and this
is the least bad one:

- *Duplicate the literals* in the test. Drifts silently, which is the failure
  this file exists to prevent: the whole value of a doc-drift test is that
  renaming the thing breaks the test.
- *Ask the binary*. Right where the binary already says something (`dl --help`,
  the timing document), and those tests do exactly that. But it means growing CLI
  surface for the benefit of tests wherever it does not.
- *Read the source*, here. A deliberate coupling to the Rust source's spelling,
  in one file, where it is the subject rather than a detail of some other test.

Every reader below asserts it found what it went looking for, so a rename or a
refactor in the Rust source fails loudly here rather than silently returning an
empty answer that the caller then compares against nothing.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Tuple

CORE = Path(__file__).resolve().parent.parent.parent / "rust" / "devlaunch-core" / "src"
TIMING_RS = CORE / "timing.rs"


def _read(path: Path) -> str:
    assert path.is_file(), f"{path} is gone; the Rust source moved and this reader must follow"
    return path.read_text(encoding="utf-8")


def _impl_block(type_name: str) -> str:
    """The body of `impl <type_name> { ... }` in timing.rs.

    Scoping matters more than it looks: `AttachShape` also has a
    `fn name(self) -> &'static str`, so a search over the whole file returns
    whichever impl happens to come first. Anchoring on the impl keeps a reordering
    of two unrelated types from changing what this reads.
    """
    source = _read(TIMING_RS)
    start = source.find(f"impl {type_name} {{")
    assert start != -1, f"{TIMING_RS.name} no longer has an `impl {type_name}` block"
    # To the next top-level item: the first line after `start` that begins in
    # column zero with a closing brace.
    end = source.find("\n}\n", start)
    assert end != -1, f"`impl {type_name}` in {TIMING_RS.name} is not closed at column zero"
    return source[start:end]


def _str_const(source: str, name: str, path: Path) -> str:
    """The value of a `const NAME: &str = "...";` declaration."""
    match = re.search(rf'const {re.escape(name)}: &str = "([^"]*)";', source)
    assert match, f"{path.name} no longer declares a `const {name}: &str`"
    return match.group(1)


def timing_stages() -> Tuple[str, ...]:
    """The stage vocabulary, in the order a launch meets it.

    Parsed from `Stage::name()`'s match arms rather than from `Stage::ALL`, because
    the arms are what decide the strings that reach the timing document, and the
    document is what everything downstream reads. `Stage::ALL` is read separately
    by `timing_stage_variants`, and `test/unit/test_rust_source.py` holds the two to
    the same order -- so this returning the vocabulary in a different order from the
    one the enum declares is a failure rather than a silent difference.
    """
    impl_block = _impl_block("Stage")
    body = re.search(r"fn name\(self\) -> &'static str \{(.+?)\n    \}", impl_block, re.S)
    assert body, "timing.rs no longer has a `Stage::name()` whose arms this can read"
    stages = tuple(re.findall(r'Stage::\w+ => "([^"]+)",', body.group(1)))
    assert stages, "timing.rs's `Stage::name()` yielded no stage names"
    return stages


def timing_stage_variants() -> Tuple[str, ...]:
    """`Stage::ALL`'s variants, in declaration order."""
    source = _read(TIMING_RS)
    body = re.search(r"const ALL: \[Stage; \d+\] = \[(.+?)\];", source, re.S)
    assert body, "timing.rs no longer declares `Stage::ALL`"
    return tuple(re.findall(r"Stage::(\w+)", body.group(1)))


def handoff_stage() -> str:
    """The one stage no launch runs: the gap before dl started.

    Named separately because it is the one `bench_points.py` does not require --
    a launch nobody handed off to has no handoff to report.
    """
    source = _read(TIMING_RS)
    match = re.search(r'Stage::Handoff => "([^"]+)",', source)
    assert match, "timing.rs no longer maps `Stage::Handoff` to a name"
    return match.group(1)


def timing_env_var() -> str:
    """The switch that asks dl for a timing summary."""
    return _str_const(_read(TIMING_RS), "ENV_VAR", TIMING_RS)


def timing_json_value() -> str:
    """The value of that switch that asks for the machine-readable document."""
    return _str_const(_read(TIMING_RS), "JSON_VALUE", TIMING_RS)


def timing_json_prefix() -> str:
    """The marker the machine-readable document line starts with."""
    return _str_const(_read(TIMING_RS), "JSON_PREFIX", TIMING_RS)


def timing_total_epoch() -> str:
    """What the document's `total` is measured over, as it reports it."""
    return _str_const(_read(TIMING_RS), "TOTAL_EPOCH", TIMING_RS)


# ---------------------------------------------------------------------------
# what a session manager is told, which two binaries spell separately
# ---------------------------------------------------------------------------

HERDR_RS = CORE / "clients" / "herdr.rs"
AID = Path(__file__).resolve().parent.parent.parent / "rust" / "aid" / "src"
AID_MAIN_RS = AID / "main.rs"
AID_REWRITE_RS = AID / "rewrite.rs"
LAUNCH_RS = CORE / "flows" / "launch.rs"
DL = Path(__file__).resolve().parent.parent.parent / "rust" / "dl" / "src"
DL_PANE_SHELL_RS = DL / "pane_shell.rs"


def _visible_str_const(source: str, name: str, path: Path) -> str:
    """`[pub[(crate)]] const NAME: &str = "...";`, whatever its visibility.

    Separate from `_str_const` because the two constants this reads differ in
    exactly that: core's is `pub(crate)` and aid's is private, and a reader that
    insisted on one spelling would pass by returning nothing about the other.
    """
    match = re.search(
        rf'(?:pub(?:\(crate\))?\s+)?const {re.escape(name)}: &str = "([^"]*)";', source
    )
    assert match, f"{path.name} no longer declares a `const {name}: &str`"
    return match.group(1)


def core_session_manager_agent_var() -> str:
    """The variable core writes for a `dl <ws> -- <agent>` that names an agent."""
    return _visible_str_const(_read(HERDR_RS), "AGENT_VAR", HERDR_RS)


def aid_session_manager_agent_var() -> str:
    """The variable aid writes for the session it opens."""
    return _visible_str_const(_read(AID_MAIN_RS), "SESSION_MANAGER_AGENT_VAR", AID_MAIN_RS)


def core_agent_names() -> Tuple[str, ...]:
    """The agents core will name to a session manager, as `AGENT_NAMES` lists them."""
    source = _read(HERDR_RS)
    match = re.search(r"const AGENT_NAMES: &\[&str\] = &\[([^\]]*)\];", source)
    assert match, f"{HERDR_RS.name} no longer declares `const AGENT_NAMES: &[&str]`"
    names = tuple(re.findall(r'"([^"]+)"', match.group(1)))
    assert names, f"{HERDR_RS.name} declares AGENT_NAMES with no names in it"
    return names


def aid_agent_names() -> Tuple[str, ...]:
    """The keys of aid's own agent table, which knows far more than their names.

    Read from the `const AGENTS: &[(&str, Agent)]` block rather than from
    `agent_names()`, because the table is the thing that decides which agents exist
    and the function is only its sorted keys.
    """
    source = _read(AID_REWRITE_RS)
    start = source.find("const AGENTS: &[(&str, Agent)] = &[")
    assert start != -1, f"{AID_REWRITE_RS.name} no longer declares `const AGENTS`"
    end = source.find("\n];", start)
    assert end != -1, f"`const AGENTS` in {AID_REWRITE_RS.name} is not closed at column zero"
    names = tuple(re.findall(r'^\s{8}"([^"]+)",$', source[start:end], re.MULTILINE))
    assert names, f"{AID_REWRITE_RS.name} declares AGENTS with no agent names in it"
    return names


def core_session_manager_tab_var() -> str:
    """The variable the pane shell reads to learn which tab it was spawned in.

    Declared in `flows/launch.rs` rather than beside herdr's other exports: the
    tab rename got there first, and one declaration in the less tidy place beats
    two in the right ones.
    """
    return _visible_str_const(_read(LAUNCH_RS), "HERDR_TAB_VAR", LAUNCH_RS)


def core_herdr_program() -> str:
    """The manager's own binary name, for a host that exports no path to it."""
    return _visible_str_const(_read(LAUNCH_RS), "HERDR_BIN_FALLBACK", LAUNCH_RS)


def core_pane_questions() -> Tuple[Tuple[str, ...], ...]:
    """The two argvs core builds to ask herdr what a tab holds.

    Read out of the two builders rather than out of a table, because the builders
    are what actually runs: a table beside them would be a fourth copy of the same
    words with nothing diffing it against the third.
    """
    source = _read(HERDR_RS)
    found = []
    for name in ("pane_list_argv", "process_info_argv"):
        start = source.find(f"pub(crate) fn {name}(")
        assert start != -1, f"{HERDR_RS.name} no longer declares `fn {name}`"
        end = source.find("\n}", start)
        assert end != -1, f"`fn {name}` in {HERDR_RS.name} is not closed at column zero"
        words = tuple(re.findall(r'"([^"]*)"\.to_owned\(\)', source[start:end]))
        assert words, f"`fn {name}` in {HERDR_RS.name} builds an argv with no literals in it"
        found.append(words)
    return tuple(found)


def dl_pane_shell_name() -> str:
    """The name `dl --install` links, and herdr's `default_shell` points at."""
    return _visible_str_const(_read(DL_PANE_SHELL_RS), "NAME", DL_PANE_SHELL_RS)
