"""The contract `docs/agents-using-dl.md` publishes, and the `--help` line that sends
a reader to it.

The page states four properties of `dl <ws> -- <command>` that a caller writes code
against: the exit status is the command's, stdout is the command's, stdin reaches it,
and none of that needs a terminal. All four held before the page existed, which is
exactly why they needed writing down and then guarding. A promise nothing states is a
promise a refactor cannot see it is breaking, and the caller who finds out is a script
somewhere else that now reads `dl`'s progress chatter as its JSON.

The behaviour itself is pinned in `test/e2e/test_agent_subprocess_contract.py`, which
needs a Docker daemon and is skipped by default. What is guarded *here* is everything
that can rot without one:

- the page still makes the claims the e2e test is the evidence for, so deleting a
  paragraph cannot quietly leave a test proving something nobody promises;
- every `dl` flag the page writes on a copyable line is a flag `dl` accepts, the same
  rule `test_readme_cli_doc.py` applies to the README, and reusing its extraction
  rather than re-implementing it is deliberate -- two parsers of the same markdown
  drift, and the one in the file with fewer eyes on it drifts first;
- the pointer in `dl --help` still names a page that is in the tree.

That last one is the reason the pointer is a URL. An agent handed the binary out of a
pixi environment has `--help` and no repository, so a relative path would name nothing
for the reader it is addressed to. The cost of a URL is that it can outlive the file it
names with nothing failing, and `the_help_pointer_names_a_page_that_exists` is what
pays it.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

from fixtures.e2e_helpers import dl_command
from fixtures.markdown_sections import section

# The extraction that already knows how this repository writes a `dl` line: which
# code spans are commands, where somebody else's argv begins, how a `$` prompt is
# stripped. Imported rather than copied, so the two pages are judged by one reading
# of the same convention.
from test_readme_cli_doc import LONG_FLAG, MUTUALLY_EXCLUSIVE_PROBE, dl_command_lines

REPO_ROOT = Path(__file__).resolve().parent.parent
PAGE = REPO_ROOT / "docs" / "agents-using-dl.md"
CLI_RS = REPO_ROOT / "rust" / "dl" / "src" / "cli.rs"

# The URL the help block prints, and the path inside it that has to resolve. Spelled
# once, here, so the assertion that the page exists and the assertion that `--help`
# still says so cannot disagree about which page is meant.
DOC_URL = "https://github.com/blooop/devlaunch/blob/main/docs/agents-using-dl.md"
DOC_PATH_IN_URL = "docs/agents-using-dl.md"

# The heading the four promises live under. Matched on the heading rather than on a
# sentence, so the prose under it stays free to be rewritten.
CONTRACT_HEADING = "## The subprocess contract"

# One phrase per promise: the words the page would have to lose for the claim to be
# gone rather than merely reworded. Short on purpose -- a long quotation here turns
# every edit to the page into a test failure, which is how a guard gets deleted.
PROMISES = {
    "exit status": "exit status is the command's",
    "stdout": "stdout is the command's, verbatim",
    "stdin": "stdin is the command's",
    "no terminal": "No terminal is required",
}


def _page() -> str:
    return PAGE.read_text(encoding="utf-8")


@pytest.mark.unit
def test_the_page_a_reader_is_sent_to_is_there():
    """The premise every assertion below rests on.

    A missing page would leave the parametrized promise check collecting four
    failures with the same unhelpful cause, and the flag check collecting nothing
    at all, which reads like a clean run.
    """
    assert PAGE.is_file(), (
        f"{PAGE.relative_to(REPO_ROOT)} is gone; `dl --help` and the README's Docs "
        "table both send a reader to it"
    )


@pytest.mark.unit
@pytest.mark.parametrize("promise", sorted(PROMISES), ids=sorted(PROMISES))
def test_the_page_still_promises_what_the_e2e_test_proves(promise):
    """Each half of the pairing, asked of the page.

    The e2e test is the evidence and this is the claim, and a claim that quietly
    disappears leaves evidence for nothing. Scoped to the contract section so a
    sentence elsewhere on the page that happens to use the same words cannot stand
    in for the promise.
    """
    contract = section(PAGE, CONTRACT_HEADING)
    assert PROMISES[promise] in contract, (
        f"{PAGE.relative_to(REPO_ROOT)} no longer states the {promise!r} promise "
        f"under {CONTRACT_HEADING!r}. test/e2e/test_agent_subprocess_contract.py "
        "still tests it, so either the page lost a claim or the pair has come apart"
    )


@pytest.mark.unit
def test_the_help_pointer_names_a_page_that_exists():
    """A URL cannot 404 in a test suite, so the path inside it is resolved instead.

    This is the whole cost of pointing at a page by URL rather than by relative
    path, and it is worth paying: the reader the block is addressed to may hold no
    repository at all. `test_docs_links.py` resolves the repository's relative links
    and deliberately skips `http(s)://` ones, so without this the help text is the
    one citation in the tree that nothing checks.
    """
    named = REPO_ROOT / DOC_PATH_IN_URL
    assert DOC_PATH_IN_URL in DOC_URL, "DOC_PATH_IN_URL is not the path DOC_URL names"
    assert named.is_file(), (
        f"`dl --help` sends a reader to {DOC_URL}, and {DOC_PATH_IN_URL} is not in "
        "the tree; the page moved and the help block did not follow"
    )


@pytest.mark.unit
def test_the_help_block_is_in_the_source_that_renders_it():
    """Read off `cli.rs` rather than off a run, so this half needs no binary.

    The pairing is the same one `test_agents_doc.py` makes about the `dev-build`
    marker: assert the fact at the place that produces it *and* at the place that
    promises it, because a change to either alone is the failure worth catching.
    The `--help` run below is the other end.
    """
    source = CLI_RS.read_text(encoding="utf-8")
    assert DOC_URL in source, (
        f"{CLI_RS.relative_to(REPO_ROOT)} no longer carries {DOC_URL}; the after-help "
        "block stopped pointing at the page"
    )
    assert "Scripting dl" in source, (
        f"{CLI_RS.relative_to(REPO_ROOT)} no longer carries the scripting heading; "
        "a caller reading `dl --help` has nothing telling them the contract exists"
    )


@pytest.mark.integration
def test_dl_help_sends_a_caller_to_the_page():
    """The other end: what the binary actually renders.

    `after_help` is a clap field, and a field can be dropped from the `#[command]`
    attribute without the const it named becoming dead code anywhere the compiler
    would mention. So the const being right is not evidence that `--help` prints it.
    """
    rendered = subprocess.run(
        dl_command() + ["--help"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert rendered.returncode == 0, (
        f"`dl --help` exited {rendered.returncode}\nstderr: {rendered.stderr}"
    )
    assert DOC_URL in rendered.stdout, (
        "`dl --help` does not point at the agent contract page. An agent holding the "
        "binary and no repository has this text and nothing else:\n" + rendered.stdout
    )


def documented_flags() -> dict[str, str]:
    """Every long flag the page writes on a `dl` line, and one line writing it.

    The line travels with the flag so a failure can say where to look without this
    file storing a line number that the next inserted paragraph would invalidate.
    """
    found: dict[str, str] = {}
    for line in dl_command_lines(_page()):
        for flag in LONG_FLAG.findall(line):
            found.setdefault(flag, line)
    return found


DOCUMENTED_FLAGS = documented_flags()


@pytest.mark.unit
def test_the_page_still_writes_dl_lines_a_reader_could_copy():
    """The floor under the parametrized check below.

    Deliberately low. This is a smoke test for the extraction, not an opinion about
    how many flags the page ought to show: a page rewritten until it demonstrates
    none would otherwise leave the check collecting nothing and passing.
    """
    assert DOCUMENTED_FLAGS, (
        "no `dl` flags were extracted from the agent page; either it stopped showing "
        "commands or it writes them in a shape `dl_command_lines` does not recognise"
    )


@pytest.mark.integration
@pytest.mark.parametrize("flag", sorted(DOCUMENTED_FLAGS))
def test_every_flag_the_page_hands_an_agent_is_a_flag_dl_accepts(flag):
    """A flag on a line a caller can paste must not exit 2 on contact.

    The probe is `test_readme_cli_doc.py`'s: two real flags that cannot be combined,
    so the parser refuses the line before anything launches, stops, deletes or lists,
    and the only thing read off the refusal is which one it was.
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
        f"docs/agents-using-dl.md writes `{DOCUMENTED_FLAGS[flag]}`, but dl has no "
        f"{flag}: {first_line}"
    )
