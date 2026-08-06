# Working with demanding devcontainer projects

Most projects need nothing from this page: `dl owner/repo` clones, builds and
attaches. It covers the two places where devlaunch has to hand a project more
than the devcontainer spec provides, and what a project has to do to meet it.

## Workspace identity for host-side hooks

`devcontainer.json`'s `initializeCommand` runs **on the host**, before the
container exists. Projects use it to prepare bind-mount targets and to generate
the `.env` that `docker compose` interpolates.

A project that supports several checkouts at once has to name each one — the
container name, the compose project name and any per-checkout image tag all have
to differ, or two branches fight over one container. Such projects normally
derive that name from the checkout's path. That doesn't work here: devlaunch
clones every branch to `<repo>/<branch>`, so sibling branches share every path
component that a heuristic would key on.

devpod passes `initializeCommand` no workspace identity of its own, so devlaunch
injects one with devpod's `--init-env`, which reaches the hook:

| Variable | Value |
|----------|-------|
| `DEVLAUNCH_WORKSPACE_ID` | The workspace id, e.g. `myrepo-nb4` |

A project's hook should prefer it over any path-derived guess, and fall back to
the guess when it is unset (a plain `git clone` opened in VS Code):

```bash
INSTANCE_ID="${DEVLAUNCH_WORKSPACE_ID:-$(basename "$(dirname "$PWD")")}"
```

## Choosing between several devcontainer.json files

Repos that build for more than one target — a second architecture, or a GPU/
simulator mode on a different compose file — keep several variants. The spec
discovers them at `.devcontainer/<name>/devcontainer.json`, one level deep, so a
bare name is enough:

```bash
dl org/repo --devcontainer sim     # -> devpod --devcontainer-id sim
dl org/repo --devcontainer ./somewhere-else.json   # -> devpod --devcontainer-path
```

A bare name becomes a devpod `--devcontainer-id`, so devpod resolves the
`.devcontainer/<name>/` location itself rather than devlaunch hand-building the
path. Anything containing `/` or ending in `.json` is passed as a path instead.

**This is a one-time argument.** devpod stores the chosen config with the
workspace, so later `dl org/repo` calls reuse it — including after a `stop`. Only
switching an existing workspace to a *different* variant needs a rebuild:

```bash
dl org/repo recreate --devcontainer robot
```

Note that the spec gives exactly one zero-argument config,
`.devcontainer/devcontainer.json` — every other location needs an explicit path.
A project whose primary workflow is one particular variant should put that
variant in the root file, not in a subfolder.

Workspace ids do **not** encode the variant, so one branch holds one variant at
a time. Use a second branch-workspace if you want both at once.

### Moving a devcontainer.json strands existing workspaces

devpod re-parses a workspace's `devcontainer.json` when *deleting* it, to tear the
container down. So if a project moves or renames the config an existing workspace
points at, `dl <ws> rm` fails — the file it recorded is gone. Recover with:

```bash
devpod delete <ws> --force          # drops devpod's record
docker rm -f <container>            # then clean up by hand
```

`dl` keeps the local clone whenever devpod's delete fails, so the workspace stays
retryable: restore the old path and the normal delete works again. Projects
restructuring their variants should expect existing workspaces to need this.

## Compose projects: what devpod owns

devpod names the compose project itself, from the workspace id — it does not
honour `COMPOSE_PROJECT_NAME` from the project's `.env`. Two consequences for
compose-based devcontainers:

- **Every service the devcontainer needs must be in `runServices`.** A sidecar
  started later by the project's own tooling (which computes its own project
  name) lands in a *different* compose project, so it gets a different default
  network and container-to-container discovery silently fails.
- Without `runServices` a spec-conforming tool starts **all** services. Compose
  files that carry an abstract template service — one that exists only to be
  `extends:`-ed, and is visible because another file `include:`s it — bring that
  template up as a real container. List the services you actually want.

## Git LFS

Workspaces are cloned from a local bare cache that holds no LFS objects, so
devlaunch clones with `GIT_LFS_SKIP_SMUDGE=1` and then runs `git lfs pull`
against the real remote once the origin URL is set. Nothing is required of the
project, but a repo whose LFS objects the remote no longer has will fail there
rather than silently leaving pointer files in the tree.
