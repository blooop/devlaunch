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
machine wrote or a command whose output is quoted. Section 6 then puts devpod's
own source beside each measurement, which is where the two caveats the running
binary cannot answer come from: the file is undocumented, and only the docker
driver was established.

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

**It is a tag, never an image id, and that is structural.** `ContainerDetails`
has exactly four members and none of them can hold one
(`pkg/devcontainer/config/container_details.go:16-34`): docker's top-level
`.Image`, the `sha256:…` digest, has nowhere to land. The container the record
above describes was running image id `sha256:a44bf1cf424e…` and the record kept
only the tag. So the reference has to be resolved through docker to become an id,
and between the write and the read the tag can have been moved.

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

At the point the inventory was taken, 13 `:devpod-`tagged images had come out of
these workspaces. Nine were named by a surviving `workspace_result.json`; four
were not, two superseded by a rebuild and two from creates that failed in their
lifecycle hooks. Plus the `devpod build` leftover, removed by hand before the
prebuild pull test. (A fourteenth tag on the same daemon, `proj-d3c2f:devpod-099e…`,
belonged to a parallel run reading devpod's source, not to these workspaces.)

That ratio is an artifact of a session that deliberately provoked every failure
mode, not a prediction about a real host. What it fixes is the shape: a read of
the records is a lower bound on the set, never the whole of it.

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
being run. `gh api repos/skevetter/devpod` confirms `fork: true`, parent
`loft-sh/devpod`; the fork's tags run `v0.8.0` to `v0.26.1`, which is where the
"0.8" this repository used to pin actually lived.

The source agrees with the measurements across both trees. In the fork,
`pkg/devcontainer/config/result.go` and `container_details.go` are identical at
`v0.8.0`, `v0.15.0` and `v0.26.1`, and the `LegacyImage` field carrying JSON tag `Image`
is byte-identical throughout. Going further back into `loft-sh`, the file appears
in the `v0.4.8` to `v0.5.0` window, together with the fourth `Result` member
`DevContainerConfigWithPath`, while the **JSON key `Image` under
`ContainerDetails.Config` has existed since `v0.1.0`** and has never been renamed.

| tag | `workspace_result.json` | `Result` members | image field |
| --- | --- | --- | --- |
| loft-sh v0.1.0 | absent | three, untagged | `Image string` |
| loft-sh v0.3.0 | absent | three | `Image`, now JSON tagged |
| loft-sh v0.5.0 | present | four, all tagged | renamed `LegacyImage`, tag still `Image` |
| loft-sh v0.6.15 | present | four | identical to v0.26.1 |
| fork v0.8.0 to v0.26.1 | present | four | identical |

One rename did happen, and it is in the *name* rather than the record: loft-sh
built `vsc-<basename>-<hash>:<prebuildHash>` (`pkg/driver/docker/build.go:246-248`
at v0.6.15) and the fork dropped the `vsc-` prefix, already gone by `v0.8.0`.
That is worth knowing only because it means the tag *shape* is fork-specific
while the *field* is not.

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

The refusal is not limited to running containers. Stopping the container first
and asking again gets the same answer:

```
$ docker stop 0a9d6ff2ffab
$ docker rmi twinb-3005e:devpod-0e1e79534ab8bce48e7918f1262ac427
Error response from daemon: conflict: unable to delete … (must be forced)
  - container 0a9d6ff2ffab is using its referenced image 2d05b4e9c6da
```

So an unforced `docker rmi` can only ever take an image that no container refers
to at all, which is the same set as "superseded by a rebuild, orphaned by a
failed create, or left by a deleted workspace". It says nothing about another tag
on the same id, and nothing about whether somebody wanted the image.

**And the cost of being wrong is a rebuild, not data.** An image is derived from
a Dockerfile and a base, both of which still exist; a volume holds the only copy
of whatever is in it. Removing an image that turns out to be wanted costs the
time to build or pull it again, which is the asymmetry that makes this a
different question from the volume sweep even though it reads the same file.

That was measured too, along with one wrinkle. The tag is a cache key: devpod
skips the build entirely when the tag already exists. A workspace over
`FROM dl447base:latest` was brought up, then the base tag was moved to a
different image and `up --recreate` run again, and devpod reused the existing tag
without rebuilding, same image id. Delete the tag and the same `up --recreate`
builds again under the same tag:

```
Untagged: base-2e2f3:devpod-788136251098c455a5ed06c67aca0fe3
Deleted:  sha256:0a8103a32e8d5bdfbf82b8db48bab35fc1bff5e5ec03885ccb98967735787fbf
…
info #6 exporting to image
info #6 naming to docker.io/library/base-2e2f3:devpod-788136251098c455a5ed06c67aca0fe3 done
new id: sha256:cc5c70a8e3a89e81b7189c1ae4479038f789279cb9d28271081957a6aba372c5
```

The wrinkle is the new id. The rebuild resolved `dl447base:latest` afresh and
picked up the image the tag had been moved to in between, so a removal is
recoverable but not necessarily byte-identical: the recipe is pinned by the hash,
what the recipe pulls is not. That is the same "the hash covers the recipe, not
what the recipe pulls" argument `CLAUDE.md` makes about the prebuild tag, seen
from the other end.

## 6. The code behind it

Every claim above is a measurement, and the source says the same. Citations are
against `github.com/skevetter/devpod` at `v0.26.1` (commit `86b6f9f5`) unless
they name `loft-sh`.

**The layout and the file names.** `pkg/provider/dir.go:17-24` declares
`WorkspaceConfigFile = "workspace.json"` and
`WorkspaceResultFile = "workspace_result.json"`; `GetWorkspaceDir`
(`pkg/provider/dir.go:112-123`) joins `contexts/<context>/workspaces/<id>` under
the config dir, which is `$DEVPOD_HOME` or `~/.devpod`
(`pkg/config/dir.go:11-26`, `pkg/config/env.go:17`). This is the layout
`clients::devpod_home` already spells, confirmed against its owner.

**The type.** `workspace_result.json` is `devcontainer/config.Result`
(`pkg/devcontainer/config/result.go:12-17`), whose four members are exactly the
four keys observed. The image lives in `ContainerDetailsConfig`
(`pkg/devcontainer/config/container_details.go:23-34`) as
the `LegacyImage` field carrying JSON tag `Image`, and it is populated because the
struct is the unmarshal target for `docker inspect`
(`pkg/docker/helper.go:324-334`). The Go field carries a comment saying it
"shouldn't get used anymore and is only there for testing", which is worth
noting and is not a reason to distrust the value: it is whatever devpod passed to
`docker run`, and the JSON key has been in place since loft-sh `v0.1.0`.

**Written once, only on success.** The single write is `cmd/up.go:452`,
`provider2.SaveWorkspaceResult(client.WorkspaceConfig(), result)`, at the tail of
`devPodUp` after every failing branch has already returned. That is the code
behind "a create that died in its hooks has no result".

**Nothing in devpod reads the image back.** The readers of the file
(`cmd/ssh.go:680`, `pkg/client/clientimplementation/workspace_client.go:220`)
want the workdir and the last devcontainer.json. The only code that touches
`ContainerDetails.Config.Image` is devpod's own e2e suite
(`e2e/tests/up/dockerfile_build.go:86,103,132,150`), which uses it to check
whether a rebuild produced a different image, which is close to the use a
reclaim path would make of it.

**The name and the hash.** `GetImageName`
(`pkg/devcontainer/build/options.go:221-226`) is
`ToDockerImageName(basename) + "-" + hash(localWorkspaceFolder)[:5] + ":" + prebuildHash`,
which is the `<folder>-<5 hex>:devpod-<32 hex>` measured. `CalculatePrebuildHash`
(`pkg/devcontainer/config/prebuild.go:33-72`) hashes four things: architecture, a
normalized config, the post-feature Dockerfile content, and a `.dockerignore`
aware hash of the build context. The normalized config keeps
`DockerfileContainer{Dockerfile, Context, Build}`, and `Build` is
`*ConfigBuildOptions` including `Options []string` (`config.go:291`) which is why
adding `build.options` moved the tag. The prebuild lookup is the same string:
`prebuildImage := prebuildRepo + ":" + prebuildHash` (`pkg/devcontainer/build.go:341`),
and on a hit it becomes `BuildInfo.ImageName` and then `docker run`'s argument,
which is why a prebuild reference is recorded verbatim.

**Why the built image has no labels.** The metadata label is set on
`BuildOptions.Labels` (`pkg/devcontainer/build/options.go:118-121`), and the
buildx command builder (`pkg/driver/docker/build.go:95-105`) emits
`-f -t --build-arg --build-context --target --platform --cache-from --cache-to`,
the caller's `CliOpts`, and the context. It never emits `--label`, and never
passes `Labels`. Only the internal buildkit path applies them
(`pkg/devcontainer/buildkit/buildkit.go:116-119`). So on the default path the
label is silently dropped, which is exactly the `Config.Labels: null` measured,
and it is inherited from upstream rather than a fork regression: loft-sh
`v0.6.15`'s `buildxBuild` emits no `--label` either.

**Why `build.options` is the only route.** `ConfigBuildOptions.Options`
(`config.go:290-291`) becomes `buildOptions.CliOpts`
(`pkg/devcontainer/build/options.go:107`) and is appended raw to the buildx
command line (`pkg/driver/docker/build.go:102`), so `["--label", "k=v"]` reaches
docker. `customizations.devpod` has exactly two fields, `prebuildRepository` and
`featureDownloadHTTPHeaders` (`config.go:346-349`), so there is no label there.
And `--extra-devcontainer-path` cannot reach a build for two independent reasons:
`AddConfigToImageMetadata` (`pkg/devcontainer/config/metadata.go:18-30`) copies
only the base, actions and non-compose parts into an `ImageMetadata` that has no
`DockerfileContainer` and no `ImageContainer` at all, and its call site
(`pkg/devcontainer/build.go:46-55`) runs *after* the build has returned. Both
halves match the measurement.

**Why a delete keeps the image.** `pkg/devcontainer/delete.go:11-40` stops and
deletes the container; `DeleteDevContainer`
(`pkg/driver/docker/docker.go:174-188`) calls `Remove`, which is plain
`docker rm <id>` with no `-v` (`pkg/docker/helper.go:150-157`); then
`DeleteWorkspaceFolder`
(`pkg/client/clientimplementation/workspace_client.go:845-868`) removes the ssh
config entry and `os.RemoveAll`s the record directory. A grep of `pkg/`, `cmd/`
and `providers/` for `rmi`, `image rm` or image pruning finds nothing. devpod
never removes an image, anywhere.

**Also on disk, outside the directory this ticket asked about.** There is a
second, agent-side `workspace.json` at
`$DEVPOD_HOME/agent/contexts/<context>/workspaces/<id>/workspace.json`
(`provider.AgentWorkspaceInfo`, `pkg/agent/workspace.go:209`), and a third copy
of the `Result` *inside* the container at `/var/run/devpod/result.json`
(`pkg/config/paths.go:17`, written by `pkg/devcontainer/setup/setup.go:97-120`).
The agent-side file carries no image reference. The in-container copy is not
reachable from the host without entering the container.

### Two caveats the source raises and the measurements cannot settle

**The file is undocumented.** `workspace_result.json` appears nowhere in devpod's
own `docs/` tree; `prebuildRepository` appears once, as a devcontainer.json
snippet. So there is no published contract for this file's shape, only the code.
It has in fact been stable since loft-sh `v0.5.0`, which is a track record rather
than a promise. `clients::devpod_home` is already the place that absorbs that
risk for the volume names, and it carries the argument in its module doc.

**Only the docker driver was established.** Every claim here is the docker
driver. The kubernetes driver builds its `ContainerDetails` by hand
(`pkg/driver/kubernetes/find.go:49-58`) and the custom driver unmarshals whatever
a provider prints (`pkg/driver/custom/custom.go:67`); whether either populates
`Config.Image` was not traced. It is moot for `dl` as it stands, which drives the
docker provider, and it would stop being moot if that ever changed.

## Answers, in one place

1. **Does devpod record the image?** Yes.
   `workspace_result.json` → `.ContainerDetails.Config.Image`, the same file the
   volume sweep already parses. Uniform across build, image and prebuild paths.
   `workspace.json` carries nothing. It is the reference devpod passed to
   `docker run`, so a tag rather than an image id, and devpod's own non-test code
   never reads it back.
2. **Is it there after a failed create?** No. A create that dies in its lifecycle
   hooks leaves `workspace.json` alone, and the image it built stands. Same on
   0.8.11. The single write is at the tail of `devPodUp` after every failing
   branch has returned (`cmd/up.go:452`).
3. **Is it stable across versions?** Yes, unchanged across 0.8.11, 0.16.6 and
   0.26.1, which spans everything `dl` has ever pinned, and the JSON key goes
   back to loft-sh `v0.1.0`. With one caveat the measurements cannot supply: the
   file is undocumented, so that is a track record and not a promise.
4. **Can `dl` cause a label at build time?** Not through `devpod up`, which
   exposes no build options, and not through `--extra-devcontainer-path`, whose
   merge drops `build.options`. Only through the repository's own
   `devcontainer.json` `build.options`, which does work and which changes the
   32-hex tag, so it invalidates the `prebuildRepository` lookup until the
   prebuild is republished, and permanently if the label's value varies.
5. **What does sharing do?** One reference can be two workspaces' live image; a
   rebuild can leave one workspace pointing at a tag another still runs;
   identical content in two folders yields two ids sharing their layers; and
   deleting a wanted image costs a rebuild or a pull, never data. An unforced
   `docker rmi` refuses while any container refers to the image, running or
   stopped, and the tag doubles as devpod's build cache key, so a deleted image
   is rebuilt on the next `up` from whatever its base resolves to then.

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
