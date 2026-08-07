#!/bin/bash
# Development installation script for DevLaunch
# Installs this working tree as `dl-next` and `aid-next`, beside the released
# `dl` and `aid` rather than on top of them, so both are on PATH at once and
# running the wrong one is not possible by accident. The released build is what
# keeps working while this checkout is mid-change; it is installed by pixi
# global from the blooop channel and this script never touches it.
#
# Editable, so `dl-next` is whatever the tree looks like right now — there is no
# build step to forget, and equally no snapshot: a half-finished edit is live the
# moment it is saved. Run it against throwaway state when that matters; dl
# resolves everything it stores through XDG_CACHE_HOME and XDG_CONFIG_HOME, so
#
#   XDG_CACHE_HOME=/tmp/dl-scratch/cache XDG_CONFIG_HOME=/tmp/dl-scratch/config dl-next ...
#
# leaves the real workspace list alone.

set -e  # Exit on error

VENV_DIR="${HOME}/.local/share/devlaunch-dev"
BIN_DIR="${HOME}/.local/bin"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Installing DevLaunch in development mode using uv..."
echo "  Venv location: ${VENV_DIR}"
echo "  Project path: ${SCRIPT_DIR}"
echo ""

# Check if uv is installed
if ! command -v uv &> /dev/null; then
    echo "Error: uv is not installed."
    echo "Install it with: curl -LsSf https://astral.sh/uv/install.sh | sh"
    exit 1
fi

# Create or reuse the virtual environment
if [ -d "${VENV_DIR}" ]; then
    echo "Using existing venv at ${VENV_DIR}"
else
    echo "Creating virtual environment..."
    uv venv "${VENV_DIR}"
fi

# Install in editable mode
echo "Installing DevLaunch in editable mode..."
uv pip install -e "${SCRIPT_DIR}" --python "${VENV_DIR}/bin/python"

# Ensure ~/.local/bin exists
mkdir -p "${BIN_DIR}"

# Symlink every entry point under a -next name. Console scripts do not care what
# they are called -- the shebang points at the venv's python either way -- so the
# only thing the name decides is which build you get when you type it.
for cmd in dl aid; do
    target="${VENV_DIR}/bin/${cmd}"
    link="${BIN_DIR}/${cmd}-next"

    if [ -L "${link}" ]; then
        rm "${link}"
    fi

    if [ -e "${link}" ]; then
        echo "Warning: ${link} exists and is not a symlink. Skipping symlink creation."
    else
        ln -s "${target}" "${link}"
        echo "Created symlink: ${link} -> ${target}"
    fi
done

# Verify installation
echo ""
echo "Verifying installation..."
"${VENV_DIR}/bin/python" -c "from devlaunch.dl import get_version; print(f'DevLaunch version: {get_version()}')"

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
echo "  dl-next --version"
echo "  dl-next owner/repo          # clone + DevPod workspace (default branch)"
echo "  dl-next owner/repo@branch   # clone + DevPod workspace (specific branch)"
echo "  aid-next owner/repo@branch  # ...with a coding agent started in it"
echo ""
echo "Against throwaway state, leaving the real workspace list alone:"
echo "  XDG_CACHE_HOME=/tmp/dl-scratch/cache XDG_CONFIG_HOME=/tmp/dl-scratch/config dl-next ..."
echo ""
echo "Plain 'dl' and 'aid' remain the released build, untouched by this script."
