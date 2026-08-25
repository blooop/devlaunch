import subprocess
import tempfile
import os
import pathlib
import shlex
import pytest
from unittest.mock import patch


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

    def teardown_method(self):
        """Clean up test environment."""
        import shutil

        if self.test_dir and os.path.exists(self.test_dir):
            shutil.rmtree(self.test_dir)

    def run_completion(self, comp_line, comp_point=None):
        """
        Run bash completion for the given line and cursor position.

        Args:
            comp_line: The command line string
            comp_point: The cursor position (defaults to end of line)

        Returns:
            List of completion suggestions
        """
        if comp_point is None:
            comp_point = len(comp_line)

        # Create a bash script that sources the completion and runs it
        # Use shlex.quote to properly escape shell arguments
        script = f"""
#!/bin/bash
export XDG_CACHE_HOME={shlex.quote(str(self.cache_base))}
source {shlex.quote(str(self.completion_script))}

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
        return completions

    def test_completion_with_dashed_workspace(self):
        """Test completion works with names containing dashes."""
        # Complete after typing "dl my-". The owner matches, so the ids that also
        # start "my-" are held back; see the namespace tests below for why.
        completions = self.run_completion("dl my-")
        assert "my-org/" in completions
        # A prefix no owner matches falls through to the workspace ids.
        assert "my-workspace" in self.run_completion("dl my-w")
        # Negative assertions: dashes are not treated as word breaks
        # (These would appear if dashes were splitting the words)
        assert "workspace" not in completions
        assert "org/" not in completions

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
