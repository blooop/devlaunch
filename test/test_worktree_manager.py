"""Tests for worktree manager."""

from devlaunch.worktree.worktree_manager import sanitize_branch_name


class TestSanitizeBranchName:
    """Tests for branch name sanitization."""

    def test_simple_branch(self):
        """Test sanitizing a simple branch name."""
        assert sanitize_branch_name("feature") == "feature"
        assert sanitize_branch_name("main") == "main"
        assert sanitize_branch_name("develop") == "develop"

    def test_branch_with_slash(self):
        """Test sanitizing branch with slashes."""
        assert sanitize_branch_name("feature/auth") == "feature-auth"
        assert sanitize_branch_name("bugfix/login-issue") == "bugfix-login-issue"
        assert sanitize_branch_name("release/v1.2.3") == "release-v1.2.3"

    def test_branch_with_special_chars(self):
        """Test sanitizing branch with special characters."""
        assert sanitize_branch_name("feature#123") == "feature_123"
        assert sanitize_branch_name("bug@fix") == "bug_fix"
        assert sanitize_branch_name("feat!important") == "feat_important"
        assert sanitize_branch_name("branch(test)") == "branch_test_"

    def test_branch_with_dots(self):
        """Test sanitizing branch with dots."""
        assert sanitize_branch_name("v1.2.3") == "v1.2.3"
        assert sanitize_branch_name(".hidden") == "hidden"
        assert sanitize_branch_name("branch.") == "branch"
        assert sanitize_branch_name("...dots...") == "dots"

    def test_branch_with_hyphens(self):
        """Test sanitizing branch with hyphens."""
        assert sanitize_branch_name("feature-branch") == "feature-branch"
        assert sanitize_branch_name("-leading") == "leading"
        assert sanitize_branch_name("trailing-") == "trailing"
        assert sanitize_branch_name("---multiple---") == "multiple"

    def test_branch_with_underscores(self):
        """Test sanitizing branch with underscores."""
        assert sanitize_branch_name("feature_branch") == "feature_branch"
        assert sanitize_branch_name("_leading") == "_leading"
        assert sanitize_branch_name("trailing_") == "trailing_"

    def test_complex_branch(self):
        """Test sanitizing complex branch names."""
        assert (
            sanitize_branch_name("feature/JIRA-123/auth-system") == "feature-JIRA-123-auth-system"
        )
        assert sanitize_branch_name("user@domain/feature") == "user_domain-feature"
        assert sanitize_branch_name("release/v2.0.0-beta.1") == "release-v2.0.0-beta.1"

    def test_empty_and_edge_cases(self):
        """Test edge cases."""
        assert sanitize_branch_name("") == ""
        assert sanitize_branch_name("a") == "a"
        assert sanitize_branch_name("123") == "123"
        assert sanitize_branch_name("...") == ""
        assert sanitize_branch_name("---") == ""
        assert sanitize_branch_name("/") == ""
