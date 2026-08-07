# Agent Instructions

## Development Environment

This project uses a devcontainer with pixi for environment management.

### Available Tools

- **GitHub CLI (`gh`)**: Available via `pixi run gh` or directly if using a login shell. `dl` forwards the host's GitHub token into the workspace as `GH_TOKEN`, so if the user is authenticated on the host, `gh` is authenticated here too.

  The bind mount of `~/.config/gh` alone is *not* enough: gh keeps its OAuth token in the host's system keyring, so the mounted `hosts.yml` carries the account name and `git_protocol` but no credential. `gh auth status` therefore also prints a failing `<user> (default)` entry for that credential-less mount — harmless, since the `GH_TOKEN` entry above it is the one gh uses, but it does make `gh auth status` exit non-zero. Gate scripts on a real call like `gh repo view` rather than on `gh auth status`.

### Running Commands

When using pixi tasks, prefer `pixi run <task>`. See `pixi task list` for available tasks.

For tools installed as dependencies (like `gh`), you can run them via:
- `pixi run gh <args>` - works in any shell
- `gh <args>` - works in login shells (`bash -l -c '...'`)

## Documentation Maintenance

- **Keep README up to date**: When modifying CLI commands, flags, or usage patterns, update the README.md to reflect the current tool behavior. Run `pixi run dl --help` to see the current help output and ensure the README matches.
