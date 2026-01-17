"""Example usage of devlaunch library functions."""

from devlaunch.dl import (
    expand_workspace_spec,
    is_path_spec,
    is_git_spec,
    validate_workspace_spec,
)

# Check if a spec is a path
print(f"Is './myproject' a path? {is_path_spec('./myproject')}")  # True
print(f"Is 'owner/repo' a path? {is_path_spec('owner/repo')}")  # False

# Check if a spec is a git reference
print(f"Is 'owner/repo' a git spec? {is_git_spec('owner/repo')}")  # True
print(f"Is './myproject' a git spec? {is_git_spec('./myproject')}")  # False

# Expand owner/repo to full URL
print(f"Expanded: {expand_workspace_spec('blooop/devlaunch')}")  # github.com/blooop/devlaunch

# Validate workspace specs
error = validate_workspace_spec("unknown", ["ws1", "ws2"])
print(f"Validation error: {error}")  # Returns error message

error = validate_workspace_spec("ws1", ["ws1", "ws2"])
print(f"Validation error for existing: {error}")  # None (valid)
