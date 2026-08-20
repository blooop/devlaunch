"""The provider guard's decision: is this devpod provider already registered?

Every input here is a byte-for-byte recording of what devpod v0.26.1 actually
printed, captured under `test/fixtures/devpod/`. That matters more than usual:
the defect this covers is that devpod colourises `provider list` and emits the
reset sequence `ESC[m` immediately before each cell, so the character in front
of `docker` is the letter `m` and a word-boundary match never fires. A
hand-written approximation of ANSI output is exactly how a bug like that
survives being fixed.
"""

import importlib.util
import json
import subprocess
from pathlib import Path
from unittest.mock import patch

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent
PROVIDER_SCRIPT = REPO_ROOT / "scripts" / "devpod_provider.py"


def _load_provider_module():
    """The guard, imported by path -- it is a script under `scripts/`, not a package.

    It moved out of `devlaunch/` with the rest of the Python implementation (#267)
    and stayed, because the machine setup that runs *before* any `dl` still needs
    it: this repo's devcontainer at create time, the bench workflow, a fresh
    `DEVPOD_HOME`. Loaded the way `test_bench_points.py` loads its script, so this
    file keeps judging the code `dev-add-docker` actually runs.
    """
    spec = importlib.util.spec_from_file_location("devpod_provider", PROVIDER_SCRIPT)
    assert spec is not None and spec.loader is not None, PROVIDER_SCRIPT
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


devpod_provider = _load_provider_module()

RECORDINGS = Path(__file__).parent / "fixtures" / "devpod"

# `devpod provider list --output json` on a host where `docker` is registered.
REGISTERED = (RECORDINGS / "provider_list.json").read_text(encoding="utf-8")

# The same command with no --output flag: the human table, with colour.
COLOURISED_TABLE = (RECORDINGS / "provider_list_plain.ansi").read_text(encoding="utf-8")

# `--output json` against a devpod home that has never had a provider added.
NOTHING_REGISTERED = (RECORDINGS / "provider_list_empty.json").read_text(encoding="utf-8")


class RecordedDevpod:
    """Stand-in for the devpod process, answering with a recording."""

    def __init__(self, listing: str, rc: int = 0):
        self.listing = listing
        self.rc = rc
        self.commands: list[list[str]] = []

    def __call__(self, cmd, **kwargs):  # noqa: ARG002
        self.commands.append(list(cmd))
        stdout = self.listing if "list" in cmd else ""
        return subprocess.CompletedProcess(cmd, self.rc, stdout=stdout, stderr="")

    @property
    def added(self) -> list[str]:
        return [c[-1] for c in self.commands if "add" in c]


def test_recordings_are_the_real_thing():
    """Guard the fixtures themselves: the table really is colourised, and the
    character before `docker` really is a word character, which is the whole
    reason a text match on it cannot be trusted."""
    assert "\x1b[" in COLOURISED_TABLE
    index = COLOURISED_TABLE.index("docker")
    assert COLOURISED_TABLE[index - 1] == "m"


def test_registered_provider_is_reported_present():
    assert devpod_provider.parse_provider_names(REGISTERED) == {"docker"}


def test_empty_listing_reports_no_providers():
    assert devpod_provider.parse_provider_names(NOTHING_REGISTERED) == set()


def test_colourised_table_is_not_mistaken_for_an_empty_listing():
    """A listing we could not read is not a listing with nothing in it. The
    original guard collapsed those two into "absent" and tried to add a
    provider that was already there."""
    with pytest.raises(devpod_provider.UnreadableProviderList):
        devpod_provider.parse_provider_names(COLOURISED_TABLE)


def test_existing_provider_is_not_added_again():
    devpod = RecordedDevpod(REGISTERED)

    assert devpod_provider.ensure_provider("docker", run=devpod) is False

    assert devpod.added == []


def test_missing_provider_is_added():
    devpod = RecordedDevpod(NOTHING_REGISTERED)

    assert devpod_provider.ensure_provider("docker", run=devpod) is True

    assert devpod.added == ["docker"]


def test_listing_is_requested_in_machine_readable_form():
    """The guard must never be handed the coloured table in the first place."""
    devpod = RecordedDevpod(REGISTERED)

    devpod_provider.ensure_provider("docker", run=devpod)

    listing_cmd = devpod.commands[0]
    assert listing_cmd[:3] == ["devpod", "provider", "list"]
    assert "--output" in listing_cmd
    assert listing_cmd[listing_cmd.index("--output") + 1] == "json"


def test_the_caller_that_names_no_runner_still_goes_through_a_patched_subprocess():
    """The runner is whatever `subprocess.run` is when the call happens.

    Every test above hands `run=` in explicitly, so none of them can see how
    this module behaves for the caller that does not -- and that caller is the
    shipped one: the `dev-add-docker` task runs the CLI with no runner at all.
    These three entry points used to name `subprocess.run` as a *default
    argument*, which Python evaluates once at import, so they held CPython's
    real `run` for the life of the process and `mock.patch` could not reach
    them by construction. A suite that patched subprocess and then entered one
    of them spawned a real `devpod provider ...` anyway (devlaunch#217).

    So the assertion is that the recorder *saw the argv*, not merely that the
    call returned a set. A leak returns a perfectly good set -- it is a real
    devpod answering -- and the only thing that distinguishes the two is which
    process did the answering.
    """
    devpod = RecordedDevpod(REGISTERED)

    with patch("subprocess.run", devpod):
        assert devpod_provider.list_provider_names() == {"docker"}

    assert devpod.commands == [["devpod", "provider", "list", "--output", "json"]]


def test_the_guard_that_names_no_runner_goes_through_a_patched_subprocess():
    """The same reachability, pinned on `ensure_provider`'s own default.

    Stated separately rather than trusted to the listing above it: this one
    reaches the patch twice over, once for the listing it delegates and once
    for the add it runs itself, and a default argument growing back on either
    is a leak the listing's pin would not notice.
    """
    devpod = RecordedDevpod(NOTHING_REGISTERED)

    with patch("subprocess.run", devpod):
        assert devpod_provider.ensure_provider("docker") is True

    assert devpod.commands == [
        ["devpod", "provider", "list", "--output", "json"],
        ["devpod", "provider", "add", "docker"],
    ]


def test_the_cli_that_names_no_runner_goes_through_a_patched_subprocess(capsys):
    """And the entry point the pixi task actually invokes.

    `python scripts/devpod_provider.py docker` passes no runner, so this is
    the one call shape in the tree that reached the import-time binding in
    production rather than only under test.
    """
    devpod = RecordedDevpod(REGISTERED)

    with patch("subprocess.run", devpod):
        assert devpod_provider.main(["docker"]) == 0

    assert "already registered" in capsys.readouterr().out
    assert devpod.commands == [["devpod", "provider", "list", "--output", "json"]]


def test_unreadable_listing_is_reported_rather_than_swallowed():
    """Recording of the failure mode the fix exists to prevent: if devpod ever
    answers with something unparsable again, the guard says so instead of
    quietly deciding the provider is missing."""
    devpod = RecordedDevpod(COLOURISED_TABLE)

    with pytest.raises(devpod_provider.UnreadableProviderList):
        devpod_provider.ensure_provider("docker", run=devpod)

    assert devpod.added == []


def test_failed_listing_is_reported_rather_than_swallowed():
    devpod = RecordedDevpod("", rc=1)

    with pytest.raises(devpod_provider.UnreadableProviderList):
        devpod_provider.ensure_provider("docker", run=devpod)

    assert devpod.added == []


def test_recorded_json_is_keyed_by_provider_name():
    """Pins the shape this parser depends on, against the real recording."""
    assert list(json.loads(REGISTERED)) == ["docker"]


def test_a_listing_that_is_not_keyed_by_name_is_unreadable():
    """devpod answering with valid JSON of the wrong shape is still an answer we
    cannot read, not an answer that says "nothing registered"."""
    with pytest.raises(devpod_provider.UnreadableProviderList):
        devpod_provider.parse_provider_names('["docker"]')


def test_cli_reports_a_provider_that_was_already_registered(capsys):
    devpod = RecordedDevpod(REGISTERED)

    assert devpod_provider.main(["docker"], run=devpod) == 0

    assert "already registered" in capsys.readouterr().out
    assert devpod.added == []


def test_cli_reports_a_provider_it_added(capsys):
    devpod = RecordedDevpod(NOTHING_REGISTERED)

    assert devpod_provider.main(["docker"], run=devpod) == 0

    assert "added" in capsys.readouterr().out
    assert devpod.added == ["docker"]


def test_cli_fails_loudly_when_devpod_cannot_be_read(capsys):
    """The pixi task that calls this must stop, not carry on as if the provider
    were missing -- that is the failure the guard exists to prevent."""
    devpod = RecordedDevpod(COLOURISED_TABLE)

    assert devpod_provider.main(["docker"], run=devpod) == 1

    assert "error:" in capsys.readouterr().out
    assert devpod.added == []


class FailingAdd(RecordedDevpod):
    """A devpod whose listing reads fine but whose `provider add` fails."""

    def __init__(self, listing: str, stderr: str = "boom"):
        super().__init__(listing)
        self.add_stderr = stderr
        self.add_kwargs: dict = {}

    def __call__(self, cmd, **kwargs):
        result = super().__call__(cmd, **kwargs)
        if "add" in cmd:
            self.add_kwargs = kwargs
            return subprocess.CompletedProcess(cmd, 1, stdout="", stderr=self.add_stderr)
        return result


def test_cli_fails_when_adding_the_provider_fails(capsys):
    assert devpod_provider.main(["docker"], run=FailingAdd(NOTHING_REGISTERED)) == 1

    assert "error:" in capsys.readouterr().out


def test_a_failed_add_reports_what_devpod_said():
    """The listing failure next door quotes devpod's stderr, and this one used
    not to -- so the one failure a user can actually act on was the quiet one."""
    devpod = FailingAdd(NOTHING_REGISTERED, stderr="provider docker already exists")

    with pytest.raises(devpod_provider.ProviderAddFailed) as raised:
        devpod_provider.ensure_provider("docker", run=devpod)

    assert "provider docker already exists" in str(raised.value)


def test_a_failed_add_is_asked_for_its_output():
    """Quoting devpod's stderr means capturing it: without capture_output there
    is nothing on the result to quote."""
    devpod = FailingAdd(NOTHING_REGISTERED)

    with pytest.raises(devpod_provider.ProviderAddFailed):
        devpod_provider.ensure_provider("docker", run=devpod)

    assert devpod.add_kwargs.get("capture_output") is True


def test_the_cli_does_not_swallow_failures_it_knows_nothing_about():
    """The handler exists to turn devpod's two known failures into an exit code.
    A RuntimeError from somewhere else is not one of them, and reporting it as
    `error: <text>` and exit 1 hides a bug behind a tidy message."""

    def exploding(*_args, **_kwargs):
        raise RuntimeError("something else entirely")

    with pytest.raises(RuntimeError, match="something else entirely"):
        devpod_provider.main(["docker"], run=exploding)
