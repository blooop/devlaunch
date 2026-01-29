# DevLaunch Worktree Backend Implementation Plan

## Executive Summary

This document outlines the plan to convert DevLaunch from a pure DevPod wrapper to a worktree-based backend that uses git worktrees for repository management and DevPod for container launching.

## Current Architecture

### Current Flow
```
User Input: dl owner/repo@branch
    ↓
Parse & Validate Spec
    ↓
Expand to: github.com/owner/repo@branch
    ↓
Create Branch Remotely (if needed via SSH)
    ↓
Generate Workspace ID
    ↓
DevPod Up: devpod up github.com/owner/repo@branch --id <workspace-id>
    ↓
DevPod clones repo into container
    ↓
DevPod creates/starts container
    ↓
DevLaunch SSH into container
```

### Current Limitations
1. **Slow cloning**: Each branch requires a full git clone
2. **Disk space**: Multiple clones of the same repo waste disk space
3. **No local git history sharing**: Each workspace has isolated git history
4. **Network dependent**: Every new branch requires network access
5. **Slow branch creation**: Must clone entire repo for each branch

## Proposed Worktree Backend Architecture

### Core Concept

Use **git worktrees** to manage multiple branches of the same repository locally, then launch DevPod containers pointing to these worktree directories as local paths.

### New Flow
```
User Input: dl owner/repo@branch
    ↓
Parse & Validate Spec
    ↓
Check for Base Repository
    ├─ Not exists → Clone base repo to ~/.devlaunch/repos/<owner>/<repo>
    └─ Exists → Fetch latest changes
    ↓
Check for Worktree
    ├─ Not exists → Create worktree for branch at ~/.devlaunch/worktrees/<owner>/<repo>/<branch>
    └─ Exists → Ensure branch is up to date
    ↓
Generate Workspace ID (branch name)
    ↓
DevPod Up: devpod up ~/.devlaunch/worktrees/<owner>/<repo>/<branch> --id <workspace-id>
    ↓
DevPod uses local path (no clone needed)
    ↓
DevPod creates/starts container with mounted worktree
    ↓
DevLaunch SSH into container
```

### Benefits

1. **Faster workspace creation**: No cloning required after initial repo clone
2. **Disk space efficiency**: Shared git objects across all branches
3. **Faster git operations**: Shared repository history
4. **Offline capable**: Work with existing branches without network
5. **Better branch management**: Native git worktree operations
6. **Faster branch switching**: Worktrees are already checked out

## Architecture Design

### Directory Structure

```
~/.devlaunch/
├── repos/                           # Base repositories (bare or regular)
│   └── <owner>/
│       └── <repo>/                  # Base repo (e.g., ~/.devlaunch/repos/blooop/devlaunch)
│           ├── .git/                # Git directory
│           └── ...                  # Working tree of default branch (optional)
├── worktrees/                       # Git worktrees
│   └── <owner>/
│       └── <repo>/
│           ├── main/                # Worktree for main branch
│           ├── feature-auth/        # Worktree for feature/auth branch
│           └── bugfix-login/        # Worktree for bugfix/login branch
└── metadata.json                    # Metadata about repos and worktrees
```

### Data Model

#### Repository Metadata
```python
@dataclass
class BaseRepository:
    """Represents a base git repository."""
    owner: str
    repo: str
    remote_url: str
    local_path: Path
    default_branch: str
    last_fetched: datetime
    worktrees: List[str]  # List of active worktree branch names
```

#### Worktree Metadata
```python
@dataclass
class WorktreeInfo:
    """Represents a git worktree."""
    owner: str
    repo: str
    branch: str
    local_path: Path
    workspace_id: str
    created_at: datetime
    last_used: datetime
    devpod_workspace_id: Optional[str]  # Associated DevPod workspace
```

### Core Components

#### 1. Repository Manager (`devlaunch/worktree/repo_manager.py`)

Handles base repository operations:

```python
class RepositoryManager:
    """Manages base git repositories."""

    def __init__(self, repos_dir: Path):
        self.repos_dir = repos_dir

    def get_repo_path(self, owner: str, repo: str) -> Path:
        """Get local path for a repository."""
        return self.repos_dir / owner / repo

    def clone_repo(self, owner: str, repo: str, remote_url: str) -> BaseRepository:
        """Clone a new base repository."""
        # Clone to repos_dir/owner/repo
        # Record metadata
        pass

    def fetch_repo(self, owner: str, repo: str) -> None:
        """Fetch latest changes from remote."""
        # git fetch --all
        pass

    def ensure_repo(self, owner: str, repo: str, remote_url: str) -> BaseRepository:
        """Ensure repo exists locally, clone if needed."""
        if self.repo_exists(owner, repo):
            self.fetch_repo(owner, repo)
            return self.get_repo(owner, repo)
        return self.clone_repo(owner, repo, remote_url)

    def repo_exists(self, owner: str, repo: str) -> bool:
        """Check if repository exists locally."""
        return self.get_repo_path(owner, repo).exists()
```

#### 2. Worktree Manager (`devlaunch/worktree/worktree_manager.py`)

Handles git worktree operations:

```python
class WorktreeManager:
    """Manages git worktrees."""

    def __init__(self, worktrees_dir: Path, repo_manager: RepositoryManager):
        self.worktrees_dir = worktrees_dir
        self.repo_manager = repo_manager

    def get_worktree_path(self, owner: str, repo: str, branch: str) -> Path:
        """Get local path for a worktree."""
        return self.worktrees_dir / owner / repo / sanitize_branch_name(branch)

    def create_worktree(self, owner: str, repo: str, branch: str) -> WorktreeInfo:
        """Create a new git worktree for a branch."""
        # Ensure base repo exists
        base_repo = self.repo_manager.ensure_repo(owner, repo, remote_url)

        # Create worktree
        # git worktree add <path> <branch>
        pass

    def remove_worktree(self, owner: str, repo: str, branch: str) -> None:
        """Remove a git worktree."""
        # git worktree remove <path>
        pass

    def list_worktrees(self, owner: str, repo: str) -> List[WorktreeInfo]:
        """List all worktrees for a repository."""
        # git worktree list
        pass

    def ensure_worktree(self, owner: str, repo: str, branch: str) -> WorktreeInfo:
        """Ensure worktree exists, create if needed."""
        if self.worktree_exists(owner, repo, branch):
            return self.get_worktree(owner, repo, branch)
        return self.create_worktree(owner, repo, branch)

    def worktree_exists(self, owner: str, repo: str, branch: str) -> bool:
        """Check if worktree exists."""
        return self.get_worktree_path(owner, repo, branch).exists()
```

#### 3. Workspace Manager (`devlaunch/worktree/workspace_manager.py`)

Integrates worktrees with DevPod:

```python
class WorkspaceManager:
    """Manages DevPod workspaces backed by worktrees."""

    def __init__(self, worktree_manager: WorktreeManager):
        self.worktree_manager = worktree_manager

    def create_workspace(
        self,
        owner: str,
        repo: str,
        branch: str,
        workspace_id: Optional[str] = None
    ) -> WorktreeInfo:
        """Create a workspace from a worktree."""
        # Ensure worktree exists
        worktree = self.worktree_manager.ensure_worktree(owner, repo, branch)

        # Launch DevPod with local path
        # devpod up <worktree.local_path> --id <workspace_id>

        # Update worktree metadata with devpod workspace ID
        pass

    def start_workspace(self, workspace_id: str) -> None:
        """Start an existing workspace."""
        # devpod up <workspace_id>
        pass

    def stop_workspace(self, workspace_id: str) -> None:
        """Stop a workspace."""
        # devpod stop <workspace_id>
        pass

    def delete_workspace(self, workspace_id: str, remove_worktree: bool = False) -> None:
        """Delete a DevPod workspace and optionally remove the worktree."""
        # devpod delete <workspace_id>

        if remove_worktree:
            # Remove the associated worktree
            pass
```

#### 4. Branch Manager (`devlaunch/worktree/branch_manager.py`)

Handles branch operations (create, fetch, track):

```python
class BranchManager:
    """Manages git branch operations."""

    def ensure_branch_exists(
        self,
        base_repo_path: Path,
        branch: str,
        remote: str = "origin",
        create_remote: bool = True
    ) -> None:
        """Ensure branch exists locally and optionally remotely."""
        # Check if branch exists locally
        # Check if branch exists remotely
        # Create if needed (reuse existing create_remote_branch logic)
        pass

    def create_local_branch(self, base_repo_path: Path, branch: str, start_point: str = "HEAD") -> None:
        """Create a new local branch."""
        # git branch <branch> <start_point>
        pass

    def track_remote_branch(self, base_repo_path: Path, branch: str, remote: str = "origin") -> None:
        """Set up tracking for a remote branch."""
        # git branch --set-upstream-to=<remote>/<branch> <branch>
        pass
```

### Integration with Existing Code

#### Modified Functions in `dl.py`

1. **`main()` function**:
   - Initialize worktree managers
   - Route spec parsing to worktree backend when appropriate
   - Maintain backward compatibility with existing workspace names

2. **`expand_workspace_spec()`**:
   - Check if spec should use worktree backend (git repos)
   - For paths, maintain existing behavior
   - For existing workspace IDs, maintain existing behavior

3. **`workspace_up()`**:
   - Detect if workspace should use worktree backend
   - If yes, delegate to `WorkspaceManager.create_workspace()`
   - Otherwise, use existing DevPod flow

4. **`workspace_delete()`**:
   - Add option to also remove worktree
   - Prompt user if they want to keep worktree

5. **`list_workspaces()`**:
   - Merge DevPod workspaces with worktree metadata
   - Show worktree path in listing

### Configuration

#### Settings in `~/.config/devlaunch/config.toml`

```toml
[worktree]
enabled = true  # Enable/disable worktree backend
repos_dir = "~/.devlaunch/repos"
worktrees_dir = "~/.devlaunch/worktrees"
auto_fetch = true  # Auto-fetch on workspace creation
fetch_interval = 3600  # Seconds between auto-fetches

[worktree.cleanup]
auto_prune = true  # Auto-remove unused worktrees
prune_after_days = 30  # Remove worktrees unused for N days
```

## Implementation Plan

### Phase 1: Core Infrastructure (Foundation)

**Goal**: Set up the basic worktree infrastructure without breaking existing functionality.

#### 1.1 Project Structure Setup
- [ ] Create `devlaunch/worktree/` module directory
- [ ] Create `devlaunch/worktree/__init__.py`
- [ ] Create `devlaunch/worktree/models.py` (data models)
- [ ] Create `devlaunch/worktree/config.py` (configuration management)
- [ ] Add new dependencies to `pyproject.toml` if needed

**Files to create**:
- `devlaunch/worktree/__init__.py`
- `devlaunch/worktree/models.py`
- `devlaunch/worktree/config.py`

**Estimated complexity**: Low

#### 1.2 Data Models
- [ ] Define `BaseRepository` dataclass
- [ ] Define `WorktreeInfo` dataclass
- [ ] Define `WorktreeConfig` dataclass
- [ ] Implement JSON serialization/deserialization
- [ ] Create metadata storage utilities

**Files to modify/create**:
- `devlaunch/worktree/models.py`
- `devlaunch/worktree/storage.py` (new)

**Estimated complexity**: Low

#### 1.3 Configuration Management
- [ ] Define default configuration values
- [ ] Implement config file loading from `~/.config/devlaunch/config.toml`
- [ ] Implement config merging (defaults + user overrides)
- [ ] Add config validation
- [ ] Create config migration utilities for existing users

**Files to create**:
- `devlaunch/worktree/config.py`

**Estimated complexity**: Medium

### Phase 2: Repository Management

**Goal**: Implement base repository cloning and management.

#### 2.1 Repository Manager
- [ ] Implement `RepositoryManager` class
- [ ] Implement `clone_repo()` - clone base repository
- [ ] Implement `fetch_repo()` - fetch updates from remote
- [ ] Implement `ensure_repo()` - ensure repo exists locally
- [ ] Implement `repo_exists()` - check if repo exists
- [ ] Implement `get_repo()` - get repository metadata
- [ ] Add error handling for git operations
- [ ] Add logging for all operations

**Files to create**:
- `devlaunch/worktree/repo_manager.py`

**Key considerations**:
- Should we use bare repos or regular repos as base?
- How to handle authentication (SSH keys, tokens)?
- How to handle git errors gracefully?

**Estimated complexity**: Medium-High

#### 2.2 Repository Management Tests
- [ ] Test cloning new repositories
- [ ] Test fetching existing repositories
- [ ] Test error handling (network failures, auth failures)
- [ ] Test concurrent operations
- [ ] Mock git commands for unit tests

**Files to create**:
- `test/test_repo_manager.py`

**Estimated complexity**: Medium

### Phase 3: Worktree Management

**Goal**: Implement git worktree creation and management.

#### 3.1 Worktree Manager
- [ ] Implement `WorktreeManager` class
- [ ] Implement `create_worktree()` - create git worktree
- [ ] Implement `remove_worktree()` - remove git worktree
- [ ] Implement `list_worktrees()` - list all worktrees
- [ ] Implement `ensure_worktree()` - ensure worktree exists
- [ ] Implement `worktree_exists()` - check if worktree exists
- [ ] Implement `get_worktree()` - get worktree metadata
- [ ] Add error handling for git worktree operations
- [ ] Handle branch name sanitization (special characters, slashes)

**Files to create**:
- `devlaunch/worktree/worktree_manager.py`

**Key considerations**:
- How to handle branch names with special characters?
- What happens if worktree directory is deleted manually?
- How to sync worktree metadata with actual git worktrees?

**Estimated complexity**: Medium-High

#### 3.2 Branch Manager
- [ ] Implement `BranchManager` class
- [ ] Implement `ensure_branch_exists()` - ensure branch exists locally/remotely
- [ ] Implement `create_local_branch()` - create new local branch
- [ ] Implement `track_remote_branch()` - set up tracking
- [ ] Refactor existing `create_remote_branch()` logic from `dl.py`
- [ ] Refactor existing `remote_branch_exists()` logic from `dl.py`
- [ ] Refactor existing `get_remote_branches()` logic from `dl.py`

**Files to create**:
- `devlaunch/worktree/branch_manager.py`

**Files to modify**:
- `devlaunch/dl.py` (extract branch management code)

**Estimated complexity**: Medium

#### 3.3 Worktree Management Tests
- [ ] Test creating worktrees
- [ ] Test removing worktrees
- [ ] Test listing worktrees
- [ ] Test branch name sanitization
- [ ] Test concurrent worktree operations
- [ ] Test error scenarios (disk full, permission denied)

**Files to create**:
- `test/test_worktree_manager.py`
- `test/test_branch_manager.py`

**Estimated complexity**: Medium

### Phase 4: DevPod Integration

**Goal**: Integrate worktrees with DevPod for container launching.

#### 4.1 Workspace Manager
- [ ] Implement `WorkspaceManager` class
- [ ] Implement `create_workspace()` - create DevPod workspace from worktree
- [ ] Implement `start_workspace()` - start existing workspace
- [ ] Implement `stop_workspace()` - stop workspace
- [ ] Implement `delete_workspace()` - delete workspace and optionally worktree
- [ ] Handle workspace ID generation for worktree-backed workspaces
- [ ] Handle DevPod command failures gracefully
- [ ] Update worktree metadata with DevPod workspace associations

**Files to create**:
- `devlaunch/worktree/workspace_manager.py`

**Key considerations**:
- How to distinguish worktree-backed workspaces from regular DevPod workspaces?
- Should workspace ID include repo information or just branch name?
- How to handle DevPod workspace deletion vs worktree removal?

**Estimated complexity**: Medium-High

#### 4.2 Workspace Manager Tests
- [ ] Test workspace creation from worktree
- [ ] Test workspace lifecycle (start, stop, delete)
- [ ] Test workspace ID generation
- [ ] Test handling of DevPod failures
- [ ] Mock DevPod commands for unit tests

**Files to create**:
- `test/test_workspace_manager.py`

**Estimated complexity**: Medium

### Phase 5: Main CLI Integration

**Goal**: Integrate worktree backend with main CLI, maintaining backward compatibility.

#### 5.1 Backend Detection and Routing
- [ ] Add `--backend` flag to `dl` CLI (worktree|devpod|auto)
- [ ] Implement backend detection logic in `main()`
- [ ] Route git specs to worktree backend
- [ ] Route existing workspace IDs to DevPod backend
- [ ] Route local paths based on configuration
- [ ] Add `DEVLAUNCH_BACKEND` environment variable support

**Files to modify**:
- `devlaunch/dl.py` (main function)

**Estimated complexity**: Medium

#### 5.2 Modify Core Functions
- [ ] Update `expand_workspace_spec()` to support worktree backend
- [ ] Update `workspace_up()` to route to appropriate backend
- [ ] Update `workspace_delete()` to handle worktree cleanup
- [ ] Update `list_workspaces()` to show worktree-backed workspaces
- [ ] Update `fuzzy_select_workspace()` to show worktree information
- [ ] Ensure `workspace_ssh()`, `workspace_stop()` work with both backends

**Files to modify**:
- `devlaunch/dl.py`

**Key considerations**:
- How to maintain backward compatibility with existing workspaces?
- Should we migrate existing DevPod workspaces to worktrees?
- How to show backend information in workspace listings?

**Estimated complexity**: Medium-High

#### 5.3 Update Help and Documentation
- [ ] Update `print_help()` to document worktree backend
- [ ] Add documentation about backend selection
- [ ] Add examples of worktree usage
- [ ] Update completion cache to include worktree information

**Files to modify**:
- `devlaunch/dl.py`
- `README.md`

**Estimated complexity**: Low

### Phase 6: Advanced Features

**Goal**: Add advanced worktree management features.

#### 6.1 Cleanup and Maintenance
- [ ] Implement `prune_worktrees()` - remove unused worktrees
- [ ] Implement `cleanup_repos()` - remove unused base repositories
- [ ] Add `--prune` command to CLI
- [ ] Add automatic cleanup based on configuration
- [ ] Add disk usage reporting

**Files to create**:
- `devlaunch/worktree/cleanup.py`

**Files to modify**:
- `devlaunch/dl.py` (add prune command)

**Estimated complexity**: Medium

#### 6.2 Workspace Migration
- [ ] Implement migration tool for existing DevPod workspaces
- [ ] Add `--migrate` command to convert workspace to worktree
- [ ] Handle migration of workspace data
- [ ] Add migration testing

**Files to create**:
- `devlaunch/worktree/migration.py`

**Files to modify**:
- `devlaunch/dl.py` (add migrate command)

**Estimated complexity**: High

#### 6.3 Enhanced Completion
- [ ] Update completion cache with worktree information
- [ ] Add worktree path completion
- [ ] Add backend selection completion
- [ ] Update bash completion script

**Files to modify**:
- `devlaunch/completion.py`
- `devlaunch/completions/dl.bash`

**Estimated complexity**: Medium

### Phase 7: Testing and Documentation

**Goal**: Ensure comprehensive testing and documentation.

#### 7.1 Integration Tests
- [ ] Test end-to-end workspace creation with worktrees
- [ ] Test workspace lifecycle with both backends
- [ ] Test backend switching
- [ ] Test error scenarios
- [ ] Test concurrent operations
- [ ] Test migration scenarios

**Files to create**:
- `test/test_integration_worktree.py`

**Estimated complexity**: High

#### 7.2 Documentation
- [ ] Update README.md with worktree backend information
- [ ] Create WORKTREE_BACKEND.md user guide
- [ ] Document configuration options
- [ ] Add troubleshooting guide
- [ ] Add architecture diagrams
- [ ] Update CLAUDE.md if needed

**Files to modify/create**:
- `README.md`
- `WORKTREE_BACKEND.md` (new)
- `docs/architecture.md` (new, optional)

**Estimated complexity**: Medium

#### 7.3 Performance Testing
- [ ] Benchmark workspace creation time (worktree vs DevPod)
- [ ] Benchmark disk space usage
- [ ] Benchmark git operation performance
- [ ] Create performance comparison report

**Files to create**:
- `test/benchmark_worktree.py`

**Estimated complexity**: Medium

### Phase 8: Polish and Release

**Goal**: Prepare for release.

#### 8.1 Bug Fixes and Polish
- [ ] Fix any bugs found during testing
- [ ] Improve error messages
- [ ] Add progress indicators for long operations
- [ ] Optimize performance bottlenecks
- [ ] Code review and refactoring

**Estimated complexity**: Variable

#### 8.2 Release Preparation
- [ ] Update version number
- [ ] Update CHANGELOG.md
- [ ] Update pyproject.toml
- [ ] Update conda recipe if needed
- [ ] Create release notes
- [ ] Tag release

**Files to modify**:
- `pyproject.toml`
- `CHANGELOG.md` (create if doesn't exist)

**Estimated complexity**: Low

## Technical Decisions to Make

### 1. Base Repository Type
**Question**: Should we use bare repositories or regular repositories as the base?

**Options**:
- **Bare repository**: More efficient, less disk space, but can't be used directly
- **Regular repository**: Can be used as a workspace itself, but uses more disk space

**Recommendation**: Regular repository with default branch checked out. This allows the base repo to also serve as a workspace for the default branch.

### 2. Workspace ID Strategy
**Question**: How should we generate workspace IDs for worktree-backed workspaces?

**Options**:
- **Branch name only**: Simple, but collisions possible across repos
- **`<owner>-<repo>-<branch>`**: Unique, but verbose
- **Keep existing behavior**: Use branch name, accept potential collisions

**Recommendation**: Keep existing behavior (branch name) for backward compatibility, but add repo information to metadata for disambiguation.

### 3. Backward Compatibility
**Question**: How do we handle existing DevPod workspaces?

**Options**:
- **Auto-migrate**: Automatically convert to worktrees on first use
- **Manual migration**: Require user to explicitly migrate
- **Dual mode**: Support both backends simultaneously

**Recommendation**: Dual mode with optional manual migration tool. This provides maximum flexibility and no disruption.

### 4. Default Backend
**Question**: Should worktree backend be the default?

**Options**:
- **Worktree default**: New installs use worktree, old installs keep DevPod
- **DevPod default**: Keep backward compatibility, opt-in to worktree
- **Auto-detect**: Use worktree for git repos, DevPod for paths

**Recommendation**: Auto-detect with config override. This provides best user experience without breaking existing workflows.

### 5. Cleanup Strategy
**Question**: When should worktrees and base repositories be cleaned up?

**Options**:
- **Manual only**: User must explicitly prune
- **Automatic**: Auto-prune after N days of inactivity
- **On-delete**: Remove worktree when workspace is deleted

**Recommendation**: Configurable with sensible defaults:
- Worktrees: Remove when workspace is deleted (with confirmation)
- Base repos: Manual cleanup only (they're shared across worktrees)

## Migration Path for Existing Users

### Scenario 1: New Installation
- Worktree backend enabled by default
- User experience unchanged (transparent backend)

### Scenario 2: Existing DevPod Workspaces
- Existing workspaces continue to work
- New workspaces use worktree backend
- User can migrate workspaces individually with `dl migrate <workspace>`

### Scenario 3: Opt-out
- User can disable worktree backend in config
- All operations fall back to DevPod backend
- No functionality lost

## Risk Analysis

### Technical Risks

1. **Git Worktree Limitations**
   - **Risk**: Git worktrees have known issues with submodules
   - **Mitigation**: Document limitations, provide fallback to DevPod for repos with submodules

2. **DevPod Local Path Support**
   - **Risk**: DevPod might have issues with local paths vs git URLs
   - **Mitigation**: Thoroughly test local path support, maintain DevPod backend fallback

3. **Concurrent Access**
   - **Risk**: Multiple processes might conflict when accessing worktrees
   - **Mitigation**: Implement file locking for metadata updates

4. **Disk Space**
   - **Risk**: Base repositories can grow large over time
   - **Mitigation**: Implement cleanup tools, git gc, disk usage monitoring

### User Experience Risks

1. **Complexity**
   - **Risk**: Worktree concept might confuse users
   - **Mitigation**: Hide implementation details, maintain simple CLI interface

2. **Migration Friction**
   - **Risk**: Existing users might resist change
   - **Mitigation**: Make migration optional, provide clear benefits, maintain backward compatibility

3. **Debugging Difficulty**
   - **Risk**: Worktree issues harder to debug than simple DevPod workspaces
   - **Mitigation**: Add verbose logging, troubleshooting guide, clear error messages

## Success Criteria

1. **Performance**
   - Workspace creation < 5s for existing repos (vs ~30s+ with DevPod clone)
   - Disk usage reduced by 50%+ for multiple branches of same repo

2. **Compatibility**
   - All existing DevPod workspaces continue to work
   - All existing CLI commands work with both backends
   - No breaking changes to CLI interface

3. **Testing**
   - >90% code coverage for new worktree modules
   - All integration tests pass
   - Performance benchmarks meet targets

4. **Documentation**
   - Complete user guide
   - Migration guide
   - Troubleshooting guide
   - Architecture documentation

## Timeline Estimate

**Total estimated effort**: 15-25 development days (assuming 1 developer)

- Phase 1: 2-3 days
- Phase 2: 3-4 days
- Phase 3: 4-5 days
- Phase 4: 3-4 days
- Phase 5: 3-4 days
- Phase 6: 2-3 days
- Phase 7: 3-4 days
- Phase 8: 1-2 days

**Note**: This is a sequential estimate. With multiple developers or parallel work on independent phases, timeline could be reduced.

## Open Questions

1. How should we handle SSH key authentication for private repos?
2. Should we support GitLab/other git hosting platforms equally?
3. What happens if a user manually modifies worktree directories?
4. Should we implement a "workspace sync" command to sync worktree state with remote?
5. How do we handle very large repositories (>1GB)?
6. Should we support sparse checkouts in worktrees for monorepos?
7. Do we need a "workspace archive" feature for long-term storage?

## Next Steps

1. **Review this plan** with stakeholders/maintainers
2. **Make technical decisions** on open questions
3. **Set up development branch** for worktree backend
4. **Begin Phase 1 implementation**
5. **Create tracking issues** for each phase
6. **Set up CI/CD** for worktree backend testing

## Appendix: Alternative Approaches Considered

### Alternative 1: DevPod Plugin
**Idea**: Implement worktree support as a DevPod provider plugin

**Pros**:
- Native DevPod integration
- Could benefit entire DevPod ecosystem

**Cons**:
- More complex development
- Depends on DevPod plugin API
- Less control over user experience

**Decision**: Rejected due to complexity and tight coupling with DevPod internals

### Alternative 2: Git Submodules
**Idea**: Use git submodules instead of worktrees

**Pros**:
- More widely understood
- Better tooling support

**Cons**:
- More complex than worktrees
- Doesn't solve the disk space problem
- Adds .gitmodules complexity

**Decision**: Rejected - worktrees are simpler and more appropriate for this use case

### Alternative 3: Custom Git Backend
**Idea**: Implement custom git repository management without worktrees

**Pros**:
- Full control over implementation
- Could optimize for specific use cases

**Cons**:
- Reinventing the wheel
- High maintenance burden
- Likely to have bugs

**Decision**: Rejected - worktrees are a proven git feature that solves our exact use case

---

**Document Version**: 1.0
**Last Updated**: 2026-01-19
**Author**: Claude (DevLaunch Planning Agent)
**Status**: Draft for Review
