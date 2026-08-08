"""The provider guard's decision: is this devpod provider already registered?

Every input here is a byte-for-byte recording of what devpod v0.26.1 actually
printed, captured under `test/fixtures/devpod/`. That matters more than usual:
the defect this covers is that devpod colourises `provider list` and emits the
reset sequence `ESC[m` immediately before each cell, so the character in front
of `docker` is the letter `m` and a word-boundary match never fires. A
hand-written approximation of ANSI output is exactly how a bug like that
survives being fixed.
"""

import json
import subprocess
from pathlib import Path

import pytest

from devlaunch import devpod_provider

RECORDINGS = Path(__file__).parent.parent / "fixtures" / "devpod"

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
