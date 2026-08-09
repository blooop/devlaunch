# pylint: disable=redefined-outer-name
"""Pin how many `devpod` processes each hot dl command spawns.

Measured on the reference machine: `devpod list --output json` costs 0.44s and
`devpod status <ws> --output json` 0.56s, against 0.09s for the whole of Python
startup (`dl --help`). Every round-trip dl makes is therefore worth ~5x the cost
of the interpreter, and the only way to make dl faster is to make fewer of them.

These tests assert the exact sequence of devpod invocations, counted at the
subprocess boundary rather than at run_devpod, so a change that reintroduces a
redundant round-trip fails here instead of quietly costing half a second.
"""

import io
import json
import subprocess
import sys
from typing import List, Optional
from unittest.mock import patch

import pytest

from devlaunch.dl import (
    DevpodNotInstalled,
    UnreadableWorkspaceList,
    attach_workspace,
    invalidate_workspace_list_cache,
    list_workspaces,
    main,
    purge_all_data,
    workspace_delete,
    workspace_stop,
    workspace_up,
)


class FinishedSession:
    """A `devpod ssh` session that wrote nothing to stderr and exited 0.

    Enough of Popen for run_devpod_session: it is used as a context manager and
    the only thing read out of it is the stderr pipe, which under a pty carries
    devpod's own warnings and errors and nothing else.
    """

    def __init__(self, argv: List[str], stderr: str = ""):
        self.args = argv
        self.stderr = io.StringIO(stderr)
        self.returncode = 0

    def __enter__(self) -> "FinishedSession":
        return self

    def __exit__(self, *_exc) -> bool:
        return False


class DevpodSpawns:
    """Stands in for subprocess.run and records every devpod command line.

    Answers the read-only devpod commands dl makes (list/status/context
    options) from in-memory state so a whole CLI invocation can run without a
    devpod binary, and reports the devpod spawns in order.

    A session attach spawns through Popen rather than run, so `popen` records
    into the same list: what these tests count is devpod processes, not which
    subprocess primitive started them.
    """

    def __init__(self, workspace_ids: List[str], state: str = "Running"):
        self.workspace_ids = workspace_ids
        self.state = state
        # Workspaces devpod lists but cannot `status` — a provider that is
        # broken, reconfigured or removed. The two answers really can differ,
        # which is why dl asks the second question before refusing.
        self.undescribable: set = set()
        self.commands: List[List[str]] = []
        # Where the listed workspaces' sources sit. `dl --purge` deletes only
        # the ones under devlaunch's own cache directory, so a test that wants
        # a workspace purge will act on points this at the cache dir it patched.
        self.source_root = "/cache"

    def __call__(self, cmd, *_args, **_kwargs) -> subprocess.CompletedProcess:
        argv = list(cmd)
        self.commands.append(argv)
        stdout = ""
        returncode = 0
        if argv[:1] == ["devpod"]:
            stdout = self._devpod_stdout(argv[1:])
            # Real devpod exits non-zero for `status` on a workspace it does
            # not have, and dl now reads that answer as "not found" -- a fake
            # that succeeds for every id would hide the cold path entirely.
            if argv[1:2] == ["status"] and (
                argv[2] not in self.workspace_ids or argv[2] in self.undescribable
            ):
                stdout = ""
                returncode = 1
        return subprocess.CompletedProcess(
            args=argv, returncode=returncode, stdout=stdout, stderr=""
        )

    def popen(self, cmd, *_args, **_kwargs) -> FinishedSession:
        """Stand in for the Popen behind an interactive or one-shot session."""
        argv = list(cmd)
        self.commands.append(argv)
        return FinishedSession(argv)

    def _devpod_stdout(self, args: List[str]) -> str:
        if args[:1] == ["list"]:
            return json.dumps(
                [
                    {
                        "id": ws,
                        "source": {"localFolder": f"{self.source_root}/{ws}"},
                        "provider": {"name": "docker"},
                        "ide": {"name": "none"},
                        "lastUsed": "2026-01-01T00:00:00Z",
                    }
                    for ws in self.workspace_ids
                ]
            )
        if args[:1] == ["status"]:
            return json.dumps({"id": args[1], "state": self.state})
        if args[:2] == ["context", "options"]:
            return json.dumps({})
        return ""

    @property
    def devpod_commands(self) -> List[List[str]]:
        """Every devpod invocation, without the leading `devpod`, in order."""
        return [argv[1:] for argv in self.commands if argv[:1] == ["devpod"]]

    @property
    def count(self) -> int:
        """How many devpod processes were spawned."""
        return len(self.devpod_commands)


@pytest.fixture
def spawns():
    """A recorder for devpod spawns, with dl's background updater disabled.

    update_cache_background is out of the way because it spawns a second dl
    process, whose own devpod calls belong to that process, not this one.
    """
    recorder = DevpodSpawns(["myws"])
    with patch("devlaunch.dl.subprocess.run", side_effect=recorder):
        with patch("devlaunch.dl.subprocess.Popen", side_effect=recorder.popen):
            with patch("devlaunch.dl.update_cache_background"):
                yield recorder


def _run_dl(*argv: str) -> int:
    with patch.object(sys, "argv", ["dl", *argv]):
        return main()


class TestHotCommandSpawnCounts:
    """The three commands the ticket measures, plus the one-shot command path."""

    def test_ls_reads_the_workspace_list_exactly_once(self, spawns):
        """`dl --ls` is one round-trip and nothing else."""
        assert _run_dl("--ls") == 0
        assert spawns.devpod_commands == [["list", "--output", "json"]]

    def test_ls_with_sizes_spawns_nothing_extra(self, spawns, tmp_path, capsys):
        """`dl --ls --size` costs a filesystem walk and not one more process.

        Sizes come from `lstat`, never from `du` and never from docker: adding a
        subprocess per workspace to a listing command is the thing this file
        exists to catch.

        The workspace has to be one dl would actually measure, or this guards
        nothing: a source outside the cache is left unmeasured by design, the
        walk never runs, and a `du` per workspace sails past. So the clone is
        put under the patched cache directory with a payload in it, and the
        rendered size is asserted before the spawn counts -- a `-` in that cell
        means the counts below are counting a code path that did not execute.
        """
        cache_dir = tmp_path / "devlaunch"
        clone = cache_dir / "repos" / "myws"
        clone.mkdir(parents=True)
        (clone / "payload.bin").write_bytes(b"\0" * 2 * 1024 * 1024)
        spawns.source_root = str(cache_dir / "repos")
        with patch("devlaunch.dl._get_cache_dir", return_value=cache_dir):
            assert _run_dl("--ls", "--size") == 0
        assert "2.0 MiB" in capsys.readouterr().out
        assert spawns.devpod_commands == [["list", "--output", "json"]]
        assert spawns.commands == [["devpod", "list", "--output", "json"]]

    def test_purge_reads_the_workspace_list_exactly_once(self, spawns, tmp_path):
        """`dl --purge -y` used to read the list twice: once to print the count
        it asks the user to confirm, once again inside purge_all_data.

        Nothing between those two reads changes what devpod would say, so the
        second one was a wasted 0.45s — and worse, the count the user confirmed
        and the set actually deleted could differ.
        """
        spawns.workspace_ids = ["ws1", "ws2"]
        cache_dir = tmp_path / "devlaunch"
        cache_dir.mkdir()
        # Both are clones devlaunch made, so both are its to delete.
        spawns.source_root = str(cache_dir / "repos")
        with patch("devlaunch.dl._get_cache_dir", return_value=cache_dir):
            assert _run_dl("--purge", "-y") == 0
        assert spawns.devpod_commands == [
            ["list", "--output", "json"],
            ["delete", "ws1", "--force"],
            ["delete", "ws2", "--force"],
        ]

    def test_attaching_to_a_running_workspace(self, spawns):
        """The interactive attach chain, pinned call by call.

        One `status` answers both questions dl has -- does devpod know this
        workspace, and is it running -- so the `list` that used to precede it
        is gone. The hostname round-trip is kept deliberately: bash reads the
        hostname when the shell starts, so it has to be set before the session
        dl hands over, and devpod ssh has no hook inside that session to fold
        it into.
        """
        assert _run_dl("myws") == 0
        assert spawns.devpod_commands == [
            ["status", "myws", "--output", "json"],
            ["ssh", "myws", "--command", "sudo hostname myws"],
            ["ssh", "myws"],
        ]

    def test_two_commands_in_one_process_each_read_the_list(self, spawns):
        """The snapshot is scoped to a command, not to the interpreter.

        dl normally is one process per command, but anything that drives main()
        twice — a test, a wrapper — must not have the first command's view of
        devpod answering the second command's questions.
        """
        assert _run_dl("--ls") == 0
        assert _run_dl("--ls") == 0
        assert spawns.devpod_commands == [["list", "--output", "json"]] * 2

    def test_a_one_shot_command_skips_the_hostname_round_trip(self, spawns):
        """`dl <ws> -- cmd` renders no prompt, so the hostname buys nothing."""
        assert _run_dl("myws", "--", "echo", "hi") == 0
        assert spawns.devpod_commands == [
            ["status", "myws", "--output", "json"],
            ["ssh", "myws", "--command", "bash -lc 'echo hi'"],
        ]

    def test_a_git_spec_one_shot_on_a_running_workspace(self, spawns):
        """The launcher path: `dl owner/repo@branch -- cmd`, workspace warm.

        This is the exact shape wayfinder hands dl for every agent launch, so
        its overhead is what sits between picking a ticket and a running
        agent. Resolving the spec must cost one `status` -- membership and
        state are the same answer -- and the command itself one `ssh`.
        """
        from devlaunch.workspace_id import WorkspaceId

        ws_id = WorkspaceId("blooop", "devlaunch", "wayfinder/devlaunch-7").value
        spawns.workspace_ids = [ws_id]
        assert _run_dl("blooop/devlaunch@wayfinder/devlaunch-7", "--", "echo", "hi") == 0
        assert spawns.devpod_commands == [
            ["status", ws_id, "--output", "json"],
            ["ssh", ws_id, "--command", "bash -lc 'echo hi'"],
        ]

    def test_a_warm_git_spec_launch_does_no_metadata_io(self, spawns):
        """A warm launch never opens metadata.json and never takes its lock.

        The clone manager is cold-path machinery: it reads config.toml, loads
        metadata.json under the metadata lock, and runs the cache migration.
        A launch that attaches to a workspace devpod already reports as
        Running uses none of that, so it must pay for none of it (#145).

        No mocks at the storage layer — the seam is observed on disk. The
        cache is seeded with an unparsable metadata.json: any code path that
        reads it quarantines it to metadata.json.corrupt and warns, and any
        path that takes the metadata lock leaves metadata.json.lock behind.
        A launch that did no metadata I/O leaves the garbage byte-identical
        and creates neither sibling file.
        """
        from devlaunch.workspace_id import WorkspaceId
        from devlaunch.xdg import devlaunch_cache

        ws_id = WorkspaceId("blooop", "devlaunch", "wayfinder/devlaunch-7").value
        spawns.workspace_ids = [ws_id]
        cache = devlaunch_cache()
        cache.mkdir(parents=True, exist_ok=True)
        marker = cache / "metadata.json"
        marker.write_text("not json", encoding="utf-8")

        assert _run_dl("blooop/devlaunch@wayfinder/devlaunch-7", "--", "echo", "hi") == 0

        assert marker.read_text(encoding="utf-8") == "not json"
        assert not (cache / "metadata.json.corrupt").exists()
        assert not (cache / "metadata.json.lock").exists()

    def test_an_unknown_bare_name_is_refused_after_asking_devpod_twice(self, spawns):
        """A typo'd workspace name costs a status and then a listing.

        The listing is the second opinion, and it is only ever paid on this
        path. `status` consults the provider while `list` reads devpod's own
        records, so a workspace whose provider is broken or gone still lists
        and cannot be described — and that is exactly the workspace somebody
        is about to `dl <ws> rm`. Refusing on the status alone would be a
        wrong diagnosis and would block the command that fixes it.
        """
        assert _run_dl("no-such-ws") == 1
        assert spawns.devpod_commands == [
            ["status", "no-such-ws", "--output", "json"],
            ["list", "--output", "json"],
        ]

    def test_a_bare_name_devpod_lists_but_cannot_describe_is_still_usable(self, spawns):
        """The reason the listing gets the final word: this workspace exists,
        and `rm` is the whole point of reaching for it."""
        spawns.workspace_ids = ["broken-ws"]
        spawns.undescribable = {"broken-ws"}
        with patch("devlaunch.dl._get_clone_manager"):
            assert _run_dl("broken-ws", "rm", "--force") == 0
        assert ["delete", "broken-ws"] in spawns.devpod_commands


class TestWorkspaceListMemoization:
    """list_workspaces is read from 6+ places; it may cost one spawn per process."""

    def test_a_second_read_costs_nothing(self, spawns):
        first = list_workspaces()
        second = list_workspaces()
        assert [ws.id for ws in first] == [ws.id for ws in second] == ["myws"]
        assert spawns.count == 1

    def test_refresh_forces_a_fresh_read(self, spawns):
        list_workspaces()
        spawns.workspace_ids = ["myws", "other"]
        assert [ws.id for ws in list_workspaces(refresh=True)] == ["myws", "other"]
        assert spawns.count == 2

    def test_invalidating_forces_a_fresh_read(self, spawns):
        list_workspaces()
        invalidate_workspace_list_cache()
        list_workspaces()
        assert spawns.count == 2

    def test_the_snapshot_cannot_be_mutated_by_a_caller(self, spawns):
        """A caller that edits the list it got back must not edit the cache.

        Both the read that fills the cache and every read served from it hand
        back a copy, so this clears the result of each in turn.
        """
        fills_the_cache = list_workspaces()
        fills_the_cache.clear()
        served_from_the_cache = list_workspaces()
        assert [ws.id for ws in served_from_the_cache] == ["myws"]
        served_from_the_cache.clear()
        assert [ws.id for ws in list_workspaces()] == ["myws"]
        assert spawns.count == 1

    def test_a_mutating_command_drops_the_snapshot(self, spawns):
        """A read after dl itself changed a workspace must see the change."""
        assert [ws.id for ws in list_workspaces()] == ["myws"]
        spawns.workspace_ids = []
        workspace_stop("myws")
        assert list_workspaces() == []

    def test_starting_a_workspace_drops_the_snapshot(self, spawns):
        """`up` is the one that can add a workspace the snapshot never had."""
        assert [ws.id for ws in list_workspaces()] == ["myws"]
        spawns.workspace_ids = ["myws", "brand-new"]
        workspace_up("brand-new", workspace_id="brand-new")
        assert [ws.id for ws in list_workspaces()] == ["myws", "brand-new"]

    def test_deleting_a_workspace_drops_the_snapshot(self, spawns):
        assert [ws.id for ws in list_workspaces()] == ["myws"]
        spawns.workspace_ids = []
        with patch("devlaunch.dl._get_clone_manager"):
            workspace_delete("myws")
        assert list_workspaces() == []

    def test_purging_drops_the_snapshot(self, spawns, tmp_path):
        """Nothing reads the list after a purge today, and nothing may inherit
        the workspaces the purge just deleted if something starts to."""
        cache_dir = tmp_path / "devlaunch"
        cache_dir.mkdir()
        # A clone devlaunch made, so the purge has something of its own to
        # delete -- a purge that deleted nothing would drop no snapshot.
        spawns.source_root = str(cache_dir / "repos")
        assert [ws.id for ws in list_workspaces()] == ["myws"]
        spawns.workspace_ids = []
        with patch("devlaunch.dl._get_cache_dir", return_value=cache_dir):
            assert purge_all_data() == 0
        assert list_workspaces() == []

    def test_a_failed_read_is_not_remembered_as_an_empty_list(self):
        """devpod exiting non-zero is not an answer, so it is neither returned
        as one nor cached as one: every read asks again, and every read says so."""
        calls: List[List[str]] = []

        def failing(cmd, *_args, **_kwargs):
            calls.append(list(cmd))
            return subprocess.CompletedProcess(args=list(cmd), returncode=1, stdout="", stderr="")

        with patch("devlaunch.dl.subprocess.run", side_effect=failing):
            for _ in range(2):
                with pytest.raises(UnreadableWorkspaceList):
                    list_workspaces()
        assert len(calls) == 2

    def test_an_honestly_empty_list_is_remembered(self, spawns):
        spawns.workspace_ids = []
        assert list_workspaces() == []
        assert list_workspaces() == []
        assert spawns.count == 1


class TestMemoizationCannotHideAMissingDevpod:
    """The cache must not turn "devpod is not installed" into "no workspaces"."""

    @staticmethod
    def _missing() -> FileNotFoundError:
        return FileNotFoundError(2, "No such file or directory", "devpod")

    def test_every_read_still_raises(self):
        with patch("devlaunch.dl.subprocess.run", side_effect=self._missing()):
            for _ in range(2):
                with pytest.raises(DevpodNotInstalled):
                    list_workspaces()

    def test_a_cached_empty_list_does_not_survive_into_a_missing_binary(self, spawns):
        """Read an honest empty list, then lose the binary: dl must still say so."""
        spawns.workspace_ids = []
        assert list_workspaces() == []
        invalidate_workspace_list_cache()
        with patch("devlaunch.dl.subprocess.run", side_effect=self._missing()):
            with pytest.raises(DevpodNotInstalled):
                list_workspaces()

    @patch("devlaunch.dl.update_cache_background")
    def test_ls_still_exits_127_with_one_line(self, _cache, capsys):
        with patch("devlaunch.dl.subprocess.run", side_effect=self._missing()):
            assert _run_dl("--ls") == 127
        captured = capsys.readouterr()
        assert captured.out == ""
        assert captured.err.strip().count("\n") == 0


class TestAttachHelper:
    """attach_workspace is the one place that decides the hostname round-trip."""

    @staticmethod
    def _hostname_calls(shell_command: Optional[str]) -> int:
        """How many hostname round-trips one attach costs."""
        with patch("devlaunch.dl.setup_hostname") as hostname:
            with patch("devlaunch.dl.workspace_ssh", return_value=0) as ssh:
                assert attach_workspace("myws", shell_command) == 0
        ssh.assert_called_once_with("myws", shell_command)
        return hostname.call_count

    def test_an_interactive_attach_sets_the_hostname(self):
        assert self._hostname_calls(None) == 1

    def test_a_one_shot_command_does_not(self):
        assert self._hostname_calls("echo hi") == 0
