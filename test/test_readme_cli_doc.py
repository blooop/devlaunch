"""The CLI surface README publishes, judged against the CLI that ships (#307).

The README is the only place a reader learns what `dl` takes, and it is a long
document whose flag examples nothing checked. That is how the worktree-backend
section came to publish worked examples of `--shared` and `--warm` -- two flags
the Rust `dl` has never had, which exit 2 on contact -- and how the transcript
under `dl --version` came to quote a release four minors behind the one it ships.
The class is the same in both cases: a hand-written copy of a fact the binary
already knows.

So the two facts are asked of their owners instead:

- *which flags exist* of the binary's own argument parser, by handing it each
  flag the README writes and seeing whether it is refused as unknown. Parsing
  `--help` would answer a different question -- it lists the flags meant for a
  reader, not the flags that work -- and the README does document one of the
  hidden completion flags;
- *which version ships* of `rust/Cargo.toml`, which is where the version is
  written down and nowhere else (see "The two published artifacts" in README.md).

Two things this file deliberately does not do. It does not read every `--word` in
the document: the README also explains git's `--shared`/`--reference` and runs
plenty of `cargo` and `pixi` lines, and a guard that failed on those would be
failing for a reason no reader could act on -- which is how guards get deleted.
What is extracted is the flags written on a line whose first word is `dl`. And it
does not require the README to keep documenting any particular flag; but if it
stops documenting them *all*, it has to stop loudly, which is why the extraction
is asserted non-empty before anything is parametrized over it.
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

import pytest

# stdlib since 3.11, which is the floor now that nothing ships on an interpreter.
import tomllib

# The argv prefix of the implementation under test -- the `DEVLAUNCH_DL_CMD` seam
# the rest of the acceptance harness judges `dl` through. Imported rather than
# rebuilt so this guard cannot end up asking a different binary than the suite
# around it.
from fixtures.e2e_helpers import dl_command

REPO_ROOT = Path(__file__).resolve().parent.parent
README = REPO_ROOT / "README.md"
CARGO_WORKSPACE = REPO_ROOT / "rust" / "Cargo.toml"

FENCE = re.compile(r"^```[^\n]*\n(.*?)^```", re.MULTILINE | re.DOTALL)
INLINE_CODE = re.compile(r"`([^`\n]+)`")
LONG_FLAG = re.compile(r"(?<![\w-])--[a-z][a-z0-9-]*")

# Where one command line ends and something else begins: a pipe, a chain, or the
# bare `--` after which the words belong to the command run *inside* the
# workspace. Everything past the first of these is somebody else's argv.
END_OF_COMMAND = re.compile(r"\|\||&&|[|;]|(?<=\s)--(?=\s|$)")

# Two real flags that cannot be used together. Prefixed to every probe so the
# parser refuses the line whatever the third flag turns out to be: the probe
# never launches, stops, deletes or lists anything, and the only thing it reads
# off the failure is *which* refusal it got.
MUTUALLY_EXCLUSIVE_PROBE = ["--ls", "--purge"]


def _readme() -> str:
    return README.read_text(encoding="utf-8")


def _code_spans(text: str) -> list[str]:
    """Every fenced line and every inline-code span in the document.

    Both, because the README documents flags in both places: the command tables
    and most of the prose use inline code, the transcripts use fences.
    """
    spans = []
    for fence in FENCE.finditer(text):
        spans.extend(fence.group(1).splitlines())
    spans.extend(match.group(1) for match in INLINE_CODE.finditer(text))
    return spans


def dl_command_lines(text: str) -> list[str]:
    """The code spans that are `dl` invocations, trimmed to their own argv.

    The first word has to be `dl` -- that is the attribution this guard rests on,
    and it is what keeps the neighbouring `git clone --shared` and `cargo build
    --release` examples out of a check about `dl`'s flags. A leading `$` prompt is
    stripped because the transcripts carry one.
    """
    lines = []
    for span in _code_spans(text):
        line = re.sub(r"^\$\s+", "", span.strip())
        head = END_OF_COMMAND.split(line)[0].strip()
        words = head.split()
        if words and words[0] == "dl":
            lines.append(head)
    return lines


def documented_flags() -> dict[str, str]:
    """Every long flag the README writes on a `dl` line, and one line writing it.

    The example line comes back with the flag so a failure can name where to
    look without this file storing a line number, which would rot on the next
    paragraph inserted above it.
    """
    found: dict[str, str] = {}
    for line in dl_command_lines(_readme()):
        for flag in LONG_FLAG.findall(line):
            found.setdefault(flag, line)
    return found


DOCUMENTED_FLAGS = documented_flags()


def _shipped_version() -> str:
    """The one place the version is written down."""
    return tomllib.loads(CARGO_WORKSPACE.read_text(encoding="utf-8"))["workspace"]["package"][
        "version"
    ]


@pytest.mark.unit
def test_the_readme_still_documents_dl_flags():
    """The premise everything below is parametrized over.

    A README rewritten out from under this file would leave the parametrized
    check collecting nothing and reporting a clean run, which reads exactly like
    a check that passed. The floor is deliberately low -- this is a smoke test
    for the extraction, not an opinion about how many flags belong in the
    document.
    """
    assert len(DOCUMENTED_FLAGS) >= 8, (
        f"only {sorted(DOCUMENTED_FLAGS)} were extracted from README `dl` command "
        "lines; either the document stopped documenting flags or the extraction "
        "no longer recognises how it writes them"
    )


@pytest.mark.integration
@pytest.mark.parametrize("flag", sorted(DOCUMENTED_FLAGS))
def test_every_flag_the_readme_hands_a_reader_is_a_flag_dl_accepts(flag):
    """A flag in a `dl` line the reader can copy must not exit 2 on contact.

    Asked of the parser rather than of `--help`, so a flag that exists but is
    hidden from the help (the completion plumbing) counts as existing, which is
    the honest answer to "would this work if I typed it".
    """
    probe = subprocess.run(
        dl_command() + MUTUALLY_EXCLUSIVE_PROBE + [flag],
        capture_output=True,
        text=True,
        check=False,
    )
    complaint = probe.stdout + probe.stderr
    rejected_as_unknown = "unexpected argument" in complaint and flag in complaint
    first_line = complaint.strip().splitlines()[0] if complaint.strip() else "(no output)"
    assert not rejected_as_unknown, (
        f"README documents `{DOCUMENTED_FLAGS[flag]}`, but dl has no {flag}: {first_line}"
    )


# ---------------------------------------------------------------------------
# the version the README quotes
# ---------------------------------------------------------------------------

# The transcript whose output line is the claim: the fence that runs
# `dl --version`. Matched on the command rather than on a heading, because the
# section around it is free to be rewritten and the transcript is not free to be
# wrong.
VERSION_TRANSCRIPT = re.compile(r"^\$ dl --version\n(?P<output>.+)$", re.MULTILINE)

# The conda badge, which republishes the same fact as a picture. Only the version
# is captured; the colour and the logo are nobody's business here.
CONDA_BADGE = re.compile(r"img\.shields\.io/badge/conda-v(?P<version>[0-9][^-]*)-")


@pytest.mark.unit
def test_the_version_transcript_quotes_the_version_that_ships():
    """`dl --version` printed in the README must be what it prints.

    This is the guard's cheapest catch and the one that had already gone wrong:
    the transcript quoted 0.1.0 for four minor releases, under a paragraph
    promising "the version and nothing else". Compared against
    `rust/Cargo.toml` rather than against a `dl --version` run, because that file
    is where the version is written down -- a run would confirm the binary agrees
    with itself and let a `-dev` build launder a stale number.
    """
    transcript = VERSION_TRANSCRIPT.search(_readme())
    assert transcript, "the README no longer shows a `$ dl --version` transcript"
    assert transcript.group("output").strip() == f"dl {_shipped_version()}", (
        f"the README quotes {transcript.group('output').strip()!r} as the output of "
        f"`dl --version`; rust/Cargo.toml says the version is {_shipped_version()}"
    )


@pytest.mark.unit
def test_the_conda_badge_names_the_version_that_ships():
    """The badge is a hand-written version string wearing a picture.

    Every other badge in the header is generated from live state by shields.io;
    this one hard-codes the number, and had been advertising 0.0.9 since well
    before 0.6.0 shipped. Guarded rather than removed because the channel it
    points at is real: what is wrong is the copy, not the link.
    """
    badge = CONDA_BADGE.search(_readme())
    assert badge, "the README header no longer carries a conda version badge"
    assert badge.group("version") == _shipped_version(), (
        f"the conda badge advertises v{badge.group('version')}; rust/Cargo.toml says "
        f"the version is {_shipped_version()}. Both move together at release."
    )
