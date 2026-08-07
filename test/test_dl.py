"""Tests for dl (DevLaunch CLI) functionality."""

import json
import subprocess
import sys
import tempfile
import pathlib
from unittest.mock import patch, MagicMock
import pytest

from devlaunch.dl import (
    expand_workspace_spec,
    is_path_spec,
    is_git_spec,
    validate_workspace_spec,
    parse_owner_repo_from_url,
    parse_owner_repo_branch,
    discover_repos_from_workspaces,
    discover_repos_from_cache_dir,
    get_known_repos,
    Workspace,
    list_workspaces,
    get_workspace_ids,
    OWNER_REPO_PATTERN,
    spec_to_workspace_id,
    get_version,
    read_completion_cache,
    write_completion_cache,
    write_bash_completion_cache,
    update_completion_cache,
    remote_branch_exists,
    get_remote_head_sha,
    get_remote_branches,
    get_local_branches,
    main,
    print_help,
    print_workspaces,
    workspace_stop,
    workspace_delete,
    workspace_ssh,
    run_devpod,
    extract_devcontainer_flag,
    workspace_up,
    setup_hostname,
    get_workspace_state,
    get_gh_token,
)


class TestIsPathSpec:
    """Tests for is_path_spec function."""

    def test_dot_slash_is_path(self):
        """Test ./path is recognized as path."""
        assert is_path_spec("./my-project")

    def test_absolute_is_path(self):
        """Test /path is recognized as path."""
        assert is_path_spec("/home/user/project")

    def test_tilde_is_path(self):
        """Test ~/path is recognized as path."""
        assert is_path_spec("~/projects/test")

    def test_simple_name_not_path(self):
        """Test simple name is not a path."""
        assert not is_path_spec("myworkspace")

    def test_owner_repo_not_path(self):
        """Test owner/repo is not a path."""
        assert not is_path_spec("owner/repo")


class TestIsGitSpec:
    """Tests for is_git_spec function."""

    def test_owner_repo_is_git(self):
        """Test owner/repo is recognized as git."""
        assert is_git_spec("owner/repo")

    def test_owner_repo_with_branch_is_git(self):
        """Test owner/repo@branch is recognized as git."""
        assert is_git_spec("blooop/devlaunch@main")

    def test_github_url_is_git(self):
        """Test github.com URL is recognized as git."""
        assert is_git_spec("github.com/owner/repo")

    def test_gitlab_url_is_git(self):
        """Test gitlab.com URL is recognized as git."""
        assert is_git_spec("gitlab.com/owner/repo")

    def test_https_url_is_git(self):
        """Test https URL is recognized as git."""
        assert is_git_spec("https://github.com/owner/repo")

    def test_simple_name_not_git(self):
        """Test simple name is not git."""
        assert not is_git_spec("myworkspace")

    def test_path_not_git(self):
        """Test path is not git."""
        assert not is_git_spec("./my-project")


class TestValidateWorkspaceSpec:
    """Tests for validate_workspace_spec function."""

    def test_existing_workspace_valid(self):
        """Test existing workspace name is valid."""
        error = validate_workspace_spec("myws", ["myws", "other"])
        assert error is None

    def test_owner_repo_valid(self):
        """Test owner/repo is valid even if not existing."""
        error = validate_workspace_spec("owner/repo", [])
        assert error is None

    def test_owner_repo_with_branch_valid(self):
        """Test owner/repo@branch is valid."""
        error = validate_workspace_spec("blooop/devlaunch@main", [])
        assert error is None

    def test_path_valid(self):
        """Test path is valid even if not existing."""
        error = validate_workspace_spec("./my-project", [])
        assert error is None

    def test_unknown_name_invalid(self):
        """Test unknown simple name returns error."""
        error = validate_workspace_spec("blo", ["myws", "other"])
        assert error is not None
        assert "Unknown workspace 'blo'" in error

    def test_partial_name_invalid(self):
        """Test partial match is not valid."""
        error = validate_workspace_spec("my", ["myws", "myother"])
        assert error is not None


class TestExpandWorkspaceSpec:
    """Tests for expand_workspace_spec function."""

    def test_expand_owner_repo(self):
        """Test owner/repo expands to SSH URL."""
        assert expand_workspace_spec("loft-sh/devpod") == "git@github.com:loft-sh/devpod.git"

    def test_expand_owner_repo_with_branch(self):
        """Test owner/repo@branch expands correctly to SSH URL."""
        assert (
            expand_workspace_spec("blooop/devlaunch@main")
            == "git@github.com:blooop/devlaunch.git@main"
        )

    def test_expand_owner_repo_with_feature_branch(self):
        """Test owner/repo@feature/branch expands correctly to SSH URL."""
        assert (
            expand_workspace_spec("owner/repo@feature/my-branch")
            == "git@github.com:owner/repo.git@feature/my-branch"
        )

    @pytest.mark.parametrize(
        "spec",
        [
            # GitHub SSH URL without branch
            "git@github.com:owner/repo.git",
            # GitHub SSH URL with explicit branch suffix
            "git@github.com:owner/repo.git@feature/my-branch",
            # Other common SSH hosts to guard against accidental expansion
            "git@gitlab.com:owner/repo.git",
            "git@bitbucket.org:owner/repo.git",
            # Enterprise git hosts
            "git@enterprise.example.com:owner/repo.git",
        ],
    )
    def test_no_expand_ssh_url(self, spec):
        """Test SSH-style git@host: URLs (with/without branch) are not double-expanded."""
        assert expand_workspace_spec(spec) == spec

    def test_no_expand_local_path_dot(self):
        """Test ./path is not expanded."""
        assert expand_workspace_spec("./my-project") == "./my-project"

    def test_no_expand_local_path_absolute(self):
        """Test /path is not expanded."""
        assert expand_workspace_spec("/home/user/project") == "/home/user/project"

    def test_no_expand_local_path_tilde(self):
        """Test ~/path is not expanded."""
        assert expand_workspace_spec("~/projects/test") == "~/projects/test"

    def test_no_expand_github_url(self):
        """Test github.com/ URLs are not double-expanded."""
        assert expand_workspace_spec("github.com/owner/repo") == "github.com/owner/repo"

    def test_no_expand_gitlab_url(self):
        """Test gitlab.com/ URLs are not expanded."""
        assert expand_workspace_spec("gitlab.com/owner/repo") == "gitlab.com/owner/repo"

    def test_no_expand_full_url(self):
        """Test full URLs with protocol are not expanded."""
        assert (
            expand_workspace_spec("https://github.com/owner/repo")
            == "https://github.com/owner/repo"
        )

    def test_no_expand_workspace_name(self):
        """Test simple workspace names are not expanded."""
        assert expand_workspace_spec("myworkspace") == "myworkspace"

    def test_no_expand_workspace_with_dashes(self):
        """Test workspace names with dashes are not expanded."""
        assert expand_workspace_spec("my-workspace") == "my-workspace"


class TestOwnerRepoPattern:
    """Tests for the OWNER_REPO_PATTERN regex."""

    def test_matches_simple(self):
        """Test simple owner/repo matches."""
        assert OWNER_REPO_PATTERN.match("owner/repo")

    def test_matches_with_dashes(self):
        """Test owner/repo with dashes matches."""
        assert OWNER_REPO_PATTERN.match("loft-sh/devpod")

    def test_matches_with_dots(self):
        """Test owner/repo with dots matches."""
        assert OWNER_REPO_PATTERN.match("user.name/repo.name")

    def test_matches_with_underscores(self):
        """Test owner/repo with underscores matches."""
        assert OWNER_REPO_PATTERN.match("my_user/my_repo")

    def test_matches_with_branch(self):
        """Test owner/repo@branch matches."""
        assert OWNER_REPO_PATTERN.match("owner/repo@main")

    def test_matches_with_feature_branch(self):
        """Test owner/repo@feature/branch matches."""
        assert OWNER_REPO_PATTERN.match("owner/repo@feature/my-feature")

    def test_no_match_single_word(self):
        """Test single word doesn't match."""
        assert not OWNER_REPO_PATTERN.match("workspace")

    def test_no_match_path(self):
        """Test path doesn't match."""
        assert not OWNER_REPO_PATTERN.match("./path/to/project")

    def test_no_match_absolute_path(self):
        """Test absolute path doesn't match."""
        assert not OWNER_REPO_PATTERN.match("/home/user/project")


class TestWorkspace:
    """Tests for Workspace dataclass."""

    def test_from_json_local_folder(self):
        """Test parsing workspace with local folder source."""
        data = {
            "id": "myproject",
            "source": {"localFolder": "/home/user/myproject"},
            "lastUsed": "2024-01-01T12:00:00Z",
            "provider": {"name": "docker"},
            "ide": {"name": "vscode"},
        }
        ws = Workspace.from_json(data)
        assert ws.id == "myproject"
        assert ws.source_type == "local"
        assert ws.source == "/home/user/myproject"
        assert ws.provider == "docker"
        assert ws.ide == "vscode"

    def test_from_json_git_repository(self):
        """Test parsing workspace with git repository source."""
        data = {
            "id": "devpod",
            "source": {"gitRepository": "github.com/loft-sh/devpod"},
            "lastUsed": "2024-01-01T12:00:00Z",
            "provider": {"name": "docker"},
            "ide": {"name": "none"},
        }
        ws = Workspace.from_json(data)
        assert ws.id == "devpod"
        assert ws.source_type == "git"
        assert ws.source == "github.com/loft-sh/devpod"

    def test_from_json_unknown_source(self):
        """Test parsing workspace with unknown source type."""
        data = {
            "id": "unknown",
            "source": {"someOther": "value"},
            "lastUsed": "",
            "provider": {},
            "ide": {},
        }
        ws = Workspace.from_json(data)
        assert ws.id == "unknown"
        assert ws.source_type == "unknown"

    def test_from_json_missing_fields(self):
        """Test parsing workspace with missing optional fields."""
        data = {"id": "minimal"}
        ws = Workspace.from_json(data)
        assert ws.id == "minimal"
        assert ws.source_type == "unknown"
        assert ws.last_used == ""
        assert ws.provider == ""
        assert ws.ide == ""


class TestListWorkspaces:
    """Tests for list_workspaces function."""

    @patch("devlaunch.dl.run_devpod")
    def test_list_workspaces_success(self, mock_run):
        """Test successful workspace listing."""
        mock_result = MagicMock()
        mock_result.returncode = 0
        mock_result.stdout = json.dumps(
            [
                {
                    "id": "ws1",
                    "source": {"localFolder": "/path/to/ws1"},
                    "lastUsed": "2024-01-01T12:00:00Z",
                    "provider": {"name": "docker"},
                    "ide": {"name": "vscode"},
                },
                {
                    "id": "ws2",
                    "source": {"gitRepository": "github.com/owner/repo"},
                    "lastUsed": "2024-01-02T12:00:00Z",
                    "provider": {"name": "docker"},
                    "ide": {"name": "none"},
                },
            ]
        )
        mock_run.return_value = mock_result

        workspaces = list_workspaces()

        assert len(workspaces) == 2
        assert workspaces[0].id == "ws1"
        assert workspaces[1].id == "ws2"

    @patch("devlaunch.dl.run_devpod")
    def test_list_workspaces_empty(self, mock_run):
        """Test empty workspace list."""
        mock_result = MagicMock()
        mock_result.returncode = 0
        mock_result.stdout = "[]"
        mock_run.return_value = mock_result

        workspaces = list_workspaces()
        assert workspaces == []

    @patch("devlaunch.dl.run_devpod")
    def test_list_workspaces_error(self, mock_run):
        """Test handling of devpod error."""
        mock_result = MagicMock()
        mock_result.returncode = 1
        mock_result.stdout = ""
        mock_run.return_value = mock_result

        workspaces = list_workspaces()
        assert workspaces == []

    @patch("devlaunch.dl.run_devpod")
    def test_list_workspaces_invalid_json(self, mock_run):
        """Test handling of invalid JSON output."""
        mock_result = MagicMock()
        mock_result.returncode = 0
        mock_result.stdout = "not valid json"
        mock_run.return_value = mock_result

        workspaces = list_workspaces()
        assert workspaces == []


class TestGetWorkspaceIds:
    """Tests for get_workspace_ids function."""

    @patch("devlaunch.dl.list_workspaces")
    def test_get_workspace_ids(self, mock_list):
        """Test getting workspace IDs."""
        mock_list.return_value = [
            Workspace("ws1", "local", "/path", "", "docker", "vscode"),
            Workspace("ws2", "git", "github.com/o/r", "", "docker", "none"),
        ]

        ids = get_workspace_ids()
        assert ids == ["ws1", "ws2"]

    @patch("devlaunch.dl.list_workspaces")
    def test_get_workspace_ids_empty(self, mock_list):
        """Test getting workspace IDs when empty."""
        mock_list.return_value = []

        ids = get_workspace_ids()
        assert ids == []


class TestParseOwnerRepoFromUrl:
    """Tests for parse_owner_repo_from_url function."""

    def test_parse_ssh_url(self):
        """Test parsing git@github.com:owner/repo.git URL."""
        result = parse_owner_repo_from_url("git@github.com:blooop/python_template.git")
        assert result == ("blooop", "python_template")

    def test_parse_ssh_url_no_git_suffix(self):
        """Test parsing git@github.com:owner/repo URL without .git."""
        result = parse_owner_repo_from_url("git@github.com:blooop/devlaunch")
        assert result == ("blooop", "devlaunch")

    def test_parse_https_url(self):
        """Test parsing https://github.com/owner/repo.git URL."""
        result = parse_owner_repo_from_url("https://github.com/loft-sh/devpod.git")
        assert result == ("loft-sh", "devpod")

    def test_parse_https_url_no_git_suffix(self):
        """Test parsing https://github.com/owner/repo URL."""
        result = parse_owner_repo_from_url("https://github.com/owner/repo")
        assert result == ("owner", "repo")

    def test_parse_github_com_url(self):
        """Test parsing github.com/owner/repo URL."""
        result = parse_owner_repo_from_url("github.com/blooop/test")
        assert result == ("blooop", "test")

    def test_parse_invalid_url(self):
        """Test parsing non-GitHub URL returns None."""
        result = parse_owner_repo_from_url("https://gitlab.com/owner/repo")
        assert result is None

    def test_parse_random_string(self):
        """Test parsing random string returns None."""
        result = parse_owner_repo_from_url("not a url")
        assert result is None


class TestParseOwnerRepoBranch:
    """Tests for parse_owner_repo_branch function."""

    def test_simple_owner_repo(self):
        """Test owner/repo without branch."""
        result = parse_owner_repo_branch("blooop/devlaunch")
        assert result == ("blooop/devlaunch", None)

    def test_owner_repo_with_branch(self):
        """Test owner/repo@branch."""
        result = parse_owner_repo_branch("blooop/devlaunch@main")
        assert result == ("blooop/devlaunch", "main")

    def test_owner_repo_with_feature_branch(self):
        """Test owner/repo@feature/branch."""
        result = parse_owner_repo_branch("owner/repo@feature/my-branch")
        assert result == ("owner/repo", "feature/my-branch")

    def test_path_returns_none(self):
        """Test path spec returns None."""
        assert parse_owner_repo_branch("./my-project") is None
        assert parse_owner_repo_branch("/home/user/project") is None
        assert parse_owner_repo_branch("~/projects/test") is None

    def test_path_with_at_returns_none(self):
        """Test path spec with @ is still treated as path, not branch."""
        assert parse_owner_repo_branch("./my-project@foo") is None
        assert parse_owner_repo_branch("/home/user/project@branch") is None
        assert parse_owner_repo_branch("~/projects/test@main") is None

    def test_url_returns_none(self):
        """Test full URL returns None."""
        assert parse_owner_repo_branch("https://github.com/owner/repo") is None
        assert parse_owner_repo_branch("github.com/owner/repo") is None

    def test_url_with_at_returns_none(self):
        """Test full URL with @ is still treated as URL, not owner/repo+branch."""
        assert parse_owner_repo_branch("https://github.com/owner/repo@main") is None
        assert parse_owner_repo_branch("github.com/owner/repo@branch") is None

    def test_simple_name_returns_none(self):
        """Test simple workspace name returns None."""
        assert parse_owner_repo_branch("myworkspace") is None


class TestRemoteBranchFunctions:
    """Tests for remote branch functions."""

    @patch("subprocess.run")
    def test_remote_branch_exists_true(self, mock_run):
        """Test branch exists returns True."""
        mock_run.return_value = MagicMock(
            returncode=0,
            stdout="abc123\trefs/heads/main\n",
        )
        assert remote_branch_exists("owner/repo", "main") is True

    @patch("subprocess.run")
    def test_remote_branch_exists_false(self, mock_run):
        """Test branch doesn't exist returns False."""
        mock_run.return_value = MagicMock(
            returncode=0,
            stdout="",
        )
        assert remote_branch_exists("owner/repo", "nonexistent") is False

    @patch("subprocess.run")
    def test_remote_branch_exists_error(self, mock_run):
        """Test git error returns False."""
        mock_run.return_value = MagicMock(returncode=1, stdout="")
        assert remote_branch_exists("owner/repo", "main") is False

    @patch("subprocess.run")
    def test_get_remote_head_sha(self, mock_run):
        """Test getting HEAD SHA."""
        mock_run.return_value = MagicMock(
            returncode=0,
            stdout="abc123def456\tHEAD\n",
        )
        assert get_remote_head_sha("owner/repo") == "abc123def456"

    @patch("subprocess.run")
    def test_get_remote_head_sha_error(self, mock_run):
        """Test git error returns None."""
        mock_run.return_value = MagicMock(returncode=1, stdout="")
        assert get_remote_head_sha("owner/repo") is None

    @patch("subprocess.run")
    def test_get_remote_branches_success(self, mock_run):
        """Test getting list of branches from remote."""
        mock_run.return_value = MagicMock(
            returncode=0,
            stdout="abc123\trefs/heads/main\ndef456\trefs/heads/feature/test\n",
        )
        branches = get_remote_branches("owner/repo")
        assert branches == ["main", "feature/test"]

    @patch("subprocess.run")
    def test_get_remote_branches_empty(self, mock_run):
        """Test getting branches from repo with no branches."""
        mock_run.return_value = MagicMock(returncode=0, stdout="")
        branches = get_remote_branches("owner/repo")
        assert branches == []

    @patch("subprocess.run")
    def test_get_remote_branches_error(self, mock_run):
        """Test git error returns empty list."""
        mock_run.return_value = MagicMock(returncode=1, stdout="")
        branches = get_remote_branches("owner/repo")
        assert branches == []

    @patch("subprocess.run")
    def test_get_remote_branches_timeout(self, mock_run):
        """Test timeout returns empty list."""
        mock_run.side_effect = subprocess.TimeoutExpired(cmd="git", timeout=5)
        branches = get_remote_branches("owner/repo")
        assert branches == []

    @patch("subprocess.run")
    def test_get_remote_branches_os_error(self, mock_run):
        """Test OSError returns empty list."""
        mock_run.side_effect = OSError("git not found")
        branches = get_remote_branches("owner/repo")
        assert branches == []

    @patch("subprocess.run")
    def test_remote_branch_exists_os_error(self, mock_run):
        """Test OSError returns False."""
        mock_run.side_effect = OSError("git not found")
        assert remote_branch_exists("owner/repo", "main") is False

    @patch("subprocess.run")
    def test_get_remote_head_sha_os_error(self, mock_run):
        """Test OSError returns None."""
        mock_run.side_effect = OSError("git not found")
        assert get_remote_head_sha("owner/repo") is None

    @patch("subprocess.run")
    def test_get_remote_head_sha_empty_output(self, mock_run):
        """Test empty output returns None."""
        mock_run.return_value = MagicMock(returncode=0, stdout="")
        assert get_remote_head_sha("owner/repo") is None


class TestDiscoverReposFromWorkspaces:
    """Tests for discover_repos_from_workspaces function."""

    def test_discover_from_git_workspace(self):
        """Test discovering repo from git workspace."""
        workspaces = [
            Workspace("ws1", "git", "github.com/owner/repo", "", "docker", "vscode"),
        ]
        repos = discover_repos_from_workspaces(workspaces)
        assert repos == {"owner": ["repo"]}

    @patch("devlaunch.dl.get_git_remote_url")
    def test_discover_from_local_workspace(self, mock_remote):
        """Test discovering repo from local workspace with git remote."""
        mock_remote.return_value = "git@github.com:blooop/python_template.git"
        workspaces = [
            Workspace("ws1", "local", "/home/user/project", "", "docker", "vscode"),
        ]
        repos = discover_repos_from_workspaces(workspaces)
        assert repos == {"blooop": ["python_template"]}

    @patch("devlaunch.dl.get_git_remote_url")
    def test_discover_multiple_repos(self, mock_remote):
        """Test discovering multiple repos from different owners."""
        mock_remote.side_effect = [
            "git@github.com:owner1/repo1.git",
            "git@github.com:owner2/repo2.git",
            "git@github.com:owner1/repo3.git",
        ]
        workspaces = [
            Workspace("ws1", "local", "/path1", "", "docker", "vscode"),
            Workspace("ws2", "local", "/path2", "", "docker", "vscode"),
            Workspace("ws3", "local", "/path3", "", "docker", "vscode"),
        ]
        repos = discover_repos_from_workspaces(workspaces)
        assert repos == {"owner1": ["repo1", "repo3"], "owner2": ["repo2"]}

    @patch("devlaunch.dl.get_git_remote_url")
    def test_discover_no_remote(self, mock_remote):
        """Test workspace without git remote is skipped."""
        mock_remote.return_value = None
        workspaces = [
            Workspace("ws1", "local", "/path", "", "docker", "vscode"),
        ]
        repos = discover_repos_from_workspaces(workspaces)
        assert repos == {}


class TestDiscoverReposFromCacheDir:
    """Tests for discover_repos_from_cache_dir function."""

    def test_discovers_bare_repos(self):
        """Test discovering repos from bare repo directories."""
        with tempfile.TemporaryDirectory() as tmpdir:
            repos_dir = pathlib.Path(tmpdir)
            # Create owner/repo/.bare/ structures
            (repos_dir / "owner1" / "repoA" / ".bare").mkdir(parents=True)
            (repos_dir / "owner1" / "repoB" / ".bare").mkdir(parents=True)
            (repos_dir / "owner2" / "repoC" / ".bare").mkdir(parents=True)

            mock_config = MagicMock()
            mock_config.repos_dir = str(repos_dir)
            with patch("devlaunch.dl.get_worktree_config", return_value=mock_config):
                repos = discover_repos_from_cache_dir()

            assert sorted(repos.keys()) == ["owner1", "owner2"]
            assert sorted(repos["owner1"]) == ["repoA", "repoB"]
            assert repos["owner2"] == ["repoC"]

    def test_ignores_dirs_without_bare(self):
        """Test that directories without .bare/ are ignored."""
        with tempfile.TemporaryDirectory() as tmpdir:
            repos_dir = pathlib.Path(tmpdir)
            (repos_dir / "owner" / "has-bare" / ".bare").mkdir(parents=True)
            (repos_dir / "owner" / "no-bare").mkdir(parents=True)

            mock_config = MagicMock()
            mock_config.repos_dir = str(repos_dir)
            with patch("devlaunch.dl.get_worktree_config", return_value=mock_config):
                repos = discover_repos_from_cache_dir()

            assert repos == {"owner": ["has-bare"]}

    def test_empty_repos_dir(self):
        """Test with an empty repos directory."""
        with tempfile.TemporaryDirectory() as tmpdir:
            mock_config = MagicMock()
            mock_config.repos_dir = tmpdir
            with patch("devlaunch.dl.get_worktree_config", return_value=mock_config):
                repos = discover_repos_from_cache_dir()

            assert repos == {}

    def test_nonexistent_repos_dir(self):
        """Test with a repos directory that doesn't exist."""
        mock_config = MagicMock()
        mock_config.repos_dir = "/nonexistent/path"
        with patch("devlaunch.dl.get_worktree_config", return_value=mock_config):
            repos = discover_repos_from_cache_dir()

        assert repos == {}

    def test_handles_oserror_from_iterdir(self):
        """discover_repos_from_cache_dir returns empty dict on OSError during iteration."""
        with tempfile.TemporaryDirectory() as tmpdir:
            repos_dir = pathlib.Path(tmpdir)
            # Create a valid-looking structure so is_dir() passes
            (repos_dir / "owner" / "repo" / ".bare").mkdir(parents=True)

            mock_config = MagicMock()
            mock_config.repos_dir = str(repos_dir)
            with (
                patch("devlaunch.dl.get_worktree_config", return_value=mock_config),
                patch("pathlib.Path.iterdir", side_effect=OSError("permission denied")),
            ):
                repos = discover_repos_from_cache_dir()

            assert repos == {}


class TestGetLocalBranches:
    """Focused tests for get_local_branches behavior."""

    @patch("devlaunch.dl.get_worktree_config")
    @patch("devlaunch.dl.subprocess.run")
    def test_parses_branches_from_git_output(self, mock_run, mock_config, tmp_path):
        """Branches are parsed from realistic git for-each-ref output."""
        bare_path = tmp_path / "owner" / "repo" / ".bare"
        bare_path.mkdir(parents=True)
        mock_config.return_value.repos_dir = str(tmp_path)
        mock_run.return_value = MagicMock(returncode=0, stdout="main\nfeature/test\ndevelop\n")

        branches = get_local_branches("owner/repo")

        assert branches == ["main", "feature/test", "develop"]
        mock_run.assert_called_once()

    @patch("devlaunch.dl.get_worktree_config")
    @patch("devlaunch.dl.subprocess.run")
    def test_returns_empty_on_nonzero_exit(self, mock_run, mock_config, tmp_path):
        """Non-zero return code results in empty list."""
        bare_path = tmp_path / "owner" / "repo" / ".bare"
        bare_path.mkdir(parents=True)
        mock_config.return_value.repos_dir = str(tmp_path)
        mock_run.return_value = MagicMock(returncode=1, stdout="main\n")

        assert get_local_branches("owner/repo") == []

    @patch("devlaunch.dl.get_worktree_config")
    @patch("devlaunch.dl.subprocess.run")
    def test_returns_empty_on_empty_stdout(self, mock_run, mock_config, tmp_path):
        """Zero return code with empty stdout results in empty list."""
        bare_path = tmp_path / "owner" / "repo" / ".bare"
        bare_path.mkdir(parents=True)
        mock_config.return_value.repos_dir = str(tmp_path)
        mock_run.return_value = MagicMock(returncode=0, stdout="")

        assert get_local_branches("owner/repo") == []

    @patch("devlaunch.dl.get_worktree_config")
    @patch("devlaunch.dl.subprocess.run")
    def test_handles_timeout(self, mock_run, mock_config, tmp_path):
        """TimeoutExpired returns empty list."""
        bare_path = tmp_path / "owner" / "repo" / ".bare"
        bare_path.mkdir(parents=True)
        mock_config.return_value.repos_dir = str(tmp_path)
        mock_run.side_effect = subprocess.TimeoutExpired(cmd=["git"], timeout=2)

        assert get_local_branches("owner/repo") == []

    @patch("devlaunch.dl.get_worktree_config")
    @patch("devlaunch.dl.subprocess.run")
    def test_handles_oserror(self, mock_run, mock_config, tmp_path):
        """OSError (e.g. git not installed) returns empty list."""
        bare_path = tmp_path / "owner" / "repo" / ".bare"
        bare_path.mkdir(parents=True)
        mock_config.return_value.repos_dir = str(tmp_path)
        mock_run.side_effect = OSError("git not found")

        assert get_local_branches("owner/repo") == []

    @patch("devlaunch.dl.get_worktree_config")
    @patch("devlaunch.dl.subprocess.run")
    def test_missing_bare_dir_returns_empty(self, mock_run, mock_config, tmp_path):
        """Missing .bare directory short-circuits without calling git."""
        mock_config.return_value.repos_dir = str(tmp_path)
        # Don't create any .bare directory

        assert get_local_branches("owner/repo") == []
        mock_run.assert_not_called()


class TestGetKnownRepos:
    """Tests for get_known_repos function."""

    @patch("devlaunch.dl.list_workspaces")
    def test_get_known_repos(self, mock_list):
        """Test getting known repos as sorted list."""
        mock_list.return_value = [
            Workspace("ws1", "git", "github.com/zowner/zrepo", "", "docker", "vscode"),
            Workspace("ws2", "git", "github.com/aowner/arepo", "", "docker", "vscode"),
        ]
        repos = get_known_repos()
        assert repos == ["aowner/arepo", "zowner/zrepo"]

    @patch("devlaunch.dl.list_workspaces")
    def test_get_known_repos_empty(self, mock_list):
        """Test getting known repos when no workspaces."""
        mock_list.return_value = []
        repos = get_known_repos()
        assert repos == []


class TestGetVersion:
    """Tests for get_version function."""

    def test_get_version_returns_string(self):
        """Test that get_version returns a string."""
        version = get_version()
        assert isinstance(version, str)
        assert len(version) > 0

    @patch("devlaunch.dl.pkg_version")
    def test_get_version_package_not_found(self, mock_pkg_version):
        """Test get_version returns 'unknown' when package not found."""
        from importlib.metadata import PackageNotFoundError

        mock_pkg_version.side_effect = PackageNotFoundError("devlaunch")
        version = get_version()
        assert version == "unknown"


class TestSpecToWorkspaceId:
    """Tests for spec_to_workspace_id function."""

    def test_owner_repo_produces_repo_name(self):
        """Test owner/repo generates sanitized repo name as workspace ID."""
        assert spec_to_workspace_id("blooop/devlaunch") == "devlaunch"

    def test_owner_repo_with_branch_uses_repo_branch(self):
        """Test owner/repo@branch uses <repo>-<branch> as workspace ID."""
        assert spec_to_workspace_id("blooop/devlaunch@main") == "devlaunch-main"

    def test_owner_repo_with_feature_branch(self):
        """Test owner/repo@feature/branch sanitizes branch name."""
        assert spec_to_workspace_id("owner/repo@feature/my-branch") == "repo-feature-my-branch"

    def test_owner_repo_with_uppercase_branch(self):
        """Test branch name is lowercased and underscores replaced."""
        assert spec_to_workspace_id("Owner/Repo@Feature/MyBranch") == "repo-feature-mybranch"

    def test_github_url_sanitized(self):
        """Test github.com/owner/repo generates sanitized ID (fallback path)."""
        assert spec_to_workspace_id("github.com/loft-sh/devpod") == "github-com-loft-sh-devpod"

    def test_https_url_strips_protocol(self):
        """Test https URL strips protocol and sanitizes (fallback path)."""
        assert spec_to_workspace_id("https://github.com/owner/repo") == "github-com-owner-repo"

    def test_url_with_git_suffix_strips_it(self):
        """Test URL with .git suffix strips it (fallback path)."""
        assert spec_to_workspace_id("github.com/owner/repo.git") == "github-com-owner-repo"

    def test_underscore_replaced_in_repo(self):
        """Test underscores are replaced with hyphens in repo name."""
        assert spec_to_workspace_id("blooop/test_renv") == "test-renv"

    def test_branch_allows_multiple_workspaces(self):
        """Test different branches get different workspace IDs."""
        assert spec_to_workspace_id("blooop/test_renv@nb12") == "test-renv-nb12"
        assert spec_to_workspace_id("blooop/test_renv@nb14") == "test-renv-nb14"
        # Different branches = different IDs = can be open simultaneously

    def test_branch_truncation(self):
        """Test branch name is truncated so total stays <= 48 chars."""
        long_branch = "a" * 60
        result = spec_to_workspace_id(f"owner/repo@{long_branch}")
        assert len(result) <= 48
        assert result.startswith("repo-")

    def test_branch_truncation_strips_trailing_hyphen(self):
        """Test truncated branch doesn't end with a hyphen."""
        # Use a branch that after truncation would end with '-'
        result = spec_to_workspace_id("owner/myrepo@feature/some-very-long-branch-name-here")
        assert not result.endswith("-")
        assert len(result) <= 48

    def test_path_extracts_directory_name(self):
        """Test path extracts directory name."""
        result = spec_to_workspace_id("./my-project")
        assert result == "my-project"

    def test_existing_workspace_id(self):
        """Test existing workspace ID is returned as-is."""
        assert spec_to_workspace_id("myworkspace") == "myworkspace"

    def test_python_template_example(self):
        """Test the motivating example from the plan."""
        assert spec_to_workspace_id("blooop/python_template@nb4") == "python-template-nb4"

    def test_no_branch_no_suffix(self):
        """Test owner/repo without branch produces just repo name."""
        assert spec_to_workspace_id("blooop/python_template") == "python-template"


class TestCacheFunctions:
    """Tests for cache read/write functions."""

    def test_write_and_read_completion_cache(self):
        """Test writing and reading completion cache."""
        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl.CACHE_FILE", pathlib.Path(tmpdir) / "cache.json"):
                data = {"workspaces": ["ws1", "ws2"], "repos": ["a/b"], "owners": ["a"]}
                write_completion_cache(data)
                result = read_completion_cache()
                assert result == data

    def test_read_nonexistent_cache(self):
        """Test reading nonexistent cache returns None."""
        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl.CACHE_FILE", pathlib.Path(tmpdir) / "nonexistent.json"):
                result = read_completion_cache()
                assert result is None

    def test_write_bash_completion_cache(self):
        """Test writing bash completion cache."""
        with tempfile.TemporaryDirectory() as tmpdir:
            bash_file = pathlib.Path(tmpdir) / "completions.bash"
            with patch("devlaunch.dl.BASH_CACHE_FILE", bash_file):
                data = {"workspaces": ["ws1", "ws2"], "repos": ["a/b"], "owners": ["a"]}
                write_bash_completion_cache(data)
                content = bash_file.read_text()
                assert 'DL_WORKSPACES="ws1 ws2"' in content
                assert 'DL_REPOS="a/b"' in content
                assert 'DL_OWNERS="a"' in content

    def test_write_bash_completion_cache_with_branches(self):
        """Test writing bash completion cache includes branches."""
        with tempfile.TemporaryDirectory() as tmpdir:
            bash_file = pathlib.Path(tmpdir) / "completions.bash"
            with patch("devlaunch.dl.BASH_CACHE_FILE", bash_file):
                data = {
                    "workspaces": ["ws1"],
                    "repos": ["owner/repo"],
                    "owners": ["owner"],
                    "branches": ["owner/repo@main", "owner/repo@develop"],
                }
                write_bash_completion_cache(data)
                content = bash_file.read_text()
                assert 'DL_BRANCHES="owner/repo@main owner/repo@develop"' in content

    def test_write_and_read_cache_with_branches(self):
        """Test cache roundtrip includes branches."""
        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl.CACHE_FILE", pathlib.Path(tmpdir) / "cache.json"):
                data = {
                    "workspaces": ["ws1"],
                    "repos": ["owner/repo"],
                    "owners": ["owner"],
                    "branches": ["owner/repo@main", "owner/repo@feature/test"],
                }
                write_completion_cache(data)
                result = read_completion_cache()
                assert result is not None
                assert result == data
                assert result["branches"] == ["owner/repo@main", "owner/repo@feature/test"]

    @patch("devlaunch.dl.discover_repos_from_cache_dir", return_value={})
    @patch("devlaunch.dl.get_local_branches", return_value=[])
    @patch("devlaunch.dl.get_remote_branches")
    @patch("devlaunch.dl.discover_repos_from_workspaces")
    @patch("devlaunch.dl.list_workspaces")
    def test_update_completion_cache_fetches_branches(
        self, mock_list, mock_discover, mock_remote, _mock_local, _mock_cache_dir
    ):
        """Test update_completion_cache fetches branches for all repos."""
        mock_list.return_value = [
            Workspace("ws1", "git", "github.com/owner/repo1", "", "docker", "vscode"),
        ]
        mock_discover.return_value = {"owner": ["repo1", "repo2"]}
        mock_remote.side_effect = [
            ["main", "develop"],  # branches for owner/repo1
            ["main", "feature/x"],  # branches for owner/repo2
        ]

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl.CACHE_FILE", pathlib.Path(tmpdir) / "cache.json"):
                with patch(
                    "devlaunch.dl.BASH_CACHE_FILE", pathlib.Path(tmpdir) / "completions.bash"
                ):
                    data = update_completion_cache()

        assert "branches" in data
        assert "owner/repo1@main" in data["branches"]
        assert "owner/repo1@develop" in data["branches"]
        assert "owner/repo2@main" in data["branches"]
        assert "owner/repo2@feature/x" in data["branches"]
        assert len(data["branches"]) == 4

    @patch("devlaunch.dl.discover_repos_from_cache_dir", return_value={})
    @patch("devlaunch.dl.get_local_branches", return_value=[])
    @patch("devlaunch.dl.get_remote_branches")
    @patch("devlaunch.dl.discover_repos_from_workspaces")
    @patch("devlaunch.dl.list_workspaces")
    def test_update_completion_cache_handles_branch_fetch_failure(
        self, mock_list, mock_discover, mock_remote, _mock_local, _mock_cache_dir
    ):
        """Test update_completion_cache handles repos where branch fetch fails."""
        mock_list.return_value = []
        mock_discover.return_value = {"owner": ["repo1"]}
        mock_remote.return_value = []  # Branch fetch failed

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl.CACHE_FILE", pathlib.Path(tmpdir) / "cache.json"):
                with patch(
                    "devlaunch.dl.BASH_CACHE_FILE", pathlib.Path(tmpdir) / "completions.bash"
                ):
                    data = update_completion_cache()

        assert data["branches"] == []

    @patch("devlaunch.dl.discover_repos_from_cache_dir", return_value={})
    @patch("devlaunch.dl.get_local_branches")
    @patch("devlaunch.dl.get_remote_branches")
    @patch("devlaunch.dl.discover_repos_from_workspaces")
    @patch("devlaunch.dl.list_workspaces")
    def test_update_completion_cache_includes_local_branches(
        self, mock_list, mock_discover, mock_remote, mock_local, _mock_cache_dir
    ):
        """Test that locally-created branches appear in completion cache."""
        mock_list.return_value = [
            Workspace("ws1", "git", "github.com/owner/repo1", "", "docker", "vscode"),
        ]
        mock_discover.return_value = {"owner": ["repo1"]}
        mock_remote.return_value = ["main", "develop"]
        mock_local.return_value = ["main", "my-local-branch"]  # local-only branch

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl.CACHE_FILE", pathlib.Path(tmpdir) / "cache.json"):
                with patch(
                    "devlaunch.dl.BASH_CACHE_FILE", pathlib.Path(tmpdir) / "completions.bash"
                ):
                    data = update_completion_cache()

        assert "owner/repo1@main" in data["branches"]
        assert "owner/repo1@develop" in data["branches"]
        assert "owner/repo1@my-local-branch" in data["branches"]
        assert len(data["branches"]) == 3  # deduplicated "main"

    @patch("devlaunch.dl.get_local_branches")
    @patch("devlaunch.dl.get_remote_branches")
    @patch("devlaunch.dl.discover_repos_from_cache_dir")
    @patch("devlaunch.dl.discover_repos_from_workspaces")
    @patch("devlaunch.dl.list_workspaces")
    def test_update_completion_cache_includes_cache_dir_repos(
        self, mock_list, mock_ws_repos, mock_cache_repos, mock_remote, mock_local
    ):
        """Test repos from cache dir appear in completions even when devpod list is empty."""
        mock_list.return_value = []  # No devpod workspaces
        mock_ws_repos.return_value = {}  # No workspace-discovered repos
        mock_cache_repos.return_value = {"newowner": ["newrepo"]}
        mock_remote.return_value = ["main"]
        mock_local.return_value = ["main", "feature-x"]

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl.CACHE_FILE", pathlib.Path(tmpdir) / "cache.json"):
                with patch(
                    "devlaunch.dl.BASH_CACHE_FILE", pathlib.Path(tmpdir) / "completions.bash"
                ):
                    data = update_completion_cache()

        assert "newowner/newrepo" in data["repos"]
        assert "newowner" in data["owners"]
        assert "newowner/newrepo@main" in data["branches"]
        assert "newowner/newrepo@feature-x" in data["branches"]
        assert len(data["branches"]) == 2  # deduplicated "main"

    @patch("devlaunch.dl.get_local_branches", return_value=[])
    @patch("devlaunch.dl.get_remote_branches")
    @patch("devlaunch.dl.discover_repos_from_cache_dir")
    @patch("devlaunch.dl.discover_repos_from_workspaces")
    @patch("devlaunch.dl.list_workspaces")
    def test_update_completion_cache_merges_workspace_and_cache_repos(
        self, mock_list, mock_ws_repos, mock_cache_repos, mock_remote, _mock_local
    ):
        """Test repos from both sources are merged without duplicates."""
        mock_list.return_value = []
        mock_ws_repos.return_value = {"owner": ["repo1"]}
        mock_cache_repos.return_value = {"owner": ["repo1", "repo2"], "other": ["repo3"]}
        mock_remote.side_effect = [
            ["main"],  # owner/repo1
            ["main"],  # owner/repo2
            ["main"],  # other/repo3
        ]

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl.CACHE_FILE", pathlib.Path(tmpdir) / "cache.json"):
                with patch(
                    "devlaunch.dl.BASH_CACHE_FILE", pathlib.Path(tmpdir) / "completions.bash"
                ):
                    data = update_completion_cache()

        assert sorted(data["repos"]) == ["other/repo3", "owner/repo1", "owner/repo2"]
        assert sorted(data["owners"]) == ["other", "owner"]


class TestRunDevpod:
    """Tests for run_devpod function."""

    @patch("devlaunch.dl.subprocess.run")
    def test_run_devpod_basic(self, mock_run):
        """Test basic devpod command execution."""
        mock_run.return_value = MagicMock(returncode=0)
        result = run_devpod(["list"])
        mock_run.assert_called_once()
        assert result.returncode == 0

    @patch("devlaunch.dl.subprocess.run")
    def test_run_devpod_capture(self, mock_run):
        """Test devpod command with capture."""
        mock_run.return_value = MagicMock(returncode=0, stdout="output")
        run_devpod(["list"], capture=True)
        mock_run.assert_called_once()
        call_kwargs = mock_run.call_args[1]
        assert call_kwargs["capture_output"] is True


class TestWorkspaceIdentityEnv:
    """Tests for the workspace identity handed to devpod's initializeCommand."""

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_workspace_up_injects_workspace_id(self, mock_run, _mock_ctx):
        """The id is injected via --init-env, devpod's channel into the hook.

        devpod gives initializeCommand no workspace identity of its own, and it
        runs on the host before the container exists.
        """
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path/to/clone", workspace_id=None, workspace_identity="repo-nb4")
        args = mock_run.call_args[0][0]
        assert "--init-env" in args
        assert args[args.index("--init-env") + 1] == "DEVLAUNCH_WORKSPACE_ID=repo-nb4"

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_workspace_up_falls_back_to_the_creation_id(self, mock_run, _mock_ctx):
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path", workspace_id="repo-main")
        args = mock_run.call_args[0][0]
        assert args[args.index("--init-env") + 1] == "DEVLAUNCH_WORKSPACE_ID=repo-main"

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_workspace_up_omits_init_env_without_an_identity(self, mock_run, _mock_ctx):
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        assert "--init-env" not in mock_run.call_args[0][0]


class TestGhTokenForwarding:
    """gh must work inside the workspace without a keyring in the container.

    Bind-mounting ~/.config/gh is not enough: gh keeps the OAuth token in the
    host keyring, so the mount carries config but no credential.
    """

    @patch("devlaunch.dl.subprocess.run")
    def test_prefers_an_existing_env_token_over_shelling_out(self, mock_run, monkeypatch):
        monkeypatch.setenv("GH_TOKEN", "gho_from_env")
        assert get_gh_token() == "gho_from_env"
        mock_run.assert_not_called()

    @patch("devlaunch.dl.subprocess.run")
    def test_falls_back_to_gh_auth_token(self, mock_run, monkeypatch):
        monkeypatch.delenv("GH_TOKEN", raising=False)
        monkeypatch.delenv("GITHUB_TOKEN", raising=False)
        mock_run.return_value = MagicMock(returncode=0, stdout="gho_from_keyring\n")
        assert get_gh_token() == "gho_from_keyring"
        assert mock_run.call_args[0][0] == ["gh", "auth", "token"]

    @patch("devlaunch.dl.subprocess.run")
    def test_returns_none_when_gh_is_not_logged_in(self, mock_run, monkeypatch):
        monkeypatch.delenv("GH_TOKEN", raising=False)
        monkeypatch.delenv("GITHUB_TOKEN", raising=False)
        mock_run.return_value = MagicMock(returncode=1, stdout="")
        assert get_gh_token() is None

    @patch("devlaunch.dl.subprocess.run", side_effect=FileNotFoundError)
    def test_returns_none_when_gh_is_not_installed(self, _mock_run, monkeypatch):
        """A missing gh must never stop a workspace from starting."""
        monkeypatch.delenv("GH_TOKEN", raising=False)
        monkeypatch.delenv("GITHUB_TOKEN", raising=False)
        assert get_gh_token() is None

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.get_gh_token", return_value="gho_secret")
    @patch("devlaunch.dl.run_devpod")
    def test_workspace_up_forwards_the_token_in_a_file(
        self, mock_run, _mock_token, _mock_ctx, monkeypatch
    ):
        """The token goes in a file, never in argv, which /proc exposes."""
        monkeypatch.delenv("DEVLAUNCH_NO_GH_TOKEN", raising=False)
        seen = {}

        def capture(args, **_kwargs):
            path = pathlib.Path(args[args.index("--workspace-env-file") + 1])
            seen["contents"] = path.read_text(encoding="utf-8")
            seen["mode"] = path.stat().st_mode & 0o777
            seen["path"] = path
            return MagicMock(returncode=0)

        mock_run.side_effect = capture
        workspace_up("/path")

        assert seen["contents"] == "GH_TOKEN=gho_secret\n"
        assert seen["mode"] == 0o600
        assert not seen["path"].exists(), "temp token file must be cleaned up"
        assert not any("gho_secret" in a for a in mock_run.call_args[0][0])

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.get_gh_token", return_value="gho_secret")
    @patch("devlaunch.dl.run_devpod", side_effect=RuntimeError("devpod blew up"))
    def test_token_file_is_removed_even_when_devpod_fails(
        self, mock_run, _mock_token, _mock_ctx, monkeypatch
    ):
        monkeypatch.delenv("DEVLAUNCH_NO_GH_TOKEN", raising=False)
        with pytest.raises(RuntimeError):
            workspace_up("/path")
        path = pathlib.Path(mock_run.call_args[0][0][-1])
        assert not path.exists()

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.get_gh_token", return_value=None)
    @patch("devlaunch.dl.run_devpod")
    def test_no_flag_when_there_is_no_token(self, mock_run, _mock_token, _mock_ctx, monkeypatch):
        monkeypatch.delenv("DEVLAUNCH_NO_GH_TOKEN", raising=False)
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        assert "--workspace-env-file" not in mock_run.call_args[0][0]

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.get_gh_token")
    @patch("devlaunch.dl.run_devpod")
    def test_opt_out_skips_the_lookup_entirely(self, mock_run, mock_token, _mock_ctx, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_NO_GH_TOKEN", "1")
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        mock_token.assert_not_called()
        assert "--workspace-env-file" not in mock_run.call_args[0][0]

    @patch("devlaunch.dl.get_gh_token", return_value="gho_secret")
    @patch("devlaunch.dl.run_devpod")
    def test_ssh_forwards_the_token_for_fast_attach(self, mock_run, _mock_token, monkeypatch):
        """`dl <ws>` on a running workspace never calls up, so ssh must forward.

        Without this, attaching to an already-running container leaves gh
        unauthenticated no matter what up did.
        """
        monkeypatch.delenv("DEVLAUNCH_NO_GH_TOKEN", raising=False)
        mock_run.return_value = MagicMock(returncode=0)
        workspace_ssh("myws")

        args, kwargs = mock_run.call_args[0][0], mock_run.call_args[1]
        assert args[args.index("--send-env") + 1] == "GH_TOKEN"
        assert kwargs["env"]["GH_TOKEN"] == "gho_secret"
        assert not any("gho_secret" in a for a in args), "token must not reach argv"

    @patch("devlaunch.dl.get_gh_token", return_value=None)
    @patch("devlaunch.dl.run_devpod")
    def test_ssh_inherits_the_environment_when_there_is_no_token(
        self, mock_run, _mock_token, monkeypatch
    ):
        monkeypatch.delenv("DEVLAUNCH_NO_GH_TOKEN", raising=False)
        mock_run.return_value = MagicMock(returncode=0)
        workspace_ssh("myws")
        assert "--send-env" not in mock_run.call_args[0][0]
        assert mock_run.call_args[1]["env"] is None

    @patch("devlaunch.dl.get_gh_token")
    @patch("devlaunch.dl.run_devpod")
    def test_ssh_honours_the_opt_out(self, mock_run, mock_token, monkeypatch):
        monkeypatch.setenv("DEVLAUNCH_NO_GH_TOKEN", "1")
        mock_run.return_value = MagicMock(returncode=0)
        workspace_ssh("myws")
        mock_token.assert_not_called()
        assert "--send-env" not in mock_run.call_args[0][0]

    @patch("devlaunch.dl.get_workspace_state", return_value="Running")
    @patch("devlaunch.dl.get_workspace_ids", return_value=["myws"])
    @patch("devlaunch.dl.get_gh_token", return_value="gho_secret")
    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_fast_attach_through_main_still_forwards(
        self, mock_run, _mock_ctx, _mock_token, _mock_ids, _mock_state, monkeypatch
    ):
        """End-to-end through argv: the real `dl myws` path on a live workspace."""
        monkeypatch.delenv("DEVLAUNCH_NO_GH_TOKEN", raising=False)
        mock_run.return_value = MagicMock(returncode=0, stdout="", stderr="")
        with patch.object(sys, "argv", ["dl", "myws"]):
            main()

        ssh_calls = [c for c in mock_run.call_args_list if c[0][0][:1] == ["ssh"]]
        assert ssh_calls, "expected a devpod ssh call"
        assert "--send-env" in ssh_calls[-1][0][0]
        assert ssh_calls[-1][1]["env"]["GH_TOKEN"] == "gho_secret"


class TestIdentityReachesDevpod:
    """The workspace identity travels as an argument, not inherited env."""

    @patch("devlaunch.dl.subprocess.run")
    def test_run_devpod_does_not_touch_the_environment(self, mock_run):
        """The identity travels as a devpod argument, not as inherited env."""
        mock_run.return_value = MagicMock(returncode=0)
        run_devpod(["up", "/path"])
        assert mock_run.call_args[1].get("env") is None

    @patch("devlaunch.dl.get_workspace_ids", return_value=["myws"])
    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_identity_reaches_devpod_through_main(self, mock_run, _mock_ctx, _mock_ssh, _mock_ids):
        """End-to-end through argv, so the main() wiring itself is pinned.

        Asserting on workspace_up's kwargs alone let the whole feature be
        disabled in main() without any test failing.
        """
        mock_run.return_value = MagicMock(returncode=0, stdout="", stderr="")
        with patch.object(sys, "argv", ["dl", "myws"]):
            main()
        up_calls = [c for c in mock_run.call_args_list if c[0][0][:1] == ["up"]]
        assert up_calls, "expected a devpod up call"
        assert "DEVLAUNCH_WORKSPACE_ID=myws" in up_calls[0][0][0]


class TestIdeSelection:
    """Tests for which IDE devpod is told to open."""

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_default_up_opens_no_ide(self, mock_run, _mock_ctx):
        """dl attaches a terminal shell, so devpod must not also open an editor.

        Left unset, devpod falls back to its configured default (vscode) and pops
        a window open on every `dl <ws>`.
        """
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        args = mock_run.call_args[0][0]
        assert args[args.index("--ide") + 1] == "none"

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_explicit_ide_is_honoured(self, mock_run, _mock_ctx):
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path", ide="vscode")
        args = mock_run.call_args[0][0]
        assert args[args.index("--ide") + 1] == "vscode"


class TestDevcontainerPath:
    """Tests for selecting a non-default devcontainer.json."""

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_workspace_up_passes_the_selection(self, mock_run, _mock_ctx):
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path", devcontainer=".devcontainer/sim/devcontainer.json")
        args = mock_run.call_args[0][0]
        assert args[args.index("--devcontainer-path") + 1] == (
            ".devcontainer/sim/devcontainer.json"
        )

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_workspace_up_omits_it_by_default(self, mock_run, _mock_ctx):
        """No flag means devpod uses the repo's default devcontainer.json."""
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        assert "--devcontainer-path" not in mock_run.call_args[0][0]

    def test_flag_is_stripped_from_args(self):
        args, selection = extract_devcontainer_flag(["repo", "--devcontainer", "x.json", "stop"])
        assert args == ["repo", "stop"]
        assert selection == "x.json"

    def test_flag_absent(self):
        args, selection = extract_devcontainer_flag(["repo", "stop"])
        assert args == ["repo", "stop"]
        assert selection is None

    def test_equals_form_is_accepted(self):
        args, selection = extract_devcontainer_flag(["repo", "--devcontainer=sim"])
        assert args == ["repo"]
        assert selection == ".devcontainer/sim/devcontainer.json"

    def test_bare_name_expands_to_the_spec_location(self):
        """devpod's --devcontainer-id is ignored in 0.26.1, so build the path."""
        _, selection = extract_devcontainer_flag(["repo", "--devcontainer", "sim"])
        assert selection == ".devcontainer/sim/devcontainer.json"

    def test_path_shaped_values_become_a_path(self):
        for given in (
            ".devcontainer/sim/devcontainer.json",
            "sub/dir/devcontainer.json",
            ".devcontainer.json",
            "./weird.json",
        ):
            _, selection = extract_devcontainer_flag(["repo", "--devcontainer", given])
            assert selection == given

    def test_dangling_flag_is_an_error(self):
        """A dangling --devcontainer is an error, not a silently ignored flag."""
        with pytest.raises(ValueError):
            extract_devcontainer_flag(["repo", "--devcontainer"])

    @pytest.mark.parametrize("bad", ["--help", "-x", "", "   "])
    def test_non_value_is_rejected(self, bad):
        """`dl --devcontainer --help` must not become .devcontainer/--help/...."""
        with pytest.raises(ValueError):
            extract_devcontainer_flag(["repo", "--devcontainer", bad])

    def test_scanning_stops_at_the_shell_command_separator(self):
        """Args after `--` are the workspace command and must survive verbatim.

        `dl ws -- pytest --devcontainer sim` previously had its arguments eaten
        and silently triggered a rebuild against a different config.
        """
        args, selection = extract_devcontainer_flag(
            ["ws", "--", "pytest", "--devcontainer", "sim", "-k", "x"]
        )
        assert args == ["ws", "--", "pytest", "--devcontainer", "sim", "-k", "x"]
        assert selection is None

    def test_flag_before_the_separator_still_applies(self):
        args, selection = extract_devcontainer_flag(
            ["ws", "--devcontainer", "robot", "--", "echo", "--devcontainer", "hi"]
        )
        assert args == ["ws", "--", "echo", "--devcontainer", "hi"]
        assert selection == ".devcontainer/robot/devcontainer.json"

    @patch("devlaunch.dl.get_workspace_ids", return_value=["myws"])
    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_selection_reaches_devpod_through_main(
        self, mock_run, _mock_ctx, _mock_state, _mock_ssh, _mock_ids
    ):
        """The main() wiring is pinned, not just workspace_up's signature."""
        mock_run.return_value = MagicMock(returncode=0, stdout="", stderr="")
        with patch.object(sys, "argv", ["dl", "myws", "--devcontainer", "sim"]):
            main()
        up_calls = [c for c in mock_run.call_args_list if c[0][0][:1] == ["up"]]
        assert up_calls, "expected a devpod up call"
        up_args = up_calls[0][0][0]
        assert up_args[up_args.index("--devcontainer-path") + 1] == (
            ".devcontainer/sim/devcontainer.json"
        )

    @patch("devlaunch.dl.get_workspace_ids", return_value=["myws"])
    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.get_workspace_state", return_value="Running")
    @patch("devlaunch.dl.workspace_up")
    def test_ignored_on_a_running_workspace_warns(
        self, mock_up, _mock_state, _mock_ssh, _mock_ids, caplog
    ):
        """Fast-attach skips workspace_up entirely, so the flag does nothing."""
        with patch.object(sys, "argv", ["dl", "myws", "--devcontainer", "sim"]):
            main()
        mock_up.assert_not_called()
        assert "Ignoring --devcontainer" in caplog.text

    @patch("devlaunch.dl.get_workspace_ids", return_value=["myws"])
    @patch("devlaunch.dl.workspace_stop", return_value=0)
    def test_ignored_on_stop_warns(self, _mock_stop, _mock_ids, caplog):
        with patch.object(sys, "argv", ["dl", "myws", "--devcontainer", "sim", "stop"]):
            main()
        assert "Ignoring --devcontainer" in caplog.text


class TestWorkspaceOperations:
    """Tests for workspace operation functions."""

    @patch("devlaunch.dl.run_devpod")
    def test_workspace_stop(self, mock_run):
        """Test workspace_stop calls devpod stop."""
        mock_run.return_value = MagicMock(returncode=0)
        result = workspace_stop("myworkspace")
        mock_run.assert_called_once_with(["stop", "myworkspace"])
        assert result == 0

    @patch("devlaunch.dl.run_devpod")
    def test_workspace_delete(self, mock_run):
        """Test workspace_delete calls devpod delete."""
        mock_run.return_value = MagicMock(returncode=0)
        result = workspace_delete("myworkspace")
        mock_run.assert_called_once_with(["delete", "myworkspace"])
        assert result == 0

    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.run_devpod")
    def test_workspace_delete_keeps_clone_when_devpod_fails(self, mock_run, mock_mgr):
        """A failed `devpod delete` must leave the clone alone.

        devpod re-parses the workspace's devcontainer.json to tear the container
        down, so deletion fails if that file moved. Removing the clone anyway
        strands the workspace: devpod can then never find the config to retry.
        """
        mock_run.return_value = MagicMock(returncode=1)
        result = workspace_delete("myworkspace")
        assert result == 1
        mock_mgr.return_value.remove_workspace_by_id.assert_not_called()

    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.run_devpod")
    def test_workspace_delete_removes_clone_on_success(self, mock_run, mock_mgr):
        mock_run.return_value = MagicMock(returncode=0)
        mock_mgr.return_value.remove_workspace_by_id.return_value = True
        assert workspace_delete("myworkspace") == 0
        mock_mgr.return_value.remove_workspace_by_id.assert_called_once_with("myworkspace")


class TestPrintFunctions:
    """Tests for print functions."""

    def test_print_help(self, capsys):
        """Test print_help outputs help text."""
        print_help()
        captured = capsys.readouterr()
        assert "dl - DevLaunch CLI" in captured.out
        assert "Usage:" in captured.out
        assert "--ls" in captured.out

    @patch("devlaunch.dl.list_workspaces")
    def test_print_workspaces(self, mock_list, capsys):
        """Test print_workspaces outputs workspace table."""
        mock_list.return_value = [
            Workspace("ws1", "local", "/path/to/ws1", "2024-01-01", "docker", "vscode"),
        ]
        print_workspaces()
        captured = capsys.readouterr()
        assert "ws1" in captured.out

    @patch("devlaunch.dl.list_workspaces")
    def test_print_workspaces_empty(self, mock_list, capsys):
        """Test print_workspaces with no workspaces."""
        mock_list.return_value = []
        print_workspaces()
        captured = capsys.readouterr()
        assert "No workspaces found" in captured.out


class TestMainCLI:
    """Tests for main() CLI entry point."""

    def test_main_help_flag(self, capsys):
        """Test --help flag shows help."""
        with patch.object(sys, "argv", ["dl", "--help"]):
            result = main()
        assert result == 0
        captured = capsys.readouterr()
        assert "dl - DevLaunch CLI" in captured.out

    def test_main_h_flag(self, capsys):
        """Test -h flag shows help."""
        with patch.object(sys, "argv", ["dl", "-h"]):
            result = main()
        assert result == 0
        captured = capsys.readouterr()
        assert "dl - DevLaunch CLI" in captured.out

    def test_main_version_flag(self, capsys):
        """Test --version flag shows version."""
        with patch.object(sys, "argv", ["dl", "--version"]):
            result = main()
        assert result == 0
        captured = capsys.readouterr()
        assert "dl " in captured.out

    @patch("devlaunch.dl.list_workspaces")
    def test_main_ls_flag(self, mock_list, capsys):
        """Test --ls flag lists workspaces."""
        mock_list.return_value = []
        with patch.object(sys, "argv", ["dl", "--ls"]):
            result = main()
        assert result == 0
        captured = capsys.readouterr()
        assert "No workspaces found" in captured.out

    @patch("devlaunch.dl.read_completion_cache")
    def test_main_repos_flag(self, mock_cache, capsys):
        """Test --repos flag outputs repos."""
        mock_cache.return_value = {"repos": ["owner/repo1", "owner/repo2"]}
        with patch.object(sys, "argv", ["dl", "--repos"]):
            result = main()
        assert result == 0
        captured = capsys.readouterr()
        assert "owner/repo1" in captured.out

    @patch("devlaunch.dl.update_completion_cache")
    def test_main_update_cache_flag(self, mock_update):
        """Test --update-cache flag updates cache."""
        mock_update.return_value = {}
        with patch.object(sys, "argv", ["dl", "--update-cache"]):
            result = main()
        assert result == 0
        mock_update.assert_called_once()

    @patch("devlaunch.dl.read_completion_cache")
    def test_main_completion_data_flag(self, mock_cache, capsys):
        """Test --completion-data flag outputs JSON."""
        mock_cache.return_value = {"workspaces": ["ws1"], "repos": [], "owners": []}
        with patch.object(sys, "argv", ["dl", "--completion-data"]):
            result = main()
        assert result == 0
        captured = capsys.readouterr()
        data = json.loads(captured.out)
        assert "workspaces" in data

    @patch("devlaunch.dl.update_completion_cache")
    @patch("devlaunch.dl.install_completions")
    def test_main_install_flag(self, mock_install, mock_update):
        """Test --install flag installs completions."""
        mock_install.return_value = 0
        mock_update.return_value = {}
        with patch.object(sys, "argv", ["dl", "--install"]):
            result = main()
        assert result == 0
        mock_install.assert_called_once()

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_stop")
    def test_main_workspace_stop(self, mock_stop, mock_ids):
        """Test workspace stop command."""
        mock_ids.return_value = ["myws"]
        mock_stop.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "stop"]):
            result = main()
        assert result == 0
        mock_stop.assert_called_once_with("myws")

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_delete")
    def test_main_workspace_rm(self, mock_delete, mock_ids):
        """Test workspace rm command."""
        mock_ids.return_value = ["myws"]
        mock_delete.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "rm"]):
            result = main()
        assert result == 0
        mock_delete.assert_called_once_with("myws")

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_delete")
    def test_main_workspace_prune(self, mock_delete, mock_ids):
        """Test workspace prune command (alias for rm)."""
        mock_ids.return_value = ["myws"]
        mock_delete.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "prune"]):
            result = main()
        assert result == 0
        mock_delete.assert_called_once()

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    def test_main_workspace_code(self, mock_up, mock_ids):
        """Test workspace code command."""
        mock_ids.return_value = ["myws"]
        mock_up.return_value = MagicMock(returncode=0)
        with patch.object(sys, "argv", ["dl", "myws", "code"]):
            result = main()
        assert result == 0
        mock_up.assert_called_once_with(
            "myws", ide="vscode", workspace_id=None, workspace_identity="myws", devcontainer=None
        )

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_main_workspace_recreate(self, mock_ssh, mock_up, mock_ids):
        """Test workspace recreate command."""
        mock_ids.return_value = ["myws"]
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "recreate"]):
            result = main()
        assert result == 0
        mock_up.assert_called_once_with(
            "myws", recreate=True, workspace_id=None, workspace_identity="myws", devcontainer=None
        )

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_stop")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_main_workspace_restart(self, mock_ssh, mock_up, mock_stop, mock_ids):
        """Test workspace restart command."""
        mock_ids.return_value = ["myws"]
        mock_stop.return_value = 0
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "restart"]):
            result = main()
        assert result == 0
        mock_stop.assert_called_once()
        mock_up.assert_called_once_with(
            "myws", workspace_id=None, workspace_identity="myws", devcontainer=None
        )

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_main_workspace_reset(self, mock_ssh, mock_up, mock_ids):
        """Test workspace reset command."""
        mock_ids.return_value = ["myws"]
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "reset"]):
            result = main()
        assert result == 0
        mock_up.assert_called_once_with(
            "myws", reset=True, workspace_id=None, workspace_identity="myws", devcontainer=None
        )

    @patch("devlaunch.dl.get_workspace_ids")
    def test_main_unknown_command_error(self, mock_ids, caplog):
        """Test unknown subcommand returns error."""
        mock_ids.return_value = ["myws"]
        with patch.object(sys, "argv", ["dl", "myws", "badcmd"]):
            result = main()
        assert result == 1
        assert "Unknown command" in caplog.text

    @patch("devlaunch.dl.get_workspace_ids")
    def test_main_invalid_workspace_error(self, mock_ids, caplog):
        """Test invalid workspace spec returns error."""
        mock_ids.return_value = []
        with patch.object(sys, "argv", ["dl", "nonexistent"]):
            result = main()
        assert result == 1
        assert "Unknown workspace" in caplog.text

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.setup_hostname")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_workspace_shell_command(self, _cache, _hostname, mock_ssh, mock_up, mock_ids):
        """Test running shell command with -- separator."""
        mock_ids.return_value = ["myws"]
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "--", "echo", "hello"]):
            result = main()
        assert result == 0
        mock_ssh.assert_called_once_with("myws", "echo hello")

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.setup_hostname")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_workspace_default(self, _cache, _hostname, mock_ssh, mock_up, mock_ids):
        """Test default workspace start and attach."""
        mock_ids.return_value = ["myws"]
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws"]):
            result = main()
        assert result == 0
        mock_up.assert_called_once()
        mock_ssh.assert_called_once_with("myws", None)

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.setup_hostname")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_new_workspace_from_repo(
        self, _cache, _hostname, mock_ssh, mock_up, mock_clone_mgr, mock_ids
    ):
        """Test creating workspace from owner/repo resolves default branch."""
        mock_ids.return_value = []  # Not existing
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.get_default_branch.return_value = "main"
        mock_mgr.ensure_workspace.return_value = pathlib.Path("/tmp/ws/repo-main")
        mock_clone_mgr.return_value = mock_mgr
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 0
        # Should resolve default branch
        mock_mgr.repo_manager.get_default_branch.assert_called_once_with("owner", "repo")
        # Should ensure branch exists
        mock_mgr.ensure_branch.assert_called_once_with("owner", "repo", "main")
        # Workspace ID includes resolved branch
        mock_mgr.ensure_workspace.assert_called_once_with(
            "owner", "repo", "main", "git@github.com:owner/repo.git", "repo-main"
        )
        mock_up.assert_called_once_with(
            "/tmp/ws/repo-main",
            workspace_id="repo-main",
            workspace_identity="repo-main",
            devcontainer=None,
        )
        mock_ssh.assert_called_once_with("repo-main", None)

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_new_workspace_from_repo_with_branch(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_ids
    ):
        """Test creating workspace from owner/repo@branch uses ensure_branch."""
        mock_ids.return_value = []  # Not existing
        mock_mgr = MagicMock()
        mock_mgr.ensure_workspace.return_value = pathlib.Path("/tmp/ws/repo-main")
        mock_clone_mgr.return_value = mock_mgr
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 0
        # Should NOT resolve default branch (branch was specified)
        mock_mgr.repo_manager.get_default_branch.assert_not_called()
        # Should ensure branch exists via clone_mgr
        mock_mgr.ensure_branch.assert_called_once_with("owner", "repo", "main")
        mock_mgr.ensure_workspace.assert_called_once_with(
            "owner", "repo", "main", "git@github.com:owner/repo.git", "repo-main"
        )
        mock_up.assert_called_once_with(
            "/tmp/ws/repo-main",
            workspace_id="repo-main",
            workspace_identity="repo-main",
            devcontainer=None,
        )

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_new_workspace_creates_branch(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_ids
    ):
        """Test creating workspace from owner/repo@newbranch creates the branch."""
        mock_ids.return_value = []  # Not existing
        mock_mgr = MagicMock()
        mock_mgr.ensure_workspace.return_value = pathlib.Path("/tmp/ws/repo-newbranch")
        mock_clone_mgr.return_value = mock_mgr
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@newbranch"]):
            result = main()
        assert result == 0
        mock_mgr.ensure_branch.assert_called_once_with("owner", "repo", "newbranch")
        mock_mgr.ensure_workspace.assert_called_once_with(
            "owner", "repo", "newbranch", "git@github.com:owner/repo.git", "repo-newbranch"
        )

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_main_branch_creation_fails(self, mock_clone_mgr, mock_ids):
        """Test error when branch ensure fails."""
        mock_ids.return_value = []  # Not existing
        mock_mgr = MagicMock()
        mock_mgr.ensure_branch.side_effect = RuntimeError("push failed")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@newbranch"]):
            result = main()
        assert result == 1
        mock_mgr.ensure_branch.assert_called_once_with("owner", "repo", "newbranch")

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_main_clone_fails_no_branch(self, mock_clone_mgr, mock_ids):
        """Test error when ensure_repo fails (no branch specified, triggers clone for default branch)."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.ensure_repo.side_effect = RuntimeError("repository not found")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 1
        mock_mgr.repo_manager.ensure_repo.assert_called_once()

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_main_clone_fails_with_branch(self, mock_clone_mgr, mock_ids):
        """Test error when ensure_repo fails (branch specified, workspace not existing)."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.ensure_repo.side_effect = OSError("network unreachable")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@newbranch"]):
            result = main()
        assert result == 1
        mock_mgr.repo_manager.ensure_repo.assert_called_once()

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_feature_branch_with_slash(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_ids
    ):
        """Test creating workspace with feature/branch style branch name."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.ensure_workspace.return_value = pathlib.Path("/tmp/ws/repo-feature-my-feature")
        mock_clone_mgr.return_value = mock_mgr
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@feature/my-feature"]):
            result = main()
        assert result == 0
        mock_mgr.ensure_branch.assert_called_once_with("owner", "repo", "feature/my-feature")
        mock_mgr.ensure_workspace.assert_called_once_with(
            "owner",
            "repo",
            "feature/my-feature",
            "git@github.com:owner/repo.git",
            "repo-feature-my-feature",
        )

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_existing_workspace_no_clone_manager(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_ids
    ):
        """Test existing workspace doesn't use clone manager."""
        mock_ids.return_value = ["myworkspace"]  # Existing
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myworkspace"]):
            result = main()
        assert result == 0
        mock_clone_mgr.assert_not_called()

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_repo_without_branch_resolves_default(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_ids
    ):
        """Test owner/repo without @branch resolves default branch."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.get_default_branch.return_value = "main"
        mock_mgr.ensure_workspace.return_value = pathlib.Path("/tmp/ws/repo-main")
        mock_clone_mgr.return_value = mock_mgr
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 0
        # Should resolve default branch and use it
        mock_mgr.repo_manager.get_default_branch.assert_called_once_with("owner", "repo")
        mock_mgr.ensure_branch.assert_called_once_with("owner", "repo", "main")


class TestPurgeFunctionality:
    """Tests for purge functionality."""

    @patch("devlaunch.dl.run_devpod")
    @patch("devlaunch.dl.list_workspaces")
    def test_purge_deletes_all_workspaces(self, mock_list, mock_run):
        """Test purge_all_data deletes all workspaces."""
        from devlaunch.dl import purge_all_data

        mock_list.return_value = [
            Workspace("ws1", "local", "/path", "", "docker", "vscode"),
            Workspace("ws2", "git", "github.com/o/r", "", "docker", "none"),
        ]
        mock_run.return_value = MagicMock(returncode=0)

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl._get_cache_dir", return_value=pathlib.Path(tmpdir)):
                result = purge_all_data()

        assert result == 0
        # Should have called delete for each workspace
        assert mock_run.call_count == 2
        delete_calls = list(mock_run.call_args_list)
        assert delete_calls[0][0][0] == ["delete", "ws1", "--force"]
        assert delete_calls[1][0][0] == ["delete", "ws2", "--force"]

    @patch("devlaunch.dl.run_devpod")
    @patch("devlaunch.dl.list_workspaces")
    def test_purge_removes_cache_dir(self, mock_list, mock_run):
        """Test purge_all_data removes the cache directory."""
        from devlaunch.dl import purge_all_data

        mock_list.return_value = []
        mock_run.return_value = MagicMock(returncode=0)

        with tempfile.TemporaryDirectory() as tmpdir:
            cache_dir = pathlib.Path(tmpdir) / "devlaunch"
            cache_dir.mkdir()
            test_file = cache_dir / "test.txt"
            test_file.write_text("test")
            assert test_file.exists()

            with patch("devlaunch.dl._get_cache_dir", return_value=cache_dir):
                result = purge_all_data()

            assert result == 0
            assert not cache_dir.exists()

    @patch("devlaunch.dl.run_devpod")
    @patch("devlaunch.dl.list_workspaces")
    def test_purge_handles_workspace_delete_failure(self, mock_list, mock_run, caplog):
        """Test purge continues even if workspace delete fails."""
        from devlaunch.dl import purge_all_data

        mock_list.return_value = [
            Workspace("ws1", "local", "/path", "", "docker", "vscode"),
            Workspace("ws2", "git", "github.com/o/r", "", "docker", "none"),
        ]
        # First delete fails, second succeeds
        mock_run.side_effect = [
            MagicMock(returncode=1, stderr="error"),
            MagicMock(returncode=0),
        ]

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl._get_cache_dir", return_value=pathlib.Path(tmpdir)):
                result = purge_all_data()

        # Should still return 0 (cache cleanup succeeded)
        assert result == 0
        # Should have tried to delete both workspaces
        assert mock_run.call_count == 2
        # Should have logged warning for failed delete
        assert "Failed to delete workspace ws1" in caplog.text

    @patch("devlaunch.dl.list_workspaces")
    def test_main_purge_with_yes_flag(self, mock_list, capsys):
        """Test --purge -y skips confirmation."""
        mock_list.return_value = []

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl._get_cache_dir", return_value=pathlib.Path(tmpdir)):
                with patch.object(sys, "argv", ["dl", "--purge", "-y"]):
                    result = main()

        assert result == 0
        captured = capsys.readouterr()
        assert "No data to purge" in captured.out or "Removed" in captured.out


class TestHostnameAndWorkdir:
    """Tests for hostname setup and container workdir."""

    @patch("devlaunch.dl.get_workspace_ids", return_value=["myws"])
    @patch("devlaunch.dl.get_workspace_state", return_value="Running")
    @patch("devlaunch.dl.setup_hostname")
    @patch("devlaunch.dl.run_devpod")
    def test_attach_does_not_override_workdir(self, mock_run, _mock_host, _mock_state, _mock_ids):
        """devpod ssh already starts in devcontainer.json's workspaceFolder.

        Asserted through main(), because that is where the guessed
        /workspaces/<id> used to be built — and devpod silently drops the session
        in $HOME for any project with a custom workspaceFolder.
        """
        mock_run.return_value = MagicMock(returncode=0, stdout="", stderr="")
        with patch.object(sys, "argv", ["dl", "myws"]):
            main()
        ssh_calls = [c for c in mock_run.call_args_list if c[0][0][:1] == ["ssh"]]
        assert ssh_calls, "expected a devpod ssh call"
        assert "--workdir" not in ssh_calls[0][0][0]

    @patch("devlaunch.dl.run_devpod")
    def test_setup_hostname_success(self, mock_run):
        """Test setup_hostname sets container hostname via SSH."""
        mock_run.return_value = MagicMock(returncode=0)
        result = setup_hostname("my-workspace")
        assert result is True
        mock_run.assert_called_once_with(
            ["ssh", "my-workspace", "--command", "sudo hostname my-workspace"],
            capture=True,
        )

    @patch("devlaunch.dl.run_devpod")
    def test_setup_hostname_failure_is_silent(self, mock_run):
        """Test setup_hostname returns False on failure (best-effort, no output)."""
        mock_run.return_value = MagicMock(returncode=1)
        result = setup_hostname("my-workspace")
        assert result is False


class TestWorkspaceSsh:
    """Tests for workspace_ssh."""

    @patch("devlaunch.dl.run_devpod")
    def test_workspace_ssh_basic(self, mock_run):
        """Test basic SSH."""
        mock_run.return_value = MagicMock(returncode=0)
        result = workspace_ssh("myws")
        assert result == 0
        mock_run.assert_called_once_with(["ssh", "myws"], env=None)

    @patch("devlaunch.dl.run_devpod")
    def test_workspace_ssh_with_command(self, mock_run):
        """Test SSH with command."""
        mock_run.return_value = MagicMock(returncode=0)
        result = workspace_ssh("myws", command="echo hello")
        assert result == 0
        mock_run.assert_called_once_with(["ssh", "myws", "--command", "echo hello"], env=None)

    @patch("devlaunch.dl.run_devpod")
    def test_workspace_ssh_with_workdir(self, mock_run):
        """Test SSH with workdir."""
        mock_run.return_value = MagicMock(returncode=0)
        result = workspace_ssh("myws", workdir="/some/path")
        assert result == 0
        mock_run.assert_called_once_with(["ssh", "myws", "--workdir", "/some/path"], env=None)

    @patch("devlaunch.dl.run_devpod")
    def test_workspace_ssh_with_workdir_and_command(self, mock_run):
        """Test SSH with both workdir and command."""
        mock_run.return_value = MagicMock(returncode=0)
        result = workspace_ssh("myws", command="make test", workdir="/workspaces/myws")
        assert result == 0
        mock_run.assert_called_once_with(
            ["ssh", "myws", "--workdir", "/workspaces/myws", "--command", "make test"], env=None
        )


class TestGetWorkspaceState:
    """Tests for get_workspace_state helper."""

    @patch("devlaunch.dl.run_devpod")
    def test_running_state(self, mock_run):
        """Test returns 'Running' for a running workspace."""
        mock_run.return_value = MagicMock(returncode=0, stdout=json.dumps({"state": "Running"}))
        assert get_workspace_state("myws") == "Running"
        mock_run.assert_called_once_with(["status", "myws", "--output", "json"], capture=True)

    @patch("devlaunch.dl.run_devpod")
    def test_stopped_state(self, mock_run):
        """Test returns 'Stopped' for a stopped workspace."""
        mock_run.return_value = MagicMock(returncode=0, stdout=json.dumps({"state": "Stopped"}))
        assert get_workspace_state("myws") == "Stopped"

    @patch("devlaunch.dl.run_devpod")
    def test_command_failure(self, mock_run):
        """Test returns None when devpod command fails."""
        mock_run.return_value = MagicMock(returncode=1, stdout="")
        assert get_workspace_state("myws") is None

    @patch("devlaunch.dl.run_devpod")
    def test_invalid_json(self, mock_run):
        """Test returns None for invalid JSON output."""
        mock_run.return_value = MagicMock(returncode=0, stdout="not json")
        assert get_workspace_state("myws") is None

    @patch("devlaunch.dl.run_devpod")
    def test_missing_state_key(self, mock_run):
        """Test returns None when state key is missing."""
        mock_run.return_value = MagicMock(returncode=0, stdout=json.dumps({"id": "myws"}))
        assert get_workspace_state("myws") is None


class TestFastAttach:
    """Tests for fast-attach optimization (skipping clone manager and workspace_up)."""

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_git_spec_existing_workspace_skips_clone_manager(
        self, _cache, mock_ssh, mock_up, mock_state, mock_clone_mgr, mock_ids
    ):
        """Test git spec with existing workspace skips clone manager."""
        mock_ids.return_value = ["repo-main"]  # Workspace already exists
        mock_state.return_value = "Stopped"  # Not running, so workspace_up still called
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 0
        # Clone manager created but ensure_branch/ensure_workspace NOT called (fast path)
        mock_mgr = mock_clone_mgr.return_value
        mock_mgr.ensure_branch.assert_not_called()
        mock_mgr.ensure_workspace.assert_not_called()
        # workspace_up called with just the ID (no local path), no custom --id
        mock_up.assert_called_once_with(
            "repo-main", workspace_id=None, workspace_identity="repo-main", devcontainer=None
        )

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.setup_hostname")
    @patch("devlaunch.dl.update_cache_background")
    def test_git_spec_running_workspace_skips_workspace_up(
        self, _cache, _hostname, mock_ssh, mock_up, mock_state, mock_clone_mgr, mock_ids
    ):
        """Test git spec with Running workspace skips workspace_up()."""
        mock_ids.return_value = ["repo-main"]  # Workspace exists
        mock_state.return_value = "Running"  # Already running
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 0
        # Clone manager created but ensure_branch/ensure_workspace NOT called
        mock_mgr = mock_clone_mgr.return_value
        mock_mgr.ensure_branch.assert_not_called()
        mock_mgr.ensure_workspace.assert_not_called()
        # workspace_up should NOT be called (fast-attach)
        mock_up.assert_not_called()
        # Should SSH in to attach
        mock_ssh.assert_called_once_with("repo-main", None)

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_git_spec_stopped_workspace_calls_workspace_up(
        self, _cache, mock_ssh, mock_up, mock_state, mock_clone_mgr, mock_ids
    ):
        """Test git spec with Stopped workspace still calls workspace_up() with ID only."""
        mock_ids.return_value = ["repo-main"]  # Workspace exists
        mock_state.return_value = "Stopped"  # Not running
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 0
        # Clone manager created but ensure_branch/ensure_workspace NOT called (fast path)
        mock_mgr = mock_clone_mgr.return_value
        mock_mgr.ensure_branch.assert_not_called()
        mock_mgr.ensure_workspace.assert_not_called()
        # workspace_up IS called (need to start it)
        mock_up.assert_called_once_with(
            "repo-main", workspace_id=None, workspace_identity="repo-main", devcontainer=None
        )

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.setup_hostname")
    @patch("devlaunch.dl.update_cache_background")
    def test_existing_id_running_skips_workspace_up(
        self, _cache, _hostname, mock_ssh, mock_up, mock_state, mock_ids
    ):
        """Test existing raw workspace ID with Running state skips workspace_up()."""
        mock_ids.return_value = ["python-template-ws3"]
        mock_state.return_value = "Running"
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "python-template-ws3"]):
            result = main()
        assert result == 0
        # workspace_up should NOT be called
        mock_up.assert_not_called()
        # Should SSH in to attach
        mock_ssh.assert_called_once_with("python-template-ws3", None)

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.setup_hostname")
    @patch("devlaunch.dl.update_cache_background")
    def test_git_spec_no_branch_existing_workspace_skips_clone_manager(
        self, _cache, _hostname, mock_ssh, mock_up, mock_state, mock_clone_mgr, mock_ids
    ):
        """Test owner/repo (no branch) with existing workspace skips full clone pipeline."""
        mock_ids.return_value = ["repo-main"]  # Workspace exists
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.get_default_branch.return_value = "main"
        mock_clone_mgr.return_value = mock_mgr
        mock_state.return_value = "Running"
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 0
        # Clone manager called only for ensure_repo + get_default_branch (to resolve branch)
        mock_mgr.repo_manager.ensure_repo.assert_called_once()
        mock_mgr.repo_manager.get_default_branch.assert_called_once()
        # But ensure_branch and ensure_workspace NOT called (fast path)
        mock_mgr.ensure_branch.assert_not_called()
        mock_mgr.ensure_workspace.assert_not_called()
        # workspace_up NOT called (Running)
        mock_up.assert_not_called()


class TestGetContextOptions:
    """Tests for get_context_options()."""

    @patch("devlaunch.dl.run_devpod")
    def test_returns_option_values(self, mock_devpod):
        """Returns dict of option name -> value."""
        mock_devpod.return_value = MagicMock(
            returncode=0,
            stdout='{"DOTFILES_URL": {"value": "https://github.com/user/dots"}, "DOTFILES_SCRIPT": {"value": "install.sh"}}',
        )
        from devlaunch.dl import get_context_options

        result = get_context_options()
        assert result == {
            "DOTFILES_URL": "https://github.com/user/dots",
            "DOTFILES_SCRIPT": "install.sh",
        }

    @patch("devlaunch.dl.run_devpod")
    def test_skips_empty_values(self, mock_devpod):
        """Options with empty or missing values are excluded."""
        mock_devpod.return_value = MagicMock(
            returncode=0,
            stdout='{"DOTFILES_URL": {"value": ""}, "OTHER": {"value": "x"}}',
        )
        from devlaunch.dl import get_context_options

        result = get_context_options()
        assert result == {"OTHER": "x"}

    @patch("devlaunch.dl.run_devpod")
    def test_returns_empty_on_failure(self, mock_devpod):
        """Returns empty dict when devpod command fails."""
        mock_devpod.return_value = MagicMock(returncode=1, stdout="")
        from devlaunch.dl import get_context_options

        assert get_context_options() == {}

    @patch("devlaunch.dl.run_devpod")
    def test_returns_empty_on_invalid_json(self, mock_devpod):
        """Returns empty dict when output is not valid JSON."""
        mock_devpod.return_value = MagicMock(returncode=0, stdout="not json")
        from devlaunch.dl import get_context_options

        assert get_context_options() == {}


class TestWorkspaceUpDotfiles:
    """Tests for dotfiles passthrough in workspace_up."""

    @patch("devlaunch.dl.get_context_options")
    @patch("devlaunch.dl.run_devpod")
    def test_passes_dotfiles_args(self, mock_devpod, mock_ctx):
        """workspace_up passes dotfiles URL and script from context."""
        mock_ctx.return_value = {
            "DOTFILES_URL": "https://github.com/user/dots",
            "DOTFILES_SCRIPT": "install.sh",
        }
        mock_devpod.return_value = MagicMock(returncode=0)

        workspace_up("myws")
        args = mock_devpod.call_args[0][0]
        assert "--dotfiles" in args
        assert "https://github.com/user/dots" in args
        assert "--dotfiles-script" in args
        assert "install.sh" in args

    @patch("devlaunch.dl.get_context_options")
    @patch("devlaunch.dl.run_devpod")
    def test_no_dotfiles_when_empty(self, mock_devpod, mock_ctx):
        """workspace_up omits dotfiles args when context has none."""
        mock_ctx.return_value = {}
        mock_devpod.return_value = MagicMock(returncode=0)

        workspace_up("myws")
        args = mock_devpod.call_args[0][0]
        assert "--dotfiles" not in args
        assert "--dotfiles-script" not in args


class TestCLIErrorMessages:
    """Comprehensive tests for CLI error messages and exit codes.

    Ensures every error path produces a single, clean error message
    (no duplicates) and returns exit code 1.
    """

    # --- Invalid workspace spec errors ---

    @patch("devlaunch.dl.get_workspace_ids")
    def test_invalid_spec_bare_word(self, mock_ids, caplog):
        """Bare word that isn't an existing workspace returns error."""
        mock_ids.return_value = ["real-ws"]
        with patch.object(sys, "argv", ["dl", "nonexistent"]):
            result = main()
        assert result == 1
        assert "Unknown workspace 'nonexistent'" in caplog.text

    @patch("devlaunch.dl.get_workspace_ids")
    def test_invalid_spec_partial_match(self, mock_ids, caplog):
        """Partial workspace name doesn't match."""
        mock_ids.return_value = ["my-workspace"]
        with patch.object(sys, "argv", ["dl", "my-work"]):
            result = main()
        assert result == 1
        assert "Unknown workspace" in caplog.text

    @patch("devlaunch.dl.get_workspace_ids")
    def test_invalid_spec_suggests_alternatives(self, mock_ids, caplog):
        """Error message suggests using --ls or owner/repo."""
        mock_ids.return_value = []
        with patch.object(sys, "argv", ["dl", "badname"]):
            result = main()
        assert result == 1
        assert "dl --ls" in caplog.text
        assert "owner/repo" in caplog.text

    # --- Unknown subcommand errors ---

    @patch("devlaunch.dl.get_workspace_ids")
    def test_unknown_subcommand_message(self, mock_ids, caplog):
        """Unknown subcommand produces helpful error with -- hint."""
        mock_ids.return_value = ["myws"]
        with patch.object(sys, "argv", ["dl", "myws", "badcmd"]):
            result = main()
        assert result == 1
        assert "Unknown command 'badcmd'" in caplog.text
        assert "dl myws -- badcmd" in caplog.text

    @patch("devlaunch.dl.get_workspace_ids")
    def test_unknown_subcommand_with_git_spec(self, mock_ids, caplog):
        """Unknown subcommand with owner/repo spec returns error."""
        mock_ids.return_value = ["repo-main"]
        with patch.object(sys, "argv", ["dl", "repo-main", "deploy"]):
            result = main()
        assert result == 1
        assert "Unknown command 'deploy'" in caplog.text

    # --- Clone failure errors (no duplicate messages) ---

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_clone_fail_no_branch_single_error(self, mock_clone_mgr, mock_ids, caplog):
        """Clone failure (no branch) produces exactly one error line."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.ensure_repo.side_effect = RuntimeError(
            "Failed to clone repository: repository not found"
        )
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 1
        # Should contain the repo name for context
        assert "owner/repo" in caplog.text
        # Should NOT have "Failed to clone" repeated twice in the same message
        error_records = [r for r in caplog.records if r.levelname == "ERROR"]
        assert len(error_records) == 1
        assert error_records[0].message.startswith("Repository 'owner/repo':")

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_clone_fail_with_branch_single_error(self, mock_clone_mgr, mock_ids, caplog):
        """Clone failure (with branch) produces exactly one error line."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.ensure_repo.side_effect = OSError("network unreachable")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@feature"]):
            result = main()
        assert result == 1
        error_records = [r for r in caplog.records if r.levelname == "ERROR"]
        assert len(error_records) == 1
        assert "owner/repo" in error_records[0].message

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_clone_fail_runtime_error_message(self, mock_clone_mgr, mock_ids, caplog):
        """RuntimeError from clone surfaces the original error text."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.ensure_repo.side_effect = RuntimeError(
            "Failed to clone repository: ERROR: Repository not found."
        )
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 1
        assert "Repository not found" in caplog.text

    # --- Branch ensure failure errors ---

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_branch_ensure_runtime_error(self, mock_clone_mgr, mock_ids, caplog):
        """Branch ensure RuntimeError logged with branch name."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.ensure_branch.side_effect = RuntimeError("push failed: permission denied")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@newbranch"]):
            result = main()
        assert result == 1
        assert "newbranch" in caplog.text
        assert "push failed" in caplog.text

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_branch_ensure_os_error(self, mock_clone_mgr, mock_ids, caplog):
        """Branch ensure OSError returns 1."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.ensure_branch.side_effect = OSError("git not found")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@develop"]):
            result = main()
        assert result == 1
        assert "develop" in caplog.text

    # --- Workspace prepare failure errors ---

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_ensure_workspace_runtime_error(self, mock_clone_mgr, mock_ids, caplog):
        """ensure_workspace RuntimeError returns 1 with message."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.ensure_workspace.side_effect = RuntimeError("worktree creation failed")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 1
        assert "Failed to prepare workspace" in caplog.text
        assert "worktree creation failed" in caplog.text

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_ensure_workspace_os_error(self, mock_clone_mgr, mock_ids, caplog):
        """ensure_workspace OSError returns 1."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.ensure_workspace.side_effect = OSError("disk full")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 1
        assert "disk full" in caplog.text

    # --- workspace_up failure errors ---

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_workspace_up_exception(self, _mock_ssh, mock_up, mock_ids, caplog):
        """workspace_up exception returns 1 with message."""
        mock_ids.return_value = ["myws"]
        mock_up.side_effect = RuntimeError("devpod crashed")
        with patch.object(sys, "argv", ["dl", "myws"]):
            result = main()
        assert result == 1
        assert "Failed to create workspace" in caplog.text

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_workspace_up_nonzero_exit(self, mock_ssh, mock_up, mock_ids):
        """workspace_up returning non-zero propagates exit code."""
        mock_ids.return_value = ["myws"]
        mock_up.return_value = MagicMock(returncode=2)
        with patch.object(sys, "argv", ["dl", "myws"]):
            result = main()
        assert result == 2
        mock_ssh.assert_not_called()

    # --- Subcommand failure propagation ---

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    def test_recreate_up_failure_propagates(self, mock_up, mock_ids):
        """recreate subcommand propagates workspace_up failure."""
        mock_ids.return_value = ["myws"]
        mock_up.return_value = MagicMock(returncode=3)
        with patch.object(sys, "argv", ["dl", "myws", "recreate"]):
            result = main()
        assert result == 3

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_stop")
    def test_restart_stop_failure_propagates(self, mock_stop, mock_ids):
        """restart subcommand propagates stop failure."""
        mock_ids.return_value = ["myws"]
        mock_stop.return_value = 1
        with patch.object(sys, "argv", ["dl", "myws", "restart"]):
            result = main()
        assert result == 1

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_stop")
    @patch("devlaunch.dl.workspace_up")
    def test_restart_up_failure_propagates(self, mock_up, mock_stop, mock_ids):
        """restart subcommand propagates workspace_up failure after successful stop."""
        mock_ids.return_value = ["myws"]
        mock_stop.return_value = 0
        mock_up.return_value = MagicMock(returncode=4)
        with patch.object(sys, "argv", ["dl", "myws", "restart"]):
            result = main()
        assert result == 4

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    def test_reset_up_failure_propagates(self, mock_up, mock_ids):
        """reset subcommand propagates workspace_up failure."""
        mock_ids.return_value = ["myws"]
        mock_up.return_value = MagicMock(returncode=5)
        with patch.object(sys, "argv", ["dl", "myws", "reset"]):
            result = main()
        assert result == 5

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    def test_code_up_failure_propagates(self, mock_up, mock_ids):
        """code subcommand propagates workspace_up failure."""
        mock_ids.return_value = ["myws"]
        mock_up.return_value = MagicMock(returncode=1)
        with patch.object(sys, "argv", ["dl", "myws", "code"]):
            result = main()
        assert result == 1

    # --- No duplicate error messages ---

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_clone_fail_no_duplicate_failed_to_clone(self, mock_clone_mgr, mock_ids, caplog):
        """Verify 'Failed to clone' doesn't appear twice in error output."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.ensure_repo.side_effect = RuntimeError(
            "Failed to clone repository: Cloning into bare repository...\nERROR: Repository not found."
        )
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 1
        # Count ERROR-level records - should be exactly 1
        error_records = [r for r in caplog.records if r.levelname == "ERROR"]
        assert len(error_records) == 1
        # The message should not wrap "Failed to clone" inside another "Failed to clone"
        msg = error_records[0].message
        assert not msg.startswith("Failed to clone")
        assert msg.startswith("Repository 'owner/repo':")

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_fetch_fail_no_duplicate_messages(self, mock_clone_mgr, mock_ids, caplog):
        """Verify fetch failure doesn't produce duplicate error messages."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.ensure_repo.side_effect = RuntimeError(
            "Failed to fetch repository: network timeout"
        )
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 1
        error_records = [r for r in caplog.records if r.levelname == "ERROR"]
        assert len(error_records) == 1

    # --- Edge cases ---

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_workspace_up_os_error(self, _mock_ssh, mock_up, mock_ids, caplog):
        """workspace_up OSError is caught and returns 1."""
        mock_ids.return_value = ["myws"]
        mock_up.side_effect = OSError("connection refused")
        with patch.object(sys, "argv", ["dl", "myws"]):
            result = main()
        assert result == 1
        assert "connection refused" in caplog.text

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl._get_clone_manager")
    def test_clone_fail_empty_error_message(self, mock_clone_mgr, mock_ids, caplog):
        """Clone failure with empty error still returns 1."""
        mock_ids.return_value = []
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.ensure_repo.side_effect = RuntimeError("")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 1
        error_records = [r for r in caplog.records if r.levelname == "ERROR"]
        assert len(error_records) == 1
