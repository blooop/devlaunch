"""Tests for dl (DevLaunch CLI) functionality."""

import io
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import pathlib
from unittest.mock import patch, MagicMock
import pytest

from devlaunch import dl as dl_module
from devlaunch import gh_auth
from devlaunch.devpod_ssh import RemoteExit
from devlaunch.workspace_id import TARGET_LENGTH, WorkspaceId
from devlaunch.dl import (
    expand_workspace_spec,
    is_path_spec,
    is_git_spec,
    parse_owner_repo_from_url,
    parse_owner_repo_branch,
    discover_repos_from_workspaces,
    discover_repos_from_cache_dir,
    get_known_repos,
    UnreadableWorkspaceList,
    GitRepository,
    LocalFolder,
    UnrecognisedSource,
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
    get_remote_branches,
    get_local_branches,
    main,
    print_help,
    print_workspaces,
    workspace_stop,
    workspace_delete,
    workspace_ssh,
    dotfiles_update,
    run_devpod,
    extract_devcontainer_flag,
    workspace_up,
    get_workspace_state,
    DevpodNotInstalled,
    get_cache_path,
    update_cache_background,
    completion_cache_is_fresh,
    cache_refresh_spawned,
    reset_cache_refresh_state,
    COMPLETION_CACHE_TTL_SECONDS,
)


def stub_devpod_session(mock_popen, returncode=0, stderr=""):
    """Make a patched subprocess.Popen stand in for a finished devpod session.

    run_devpod_session drives Popen as a context manager and reads the stderr
    pipe, so a plain return_value is not enough.
    """
    proc = MagicMock(returncode=returncode)
    proc.stderr = io.StringIO(stderr)
    mock_popen.return_value.__enter__.return_value = proc
    return proc


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
        assert ws.source == LocalFolder("/home/user/myproject")
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
        assert ws.source == GitRepository("github.com/loft-sh/devpod")

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
        assert ws.source == UnrecognisedSource({"someOther": "value"})

    def test_from_json_missing_fields(self):
        """Test parsing workspace with missing optional fields."""
        data = {"id": "minimal"}
        ws = Workspace.from_json(data)
        assert ws.id == "minimal"
        assert ws.source == UnrecognisedSource({})
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
        """A devpod that exited non-zero has not said there are no workspaces."""
        mock_result = MagicMock()
        mock_result.returncode = 1
        mock_result.stdout = ""
        mock_result.stderr = ""
        mock_run.return_value = mock_result

        with pytest.raises(UnreadableWorkspaceList):
            list_workspaces()

    @patch("devlaunch.dl.run_devpod")
    def test_list_workspaces_invalid_json(self, mock_run):
        """Nor has a devpod whose output could not be parsed."""
        mock_result = MagicMock()
        mock_result.returncode = 0
        mock_result.stdout = "not valid json"
        mock_run.return_value = mock_result

        with pytest.raises(UnreadableWorkspaceList):
            list_workspaces()


class TestGetWorkspaceIds:
    """Tests for get_workspace_ids function."""

    @patch("devlaunch.dl.list_workspaces")
    def test_get_workspace_ids(self, mock_list):
        """Test getting workspace IDs."""
        mock_list.return_value = [
            Workspace("ws1", LocalFolder("/path"), "", "docker", "vscode"),
            Workspace("ws2", GitRepository("github.com/o/r"), "", "docker", "none"),
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


class TestDiscoverReposFromWorkspaces:
    """Tests for discover_repos_from_workspaces function."""

    def test_discover_from_git_workspace(self):
        """Test discovering repo from git workspace."""
        workspaces = [
            Workspace("ws1", GitRepository("github.com/owner/repo"), "", "docker", "vscode"),
        ]
        repos = discover_repos_from_workspaces(workspaces)
        assert repos == {"owner": ["repo"]}

    @patch("devlaunch.dl.get_git_remote_url")
    def test_discover_from_local_workspace(self, mock_remote):
        """Test discovering repo from local workspace with git remote."""
        mock_remote.return_value = "git@github.com:blooop/python_template.git"
        workspaces = [
            Workspace("ws1", LocalFolder("/home/user/project"), "", "docker", "vscode"),
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
            Workspace("ws1", LocalFolder("/path1"), "", "docker", "vscode"),
            Workspace("ws2", LocalFolder("/path2"), "", "docker", "vscode"),
            Workspace("ws3", LocalFolder("/path3"), "", "docker", "vscode"),
        ]
        repos = discover_repos_from_workspaces(workspaces)
        assert repos == {"owner1": ["repo1", "repo3"], "owner2": ["repo2"]}

    @patch("devlaunch.dl.get_git_remote_url")
    def test_discover_no_remote(self, mock_remote):
        """Test workspace without git remote is skipped."""
        mock_remote.return_value = None
        workspaces = [
            Workspace("ws1", LocalFolder("/path"), "", "docker", "vscode"),
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
            Workspace("ws1", GitRepository("github.com/zowner/zrepo"), "", "docker", "vscode"),
            Workspace("ws2", GitRepository("github.com/aowner/arepo"), "", "docker", "vscode"),
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


def _dist_reporting(direct_url_text):
    """A stub of the installed-dist metadata reader (a system boundary).

    ``direct_url_text`` is what ``Distribution.read_text('direct_url.json')``
    hands back: the file's contents, or None when the file is not there.
    """
    dist = MagicMock()
    dist.read_text.return_value = direct_url_text
    return dist


@patch("devlaunch.dl.pkg_version", return_value="1.2.3")
class TestVersionProvenance:
    """--version distinguishes an editable dev install from a released one."""

    @patch("devlaunch.dl.distribution")
    def test_editable_install_is_named_as_dev_with_its_tree_path(
        self, mock_distribution, _mock_pkg_version
    ):
        """An editable install says so and names the tree it resolves to."""
        mock_distribution.return_value = _dist_reporting(
            '{"url":"file:///srv/checkouts/devlaunch","dir_info":{"editable":true}}'
        )
        version = get_version()
        assert version.startswith("1.2.3 ")
        assert "dev" in version
        assert "/srv/checkouts/devlaunch" in version

    @patch("devlaunch.dl.distribution")
    def test_percent_encoded_tree_path_is_decoded(self, mock_distribution, _mock_pkg_version):
        """The url is a file:// URI, so its path is decoded, not string-stripped."""
        mock_distribution.return_value = _dist_reporting(
            '{"url":"file:///srv/my%20checkouts/devlaunch","dir_info":{"editable":true}}'
        )
        assert "/srv/my checkouts/devlaunch" in get_version()

    @patch("devlaunch.dl.distribution")
    def test_non_editable_install_reports_bare_version(self, mock_distribution, _mock_pkg_version):
        """A released build's output is unchanged: just the version."""
        mock_distribution.return_value = _dist_reporting(
            '{"dir_info": {}, "url": "file:///build/work"}'
        )
        assert get_version() == "1.2.3"

    @patch("devlaunch.dl.distribution")
    def test_absent_direct_url_metadata_reports_bare_version(
        self, mock_distribution, _mock_pkg_version
    ):
        """A plain wheel install has no direct-url metadata at all."""
        mock_distribution.return_value = _dist_reporting(None)
        assert get_version() == "1.2.3"

    @patch("devlaunch.dl.distribution")
    def test_malformed_direct_url_metadata_reports_bare_version(
        self, mock_distribution, _mock_pkg_version
    ):
        """Unparsable direct-url metadata degrades instead of raising."""
        mock_distribution.return_value = _dist_reporting("{not json at all")
        assert get_version() == "1.2.3"

    @patch("devlaunch.dl.distribution")
    def test_direct_url_metadata_missing_keys_reports_bare_version(
        self, mock_distribution, _mock_pkg_version
    ):
        """Editable metadata with no url to name degrades instead of raising."""
        mock_distribution.return_value = _dist_reporting('{"dir_info":{"editable":true}}')
        assert get_version() == "1.2.3"

    @patch("devlaunch.dl.distribution")
    def test_unreadable_dist_metadata_reports_bare_version(
        self, mock_distribution, _mock_pkg_version
    ):
        """A metadata reader that blows up must not take --version with it."""
        mock_distribution.side_effect = OSError("dist-info is gone")
        assert get_version() == "1.2.3"


class TestSpecToWorkspaceId:
    """Tests for spec_to_workspace_id function."""

    def test_owner_repo_produces_repo_name(self):
        """Test owner/repo generates sanitized repo name as workspace ID."""
        assert spec_to_workspace_id("blooop/devlaunch") == "devlaunch"

    def test_owner_repo_with_branch_uses_repo_branch_suffix(self):
        """owner/repo@branch derives <repo-slug>-<branch-slug>-<syl3>.

        The syllable suffix is new: without it the id was not injective, and
        `blooop/devlaunch@feature/auth` and `@feature-auth` named one workspace.
        """
        assert spec_to_workspace_id("blooop/devlaunch@main") == "devlaunch-main-zovomobo"

    def test_owner_repo_with_feature_branch(self):
        """Test owner/repo@feature/branch slugs the branch name."""
        result = spec_to_workspace_id("owner/repo@feature/my-branch")
        assert result == WorkspaceId("owner", "repo", "feature/my-branch").value
        assert result.startswith("repo-feature-my-branch-")

    def test_owner_repo_with_uppercase_branch(self):
        """The repo and branch are lowercased into the slug; the owner is not in it.

        The owner still shapes the id through the suffix, and is case-folded there
        because GitHub owner names are case-insensitive.
        """
        result = spec_to_workspace_id("Owner/Repo@Feature/MyBranch")
        assert result.startswith("repo-feature-mybranch-")

    def test_repo_case_does_not_fork_the_workspace(self):
        """One GitHub repo is one workspace, whatever case the user typed.

        Hashing owner and repo raw made every spelling its own container and its own
        full clone: `dl NVIDIA/cuda-samples@main` and `dl nvidia/cuda-samples@main`
        were two workspaces. The old derivation lowercased, so this was a regression.
        """
        spellings = [
            "blooop/devlaunch@main",
            "Blooop/devlaunch@main",
            "blooop/DevLaunch@main",
            "BLOOOP/DEVLAUNCH@main",
        ]
        assert len({spec_to_workspace_id(s) for s in spellings}) == 1

    def test_branch_case_does_fork_the_workspace(self):
        """Refs are case-sensitive in git, so these are genuinely two workspaces."""
        assert spec_to_workspace_id("blooop/devlaunch@main") != spec_to_workspace_id(
            "blooop/devlaunch@Main"
        )

    def test_owner_is_part_of_the_identity(self):
        """The old derivation dropped the owner, so two forks shared an id."""
        assert spec_to_workspace_id("blooop/devlaunch@main") != spec_to_workspace_id(
            "someone/devlaunch@main"
        )

    def test_slash_and_dash_branches_are_distinct(self):
        """Defect 1 from #55: these two used to both derive `devlaunch-feature-auth`."""
        assert spec_to_workspace_id("blooop/devlaunch@feature/auth") != spec_to_workspace_id(
            "blooop/devlaunch@feature-auth"
        )

    def test_github_url_sanitized(self):
        """Test github.com/owner/repo generates sanitized ID (fallback path)."""
        assert spec_to_workspace_id("github.com/loft-sh/devpod").startswith(
            "github-com-loft-sh-devpod-"
        )

    def test_https_url_strips_protocol(self):
        """Test https URL strips protocol and sanitizes (fallback path)."""
        assert spec_to_workspace_id("https://github.com/owner/repo").startswith(
            "github-com-owner-repo-"
        )

    def test_url_with_git_suffix_strips_it(self):
        """Test URL with .git suffix strips it (fallback path)."""
        assert spec_to_workspace_id("github.com/owner/repo.git") == spec_to_workspace_id(
            "github.com/owner/repo"
        )

    def test_url_specs_are_injective(self):
        """`my_repo`, `my-repo` and `my.repo` share a slug but are three repos.

        The first cut of the new scheme applied the slug rule here with no suffix,
        which collapsed all three onto `gitlab-com-group-my-repo` — trading the
        old derivation's collision for a new one.
        """
        sources = [
            "gitlab.com/group/my_repo",
            "gitlab.com/group/my-repo",
            "gitlab.com/group/my.repo",
        ]
        assert len({spec_to_workspace_id(s) for s in sources}) == 3

    def test_url_specs_are_capped(self):
        """A long URL used to yield 92 characters, past devpod's 48-char ceiling."""
        spec = f"github.com/{'o' * 40}/{'r' * 40}"
        assert len(spec_to_workspace_id(spec)) <= TARGET_LENGTH

    def test_url_specs_are_case_insensitive(self):
        assert spec_to_workspace_id("github.com/Blooop/DevLaunch") == spec_to_workspace_id(
            "github.com/blooop/devlaunch"
        )

    def test_underscore_replaced_in_repo(self):
        """Test underscores are replaced with hyphens in repo name."""
        assert spec_to_workspace_id("blooop/test_renv") == "test-renv"

    def test_repo_label_is_capped(self):
        """The ref-less repo label must respect the budget too; it yielded 60 chars."""
        assert len(spec_to_workspace_id(f"owner/{'r' * 60}")) <= TARGET_LENGTH

    def test_branch_allows_multiple_workspaces(self):
        """Test different branches get different workspace IDs."""
        nb12 = spec_to_workspace_id("blooop/test_renv@nb12")
        nb14 = spec_to_workspace_id("blooop/test_renv@nb14")
        assert nb12.startswith("test-renv-nb12-")
        assert nb14.startswith("test-renv-nb14-")
        # Different branches = different IDs = can be open simultaneously
        assert nb12 != nb14

    def test_branch_truncation(self):
        """Test branch name is truncated so total stays within the target length."""
        long_branch = "a" * 60
        result = spec_to_workspace_id(f"owner/repo@{long_branch}")
        assert len(result) <= TARGET_LENGTH
        assert result.startswith("repo-")

    def test_branch_truncation_strips_trailing_hyphen(self):
        """Test truncated branch doesn't end with a hyphen."""
        # Use a branch that after truncation would end with '-'
        result = spec_to_workspace_id("owner/myrepo@feature/some-very-long-branch-name-here")
        assert not result.endswith("-")
        assert len(result) <= TARGET_LENGTH

    def test_long_repo_name_is_capped(self):
        """Defect 2 from #55: a 47-char repo name skipped truncation and gave 80 chars."""
        result = spec_to_workspace_id(f"owner/{'r' * 47}@main")
        assert len(result) <= TARGET_LENGTH

    def test_path_extracts_directory_name(self):
        """Test path extracts directory name."""
        result = spec_to_workspace_id("./my-project")
        assert result == "my-project"

    def test_existing_workspace_id(self):
        """Test existing workspace ID is returned as-is."""
        assert spec_to_workspace_id("myworkspace") == "myworkspace"

    def test_python_template_example(self):
        """Test the motivating example from the plan."""
        result = spec_to_workspace_id("blooop/python_template@nb4")
        assert result.startswith("python-template-nb4-")

    def test_no_branch_no_suffix(self):
        """owner/repo with no branch is a repo label, not a workspace identity.

        There is no ref to hash, so there is nothing to identify. Every path that
        creates a workspace resolves the default branch first and derives the id
        from the resolved triple.
        """
        assert spec_to_workspace_id("blooop/python_template") == "python-template"

    def test_unsafe_branch_is_rejected(self):
        """Bad input gets one response everywhere: rejection at the constructor."""
        with pytest.raises(ValueError, match="Invalid git ref"):
            spec_to_workspace_id("owner/repo@bad%branch")


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
            Workspace("ws1", GitRepository("github.com/owner/repo1"), "", "docker", "vscode"),
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
            Workspace("ws1", GitRepository("github.com/owner/repo1"), "", "docker", "vscode"),
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


def _devpod_up_args(mock_run):
    """The argv of the `devpod up` call, out of everything workspace_up spawns.

    `up` is no longer the last thing workspace_up does -- it goes on to install
    the session tools (devlaunch.tools), which is another run_devpod call -- so
    reading call_args would give an `ssh --command` instead.
    """
    up_calls = [c for c in mock_run.call_args_list if c[0][0][:1] == ["up"]]
    assert up_calls, "expected a devpod up call"
    return up_calls[0][0][0]


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
        args = _devpod_up_args(mock_run)
        assert "--init-env" in args
        assert args[args.index("--init-env") + 1] == "DEVLAUNCH_WORKSPACE_ID=repo-nb4"

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_workspace_up_falls_back_to_the_creation_id(self, mock_run, _mock_ctx):
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path", workspace_id="repo-main")
        args = _devpod_up_args(mock_run)
        assert args[args.index("--init-env") + 1] == "DEVLAUNCH_WORKSPACE_ID=repo-main"

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_workspace_up_omits_init_env_without_an_identity(self, mock_run, _mock_ctx):
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        assert "--init-env" not in mock_run.call_args[0][0]

    @patch("devlaunch.dl.subprocess.run")
    def test_run_devpod_does_not_touch_the_environment(self, mock_run):
        """The identity travels as a devpod argument, not as inherited env."""
        mock_run.return_value = MagicMock(returncode=0)
        run_devpod(["up", "/path"])
        assert mock_run.call_args[1].get("env") is None

    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_identity_reaches_devpod_through_main(
        self, mock_run, _mock_ctx, _mock_ssh, _mock_state
    ):
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


class TestGhTokenForwarding:
    """Tests for handing the host's gh login to whatever container is launched.

    These patch subprocess.run rather than run_devpod so the flags devpod
    actually receives are pinned, not just devlaunch's intent to send them.
    """

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.gh_auth.resolve_token", return_value="gho_hosttoken")
    @patch("devlaunch.dl.subprocess.run")
    def test_up_hands_devpod_the_token_out_of_band(self, mock_run, _mock_token, _mock_ctx):
        """The file has to be readable while devpod runs, and the token stays out of argv."""
        seen = {}

        def read_env_file(cmd, **_kwargs):
            path = cmd[cmd.index("--workspace-env-file") + 1]
            seen["contents"] = pathlib.Path(path).read_text(encoding="utf-8")
            seen["cmd"] = cmd
            return MagicMock(returncode=0)

        mock_run.side_effect = read_env_file
        workspace_up("/path")
        assert seen["contents"] == "GH_TOKEN=gho_hosttoken\n"
        assert "gho_hosttoken" not in " ".join(seen["cmd"])

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.gh_auth.resolve_token", return_value=None)
    @patch("devlaunch.dl.subprocess.run")
    def test_up_forwards_nothing_when_the_host_has_no_token(self, mock_run, _mock_token, _mock_ctx):
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        assert "--workspace-env-file" not in mock_run.call_args[0][0]

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.subprocess.run")
    def test_the_opt_out_stops_a_token_that_is_there_for_the_taking(
        self, mock_run, _mock_ctx, monkeypatch
    ):
        """DEVLAUNCH_NO_GH_TOKEN, not the absence of a token, is what stops this."""
        monkeypatch.setenv("GH_TOKEN", "gho_hosttoken")
        monkeypatch.setenv(gh_auth.DISABLE_VAR, "1")
        gh_auth.resolve_token.cache_clear()
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        assert "--workspace-env-file" not in mock_run.call_args[0][0]

    @patch("devlaunch.gh_auth.resolve_token", return_value="gho_hosttoken")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_attach_sends_the_token_through_devpods_environment(self, mock_popen, _mock_token):
        stub_devpod_session(mock_popen)
        workspace_ssh("myws")
        cmd = mock_popen.call_args[0][0]
        assert cmd[cmd.index("--send-env") + 1] == "GH_TOKEN"
        assert "gho_hosttoken" not in " ".join(cmd)
        assert mock_popen.call_args[1]["env"]["GH_TOKEN"] == "gho_hosttoken"

    @patch("devlaunch.gh_auth.resolve_token", return_value=None)
    @patch("devlaunch.dl.subprocess.Popen")
    def test_attach_leaves_the_environment_alone_without_a_token(self, mock_popen, _mock_token):
        stub_devpod_session(mock_popen)
        workspace_ssh("myws")
        assert "--send-env" not in mock_popen.call_args[0][0]
        assert mock_popen.call_args[1].get("env") is None

    @patch("devlaunch.dl.update_cache_background")
    @patch("devlaunch.dl.get_workspace_state", return_value="Running")
    @patch("devlaunch.gh_auth.resolve_token", return_value="gho_hosttoken")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_an_already_running_workspace_still_gets_the_token(
        self, mock_popen, _mock_token, _mock_state, _mock_cache
    ):
        """Attaching to a running workspace skips `devpod up` and its workspace env."""
        stub_devpod_session(mock_popen)
        with patch.object(sys, "argv", ["dl", "myws"]):
            main()
        forwarded = [c for c in mock_popen.call_args_list if "--send-env" in c[0][0]]
        assert forwarded, "expected the attach to forward gh auth"
        assert forwarded[0][1]["env"]["GH_TOKEN"] == "gho_hosttoken"


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

    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_selection_reaches_devpod_through_main(
        self, mock_run, _mock_ctx, _mock_state, _mock_ssh
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

    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.get_workspace_state", return_value="Running")
    @patch("devlaunch.dl.workspace_up")
    def test_ignored_on_a_running_workspace_warns(self, mock_up, _mock_state, _mock_ssh, caplog):
        """Fast-attach skips workspace_up entirely, so the flag does nothing."""
        with patch.object(sys, "argv", ["dl", "myws", "--devcontainer", "sim"]):
            main()
        mock_up.assert_not_called()
        assert "Ignoring --devcontainer" in caplog.text

    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    @patch("devlaunch.dl.workspace_stop", return_value=0)
    def test_ignored_on_stop_warns(self, _mock_stop, _mock_state, caplog):
        with patch.object(sys, "argv", ["dl", "myws", "--devcontainer", "sim", "stop"]):
            main()
        assert "Ignoring --devcontainer" in caplog.text


class TestSharedPixiCache:
    """Tests for the pixi package cache every container downloads into once.

    A container's dotfiles install runs `pixi global sync`, which on an empty
    cache spends 62-113s and 1.2GB of network fetching packages that the last
    container downloaded already. The downloads are content-addressed and safe
    to share -- rattler locks per package -- so one host directory bound into
    every container turns that into an 18-28s unpack from disk (#232).

    What is shared is only the *cache*. Environments and trampolines are
    prefix-baked and stay per-container, which is why PIXI_HOME is never on
    this mount: two syncs sharing one env prefix is prefix-dev/pixi#5476.
    """

    @pytest.mark.parametrize("home", ["/home/", "/root/"])
    def test_the_mount_target_is_outside_every_home_directory(self, home):
        """The invariant #240 was the violation of.

        A bind target whose parent the image does not ship is created by the
        runtime as root, and a target under `$HOME` therefore hands the
        container a root-owned home cache: `~/.cache` stops being the user's
        own, and pip, uv, pre-commit and fontconfig lose it -- measured broken
        on stock `devcontainers/base:ubuntu` and `rust:latest`, which ship no
        `~/.cache` of their own.

        Stated over homes rather than over the one path that regressed,
        because the fault was never `/home/vscode` in particular: any home is
        a directory the image owns, whose layout dl cannot see and whose
        contents a dotfiles install may chown out from under the mount.
        """
        assert not dl_module.PIXI_CACHE_TARGET.startswith(home)

    def test_an_unmounted_target_is_still_a_directory_the_container_can_write(self):
        """Not-under-a-home is only half of it: the target must survive unmounted.

        devpod re-applies `--workspace-env` on every `up`, but a bind mount
        only lands when a container is created. So every container built
        before a target change gets the new `PIXI_CACHE_DIR` pointing at a
        path with nothing mounted on it, and pixi does not degrade there: it
        creates the cache or it fails the install outright. Measured on stock
        `devcontainers/base:ubuntu` as uid 1000 with no mount,
        `pixi global install jq` exits 1 with `Permission denied` under
        `/var/cache/devlaunch/pixi` and 0 under `/var/tmp/devlaunch-pixi`.

        The property that buys that is the parent's mode, so the parent has to
        be one of the two directories FHS requires to be world-writable and
        sticky. Both also pre-exist in every image, which is the other half:
        an intermediate directory the runtime has to invent is invented as
        root (`/var/cache/devlaunch` came out `root:root`), and a
        world-writable leaf under a root-owned parent is not something dl can
        arrange from outside the container.
        """
        fhs_world_writable = {"/tmp", "/var/tmp"}
        parent = str(pathlib.PurePosixPath(dl_module.PIXI_CACHE_TARGET).parent)
        assert parent in fhs_world_writable

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_up_binds_the_host_cache_into_the_container(
        self, mock_run, _mock_ctx, tmp_path, monkeypatch
    ):
        """The mount, spelled as devpod's `--mount` takes it.

        The source is a dedicated directory rather than the host's own
        `~/.cache/rattler/cache`: containers write into it as whatever uid
        their remoteUser has, and a host-side `pixi clean cache` must not be
        able to pull packages out from under a live container.
        """
        monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        args = _devpod_up_args(mock_run)
        assert args[args.index("--mount") + 1] == (
            f"type=bind,source={tmp_path}/cache/devlaunch/pixi,target=/var/tmp/devlaunch-pixi"
        )

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_up_points_pixi_at_the_mount_for_the_workspace(self, mock_run, _mock_ctx):
        """PIXI_CACHE_DIR wins over RATTLER_CACHE_DIR, so setting it is enough."""
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        args = _devpod_up_args(mock_run)
        assert args[args.index("--workspace-env") + 1] == ("PIXI_CACHE_DIR=/var/tmp/devlaunch-pixi")

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_up_points_pixi_at_the_mount_for_the_dotfiles_script(self, mock_run, _mock_ctx):
        """The dotfiles install script is the actual consumer, and devpod gives
        it an environment of its own -- a workspace env alone never reaches it,
        so the same assignment is passed twice on purpose."""
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        args = _devpod_up_args(mock_run)
        assert args[args.index("--dotfiles-script-env") + 1] == (
            "PIXI_CACHE_DIR=/var/tmp/devlaunch-pixi"
        )

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_the_host_directory_exists_before_devpod_is_asked_to_bind_it(
        self, mock_run, _mock_ctx, tmp_path, monkeypatch
    ):
        """dl creates the source, rather than leaving it to whatever is underneath.

        A bind source that does not exist is refused outright by some
        runtimes and created as root by others, and a root-owned directory is
        one the container cannot write a single package into -- which is the
        whole feature, failing silently and slowly.

        Asserted as a real directory on disk and from inside the spawn, so a
        creation that happened after `devpod up` returned would not pass.
        """
        monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
        expected = tmp_path / "cache" / "devlaunch" / "pixi"
        seen = {}

        def observe(argv, **_kwargs):
            seen[argv[0]] = expected.is_dir()
            return MagicMock(returncode=0)

        mock_run.side_effect = observe
        workspace_up("/path")
        assert seen["up"] is True

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_a_cache_directory_that_cannot_be_created_is_not_a_failed_launch(
        self, mock_run, _mock_ctx, monkeypatch
    ):
        """An unwritable cache home costs the sharing, not the container.

        Same call dl already makes about its launch lock: a full disk, a
        read-only mount or a directory some container left owned by another
        uid must not turn a `devpod up` that would have worked into a
        traceback. Without the mount the container downloads its own packages,
        which is exactly what it did before this feature.
        """
        blocked = pathlib.Path("/proc/version/cache")
        monkeypatch.setenv("XDG_CACHE_HOME", str(blocked))
        mock_run.return_value = MagicMock(returncode=0)
        workspace_up("/path")
        args = _devpod_up_args(mock_run)
        assert "--mount" not in args
        assert "--workspace-env" not in args
        assert "--dotfiles-script-env" not in args

    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_a_source_that_is_not_there_after_all_is_left_out_of_the_argv(
        self, mock_run, _mock_ctx, tmp_path, monkeypatch
    ):
        """The args are emitted only when the source really exists.

        dl creates the source itself, so the only way here is the narrow one:
        the creation reported success and the directory still is not there.
        The test has to monkeypatch `mkdir` to reach it at all, which is the
        honest measure of how narrow -- the check covers the microseconds
        between the mkdir and itself, not the wide window after it. What it
        pins is the choice of answer: no mount rather than a mount devpod
        cannot honour, since a missing bind source fails `up` outright and the
        `ssh` that follows a failed `up` starts the container anyway, without
        the mount and without saying so.
        """
        monkeypatch.setenv("XDG_CACHE_HOME", str(tmp_path / "cache"))
        mock_run.return_value = MagicMock(returncode=0)
        source = tmp_path / "cache" / "devlaunch" / "pixi"
        real_mkdir = pathlib.Path.mkdir

        def mkdir_that_leaves_only_this_one_behind(self, *args, **kwargs):
            real_mkdir(self, *args, **kwargs)
            if self == source:
                # ignore_errors so the removal itself never raises out of
                # mkdir: an OSError here would be caught by the branch that
                # already drops the args, and this test would pass without
                # ever exercising the existence check it is about.
                shutil.rmtree(self, ignore_errors=True)

        monkeypatch.setattr(pathlib.Path, "mkdir", mkdir_that_leaves_only_this_one_behind)
        workspace_up("/path")
        assert not source.exists()
        args = _devpod_up_args(mock_run)
        assert "--mount" not in args
        assert "--workspace-env" not in args
        assert "--dotfiles-script-env" not in args

    def test_the_host_directory_is_the_users_own_and_not_a_written_down_path(
        self, tmp_path, monkeypatch, home_cache_default
    ):  # pylint: disable=unused-argument
        """With no XDG_CACHE_HOME the answer follows the invoking user's home.

        The container-side path is a fixed system location and is rightly a
        constant; the host side is not, and a hardcoded one would send every
        user's packages to somebody else's home directory.
        """
        monkeypatch.setenv("HOME", str(tmp_path))
        assert dl_module._pixi_cache_source() == tmp_path / ".cache" / "devlaunch" / "pixi"  # pylint: disable=protected-access

    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.get_context_options", return_value={})
    @patch("devlaunch.dl.run_devpod")
    def test_the_mount_reaches_devpod_through_main(
        self, mock_run, _mock_ctx, _mock_ssh, _mock_state
    ):
        """End-to-end through argv, so a launch really carries it."""
        mock_run.return_value = MagicMock(returncode=0, stdout="", stderr="")
        with patch.object(sys, "argv", ["dl", "myws"]):
            main()
        args = _devpod_up_args(mock_run)
        assert "PIXI_CACHE_DIR=/var/tmp/devlaunch-pixi" in args


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
            Workspace("ws1", LocalFolder("/path/to/ws1"), "2024-01-01", "docker", "vscode"),
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
        """Test --update-cache flag updates a stale cache."""
        mock_update.return_value = {}
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
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

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_stop")
    def test_main_workspace_stop(self, mock_stop, mock_state):
        """Test workspace stop command."""
        mock_state.return_value = "Stopped"
        mock_stop.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "stop"]):
            result = main()
        assert result == 0
        mock_stop.assert_called_once_with("myws")

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_delete")
    def test_main_workspace_rm(self, mock_delete, mock_state):
        """Test workspace rm command."""
        mock_state.return_value = "Stopped"
        mock_delete.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "rm"]):
            result = main()
        assert result == 0
        mock_delete.assert_called_once_with("myws", ignore_missing=False)

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_delete")
    def test_main_workspace_prune(self, mock_delete, mock_state):
        """Test workspace prune command (alias for rm)."""
        mock_state.return_value = "Stopped"
        mock_delete.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "prune"]):
            result = main()
        assert result == 0
        mock_delete.assert_called_once()

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    def test_main_workspace_code(self, mock_up, mock_state):
        """Test workspace code command."""
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=0)
        with patch.object(sys, "argv", ["dl", "myws", "code"]):
            result = main()
        assert result == 0
        mock_up.assert_called_once_with(
            "myws", ide="vscode", workspace_id=None, workspace_identity="myws", devcontainer=None
        )

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_main_workspace_recreate(self, mock_ssh, mock_up, mock_state):
        """Test workspace recreate command."""
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "recreate"]):
            result = main()
        assert result == 0
        mock_up.assert_called_once_with(
            "myws", recreate=True, workspace_id=None, workspace_identity="myws", devcontainer=None
        )

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_stop")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_main_workspace_restart(self, mock_ssh, mock_up, mock_stop, mock_state):
        """Test workspace restart command."""
        mock_state.return_value = "Stopped"
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

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_main_workspace_reset(self, mock_ssh, mock_up, mock_state):
        """Test workspace reset command."""
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "reset"]):
            result = main()
        assert result == 0
        mock_up.assert_called_once_with(
            "myws", reset=True, workspace_id=None, workspace_identity="myws", devcontainer=None
        )

    @patch("devlaunch.dl.get_workspace_state")
    def test_main_unknown_command_error(self, mock_state, caplog):
        """Test unknown subcommand returns error."""
        mock_state.return_value = "Stopped"
        with patch.object(sys, "argv", ["dl", "myws", "badcmd"]):
            result = main()
        assert result == 1
        assert "Unknown command" in caplog.text

    @patch("devlaunch.dl.get_workspace_state")
    def test_main_invalid_workspace_error(self, mock_state, caplog):
        """Test invalid workspace spec returns error."""
        mock_state.return_value = None
        with patch.object(sys, "argv", ["dl", "nonexistent"]):
            result = main()
        assert result == 1
        assert "Unknown workspace" in caplog.text

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_workspace_shell_command(self, _cache, mock_ssh, mock_up, mock_state):
        """Test running shell command with -- separator."""
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "--", "echo", "hello"]):
            result = main()
        assert result == 0
        mock_ssh.assert_called_once_with("myws", "echo hello")

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_workspace_default(self, _cache, mock_ssh, mock_up, mock_state):
        """Test default workspace start and attach."""
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws"]):
            result = main()
        assert result == 0
        mock_up.assert_called_once()
        mock_ssh.assert_called_once_with("myws", None)

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_new_workspace_from_repo(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_state
    ):
        """Test creating workspace from owner/repo resolves default branch."""
        mock_state.return_value = None  # Not existing
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.get_default_branch.return_value = "main"
        mock_mgr.prepare_cold.return_value = pathlib.Path("/tmp/ws/repo-main")
        mock_clone_mgr.return_value = mock_mgr
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 0
        # Should resolve default branch
        mock_mgr.repo_manager.get_default_branch.assert_called_once_with("owner", "repo")
        # One call prepares the clone, the branch and the workspace, and the
        # workspace ID includes the resolved branch
        mock_mgr.prepare_cold.assert_called_once_with(
            "owner", "repo", "main", "git@github.com:owner/repo.git"
        )
        main_id = WorkspaceId("owner", "repo", "main").value
        mock_up.assert_called_once_with(
            "/tmp/ws/repo-main",
            workspace_id=main_id,
            workspace_identity=main_id,
            devcontainer=None,
        )
        mock_ssh.assert_called_once_with(main_id, None)

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_new_workspace_from_repo_with_branch(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_state
    ):
        """A named branch goes straight to the cold entrypoint."""
        mock_state.return_value = None  # Not existing
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.return_value = pathlib.Path("/tmp/ws/repo-main")
        mock_clone_mgr.return_value = mock_mgr
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 0
        # Should NOT resolve default branch (branch was specified), and should
        # not take a repo-lock cycle of its own to find that out
        mock_mgr.repo_manager.get_default_branch.assert_not_called()
        mock_mgr.repo_manager.ensure_repo.assert_not_called()
        mock_mgr.prepare_cold.assert_called_once_with(
            "owner", "repo", "main", "git@github.com:owner/repo.git"
        )
        main_id = WorkspaceId("owner", "repo", "main").value
        mock_up.assert_called_once_with(
            "/tmp/ws/repo-main",
            workspace_id=main_id,
            workspace_identity=main_id,
            devcontainer=None,
        )

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_new_workspace_creates_branch(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_state
    ):
        """Test creating workspace from owner/repo@newbranch creates the branch."""
        mock_state.return_value = None  # Not existing
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.return_value = pathlib.Path("/tmp/ws/repo-newbranch")
        mock_clone_mgr.return_value = mock_mgr
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@newbranch"]):
            result = main()
        assert result == 0
        mock_mgr.prepare_cold.assert_called_once_with(
            "owner", "repo", "newbranch", "git@github.com:owner/repo.git"
        )

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_main_branch_creation_fails(self, mock_clone_mgr, mock_state):
        """Test error when preparing the branch fails."""
        mock_state.return_value = None  # Not existing
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.side_effect = RuntimeError("push failed")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@newbranch"]):
            result = main()
        assert result == 1
        mock_mgr.prepare_cold.assert_called_once_with(
            "owner", "repo", "newbranch", "git@github.com:owner/repo.git"
        )

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_main_clone_fails_no_branch(self, mock_clone_mgr, mock_state):
        """Test error when ensure_repo fails (no branch specified, triggers clone for default branch)."""
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.ensure_repo.side_effect = RuntimeError("repository not found")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 1
        mock_mgr.repo_manager.ensure_repo.assert_called_once()

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_main_clone_fails_with_branch(self, mock_clone_mgr, mock_state):
        """Test error when the clone fails (branch specified, workspace not existing).

        With a branch named, clone-if-missing happens inside the cold entrypoint
        rather than in a call of its own, so that is where the failure comes from.
        """
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.side_effect = OSError("network unreachable")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@newbranch"]):
            result = main()
        assert result == 1
        mock_mgr.prepare_cold.assert_called_once()

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    def test_main_unsafe_branch_is_reported_not_raised(
        self, mock_up, mock_clone_mgr, mock_state, caplog
    ):
        """An unsafe ref is rejected at the constructor with a message, not a traceback.

        `%` passes OWNER_REPO_PATTERN, so this spec reaches id derivation and is
        stopped there — before it can name a container or a directory.
        """
        mock_state.return_value = None
        mock_clone_mgr.return_value = MagicMock()
        with patch.object(sys, "argv", ["dl", "owner/repo@bad%branch"]):
            result = main()
        assert result == 1
        assert "Invalid git ref name" in caplog.text
        mock_clone_mgr.return_value.prepare_cold.assert_not_called()
        mock_up.assert_not_called()

    @pytest.mark.parametrize(
        "spec,kind",
        [("x/..", "repo"), ("../x", "owner"), ("x/.", "repo"), ("../..", "owner")],
    )
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    def test_main_rejects_traversal_before_touching_the_cache(
        self, mock_up, mock_clone_mgr, mock_state, spec, kind, caplog
    ):
        """Owner and repo are validated before anything builds a path from them.

        ensure_repo() joins repos_dir/<owner>/<repo>, and (repos_dir/'x'/'..')
        resolves to repos_dir itself while '../x' escapes it entirely. Validation
        used to happen only at the WorkspaceId, which is constructed *after* that
        call, so the traversal reached the filesystem and was rejected afterwards.
        """
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", spec]):
            result = main()
        assert result == 1
        assert f"Invalid git {kind} name" in caplog.text
        # Nothing may reach the cache or devpod.
        mock_mgr.repo_manager.ensure_repo.assert_not_called()
        mock_mgr.repo_manager.get_default_branch.assert_not_called()
        mock_mgr.prepare_cold.assert_not_called()
        mock_up.assert_not_called()

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_feature_branch_with_slash(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_state
    ):
        """Test creating workspace with feature/branch style branch name."""
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.return_value = pathlib.Path("/tmp/ws/repo-feature-my-feature")
        mock_clone_mgr.return_value = mock_mgr
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo@feature/my-feature"]):
            result = main()
        assert result == 0
        mock_mgr.prepare_cold.assert_called_once_with(
            "owner", "repo", "feature/my-feature", "git@github.com:owner/repo.git"
        )

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_existing_workspace_no_clone_manager(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_state
    ):
        """Test existing workspace doesn't use clone manager."""
        mock_state.return_value = "Stopped"  # Existing
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "myworkspace"]):
            result = main()
        assert result == 0
        mock_clone_mgr.assert_not_called()

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_main_repo_without_branch_resolves_default(
        self, _cache, mock_ssh, mock_up, mock_clone_mgr, mock_state
    ):
        """Test owner/repo without @branch resolves default branch."""
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.get_default_branch.return_value = "main"
        mock_mgr.prepare_cold.return_value = pathlib.Path("/tmp/ws/repo-main")
        mock_clone_mgr.return_value = mock_mgr
        mock_up.return_value = MagicMock(returncode=0)
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 0
        # Should resolve default branch and use it
        mock_mgr.repo_manager.get_default_branch.assert_called_once_with("owner", "repo")
        mock_mgr.prepare_cold.assert_called_once_with(
            "owner", "repo", "main", "git@github.com:owner/repo.git"
        )


class TestPurgeFunctionality:
    """Tests for purge functionality."""

    @staticmethod
    def _clones(cache_dir: pathlib.Path, *ids: str):
        """Workspaces in the shape devlaunch creates them: clones under its cache.

        These used to be sourced from `/path` and a git URL, which is what a
        workspace someone else made looks like -- so this test asserted that
        `--purge` deleted other people's work. See test/unit/test_purge_ownership.py.
        """
        return [
            Workspace(
                ws, LocalFolder(str(cache_dir / "repos" / "o" / "r" / ws)), "", "docker", "none"
            )
            for ws in ids
        ]

    @patch("devlaunch.dl.run_devpod")
    @patch("devlaunch.dl.list_workspaces")
    def test_purge_deletes_the_workspaces_devlaunch_created(self, mock_list, mock_run):
        """Test purge_all_data deletes every workspace devlaunch created."""
        from devlaunch.dl import purge_all_data

        mock_run.return_value = MagicMock(returncode=0)

        with tempfile.TemporaryDirectory() as tmpdir:
            cache_dir = pathlib.Path(tmpdir)
            mock_list.return_value = self._clones(cache_dir, "ws1", "ws2")
            with patch("devlaunch.dl._get_cache_dir", return_value=cache_dir):
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

        # First delete fails, second succeeds
        mock_run.side_effect = [
            MagicMock(returncode=1, stderr="error"),
            MagicMock(returncode=0),
        ]

        with tempfile.TemporaryDirectory() as tmpdir:
            cache_dir = pathlib.Path(tmpdir)
            mock_list.return_value = self._clones(cache_dir, "ws1", "ws2")
            with patch("devlaunch.dl._get_cache_dir", return_value=cache_dir):
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


class TestContainerWorkdir:
    """Where a devpod ssh session lands, which is devpod's business and not dl's."""

    @patch("devlaunch.dl.get_workspace_state", return_value="Running")
    @patch("devlaunch.dl.run_devpod_session")
    def test_attach_does_not_override_workdir(self, mock_session, _mock_state):
        """devpod ssh already starts in devcontainer.json's workspaceFolder.

        Asserted through main(), because that is where the guessed
        /workspaces/<id> used to be built — and devpod silently drops the session
        in $HOME for any project with a custom workspaceFolder.
        """
        mock_session.return_value = RemoteExit(0)
        with patch.object(sys, "argv", ["dl", "myws"]):
            main()
        ssh_calls = [c for c in mock_session.call_args_list if c[0][0][:1] == ["ssh"]]
        assert ssh_calls, "expected a devpod ssh call"
        assert "--workdir" not in ssh_calls[0][0][0]


class TestWorkspaceSsh:
    """Tests for workspace_ssh."""

    @patch("devlaunch.dl.run_devpod_session")
    def test_workspace_ssh_basic(self, mock_session):
        """Test basic SSH."""
        mock_session.return_value = RemoteExit(0)
        result = workspace_ssh("myws")
        assert result == 0
        mock_session.assert_called_once_with(["ssh", "myws"], env=None)

    @patch("devlaunch.dl.run_devpod_session")
    def test_workspace_ssh_with_command(self, mock_session):
        """Test SSH with command."""
        mock_session.return_value = RemoteExit(0)
        result = workspace_ssh("myws", command="echo hello")
        assert result == 0
        mock_session.assert_called_once_with(
            ["ssh", "myws", "--command", "bash -lc 'echo hello'"], env=None
        )

    @patch("devlaunch.dl.run_devpod_session")
    def test_workspace_ssh_interactive_is_not_wrapped(self, mock_session):
        """An interactive attach gets no --command and no shell wrapper."""
        mock_session.return_value = RemoteExit(0)
        result = workspace_ssh("myws", workdir="/workspaces/myws")
        assert result == 0
        sent_args = mock_session.call_args.args[0]
        assert "--command" not in sent_args
        assert not any("bash -lc" in arg for arg in sent_args)

    @patch("devlaunch.dl.run_devpod_session")
    def test_workspace_ssh_command_with_quotes_runs_in_login_shell(self, mock_session):
        """A payload containing single quotes survives the login-shell wrapper."""
        mock_session.return_value = RemoteExit(0)
        result = workspace_ssh("myws", command="claude 'do the thing'")
        assert result == 0
        # shlex.quote closes the outer quote, emits '"'"' for each literal
        # single quote, and reopens -- so the inner quotes reach the payload
        # intact instead of terminating it.
        mock_session.assert_called_once_with(
            ["ssh", "myws", "--command", "bash -lc 'claude '\"'\"'do the thing'\"'\"''"],
            env=None,
        )

    @patch("devlaunch.dl.run_devpod_session")
    def test_workspace_ssh_with_workdir(self, mock_session):
        """Test SSH with workdir."""
        mock_session.return_value = RemoteExit(0)
        result = workspace_ssh("myws", workdir="/some/path")
        assert result == 0
        mock_session.assert_called_once_with(["ssh", "myws", "--workdir", "/some/path"], env=None)

    @patch("devlaunch.dl.run_devpod_session")
    def test_workspace_ssh_with_workdir_and_command(self, mock_session):
        """Test SSH with both workdir and command."""
        mock_session.return_value = RemoteExit(0)
        result = workspace_ssh("myws", command="make test", workdir="/workspaces/myws")
        assert result == 0
        mock_session.assert_called_once_with(
            ["ssh", "myws", "--workdir", "/workspaces/myws", "--command", "bash -lc 'make test'"],
            env=None,
        )


class TestZellijSessionWrap:
    """The opt-in terminal-beside-the-agent wrap (#242), on and off.

    What the capability actually needs is measured and narrow: with a zellij
    session merely *existing* in the container, `zellij -s <name> action
    new-pane -- <cmd>` opens a working pane from a command that is not itself
    in any session, with no TTY anywhere. So the wrap ensures the named session
    exists and then runs the command **beside** it, rather than inside a pane.

    Running the command inside a pane would have taken its stdout and its exit
    status away from `dl` -- zellij would own the pty and report its own status
    -- and both are contracts the rest of this file pins: `dl <ws> -- cmd >
    file` puts the command's output in the file, and `TestSessionExitStatus`
    below exists entirely to keep the remote program's status intact. The
    capability the ticket asks for is delivered either way; only one of the two
    designs breaks every scripted caller.

    Default off is the other half, and it is asserted as an absence: the
    payload pins in `TestWorkspaceSsh` above run with the switch unset and were
    not edited for this feature.
    """

    ENSURE = "zellij attach -b devlaunch >/dev/null 2>&1 || true"

    @patch("devlaunch.dl.run_devpod_session")
    def test_it_is_off_unless_switched_on(self, mock_session, monkeypatch):
        """No existing invocation changes meaning. The whole default."""
        monkeypatch.delenv(dl_module.ZELLIJ_WRAP_VAR, raising=False)
        mock_session.return_value = RemoteExit(0)
        assert workspace_ssh("myws", command="echo hello") == 0
        mock_session.assert_called_once_with(
            ["ssh", "myws", "--command", "bash -lc 'echo hello'"], env=None
        )

    @patch("devlaunch.dl.run_devpod_session")
    def test_switched_on_the_command_runs_beside_a_named_session(self, mock_session, monkeypatch):
        """The payload, pinned whole.

        `attach -b` and not `attach -c`: `-b` creates the session detached and
        returns, which is the only form a non-TTY `devpod ssh --command` can
        use. Measured live in `devcontainers/base:ubuntu`.
        """
        monkeypatch.setenv(dl_module.ZELLIJ_WRAP_VAR, "1")
        mock_session.return_value = RemoteExit(0)
        assert workspace_ssh("myws", command="echo hello") == 0
        mock_session.assert_called_once_with(
            ["ssh", "myws", "--command", f"bash -lc '{self.ENSURE}; echo hello'"],
            env=None,
        )

    @patch("devlaunch.dl.run_devpod_session")
    def test_a_session_that_is_already_there_is_not_an_error(self, mock_session, monkeypatch):
        """Measured: a second `zellij attach -b <name>` exits **1** once the
        session exists, which is the case every launch after the first takes.
        Without the `|| true` the ensure would fail on exactly the common
        path, so the tolerance is pinned rather than left to reading."""
        monkeypatch.setenv(dl_module.ZELLIJ_WRAP_VAR, "1")
        mock_session.return_value = RemoteExit(0)
        workspace_ssh("myws", command="true")
        payload = mock_session.call_args.args[0][-1]
        assert "|| true" in payload
        assert "&&" not in payload

    @patch("devlaunch.dl.run_devpod_session")
    def test_the_commands_own_status_still_comes_back(self, mock_session, monkeypatch):
        """The ensure runs in front of the command and `;` separates them, so
        what the payload exits with is the command's status and never the
        session setup's. `dl <ws> -- pytest` stays scriptable."""
        monkeypatch.setenv(dl_module.ZELLIJ_WRAP_VAR, "1")
        mock_session.return_value = RemoteExit(2)
        assert workspace_ssh("myws", command="pytest") == 2

    @patch("devlaunch.dl.run_devpod_session")
    def test_the_interactive_session_itself_is_untouched_even_switched_on(
        self, mock_session, monkeypatch
    ):
        """What the session of a bare `dl <ws>` does, stated as a test.

        Nothing. It sends no `--command` at all -- that is what gets it a pty
        from devpod -- so there is no payload to wrap, and giving it one would
        cost either the pty or a round trip of its own in front of every
        shell (#183's lesson). A human at that shell has zellij on PATH and can
        attach to or create the session by hand; the wrap exists for the
        program dl launches, which cannot.

        This is the session, not the whole of `dl <ws>`: see
        `test_an_opted_in_refresh_carries_the_ensure_in_front_of_the_shell` for
        the one command a bare attach can send ahead of it.
        """
        monkeypatch.setenv(dl_module.ZELLIJ_WRAP_VAR, "1")
        mock_session.return_value = RemoteExit(0)
        assert workspace_ssh("myws") == 0
        mock_session.assert_called_once_with(["ssh", "myws"], env=None)

    @patch("devlaunch.dl.run_devpod_session")
    def test_an_opted_in_refresh_carries_the_ensure_in_front_of_the_shell(
        self, mock_session, monkeypatch
    ):
        """The one way a bare `dl <ws>` does send a wrapped command.

        `attach_workspace` puts the opt-in dotfiles refresh in front of the
        session, and a refresh *is* a command, so with both switches on it is
        wrapped like any other -- the session exists by the time the shell is
        handed over. Benign, arguably the nicest arrival there is, but it means
        "a bare attach is untouched" is true of the session and not of the
        command that can precede it. Pinned so the two switches are known to
        compose rather than assumed to.
        """
        monkeypatch.setenv(dl_module.ZELLIJ_WRAP_VAR, "1")
        monkeypatch.setenv(dl_module.DOTFILES_ON_ATTACH_VAR, "1")
        mock_session.return_value = RemoteExit(0)
        dl_module.attach_workspace("myws")
        refresh = mock_session.call_args_list[0].args[0][-1]
        assert self.ENSURE in refresh
        # ...and the session behind it is still a bare, wrap-free `ssh`.
        assert mock_session.call_args_list[-1].args[0] == ["ssh", "myws"]

    @patch("devlaunch.dl.run_devpod_session")
    def test_a_command_with_quotes_survives_both_wrappers(self, mock_session, monkeypatch):
        """One `shlex.quote`, applied after the ensure is prepended, so the
        composed payload is quoted once as a whole rather than twice."""
        monkeypatch.setenv(dl_module.ZELLIJ_WRAP_VAR, "1")
        mock_session.return_value = RemoteExit(0)
        workspace_ssh("myws", command="claude 'do the thing'")
        inner = f"{self.ENSURE}; claude 'do the thing'"
        assert mock_session.call_args.args[0][-1] == f"bash -lc {shlex.quote(inner)}"

    @pytest.mark.parametrize("setting", ["", "0", "false", "no", "NO", " 0 "])
    @patch("devlaunch.dl.run_devpod_session")
    def test_a_switch_set_to_a_denial_is_still_off(self, mock_session, monkeypatch, setting):
        """The same vocabulary of denials the other switches read, so an
        `export DEVLAUNCH_ZELLIJ=0` means off here too."""
        monkeypatch.setenv(dl_module.ZELLIJ_WRAP_VAR, setting)
        mock_session.return_value = RemoteExit(0)
        workspace_ssh("myws", command="echo hello")
        assert mock_session.call_args.args[0][-1] == "bash -lc 'echo hello'"


class TestSessionExitStatus:
    """What `dl` reports when a session ends, driven through subprocess.Popen.

    devpod turns every nonzero remote exit into its own generic failure — exit
    code 1 plus an error and a fatal line — because it type-asserts on an
    *ssh.ExitError it has already wrapped three times. These pin devlaunch
    against that, from the process boundary inwards.
    """

    DEVPOD_NOISE = (
        "\x1b[97;1m20:41:27 \x1b[0m\x1b[91;1merror \x1b[0m"
        "Try using the --debug flag to see a more verbose output    root.go:106\n"
        "\x1b[97;1m20:41:27 \x1b[0m\x1b[91;1mfatal \x1b[0m"
        "tunnel to container: run in container: ssh session: "
        "Process exited with status 130                            root.go:113\n"
    )

    @patch("devlaunch.dl.subprocess.Popen")
    def test_a_shell_that_exits_130_is_reported_as_130_not_as_devpods_1(self, mock_popen, capsys):
        """The reported bug: `exit` after a Ctrl-C printed a fatal and returned 1."""
        stub_devpod_session(mock_popen, returncode=1, stderr=self.DEVPOD_NOISE)
        assert workspace_ssh("myws") == 130
        assert "fatal" not in capsys.readouterr().err

    @patch("devlaunch.dl.subprocess.Popen")
    def test_a_command_that_fails_keeps_its_own_status(self, mock_popen):
        """`dl ws -- pytest` is scriptable only if the status survives the trip."""
        stderr = self.DEVPOD_NOISE.replace("status 130", "status 2")
        stub_devpod_session(mock_popen, returncode=1, stderr=stderr)
        assert workspace_ssh("myws", command="pytest") == 2

    @patch("devlaunch.dl.subprocess.Popen")
    def test_devpods_own_failures_still_surface(self, mock_popen, capsys):
        """Only the noise devpod prints in place of a status is held back."""
        stderr = "20:41:27 fatal tunnel to container: dial tcp: connection refused\n"
        stub_devpod_session(mock_popen, returncode=1, stderr=stderr)
        assert workspace_ssh("myws") == 1
        assert capsys.readouterr().err == stderr

    @patch("devlaunch.dl.subprocess.Popen")
    def test_the_session_keeps_the_terminals_stdin_and_stdout(self, mock_popen):
        """devpod puts the real terminal into raw mode and asks for a pty on that
        basis, so only stderr may be taken away from it."""
        stub_devpod_session(mock_popen)
        workspace_ssh("myws")
        kwargs = mock_popen.call_args[1]
        assert kwargs["stderr"] is subprocess.PIPE
        assert "stdout" not in kwargs
        assert "stdin" not in kwargs


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

    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_git_spec_existing_workspace_skips_clone_manager(
        self, _cache, mock_ssh, mock_up, mock_state, mock_clone_mgr
    ):
        """Test git spec with existing workspace skips clone manager."""
        main_id = WorkspaceId("owner", "repo", "main").value
        mock_state.return_value = "Stopped"  # Known but not running, so workspace_up still called
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
            main_id, workspace_id=None, workspace_identity=main_id, devcontainer=None
        )

    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_git_spec_running_workspace_skips_workspace_up(
        self, _cache, mock_ssh, mock_up, mock_state, mock_clone_mgr
    ):
        """Test git spec with Running workspace skips workspace_up()."""
        main_id = WorkspaceId("owner", "repo", "main").value
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
        mock_ssh.assert_called_once_with(main_id, None)

    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_git_spec_stopped_workspace_calls_workspace_up(
        self, _cache, mock_ssh, mock_up, mock_state, mock_clone_mgr
    ):
        """Test git spec with Stopped workspace still calls workspace_up() with ID only."""
        main_id = WorkspaceId("owner", "repo", "main").value
        mock_state.return_value = "Stopped"  # Known but not running
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
            main_id, workspace_id=None, workspace_identity=main_id, devcontainer=None
        )

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_existing_id_running_skips_workspace_up(self, _cache, mock_ssh, mock_up, mock_state):
        """Test existing raw workspace ID with Running state skips workspace_up()."""
        mock_state.return_value = "Running"
        mock_ssh.return_value = 0
        with patch.object(sys, "argv", ["dl", "python-template-ws3"]):
            result = main()
        assert result == 0
        # workspace_up should NOT be called
        mock_up.assert_not_called()
        # Should SSH in to attach
        mock_ssh.assert_called_once_with("python-template-ws3", None)

    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.update_cache_background")
    def test_git_spec_no_branch_existing_workspace_skips_clone_manager(
        self, _cache, mock_ssh, mock_up, mock_state, mock_clone_mgr
    ):
        """Test owner/repo (no branch) with existing workspace skips full clone pipeline."""
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

    @patch("devlaunch.dl.get_workspace_state")
    def test_invalid_spec_bare_word(self, mock_state, caplog):
        """Bare word that devpod has no workspace for returns error."""
        mock_state.return_value = None
        with patch.object(sys, "argv", ["dl", "nonexistent"]):
            result = main()
        assert result == 1
        assert "Unknown workspace 'nonexistent'" in caplog.text

    @patch("devlaunch.dl.get_workspace_state")
    def test_invalid_spec_partial_match(self, mock_state, caplog):
        """Partial workspace name doesn't match."""
        mock_state.return_value = None
        with patch.object(sys, "argv", ["dl", "my-work"]):
            result = main()
        assert result == 1
        assert "Unknown workspace" in caplog.text

    @patch("devlaunch.dl.get_workspace_state")
    def test_invalid_spec_suggests_alternatives(self, mock_state, caplog):
        """Error message suggests using --ls or owner/repo."""
        mock_state.return_value = None
        with patch.object(sys, "argv", ["dl", "badname"]):
            result = main()
        assert result == 1
        assert "dl --ls" in caplog.text
        assert "owner/repo" in caplog.text

    # --- Unknown subcommand errors ---

    @patch("devlaunch.dl.get_workspace_state")
    def test_unknown_subcommand_message(self, mock_state, caplog):
        """Unknown subcommand produces helpful error with -- hint."""
        mock_state.return_value = "Stopped"
        with patch.object(sys, "argv", ["dl", "myws", "badcmd"]):
            result = main()
        assert result == 1
        assert "Unknown command 'badcmd'" in caplog.text
        assert "dl myws -- badcmd" in caplog.text

    @patch("devlaunch.dl.get_workspace_state")
    def test_unknown_subcommand_with_git_spec(self, mock_state, caplog):
        """Unknown subcommand with owner/repo spec returns error."""
        mock_state.return_value = "Stopped"
        with patch.object(sys, "argv", ["dl", "repo-main", "deploy"]):
            result = main()
        assert result == 1
        assert "Unknown command 'deploy'" in caplog.text

    # --- Clone failure errors (no duplicate messages) ---

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_clone_fail_no_branch_single_error(self, mock_clone_mgr, mock_state, caplog):
        """Clone failure (no branch) produces exactly one error line."""
        mock_state.return_value = None
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

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_clone_fail_with_branch_single_error(self, mock_clone_mgr, mock_state, caplog):
        """Clone failure (with branch) produces exactly one error line."""
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.side_effect = OSError("network unreachable")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@feature"]):
            result = main()
        assert result == 1
        error_records = [r for r in caplog.records if r.levelname == "ERROR"]
        assert len(error_records) == 1
        assert "owner/repo" in error_records[0].message

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_clone_fail_runtime_error_message(self, mock_clone_mgr, mock_state, caplog):
        """RuntimeError from clone surfaces the original error text."""
        mock_state.return_value = None
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

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_branch_ensure_runtime_error(self, mock_clone_mgr, mock_state, caplog):
        """Branch preparation RuntimeError logged with the branch name."""
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.side_effect = RuntimeError("push failed: permission denied")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@newbranch"]):
            result = main()
        assert result == 1
        assert "newbranch" in caplog.text
        assert "push failed" in caplog.text

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_branch_ensure_os_error(self, mock_clone_mgr, mock_state, caplog):
        """Branch preparation OSError returns 1."""
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.side_effect = OSError("git not found")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@develop"]):
            result = main()
        assert result == 1
        assert "develop" in caplog.text

    # --- Workspace prepare failure errors ---

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_prepare_cold_runtime_error(self, mock_clone_mgr, mock_state, caplog):
        """prepare_cold RuntimeError returns 1 with message."""
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.side_effect = RuntimeError("worktree creation failed")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 1
        assert "Failed to prepare workspace" in caplog.text
        assert "worktree creation failed" in caplog.text

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_prepare_cold_os_error(self, mock_clone_mgr, mock_state, caplog):
        """prepare_cold OSError returns 1."""
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.side_effect = OSError("disk full")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 1
        assert "disk full" in caplog.text

    # --- workspace_up failure errors ---

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_workspace_up_exception(self, _mock_ssh, mock_up, mock_state, caplog):
        """workspace_up exception returns 1 with message."""
        mock_state.return_value = "Stopped"
        mock_up.side_effect = RuntimeError("devpod crashed")
        with patch.object(sys, "argv", ["dl", "myws"]):
            result = main()
        assert result == 1
        assert "Failed to create workspace" in caplog.text

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_workspace_up_nonzero_exit(self, mock_ssh, mock_up, mock_state):
        """workspace_up returning non-zero propagates exit code."""
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=2)
        with patch.object(sys, "argv", ["dl", "myws"]):
            result = main()
        assert result == 2
        mock_ssh.assert_not_called()

    # --- Subcommand failure propagation ---

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    def test_recreate_up_failure_propagates(self, mock_up, mock_state):
        """recreate subcommand propagates workspace_up failure."""
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=3)
        with patch.object(sys, "argv", ["dl", "myws", "recreate"]):
            result = main()
        assert result == 3

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_stop")
    def test_restart_stop_failure_propagates(self, mock_stop, mock_state):
        """restart subcommand propagates stop failure."""
        mock_state.return_value = "Stopped"
        mock_stop.return_value = 1
        with patch.object(sys, "argv", ["dl", "myws", "restart"]):
            result = main()
        assert result == 1

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_stop")
    @patch("devlaunch.dl.workspace_up")
    def test_restart_up_failure_propagates(self, mock_up, mock_stop, mock_state):
        """restart subcommand propagates workspace_up failure after successful stop."""
        mock_state.return_value = "Stopped"
        mock_stop.return_value = 0
        mock_up.return_value = MagicMock(returncode=4)
        with patch.object(sys, "argv", ["dl", "myws", "restart"]):
            result = main()
        assert result == 4

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    def test_reset_up_failure_propagates(self, mock_up, mock_state):
        """reset subcommand propagates workspace_up failure."""
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=5)
        with patch.object(sys, "argv", ["dl", "myws", "reset"]):
            result = main()
        assert result == 5

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    def test_code_up_failure_propagates(self, mock_up, mock_state):
        """code subcommand propagates workspace_up failure."""
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=1)
        with patch.object(sys, "argv", ["dl", "myws", "code"]):
            result = main()
        assert result == 1

    # --- No duplicate error messages ---

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_clone_fail_no_duplicate_failed_to_clone(self, mock_clone_mgr, mock_state, caplog):
        """Verify 'Failed to clone' doesn't appear twice in error output."""
        mock_state.return_value = None
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

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_fetch_fail_no_duplicate_messages(self, mock_clone_mgr, mock_state, caplog):
        """Verify fetch failure doesn't produce duplicate error messages."""
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.prepare_cold.side_effect = RuntimeError(
            "Failed to fetch repository: network timeout"
        )
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo@main"]):
            result = main()
        assert result == 1
        error_records = [r for r in caplog.records if r.levelname == "ERROR"]
        assert len(error_records) == 1

    # --- Edge cases ---

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.workspace_ssh")
    def test_workspace_up_os_error(self, _mock_ssh, mock_up, mock_state, caplog):
        """workspace_up OSError is caught and returns 1."""
        mock_state.return_value = "Stopped"
        mock_up.side_effect = OSError("connection refused")
        with patch.object(sys, "argv", ["dl", "myws"]):
            result = main()
        assert result == 1
        assert "connection refused" in caplog.text

    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl._get_clone_manager")
    def test_clone_fail_empty_error_message(self, mock_clone_mgr, mock_state, caplog):
        """Clone failure with empty error still returns 1."""
        mock_state.return_value = None
        mock_mgr = MagicMock()
        mock_mgr.repo_manager.ensure_repo.side_effect = RuntimeError("")
        mock_clone_mgr.return_value = mock_mgr
        with patch.object(sys, "argv", ["dl", "owner/repo"]):
            result = main()
        assert result == 1
        error_records = [r for r in caplog.records if r.levelname == "ERROR"]
        assert len(error_records) == 1


def _devpod_missing():
    """The error the OS raises when `devpod` is not on PATH."""
    return FileNotFoundError(2, "No such file or directory", "devpod")


class TestMissingDevpodBinary:
    """A missing devpod binary must produce one actionable line, not a traceback."""

    INSTALL_URL = "https://devpod.sh/docs/getting-started/install"

    def test_signal_is_not_an_oserror(self):
        """The signal must dodge every broad OSError/RuntimeError handler in dl.

        FileNotFoundError is an OSError, and dl catches OSError in a dozen
        places to degrade gracefully (empty branch lists, "failed to prepare
        workspace"). A missing binary reported through those handlers is
        reported wrongly, so it travels as its own type.
        """
        assert issubclass(DevpodNotInstalled, Exception)
        assert not issubclass(DevpodNotInstalled, OSError)
        assert not issubclass(DevpodNotInstalled, RuntimeError)

    @pytest.mark.parametrize("capture", [False, True])
    def test_run_devpod_translates_the_os_error(self, capture):
        """The single devpod seam converts FileNotFoundError into the signal."""
        with patch("devlaunch.dl.subprocess.run", side_effect=_devpod_missing()):
            with pytest.raises(DevpodNotInstalled) as excinfo:
                run_devpod(["list"], capture=capture)
        message = str(excinfo.value)
        assert "devpod" in message
        assert self.INSTALL_URL in message

    def test_run_devpod_still_reports_a_command_that_ran_and_failed(self):
        """A devpod that exists and exits non-zero is not a missing binary."""
        with patch("devlaunch.dl.subprocess.run", return_value=MagicMock(returncode=1)):
            assert run_devpod(["list"]).returncode == 1

    @patch("devlaunch.dl.update_cache_background")
    def test_ls_prints_one_line_to_stderr_and_exits_127(self, _cache, capsys):
        """`dl --ls` is the ticket's repro: one line on stderr, exit 127."""
        with patch("devlaunch.dl.subprocess.run", side_effect=_devpod_missing()):
            with patch.object(sys, "argv", ["dl", "--ls"]):
                result = main()
        assert result == 127
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err.strip().count("\n") == 0
        assert "devpod" in captured.err
        assert self.INSTALL_URL in captured.err
        assert "Traceback" not in captured.err

    @patch("devlaunch.dl.update_cache_background")
    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    def test_workspace_up_handler_does_not_swallow_it(self, _state, _cache, capsys, caplog):
        """Proof for main()'s `except (RuntimeError, OSError)` around workspace_up.

        workspace_up shells out to devpod from inside that try block; the
        generic "Failed to create workspace" must not appear.
        """
        with patch("devlaunch.dl.subprocess.run", side_effect=_devpod_missing()):
            with patch.object(sys, "argv", ["dl", "myws"]):
                result = main()
        assert result == 127
        assert "Failed to create workspace" not in caplog.text
        assert "devpod" in capsys.readouterr().err

    @patch("devlaunch.dl.update_cache_background")
    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    def test_delete_handler_does_not_swallow_it(self, _state, _cache, capsys, caplog):
        """Second call site: `dl <ws> rm`, which has its own broad handlers.

        workspace_delete reports devpod failures itself and wraps the local
        clone cleanup in `except Exception`; neither of those messages may
        stand in for "devpod is not installed".
        """
        with patch("devlaunch.dl.subprocess.run", side_effect=_devpod_missing()):
            with patch.object(sys, "argv", ["dl", "myws", "rm"]):
                result = main()
        assert result == 127
        assert "could not delete" not in caplog.text
        assert "Failed to remove local clone" not in caplog.text
        assert "devpod" in capsys.readouterr().err

    @patch("devlaunch.dl.update_cache_background")
    @patch("devlaunch.dl.read_completion_cache", return_value=None)
    def test_repos_flag_keeps_stdout_clean(self, _cache_read, _cache, capsys):
        """`dl --repos` feeds shell completion: nothing may reach stdout."""
        with patch("devlaunch.dl.subprocess.run", side_effect=_devpod_missing()):
            with patch.object(sys, "argv", ["dl", "--repos"]):
                result = main()
        assert result == 127
        captured = capsys.readouterr()
        assert captured.out == ""
        assert "devpod" in captured.err

    @patch("devlaunch.dl.update_cache_background")
    @patch("devlaunch.dl.read_completion_cache", return_value=None)
    def test_completion_data_flag_keeps_stdout_clean(self, _cache_read, _cache, capsys):
        """`dl --completion-data` must not emit half a JSON document."""
        with patch("devlaunch.dl.subprocess.run", side_effect=_devpod_missing()):
            with patch.object(sys, "argv", ["dl", "--completion-data"]):
                result = main()
        assert result == 127
        captured = capsys.readouterr()
        assert captured.out == ""
        assert "devpod" in captured.err

    @patch("devlaunch.dl.write_bash_completion_cache")
    @patch("devlaunch.dl.write_completion_cache")
    def test_update_cache_flag_leaves_the_cache_alone(self, mock_write, mock_write_bash, capsys):
        """The background updater must not overwrite a good cache with nothing.

        The cache is backdated first so the TTL does not skip the sweep before it
        can reach devpod -- a fresh cache is a second, unrelated reason for the
        updater to write nothing, and this test is about the missing binary.
        """
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch("devlaunch.dl.subprocess.run", side_effect=_devpod_missing()):
            with patch.object(sys, "argv", ["dl", "--update-cache"]):
                result = main()
        assert result == 127
        mock_write.assert_not_called()
        mock_write_bash.assert_not_called()
        assert "devpod" in capsys.readouterr().err

    @patch("devlaunch.dl.update_cache_background")
    def test_help_never_touches_devpod(self, _cache, capsys):
        """--help must work on a box with no devpod at all."""
        with patch("devlaunch.dl.subprocess.run", side_effect=_devpod_missing()) as mock_run:
            with patch.object(sys, "argv", ["dl", "--help"]):
                result = main()
        assert result == 0
        mock_run.assert_not_called()
        captured = capsys.readouterr()
        assert "dl - DevLaunch CLI" in captured.out
        assert captured.err == ""

    @patch("devlaunch.dl.update_cache_background")
    def test_version_never_touches_devpod(self, _cache, capsys):
        """--version must work on a box with no devpod at all."""
        with patch("devlaunch.dl.subprocess.run", side_effect=_devpod_missing()) as mock_run:
            with patch.object(sys, "argv", ["dl", "--version"]):
                result = main()
        assert result == 0
        mock_run.assert_not_called()
        captured = capsys.readouterr()
        assert captured.out.startswith("dl ")
        assert captured.err == ""

    def test_exit_code_reaches_the_shell(self, tmp_path):
        """End to end: run dl with a PATH that has no devpod on it."""
        env = {
            "PATH": str(tmp_path / "empty-bin"),
            "HOME": str(tmp_path),
            "XDG_CACHE_HOME": str(tmp_path / "cache"),
            "DEVLAUNCH_NO_GH_TOKEN": "1",
        }
        proc = subprocess.run(
            [sys.executable, "-m", "devlaunch.dl", "--ls"],
            cwd=str(pathlib.Path(__file__).resolve().parent.parent),
            capture_output=True,
            text=True,
            check=False,
            env=env,
            timeout=60,
        )
        assert proc.returncode == 127
        assert proc.stdout == ""
        assert "Traceback" not in proc.stderr
        assert self.INSTALL_URL in proc.stderr
        assert proc.stderr.strip().count("\n") == 0


def _age_completion_cache(seconds: float) -> None:
    """Backdate the completion cache's mtime so it reads as `seconds` old."""
    stamp = time.time() - seconds
    os.utime(get_cache_path(), (stamp, stamp))


class TestCompletionCacheFreshness:
    """The TTL that decides whether a background refresh is worth spawning."""

    def test_ttl_mirrors_the_lazy_fetch_interval(self):
        """One hour: the same staleness RepositoryManager already allows a repo."""
        assert COMPLETION_CACHE_TTL_SECONDS == 3600

    def test_a_just_written_cache_is_fresh(self):
        assert completion_cache_is_fresh()

    def test_a_cache_older_than_the_ttl_is_stale(self):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        assert not completion_cache_is_fresh()

    def test_a_cache_just_inside_the_ttl_is_fresh(self):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS - 60)
        assert completion_cache_is_fresh()

    def test_a_missing_cache_is_not_fresh(self):
        get_cache_path().unlink()
        assert not completion_cache_is_fresh()

    def test_freshness_follows_the_file_not_its_contents(self):
        """mtime is the timestamp, so a cache written by an older dl still counts."""
        get_cache_path().write_text("{}", encoding="utf-8")
        assert completion_cache_is_fresh()


class TestBackgroundRefreshSpawning:
    """update_cache_background: at most one spawn, and none for a fresh cache."""

    @patch("devlaunch.dl.subprocess.Popen")
    def test_fresh_cache_costs_no_subprocess(self, mock_popen):
        update_cache_background()
        mock_popen.assert_not_called()

    @patch("devlaunch.dl.subprocess.Popen")
    def test_stale_cache_spawns_a_refresh(self, mock_popen):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        update_cache_background()
        mock_popen.assert_called_once()
        assert mock_popen.call_args[0][0][1:] == ["-m", "devlaunch.dl", "--update-cache"]

    @patch("devlaunch.dl.subprocess.Popen")
    def test_missing_cache_spawns_a_refresh(self, mock_popen):
        get_cache_path().unlink()
        update_cache_background()
        mock_popen.assert_called_once()

    @patch("devlaunch.dl.subprocess.Popen")
    def test_force_ignores_the_ttl(self, mock_popen):
        """A caller that just changed the workspace list knows better than the TTL."""
        update_cache_background(force=True)
        mock_popen.assert_called_once()
        assert mock_popen.call_args[0][0][-1] == "--force"

    @patch("devlaunch.dl.subprocess.Popen")
    def test_only_one_spawn_per_process(self, mock_popen):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        update_cache_background()
        update_cache_background()
        update_cache_background(force=True)
        mock_popen.assert_called_once()

    @patch("devlaunch.dl.subprocess.Popen")
    def test_skipping_on_freshness_does_not_use_up_the_one_spawn(self, mock_popen):
        """A TTL skip is 'not needed yet', not 'already done'."""
        update_cache_background()
        update_cache_background(force=True)
        mock_popen.assert_called_once()

    @patch("devlaunch.dl.subprocess.Popen")
    def test_the_latch_is_observable_and_resettable(self, mock_popen):
        assert not cache_refresh_spawned()
        update_cache_background(force=True)
        assert cache_refresh_spawned()
        reset_cache_refresh_state()
        assert not cache_refresh_spawned()
        update_cache_background(force=True)
        assert mock_popen.call_count == 2

    @patch("devlaunch.dl.subprocess.Popen", side_effect=OSError("no fork for you"))
    def test_a_failed_spawn_is_survivable(self, _mock_popen):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        update_cache_background()


class TestRefreshChildRechecksFreshness:
    """The spawned child re-checks the TTL, to close the two-parents race."""

    @patch("devlaunch.dl.update_completion_cache")
    def test_child_skips_the_sweep_when_the_cache_is_already_fresh(self, mock_update):
        with patch.object(sys, "argv", ["dl", "--update-cache"]):
            assert main() == 0
        mock_update.assert_not_called()

    @patch("devlaunch.dl.update_completion_cache")
    def test_child_sweeps_when_the_cache_is_stale(self, mock_update):
        mock_update.return_value = {}
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch.object(sys, "argv", ["dl", "--update-cache"]):
            assert main() == 0
        mock_update.assert_called_once()

    @patch("devlaunch.dl.update_completion_cache")
    def test_child_sweeps_a_fresh_cache_when_forced(self, mock_update):
        """The forced spawn follows a workspace change: age says nothing about it."""
        mock_update.return_value = {}
        with patch.object(sys, "argv", ["dl", "--update-cache", "--force"]):
            assert main() == 0
        mock_update.assert_called_once()


class TestNoRefreshForCacheFreeCommands:
    """--help/--version/--install/--purge never spawn a refresh."""

    @patch("devlaunch.dl.subprocess.Popen")
    def test_help_spawns_nothing(self, mock_popen):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch.object(sys, "argv", ["dl", "--help"]):
            assert main() == 0
        mock_popen.assert_not_called()

    @patch("devlaunch.dl.subprocess.Popen")
    def test_short_help_spawns_nothing(self, mock_popen):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch.object(sys, "argv", ["dl", "-h"]):
            assert main() == 0
        mock_popen.assert_not_called()

    @patch("devlaunch.dl.subprocess.Popen")
    def test_version_spawns_nothing(self, mock_popen):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch.object(sys, "argv", ["dl", "--version"]):
            assert main() == 0
        mock_popen.assert_not_called()

    @patch("devlaunch.dl.install_completions", return_value=0)
    @patch("devlaunch.dl.update_completion_cache")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_install_builds_the_cache_in_the_foreground_and_spawns_nothing(
        self, mock_popen, mock_update, _mock_install
    ):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch.object(sys, "argv", ["dl", "--install"]):
            assert main() == 0
        mock_update.assert_called_once()
        mock_popen.assert_not_called()

    @patch("devlaunch.dl.list_workspaces", return_value=[])
    @patch("devlaunch.dl.purge_all_data", return_value=0)
    @patch("devlaunch.dl.subprocess.Popen")
    def test_purge_spawns_nothing(self, mock_popen, _mock_purge, _mock_ls):
        """Nothing should be racing to rewrite a cache directory being deleted."""
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch.object(sys, "argv", ["dl", "--purge", "-y"]):
            assert main() == 0
        mock_popen.assert_not_called()


class TestManualRefreshIgnoresTheTtl:
    """--refresh is the escape hatch: it always refreshes, in the foreground."""

    @patch("devlaunch.dl.update_completion_cache")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_refresh_sweeps_even_a_fresh_cache(self, mock_popen, mock_update, capsys):
        mock_update.return_value = {"workspaces": ["ws1"]}
        with patch.object(sys, "argv", ["dl", "--refresh"]):
            assert main() == 0
        mock_update.assert_called_once()
        mock_popen.assert_not_called()
        assert "1 workspaces found" in capsys.readouterr().out


class TestCacheReadingCommandsWarmTheCache:
    """The commands that serve completions keep the cache warm, TTL permitting."""

    @patch("devlaunch.dl.print_workspaces")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_ls_spawns_once_when_the_cache_is_stale(self, mock_popen, _mock_print):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch.object(sys, "argv", ["dl", "--ls"]):
            assert main() == 0
        mock_popen.assert_called_once()

    @patch("devlaunch.dl.print_workspaces")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_ls_spawns_nothing_when_the_cache_is_fresh(self, mock_popen, _mock_print):
        with patch.object(sys, "argv", ["dl", "--ls"]):
            assert main() == 0
        mock_popen.assert_not_called()

    @patch("devlaunch.dl.read_completion_cache", return_value={"repos": ["owner/repo"]})
    @patch("devlaunch.dl.subprocess.Popen")
    def test_repos_completion_warms_a_stale_cache(self, mock_popen, _mock_cache):
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch.object(sys, "argv", ["dl", "--repos"]):
            assert main() == 0
        mock_popen.assert_called_once()

    @patch("devlaunch.dl.read_completion_cache", return_value={"repos": []})
    @patch("devlaunch.dl.subprocess.Popen")
    def test_repos_completion_is_free_when_the_cache_is_fresh(self, mock_popen, _mock_cache):
        with patch.object(sys, "argv", ["dl", "--repos"]):
            assert main() == 0
        mock_popen.assert_not_called()

    @patch("devlaunch.dl.read_completion_cache", return_value={"repos": []})
    @patch("devlaunch.dl.subprocess.Popen")
    def test_completion_data_is_free_when_the_cache_is_fresh(self, mock_popen, _mock_cache):
        with patch.object(sys, "argv", ["dl", "--completion-data"]):
            assert main() == 0
        mock_popen.assert_not_called()


class TestWorkspaceCommandsRefreshOnceAfterwards:
    """Workspace commands change what the cache describes, so they force one
    refresh -- after the command, not before, and never more than one."""

    @patch("devlaunch.dl.run_devpod")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_stop_forces_exactly_one_refresh(self, mock_popen, mock_devpod):
        # The one devpod spawn before the command is the `status` probe that
        # resolves the spec; it answers through run_devpod like the stop itself.
        mock_devpod.return_value = MagicMock(returncode=0, stdout='{"state": "Stopped"}')
        with patch.object(sys, "argv", ["dl", "myws", "stop"]):
            assert main() == 0
        mock_popen.assert_called_once()
        assert mock_popen.call_args[0][0][-1] == "--force"

    @patch("devlaunch.dl.run_devpod")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_a_stale_cache_buys_no_refresh_of_the_state_about_to_change(
        self, mock_popen, mock_devpod
    ):
        """The one refresh a stop gets is the one that runs after the stop.

        A stale cache used to mean an up-front sweep that indexed the workspace
        list as it was *before* the command, and the post-command refresh then
        had to race it.
        """
        mock_devpod.return_value = MagicMock(returncode=0, stdout='{"state": "Stopped"}')
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch.object(sys, "argv", ["dl", "myws", "stop"]):
            assert main() == 0
        mock_popen.assert_called_once()
        assert mock_popen.call_args[0][0][-1] == "--force"

    @patch("devlaunch.dl._get_clone_manager")
    @patch("devlaunch.dl.run_devpod")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_delete_forces_exactly_one_refresh(self, mock_popen, mock_devpod, _mock_mgr):
        mock_devpod.return_value = MagicMock(returncode=0, stdout='{"state": "Stopped"}')
        with patch.object(sys, "argv", ["dl", "myws", "rm"]):
            assert main() == 0
        mock_popen.assert_called_once()
        assert mock_popen.call_args[0][0][-1] == "--force"

    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.get_workspace_state", return_value="Running")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_attaching_to_a_running_workspace_refreshes_once(
        self, mock_popen, _mock_state, _mock_ssh
    ):
        with patch.object(sys, "argv", ["dl", "myws"]):
            assert main() == 0
        mock_popen.assert_called_once()

    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_starting_a_workspace_refreshes_once(self, mock_popen, _mock_state, mock_up, _mock_ssh):
        mock_up.return_value = MagicMock(returncode=0)
        with patch.object(sys, "argv", ["dl", "myws"]):
            assert main() == 0
        mock_popen.assert_called_once()

    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.run_devpod")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_restart_stops_and_starts_but_still_refreshes_once(
        self, mock_popen, mock_devpod, mock_up, _mock_ssh
    ):
        """The old code spawned twice here: once up front and once from stop."""
        mock_devpod.return_value = MagicMock(returncode=0, stdout='{"state": "Stopped"}')
        mock_up.return_value = MagicMock(returncode=0)
        with patch.object(sys, "argv", ["dl", "myws", "restart"]):
            assert main() == 0
        mock_popen.assert_called_once()

    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_code_refreshes_once(self, mock_popen, _mock_state, mock_up):
        mock_up.return_value = MagicMock(returncode=0)
        with patch.object(sys, "argv", ["dl", "myws", "code"]):
            assert main() == 0
        mock_popen.assert_called_once()

    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_recreate_refreshes_once(self, mock_popen, _mock_state, mock_up, _mock_ssh):
        mock_up.return_value = MagicMock(returncode=0)
        with patch.object(sys, "argv", ["dl", "myws", "recreate"]):
            assert main() == 0
        mock_popen.assert_called_once()

    @patch("devlaunch.dl.workspace_ssh", return_value=0)
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.get_workspace_state", return_value="Stopped")
    @patch("devlaunch.dl.subprocess.Popen")
    def test_reset_refreshes_once(self, mock_popen, _mock_state, mock_up, _mock_ssh):
        mock_up.return_value = MagicMock(returncode=0)
        with patch.object(sys, "argv", ["dl", "myws", "reset"]):
            assert main() == 0
        mock_popen.assert_called_once()

    # Both answers are needed to reject a bare name: devpod cannot describe it
    # *and* does not list it. Either one alone is not a missing workspace.
    @patch("devlaunch.dl.get_workspace_ids", return_value=[])
    @patch("devlaunch.dl.get_workspace_state", return_value=None)
    @patch("devlaunch.dl.subprocess.Popen")
    def test_a_rejected_workspace_spec_spawns_nothing(self, mock_popen, _mock_state, _mock_ids):
        """A spec dl refuses to act on changed nothing worth re-indexing."""
        _age_completion_cache(COMPLETION_CACHE_TTL_SECONDS + 60)
        with patch.object(sys, "argv", ["dl", "nonexistent"]):
            assert main() == 1
        mock_popen.assert_not_called()


class TestDotfilesUpdate:
    """Tests for dotfiles_update()."""

    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_context_options")
    def test_runs_chezmoi_update_command(self, mock_ctx, mock_ssh):
        """dotfiles_update runs chezmoi update + pixi global sync via SSH."""
        mock_ctx.return_value = {"DOTFILES_URL": "https://github.com/user/dots"}
        mock_ssh.return_value = 0
        result = dotfiles_update("myws")
        assert result == 0
        mock_ssh.assert_called_once()
        cmd = mock_ssh.call_args[1]["command"]
        assert "chezmoi update --force" in cmd
        assert "pixi global sync" in cmd

    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_context_options")
    def test_fallback_includes_dotfiles_url(self, mock_ctx, mock_ssh):
        """Fallback clone uses DOTFILES_URL from context."""
        mock_ctx.return_value = {"DOTFILES_URL": "https://github.com/user/dots"}
        mock_ssh.return_value = 0
        dotfiles_update("myws")
        cmd = mock_ssh.call_args[1]["command"]
        assert "https://github.com/user/dots" in cmd

    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_context_options")
    def test_fallback_quotes_the_url(self, mock_ctx, mock_ssh):
        """The URL is interpolated into a shell command, so it is quoted.

        DOTFILES_URL comes from local devpod config rather than anywhere
        hostile, but a URL containing a space would otherwise split into two
        `git clone` arguments and fail with a message about the wrong thing.
        """
        mock_ctx.return_value = {"DOTFILES_URL": "https://example.com/a repo"}
        mock_ssh.return_value = 0
        dotfiles_update("myws")
        cmd = mock_ssh.call_args[1]["command"]
        assert "git clone 'https://example.com/a repo'" in cmd

    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_context_options")
    def test_a_typed_refresh_is_given_no_deadline(self, mock_ctx, mock_ssh):
        """`dl <ws> dotfiles` runs to completion however long it takes.

        The deadline exists for the refresh nobody asked for, which sits in
        front of a shell somebody is waiting on. A refresh somebody typed is in
        the foreground and interruptible, and a first `pixi global sync` on a
        full manifest is legitimately slow -- killing that at some fixed second
        would abandon a half-finished sync to save nobody any time.
        """
        mock_ctx.return_value = {}
        mock_ssh.return_value = 0
        dotfiles_update("myws")
        assert "timeout" not in mock_ssh.call_args[1]["command"]

    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_context_options")
    def test_a_bounded_refresh_still_carries_the_whole_payload(self, mock_ctx, mock_ssh):
        """A deadline wraps the refresh rather than replacing part of it.

        The bound goes around everything, not around the `chezmoi update` alone:
        the fallback `git clone` reaches the network too, and three separate
        deadlines would bound each step while leaving the trip as a whole
        unbounded at three times the number written down.
        """
        mock_ctx.return_value = {"DOTFILES_URL": "https://github.com/user/dots"}
        mock_ssh.return_value = 0
        dotfiles_update("myws", timeout=17)
        cmd = mock_ssh.call_args[1]["command"]
        assert cmd.startswith("timeout 17 ")
        assert "chezmoi update --force" in cmd
        assert "pixi global sync" in cmd
        assert "https://github.com/user/dots" in cmd

    @patch("devlaunch.dl.workspace_ssh")
    @patch("devlaunch.dl.get_context_options")
    def test_no_dotfiles_url_fallback_exits(self, mock_ctx, mock_ssh):
        """Without DOTFILES_URL, fallback reports error."""
        mock_ctx.return_value = {}
        mock_ssh.return_value = 0
        dotfiles_update("myws")
        cmd = mock_ssh.call_args[1]["command"]
        assert "no DOTFILES_URL configured" in cmd


class TestMainDotfilesSubcommand:
    """Tests for dotfiles subcommand in main()."""

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.dotfiles_update")
    def test_dotfiles_running_workspace(self, mock_dotfiles, mock_state, mock_ids):
        """dotfiles subcommand on running workspace skips workspace_up."""
        mock_ids.return_value = ["myws"]
        mock_state.return_value = "Running"
        mock_dotfiles.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "dotfiles"]):
            result = main()
        assert result == 0
        mock_dotfiles.assert_called_once_with("myws")

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    @patch("devlaunch.dl.dotfiles_update")
    def test_dotfiles_stopped_workspace_starts_first(
        self, mock_dotfiles, mock_up, mock_state, mock_ids
    ):
        """dotfiles subcommand on stopped workspace starts it first."""
        mock_ids.return_value = ["myws"]
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=0)
        mock_dotfiles.return_value = 0
        with patch.object(sys, "argv", ["dl", "myws", "dotfiles"]):
            result = main()
        assert result == 0
        mock_up.assert_called_once()
        mock_dotfiles.assert_called_once_with("myws")

    @patch("devlaunch.dl.get_workspace_ids")
    @patch("devlaunch.dl.get_workspace_state")
    @patch("devlaunch.dl.workspace_up")
    def test_dotfiles_start_failure_propagates(self, mock_up, mock_state, mock_ids):
        """dotfiles subcommand propagates workspace_up failure."""
        mock_ids.return_value = ["myws"]
        mock_state.return_value = "Stopped"
        mock_up.return_value = MagicMock(returncode=3)
        with patch.object(sys, "argv", ["dl", "myws", "dotfiles"]):
            result = main()
        assert result == 3
