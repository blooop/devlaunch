#!/bin/bash
# Development installation script for DevLaunch using pixi
# Installs DevLaunch in editable mode within pixi environment

set -e  # Exit on error

echo "Installing DevLaunch in development mode with pixi..."

# Run pip install in editable mode within pixi environment
echo "Installing in pixi environment..."
pixi run pip install -e .

# Verify installation
echo ""
echo "Verifying installation..."
pixi run which dl || echo "Note: 'dl' command installed in pixi environment"

echo ""
echo "Testing import..."
pixi run python -c "from devlaunch.dl import get_version; print(f'DevLaunch version: {get_version()}')"

echo ""
echo "Development installation complete!"
echo ""
echo "You can now test DevLaunch with:"
echo "  pixi run dl --help"
echo "  pixi run dl --version"
echo ""
echo "To test worktree backend (enabled by default):"
echo "  pixi run dl owner/repo@branch"
echo ""
echo "To test with legacy DevPod backend:"
echo "  pixi run dl --backend devpod owner/repo@branch"
echo ""
echo "Or activate the pixi shell to use 'dl' directly:"
echo "  pixi shell"
echo "  dl --help"