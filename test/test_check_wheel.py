"""`scripts/check_wheel.py` holds the wheel to exactly the binaries it lists.

The wheel ships `dl` and `dla` and nothing else under `.data/scripts/`. A check
that only asked whether each listed binary is present would pass a wheel that
still carried a retired one, such as `aid`, which pip would then install onto
PATH beside the two that are meant to be there. These tests build a small wheel
zip with the layout the script reads and run the real script against it.
"""

import subprocess
import sys
import zipfile
from pathlib import Path

import pytest

ROOT = Path(__file__).parent.parent
CHECK = (ROOT / "scripts" / "check_wheel.py").resolve()
VERSION = "0.62.1"


def wheel(tmp_path: Path, binaries: list[str]) -> Path:
    """A wheel named for VERSION whose `.data/scripts/` holds `binaries`."""
    path = tmp_path / f"devlaunch-{VERSION}-py3-none-manylinux_2_17_x86_64.whl"
    with zipfile.ZipFile(path, "w") as archive:
        dist_info = f"devlaunch-{VERSION}.dist-info"
        archive.writestr(f"{dist_info}/METADATA", f"Name: devlaunch\nVersion: {VERSION}\n")
        archive.writestr(f"{dist_info}/WHEEL", "Wheel-Version: 1.0\n")
        archive.writestr(f"{dist_info}/RECORD", "")
        for binary in binaries:
            archive.writestr(f"devlaunch-{VERSION}.data/scripts/{binary}", "\x7fELF")
    return path


def run(path: Path) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(CHECK), str(path), "--version", VERSION],
        capture_output=True,
        text=True,
        check=False,
    )


@pytest.mark.unit
def test_a_wheel_with_exactly_the_listed_binaries_passes(tmp_path: Path) -> None:
    done = run(wheel(tmp_path, ["dl", "dla"]))
    assert done.returncode == 0, done.stderr


@pytest.mark.unit
def test_a_wheel_with_a_binary_it_does_not_list_fails_naming_it(tmp_path: Path) -> None:
    done = run(wheel(tmp_path, ["dl", "dla", "aid"]))
    assert done.returncode == 1
    assert "aid" in done.stderr


@pytest.mark.unit
def test_a_wheel_missing_a_listed_binary_fails_naming_it(tmp_path: Path) -> None:
    done = run(wheel(tmp_path, ["dl"]))
    assert done.returncode == 1
    assert "dla" in done.stderr
