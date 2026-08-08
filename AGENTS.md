# Agent Instructions

## Development Environment

This project uses a devcontainer with pixi for environment management.

### Available Tools

- **GitHub CLI (`gh`)**: Available via `pixi run gh` or directly if using a login shell. Authentication comes from the host: `dl` forwards the host's token into every workspace it opens as `GH_TOKEN`, and this project's devcontainer also mounts `~/.config/gh` for containers started some other way (VS Code, plain `devpod up`).

### Running Commands

When using pixi tasks, prefer `pixi run <task>`. See `pixi task list` for available tasks.

For tools installed as dependencies (like `gh`), you can run them via:
- `pixi run gh <args>` - works in any shell
- `gh <args>` - works in login shells (`bash -l -c '...'`)

## Two installs: `dl` and `dl-next`

`dl` and `aid` on PATH are the **released** build, installed by pixi global from
the `blooop` channel (`~/.pixi/bin/dl`). Leave them alone: they are what keeps
working while this checkout is mid-change, and what actually opens the user's
workspaces.

`dl-next` and `aid-next` are **this working tree**, installed by `./dev.sh` into
`~/.local/share/devlaunch-dev` and symlinked from `~/.local/bin`. Both builds are
on PATH at once under names that cannot collide, so running the wrong one is not
possible by accident.

```
./dev.sh
```

Two things to know about it:

- **The install is editable, so there is no build step and no snapshot.**
  `dl-next` is whatever the tree looks like at the moment you run it — a
  half-finished edit is live as soon as it is saved. (`wf-next` in
  blooop/wayfinder is the same idea with the opposite trade: a compiled copy that
  only moves when you rebuild it.) `dl-next --version` names the tree it resolves
  to — `dl 0.0.9 (dev, editable from /path/to/checkout)` — where the released
  `dl --version` prints the bare version, so the two are told apart by output as
  well as by name.
- **It touches real state.** `dl` mutates `metadata.json`, the bare clone cache
  and live devpod workspaces, which is how a half-finished change costs someone
  their workspace list. Everything it stores resolves through `XDG_CACHE_HOME`
  and `XDG_CONFIG_HOME`, so point those at a scratch directory when the change
  being tested is anywhere near storage:

  ```
  XDG_CACHE_HOME=/tmp/dl-scratch/cache XDG_CONFIG_HOME=/tmp/dl-scratch/config dl-next owner/repo
  ```

  That isolates the bookkeeping, not the machine — `dl` drives devpod and docker
  on the host either way, so the workspaces a scratch run creates are real ones
  that `devpod list` shows and that need deleting like any other.

## Documentation Maintenance

- **Keep README up to date**: When modifying CLI commands, flags, or usage patterns, update the README.md to reflect the current tool behavior. Run `pixi run dl --help` to see the current help output and ensure the README matches.
