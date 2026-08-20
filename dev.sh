#!/bin/bash
# Development installation script for DevLaunch
# Builds this working tree and installs it as `dl-next` and `aid-next`, beside the
# released `dl` and `aid` rather than on top of them, so both are on PATH at once
# and running the wrong one is not possible by accident. The released build is what
# keeps working while this checkout is mid-change; it is installed by pixi global
# from the blooop channel and this script never touches it.
#
# Compiled, not editable, and that is a trade rather than an oversight (#268). The
# shipped implementation is Rust, so there is no editable install to be had: what
# `dl-next` names is a *snapshot*, and it moves when you re-run this script and at
# no other time. The old Python build had the opposite trade -- no build step, but
# equally no snapshot, so a half-finished edit was live the moment it was saved.
# Under this one a half-finished edit is invisible until it compiles, which is the
# better half of the trade for a tree that is mid-change.
#
# The copies live in this script's own directory rather than being symlinked at
# `rust/target/release/`, and that is what makes "moves only when you re-run this"
# true: a plain `cargo build --release` in the tree would otherwise overwrite what
# `dl-next` resolves to (dropping the -dev marker with it), and a `cargo clean`
# would delete it outright.
#
# Run it against throwaway state when the change is anywhere near storage; dl
# resolves everything it stores through XDG_CACHE_HOME, so
#
#   XDG_CACHE_HOME=/tmp/dl-scratch/cache dl-next ...
#
# leaves the real workspace list alone. Scope that variable only, as a trade
# rather than a free simplification: a scratch XDG_CONFIG_HOME would also hide a
# personal config.toml, which can pin repos_dir back at the real cache (which is
# why test/conftest.py scopes both), but it hides the host's gh login too, so
# every workspace opens with no GitHub credentials. The credential loss happens
# on every run; the repos_dir hazard needs a config.toml most hosts do not have.

set -e  # Exit on error

INSTALL_DIR="${HOME}/.local/share/devlaunch-dev"
BIN_DIR="${HOME}/.local/bin"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CARGO_DIR="${SCRIPT_DIR}/rust"

echo "Installing DevLaunch in development mode from ${CARGO_DIR}..."
echo "  Install location: ${INSTALL_DIR}"
echo "  Project path: ${SCRIPT_DIR}"
echo ""

# Check if cargo is installed. First, and before anything is written, so a host
# without a Rust toolchain gets one clear refusal rather than a part-install --
# which is the behaviour this repo's devcontainer advice rests on (see AGENTS.md:
# in there the answer is `pixi run dl`, and this script is expected to stop here).
if ! command -v cargo &> /dev/null; then
    echo "Error: cargo is not installed."
    echo "Install a Rust toolchain from https://rustup.rs, or use this repo's"
    echo "pixi environment, which pins the same one: pixi run cargo --version"
    exit 1
fi

# Build the shipping binaries, in the profile they ship in.
#
# `--features .../dev-build` is the whole reason `dl-next --version` can be told
# apart from the released `dl --version`: it appends `-dev` to the version line
# (see the feature's comment in rust/dl/Cargo.toml). The feature is named per
# package because this is a multi-package build; `aid`'s forwards to `dl`'s, so
# the two binaries cannot report different provenance.
#
# `--locked` so this builds what the lockfile says, exactly as CI does. A tree
# whose Cargo.lock is stale should say so here rather than resolve something else.
echo "Building the release binaries (this is a compile, not an editable install)..."
BUILD_LOG="$(mktemp)"
trap 'rm -f "${BUILD_LOG}"' EXIT

# The executables are read out of cargo's own report rather than named here,
# because a list here is one somebody has to remember: under the old script `aid`
# was added as a second entry point and the script knew nothing about it, so `aid`
# kept resolving to the released build while its change sat in the tree untested.
# `--message-format=json-render-diagnostics` keeps compiler diagnostics on stderr
# where they belong and puts one JSON object per artifact on stdout; only real
# executables carry a non-null `executable`, so this names exactly the binaries
# this build produced -- including a third one, the day there is one.
(
    cd "${CARGO_DIR}"
    cargo build --release --locked \
        -p dl -p aid \
        --features dl/dev-build,aid/dev-build \
        --message-format=json-render-diagnostics
) > "${BUILD_LOG}"

mapfile -t BUILT < <(sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' "${BUILD_LOG}")

if [ ${#BUILT[@]} -eq 0 ]; then
    echo "Error: the build reported no executables. Nothing was installed." >&2
    exit 1
fi

# Install the copies, then point the -next names at them.
mkdir -p "${INSTALL_DIR}/bin"
mkdir -p "${BIN_DIR}"

echo ""
for built in "${BUILT[@]}"; do
    name="$(basename "${built}")"
    copy="${INSTALL_DIR}/bin/${name}"
    link="${BIN_DIR}/${name}-next"

    # `install` rather than `cp`: it replaces the target by rename, so a `dl-next`
    # running right now keeps the binary it started with instead of having it
    # rewritten underneath it.
    install -m 755 "${built}" "${copy}"
    echo "Installed: ${copy}"

    if [ -L "${link}" ]; then
        rm "${link}"
    fi

    if [ -e "${link}" ]; then
        echo "Warning: ${link} exists and is not a symlink. Skipping symlink creation."
    else
        ln -s "${copy}" "${link}"
        echo "Created symlink: ${link} -> ${copy}"
    fi
done

# Verify the installation, and verify the thing worth verifying: that these
# binaries say which build they are. A `-next` that printed the released version
# string would be the one failure mode this whole two-names arrangement exists to
# prevent, and it would otherwise be discovered by trusting the wrong build.
echo ""
echo "Verifying installation..."
for built in "${BUILT[@]}"; do
    name="$(basename "${built}")"
    reported="$("${INSTALL_DIR}/bin/${name}" --version)"
    case "${reported}" in
        *-dev)
            echo "  ${name}-next: ${reported}"
            ;;
        *)
            echo "Error: ${name}-next reports '${reported}', which carries no -dev" >&2
            echo "marker -- it is indistinguishable from the released build. The" >&2
            echo "dev-build feature did not reach it." >&2
            exit 1
            ;;
    esac
done

echo ""
echo "Development installation complete!"
echo ""

# Check if ~/.local/bin is in PATH
if [[ ":$PATH:" != *":${BIN_DIR}:"* ]]; then
    echo "Note: ${BIN_DIR} is not in your PATH."
    echo "Add it with: export PATH=\"\${HOME}/.local/bin:\${PATH}\""
    echo ""
fi

echo "You can now test this working tree with:"
echo "  dl-next --help"
echo "  dl-next --version           # prints <version>-dev; plain dl does not"
echo "  dl-next owner/repo          # clone + DevPod workspace (default branch)"
echo "  dl-next owner/repo@branch   # clone + DevPod workspace (specific branch)"
echo "  aid-next owner/repo@branch  # ...with a coding agent started in it"
echo ""
echo "Against throwaway state, leaving the real workspace list alone:"
echo "  XDG_CACHE_HOME=/tmp/dl-scratch/cache dl-next ..."
echo "  (that variable only -- a scratch XDG_CONFIG_HOME hides your gh login)"
echo ""
echo "Re-run this script after every change you want dl-next to pick up; it is a"
echo "compiled snapshot, not an editable install."
echo ""
echo "Plain 'dl' and 'aid' remain the released build, untouched by this script."
