# Making a repository dl-ready

This page is for an agent that converts a repository so `dl owner/repo@branch` opens it
quickly and cleanly. It is a checklist, learned from a sweep across the owner's repositories
in October 2026. Each tip names the repository that taught it.

## The target shape

- `.devcontainer/devcontainer.json` with
  `"image": "ghcr.io/blooop/python_template/devcontainer:latest"`. That image is Ubuntu 26.04
  with pixi, git-lfs, the claude-code feature and the emSign G3 CA fix baked in. Copy
  python_template's `.devcontainer/` (not its `ci/`), and keep `"name"` as the last key, so
  `update-from-template-repo` merges do not conflict on it.
- Everything else comes from pixi. Python, uv, node and Rust tools go in the pixi manifest,
  not in a Dockerfile.
- Only when something cannot come from conda-forge: `"build"` with
  `FROM ghcr.io/blooop/python_template/devcontainer:latest` plus one extra layer.

## Tips

- **No `features` block on an `"image"` config.** Any feature makes the devcontainer CLI
  build a derived image on the first launch of every branch. The base already has
  common-utils and git (dbw, planthood).
- **Do not use Microsoft's `base:ubuntu-24.04` or `python:0-3.10`.** The 24.04 tag stopped
  being rebuilt in October 2025 and ships a CA bundle that still trusts emSign G3.
  `python:0-3.10` no longer builds: `apt install git-lfs` exits 100 (leetcode_rerun,
  rerun_examples). A repository that really needs its own base uses `base:3-ubuntu26.04`.
- **Post-create must not rewrite the lock.** A plain `pixi install` or `pixi run <task>`
  with a newer pixi rewrites `pixi.lock` (pixi 0.83 turns a v6 lock into v7), so every
  fresh workspace is dirty and `dl rm` refuses it. Use `pixi install --frozen` and
  `pixi run --frozen <task>` in post-create (python_template#177). `--locked` fails
  container creation on an older lock format.
- **Commit the lock in the current format.** Once the lock is v7, pixi 0.83 has nothing to
  rewrite, so a `pixi run test` typed by a person does not dirty the clone either. Fix
  pixi's manifest warnings in the same change.
- **The `.pixi` volume is root-owned on create.** Post-create runs `sudo chown vscode .pixi`
  before `pixi install` (python_template's `post-create.sh`).
- **Seed `known_hosts` in post-create**, not by mounting `~/.ssh`. devpod forwards the ssh
  agent.
- **Keep `--network=host` only with a reason**: mDNS for homeassistant-config, ROS 2 DDS for
  colcon-runner. Everything else uses the bridge network, so two branches of one repository
  do not fight over ports.
- **Environment variables go in `containerEnv`**, not in an `env` key (which is not valid)
  or a Dockerfile `ENV` (deps_rocker, rockerc).
- **Docker-driving repositories keep docker-in-docker or docker-outside-of-docker** as a
  feature and accept the derived build (rockerc, worktree_docker, deps_rocker). On the 26.04
  image docker-in-docker needs major 4: `docker-in-docker:2` has no Moby packages for
  Ubuntu 26.04 and fails in seconds (devcontainers/features#1738).
  `docker-outside-of-docker:1` already works.
- **Repositories with Git LFS files need git-lfs in the container.** Without it, `git status`
  fails with "clean filter 'lfs' failed" (ArjanCodesSoftwareDesignerMindset). The shared
  image has carried git-lfs since python_template#177, so a repository on it needs nothing.
  One on its own base adds an apt `git-lfs` layer. Find them with `git lfs ls-files`.
- **Old flit repositories keep flit as the build backend.** pixi needs only `python`,
  `<pkg> = { path = ".", editable = true }` under pypi-dependencies and
  `default = { features = ["test"] }`. Old pins can cap Python: taichi 1.7.0 has no cp312
  wheel, so ant_colony_bench uses 3.11.
- **Repositories with no pixi manifest** (devcontainer-features, devcontainer-templates) use
  `"build"` from the shared image plus the features they need, and skip the template's
  `post-create.sh`, which runs `pixi install`.

## Blockers to expect

- **Template bind mounts fail where the host lacks the source.** The template mounts
  `~/.config/gh` and `~/.claude`. A CI runner has neither, and a missing bind source stops
  the container. Create both directories in CI first (deps_rocker's `ci.yml`).
- **A host-dependent `initializeCommand` stops the workspace.** stata's `init-host.sh`
  exits 1 when `~/.config/stata` is absent. Make the script create or skip what it needs.
- **One empty devpod workspace record repeats devpod's warning on every dl call**, and
  `dl --ls` does not show that workspace (open: [#705](https://github.com/blooop/devlaunch/issues/705)).
  The warning names the workspace. Delete that record's folder under
  `~/.devpod/contexts/default/workspaces/`.

Fixed on dl's `main` after v0.63.2, so a conversion on a later dl no longer needs to work around them:

- A repository with no devcontainer got devpod's own `base:ubuntu`, because dl never passed
  `[worktree] fallback_image` on
  ([#706](https://github.com/blooop/devlaunch/issues/706), fixed in [#710](https://github.com/blooop/devlaunch/pull/710)). dl now passes it, and it
  defaults to the shared image.
- `dl rm` refused every such repository, because devpod writes its fallback config into the
  clone as `.devcontainer.json`
  ([#707](https://github.com/blooop/devlaunch/issues/707), fixed in [#711](https://github.com/blooop/devlaunch/pull/711)). The build files devpod writes under `.devcontainer/.devpod-internal/` for a config with features no longer count either.
- "the hostname setup stage exited 1" printed on every cold launch
  ([#704](https://github.com/blooop/devlaunch/issues/704), fixed in [#709](https://github.com/blooop/devlaunch/pull/709)). It was always harmless.

## Testing a conversion

- Test the pushed branch and `@main`, for a before and after:
  `dl blooop/<repo>@<branch> -- bash -lc '<check>'`. That form runs with no pty, which is
  expected and fakes some failures (the clipboard, for one).
- Pre-check a Python pin with a throwaway `docker run` of the shared image before the dl
  test.
- Delete both workspaces after: `dl blooop/<repo>@<branch> rm`.
- A cold create on the shared image, image already pulled, took 29s to 42s for a small pixi
  repository and 74s for colcon-runner. A much longer create means something is building.
