import subprocess
import tempfile
import os
import pathlib
import shlex
import pytest
from unittest.mock import patch

from fixtures.e2e_helpers import dl_command


class TestBashCompletion:
    """Test bash completion functionality."""

    # Declare class attributes for type checking
    test_dir: str
    completion_script: pathlib.Path
    cache_base: pathlib.Path
    cache_dir: pathlib.Path
    cache_file: pathlib.Path

    def setup_method(self):
        """Set up test environment."""
        self.test_dir = tempfile.mkdtemp()
        # The script both binaries embed with `include_str!`, and now the only
        # copy: it lived under `devlaunch/completions/` as well until the Python
        # implementation was retired (#267), and a Rust test asserted the two were
        # byte-identical. One copy needs no such test.
        self.completion_script = (
            pathlib.Path(__file__).parent.parent
            / "rust"
            / "devlaunch-core"
            / "completions"
            / "dl.bash"
        )

        # Create a test cache file with sample data
        # The cache structure should be XDG_CACHE_HOME/devlaunch/completions.bash
        self.cache_base = pathlib.Path(self.test_dir) / "cache"
        self.cache_dir = self.cache_base / "devlaunch"
        self.cache_dir.mkdir(parents=True)
        self.cache_file = self.cache_dir / "completions.bash"

        # Write test completion data
        with open(self.cache_file, "w", encoding="utf-8") as f:
            f.write('DL_WORKSPACES="my-workspace another-ws test-project"\n')
            f.write('DL_REPOS="my-org/my-repo another-org/another-repo github-org/test-repo"\n')
            f.write('DL_OWNERS="my-org another-org github-org"\n')
            f.write(
                'DL_BRANCHES="my-org/my-repo@main my-org/my-repo@feature-branch another-org/another-repo@develop"\n'
            )

    def test_herdr_environment_completes_actions_without_workspace_specs(self):
        assert set(self.run_completion("dl --herdr-env ")) == {
            "set", "unset", "profile", "show", "clear"
        }
        assert self.run_completion("dl --herdr-workspace ") == []
        assert self.run_completion("dl --herdr-env show ") == []

    def teardown_method(self):
        """Clean up test environment."""
        import shutil

        if self.test_dir and os.path.exists(self.test_dir):
            shutil.rmtree(self.test_dir)

    def run_completion(self, comp_line, comp_point=None, env=None):
        """
        Run bash completion for the given line and cursor position.

        Args:
            comp_line: The command line string
            comp_point: The cursor position (defaults to end of line)

        Returns:
            List of completion suggestions
        """
        return self.run_completion_with_options(comp_line, comp_point, env)[0]

    def run_completion_with_options(self, comp_line, comp_point=None, env=None):
        """
        As `run_completion`, and also the options the function asked bash for.

        `compopt` is how the script says whether a candidate is a whole word or a
        prefix to keep typing, and it is a real difference to the user: a trailing
        space after a workspace id, none after an `owner/`. It cannot be observed
        from COMPREPLY, and the real builtin refuses to run at all outside a live
        completion ("not currently executing completion function"), which is also
        what puts an error on stderr every time this harness runs. Shadowing it
        with a function records the request and silences that. A shell function
        beats a builtin of the same name, so the script needs no seam of its own.

        Returns:
            (completions, options) -- options is the list of words passed to each
            `compopt` call, in order.
        """
        if comp_point is None:
            comp_point = len(comp_line)

        options_file = pathlib.Path(self.test_dir) / "compopt.log"
        options_file.unlink(missing_ok=True)

        # Create a bash script that sources the completion and runs it
        # Use shlex.quote to properly escape shell arguments
        # Exported inside the script rather than passed to `subprocess`, so it is
        # visible in the same place as XDG_CACHE_HOME when a failure prints the script.
        exports = "\n".join(
            f"export {name}={shlex.quote(str(value))}" for name, value in (env or {}).items()
        )
        script = f"""
#!/bin/bash
export XDG_CACHE_HOME={shlex.quote(str(self.cache_base))}
{exports}
source {shlex.quote(str(self.completion_script))}

compopt() {{ printf '%s\\n' "$*" >> {shlex.quote(str(options_file))}; }}

# Set completion environment variables
export COMP_LINE={shlex.quote(comp_line)}
export COMP_POINT={comp_point}

# Call the completion function
_dl_completion

# Output completions
for reply in "${{COMPREPLY[@]}}"; do
    echo "$reply"
done
"""

        # Run the script
        result = subprocess.run(["bash", "-c", script], capture_output=True, text=True, check=False)

        # Parse output
        completions = [line.strip() for line in result.stdout.strip().split("\n") if line.strip()]
        recorded = options_file.read_text(encoding="utf-8") if options_file.exists() else ""
        options = [line.strip() for line in recorded.strip().split("\n") if line.strip()]
        return completions, options

    # ------------------------------------------------------------------
    # --claude-profile: the completion and the resolver have to agree
    # ------------------------------------------------------------------

    # Directory names a profile root can hold, and whether `ProfileName::parse`
    # accepts each. The invalid ones are not hypothetical: a profile directory is
    # made by hand, and `-` and a space are what a hand produces.
    PROFILE_FIXTURES = {
        "work": True,
        "personal.2": True,
        "with_underscore": True,
        "with-dash": True,
        "-flag": False,
        "has space": False,
        "tab\there": False,
        "unicode-é": False,
        ".hidden": False,
    }

    def _a_profile_root(self):
        """A profiles directory holding every name in PROFILE_FIXTURES."""
        root = pathlib.Path(self.test_dir) / "claude-profiles"
        for name in self.PROFILE_FIXTURES:
            (root / name).mkdir(parents=True)
        return root

    def test_the_completion_offers_only_names_a_launch_accepts(self):
        """The completion's grammar, diffed against the binary's own listing.

        `dl.bash` restates `ProfileName::parse`, which the repo's standing rule
        allows only with a test beside it that diffs the copy against the first.
        This is that diff, and it is a real one rather than a restatement: the
        expectation comes from running `dl --claude-profiles`, so the two
        implementations have to agree about the same directory rather than both
        having to agree with a list written here.

        Sourcery caught the original, which offered every directory. Tab-completing
        `-flag` and then being refused for a name you never typed is worse than no
        completion at all.
        """
        root = self._a_profile_root()
        env = {"DEVLAUNCH_CLAUDE_PROFILES_DIR": str(root)}

        offered = set(self.run_completion("dl --claude-profile ", env=env))

        listed = subprocess.run(
            dl_command() + ["--claude-profiles"],
            capture_output=True,
            text=True,
            check=True,
            env={**os.environ, **env},
        ).stdout
        # The NAME column, minus the header.
        named = {
            line.split()[0]
            for line in listed.splitlines()[1:]
            if line.strip() and not line.startswith(" ")
        }

        assert offered == named, (
            f"the completion offers {sorted(offered)} and `dl --claude-profiles` "
            f"lists {sorted(named)}; a name in one and not the other is either a "
            "completion the launch refuses or a profile you cannot tab to"
        )

    def test_the_completion_offers_the_valid_names_and_default(self):
        """The absolute half of the diff above.

        Two implementations agreeing on nothing would satisfy the comparison, so
        this pins what the answer actually is -- and the `default` row, which is a
        name the resolver answers for with no directory behind it at all.
        """
        root = self._a_profile_root()
        offered = set(
            self.run_completion(
                "dl --claude-profile ", env={"DEVLAUNCH_CLAUDE_PROFILES_DIR": str(root)}
            )
        )

        wanted = {name for name, ok in self.PROFILE_FIXTURES.items() if ok}
        assert offered == wanted | {"default"}, sorted(offered)
        for name, ok in self.PROFILE_FIXTURES.items():
            if not ok:
                assert name not in offered, f"{name!r} is not a name a launch accepts"

    def test_a_partial_profile_name_still_filters(self):
        """The filter must not have replaced compgen's own prefix matching."""
        root = self._a_profile_root()
        offered = set(
            self.run_completion(
                "dl --claude-profile with", env={"DEVLAUNCH_CLAUDE_PROFILES_DIR": str(root)}
            )
        )
        assert offered == {"with_underscore", "with-dash"}, sorted(offered)

    def test_the_default_name_is_offered_with_no_profile_root_at_all(self):
        """A host with no profiles directory still has one name to complete."""
        missing = pathlib.Path(self.test_dir) / "nothing-here"
        offered = self.run_completion(
            "dl --claude-profile ", env={"DEVLAUNCH_CLAUDE_PROFILES_DIR": str(missing)}
        )
        assert offered == ["default"]

    def test_completion_with_dashed_workspace(self):
        """Test completion works with names containing dashes, in both namespaces."""
        # "dl my-" matches an owner, so the ids that also start "my-" are held
        # back; "dl my-w" matches none, so it falls through to them. Both halves
        # are here because a dash treated as a word break would show up in one
        # list or the other, and the tail is what it would leave behind.
        owners = self.run_completion("dl my-")
        ids = self.run_completion("dl my-w")

        assert owners == ["my-org/"]
        assert ids == ["my-workspace"]
        assert "org/" not in owners
        assert "workspace" not in ids

    def test_completion_with_dashed_org_name(self):
        """Test completion works with organization names containing dashes."""
        # Complete after typing "dl my-org/"
        completions = self.run_completion("dl my-org/")
        assert "my-org/my-repo" in completions

    def test_completion_with_dashed_repo_name(self):
        """Test completion works with repository names containing dashes."""
        # Complete after typing "dl my-org/my-"
        completions = self.run_completion("dl my-org/my-")
        assert "my-org/my-repo" in completions

    def test_completion_after_dashed_workspace(self):
        """Test subcommand completion after a workspace with dashes."""
        # Complete after typing "dl my-workspace " (note the trailing space)
        completions = self.run_completion("dl my-workspace ")
        expected = ["up", "stop", "rm", "code", "restart", "recreate", "reset", "--"]
        for cmd in expected:
            assert cmd in completions

    def test_completion_with_branch_containing_dash(self):
        """Test completion with branch names containing dashes."""
        # Complete after typing "dl my-org/my-repo@feature-"
        completions = self.run_completion("dl my-org/my-repo@feature-")
        assert "my-org/my-repo@feature-branch" in completions

    def test_completion_global_flags(self):
        """Test completion of global flags."""
        # Complete after typing "dl --"
        completions = self.run_completion("dl --")
        expected = ["--ls", "--install", "--help", "--version"]
        for flag in expected:
            assert flag in completions

    def test_completion_short_flags(self):
        """Test completion of short flags."""
        # Complete after typing "dl -"
        completions = self.run_completion("dl -")
        assert "-h" in completions

    def test_no_completion_after_global_flag(self):
        """Test no completion after global flag."""
        # Complete after typing "dl --ls " (note the trailing space)
        completions = self.run_completion("dl --ls ")
        # Should not suggest subcommands after a global flag
        assert "stop" not in completions
        assert "rm" not in completions

    def test_aid_completes_the_same_workspaces_as_dl(self):
        """aid's first argument is a dl workspace spec, so it completes alike."""
        assert self.run_completion("aid my-") == self.run_completion("dl my-")

    def test_aid_completes_agent_flags(self):
        """Test completion of aid's own flags."""
        completions = self.run_completion("aid --")
        for flag in ["--claude", "--codex", "--gemini", "--devcontainer"]:
            assert flag in completions
        # dl's workspace-management flags are not aid's business
        assert "--ls" not in completions
        assert "--install" not in completions

    def test_no_subcommand_completion_after_an_aid_workspace(self):
        """Everything after an aid workspace is the prompt, not a subcommand."""
        completions = self.run_completion("aid my-workspace ")
        for cmd in ["up", "stop", "rm", "code", "restart", "recreate", "reset"]:
            assert cmd not in completions

    def test_an_agent_flag_does_not_move_the_workspace_spec(self):
        """`aid --codex <spec>` completes its spec exactly as `aid <spec>` does.

        aid reads its leading flags and calls the first word that is not one the
        workspace spec (`parse_aid_args`), so an agent flag does not occupy the
        spec's position. The script counted words from the command instead and
        offered a spec at word two alone, which made every `aid --codex`,
        `aid --claude` and `aid --gemini` line complete nothing at all.
        """
        # Both halves of what the function answers, because "the same" includes
        # whether the cursor is left against the `/` -- an owner completed with a
        # trailing space is a different gesture from one without, and COMPREPLY
        # cannot show the difference. `run_completion` alone would pass a script
        # that appended a space here and not there.
        for spec in ["my-", "", "my-org/", "my-org/my-repo@", "./"]:
            bare = self.run_completion_with_options(f"aid {spec}")
            assert bare[0], f"`aid {spec}` completes something to begin with"
            for flag in ["--claude", "--codex", "--gemini"]:
                assert self.run_completion_with_options(f"aid {flag} {spec}") == bare, (
                    f"`aid {flag} {spec}` completes differently from `aid {spec}`, "
                    "but the flag is read before the spec and does not stand in its "
                    "place"
                )

    def test_a_value_option_and_its_value_do_not_move_the_workspace_spec(self):
        """`dl --devcontainer robot <spec>` completes its spec too.

        The same defect as the agent flags, two words wide instead of one:
        clap puts options anywhere among the positional words, so the variant
        name is not the workspace and the workspace is not word two.
        """
        bare = self.run_completion("dl my-")
        assert bare
        assert self.run_completion("dl --devcontainer robot my-") == bare
        assert self.run_completion("dl --claude-profile work my-") == bare

    def test_a_leading_modifier_leaves_the_verb_where_it_was(self):
        """`dl --rm <ws> <verb>`: a flag ahead of the spec shifts nothing.

        `--rm` modifies a launch rather than being one, so the spec is still to
        come after it and a verb after that.

        The anchor is not decoration: without it both sides are `[]` when the
        verb branch never fires, and the test passes against a script that
        completes nothing at all. Mutating `position == 1` to `position == 99`
        was green before it was added.
        """
        verbs = self.run_completion("dl my-workspace ")
        assert "stop" in verbs, "the line the flag is added to offers verbs at all"
        assert self.run_completion("dl --rm my-workspace ") == verbs

    def test_a_command_flag_ends_the_line_whatever_follows_it(self):
        """A flag that *is* the command takes neither a workspace nor a verb.

        The distinction the scan turns on, and the half that is easy to lose
        when the other half starts working: `--devcontainer` has not started a
        line and `--ls` has finished one, so only the second stops here.
        """
        for line in ["dl --ls ", "dl --purge ", "dl --prune ", "dl --version ", "dl --ls my-"]:
            assert self.run_completion(line) == [], (
                f"`{line}` completes something, but the flag on it names the whole "
                "command and no workspace follows one"
            )

    def test_only_a_flag_a_spec_can_follow_is_completed_past(self):
        """A workspace is offered after a flag only if the CLI takes one there.

        The ten flags below all refuse a workspace, and each refusal is the
        proof: `dl --repos my-ws` answers "--repos takes no workspace",
        `dl --yes my-ws` answers "--yes means nothing for a workspace command",
        and `dl --json my-ws` is a clap error about the missing `--ls`. Tabbing
        to a name and then being refused for something you never typed is worse
        than no completion at all, which is the same bar
        `test_the_completion_offers_only_names_a_launch_accepts` holds the
        profile names to.

        Three groups, and they are why the scan's default arm has to be "this
        flag ends the line" rather than "a spec is still to come": the hidden
        members of the command group, the five flags that modify a command
        already on the line, and the two retired spellings.
        """
        for flag in [
            "--repos",
            "--completion-data",
            "--update-cache",
            "--json",
            "--size",
            "--yes",
            "--force",
            "--force-worktrees",
            "--stop",
            "--autorm",
        ]:
            assert self.run_completion(f"dl {flag} ") == [], (
                f"`dl {flag} <tab>` offers a workspace, and `dl {flag} <ws>` is a "
                "line the CLI refuses"
            )

    def test_a_spec_still_follows_the_flags_that_modify_a_launch(self):
        """The other side of that rule: the three dl flags a spec does follow.

        Named as the whole set rather than sampled, because the fix turns on
        the default arm being a refusal -- so this is the list of exceptions,
        and a flag missing from it completes nothing.
        """
        for flag in ["--rm", "--devcontainer robot", "--claude-profile work"]:
            assert self.run_completion(f"dl {flag} my-") == ["my-org/"], (
                f"`dl {flag} my-<tab>` offers no spec, but the flag modifies a "
                "launch and a workspace is what comes next"
            )

    def test_an_unknown_leading_flag_completes_nothing_for_either_binary(self):
        """A flag neither grammar knows ends the line rather than shifting it.

        For dl because clap refuses the flag outright, and for aid because it
        cannot know whether an unknown flag takes a value: `parse_aid_args`
        reads one as a boolean dl option, so on
        `aid --unknown-taking-a-value foo owner/repo` it calls `foo` the spec.
        Completing nothing is the honest answer to a line whose slots aid
        itself cannot place.
        """
        for line in ["dl --bogus my-", "aid --bogus my-", "dl --bogus ", "aid --bogus "]:
            assert self.run_completion(line) == []

    def test_nothing_is_completed_past_the_double_dash(self):
        """After `--` the words are the command run inside the workspace."""
        # Anchored the same way: each line without its `--` completes something,
        # so an empty answer is the `--` and not a script that answers nothing.
        for line, before in [
            ("dl my-workspace -- ", "dl my-workspace "),
            ("dl my-workspace -- ls my-", "dl my-workspace "),
            ("dl --rm my-ws -- ", "dl --rm "),
        ]:
            assert self.run_completion(before), f"`{before}` completes something"
            assert self.run_completion(line) == []

    def test_an_aid_prompt_is_never_a_verb_however_the_line_began(self):
        """aid's prompt starts after the spec, leading flags or not."""
        for line in ["aid my-workspace ", "aid --codex my-workspace ", "aid --codex my-ws stop "]:
            for verb in ["up", "stop", "rm", "code", "restart", "recreate", "reset"]:
                assert verb not in self.run_completion(line), (
                    f"`{line}` offers the verb {verb}, but everything after an aid "
                    "workspace is prompt text"
                )

    def test_completion_partial_workspace_match(self):
        """Test partial matching of workspace names."""
        # Complete after typing "dl test"
        completions = self.run_completion("dl test")
        assert "test-project" in completions

    def test_completion_partial_org_match(self):
        """Test partial matching of organization names."""
        # Complete after typing "dl git"
        completions = self.run_completion("dl git")
        assert "github-org/" in completions

    def test_completion_partial_repo_match(self):
        """Test partial matching of repository names."""
        # Complete after typing "dl another-org/ano"
        completions = self.run_completion("dl another-org/ano")
        assert "another-org/another-repo" in completions

    # --- the two namespaces in the first word -------------------------------
    #
    # An owner and the workspace ids of its own repo share a prefix whenever the
    # repo slug is a prefix-neighbour of the owner name, which needs no fork and
    # no second owner: `kinisi-robotics` against ids derived from `kinisi_ros`,
    # whose slug is `kinisi-ros`. Offering both as one list stalled bash at the
    # longest common prefix, `kinisi-ro`. These pin the rule that replaced it.

    def write_colliding_cache(self):
        """The real-world shape: one owner whose name collides with its own ids."""
        with open(self.cache_file, "w", encoding="utf-8") as f:
            f.write(
                'DL_WORKSPACES="kinisi-ros-nb2-lobi kinisi-ros-remove-pins-tiha '
                'kinisi-ros-update-bencher-jegi"\n'
            )
            f.write('DL_REPOS="kinisi-robotics/kinisi_ros"\n')
            f.write('DL_OWNERS="kinisi-robotics"\n')
            f.write('DL_BRANCHES="kinisi-robotics/kinisi_ros@main"\n')

    def test_an_owner_completes_past_the_ids_of_its_own_repo(self):
        """`dl kin<TAB>` reaches the owner instead of stalling on shared prefix."""
        self.write_colliding_cache()

        assert self.run_completion("dl kin") == ["kinisi-robotics/"]

    def test_the_owner_and_then_the_repo_is_two_tabs(self):
        """The second tab continues through the `/` branch to the whole spec."""
        self.write_colliding_cache()

        assert self.run_completion("dl kinisi-robotics/") == ["kinisi-robotics/kinisi_ros"]

    def test_workspace_ids_are_offered_when_no_owner_matches(self):
        """An id half-typed out of `dl --ls` still completes: `dl <id>` is a spec."""
        self.write_colliding_cache()

        # `kinisi-ros-` matches no owner -- `kinisi-robotics` diverges at the `b`.
        assert self.run_completion("dl kinisi-ros-nb") == ["kinisi-ros-nb2-lobi"]

    def test_ids_are_held_back_only_while_an_owner_matches(self):
        """The fallback is per-prefix, not a mode: one keystroke swaps the list."""
        self.write_colliding_cache()

        assert self.run_completion("dl kinisi-ro") == ["kinisi-robotics/"]
        assert self.run_completion("dl kinisi-ros") == [
            "kinisi-ros-nb2-lobi",
            "kinisi-ros-remove-pins-tiha",
            "kinisi-ros-update-bencher-jegi",
        ]

    def test_bare_tab_offers_both_namespaces(self):
        """Nothing typed is no collision, so `dl <TAB>` still shows everything."""
        self.write_colliding_cache()

        completions = self.run_completion("dl ")

        assert "kinisi-robotics/" in completions
        assert "kinisi-ros-nb2-lobi" in completions

    def test_an_id_typed_in_full_is_offered_beside_the_owner_it_shares_a_name_with(self):
        """`DL_WORKSPACES` is every devpod workspace, including hand-made names."""
        with open(self.cache_file, "w", encoding="utf-8") as f:
            f.write('DL_WORKSPACES="blooop other-main-abcd"\n')
            f.write('DL_REPOS="blooop/devlaunch"\n')
            f.write('DL_OWNERS="blooop"\n')
            f.write('DL_BRANCHES="blooop/devlaunch@main"\n')

        # No longer prefix leaves the owner behind, so without this the workspace
        # can never be completed and TAB appends a `/` to an already-whole word.
        assert sorted(self.run_completion("dl blooop")) == ["blooop", "blooop/"]

    def test_an_owner_keeps_the_cursor_against_the_slash_and_an_id_does_not(self):
        """An owner is a prefix to keep typing; an id is a whole word."""
        self.write_colliding_cache()

        _, after_owner = self.run_completion_with_options("dl kin")
        _, after_id = self.run_completion_with_options("dl kinisi-ros-nb")

        assert after_owner == ["-o nospace"]
        assert after_id == []

    def test_ids_complete_when_the_cache_knows_no_owners(self):
        """A cache with workspaces and no repos of dl's own is still completable."""
        with open(self.cache_file, "w", encoding="utf-8") as f:
            f.write('DL_WORKSPACES="handmade-workspace"\n')
            f.write('DL_REPOS=""\n')
            f.write('DL_OWNERS=""\n')
            f.write('DL_BRANCHES=""\n')

        assert self.run_completion("dl hand") == ["handmade-workspace"]

    def test_completion_branch_at_symbol(self):
        """Test completion triggers after @ symbol for branches."""
        # Complete after typing "dl my-org/my-repo@"
        completions = self.run_completion("dl my-org/my-repo@")
        assert "my-org/my-repo@main" in completions
        assert "my-org/my-repo@feature-branch" in completions

    def test_completion_path_with_dot_slash(self):
        """Test path completion with ./"""
        # Create a test directory
        assert self.test_dir is not None
        test_subdir = pathlib.Path(self.test_dir) / "test-dir"
        test_subdir.mkdir()

        # Complete after typing "dl ./"
        with patch.dict(os.environ, {"PWD": self.test_dir}):
            completions = self.run_completion("dl ./")
            # Basic invariants that should hold across environments:
            # - completion runs without error (implicit if we reach here)
            # - we get some completions back
            assert completions is not None

    def test_completion_multiple_dashes_in_name(self):
        """Test completion with names containing multiple dashes."""
        # Add test data with multiple dashes
        assert self.cache_file is not None
        with open(self.cache_file, "w", encoding="utf-8") as f:
            f.write('DL_WORKSPACES="my-test-workspace feature-dev-branch"\n')
            f.write('DL_REPOS="my-test-org/my-test-repo"\n')
            f.write('DL_OWNERS="my-test-org"\n')
            f.write('DL_BRANCHES="my-test-org/my-test-repo@feature-dev-branch"\n')

        # Test workspace with multiple dashes
        completions = self.run_completion("dl my-test-")
        assert "my-test-workspace" in completions or "my-test-org/" in completions

        # Test repo with multiple dashes
        completions = self.run_completion("dl my-test-org/")
        assert "my-test-org/my-test-repo" in completions

    def test_completion_consecutive_dashes(self):
        """Test completion with consecutive dashes (edge case)."""
        # Add test data with consecutive dashes
        assert self.cache_file is not None
        with open(self.cache_file, "w", encoding="utf-8") as f:
            f.write('DL_WORKSPACES="my--workspace"\n')
            f.write('DL_REPOS="org--name/repo--name"\n')
            f.write('DL_OWNERS="org--name"\n')
            f.write('DL_BRANCHES=""\n')

        # Test workspace with consecutive dashes
        completions = self.run_completion("dl my--")
        assert "my--workspace" in completions

    def test_completion_underscore_in_names(self):
        """Test completion with underscores in names."""
        # Add test data with underscores
        assert self.cache_file is not None
        with open(self.cache_file, "w", encoding="utf-8") as f:
            f.write('DL_WORKSPACES="my_workspace test_project_2"\n')
            f.write('DL_REPOS="my_org/my_repo"\n')
            f.write('DL_OWNERS="my_org"\n')
            f.write('DL_BRANCHES="my_org/my_repo@feature_branch"\n')

        # Test workspace with underscores
        completions = self.run_completion("dl my_")
        assert "my_workspace" in completions or "my_org/" in completions

    def test_completion_numeric_in_names(self):
        """Test completion with numeric characters in names."""
        # Add test data with numbers
        assert self.cache_file is not None
        with open(self.cache_file, "w", encoding="utf-8") as f:
            f.write('DL_WORKSPACES="project-123 test-456"\n')
            f.write('DL_REPOS="user123/repo456"\n')
            f.write('DL_OWNERS="user123"\n')
            f.write('DL_BRANCHES="user123/repo456@v1.2.3"\n')

        # Test workspace with numbers
        completions = self.run_completion("dl project-")
        assert "project-123" in completions

        # Test version branch
        completions = self.run_completion("dl user123/repo456@v")
        assert "user123/repo456@v1.2.3" in completions

    def test_word_count_accuracy(self):
        """Test that word counting is accurate with various inputs."""
        # Test empty line - should complete first argument position
        completions = self.run_completion("dl ")
        assert completions is not None

        # Test single word - should complete command name
        completions = self.run_completion("dl")
        assert completions is not None

        # Test two words - should complete workspace
        completions = self.run_completion("dl my-workspace")
        assert "my-workspace" in completions

        # Test three words - should complete subcommands
        completions = self.run_completion("dl my-workspace ")
        assert "stop" in completions

    def test_completion_cursor_position(self):
        """Test completion at different cursor positions."""
        # Cursor in middle of word
        completions = self.run_completion("dl my-work", 7)  # Cursor after "my-"
        # Should still complete words that match the prefix "my-"
        assert "my-workspace" in completions or "my-org/" in completions

    def test_empty_cache_file(self):
        """Test completion with empty cache file."""
        # Create empty cache file
        assert self.cache_file is not None
        with open(self.cache_file, "w", encoding="utf-8") as f:
            f.write("")

        # Should still complete global flags
        completions = self.run_completion("dl --")
        assert "--help" in completions

    def test_missing_cache_file(self):
        """Test completion with missing cache file."""
        # Remove cache file
        assert self.cache_file is not None
        os.unlink(self.cache_file)

        # Should still complete global flags
        completions = self.run_completion("dl --")
        assert "--help" in completions

    def test_malformed_cache_data(self):
        """Test completion with malformed cache data."""
        # Write malformed cache
        assert self.cache_file is not None
        with open(self.cache_file, "w", encoding="utf-8") as f:
            f.write("DL_WORKSPACES=\n")  # Missing quotes
            f.write('DL_REPOS=""\n')

        # Should still complete global flags
        completions = self.run_completion("dl --")
        assert "--help" in completions


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
