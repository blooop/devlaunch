# Publishing to conda-forge

This guide explains how to submit devlaunch to conda-forge for the first time.

## Prerequisites

- GitHub account
- Fork of [conda-forge/staged-recipes](https://github.com/conda-forge/staged-recipes)

## Step-by-Step Submission Process

### 1. Get the SHA256 Hash

First, download the PyPI tarball and calculate its SHA256 hash:

```bash
# Download the source tarball from PyPI
wget https://pypi.io/packages/source/d/devlaunch/devlaunch-0.0.3.tar.gz

# Calculate SHA256 hash
sha256sum devlaunch-0.0.3.tar.gz
```

### 2. Fork staged-recipes

1. Go to https://github.com/conda-forge/staged-recipes
2. Click "Fork" to create your own fork
3. Clone your fork locally:

```bash
git clone https://github.com/YOUR_USERNAME/staged-recipes.git
cd staged-recipes
```

### 3. Create a New Branch

```bash
git checkout -b devlaunch
```

### 4. Copy the Recipe

Copy the recipe from this repository to staged-recipes:

```bash
# From the staged-recipes directory
cp -r /path/to/devlaunch/recipe recipes/devlaunch
```

### 5. Update the SHA256 Hash

Edit `recipes/devlaunch/meta.yaml` and replace the comment with the actual SHA256 hash:

```yaml
source:
  url: https://pypi.io/packages/source/d/devlaunch/devlaunch-0.0.3.tar.gz
  sha256: YOUR_SHA256_HASH_HERE  # Replace with actual hash from step 1
```

### 6. Test the Recipe Locally (Optional but Recommended)

```bash
# Install conda-build if not already installed
conda install conda-build

# Build the package
conda build recipes/devlaunch

# Test the built package
conda create -n test-devlaunch devlaunch --use-local
conda activate test-devlaunch
dl --help
dl --version
conda deactivate
```

### 7. Commit and Push

```bash
git add recipes/devlaunch
git commit -m "Add devlaunch recipe"
git push origin devlaunch
```

### 8. Create Pull Request

1. Go to your fork on GitHub
2. Click "Compare & pull request"
3. Fill in the PR template with:
   - Checklist items (make sure all requirements are met)
   - Brief description of the package
   - Link to the source repository
4. Submit the PR

### 9. Wait for Review

- The conda-forge bot will run automatic checks
- Maintainers will review your submission
- Address any feedback or requested changes
- Once approved, the recipe will be merged

### 10. After Acceptance

Once your PR is merged:

1. A new feedstock repository will be created: `conda-forge/devlaunch-feedstock`
2. You will be added as a maintainer
3. The package will be built and published to conda-forge
4. Users can install it with: `conda install -c conda-forge devlaunch`

## Future Updates

After the initial submission, you don't need to submit to staged-recipes again. Instead:

1. Updates are made directly to the feedstock repository
2. You can update the recipe by submitting PRs to `conda-forge/devlaunch-feedstock`
3. Or use the conda-forge bot to automatically update when new versions are released on PyPI

## Automated Updates

To enable automatic version updates:

1. Go to https://github.com/conda-forge/devlaunch-feedstock
2. The conda-forge bot can automatically detect new PyPI releases
3. It will create PRs to update the feedstock when new versions are published

## Additional Resources

- [conda-forge Documentation](https://conda-forge.org/docs/)
- [Contributing Packages](https://conda-forge.org/docs/maintainer/adding_pkgs.html)
- [staged-recipes Guidelines](https://github.com/conda-forge/staged-recipes)
- [Feedstock Maintenance](https://conda-forge.org/docs/maintainer/updating_pkgs.html)

## Troubleshooting

### Common Issues

**Build fails with missing dependencies:**
- Check that all dependencies are available on conda-forge
- If a dependency is missing, it needs to be added to conda-forge first

**Tests fail:**
- Ensure entry points are correctly defined
- Verify imports work correctly
- Check that test commands run successfully

**Linting errors:**
- Run `conda-smithy` locally to validate the recipe
- Follow conda-forge recipe style guidelines

## Notes

- The recipe uses `noarch: python` because it's a pure Python package
- The build uses `hatchling` as specified in pyproject.toml
- Entry point `dl` is automatically created during installation
- Tests verify both imports and CLI functionality
