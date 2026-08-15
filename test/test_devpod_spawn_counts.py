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

import inspect
import io
import json
import pathlib
import shlex
import subprocess
import sys
import traceback
from typing import List, Optional, Union
from unittest.mock import patch

import pytest

from devlaunch import dl as dl_module
from devlaunch import tools
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


# Captured before anything is patched, because patching `devlaunch.dl.subprocess`
# patches the subprocess *module* -- dl imports the module rather than a name out
# of it, so there is only one `subprocess.run` in the process and a stub put
# there answers every module's calls, not just dl's.
_real_run = subprocess.run
_real_popen = subprocess.Popen

DEVPOD_REACHED_THE_REAL_RUN = (
    "a devpod argv reached the real subprocess.run -- the recorder should have "
    "answered it. Something called subprocess.run through a reference the "
    "fixture's patch cannot reach (a binding captured before it was applied, "
    "such as a `run=subprocess.run` default argument), so the recorder never "
    "saw the call, and the real run built its process out of the patched "
    "Popen. Whatever devpod spawn is in the traceback below is unrecorded and "
    "uncounted, which is the whole of what this file measures."
)


def _refuse_a_devpod_argv_inside_the_real_run(argv: List[str]) -> None:
    """Fail by name when a devpod argv arrives here from inside a real `run`.

    Left alone this is not a test failure anybody can read. `subprocess.run`
    resolves `Popen` from the subprocess module's own globals, which is exactly
    what this fixture patches, so a real `run` reached with a devpod argv gets
    a `FinishedSession` back -- a session object with no process behind it.
    `run` then calls `process.communicate(...)`, which FinishedSession does not
    have; the bare `except:` that exists to re-raise that runs `process.kill()`
    first, which it also does not have, and the AttributeError from the cleanup
    buries the one that explains it. What comes out is
    `AttributeError: 'FinishedSession' object has no attribute 'kill'` raised
    from CPython's subprocess.py, naming neither devpod nor this fixture. It
    was seen once in ~90 runs while this fixture was under review, and no
    reader of that traceback would have found their way back to here.

    The recorder cannot open that door itself: `__call__` hands `_real_run`
    only an argv it has already tested as non-devpod, and `popen` applies the
    identical test to the identical argv, so a pass-through cannot round-trip
    into a FinishedSession. It takes a caller holding a reference to
    `subprocess.run` that patching the module attribute does not reach. The
    tree's one known shape of those -- `devlaunch/devpod_provider.py` binding
    `run: Callable[...] = subprocess.run` as a *default argument* on
    `list_provider_names`, `ensure_provider` and `main`, evaluated once at
    import and so permanently wired to the real run -- was closed by #217,
    which resolves `run=None` at call time. The guard stays because it names
    the *class*, not that instance: any future caller holding its own
    reference to the real run re-opens the door, and "rare" is what a flake
    is made of.

    So the leak is named where it happens instead of being left to surface as a
    missing attribute two libraries away. It also fails *before* the real `run`
    can build anything, so there is no half-spawned process to clean up.
    """
    frame = inspect.currentframe()
    while frame is not None:
        if frame.f_code is _real_run.__code__:
            raise AssertionError(
                f"{DEVPOD_REACHED_THE_REAL_RUN}\n\nargv: {argv}\n\n"
                + "".join(traceback.format_stack())
            )
        frame = frame.f_back


class DevpodSpawns:
    """Stands in for subprocess.run and records every devpod command line.

    Answers the read-only devpod commands dl makes (list/status/context
    options) from in-memory state so a whole CLI invocation can run without a
    devpod binary, and reports the devpod spawns in order.

    **Only devpod.** Every other command runs for real, and that is a
    correctness property rather than a convenience: the clone classification
    behind `--prune` is a real `git status` and a real `git log --not
    --remotes`, and a stub that answered them too would hand back a clean exit
    with empty output -- which reads as "this clone holds nothing", which is
    the answer that removes it. A fixture earning its removal that way models a
    deletion the shipped code will not perform (devlaunch#184).

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

    def __call__(self, cmd, *args, **kwargs) -> subprocess.CompletedProcess:
        argv = list(cmd)
        # Recorded before the pass-through, not after the devpod test, so that
        # `commands` is every spawn and not merely every devpod spawn. The
        # difference is the whole value of the strict-equality assertion in
        # test_ls_with_sizes_spawns_nothing_extra: a recorder that only ever
        # holds devpod entries makes `commands == [["devpod", ...]]` a
        # restatement of the `devpod_commands` assertion above it, and the one
        # guard in this file against a *non-devpod* spawn -- a `du` per
        # workspace on a listing command -- stops guarding anything.
        self.commands.append(argv)
        if argv[:1] != ["devpod"]:
            # pylint: disable=subprocess-run-check  # the caller's own kwargs carry it
            return _real_run(cmd, *args, **kwargs)
        returncode = 0
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

    def popen(self, cmd, *args, **kwargs) -> Union["FinishedSession", subprocess.Popen]:
        """Stand in for the Popen behind an interactive or one-shot session.

        Passes non-devpod commands through for the same reason `__call__` does,
        and it is not only for symmetry: `subprocess.run` is itself written in
        terms of `Popen`, so a passed-through `git status` arrives back here on
        its way to the real binary and would otherwise be answered by a session
        object that has no process behind it.

        A pass-through records nothing, because `__call__` already recorded the
        argv on its way past: the two would otherwise log one `git status`
        twice. Only a devpod argv, which reaches Popen directly and never
        through `__call__`, is recorded here.
        """
        argv = list(cmd)
        if argv[:1] != ["devpod"]:
            return _real_popen(cmd, *args, **kwargs)
        _refuse_a_devpod_argv_inside_the_real_run(argv)
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

    It is also the first thing suspected whenever this file flakes, and it is
    worth writing down that it cannot be the culprit for the class of flake
    `_refuse_a_devpod_argv_inside_the_real_run` guards. Every test here that
    drives main() takes this fixture, so the updater is scrubbed on all of
    them; and even unscrubbed it spawns a *detached* `python -m devlaunch.dl
    --update-cache`, a separate process whose devpod spawns and exceptions both
    stay in that process. A leak into *this* process's `subprocess.run` has to
    come from a caller in this process holding a pre-patch reference to it --
    see that guard for what those are and why the failure is unreadable
    without it.
    """
    recorder = DevpodSpawns(["myws"])
    with patch("devlaunch.dl.subprocess.run", side_effect=recorder):
        with patch("devlaunch.dl.subprocess.Popen", side_effect=recorder.popen):
            with patch("devlaunch.dl.update_cache_background"):
                yield recorder


def _run_dl(*argv: str) -> int:
    with patch.object(sys, "argv", ["dl", *argv]):
        return main()


def _git(*args: str, cwd: pathlib.Path) -> None:
    """Run git for real, failing the test with git's own words if it refuses."""
    result = _real_run(["git", *args], cwd=cwd, capture_output=True, text=True, check=False)
    assert result.returncode == 0, f"git {' '.join(args)}: {result.stderr}"


def _fully_pushed_clone(clone: pathlib.Path, scratch: pathlib.Path) -> pathlib.Path:
    """A real clone at *clone* that git will say holds nothing worth keeping.

    A local path is a real git remote, so a clone made this way has a genuine
    remote-tracking ref and `git log --oneline main --not --remotes` is empty
    because the commit really is on the remote -- which is the state
    `--prune` removes a directory for. An empty directory with a file in it
    is not that state and is not removable: git cannot read it as a
    repository, so it classifies as work that could not be asked about and is
    kept unless `--force` says otherwise (devlaunch#184).
    """
    seed = scratch / "seed"
    seed.mkdir()
    _git("init", "-b", "main", ".", cwd=seed)
    (seed / "README.md").write_text("seed\n", encoding="utf-8")
    _git("add", "-A", cwd=seed)
    _git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "seed", cwd=seed)
    origin = scratch / "origin.git"
    _git("clone", "--bare", str(seed), str(origin), cwd=scratch)
    clone.parent.mkdir(parents=True, exist_ok=True)
    _git("clone", str(origin), str(clone), cwd=scratch)
    return clone


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

    def test_prune_lists_workspaces_once_per_pass_and_asks_no_status(self, spawns, tmp_path):
        """`dl --prune` costs one `devpod list` per pass and nothing else.

        A workspace's *state* has no bearing on whether a clone directory is
        referenced -- a stopped workspace still opens the one it was made from
        -- so a `devpod status` per workspace here would be half a second each
        to learn something the answer does not depend on. That is the cost this
        pins, and the reason this command is never on a launch path and never on
        `--ls`: the whole scan was measured at 1017 ms on the reference host,
        most of it the listing and the git probes, and it gets slower exactly as
        the cache it is for gets fuller.

        Two listings, not one, and the second is not an oversight. Everything a
        plan rests on can move while the user is reading it, and whether a clone
        became *referenced* is the one thing that cannot be re-derived from
        disk: a launch that completes in that window registers a workspace for
        the very directory in the plan, because the clone path for
        `(owner, repo, branch)` is deterministic. One extra O(1) listing, paid
        only after somebody has said yes to a deletion, buys the re-check.

        The clone goes under the cache dl actually resolves, and the assertion
        that it was removed comes first: a fixture whose directories sit
        somewhere the command never looks would leave these counts describing a
        scan that classified nothing at all.

        It is a real clone with a real remote, and that is what makes the
        removal mean anything. Built as a bare directory with a file in it, it
        was removed only because the recorder was answering git as well as
        devpod -- clean exit, empty output, read as "holds nothing". The
        shipped code classifies that directory `CouldNotTell` and keeps it, so
        the counts below were being taken over a scan that removed something
        `dl --prune` would not (devlaunch#184).
        """
        from devlaunch.xdg import devlaunch_cache

        stale = _fully_pushed_clone(
            devlaunch_cache() / "repos" / "o" / "r" / "r-main-abcdefgh", tmp_path
        )
        assert _run_dl("--prune", "-y") == 0
        assert not stale.exists()
        assert spawns.devpod_commands == [
            ["list", "--output", "json"],
            ["list", "--output", "json"],
        ]

    def test_prune_lists_nothing_twice_when_there_is_nothing_to_do(self, spawns):
        """The second listing belongs to the pass that acts, so a run that does
        not act does not pay for it -- `dl --prune` answered `n`, or a cache
        with nothing in it, is one listing."""
        assert _run_dl("--prune", "-y") == 0
        assert spawns.devpod_commands == [["list", "--output", "json"]]

    def test_attaching_to_a_running_workspace(self, spawns):
        """The interactive attach chain, pinned call by call: two trips.

        One `status` answers both questions dl has -- does devpod know this
        workspace, and is it running -- so the `list` that used to precede it
        is gone. The hostname round-trip that used to sit between the status and
        the session is gone too, and it did not merely die: naming the container
        is a stage of the setup pass every entry into Running already pays
        (#167), so a Running workspace arrives here already named and a trip
        here would be a second ~1.7s spent re-answering that.
        """
        assert _run_dl("myws") == 0
        assert spawns.devpod_commands == [
            ["status", "myws", "--output", "json"],
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
        assert ["delete", "broken-ws", "--ignore-not-found"] in spawns.devpod_commands


class TestColdLaunchSpawnCounts:
    """The cold path's trips, which had no pin here at all until #168.

    Every pin above this is warm, which is why the cold trip count could move
    without anything failing — and it is the path with the trips on it: the
    warm chain is a `status` and a session, while a cold one runs `devpod up`
    and then talks to the container it built.

    Two things are stubbed, and both would otherwise make this assert a
    different sequence per developer rather than a property of dl:

    - **`host_payload`**, because what a host has to lend decides which arm the
      setup takes. A machine with a claude and a gh takes the transfer; a
      machine with neither takes the network install instead.
    - **the GitHub token**, because `devpod up` carries a
      `--workspace-env-file` flag only on a host that is logged in, and the
      path in it is a fresh temporary file every run.
    """

    @staticmethod
    def _payload(tmp_path) -> tools.HostPayload:
        """A lendable payload made of scratch files, not the host's real 300MB."""
        claude = tmp_path / "claude-2.0.1"
        claude.write_bytes(b"#!/bin/sh\n")
        gh = tmp_path / "gh"
        gh.write_bytes(b"\x7fELF")
        return tools.HostPayload(
            claude_version="2.0.1",
            members=((claude, ".local/share/claude/versions/2.0.1"), (gh, ".local/bin/gh")),
        )

    @staticmethod
    def _up(spawns):
        with patch("devlaunch.gh_auth.resolve_token", return_value=None):
            workspace_up("brand-new", workspace_id="brand-new", workspace_identity="brand-new")
        return spawns.devpod_commands

    def test_a_cold_launch_with_a_host_that_can_lend(self, spawns, tmp_path):
        """Two trips into the container, and the first one carries the naming.

        The `up` builds the container; the setup pass then asks what it has and
        names it in the same trip; the transfer lends the host's binaries. There
        is no third trip setting a hostname, which is the ~1.9s this ticket
        saves (#157, measured against real devpod against a real container).
        """
        with patch("devlaunch.tools.host_payload", return_value=self._payload(tmp_path)):
            commands = self._up(spawns)
        assert commands == [
            ["context", "options", "--output", "json"],
            [
                "up",
                "brand-new",
                "--id",
                "brand-new",
                "--ide",
                "none",
                "--init-env",
                "DEVLAUNCH_WORKSPACE_ID=brand-new",
            ],
            [
                "ssh",
                "brand-new",
                "--command",
                f"bash -lc {shlex.quote(tools.setup_script('brand-new'))}",
            ],
            [
                "ssh",
                "brand-new",
                "--command",
                f"bash -c {shlex.quote(tools.transfer_script(self._payload(tmp_path)))}",
            ],
        ]

    def test_a_cold_launch_with_nothing_to_lend(self, spawns):
        """The other arm, pinned for the same reason: the network install is a
        third trip, and the naming still costs none of its own."""
        with patch("devlaunch.tools.host_payload", return_value=None):
            commands = self._up(spawns)
        assert [argv[:1] for argv in commands] == [["context"], ["up"], ["ssh"], ["ssh"]]
        assert commands[2][3] == f"bash -lc {shlex.quote(tools.setup_script('brand-new'))}"
        assert commands[3][3] == f"bash -lc {shlex.quote(tools.provision_script())}"

    def test_no_trip_of_the_launch_is_a_hostname_of_its_own(self, spawns, tmp_path):
        """Stated as the absence it is, so a reintroduced trip fails here."""
        with patch("devlaunch.tools.host_payload", return_value=self._payload(tmp_path)):
            commands = self._up(spawns)
        assert not any("sudo hostname brand-new" in argv for argv in commands)
        assert tools.setup_script("brand-new").count("sudo hostname brand-new") == 1


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


class TestOptInDotfilesRefreshOnAttach:
    """What switching the dotfiles refresh on costs, and what leaving it off does not.

    The feature is measured here rather than anywhere else because the whole
    question it lost on the first time is a spawn count (#183). The pins in
    `TestHotCommandSpawnCounts` above are the off-by-default requirement: they
    run with the switch unset, they were not edited to accommodate this, and an
    unconditional refresh fails five of them. So the counts below are the *other*
    half — what the user who asked for it actually pays.
    """

    def test_an_opted_in_interactive_attach_refreshes_before_the_shell(self, spawns, monkeypatch):
        """Switched on, an interactive attach pays one refresh trip, then the session.

        The order is the feature: the point of refreshing on attach rather than
        on some later timer is that the shell being handed over is the one that
        reads the new dotfiles. A refresh that landed after the session started
        would be updating files the shell had already sourced.
        """
        monkeypatch.setenv("DEVLAUNCH_DOTFILES_ON_ATTACH", "1")
        assert _run_dl("myws") == 0
        heads = [argv[:2] for argv in spawns.devpod_commands]
        assert heads == [
            ["status", "myws"],
            ["context", "options"],
            ["ssh", "myws"],
            ["ssh", "myws"],
        ]
        refresh, session = spawns.devpod_commands[2], spawns.devpod_commands[3]
        assert refresh[2] == "--command"
        assert "chezmoi update" in refresh[3]
        assert session == ["ssh", "myws"]

    def test_the_refresh_trip_is_bounded(self, spawns, monkeypatch):
        """The refresh that crosses into the container carries its own deadline.

        An unreachable dotfiles remote must hand the shell over rather than sit
        in front of it, and nothing on dl's side bounds a `chezmoi update` that
        is blocked on a network read or a credential prompt -- tolerating a
        non-zero exit only helps once the command has decided to exit.

        The bound is asserted as a literal in the payload rather than by waiting
        out a real hang: a test that slept out a timeout would be measuring
        coreutils, slowly, and would still not prove dl had asked for one.
        """
        monkeypatch.setenv("DEVLAUNCH_DOTFILES_ON_ATTACH", "1")
        assert _run_dl("myws") == 0
        payload = spawns.devpod_commands[2][3]
        assert f"timeout {dl_module.DOTFILES_ATTACH_TIMEOUT_SECONDS}" in payload
        assert dl_module.DOTFILES_ATTACH_TIMEOUT_SECONDS > 0

    def test_a_one_shot_command_is_not_worth_a_refresh(self, spawns, monkeypatch):
        """Switched on, `dl <ws> -- cmd` still costs exactly status plus the command.

        Same reasoning the attach already applies to everything it does not do
        for a one-shot: the command renders no prompt and sources no interactive
        shell, so a refresh in front of it buys that command nothing and costs it
        a round-trip. This is the shape wayfinder hands dl for every agent
        launch, so it is the one that must not grow.
        """
        monkeypatch.setenv("DEVLAUNCH_DOTFILES_ON_ATTACH", "1")
        assert _run_dl("myws", "--", "echo", "hi") == 0
        assert spawns.devpod_commands == [
            ["status", "myws", "--output", "json"],
            ["ssh", "myws", "--command", "bash -lc 'echo hi'"],
        ]

    @pytest.mark.parametrize("setting", ["", "0", "false", "no", "FALSE", " no "])
    def test_a_switch_set_to_a_denial_is_still_off(self, spawns, monkeypatch, setting):
        """Only an affirmative value turns it on.

        A variable exported empty is what an unset variable looks like to a
        shell that mentions it, and `=0` is what someone who once turned it on
        writes to turn it back off. Reading mere presence as consent would make
        both of those a latency regression the user believed they had declined.

        The accepted denials are the vocabulary dl already reads for
        `DEVLAUNCH_NO_TTY` and `DEVLAUNCH_NO_GH_TOKEN`, case and surrounding
        space included, rather than a third spelling of the same idea. A user
        who has learned what dl treats as off should not have to learn it twice
        -- so the parametrization is deliberately that list and not a longer one
        containing, say, `off`, which those variables do not accept either.
        """
        monkeypatch.setenv("DEVLAUNCH_DOTFILES_ON_ATTACH", setting)
        assert _run_dl("myws") == 0
        assert spawns.devpod_commands == [
            ["status", "myws", "--output", "json"],
            ["ssh", "myws"],
        ]


class TestAttachHelper:
    """The attach is one trip now, and the same one trip either way.

    It used to branch: an interactive attach paid a hostname round-trip in
    front of the session and a one-shot `-- cmd` skipped it, because a one-shot
    renders no prompt for a hostname to appear in. With the naming folded into
    the setup pass there is nothing left to skip, so the branch is gone and both
    shapes cost exactly the session.
    """

    @staticmethod
    def _trips(shell_command: Optional[str]) -> int:
        with patch("devlaunch.dl.run_devpod") as run_devpod:
            with patch("devlaunch.dl.workspace_ssh", return_value=0) as ssh:
                assert attach_workspace("myws", shell_command) == 0
        ssh.assert_called_once_with("myws", shell_command)
        return run_devpod.call_count

    def test_an_interactive_attach_spawns_nothing_but_the_session(self):
        assert self._trips(None) == 0

    def test_a_one_shot_command_spawns_nothing_but_the_session(self):
        assert self._trips("echo hi") == 0
