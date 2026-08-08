"""The claims AGENTS.md makes about the two builds, checked against the tree.

Most of that document is advice and gets no test. What is guarded here is the
handful of statements a reader would act on and that the repository could
silently invalidate: that the devcontainer arrives with the checkout already
installed, that ``./dev.sh`` refuses in there rather than half-installing, and
that the provenance string quoted as an example is the one ``--version`` prints.
A change that makes any of those false should fail here rather than be
discovered by an agent following stale instructions.
"""

import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest
import tomli

REPO_ROOT = Path(__file__).resolve().parent.parent
AGENTS_MD = REPO_ROOT / "AGENTS.md"
DEV_SH = REPO_ROOT / "dev.sh"

# The heading the in-container guidance lives under. Matching on the heading
# rather than on a phrase keeps the assertions pointed at one section while the
# prose around it is free to change.
CONTAINER_HEADING = "### Inside the devcontainer: one build, and it is `pixi run dl`"


def _agents_md() -> str:
    return AGENTS_MD.read_text(encoding="utf-8")


def _container_section() -> str:
    """The in-container section only, from its heading to the next one."""
    text = _agents_md()
    start = text.index(CONTAINER_HEADING)
    rest = text[start + len(CONTAINER_HEADING) :]
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
    """The one instruction above that an in-container reader must not follow."""
    section = _container_section()
    assert "`./dev.sh`" in section
    assert "uv" in section


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
    for tool in ("dirname",):
        real = shutil.which(tool)
        assert real, f"{tool} is needed to run dev.sh at all"
        (fake_bin / tool).symlink_to(real)
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
    post_create = re.search(r'"postCreateCommand"\s*:\s*"([^"]*)"', devcontainer)
    assert post_create, "devcontainer.json declares no postCreateCommand"
    assert "pixi install" in post_create.group(1)


@pytest.mark.unit
def test_the_provenance_example_matches_what_version_prints():
    """Both the section and the host advice quote the string an editable build emits."""
    source = (REPO_ROOT / "devlaunch" / "dl.py").read_text(encoding="utf-8")
    assert 'f"dev, editable from {tree}"' in source

    assert "dev, editable from" in _container_section()
    assert "dev, editable from" in _agents_md()


@pytest.mark.unit
def test_agents_md_does_not_claim_version_hides_provenance():
    """A stale claim `--version` outgrew; the container section's example depends on it."""
    text = _agents_md().lower()
    assert "not its provenance" not in text
    assert "the name is the only thing distinguishing" not in text
