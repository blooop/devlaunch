# pylint: disable=redefined-outer-name
"""Pin the env-gated wall-clock timing summary (ticket #140).

`DEVLAUNCH_TIMING=1` makes every dl process end with one stderr summary naming
each subprocess round trip and the total wall time, so a perf change can land
with before/after numbers. With the variable unset, dl must write no timing
output at all: the hot path must not pay for its own thermometer.

These tests pin behavior at the same boundaries as test_devpod_spawn_counts:
the CLI entry point on one side, the subprocess module on the other.
"""

import re

from devlaunch.dl import main

TIMING_LINE = re.compile(r"^dl-timing: (.+) \d+\.\d{3}s$", re.MULTILINE)


def timing_labels(stderr: str):
    """The labels of every timing line in *stderr*, in order."""
    return TIMING_LINE.findall(stderr)


class TestSummaryGate:
    """Timing lines appear iff DEVLAUNCH_TIMING is set."""

    def test_no_timing_output_when_env_unset(self, monkeypatch, capsys):
        monkeypatch.delenv("DEVLAUNCH_TIMING", raising=False)
        assert main(["--version"]) == 0
        assert "dl-timing:" not in capsys.readouterr().err

    def test_summary_ends_with_total_when_env_set(self, monkeypatch, capsys):
        monkeypatch.setenv("DEVLAUNCH_TIMING", "1")
        assert main(["--version"]) == 0
        assert timing_labels(capsys.readouterr().err) == ["total"]
