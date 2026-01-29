#!/bin/bash
# Development installation script for DevLaunch
# Installs DevLaunch in editable mode for testing changes

set -e  # Exit on error

echo "Installing DevLaunch in development mode..."

# Install in editable mode with dependencies
echo "Installing DevLaunch and dependencies..."
pip install -e . --break-system-packages

# Verify installation
echo ""
echo "Verifying installation..."
which dl || echo "Warning: 'dl' command not found in PATH"

echo ""
echo "Testing import..."
python -c "from devlaunch.dl import get_version; print(f'DevLaunch version: {get_version()}')"

echo ""
echo "Development installation complete!"
echo ""
echo "You can now test DevLaunch with:"
echo "  dl --help"
echo "  dl --version"
echo ""
echo "To test worktree backend (enabled by default):"
echo "  dl owner/repo@branch"
echo ""
echo "To test with legacy DevPod backend:"
echo "  dl --backend devpod owner/repo@branch"