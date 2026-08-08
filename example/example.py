"""Example usage of devlaunch library functions."""

from devlaunch.dl import (
    expand_workspace_spec,
    is_path_spec,
    is_git_spec,
    spec_to_workspace_id,
)

# Check if a spec is a path
print(f"Is './myproject' a path? {is_path_spec('./myproject')}")  # True
print(f"Is 'owner/repo' a path? {is_path_spec('owner/repo')}")  # False

# Check if a spec is a git reference
print(f"Is 'owner/repo' a git spec? {is_git_spec('owner/repo')}")  # True
print(f"Is './myproject' a git spec? {is_git_spec('./myproject')}")  # False

# Expand owner/repo to full URL
print(f"Expanded: {expand_workspace_spec('blooop/devlaunch')}")  # github.com/blooop/devlaunch

# Derive the workspace a spec names. A branch spec is a full identity; a bare
# name is already one and comes back unchanged.
print(f"Workspace id: {spec_to_workspace_id('blooop/devlaunch@main')}")
print(f"Workspace id: {spec_to_workspace_id('myws')}")  # myws
