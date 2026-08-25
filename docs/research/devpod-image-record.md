# Does devpod record the image it built for a workspace

Research for [#447](https://github.com/blooop/devlaunch/issues/447) on the
[reclaim map](https://github.com/blooop/devlaunch/issues/444). This is knowledge
only. What `dl` should *do* about it is the blocked sibling,
[#450](https://github.com/blooop/devlaunch/issues/450).

**Short answer: yes.** devpod records, per workspace, the exact image reference
the container was created from, in `workspace_result.json` at
`.ContainerDetails.Config.Image`. It is the same file and the same walk the
volume sweep already reads, it holds the reference whether devpod built the
image, pulled a named one, or pulled a prebuild, and it is byte-identical in
shape across devpod 0.8.11, 0.16.6 and 0.26.1. Four things the record does *not*
cover are measured below.

## How this was established

Measured 2026-08-25 from inside this repository's own devcontainer: nested
docker-in-docker, docker 29.7.2-2, devpod v0.26.1 from the project env
(`pixi run devpod`), plus devpod 0.8.11 and 0.16.6 run out of `pixi exec` against
the same daemon. Every run had `DEVPOD_HOME` scoped to a scratch directory
(`dph`, `dph8`, `dph16`) and used the `docker` provider added into that scratch
home, so nothing below touched the real workspace list.

Thirteen throwaway workspaces over one-line devcontainers, covering the image
path (`{"image": "alpine:3.20"}`), the build path
(`{"build": {"dockerfile": "Dockerfile"}}` over a two-line Dockerfile), a create
that dies in `postCreateCommand`, a rebuild after a content change, two
workspaces over one folder, two folders with identical content, a build with
`build.options`, an `--extra-devcontainer-path` override, and a prebuild
published to a local `registry:2` and pulled back.

Nothing here is inferred from devpod's documentation. Every claim is a file this
machine wrote or a command whose output is quoted.

## 1. The record exists, and it is one field

`~/.devpod/contexts/<context>/workspaces/<id>/workspace_result.json` has exactly
four top-level keys, on all three devpod versions tested:

```
["ContainerDetails", "DevContainerConfigWithPath", "MergedConfig", "SubstitutionContext"]
```

`ContainerDetails` is devpod's snapshot of the container it created, and it
carries the image reference:

```json
{
  "ID": "e1a4f3fb0a97faabea06ff9f2d1799c01db500a63268d6db92ee603e9125918e",
  "Created": "2026-08-25T13:37:56.347742114Z",
  "State": { "Status": "running", "StartedAt": "2026-08-25T13:37:56.419257849Z" },
  "Config": {
    "Labels": {
      "dev.containers.id": "default-bl-53de0",
      "devcontainer.metadata": "[{}]",
      "devpod.user": ""
    },
    "WorkingDir": "/",
    "Image": "bld-c745b:devpod-9f56be24be1c0b2903995ed4cbfe8a9e"
  }
}
```

That `Image` is the whole answer to the ticket's question. It is a reference
devpod chose, not one anybody guessed, which is the property
[devlaunch#325](https://github.com/blooop/devlaunch/issues/325) established for
volume names and `docs/cleanup.md` states as the rule.

**This is the file `dl` already reads.** `flows::lifecycle::devcontainer_volumes`
opens the same `workspace_result.json`, through the same
`clients::devpod_home::sole_workspace_result` contexts walk, to get
`SubstitutionContext.LocalWorkspaceFolder` and `SubstitutionContext.DevContainerID`.
Reading one more key off a document already parsed is not an architecture change.

`workspace.json`, the record devpod writes on the way *in*, carries no image
reference at all. Its keys, in full: `id`, `uid`, `provider.name`,
`provider.options.*`, `machine`, `ide.name`, `source.localFolder`,
`creationTimestamp`, `lastUsed`, `context`.

### The reference is a reference, and its form depends on where the image came from

| devcontainer | `ContainerDetails.Config.Image` |
| --- | --- |
| `{"image": "alpine:3.20"}` | `alpine:3.20` |
| `{"build": {"dockerfile": "Dockerfile"}}` | `bld-c745b:devpod-9f56be24be1c0b2903995ed4cbfe8a9e` |
| same, plus `customizations.devpod.prebuildRepository: "localhost:5000/dl447pre"` | `localhost:5000/dl447pre:devpod-a647918106520274321fd35ad936b5a4` |

devpod's own log lines for those three runs, in order:

```
info inspecting image: image=alpine:3.20
info inspecting image: image=bld-c745b:devpod-9f56be24be1c0b2903995ed4cbfe8a9e
info inspecting image: image=localhost:5000/dl447pre:devpod-a647918106520274321fd35ad936b5a4
info image not found, pulling image: image=localhost:5000/dl447pre:devpod-a647918106520274321fd35ad936b5a4
```

A built image is named `<workspace folder>-<5 hex>:devpod-<32 hex>`. A prebuild
hit is named `<prebuildRepository>:devpod-<32 hex>`, with the **same** 32-hex
tag: the `devpod build` that published it named it
`localhost:5000/dl447pre:devpod-a647918106520274321fd35ad936b5a4`, and the later
`devpod up`, after both local tags were deleted, looked for exactly that
reference and pulled it. So the prebuild tag and the local build tag are one hash.

### The record also says whether devpod made the image or pulled somebody else's

Which matters, because `alpine:3.20` in the table above is not devlaunch's image
to touch, and the same field holds it. The other keys in the same document
separate the cases without any pattern matching on the tag:

| workspace | `ContainerDetails.Config.Image` | `MergedConfig.image` | `config.build` |
| --- | --- | --- | --- |
| `imgws` | `alpine:3.20` | `alpine:3.20` | absent |
| `bldws` | `bld-c745b:devpod-d417…` | `null` | `{"dockerfile": "Dockerfile"}` |
| `prews` | `localhost:5000/dl447pre:devpod-a647…` | `null` | `{"dockerfile": "Dockerfile"}` |

A non-null `MergedConfig.image` means devpod pulled a reference the devcontainer
named. A null one with a `build` beside it means the reference in
`ContainerDetails.Config.Image` is one devpod derived. The prebuild case reads
as a build, and its image came out of a registry rather than a local build, so
"derived by devpod" and "built on this machine" are not the same question and the
record answers only the first.

## 2. Where the record is not

Four gaps, all measured. They are stated as facts, not as arguments for or
against anything.

### A create that failed partway has no record of its image, and the image exists

A devcontainer whose `postCreateCommand` is `exit 3`:

```
info running postCreateCommands lifecycle hook:  exit 3
fatal run agent command failed: ... lifecycle hooks pre-attach: failed to run: exit 3
```

leaves the workspace directory holding `workspace.json` and nothing else, while
the image it built is on the daemon:

```
$ ls dph/contexts/default/workspaces/failws/
workspace.json
$ docker images
fail-4e13f:devpod-9fddd6ebf9371067a664778e93522ba6   217fb471ee00
```

Same on 0.8.11 (`old8fws`: `workspace.json` only, `old8f-ca214:devpod-301e71…`
on the daemon). This is the identical shape
`clients::devpod_home::CreateRecord::NeverCompleted` already names for volumes,
and the same reason: devpod writes the result on its way out of a *successful*
up.

### `devpod delete` takes the record and leaves the image

```
$ devpod delete bldws2 --force
info deleting workspace container
info deleting devcontainer: devcontainerID=77ae000f…
done deleted workspace bldws2
```

Afterwards the `bldws2` directory is gone from `workspaces/`, the container is
gone from `docker ps -a`, and `bld-c745b:devpod-9f56be24be1c0b2903995ed4cbfe8a9e`
is still listed by `docker images`. So the only record naming an image is
destroyed at the exact moment the image stops being needed. Anything that wants
to act on the reference has to read it *before* the delete, which is the ordering
`flows::lifecycle` already uses for volumes.

### A rebuild leaves the old image with nothing pointing at it

Change one line of the Dockerfile and `devpod up --recreate` the same workspace:

```
before:  bld-c745b:devpod-9f56be24be1c0b2903995ed4cbfe8a9e
after:   bld-c745b:devpod-d41724759ef37417d3b043e7f46cce9e
```

Both are on the daemon afterwards. The workspace's record now names only the
second. The first is not dangling in docker's sense, it keeps its tag, so
`docker image prune` without `-a` does not touch it. This is the accumulation
mechanism behind the 86.5 GB figure in `docs/cleanup.md`: one workspace rebuilt
*n* times leaves *n* tagged images and one record.

### `devpod build` leaves an image behind no workspace ever records

`pixi run devcontainer-prebuild` is `devpod build .`, and that command creates a
temporary workspace, builds, pushes, and deletes the workspace again:

```
done done building and pushing image localhost:5000/dl447pre:devpod-a647918106520274321fd35ad936b5a4
info cleaning up temporary workspace
info deleting workspace container
```

Two local tags exist afterwards, `pre-3a6f7:devpod-a647…` and
`localhost:5000/dl447pre:devpod-a647…`, both resolving to image id
`9ff11b7d4a48`, and `workspaces/` holds no entry for it. So a prebuild publish
leaves a local image with no record naming it, on whatever machine ran the
publish.

### Measured coverage on the scratch daemon

Of the 14 `:devpod-`tagged images those thirteen workspaces produced, 10 were
named by a surviving `workspace_result.json` and 4 were not: two superseded by a
rebuild, two from creates that failed in their lifecycle hooks. Plus the
`devpod build` leftover, which I removed by hand before the prebuild pull test.
That ratio is an artifact of a test session that deliberately provoked every
failure mode, not a prediction about a real host. What it fixes is the shape: a
read of the records is a lower bound on the set, never the whole of it.

## 3. Attaching a label at build time

`dl` passes no build options at all today. `flows::launch` builds `devpod up`
with `--id --ide --init-env --mount --workspace-env --dotfiles
--dotfiles-script --devcontainer-path` and `--recreate`/`--reset`, and
`clients::docker`'s entire surface is `remove_volumes`.

**`devpod up` in 0.26.1 exposes no way to label or name an image.** Its full flag
list holds nothing about image naming or build arguments. The image-adjacent
flags are `--devcontainer-image` (replace the devcontainer's image outright,
which skips the build), `--fallback-image` (used when no devcontainer config is
detected) and `--prebuild-repository` (where to look for a prebuild). There is
no `--label`, no `--build-arg`, no `--tag`, no `--repository`.

`devpod build` has `--repository`, `--tag`, `--platform`, `--push` and
`--skip-push`, so it can name the image it produces. It is a separate command
that creates and deletes its own temporary workspace, and no `devpod up` flag
consumes a locally built tag other than `--devcontainer-image`, which bypasses
building.

### The one route that works is the repository's own devcontainer.json, and it moves the prebuild tag

`build.options` does reach the docker build. A devcontainer of

```json
{ "name": "lblws",
  "build": { "dockerfile": "Dockerfile",
             "options": ["--label", "devlaunch.workspace=lblws"] } }
```

produces an image carrying it:

```
$ docker image inspect lbl-eb7ca:devpod-605e9efe03c2a2a91f3c0269c1479ffa \
    --format '{{json .Config.Labels}}'
{"devlaunch.workspace":"lblws"}
```

Without `build.options`, every image devpod builds carries **no labels at all**:
`Config.Labels` is `null`. The three labels devpod does write,
`dev.containers.id`, `devcontainer.metadata` and `devpod.user`, go on the
`docker run` command line and live on the container only. `docs/cleanup.md`'s
claim that "images devpod builds carry no devlaunch or devpod label" is exactly
right as measured.

**And `build.options` is an input to the tag hash.** Removing the two options
above from that same folder and re-running `devpod up --recreate`:

```
with    --label: lbl-eb7ca:devpod-605e9efe03c2a2a91f3c0269c1479ffa
without --label: lbl-eb7ca:devpod-fd10a6821b8ef42e6dd52dda49e0fdb2
```

So a label added under `build.options` changes the 32-hex tag devpod computes,
and the prebuild lookup is for that exact tag (section 1). Adding one to this
repository's `.devcontainer/devcontainer.json` would therefore miss
`ghcr.io/blooop/devlaunch-devcontainer` until the prebuild workflow republished
under the new hash, and would miss it again on every change to the label's
value. A label whose value varies per workspace or per launch can never match a
published prebuild at all.

### `--extra-devcontainer-path` does not carry `build.options`

The one devpod flag that looks like it could inject a label per launch without
editing the repository's file merges runtime configuration and drops the build
half. An override file of

```json
{ "containerEnv": { "DL_OVERRIDE": "yes" },
  "build": { "options": ["--label", "devlaunch.workspace=bld10ws"] } }
```

passed as `--extra-devcontainer-path` on a **first** `up` of a fresh folder, so
no cached image could mask the result:

```
MergedConfig.containerEnv.DL_OVERRIDE = "yes"      <- the override did apply
docker image inspect … Config.Labels  = null       <- the label did not
```

The `containerEnv` proves the flag took effect. The absent label and the
unchanged tag prove `build.options` is not part of what it merges.

## 4. Version stability

The same workspace, built the same way, under the three devpod versions
reachable from the pinned channel:

| devpod | result file | `ContainerDetails.Config.Image` |
| --- | --- | --- |
| 0.8.11 | present | `old8-eab5a:devpod-324a21639345ab653bf2cf7c7c2f728f` |
| 0.16.6 | present | `mid16-a7ac1:devpod-2cb21e9ed2294d4b18499f25bc5c92b6` |
| 0.26.1 | present | `bld-c745b:devpod-9f56be24be1c0b2903995ed4cbfe8a9e` |

Identical four top-level keys, identical `ContainerDetails.Config` shape,
identical `<folder>-<5 hex>:devpod-<32 hex>` naming, and 0.8.11 also writes no
result for a create that failed in `postCreateCommand`. 0.8.11 is below this
repository's floor, which `pyproject.toml` and `conda.recipe/recipe.yaml` both
put at `devpod >=0.26.1`, so the field predates every version `dl` claims to
support and has not moved across the whole range it ever supported.

One thing worth writing down about primary sources here: the devpod `dl` runs is
**not** built from `loft-sh/devpod`. That repository's newest release is v0.6.15
(tag `v0.7.0-alpha.34`, last push 2025-11-14), so v0.26.1 does not exist there.
`strings` over the pinned binary shows its own module path is
`github.com/skevetter/devpod`, alongside `github.com/skevetter/api` and
`github.com/skevetter/agentapi`. Anyone reading devpod's source for this question
needs that fork, and the `loft-sh` tree is the ancestor rather than the thing
being run.

## 5. What sharing does to the answer

Three sharing cases, and they are not the same case.

**Two workspaces over one folder share one reference.** `bldws` and `bldws2`,
both `devpod up` over the same directory, recorded the identical image:

```
bldws  -> bld-c745b:devpod-9f56be24be1c0b2903995ed4cbfe8a9e
bldws2 -> bld-c745b:devpod-9f56be24be1c0b2903995ed4cbfe8a9e
```

So a reference read out of one record can be the live image of a workspace whose
record was not read. Worse for a naive read: after `bldws` was rebuilt onto a new
tag, `bldws2`'s record still named the old one, and its container was still
running on it. The old tag was simultaneously "superseded" from one workspace's
view and "in use" from another's.

**Two folders with identical content do not share a reference.** `twinA` and
`twinB`, byte-identical Dockerfiles, produced two tags and two image ids:

```
twina-8567d:devpod-ce81845c2c3d9a0a8aa80b3d9cc74233  sha256:5826e7d32fb3…
twinb-3005e:devpod-0e1e79534ab8bce48e7918f1262ac427  sha256:2d05b4e9c6da…
```

Their `Config` documents are identical and their `RootFS.Layers` are the same two
digests, so the *layers* are shared on disk even though the image ids differ.
Two image ids can therefore share almost all of their bytes, and a per-image byte
count is not additive.

**An image can be shared with something that is not a workspace.** A base image
like `alpine:3.20` is what a record names for the image path, and it may be the
base of anything else on the machine. Docker itself refuses the unforced case:

```
$ docker rmi bld-c745b:devpod-d41724759ef37417d3b043e7f46cce9e
Error response from daemon: conflict: unable to delete … (must be forced)
  - container 7469308e6b6a is using its referenced image 144b1b97cc30
```

That refusal is a live-container check only. It says nothing about a stopped
workspace that would need the image again, or about another tag on the same id.

**And the cost of being wrong is a rebuild, not data.** An image is derived from
a Dockerfile and a base, both of which still exist; a volume holds the only copy
of whatever is in it. Removing an image that turns out to be wanted costs the
time to build or pull it again, which is the asymmetry that makes this a
different question from the volume sweep even though it reads the same file.

## Answers, in one place

1. **Does devpod record the image?** Yes.
   `workspace_result.json` → `.ContainerDetails.Config.Image`, the same file the
   volume sweep already parses. Uniform across build, image and prebuild paths.
   `workspace.json` carries nothing.
2. **Is it there after a failed create?** No. A create that dies in its lifecycle
   hooks leaves `workspace.json` alone, and the image it built stands. Same on
   0.8.11.
3. **Is it stable across versions?** Yes, unchanged across 0.8.11, 0.16.6 and
   0.26.1, which spans everything `dl` has ever pinned.
4. **Can `dl` cause a label at build time?** Not through `devpod up`, which
   exposes no build options, and not through `--extra-devcontainer-path`, whose
   merge drops `build.options`. Only through the repository's own
   `devcontainer.json` `build.options`, which does work and which changes the
   32-hex tag, so it invalidates the `prebuildRepository` lookup until the
   prebuild is republished, and permanently if the label's value varies.
5. **What does sharing do?** One reference can be two workspaces' live image; a
   rebuild can leave one workspace pointing at a tag another still runs;
   identical content in two folders yields two ids sharing their layers; and
   deleting a wanted image costs a rebuild or a pull, never data.

Two further gaps beyond the failed create, for completeness: `devpod delete`
destroys the record and keeps the image, so the reference has to be read before
the delete; and `devpod build`, which is what `pixi run devcontainer-prebuild`
runs, leaves a local image that no workspace record ever named.

## Reproducing this

Everything above came from `pixi run devpod` with `DEVPOD_HOME` pointed at a
scratch directory, a `docker` provider added into that scratch home, and
`--ide none`. The one-line devcontainers are quoted where they matter. The older
devpods came from `pixi exec -c https://prefix.dev/blooop -c conda-forge --spec
"devpod==0.8.11" -- devpod …`; that channel carries 58 devpod versions from
0.8.11 up. The prebuild test used a local `docker run -d -p 5000:5000 registry:2`
and `customizations.devpod.prebuildRepository: "localhost:5000/dl447pre"`, with
`devpod build` to publish and `devpod up` to pull it back after both local tags
were deleted.
