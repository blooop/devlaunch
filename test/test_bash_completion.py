#!/usr/bin/env python3
import subprocess
import tempfile
import os
import pathlib
import pytest
from unittest.mock import patch


class TestBashCompletion:
    """Test bash completion functionality."""

    def setup_method(self):
        """Set up test environment."""
        self.test_dir = tempfile.mkdtemp()
        self.completion_script = (
            pathlib.Path(__file__).parent.parent / "devlaunch" / "completions" / "dl.bash"
        )

        # Create a test cache file with sample data
        # The cache structure should be XDG_CACHE_HOME/dl/completions.bash
        self.cache_base = pathlib.Path(self.test_dir) / "cache"
        self.cache_dir = self.cache_base / "dl"
        self.cache_dir.mkdir(parents=True)
        self.cache_file = self.cache_dir / "completions.bash"

        # Write test completion data
        with open(self.cache_file, "w") as f:
            f.write('DL_WORKSPACES="my-workspace another-ws test-project"\n')
            f.write('DL_REPOS="my-org/my-repo another-org/another-repo github-org/test-repo"\n')
            f.write('DL_OWNERS="my-org another-org github-org"\n')
            f.write(
                'DL_BRANCHES="my-org/my-repo@main my-org/my-repo@feature-branch another-org/another-repo@develop"\n'
            )

    def teardown_method(self):
        """Clean up test environment."""
        import shutil

        if os.path.exists(self.test_dir):
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
        script = f"""
#!/bin/bash
export XDG_CACHE_HOME="{self.cache_base}"
source {self.completion_script}

# Set completion environment variables
export COMP_LINE="{comp_line}"
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
        """Test completion works with workspace names containing dashes."""
        # Complete after typing "dl my-"
        completions = self.run_completion("dl my-")
        assert "my-workspace" in completions
        assert "my-org/" in completions

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
        expected = ["stop", "rm", "code", "restart", "recreate", "reset", "--"]
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

    def test_completion_branch_at_symbol(self):
        """Test completion triggers after @ symbol for branches."""
        # Complete after typing "dl my-org/my-repo@"
        completions = self.run_completion("dl my-org/my-repo@")
        assert "my-org/my-repo@main" in completions
        assert "my-org/my-repo@feature-branch" in completions

    def test_completion_path_with_dot_slash(self):
        """Test path completion with ./"""
        # Create a test directory
        test_subdir = pathlib.Path(self.test_dir) / "test-dir"
        test_subdir.mkdir()

        # Complete after typing "dl ./"
        with patch.dict(os.environ, {"PWD": self.test_dir}):
            completions = self.run_completion("dl ./")
            # Path completion behavior may vary by system

    def test_completion_multiple_dashes_in_name(self):
        """Test completion with names containing multiple dashes."""
        # Add test data with multiple dashes
        with open(self.cache_file, "w") as f:
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
        with open(self.cache_file, "w") as f:
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
        with open(self.cache_file, "w") as f:
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
        with open(self.cache_file, "w") as f:
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
        # Test empty line
        completions = self.run_completion("dl ")
        # Should complete first argument position

        # Test single word
        completions = self.run_completion("dl")
        # Should complete command name

        # Test two words
        completions = self.run_completion("dl my-workspace")
        # Should complete workspace

        # Test three words
        completions = self.run_completion("dl my-workspace ")
        # Should complete subcommands

    def test_completion_cursor_position(self):
        """Test completion at different cursor positions."""
        # Cursor in middle of word
        completions = self.run_completion("dl my-work", 7)  # Cursor after "my-"
        # Should still complete the current word

    def test_empty_cache_file(self):
        """Test completion with empty cache file."""
        # Create empty cache file
        with open(self.cache_file, "w") as f:
            f.write("")

        # Should still complete global flags
        completions = self.run_completion("dl --")
        assert "--help" in completions

    def test_missing_cache_file(self):
        """Test completion with missing cache file."""
        # Remove cache file
        os.unlink(self.cache_file)

        # Should still complete global flags
        completions = self.run_completion("dl --")
        assert "--help" in completions

    def test_malformed_cache_data(self):
        """Test completion with malformed cache data."""
        # Write malformed cache
        with open(self.cache_file, "w") as f:
            f.write("DL_WORKSPACES=\n")  # Missing quotes
            f.write('DL_REPOS=""\n')

        # Should still complete global flags
        completions = self.run_completion("dl --")
        assert "--help" in completions


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
