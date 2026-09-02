"""What the two binaries tell a session manager, held against each other.

`aid` names the agent it picked and `dl` names one whose command is an agent by
name, and the two are separate binaries: core's constants are `pub(crate)` and aid
cannot reach them, so the variable's name is spelled twice and the agent list
exists twice over. Both are the same fact written more than once, which the
standing rule in CLAUDE.md allows only with a test beside it diffing the copies.
This is that test.

The literals here are the third copy, deliberately, for
`test/unit/test_rust_source.py`'s reason: a reader compared only against another
reader passes when both stop finding anything.
"""

import re
from pathlib import Path

import pytest

from fixtures import rust_source

# What herdr reads, and what both binaries therefore write.
AGENT_VAR = "HERDR_AGENT"

# The agents devlaunch knows, in the spelling a session manager uses.
AGENTS = ("claude", "codex", "gemini")


class TestTheVariableBothBinariesWrite:
    def test_core_writes_the_variable_herdr_reads(self):
        assert rust_source.core_session_manager_agent_var() == AGENT_VAR

    def test_aid_writes_the_variable_herdr_reads(self):
        assert rust_source.aid_session_manager_agent_var() == AGENT_VAR

    def test_the_two_binaries_write_one_variable(self):
        assert rust_source.aid_session_manager_agent_var() == (
            rust_source.core_session_manager_agent_var()
        ), (
            "aid and core name different variables, so one of the two launchers is "
            "reporting to nothing"
        )


class TestTheAgentsBothTablesKnow:
    def test_core_names_the_agents_it_claims_rules_exist_for(self):
        assert rust_source.core_agent_names() == AGENTS

    def test_aids_table_holds_the_same_agents(self):
        assert rust_source.aid_agent_names() == AGENTS

    def test_neither_launcher_knows_an_agent_the_other_does_not(self):
        core = set(rust_source.core_agent_names())
        aid = set(rust_source.aid_agent_names())
        assert aid - core == set(), (
            f"aid can start {sorted(aid - core)}, which dl would not name: "
            "`dl <ws> -- <it>` leaves that agent invisible to a session manager"
        )
        assert core - aid == set(), (
            f"dl would name {sorted(core - aid)}, which aid cannot start: a name for "
            "a manager to match detection rules against nothing"
        )


@pytest.mark.parametrize("name", AGENTS)
def test_an_agent_name_is_a_bare_program_name(name):
    """A path or an argument in the list would never match what dl reads.

    `herdr::agent_in` compares the command's program by its last path component, so
    a list entry carrying a slash or a space could not be matched by any command.
    """
    assert "/" not in name and " " not in name, f"{name!r} could never match a program"


# What herdr exports into every pane it spawns, and what the pane shell therefore
# reads. Third copy of the same words, for the module docstring's reason.
TAB_VAR = "HERDR_TAB_ID"

# The field in herdr's config that names the program a new pane runs.
DEFAULT_SHELL_FIELD = "default_shell"

# The name `dl --install` links beside `dl`, because `default_shell` takes an
# executable and cannot hold `dl --herdr-shell`.
PANE_SHELL_NAME = "dl-herdr-shell"

WORKSPACE_TOOLS = (
    Path(__file__).resolve().parent.parent.parent / "docs" / "workspace-tools.md"
)


def _workspace_tools() -> str:
    assert WORKSPACE_TOOLS.is_file(), (
        f"{WORKSPACE_TOOLS} is gone. The pane shell's rationale moved; move this "
        "guard's path with it, in the same change."
    )
    return WORKSPACE_TOOLS.read_text(encoding="utf-8")


class TestTheWordsThatBelongToHerdr:
    """Every literal below is another program's, so a rename is a change to this
    feature rather than a refactor. Pinned against the source that runs them and
    the document that explains them, both.
    """

    def test_the_pane_shell_reads_the_tab_herdr_exports(self):
        assert rust_source.core_session_manager_tab_var() == TAB_VAR

    def test_the_two_questions_are_herdrs_own_subcommands(self):
        assert rust_source.core_pane_questions() == (
            ("pane", "list"),
            ("pane", "process-info", "--pane"),
        )

    def test_the_fallback_binary_is_herdrs_own_name(self):
        assert rust_source.core_herdr_program() == "herdr"

    def test_the_installed_name_is_the_one_the_docs_send_people_to(self):
        assert rust_source.dl_pane_shell_name() == PANE_SHELL_NAME
        assert PANE_SHELL_NAME in _workspace_tools()

    def test_the_docs_show_the_config_stanza_a_reader_pastes(self):
        """The stanza and not the word.

        A guard that only asked whether `default_shell` appears anywhere passes a
        document that has lost the snippet entirely, because the surrounding prose
        names the field four more times. So this matches the two lines a reader
        copies, in order, with the installed name inside the value -- which is what
        breaks if the snippet is edited away or drifts from the name dl links.
        """
        doc = _workspace_tools()
        stanza = re.search(
            rf'^\[terminal\]\n{re.escape(DEFAULT_SHELL_FIELD)} = "([^"]*)"$',
            doc,
            re.MULTILINE,
        )
        assert stanza, (
            f"docs/workspace-tools.md shows no `[terminal]` / {DEFAULT_SHELL_FIELD} "
            "stanza. That snippet is the whole of what a reader has to do, and the "
            "prose naming the field is not a substitute for it."
        )
        assert stanza.group(1).endswith(rust_source.dl_pane_shell_name()), (
            f"the documented stanza points at {stanza.group(1)!r}, which does not "
            f"end in the name dl --install links ({rust_source.dl_pane_shell_name()})"
        )

    def test_the_docs_name_the_variable_the_pane_shell_reads(self):
        assert TAB_VAR in _workspace_tools()

    def test_the_docs_say_the_reading_is_live_rather_than_remembered(self):
        """The property the whole design turns on, and the one a reader would
        otherwise have to infer from the absence of a cache. Matched as its own
        heading, so a section retitled to promise something else fails here.
        """
        doc = _workspace_tools()
        assert re.search(r"^### Nothing is remembered\b", doc, re.MULTILINE), (
            "the section arguing why the tab is read live rather than remembered is "
            "gone or retitled. It is the load-bearing half of the design."
        )
