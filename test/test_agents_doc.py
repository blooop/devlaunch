"""The claims AGENTS.md makes about the two builds, checked against the tree.

Most of that document is advice and gets no test. What is guarded here is the
handful of statements a reader would act on and that the repository could
silently invalidate: that the devcontainer can build the tree it ships, that
``./dev.sh`` refuses in there rather than half-installing, and that the two
builds are distinguishable by what ``--version`` prints.

That last one is the whole point of the ``dl``/``dl-next`` arrangement (#268), and
it is the one that can rot invisibly: drop ``--features dl/dev-build`` from
``dev.sh`` and everything still builds, installs and runs -- ``dl-next`` just
quietly starts claiming to be the released build. So the marker is asserted from
both ends, at the flag that produces it and at the prose that promises it.
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
PYPROJECT = REPO_ROOT / "pyproject.toml"
CARGO_TOOLCHAIN = REPO_ROOT / "rust" / "rust-toolchain.toml"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"

# The cargo feature that marks a working-tree build. One name, spelled here once,
# so a rename has to come through this file.
DEV_FEATURE = "dev-build"

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


def _pyproject() -> dict:
    return tomli.loads(PYPROJECT.read_text(encoding="utf-8"))


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
    assert "`cargo` is on PATH only inside" in section


# ---------------------------------------------------------------------------
# the two names print two version strings
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_dev_sh_builds_with_the_feature_that_marks_the_version():
    """The flag `dl-next`'s whole distinguishability rests on.

    Asserted per package rather than as one substring: this is a multi-package
    cargo build, and a `--features` list that named only `dl` would leave
    `aid-next` reporting the released version while `dl-next` reported `-dev` --
    exactly the half-broken state two names exist to prevent.
    """
    dev_sh = DEV_SH.read_text(encoding="utf-8")
    for package in ("dl", "aid"):
        assert f"{package}/{DEV_FEATURE}" in dev_sh, (
            f"dev.sh no longer builds {package} with the {DEV_FEATURE} feature, so "
            f"{package}-next would report the released version"
        )


@pytest.mark.unit
def test_the_feature_exists_and_aid_takes_it_from_dl():
    """One marker, owned by `dl`, forwarded by `aid`.

    The forwarding is the assertion worth making: two independently declared
    features could be enabled separately, and then the two binaries could disagree
    about which build they are.
    """
    dl_manifest = tomli.loads((REPO_ROOT / "rust" / "dl" / "Cargo.toml").read_text("utf-8"))
    assert DEV_FEATURE in dl_manifest["features"], (
        f"rust/dl/Cargo.toml no longer declares the {DEV_FEATURE} feature"
    )
    assert dl_manifest["features"][DEV_FEATURE] == [], (
        f"dl's {DEV_FEATURE} should enable nothing else; it only flips the marker"
    )

    aid_manifest = tomli.loads((REPO_ROOT / "rust" / "aid" / "Cargo.toml").read_text("utf-8"))
    assert aid_manifest["features"][DEV_FEATURE] == [f"dl/{DEV_FEATURE}"], (
        f"aid's {DEV_FEATURE} must forward to dl's, so the two cannot disagree"
    )


@pytest.mark.unit
def test_the_marker_is_off_by_default_so_released_builds_print_the_bare_version():
    """Nothing may enable the feature by default, anywhere.

    The packaging job asserts `dl --version` is exactly `dl <version>`; a default
    feature would break that, and it would break it in the artifact that ships
    rather than in a test.
    """
    for manifest in (
        REPO_ROOT / "rust" / "dl" / "Cargo.toml",
        REPO_ROOT / "rust" / "aid" / "Cargo.toml",
    ):
        features = tomli.loads(manifest.read_text("utf-8")).get("features", {})
        assert DEV_FEATURE not in features.get("default", []), (
            f"{manifest.name} enables {DEV_FEATURE} by default, so the released build "
            "would print a -dev version"
        )


@pytest.mark.unit
@pytest.mark.parametrize("section", [_host_section, _container_section], ids=["host", "container"])
def test_both_sections_quote_the_dev_version_line(section):
    """Each section promises the `-dev` suffix the feature above produces.

    Parametrized so the two are checked independently: a section is a substring of
    the document, so one assertion over the whole file would leave either half
    unguarded. The version is deliberately a placeholder -- pinning a real one
    drifts apart at the next release.
    """
    assert "`dl <version>-dev`" in section(), (
        "the section no longer quotes `dl <version>-dev`, which is what tells a "
        "working-tree build from the released one"
    )


@pytest.mark.unit
def test_the_pixi_tasks_run_the_working_tree_with_the_marker():
    """`pixi run dl` must be cargo over this tree, marked, not a stale console script."""
    tasks = _pyproject()["tool"]["pixi"]["tasks"]
    for name in ("dl", "aid"):
        command = tasks[name]
        assert "cargo run" in command, f"the {name} task no longer builds this tree"
        assert "rust/Cargo.toml" in command, f"the {name} task names no cargo manifest"
        assert f"{name}/{DEV_FEATURE}" in command, (
            f"`pixi run {name}` no longer marks its version line, so it is "
            "indistinguishable from a released build"
        )


# ---------------------------------------------------------------------------
# the toolchain pin AGENTS.md says is kept in lockstep
# ---------------------------------------------------------------------------


@pytest.mark.unit
def test_every_toolchain_pin_names_the_channel_rust_toolchain_toml_names():
    """`rust/rust-toolchain.toml` is the pin of record; three copies follow it.

    The container's pixi dependency (so `pixi run dl` builds with what ships) and
    CI's two `dtolnay/rust-toolchain` steps. Drift here is invisible and expensive:
    a container that compiles with a toolchain the gate never used is a class of
    "works for me" nobody can reproduce.
    """
    pinned = tomli.loads(CARGO_TOOLCHAIN.read_text("utf-8"))["toolchain"]["channel"]

    # A pixi *feature*, not a plain dependency (#284): only the `default`
    # environment takes it, so the py31x solves and their CI caches do not each
    # carry a 1.5GB toolchain.
    pixi_rust = _pyproject()["tool"]["pixi"]["feature"]["rust"]["dependencies"]["rust"]
    assert pixi_rust == f"{pinned}.*", (
        f"pixi pins rust {pixi_rust!r}, rust-toolchain.toml pins {pinned!r}. These have "
        "to name one toolchain: AGENTS.md tells a reader the container is pinned to the "
        "version rust-toolchain.toml names, and a looser pin here makes that false the "
        "day the next patch is published -- silently, and only inside the container."
    )

    ci = CI_WORKFLOW.read_text(encoding="utf-8")
    declared = re.findall(r"^\s*toolchain:\s*(\S+)\s*$", ci, re.M)
    assert declared, "ci.yml declares no explicit toolchain any more"
    assert set(declared) == {pinned}, (
        f"ci.yml pins {sorted(set(declared))}, rust-toolchain.toml pins {pinned!r}"
    )


# ---------------------------------------------------------------------------
# the refusal the in-container advice rests on
# ---------------------------------------------------------------------------


@pytest.mark.integration
def test_dev_sh_refuses_before_touching_anything_when_cargo_is_absent(tmp_path):
    """`./dev.sh` with no `cargo` exits loudly and installs nothing.

    This is the behaviour the in-container advice rests on: `cargo` is in the pixi
    environment and not on the bare PATH, so a bare `./dev.sh` in there stops at
    its first check rather than part-installing a second build. Run for real with
    `cargo` off PATH, against a throwaway HOME, so a future edit that moves work
    above that check fails here.
    """
    home = tmp_path / "home"
    home.mkdir()
    # A PATH carrying only what the script needs before the check it must stop
    # at -- notably not `cargo`.
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    for tool in ("dirname", "mktemp"):
        found = shutil.which(tool)
        assert found, f"{tool} is needed to run dev.sh at all"
        (fake_bin / tool).symlink_to(found)
    assert shutil.which("cargo", path=str(fake_bin)) is None

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
    assert "cargo is not installed" in result.stdout + result.stderr
    assert not (home / ".local" / "share" / "devlaunch-dev").exists()
    assert not (home / ".local" / "bin").exists()


@pytest.mark.unit
def test_devcontainer_installs_the_environment_at_create_time():
    """The container section's premise: it comes up able to build the tree.

    `pixi install` is what puts the pinned toolchain in the environment, so
    `pixi run dl` compiles rather than reporting a missing cargo.
    """
    devcontainer = (REPO_ROOT / ".devcontainer" / "devcontainer.json").read_text(encoding="utf-8")
    # The file is JSONC (it carries commented-out features), so it is matched
    # rather than parsed. `postCreateCommand` takes a string or an argv array;
    # accept both, or growing a second step turns this into a false "declares
    # none" against a config that is still correct.
    post_create = re.search(r'"postCreateCommand"\s*:\s*(?P<value>"[^"]*"|\[[^]]*\])', devcontainer)
    assert post_create, "devcontainer.json declares no postCreateCommand as a string or an array"
    assert "pixi install" in post_create.group("value")


@pytest.mark.unit
def test_agents_md_does_not_claim_the_dev_build_is_editable():
    """The premises the compiled build invalidated (#268).

    Each of these was true of the editable Python install and is now the opposite
    of true, so a stale copy of any of them is worse than no advice: it tells a
    reader their edit is already live when it needs a rebuild.
    """
    text = _agents_md().lower()
    # Claims, not words. The document is free to say what the old build did --
    # the trade it made is the clearest way to explain the one this build makes --
    # and a blanket ban on the vocabulary would forbid explaining it at all. What
    # is banned is the present tense: each of these asserted a property the
    # compiled build does not have.
    for stale in (
        "the install is editable",
        "there is no build step",
        "is live as soon as it is saved",
        "git stash",
    ):
        assert stale not in text, (
            f"AGENTS.md still claims {stale!r}, which was true of the editable "
            "Python install and is false of the compiled one"
        )
