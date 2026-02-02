# DevLaunch Worktree Backend

## Overview

The worktree backend is an alternative backend for DevLaunch that uses git worktrees for repository management and DevPod for container launching. This approach provides significant performance improvements and disk space savings when working with multiple branches of the same repository.

## Benefits

- **Faster workspace creation**: No cloning required after initial repo clone (5s vs 30s+)
- **Disk space efficiency**: Shared git objects across all branches (50%+ savings)
- **Faster git operations**: Shared repository history
- **Offline capable**: Work with existing branches without network
- **Better branch management**: Native git worktree operations

## Quick Start

### Default Backend

**The worktree backend is enabled by default for all git repositories.** This provides optimal performance and disk usage out of the box.

### Disabling Worktree Backend

If you need to use the legacy DevPod backend, you can disable worktree in several ways:

1. **Environment Variable** (temporary override):
   ```bash
   export DEVLAUNCH_BACKEND=devpod
   dl blooop/devlaunch@main
   ```

2. **Command Line Flag** (per-command override):
   ```bash
   dl --backend devpod blooop/devlaunch@main
   ```

3. **Configuration File** (`~/.config/devlaunch/config.toml`):
   ```toml
   [worktree]
   enabled = false  # Disable worktree backend globally
   ```

### Configuration

The worktree backend works with default settings, but can be customized:

```toml
[worktree]
enabled = true  # Default: true
repos_dir = "~/.devlaunch/repos"
worktrees_dir = "~/.devlaunch/worktrees"
auto_fetch = true
fetch_interval = 3600

[worktree.cleanup]
auto_prune = true
prune_after_days = 30
```

## How It Works

### Architecture

When you run `dl owner/repo@branch` with the worktree backend:

1. **Base Repository**: The first time you access a repo, it's cloned to `~/.devlaunch/repos/owner/repo`
2. **Worktree Creation**: For each branch, a lightweight worktree is created at `~/.devlaunch/worktrees/owner/repo/branch`
3. **DevPod Integration**: DevPod launches a container using the worktree directory as the source
4. **Symlink Setup**: A symlink is created at `~/work` pointing to the worktree for shorter terminal prompts
5. **Metadata Tracking**: Worktree metadata is stored in `~/.devlaunch/metadata.json`

### Terminal Prompt

Instead of seeing a long path like `/workspaces/blooop-bencher-main/.worktrees/main` in your terminal prompt, you'll see `~/work`. This is achieved by creating a symlink:

```
/home/vscode/work -> /workspaces/{workspace_id}/.worktrees/{branch}
```

Git commands work normally from `~/work` since it's just a path alias.

### Directory Structure

```
~/.devlaunch/
├── repos/                           # Base repositories
│   └── blooop/
│       └── devlaunch/               # Base repo (shared git objects)
├── worktrees/                       # Git worktrees
│   └── blooop/
│       └── devlaunch/
│           ├── main/                # Worktree for main branch
│           └── feature-auth/        # Worktree for feature branch
└── metadata.json                    # Metadata about repos and worktrees
```

## Usage Examples

### Basic Usage

```bash
# Create workspace from main branch (uses worktree by default)
dl blooop/devlaunch

# Create workspace from specific branch (uses worktree by default)
dl blooop/devlaunch@feature-auth

# Force legacy DevPod backend
dl --backend devpod blooop/devlaunch@main

# Local paths always use DevPod backend
dl ./my-project
```

### Workspace Management

```bash
# List all workspaces (shows backend type)
dl --ls

# Delete workspace and optionally remove worktree
dl blooop/devlaunch rm
# Prompts: "Also remove the git worktree? [y/N]"

# Recreate workspace (reuses existing worktree)
dl blooop/devlaunch recreate
```

## Configuration

### Full Configuration Example

```toml
[worktree]
enabled = true                              # Enable worktree backend
repos_dir = "~/.devlaunch/repos"           # Where to store base repositories
worktrees_dir = "~/.devlaunch/worktrees"   # Where to store worktrees
auto_fetch = true                           # Auto-fetch updates when creating workspaces
fetch_interval = 3600                       # Seconds between auto-fetches

[worktree.cleanup]
auto_prune = true                           # Auto-remove unused worktrees
prune_after_days = 30                       # Remove worktrees unused for N days
```

## Backend Selection Logic

DevLaunch automatically selects the appropriate backend based on:

1. **Explicit Flag**: `--backend devpod` or `--backend worktree` overrides all other settings
2. **Environment Variable**: `DEVLAUNCH_BACKEND` sets the backend (`worktree` or `devpod`)
3. **Auto-Detection** (default behavior):
   - Git repositories (`owner/repo` format) → **Worktree backend** (default)
   - Local paths (`./path` or `/path`) → DevPod backend
   - Existing workspace names → DevPod backend
4. **Configuration**: Set `enabled = false` in config to disable worktree globally
5. **Automatic Fallback**: If worktree operations fail, automatically falls back to DevPod

## Performance Comparison

| Operation | DevPod Backend | Worktree Backend |
|-----------|---------------|------------------|
| First workspace (new repo) | ~30s (full clone) | ~30s (full clone) |
| New branch (same repo) | ~30s (full clone) | ~5s (worktree creation) |
| Disk usage (5 branches) | 5x repo size | 1.2x repo size |
| Git fetch | Per workspace | Shared across branches |
| Offline branch switch | Not possible | Instant |

## Troubleshooting

### Common Issues

1. **Permission Errors**: Ensure `~/.devlaunch/` is writable
2. **Git Version**: Requires git 2.5+ for worktree support
3. **Disk Space**: Initial clone requires full repo size

### Debug Mode

Enable debug logging to troubleshoot:
```bash
export LOG_LEVEL=DEBUG
dl --backend worktree owner/repo@branch
```

### Manual Cleanup

Remove unused worktrees:
```bash
# List all worktrees for a repo
cd ~/.devlaunch/repos/owner/repo
git worktree list

# Remove specific worktree
git worktree remove ~/.devlaunch/worktrees/owner/repo/branch

# Prune stale worktree references
git worktree prune
```

## Limitations

- **Git Submodules**: Limited support for repos with submodules
- **Large Files**: No git-lfs optimization yet
- **Windows**: Worktree paths may have issues on Windows

## Development

### Running Tests

```bash
# Run worktree-specific tests
pixi run pytest test/test_worktree_*.py -v

# Run all tests
pixi run pytest
```

### Architecture Components

- `devlaunch/worktree/models.py`: Data models for repos and worktrees
- `devlaunch/worktree/config.py`: Configuration management
- `devlaunch/worktree/repo_manager.py`: Base repository operations
- `devlaunch/worktree/worktree_manager.py`: Git worktree operations
- `devlaunch/worktree/workspace_manager.py`: DevPod integration
- `devlaunch/worktree/branch_manager.py`: Branch operations
- `devlaunch/worktree/storage.py`: Metadata persistence

## Future Enhancements

- [ ] Automatic cleanup of old worktrees
- [ ] Support for multiple remotes
- [ ] Git LFS optimization
- [ ] Worktree templates for quick setup
- [ ] Shallow clone support for large repos
- [ ] Migration tool for existing DevPod workspaces
