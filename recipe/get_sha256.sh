#!/bin/bash
# Helper script to get SHA256 hash for conda-forge recipe
# Usage: ./get_sha256.sh [version]

VERSION=${1:-0.0.3}
TARBALL="devlaunch-${VERSION}.tar.gz"
URL="https://pypi.io/packages/source/d/devlaunch/${TARBALL}"

echo "Downloading ${TARBALL} from PyPI..."
wget -q "${URL}" -O "/tmp/${TARBALL}"

if [ $? -eq 0 ]; then
    echo "Calculating SHA256 hash..."
    SHA256=$(sha256sum "/tmp/${TARBALL}" | awk '{print $1}')
    echo ""
    echo "SHA256 hash for devlaunch ${VERSION}:"
    echo "${SHA256}"
    echo ""
    echo "Update meta.yaml with:"
    echo "  sha256: ${SHA256}"

    # Clean up
    rm "/tmp/${TARBALL}"
else
    echo "Error: Failed to download tarball from PyPI"
    echo "URL: ${URL}"
    exit 1
fi
