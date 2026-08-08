"""The claims AGENTS.md makes about the two builds, checked against the tree.

Most of that document is advice and gets no test. What is guarded here is the
handful of statements a reader would act on and that the repository could
silently invalidate: that the devcontainer arrives with the checkout already
installed, that ``./dev.sh`` refuses in there rather than half-installing, and
that the provenance string quoted as an example is the one ``--version`` prints.
A change that makes any of those false should fail here rather than be
discovered by an agent following stale instructions.
"""

import json
import os
import re
import shutil
import subprocess
from pathlib import Path
from unittest import mock

import pytest
import tomli

from devlaunch.dl import _install_provenance

REPO_ROOT = Path(__file__).resolve().parent.parent
AGENTS_MD = REPO_ROOT / "AGENTS.md"
DEV_SH = REPO_ROOT / "dev.sh"

# The headings the two halves of the advice live under. Matching on the heading
# rather than on a phrase keeps the assertions pointed at one section while the
# prose around it is free to change.
HOST_HEADING = "## Two installs: `dl` and `dl-next`"
CONTAINER_HEADING = "### Inside the devcontainer: one build, and it is `pixi run dl`"


def _agents_md() -> str:
    return AGENTS_MD.read_text(encoding="utf-8")


def _heading_index(text: str, heading: str) -> int:
    """Where `heading` starts, with a readable failure when it is gone.

    A renamed heading is the likeliest way these tests break, and it must say so
    rather than surface as a `ValueError` from deep inside a helper.
    """
    start = text.find(heading)
    assert start != -1, f"AGENTS.md has no heading {heading!r}; the section was renamed or removed"
    return start


def _host_section() -> str:
    """The host-side advice: the two-installs section up to the in-container one."""
    text = _agents_md()
    start = _heading_index(text, HOST_HEADING) + len(HOST_HEADING)
    return text[start : _heading_index(text, CONTAINER_HEADING)]


def _container_section() -> str:
    """The in-container section only, from its heading to the next one."""
    text = _agents_md()
    rest = text[_heading_index(text, CONTAINER_HEADING) + len(CONTAINER_HEADING) :]
    end = rest.find("\n## ")
    return rest if end == -1 else rest[:end]


@pytest.mark.unit
def test_claude_md_is_the_same_document_as_agents_md():
    """Editing one file must serve both names, or half the agents read stale text."""
    claude_md = REPO_ROOT / "CLAUDE.md"
    assert claude_md.is_symlink()
    assert os.readlink(claude_md) == "AGENTS.md"


@pytest.mark.unit
def test_agents_md_sends_in_container_work_through_pixi_run():
    """Inside the devcontainer the document must name `pixi run dl` and `pixi run aid`."""
    section = _container_section()
    assert "`pixi run dl`" in section
    assert "`pixi run aid`" in section


@pytest.mark.unit
def test_agents_md_warns_off_dev_sh_inside_the_container():
    """The one instruction above that an in-container reader must not follow.

    Naming `./dev.sh` is not enough: a section that told the reader to *run* it
    would name it too. What has to survive a rewrite is the prohibition itself,
    in the bold the reader skims for, and the reason it is not a gap to fill.
    """
    section = _container_section()
    assert re.search(r"\*\*Do not run `\./dev\.sh`[^*]*\*\*", section), (
        "the in-container section no longer forbids `./dev.sh` in bold"
    )
    assert "`uv` is not installed" in section


@pytest.mark.integration
def test_dev_sh_refuses_before_touching_anything_when_uv_is_absent(tmp_path):
    """`./dev.sh` with no `uv` exits loudly and installs nothing.

    This is the behaviour the in-container advice rests on: the container has no
    `uv`, so the script stops at its first check rather than part-installing a
    second build. Run for real with `uv` off PATH, against a throwaway HOME, so
    a future edit that moves work above that check fails here.
    """
    home = tmp_path / "home"
    home.mkdir()
    # A PATH carrying only what the script needs before the check it must stop
    # at -- notably not `uv`.
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    dirname = shutil.which("dirname")
    assert dirname, "dirname is needed to run dev.sh at all"
    (fake_bin / "dirname").symlink_to(dirname)
    assert shutil.which("uv", path=str(fake_bin)) is None

    bash = shutil.which("bash")
    assert bash, "bash is needed to run dev.sh at all"
    result = subprocess.run(
        [bash, str(DEV_SH)],
        cwd=tmp_path,
        env={"PATH": str(fake_bin), "HOME": str(home)},
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 1
    assert "uv is not installed" in result.stdout + result.stderr
    assert not (home / ".local" / "share" / "devlaunch-dev").exists()
    assert not (home / ".local" / "bin").exists()


@pytest.mark.unit
def test_devcontainer_installs_the_checkout_editable_at_create_time():
    """The section's premise: the container comes up with dev.sh's job already done."""
    pyproject = tomli.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    self_dep = pyproject["tool"]["pixi"]["pypi-dependencies"]["devlaunch"]
    assert self_dep == {"path": ".", "editable": True}

    devcontainer = (REPO_ROOT / ".devcontainer" / "devcontainer.json").read_text(encoding="utf-8")
    # The file is JSONC (it carries commented-out features), so it is matched
    # rather than parsed. `postCreateCommand` takes a string or an argv array;
    # accept both, or growing a second step turns this into a false "declares
    # none" against a config that is still correct.
    post_create = re.search(r'"postCreateCommand"\s*:\s*(?P<value>"[^"]*"|\[[^]]*\])', devcontainer)
    assert post_create, "devcontainer.json declares no postCreateCommand as a string or an array"
    assert "pixi install" in post_create.group("value")


class _FakeDist:
    """Just enough of an `importlib.metadata` distribution to carry PEP 610 data."""

    def __init__(self, direct_url: str) -> None:
        self._direct_url = direct_url

    def read_text(self, filename: str) -> str | None:
        return self._direct_url if filename == "direct_url.json" else None


def _provenance_for(tree: str) -> str:
    """What `dl --version` composes for an editable install resolving to `tree`.

    The real function, fed the PEP 610 record pip writes for `pip install -e`,
    so the assertion lands on the string that is emitted rather than on how the
    source happens to spell it.
    """
    direct_url = json.dumps({"url": Path(tree).as_uri(), "dir_info": {"editable": True}})
    with mock.patch("devlaunch.dl.distribution", return_value=_FakeDist(direct_url)):
        provenance = _install_provenance()
    assert provenance, f"dl reports no provenance for an editable install at {tree}"
    return provenance


# The trees AGENTS.md names in its two `--version` examples, host and container.
# The version is deliberately a placeholder in both: pinning a real one drifts
# apart at the next release, which is how the document came to quote two.
DOCUMENTED_PROVENANCE_EXAMPLES = (
    ("/path/to/checkout", _host_section),
    ("/workspaces/<checkout>", _container_section),
)


@pytest.mark.unit
@pytest.mark.parametrize("tree,section", DOCUMENTED_PROVENANCE_EXAMPLES)
def test_the_provenance_example_matches_what_version_prints(tree, section):
    """Each quoted `--version` line is the line an editable build would print.

    Parametrized so the host advice and the in-container section are checked
    independently: the section is a substring of the document, so asserting
    against both in one test would leave the host half unguarded.
    """
    expected = f"`dl <version> ({_provenance_for(tree)})`"
    assert expected in section(), f"AGENTS.md no longer quotes {expected} for {tree}"


@pytest.mark.unit
def test_agents_md_does_not_claim_version_hides_provenance():
    """A stale claim `--version` outgrew; the container section's example depends on it."""
    text = _agents_md().lower()
    assert "not its provenance" not in text
    assert "the name is the only thing distinguishing" not in text
