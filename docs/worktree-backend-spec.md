# Worktree Backend Specification

## Overview

The worktree backend for devlaunch (`dl`) provides an efficient way to work with multiple branches of the same repository simultaneously. Instead of cloning the entire repository for each branch/workspace, it clones once and uses git worktrees for each branch.

## Goals

1. **Efficiency**: Clone repository once, share git objects across all branches
2. **Multiple branches**: Allow working on multiple branches of the same repo simultaneously
3. **Container compatibility**: Git commands must work inside DevPod containers
4. **Descriptive naming**: Workspace IDs should include owner/repo/branch information

## Architecture

### Directory Structure

```
~/.cache/devlaunch/
├── repos/
│   └── {owner}/
│       └── {repo}/                    # Base repository (bare-ish clone)
│           ├── .git/                  # Git directory with all objects
│           │   └── worktrees/         # Git's internal worktree metadata
│           │       └── {branch}/      # Metadata for each worktree
│           └── .worktrees/            # Actual worktree directories
│               └── {branch}/          # Working directory for branch
│                   └── .git           # FILE pointing to ../.git/worktrees/{branch}
└── metadata.json                      # Devlaunch tracking data
```

### Component Interaction

```
User runs: dl owner/repo@branch
            │
            ▼
┌─────────────────────────────────────────────────────────────────┐
│ dl.py (CLI)                                                     │
│  - Parses owner/repo@branch                                     │
│  - Determines workspace_id: owner-repo-branch                   │
│  - Calls workspace_up_worktree()                                │
└─────────────────────────────────────────────────────────────────┘
            │
            ▼
┌─────────────────────────────────────────────────────────────────┐
│ workspace_manager.py                                            │
│  - Ensures worktree exists (via worktree_manager)               │
│  - Calls: devpod up {base_repo_path} --id {workspace_id}        │
│  - Returns worktree info                                        │
└─────────────────────────────────────────────────────────────────┘
            │
            ▼
┌─────────────────────────────────────────────────────────────────┐
│ worktree_manager.py                                             │
│  - Clones repo if not exists (via repo_manager)                 │
│  - Creates git worktree: git worktree add .worktrees/{branch}   │
│  - Fixes .git file to use relative paths                        │
└─────────────────────────────────────────────────────────────────┘
            │
            ▼
┌─────────────────────────────────────────────────────────────────┐
│ DevPod                                                          │
│  - Mounts base repo to /workspaces/{workspace_id}/              │
│  - Runs devcontainer                                            │
└─────────────────────────────────────────────────────────────────┘
            │
            ▼
┌─────────────────────────────────────────────────────────────────┐
│ dl.py (SSH)                                                     │
│  - Calls: devpod ssh {workspace_id}                             │
│           --workdir /workspaces/{workspace_id}/.worktrees/{branch}
└─────────────────────────────────────────────────────────────────┘
```

## Technical Challenges

### Challenge 1: Git Worktree Absolute Paths

**Problem**: Git worktrees use absolute paths by default.

When you create a worktree:
```bash
git worktree add /home/user/.cache/devlaunch/repos/owner/repo/.worktrees/main
```

Git creates a `.git` FILE (not directory) in the worktree containing:
```
gitdir: /home/user/.cache/devlaunch/repos/owner/repo/.git/worktrees/main
```

This absolute path does NOT exist inside a container because only the repo is mounted.

**Solution**: After creating the worktree, rewrite the `.git` file to use relative paths:
```
gitdir: ../../.git/worktrees/main
```

Also update the reverse pointer in `.git/worktrees/{name}/gitdir` to use relative paths.

**Implementation**: `worktree_manager.py:_fix_worktree_paths()`

### Challenge 2: What to Mount in Container

**Problem**: DevPod mounts a single directory into the container.

If we mount only the worktree directory:
- ❌ `.git/` directory is not accessible
- ❌ Git commands fail: "not a git repository"

If we mount the base repo directory:
- ✅ `.git/` directory is accessible
- ✅ All worktrees are accessible
- ✅ Git commands work (with relative path fix)

**Solution**: Mount the BASE REPO, not the worktree.

```bash
# WRONG - mounts only worktree, .git not accessible
devpod up ~/.cache/devlaunch/repos/owner/repo/.worktrees/main --id main

# CORRECT - mounts base repo, .git accessible
devpod up ~/.cache/devlaunch/repos/owner/repo --id owner-repo-main
```

**Implementation**: `workspace_manager.py:_create_workspace_locked()`

### Challenge 3: Container Working Directory

**Problem**: After mounting the base repo, the container starts in the base repo root, not the worktree.

Container layout:
```
/workspaces/{workspace_id}/           # Base repo (mounted here)
├── .git/                             # Git objects
└── .worktrees/
    └── {branch}/                     # User wants to be HERE
```

**Solution**: Create a symlink at `~/work` pointing to the worktree, then SSH with `--workdir ~/work`.

```bash
# First, create the symlink inside the container
devpod ssh {workspace_id} --command "ln -sfn /workspaces/{workspace_id}/.worktrees/{branch} /home/vscode/work"

# Then SSH to the symlink path
devpod ssh {workspace_id} --workdir /home/vscode/work
```

This provides:
- Short, consistent path: `~/work` in terminal prompt instead of long worktree path
- Git commands still work (symlink resolves to actual worktree)
- Works with any repo's existing devcontainer configuration

**Implementation**: `dl.py:setup_worktree_symlink()`, `get_worktree_symlink_path()`, and `workspace_ssh()`

### Challenge 4: Git Detached HEAD State

**Problem**: When creating a worktree tracking a remote branch:
```bash
git worktree add .worktrees/main origin/main
```

Git checks out the commit directly, resulting in "detached HEAD" state:
```
$ git status
Not currently on any branch.
```

**Solution**: Create a local branch that tracks the remote:
```bash
# Option A: Create with -b flag
git worktree add -b main .worktrees/main origin/main

# Option B: If branch already exists locally
git worktree add .worktrees/main main
```

**Current Status**: NOT IMPLEMENTED - worktrees are created in detached HEAD state.

**Implementation needed**: `worktree_manager.py:create_worktree()`

### Challenge 5: Workspace Naming

**Problem**: Multiple repos could have the same branch name (e.g., "main").

If workspace ID is just the branch name:
- `dl owner1/repo1` → workspace "main"
- `dl owner2/repo2` → workspace "main" (CONFLICT!)

**Solution**: Include owner-repo-branch in workspace ID:
- `dl owner1/repo1` → workspace "owner1-repo1-main"
- `dl owner2/repo2` → workspace "owner2-repo2-main"

**Implementation**: `dl.py:make_worktree_workspace_id()`

### Challenge 6: Purging Data

**Problem**: `dl --purge` removes local files but leaves DevPod workspaces orphaned.

Orphaned workspaces have cached container configurations that may be stale.

**Solution**: Before removing local files, delete all tracked DevPod workspaces.

**Implementation**: `dl.py:purge_all_data()`

### Challenge 7: devcontainer.json workspaceFolder

**Problem**: The target repo's devcontainer.json may specify a `workspaceFolder`.

If devcontainer.json says:
```json
{
  "workspaceFolder": "/workspaces/myproject"
}
```

But we mount to `/workspaces/owner-repo-main`, there's a mismatch.

**Current Status**: NOT ADDRESSED - we rely on DevPod's default behavior.

**Potential Solutions**:
1. Ignore devcontainer's workspaceFolder and always use our mount point
2. Parse devcontainer.json and adapt
3. Use DevPod's override mechanisms

## Current Issues

### Issue: Detached HEAD State

**Symptom**:
```bash
$ git status
Not currently on any branch.
nothing to commit, working tree clean
```

**Cause**: Worktree created with `git worktree add <path> origin/<branch>` instead of creating a local tracking branch.

**Fix Required**: Modify `worktree_manager.py:create_worktree()` to:
1. Check if local branch exists
2. If yes: `git worktree add <path> <branch>`
3. If no: `git worktree add -b <branch> <path> origin/<branch>`

## Test Gaps

The current tests mock DevPod commands and don't verify:
1. Actual git worktree creation and state
2. Container mount paths
3. SSH working directory behavior
4. Git command functionality inside containers

**Recommendation**: Add integration tests that:
1. Create actual worktrees (not mocked)
2. Verify `.git` file contents
3. Verify git commands work in worktree
4. Test the full flow with a real or simulated DevPod

## Configuration

### Environment Variables

- `DEVLAUNCH_BACKEND`: Set to `worktree` (default for git repos) or `devpod` (legacy)
- `XDG_CACHE_HOME`: Override cache directory location

### CLI Flags

- `--backend worktree|devpod`: Override backend for single command
- `--purge [-y]`: Remove all devlaunch data and DevPod workspaces

## File Reference

| File | Purpose |
|------|---------|
| `dl.py` | CLI entry point, workspace ID generation, SSH handling, symlink setup |
| `worktree/workspace_manager.py` | DevPod integration, container lifecycle |
| `worktree/worktree_manager.py` | Git worktree creation and management |
| `worktree/repo_manager.py` | Repository cloning and management |
| `worktree/storage.py` | Metadata persistence |
| `worktree/models.py` | Data classes for repos and worktrees |
| `worktree/config.py` | Configuration and paths |

## Key Functions

### `get_worktree_container_path(workspace_id, branch)`
Returns the full container path to the worktree directory.
Example: `/workspaces/blooop-bencher-main/.worktrees/main`

### `get_worktree_symlink_path()`
Returns the symlink path used for shorter terminal prompts.
Always returns `/home/vscode/work`.

### `setup_worktree_symlink(workspace_id, worktree_container_path)`
Creates the `~/work` symlink inside the container pointing to the worktree.
Called before SSH'ing to ensure the symlink exists.

## Success Criteria

A successful worktree backend implementation should:

1. ✅ Clone repository only once per owner/repo
2. ✅ Create worktrees for each branch
3. ✅ Generate unique workspace IDs (owner-repo-branch)
4. ⚠️ Have working git commands in container (PARTIAL - detached HEAD issue)
5. ✅ Start shell in correct worktree directory
6. ✅ Clean up DevPod workspaces on purge
7. ❌ Have local branch tracking remote (NOT IMPLEMENTED)
