"""The benchmarking commands the launch-time docs publish, run rather than read (#192).

There is already a guard of this shape, and it reads the wrong document. It
extracts the cold recipe from `bench_launch.py`'s own epilog and drives it
through `main()`, built after that recipe shipped twice with a `delete`
subcommand `dl` does not have. It works -- and the launch-time section carries its *own*
copies of these commands, which is where the third defect in the same lineage
actually shipped: a `python scripts/bench_launch.py` published to readers whose
host has no `python` outside pixi.

So the same recipe is pointed at the second document. Every fenced invocation of
a bench script under the launch-time section is read out of that document and then
handed to the things that would have caught each defect:

- the *interpreter* to a list of the two this repo can vouch for, because that
  is a claim about the reader's host and no parser will ever see it;
- the *flags* to the script's own parser, so a flag renamed or documented before
  it existed fails here;
- the *`pixi run` shortcuts* the prose offers beside them to the project
  manifest, since a renamed task leaves the sentence confidently wrong;
- any *`--before` reset* to the same nothing-to-delete harness the epilog's
  reset goes through.

Two things this file deliberately does not do. It does not check the numbers the
section quotes: those are a measurement of one host, not a command anything can
run. And it does not require the section to keep documenting these commands --
but if it stops, it has to stop *loudly*, which is why the extraction is
asserted non-empty before anything is parametrized over it. A guard whose
document was rewritten out from under it reads exactly like a guard that passed.
"""

import importlib.util
import re
import shlex
from pathlib import Path

import pytest

# stdlib since 3.11, which is the floor now that nothing ships on an
# interpreter (#267). It was `tomli` here, a *runtime* dependency of the
# Python `dl` -- which parsed config.toml with it -- that the tests borrowed.
import tomllib

# The same level-aware section splitter the lending-contract guard reads its
# section with. Imported rather than copied: a second implementation of "the
# text under this heading" is a second thing that can drift from the document.
from fixtures.markdown_sections import section as _section

# The harness the epilog's reset already goes through: the binary under test
# against a fake devpod that has nothing to delete. Shared rather than rebuilt, so
# the two documents' resets are judged against the same starting state.
from fixtures.bench_harness import a_reset_from_the_absent_state

REPO_ROOT = Path(__file__).resolve().parent.parent
# The launch-time material moved out of the README into docs/ when the README was
# cut back to an orientation document. What this file guards is that a reader
# sent to these commands can run them, not which file they are printed in, so the
# constant follows the prose rather than the prose being held in place by it.
BENCH_DOC = REPO_ROOT / "docs" / "performance.md"

# What the failure messages call the document, so a renamed target renames itself
# in every one of them rather than in none.
DOC = BENCH_DOC.relative_to(REPO_ROOT).as_posix()
PYPROJECT = REPO_ROOT / "pyproject.toml"
SCRIPTS = REPO_ROOT / "scripts"

# The one section whose fences are commands a reader runs against this checkout.
# Matched on the heading, so the prose under it stays free to be rewritten.
BENCH_SECTION = "## Measuring launch time"

# What may stand in front of a bench script on the *host*, and why only these
# two. `python3` is what a host that has any python at all provides under that
# name; a bare `python` is the defect that shipped, on a machine where it exists
# only inside pixi. `pixi run` is the other vouched-for route because the task
# it names brings its own interpreter -- which is why the task's own definition
# is allowed to say `python`, and why this list is about the documented command rather than
# about the project manifest.
VOUCHED_INTERPRETERS = ("python3", "pixi")

FENCE = re.compile(r"^```[^\n]*\n(.*?)^```", re.MULTILINE | re.DOTALL)
INVOKES_A_SCRIPT = re.compile(r"scripts/(\w+\.py)")


def has_bench_section() -> bool:
    """Whether the document still carries the heading everything here hangs off."""
    text = BENCH_DOC.read_text(encoding="utf-8")
    return re.search(rf"^{re.escape(BENCH_SECTION)}$", text, re.MULTILINE) is not None


def bench_section() -> str:
    """The launch-time section, or "" when the heading is gone.

    Tolerant of a missing heading, and only because this runs while pytest is
    *collecting* the module: an assertion raised here aborts the whole session
    rather than failing one test, so an edit to it would take the unrelated
    suite down with it. The heading's absence is asserted below instead, where
    it reads as the single failure it is.
    """
    return _section(BENCH_DOC, BENCH_SECTION) if has_bench_section() else ""


def documented_commands() -> list:
    """Every command line in the section's fences that runs a bench script.

    Continuation lines are joined first, because a command wrapped over two
    lines for the page is still one command; a transcript's `$ ` prompt is
    dropped, and `#` comments end the line the way a shell would. Prose that
    merely *names* a script is not a command and is not collected -- what is
    being guarded is the invocations a reader copies.
    """
    commands = []
    for block in FENCE.findall(bench_section()):
        for line in re.sub(r"\\\n\s+", " ", block).splitlines():
            stripped = line.strip().removeprefix("$ ")
            if INVOKES_A_SCRIPT.search(stripped):
                commands.append(stripped)
    return commands


def script_named_in(text: str) -> str:
    """The bench script *text* names -- of the two, the one it invokes."""
    match = INVOKES_A_SCRIPT.search(text)
    assert match, f"{text!r} names no bench script, so it should never have been collected"
    return match.group(1)


def script_and_flags(command: str) -> tuple:
    """A documented command split at its script: the script name, then its argv.

    Located by the script rather than by position, so `python3 scripts/x.py ...`
    and `pixi run python scripts/x.py ...` decompose the same way -- the
    interpreter is somebody else's question (above), and this one is only about
    what the script itself is being asked for.
    """
    argv = shlex.split(command, comments=True)
    at = next(i for i, word in enumerate(argv) if INVOKES_A_SCRIPT.search(word))
    return script_named_in(argv[at]), argv[at + 1 :]


def script_module(name: str):
    """A bench script, imported by path -- they are scripts, not a package.

    Both of them keep the command line in a `build_parser` separate from `main`
    for exactly this reason: the documented invocations can then be parsed
    without being run.
    """
    path = SCRIPTS / name
    spec = importlib.util.spec_from_file_location(path.stem, path)
    assert spec is not None and spec.loader is not None, path
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def pixi_tasks() -> dict:
    """The tasks `pixi run` can name in this project."""
    return tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))["tool"]["pixi"]["tasks"]


def a_command(command: str) -> str:
    """A parametrize id short enough to read in a failure line."""
    return command[:60]


DOCUMENTED = documented_commands()

# The shortcuts the prose offers beside the host commands ("`pixi run bench -n 5
# -- ...` in the devcontainer"). Read from the whole section rather than from its
# fences, because these are written inline, in the sentence that recommends them.
DOCUMENTED_SHORTCUTS = sorted(set(re.findall(r"pixi run ([a-z0-9][a-z0-9-]*)", bench_section())))

# One case per long flag the section spells out, for the abbreviation guard
# below: the flag, and the command it was documented in.
DOCUMENTED_FLAGS = [
    (command, word)
    for command in DOCUMENTED
    for word in shlex.split(command, comments=True)
    if word.startswith("--") and len(word) > 2
]


def documented_resets() -> list:
    """The `--before` reset of every documented command that carries one.

    Taken from the parsed arguments rather than from the text: what a reset is
    depends on how the script splits the command line, and reading it any other
    way is a second parser to be wrong in a second way.
    """
    resets = []
    for command in DOCUMENTED:
        name, flags = script_and_flags(command)
        before = getattr(script_module(name).build_parser().parse_args(flags), "before", None)
        if before:
            resets.append(before)
    return resets


@pytest.mark.unit
def test_this_guard_still_has_a_document_to_read():
    """The extraction itself, asserted before anything is parametrized over it.

    Every other test in this file is one case per thing found here, and pytest
    *skips* a parametrization with no cases. So a section that was
    renamed, or whose fences stopped holding invocations, would empty all of
    them at once and leave a run that reads exactly like a pass. This is the
    test that makes the emptying the failure, and it is one test rather than
    three because they all fail for the same reason.
    """
    assert has_bench_section(), (
        f"{DOC} no longer has the heading {BENCH_SECTION!r}; it was renamed or removed, and "
        "every guard in this file now reads an empty document"
    )
    assert {script_named_in(command) for command in DOCUMENTED} == {
        "bench_launch.py",
        "bench_points.py",
    }, (
        f"the section under {BENCH_SECTION!r} no longer documents an invocation of each bench "
        "script; those guards now cover nothing"
    )
    assert DOCUMENTED_SHORTCUTS, (
        "the section no longer offers a `pixi run` shortcut beside its host commands; "
        "that guard now covers nothing"
    )
    assert DOCUMENTED_FLAGS, (
        "the section documents no long flag; the abbreviation guard now covers nothing"
    )


@pytest.mark.unit
@pytest.mark.parametrize("command", DOCUMENTED, ids=a_command)
def test_every_documented_command_names_an_interpreter_this_repo_can_vouch_for(command):
    """The defect that shipped: an interpreter the reader's host may not have.

    A parser never sees this word, and a review reads straight past it, so it is
    asserted by name. `pixi run` counts because the task supplies the
    interpreter itself.
    """
    interpreter = shlex.split(command, comments=True)[0]
    assert interpreter in VOUCHED_INTERPRETERS, (
        f"{DOC} runs a bench script with {interpreter!r}; only {VOUCHED_INTERPRETERS} are "
        "vouched for here -- a bare `python` does not exist on a host whose python is pixi's"
    )


@pytest.mark.unit
@pytest.mark.parametrize("command", DOCUMENTED, ids=a_command)
def test_every_documented_command_names_a_script_that_is_in_the_tree(command):
    """The path is part of the command: a moved script is a broken recipe."""
    name = script_named_in(command)
    assert (SCRIPTS / name).is_file(), f"{DOC} documents scripts/{name}, which is not in the tree"


@pytest.mark.unit
@pytest.mark.parametrize("command", DOCUMENTED, ids=a_command)
def test_every_documented_command_asks_its_script_for_flags_the_script_has(command):
    """The epilog guard's seam, pointed at the documented commands.

    Handed to the real parser, so a flag renamed, spelled wrong, or documented
    before it existed fails here instead of in the reader's terminal. Parsing is
    the whole assertion: `parse_args` exits non-zero on anything it does not
    accept, which is the failure being kept out of the page.
    """
    name, flags = script_and_flags(command)
    script_module(name).build_parser().parse_args(flags)


@pytest.mark.unit
@pytest.mark.parametrize("task", DOCUMENTED_SHORTCUTS)
def test_every_documented_shortcut_names_a_task_this_project_defines(task):
    """The other half of a documented command: the route the prose recommends.

    A task renamed in the manifest leaves the sentence recommending it
    confidently wrong, and nothing else in the tree notices -- the same class of
    defect as the interpreter, one line further down the page.
    """
    assert task in pixi_tasks(), (
        f"{DOC} tells a reader to run `pixi run {task}`, which this project does not define"
    )


@pytest.mark.unit
@pytest.mark.parametrize(
    "command,flag", DOCUMENTED_FLAGS, ids=[flag for _, flag in DOCUMENTED_FLAGS]
)
def test_no_documented_flag_would_be_accepted_one_letter_short(command, flag):
    """What a guard that parses documentation is worth: exactly as much as the
    parser behind it refuses.

    An argparse parser accepts any unambiguous *prefix* of a long flag by
    default. So a script that renames `--record` to `--record-path` leaves every
    document saying `--record` -- and every guard that hands those documents to
    the parser stays green, because `--record` is now a prefix of the new name.
    The rename ships and the page is wrong about the one thing this file exists
    to check.

    Asserted by taking a flag the section really documents, cutting its last
    letter off, and requiring the refusal: a parser that accepts *that* is a
    parser these guards cannot be trusted with. Both bench scripts are covered,
    since both have a document pointed at them.
    """
    name, flags = script_and_flags(command)
    abbreviated = [flag[:-1] if word == flag else word for word in flags]
    with pytest.raises(SystemExit):
        script_module(name).build_parser().parse_args(abbreviated)


@pytest.mark.unit
def test_the_cold_recipes_reset_is_either_run_from_the_absent_state_or_left_to_the_script(
    devpod_shim,
):
    """The defect that shipped twice, guarded on this document too -- and, while
    this document shows no reset, guarded on the pointer that stands in for one.

    A cold median needs a per-run reset, and the section currently does not
    spell one out: it sends the reader to the script's own help for `--before`.
    Both of those are worth keeping true, and exactly one of them is true at a
    time, so the document decides which arm runs rather than this test. Publish
    a reset here and it is run against a devpod with nothing to delete, the way
    the epilog's is; publish neither the reset nor the pointer and the section
    has quietly stopped telling a reader how to bench a cold launch at all.
    """
    resets = documented_resets()
    if not resets:
        section = " ".join(bench_section().split())
        assert "`bench_launch.py --help`" in section and "`--before`" in section, (
            "the section documents no `--before` reset and no longer points at the script's "
            "own help for one; a reader has no way left to reach the cold recipe"
        )
        return
    for reset in resets:
        assert a_reset_from_the_absent_state(devpod_shim, reset) == 0, (
            f"{DOC} documents the reset {reset!r}, which dl refuses from the state every "
            "cold bench's first reset meets"
        )


# ---------------------------------------------------------------------------
# the other document that publishes a reset: the bench script's own --help
# ---------------------------------------------------------------------------


def test_the_epilogs_cold_reset_is_a_command_dl_actually_accepts(devpod_shim):
    """The cold recipe in `bench_launch.py --help` names a reset dl accepts.

    This exact defect shipped twice: a documented recipe whose reset was a subcommand
    dl does not have, then the same recipe moved into `--help` unfixed. So the
    documented reset is exercised rather than read -- extracted from the epilog and
    run against a devpod that has nothing to delete, the state every cold bench's
    first reset meets.

    Lived in `test_timing.py` until the Python implementation was retired (#267),
    and moved here rather than being dropped: this file already guards the document's
    copies of the same commands, and while the document currently points at this
    epilog instead of publishing a reset of its own, the epilog's is the only one
    of the two being run.
    """
    epilog = (SCRIPTS / "bench_launch.py").read_text(encoding="utf-8")
    recipe = re.search(r"--before '([^']+)'", epilog)
    assert recipe, "the cold recipe documents no --before reset"
    reset = recipe.group(1)
    assert a_reset_from_the_absent_state(devpod_shim, reset) == 0, (
        f"bench_launch.py's --help documents the reset {reset!r}, which dl refuses "
        "from the state every cold bench's first reset meets"
    )
