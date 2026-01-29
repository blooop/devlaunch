"""Tests for dl (DevLaunch CLI) functionality."""

import json
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
    create_remote_branch,
    ensure_remote_branch,
    main,
    print_help,
    print_workspaces,
    workspace_stop,
    workspace_delete,
    run_devpod,
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

    @patch("devlaunch.dl._get_git_work_dir")
    @patch("subprocess.run")
    def test_create_remote_branch_success(self, mock_run, mock_git_dir):
        """Test successful branch creation."""
        with tempfile.TemporaryDirectory() as tmpdir:
            mock_git_dir.return_value = pathlib.Path(tmpdir)
            mock_run.return_value = MagicMock(returncode=0)
            assert create_remote_branch("owner/repo", "newbranch") is True
            # Should call: git init (no .git exists), git fetch, git push
            assert mock_run.call_count == 3

    @patch("devlaunch.dl.remote_branch_exists")
    def test_ensure_branch_exists_already(self, mock_exists):
        """Test ensure returns True if branch exists."""
        mock_exists.return_value = True
        assert ensure_remote_branch("owner/repo", "main") is True

    @patch("devlaunch.dl.create_remote_branch")
    @patch("devlaunch.dl.remote_branch_exists")
    def test_ensure_branch_creates_new(self, mock_exists, mock_create):
        """Test ensure creates branch if doesn't exist."""
        mock_exists.return_value = False
        mock_create.return_value = True
        assert ensure_remote_branch("owner/repo", "newbranch") is True
        mock_create.assert_called_once_with("owner/repo", "newbranch")

    @patch("devlaunch.dl.create_remote_branch")
    @patch("devlaunch.dl.remote_branch_exists")
    def test_ensure_branch_create_fails(self, mock_exists, mock_create):
        """Test ensure returns False if branch creation fails."""
        mock_exists.return_value = False
        mock_create.return_value = False
        assert ensure_remote_branch("owner/repo", "newbranch") is False

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
        import subprocess

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

    @patch("devlaunch.dl._get_git_work_dir")
    @patch("subprocess.run")
    def test_create_remote_branch_push_fails(self, mock_run, mock_git_dir):
        """Test branch creation returns False on push failure."""
        with tempfile.TemporaryDirectory() as tmpdir:
            mock_git_dir.return_value = pathlib.Path(tmpdir)
            # git init succeeds, git fetch succeeds, git push fails
            mock_run.side_effect = [
                MagicMock(returncode=0),  # git init
                MagicMock(returncode=0),  # git fetch
                MagicMock(returncode=1, stderr="error: failed to push"),  # git push
            ]
            assert create_remote_branch("owner/repo", "newbranch") is False

    @patch("devlaunch.dl._get_git_work_dir")
    @patch("subprocess.run")
    def test_create_remote_branch_os_error(self, mock_run, mock_git_dir):
        """Test branch creation handles OSError."""
        with tempfile.TemporaryDirectory() as tmpdir:
            mock_git_dir.return_value = pathlib.Path(tmpdir)
            mock_run.side_effect = OSError("git not found")
            assert create_remote_branch("owner/repo", "newbranch") is False

    @patch("devlaunch.dl._get_git_work_dir")
    @patch("subprocess.run")
    def test_create_remote_branch_uses_cache_dir(self, mock_run, mock_git_dir):
        """Test branch creation uses cache directory for git operations."""
        with tempfile.TemporaryDirectory() as tmpdir:
            cache_dir = pathlib.Path(tmpdir)
            mock_git_dir.return_value = cache_dir
            mock_run.return_value = MagicMock(returncode=0)
            result = create_remote_branch("owner/repo", "newbranch")
            assert result is True
            # Should have called git init, git fetch, git push
            assert mock_run.call_count == 3
            # All calls should use the cache directory
            for call in mock_run.call_args_list:
                assert call[1]["cwd"] == cache_dir

    @patch("devlaunch.dl._get_git_work_dir")
    @patch("subprocess.run")
    def test_create_remote_branch_skips_init_if_exists(self, mock_run, mock_git_dir):
        """Test branch creation skips git init if .git already exists."""
        with tempfile.TemporaryDirectory() as tmpdir:
            cache_dir = pathlib.Path(tmpdir)
            # Create .git directory to simulate existing repo
            (cache_dir / ".git").mkdir()
            mock_git_dir.return_value = cache_dir
            mock_run.return_value = MagicMock(returncode=0)
            result = create_remote_branch("owner/repo", "newbranch")
            assert result is True
            # Should only call git fetch, git push (no init)
            assert mock_run.call_count == 2
            assert mock_run.call_args_list[0][0][0][0:2] == ["git", "fetch"]
            assert mock_run.call_args_list[1][0][0][0:2] == ["git", "push"]

    @patch("devlaunch.dl._get_git_work_dir")
    @patch("subprocess.run")
    def test_create_remote_branch_git_init_fails(self, mock_run, mock_git_dir):
        """Test branch creation fails gracefully if git init fails."""
        with tempfile.TemporaryDirectory() as tmpdir:
            mock_git_dir.return_value = pathlib.Path(tmpdir)
            mock_run.return_value = MagicMock(returncode=1, stderr="init failed")
            result = create_remote_branch("owner/repo", "newbranch")
            assert result is False
            # Should only call git init
            assert mock_run.call_count == 1

    @patch("devlaunch.dl._get_git_work_dir")
    @patch("subprocess.run")
    def test_create_remote_branch_fetch_fails(self, mock_run, mock_git_dir, caplog):
        """Test branch creation fails gracefully if git fetch fails."""
        with tempfile.TemporaryDirectory() as tmpdir:
            mock_git_dir.return_value = pathlib.Path(tmpdir)
            mock_run.side_effect = [
                MagicMock(returncode=0),  # git init
                MagicMock(returncode=1, stderr="fetch failed"),  # git fetch
            ]
            result = create_remote_branch("owner/repo", "newbranch")
            assert result is False
            assert mock_run.call_count == 2
            assert "Failed to fetch" in caplog.text

    @patch("devlaunch.dl._get_git_work_dir")
    @patch("subprocess.run")
    def test_create_remote_branch_ssh_auth_fails(self, mock_run, mock_git_dir, caplog):
        """Test branch creation gives helpful error when SSH auth fails."""
        with tempfile.TemporaryDirectory() as tmpdir:
            mock_git_dir.return_value = pathlib.Path(tmpdir)
            # git init succeeds, git fetch succeeds, git push fails with SSH error
            mock_run.side_effect = [
                MagicMock(returncode=0),  # git init
                MagicMock(returncode=0),  # git fetch
                MagicMock(returncode=128, stderr="git@github.com: Permission denied (publickey)."),
            ]
            result = create_remote_branch("owner/repo", "newbranch")
            assert result is False
            assert "SSH authentication failed" in caplog.text
            assert "configure SSH keys" in caplog.text

    @patch("devlaunch.dl._get_git_work_dir")
    @patch("subprocess.run")
    def test_create_remote_branch_uses_ssh_url(self, mock_run, mock_git_dir):
        """Test branch creation uses SSH URL for push."""
        with tempfile.TemporaryDirectory() as tmpdir:
            mock_git_dir.return_value = pathlib.Path(tmpdir)
            mock_run.return_value = MagicMock(returncode=0)
            create_remote_branch("owner/repo", "newbranch")
            # Check that git push (3rd call) was called with SSH URL
            push_call = mock_run.call_args_list[2]
            push_args = push_call[0][0]
            assert "git@github.com:owner/repo.git" in push_args


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

    def test_owner_repo_full_url_sanitized(self):
        """Test owner/repo generates full sanitized URL as workspace ID."""
        assert spec_to_workspace_id("blooop/devlaunch") == "github-com-blooop-devlaunch"

    def test_owner_repo_with_branch_uses_branch(self):
        """Test owner/repo@branch uses sanitized branch as workspace ID."""
        assert spec_to_workspace_id("blooop/devlaunch@main") == "main"

    def test_owner_repo_with_feature_branch(self):
        """Test owner/repo@feature/branch sanitizes branch name."""
        assert spec_to_workspace_id("owner/repo@feature/my-branch") == "feature-my-branch"

    def test_owner_repo_with_uppercase_branch(self):
        """Test branch name is lowercased."""
        assert spec_to_workspace_id("Owner/Repo@Feature/MyBranch") == "feature-mybranch"

    def test_github_url_sanitized(self):
        """Test github.com/owner/repo generates sanitized ID."""
        assert spec_to_workspace_id("github.com/loft-sh/devpod") == "github-com-loft-sh-devpod"

    def test_https_url_strips_protocol(self):
        """Test https URL strips protocol and sanitizes."""
        assert spec_to_workspace_id("https://github.com/owner/repo") == "github-com-owner-repo"

    def test_url_with_git_suffix_strips_it(self):
        """Test URL with .git suffix strips it."""
        assert spec_to_workspace_id("github.com/owner/repo.git") == "github-com-owner-repo"

    def test_underscore_removed_from_repo(self):
        """Test underscores are removed from repo-based workspace ID."""
        assert spec_to_workspace_id("blooop/test_renv") == "github-com-blooop-testrenv"

    def test_branch_allows_multiple_workspaces(self):
        """Test different branches get different workspace IDs."""
        assert spec_to_workspace_id("blooop/test_renv@nb12") == "nb12"
        assert spec_to_workspace_id("blooop/test_renv@nb14") == "nb14"
        # Different branches = different IDs = can be open simultaneously

    def test_path_extracts_directory_name(self):
        """Test path extracts directory name."""
        result = spec_to_workspace_id("./my-project")
        assert result == "my-project"

    def test_existing_workspace_id(self):
        """Test existing workspace ID is returned as-is."""
        assert spec_to_workspace_id("myworkspace") == "myworkspace"


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

    @patch("devlaunch.dl.get_remote_branches")
    @patch("devlaunch.dl.discover_repos_from_workspaces")
    @patch("devlaunch.dl.list_workspaces")
    def test_update_completion_cache_fetches_branches(
        self, mock_list, mock_discover, mock_branches
    ):
        """Test update_completion_cache fetches branches for all repos."""
        mock_list.return_value = [
            Workspace("ws1", "git", "github.com/owner/repo1", "", "docker", "vscode"),
        ]
        mock_discover.return_value = {"owner": ["repo1", "repo2"]}
        mock_branches.side_effect = [
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

    @patch("devlaunch.dl.get_remote_branches")
    @patch("devlaunch.dl.discover_repos_from_workspaces")
    @patch("devlaunch.dl.list_workspaces")
    def test_update_completion_cache_handles_branch_fetch_failure(
        self, mock_list, mock_discover, mock_branches
    ):
        """Test update_completion_cache handles repos where branch fetch fails."""
        mock_list.return_value = []
        mock_discover.return_value = {"owner": ["repo1"]}
        mock_branches.return_value = []  # Branch fetch failed

        with tempfile.TemporaryDirectory() as tmpdir:
            with patch("devlaunch.dl.CACHE_FILE", pathlib.Path(tmpdir) / "cache.json"):
                with patch(
                    "devlaunch.dl.BASH_CACHE_FILE", pathlib.Path(tmpdir) / "completions.bash"
                ):
                    data = update_completion_cache()

        assert data["branches"] == []


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
        mock_up.assert_called_once_with("myws", ide="vscode", workspace_id=None)

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
        mock_up.assert_called_once_with("myws", recreate=True, workspace_id=None)

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
        mock_up.assert_called_once_with("myws", workspace_id=None)

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
        mock_up.assert_called_once_with("myws", reset=True, workspace_id=None)

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
    @patch("devlaunch.dl.update_cache_background")
    def test_main_workspace_shell_command(self, _cache, mock_ssh, mock_up, mock_ids):
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
    @patch("devlaunch.dl.update_cache_background")
    def test_main_workspace_default(self, _cache, mock_ssh, mock_up, mock_ids):
        """Test default workspace start and attach."""
        mock_ids.return_value = ["myws"]
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws"]):
            result = main()
        assert result == 0
        mock_up.assert_called_once()
        mock_ssh.assert_called_once()

    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.expand_workspace_spec")
    @patch("devlaunch.dl.spec_to_workspace_id")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_new_workspace_from_repo(
        self, _cache, mock_ssh, mock_up, mock_spec_id, mock_expand, mock_ids, mock_use_worktree
    ):
        """Test creating workspace from owner/repo (DevPod backend)."""
        mock_use_worktree.return_value = False  # Use DevPod backend for this test
        mock_ids.return_value = []  # Not existing
        mock_expand.return_value = "github.com/owner/repo"
        mock_spec_id.return_value = "github-com-owner-repo"
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 0
        mock_expand.assert_called()
        mock_up.assert_called_once_with(
            "github.com/owner/repo", workspace_id="github-com-owner-repo"
        )
        mock_ssh.assert_called_once_with("github-com-owner-repo", None)

    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.ensure_remote_branch")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_new_workspace_from_repo_with_existing_branch(
        self, _cache, mock_ssh, mock_up, mock_ensure, mock_ids, mock_use_worktree
    ):
        """Test creating workspace from owner/repo@branch when branch exists."""
        mock_use_worktree.return_value = False  # Use DevPod backend
        mock_ids.return_value = []  # Not existing
        mock_ensure.return_value = True  # Branch exists
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 0
        mock_ensure.assert_called_once_with("owner/repo", "main")
        # workspace_id is the branch name when branch is specified
        mock_up.assert_called_once_with("git@github.com:owner/repo.git@main", workspace_id="main")

    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.ensure_remote_branch")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_new_workspace_creates_branch(
        self, _cache, mock_ssh, mock_up, mock_ensure, mock_ids, mock_use_worktree
    ):
        """Test creating workspace from owner/repo@newbranch creates the branch."""
        mock_use_worktree.return_value = False  # Use DevPod backend
        mock_ids.return_value = []  # Not existing
        mock_ensure.return_value = True  # Branch created successfully
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@newbranch"]):
            result = main()
        assert result == 0
        mock_ensure.assert_called_once_with("owner/repo", "newbranch")
        mock_up.assert_called_once_with(
            "git@github.com:owner/repo.git@newbranch", workspace_id="newbranch"
        )

    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.ensure_remote_branch")
    def test_main_branch_creation_fails(self, mock_ensure, mock_ids, mock_use_worktree):
        """Test error when branch creation fails."""
        mock_use_worktree.return_value = False  # Use DevPod backend
        mock_ids.return_value = []  # Not existing
        mock_ensure.return_value = False  # Branch creation failed
        with patch.object(sys, "argv", ["dl", "owner/repo@newbranch"]):
            result = main()
        assert result == 1
        mock_ensure.assert_called_once_with("owner/repo", "newbranch")

    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.ensure_remote_branch")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_feature_branch_with_slash(
        self, _cache, mock_ssh, mock_up, mock_ensure, mock_ids, mock_use_worktree
    ):
        """Test creating workspace with feature/branch style branch name."""
        mock_use_worktree.return_value = False  # Use DevPod backend
        mock_ids.return_value = []
        mock_ensure.return_value = True
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@feature/my-feature"]):
            result = main()
        assert result == 0
        mock_ensure.assert_called_once_with("owner/repo", "feature/my-feature")
        # Branch name is sanitized: feature/my-feature -> feature-my-feature
        mock_up.assert_called_once_with(
            "git@github.com:owner/repo.git@feature/my-feature", workspace_id="feature-my-feature"
        )

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_existing_workspace_no_branch_check(self, _cache, mock_ssh, mock_up, mock_ids):
        """Test existing workspace doesn't trigger branch check."""
        mock_ids.return_value = ["myworkspace"]  # Existing
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        # Use existing workspace name (not owner/repo format)
        with patch.object(sys, "argv", ["dl", "myworkspace"]):
            with patch("devlaunch.dl.ensure_remote_branch") as mock_ensure:
                result = main()
        assert result == 0
        mock_ensure.assert_not_called()  # No branch check for existing workspace

    @patch("devlaunch.dl.should_use_worktree_backend")
    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_repo_without_branch_no_branch_check(
        self, _cache, mock_ssh, mock_up, mock_ids, mock_use_worktree
    ):
        """Test owner/repo without @branch doesn't trigger branch check."""
        mock_use_worktree.return_value = False  # Use DevPod backend for this test
        mock_ids.return_value = []
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            with patch("devlaunch.dl.ensure_remote_branch") as mock_ensure:
                result = main()
        assert result == 0
        mock_ensure.assert_not_called()  # No branch specified
