#!/bin/bash
# Development installation script for DevLaunch
# Installs DevLaunch globally in editable mode using uv

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

# Create symlink for the dl command
DL_TARGET="${VENV_DIR}/bin/dl"
DL_LINK="${BIN_DIR}/dl"

if [ -L "${DL_LINK}" ]; then
    rm "${DL_LINK}"
fi

if [ -e "${DL_LINK}" ]; then
    echo "Warning: ${DL_LINK} exists and is not a symlink. Skipping symlink creation."
else
    ln -s "${DL_TARGET}" "${DL_LINK}"
    echo "Created symlink: ${DL_LINK} -> ${DL_TARGET}"
fi

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

echo "You can now test DevLaunch with:"
echo "  dl --help"
echo "  dl --version"
echo ""
echo "To test worktree backend (enabled by default):"
echo "  dl owner/repo@branch"
echo ""
echo "To test with legacy DevPod backend:"
echo "  dl --backend devpod owner/repo@branch"
