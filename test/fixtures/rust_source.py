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


def _str_const(source: str, name: str, path: Path) -> str:
    """The value of a `const NAME: &str = "...";` declaration."""
    match = re.search(rf'const {re.escape(name)}: &str = "([^"]*)";', source)
    assert match, f"{path.name} no longer declares a `const {name}: &str`"
    return match.group(1)


def timing_stages() -> Tuple[str, ...]:
    """The stage vocabulary, in the order a launch meets it.

    Parsed from `Stage::name()`'s match arms rather than from `Stage::ALL`,
    because the arms are what decide the strings that reach the timing document,
    and the document is what everything downstream reads. The two are written in
    the same order and `stage_order_matches_all` below checks they still are.
    """
    source = _read(TIMING_RS)
    body = re.search(r"fn name\(self\) -> &'static str \{(.+?)\n    \}", source, re.S)
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
