"""Tests for the aid entry point.

The point of aid is that it has no workspace machinery of its own, so most of
these tests are about the dl command line it builds and about the fact that dl
is the thing that runs it.
"""

import ast
import pathlib
from unittest.mock import patch

import pytest

from devlaunch import aid, dl

pytestmark = pytest.mark.unit


class TestParseAidArgs:
    """Splitting an aid command line into agent, dl options, spec and prompt."""

    def test_spec_only(self):
        parsed = aid.parse_aid_args(["owner/repo"], env={})
        assert parsed.spec == "owner/repo"
        assert parsed.prompt == ""
        assert parsed.dl_options == []
        assert parsed.agent == aid.DEFAULT_AGENT

    def test_prompt_words_are_joined(self):
        parsed = aid.parse_aid_args(["owner/repo", "fix", "the", "bug"], env={})
        assert parsed.spec == "owner/repo"
        assert parsed.prompt == "fix the bug"

    def test_quoted_prompt_arrives_as_one_word(self):
        parsed = aid.parse_aid_args(["owner/repo", "fix the bug"], env={})
        assert parsed.prompt == "fix the bug"

    def test_agent_flag(self):
        parsed = aid.parse_aid_args(["--gemini", "owner/repo", "hi"], env={})
        assert parsed.agent == "gemini"
        assert parsed.spec == "owner/repo"
        assert parsed.prompt == "hi"

    def test_agent_flag_beats_env_default(self):
        parsed = aid.parse_aid_args(["--codex", "owner/repo"], env={aid.AGENT_ENV_VAR: "gemini"})
        assert parsed.agent == "codex"

    def test_env_sets_default_agent(self):
        parsed = aid.parse_aid_args(["owner/repo"], env={aid.AGENT_ENV_VAR: "gemini"})
        assert parsed.agent == "gemini"

    def test_unknown_env_agent_is_an_error(self):
        with pytest.raises(aid.UsageError):
            aid.parse_aid_args(["owner/repo"], env={aid.AGENT_ENV_VAR: "nope"})

    def test_dl_option_with_value_is_passed_through(self):
        parsed = aid.parse_aid_args(["--devcontainer", "robot", "owner/repo", "hi"], env={})
        assert parsed.dl_options == ["--devcontainer", "robot"]
        assert parsed.spec == "owner/repo"
        assert parsed.prompt == "hi"

    def test_dl_option_value_is_not_mistaken_for_the_spec(self):
        # 'robot' is --devcontainer's value, so the spec is what follows it.
        parsed = aid.parse_aid_args(["--devcontainer", "robot", "owner/repo"], env={})
        assert parsed.spec == "owner/repo"

    def test_unknown_leading_flag_goes_to_dl(self):
        # aid does not need to know every dl flag to stay out of its way.
        parsed = aid.parse_aid_args(["--shared", "owner/repo"], env={})
        assert parsed.dl_options == ["--shared"]
        assert parsed.spec == "owner/repo"

    def test_flags_after_the_spec_belong_to_the_prompt(self):
        parsed = aid.parse_aid_args(["owner/repo", "explain", "--verbose", "mode"], env={})
        assert parsed.dl_options == []
        assert parsed.prompt == "explain --verbose mode"

    def test_missing_spec_is_an_error(self):
        with pytest.raises(aid.UsageError):
            aid.parse_aid_args(["--claude"], env={})


class TestBuildAgentCommand:
    """The shell command handed to dl's `-- <command>` form."""

    def test_claude_with_prompt(self):
        assert (
            aid.build_agent_command("claude", "fix the bug")
            == "claude --dangerously-skip-permissions 'fix the bug'"
        )

    def test_claude_without_prompt(self):
        assert aid.build_agent_command("claude") == "claude --dangerously-skip-permissions"

    def test_gemini_needs_its_interactive_flag_only_with_a_prompt(self):
        assert aid.build_agent_command("gemini", "hi") == "gemini --prompt-interactive hi"
        assert aid.build_agent_command("gemini") == "gemini"

    def test_prompt_with_quotes_is_escaped(self):
        command = aid.build_agent_command("claude", 'don\'t break "this"')
        assert command == "claude --dangerously-skip-permissions 'don'\"'\"'t break \"this\"'"

    def test_prompt_cannot_smuggle_a_second_command(self):
        command = aid.build_agent_command("claude", "hi; rm -rf /")
        assert command == "claude --dangerously-skip-permissions 'hi; rm -rf /'"

    def test_unknown_agent_is_an_error(self):
        with pytest.raises(aid.UsageError):
            aid.build_agent_command("clippy", "hi")


class TestBuildDlArgs:
    """The dl command line aid produces."""

    def test_shape_is_options_spec_dashdash_command(self):
        parsed = aid.parse_aid_args(["owner/repo@branch", "fix", "it"], env={})
        assert aid.build_dl_args(parsed) == [
            "owner/repo@branch",
            "--",
            "claude --dangerously-skip-permissions 'fix it'",
        ]

    def test_dl_options_come_first(self):
        parsed = aid.parse_aid_args(["--devcontainer", "robot", "owner/repo"], env={})
        assert aid.build_dl_args(parsed) == [
            "--devcontainer",
            "robot",
            "owner/repo",
            "--",
            "claude --dangerously-skip-permissions",
        ]

    def test_dl_reads_the_command_back_whole(self):
        """The prompt survives dl's own parsing of `-- <command>`.

        dl joins everything after `--` with spaces, so the quoting aid applies
        has to live inside a single argument, not be spread across several.
        """
        parsed = aid.parse_aid_args(["owner/repo", "fix", "the", "flaky", "test"], env={})
        dl_args = aid.build_dl_args(parsed)
        remaining, _ = dl.extract_devcontainer_flag(dl_args)
        assert (
            " ".join(remaining[2:]) == "claude --dangerously-skip-permissions 'fix the flaky test'"
        )


class TestMainDelegatesToDl:
    """aid must reach a workspace only through dl."""

    def test_main_calls_dl_main_with_the_built_args(self):
        with patch.object(aid.dl, "main", return_value=0) as mock_main:
            assert aid.main(["owner/repo", "fix", "it"]) == 0
        mock_main.assert_called_once_with(
            ["owner/repo", "--", "claude --dangerously-skip-permissions 'fix it'"]
        )

    def test_main_returns_dls_exit_code(self):
        with patch.object(aid.dl, "main", return_value=127):
            assert aid.main(["owner/repo"]) == 127

    def test_help_does_not_touch_dl(self, capsys):
        with patch.object(aid.dl, "main") as mock_main:
            assert aid.main(["--help"]) == 0
        mock_main.assert_not_called()
        assert "aid" in capsys.readouterr().out

    def test_no_args_prints_help_and_fails(self, capsys):
        with patch.object(aid.dl, "main") as mock_main:
            assert aid.main([]) == 1
        mock_main.assert_not_called()
        assert "Usage:" in capsys.readouterr().out

    def test_version_matches_dl(self, capsys):
        assert aid.main(["--version"]) == 0
        assert capsys.readouterr().out.strip() == f"aid {dl.get_version()}"

    def test_usage_error_does_not_reach_dl(self):
        with patch.object(aid.dl, "main") as mock_main:
            assert aid.main(["--gemini"]) == 1
        mock_main.assert_not_called()

    def test_aid_touches_nothing_of_dl_but_its_entry_point(self):
        """The regression this module exists to prevent.

        An aid that drives containers itself is an aid that can build one dl
        would have reused — the whole reason the previous aid diverged. Every
        container decision has to stay dl's, so aid is allowed to reach for
        dl's command line and nothing below it.
        """
        tree = ast.parse(pathlib.Path(aid.__file__).read_text(encoding="utf-8"))
        used = {
            node.attr
            for node in ast.walk(tree)
            if isinstance(node, ast.Attribute)
            and isinstance(node.value, ast.Name)
            and node.value.id == "dl"
        }
        assert used <= {"main", "get_version", "DL_VALUE_OPTIONS"}, (
            f"aid.py reaches past dl's entry point: {sorted(used)}"
        )

    def test_aid_imports_no_process_machinery(self):
        tree = ast.parse(pathlib.Path(aid.__file__).read_text(encoding="utf-8"))
        imported = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                imported.update(alias.name for alias in node.names)
            elif isinstance(node, ast.ImportFrom) and node.module:
                imported.add(node.module)
        assert "subprocess" not in imported


class TestDlIsUnchangedByAid:
    """dl's own entry point still behaves as it did before aid existed."""

    def test_main_defaults_to_sys_argv(self, capsys):
        with patch.object(dl, "update_cache_background"):
            with patch.object(dl.sys, "argv", ["dl", "--version"]):
                assert dl.main() == 0
        assert capsys.readouterr().out.startswith("dl ")

    def test_main_accepts_an_explicit_argv(self, capsys):
        with patch.object(dl, "update_cache_background"):
            assert dl.main(["--version"]) == 0
        assert capsys.readouterr().out.startswith("dl ")
