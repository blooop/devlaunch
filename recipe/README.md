# conda-forge Recipe

This directory contains the conda-forge recipe for devlaunch.

## Files

- `meta.yaml` - The conda-forge recipe definition
- `get_sha256.sh` - Helper script to calculate SHA256 hash for PyPI releases

## Quick Start

To get the SHA256 hash for a new release:

```bash
./get_sha256.sh 0.0.3  # Replace with your version
```

## Submitting to conda-forge

See [CONDA_FORGE.md](../CONDA_FORGE.md) in the project root for detailed submission instructions.

## About

This recipe defines how to build and package devlaunch for conda-forge. Once the package is accepted on conda-forge, users will be able to install it with:

```bash
conda install -c conda-forge devlaunch
```

The recipe:
- Uses the PyPI source tarball
- Builds as a `noarch: python` package (pure Python, platform-independent)
- Uses `hatchling` as the build backend
- Creates the `dl` command-line entry point
- Tests both imports and CLI functionality
