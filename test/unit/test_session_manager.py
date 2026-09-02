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
