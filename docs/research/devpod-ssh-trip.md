# What a devpod ssh trip costs, and whether it can be reused

Research for [#389](https://github.com/blooop/devlaunch/issues/389) on the
[launch latency map](https://github.com/blooop/devlaunch/issues/139). This is
knowledge only. What `dl` should *do* about any of it is the blocked sibling,
[#390](https://github.com/blooop/devlaunch/issues/390).

## How this was measured, and what does not transfer

Measured 2026-08-24 from inside this repository's own devcontainer: nested
docker-in-docker, 8 CPUs, docker 29.7.2, OpenSSH_9.6p1, devpod v0.26.1. A
throwaway workspace (`res389`) over a one-line devcontainer
(`{"image":"mcr.microsoft.com/devcontainers/base:ubuntu"}`), with `DEVPOD_HOME`
and `DEVPOD_SSH_CONFIG` scoped to `/tmp/res389`.

Two caveats, and neither is small.

Nested DinD is not the host, so absolute seconds run high. What transfers is the
ratios and the decomposition, because everything below is A/B'd against
everything else in one environment.

**The machine got busier partway through.** Three other agents were running
docker and devpod experiments in the same container. Early, quiet-machine
readings and late, loaded readings differ by 20% to 60% on the same command:
`docker exec <cid> true` reads 65ms to 74ms quiet and 129ms to 150ms loaded, and
a multiplexed trip reads 9ms to 12ms quiet and 17ms to 31ms loaded. Both are
given where it matters. No conclusion below rests on a difference smaller than
that spread.

## 1. Where the 1.8s goes

### All of it is in one gap, and the gap is before ssh does any work

`ssh -vvv` through devpod's generated config, with every line timestamped on
arrival:

```
  0.000  OpenSSH_9.6p1 Ubuntu-3ubuntu13.14, OpenSSL 3.0.13
  0.000  debug1: Reading configuration data /tmp/res389/ssh_config
  0.001  debug1: Executing proxy command: exec "…/devpod" ssh --stdio --context default --user vscode res389 …
  0.003  debug1: Local version string SSH-2.0-OpenSSH_9.6p1
  1.662  debug1: Remote protocol version 2.0, remote software version Go     <-- 1.662s gap
  …      KEX, host key, auth, channel open, exec, exit-status
  1.673  debug1: Exit status 0
```

One gap of **1.662s**, from OpenSSH writing its version string into the
ProxyCommand's stdin to reading the peer's banner back. Everything OpenSSH is
normally blamed for, the full key exchange, host key check, userauth, channel
open, `exec` request, and exit-status delivery, is the **11ms** after it.

So the cost is not ssh. It is `devpod ssh --stdio` getting something that speaks
ssh to exist inside the container. Note `remote software version Go`: there is no
`sshd` in the container. devpod runs its own Go ssh server, started per
connection as `devpod helper ssh-server --stdio`.

### Inside the ProxyCommand

Same trip with `devpod ssh --stdio --debug`, timestamps relative to the first
line:

| at | devpod's own log line | block |
|---|---|---|
| 0.000 | `acquire workspace lock` / `acquired workspace lock` | CLI init + lock, ~100ms |
| 0.100 | `connected to host`, `Run container tunnel` | |
| 0.102 | `execute SSH server command: … agent container-tunnel --workspace-info <base64>` | workspace info, ~90ms |
| 0.147 | `writing workspace info to file`, `using docker command: command=docker` | |
| 0.191 | `starting agent injection` / `execute inject script` | **agent injection, 531ms** |
| 0.722 | `received line after pong: line=done` / `done injecting` | |
| 0.725 | `done exec` | **agent version check, 531ms** |
| 1.256 | `remote agent version matches expected version` / `detected remote agent version: v0.26.1` | |
| 1.522 | `connected to container` | container connect, 266ms |
| 1.523 | `execute SSH server command: … su -c 'devpod helper ssh-server --stdio …' vscode` | the banner OpenSSH was waiting for |

The two 531ms blocks are **1.06s of the 1.66s**, and both are the same thing:
asking the in-container agent binary its own version.

Source, devpod v0.26.1 (`github.com/skevetter/devpod`):

- `pkg/agent/inject.go`, `versionChecker.buildExistsCheck` templates the
  injection script's guard as
  `! { [ -x "<path>" ] && [ "$("<path>" version 2>/dev/null)" = "<version>" ]; }`.
  That runs `devpod version` inside the container once.
- `pkg/agent/inject.go`, `versionChecker.detectRemoteAgentVersion` then runs
  `<path> version` again, as its own exec, to log the version it found.
- `pkg/inject/inject.sh` wraps both in a `ping`/`pong` handshake over the exec's
  stdio before `execute_command` runs the real tunnel command.

Measured directly, in the same container:

| command | quiet | loaded |
|---|---|---|
| `docker exec <cid> true` | 65ms to 74ms | 129ms to 150ms |
| `docker exec <cid> bash -lc true` | 125ms to 140ms | 131ms to 198ms |
| `docker exec <cid> /usr/local/bin/devpod version` | 479ms to 501ms | 592ms to 614ms |

`74 + 490` is 564ms against an observed 531ms, twice. The arithmetic closes.

### And most of *that* is telemetry

`devpod version` prints one string (`cmd/version.go` is 30 lines and calls
`version.GetVersion()`). The time is in the root command's
`PersistentPreRunE`, which calls `telemetry.StartCLI`, and in `Execute`, which
calls `telemetry.CollectorCLI.Flush()` (`cmd/root.go`). `devpod --help`
short-circuits before both and costs 47ms to 55ms.

A/B, same binary, same machine:

| | quiet | loaded |
|---|---|---|
| host `devpod version` | 398ms to 441ms | |
| host `devpod version`, context `TELEMETRY=false` | 42ms to 62ms | |
| host `devpod version`, `DEVPOD_DISABLE_TELEMETRY=true` | 45ms to 55ms | |
| container agent `devpod version` | 479ms to 501ms | 592ms to 614ms |
| container agent, `docker exec -e DEVPOD_DISABLE_TELEMETRY=true` | 99ms to 111ms | 226ms to 247ms |

`DEVPOD_DISABLE_TELEMETRY` is `pkg/config/env.go`'s `EnvDisableTelemetry`, read
by `telemetry.StartCLI` (`pkg/telemetry/collect.go`).

So roughly **0.78s of the 1.66s ProxyCommand is telemetry init and flush inside
two in-container invocations of a 118MB Go binary, each asking it to print a
version string that cannot have changed.**

Whether devlaunch can reach that switch is a separate matter, and the honest
answer measured here is *not easily*. Setting it host-side helps only the host
process:

| | ms |
|---|---|
| `devpod ssh --command true` | 2044, 2363, 2335, 2254, 2014 |
| `DEVPOD_DISABLE_TELEMETRY=true devpod ssh --command true` | 1898, 1980, 1823, 1716, 1842 |
| context `TELEMETRY=false`, `devpod ssh --command true` | 1700, 1756, 1668, 1682, 1925 |

About 350ms, one host-side flush. Neither the env var nor the context option
reaches the injected agent's own execs: a raw ssh trip with context telemetry off
measured 1651ms to 1913ms against 1666ms to 2158ms with it on, which is inside
the noise. A guess at an in-container `~/.devpod/config.json` did not take
either. **This is a lead, not a finding.**

### Per-connection against per-container-lifetime

Nothing in the 1.66s is per-connection *in substance*. The whole of it, is the
agent installed, is it the right version, where is the workspace info, is the
answer to a question about the container, and the container's answer does not
change while it lives. It is re-derived per connection only because each
ProxyCommand is a fresh OS process holding no state from the last one.

The genuinely per-connection floor is the `docker exec` that starts the ssh
server plus an ssh handshake: on the order of **75ms to 150ms**, against 1800ms
paid.

## 2. Does devpod reuse connections? No, and nothing upstream is asking it to

`devpod ssh --help` at v0.26.1 has no flag for reuse, multiplexing, a control
socket, or a persistent tunnel. The full list is `--command`, `--stdio`,
`--send-env`, `--set-env`, `--workdir`, `--user`, `--agent-forwarding`,
`--gpg-agent-forwarding`, `--forward-ports`, `--reverse-forward-ports`,
`--forward-ports-timeout`, `--git-ssh-signing-key`, `--install-terminfo`,
`--term-mode`, `--ssh-keepalive-interval`, `--start-services`.

The two flags that look like they might trim the trip do not:

| | ms |
|---|---|
| `devpod ssh --command true` | 3921, 2104, 2222, 2074 |
| `+ --start-services=false` | 2067, 2004, 2168, 2074 |
| `+ --agent-forwarding=false` | 2244, 2024, 2319, 2202 |
| both false | 2229, 1986, 1890, 2114 |

Which is the expected result: the services start *after* the tunnel, so turning
them off cannot help a cost paid before the banner.

**Which upstream, though.** The devpod this tree pins is
**`github.com/skevetter/devpod` v0.26.1** (read out of the shipped binary's Go
build info), a GitHub fork whose parent is `loft-sh/devpod`. loft-sh's own latest
release is `v0.6.15` (2025-03-10), with prereleases to `v0.7.0-alpha.34`
(2025-06-23) and a last push of 2025-11-14. The fork releases on its own line:
v0.24.0 through v0.26.1 between 2026-05-20 and 2026-06-29. So the version numbers
in this repository's pin are the fork's, and any 0.26-era behaviour question has
to be asked of `skevetter/devpod`, not of loft-sh.

Searched both for prior art: issues and PRs matching ControlMaster, multiplex,
connection reuse, ssh slow, ssh performance, latency, telemetry, inject. Nothing
proposes or discusses connection reuse in either repository. GitHub code search
does not index forks, so the fork's source was read from raw refs at tag
`v0.26.1` rather than searched.

One upstream issue is directly relevant, and not for the reason its title
suggests: [loft-sh/devpod#1929](https://github.com/loft-sh/devpod/issues/1929),
"Can't run more than 4 SSH sessions to a workspace". See section 5.

## 3. ControlMaster over devpod's config: incidental, and the config is rewritten

### Incidental

The block devpod writes is built by `pkg/ssh/config.go`, and
`sshConfigBuilder.addSSHOptions` emits exactly five options plus a
`ProxyCommand` and a `User`:

```
# DevPod Start res389.devpod
Host res389.devpod
  ForwardAgent yes
  LogLevel error
  StrictHostKeyChecking no
  UserKnownHostsFile /dev/null
  HostKeyAlgorithms rsa-sha2-256,rsa-sha2-512,ssh-rsa
  ProxyCommand "…/devpod" ssh --stdio --context default --user vscode res389 --devpod-home "/tmp/res389/devpod"
  User vscode
# DevPod End res389.devpod
```

No `ControlMaster`, `ControlPath` or `ControlPersist`, and the strings do not
appear anywhere in the v0.26.1 tree. Multiplexing works because devpod writes an
ordinary OpenSSH `Host` stanza whose transport happens to be a `ProxyCommand`,
and OpenSSH multiplexes that like any other. **Nothing in devpod knows about it,
so nothing in devpod promises it.** It is incidental. It is also, for the same
reason, not fragile in the way "incidental" usually implies: it does not depend on
devpod cooperating, only on devpod continuing to write a normal ssh config.

### Who writes it, and when

`ConfigureSSHConfig` in `pkg/ssh/config.go`. Its target is, in order:
the `SSH_CONFIG_INCLUDE_PATH` context option if set, else the `SSH_CONFIG_PATH`
context option or `devpod up --ssh-config` / `DEVPOD_SSH_CONFIG`, else
`~/.ssh/config` (`ResolveSSHConfigPath`). It removes the workspace's marker
section, re-adds it, and writes the **whole file** back with
`os.WriteFile(path, content, 0o600)`. The only concurrency guard is a
process-local `var configLock sync.Mutex`, so two devpod processes writing one
config is unprotected.

Measured, when it fires:

| action | a `ControlMaster` line added *inside* the markers | a `Host` block added *outside* them |
|---|---|---|
| `devpod ssh --command true` | survives | survives |
| `devpod up` (no `--recreate`) | **deleted** | survives |

`devpod up` is enough. Not `--recreate`, any `up`. So editing devpod's own block
is not a route: `dl`'s next cold launch destroys it.

Two routes that do survive:

- **`-o` on the ssh command line.** `dl` already does exactly this for
  `SendEnv` (`rust/devlaunch-core/src/clients/ssh.rs::command_args`).
- **A `Host *.devpod` block outside the markers.** Measured: it resolves
  (`ssh -G` reports `controlmaster auto`, `controlpath …`, `controlpersist 600`
  alongside devpod's `forwardagent yes`), survives `devpod up`, and gives 9ms
  reuse. It wins because devpod's specific block sets no `ControlMaster` for
  OpenSSH's first-value-wins rule to prefer. A wildcard block could never
  override something devpod *does* set, such as `ForwardAgent`.

A third thing worth knowing: `devpod ssh` itself is unaffected by either.
`devpod ssh --command true` still costs 2098ms and 2084ms with a live master and
a matching wildcard block in the config, because devpod's own ssh client is Go
and never reads an OpenSSH config. **Multiplexing can only ever help a path that
runs OpenSSH.**

### The safety fact: what a recreate does to a live master

This is the question the ticket calls the single most important, so it was
measured four ways. **It fails closed every time.** No measurement produced a
session that silently ran against a dead or wrong container.

The mechanism is why. The master's ProxyCommand holds a live `docker exec` into
one specific container. Destroy that container and the exec dies, the
ProxyCommand dies, the master exits, and OpenSSH unlinks its own socket.

**(a) `devpod up --recreate`, master idle.** Afterwards
`ssh -O check` reports
`Control socket connect(…): No such file or directory`, rc=255, and the socket
file is gone.

**(b) `devpod up --recreate` with trips running through the master.** 40 trips
back to back, each reading a generation marker written into the old container's
`/tmp` and the container's own hostname, with the recreate running concurrently:

```
 1..28  rc=0    GEN-TWO |host=e1fd8e1c2199      <- old container, still alive
 29     rc=255  GEN-TWO                          <- the moment it died
 30..40 rc=0    cat: /tmp/gen.txt: No such file  |host=a38859766f95   <- new container
```

One trip fails, loudly, with rc=255. The next reopens a master against the new
container and reports the new container's state correctly. Trip 30 pays the full
1.8s again, so the failure mode is "one trip errors, the next is correct and
slow", not "trips keep succeeding against a ghost".

The one wrinkle: rc=255 is OpenSSH's own transport-failure code, and it is
indistinguishable from a remote command that genuinely exited 255. Trip 29 also
emitted partial output before failing. That ambiguity is inherent to ssh and is
not made worse by multiplexing.

**(c) `docker stop` then `docker start`, same container id.** Master dies and
unlinks; the next trip costs 1862ms, reaches the restarted container, and reads
back the file it wrote before the stop.

**(d) master `SIGKILL`ed, socket left orphaned.** The next trip logs
`Stale control socket …, unlinking` / `setting up multiplex master socket`, costs
1823ms, and succeeds. Reuse after that is 11ms to 12ms. Self-healing.

Two more things measured because they would have been hazards:

- A live master does **not** block devpod. With one alive: `devpod status` 578ms
  to 640ms, `devpod ssh --command` 2145ms and 2578ms, `devpod up` 2101ms, all
  normal. The per-workspace lock the ProxyCommand takes is released after tunnel
  setup, not held for the connection's life.
- A plain `devpod up` (no recreate, container already running) leaves the master
  **alive and still fast**: 24ms then 12ms afterwards.

One thing that is *not* covered by the socket dying: `ControlPath %C` hashes the
local host, remote host, port and user. It does **not** include the container's
identity, so successive generations of a workspace share one socket path. That is
fine only because the master reliably dies. It is not defence in depth.

## 4. What a reused connection breaks

### `--send-env` / `SendEnv`: this contradicts the ticket

The ticket states that a pty-requesting trip with `SendEnv` reuses the master
"env still arriving". **It does not, unless the master was opened declaring that
same variable.** Measured with `GH_TOKEN_TEST=hello-389`:

| | result |
|---|---|
| fresh ssh, `-o SendEnv=GH_TOKEN_TEST` | `GOT=[hello-389]` |
| fresh ssh, `-t -o SendEnv=GH_TOKEN_TEST` | `GOT=[hello-389]` |
| `devpod ssh --send-env GH_TOKEN_TEST --command …` | `GOT=[hello-389]` |
| reuse, master opened **without** `SendEnv`, client passes it | **`GOT=[]`** |
| reuse, master opened **with** `SendEnv=GH_TOKEN_TEST`, client passes it | `GOT=[hello-389]` |
| reuse, master opened with `GH_TOKEN_TEST`, client passes `SendEnv=OTHER_VAR` | **`OTHER=[]`** |
| reuse, master opened with it, client passes no `SendEnv` | `GOT=[]` |

Both sides have to permit the name: the client puts its environment into the mux
request, and the **master** filters it against the master's own `SendEnv` list
before emitting the `env` channel requests. OpenSSH does not warn. There is no
"Sending environment" line on the mux path, and the exit status is 0.

For `dl`, whose one variable is `GH_TOKEN`, this is manageable, open the master
with `-o SendEnv=GH_TOKEN`, and one measurement says it is even correct across
runs: the *values* come from the reusing client, so a master opened with
`GH_TOKEN_TEST=hello-389` delivered `ROTATED-VALUE` to a later reuse. The
permit list is stale; the value is not.

But the failure mode if it is ever got wrong is the worst one available: a
workspace comes up with an empty `GH_TOKEN`, silently, and `gh` inside it is
simply unauthenticated. Any design that multiplexes needs the master's
`SendEnv` list pinned by a test, not by a comment.

devpod's Go ssh server has no `AcceptEnv` allowlist to worry about: arbitrary
names arrive on a fresh connection.

### `ForwardAgent yes`: works, and pins the agent

Agent forwarding survives reuse. Over both a fresh connection and a reused
master, `ssh-add -l` in the container listed the host key
(`SHA256:MOsCE0Rrvi… res389`).

But it pins to whichever agent opened the master, for the master's whole life:

- reuse from a client pointing at a **second** agent holding a different key:
  the container still saw the **first** agent's key.
- reuse from a client with **no** `SSH_AUTH_SOCK` at all: the container still got
  a forwarded socket, with the first agent's key.

Both a convenience and a leak. A `ControlPersist` window is a window in which
every session into that workspace reaches an agent the current caller may not
have, and may not want forwarded.

### The remote exit status: a reused ssh does not break it, it removes the need for the machinery

`rust/devlaunch-core/src/clients/devpod.rs` carries `StderrFilter`, `interpret`
and `SshOutcome` for one reason: devpod loses the remote status. Confirmed on both
sides.

Observed:

```
$ devpod ssh res389 --command "exit 3"      # devpod exits 1
fatal  tunnel to container: run in container: ssh session: Process exited with status 3
$ devpod ssh res389 --command "exit 130"    # devpod exits 1
fatal  tunnel to container: run in container: ssh session: Process exited with status 130
```

The sentence matches `REMOTE_EXIT_MARKER` in `devpod.rs`
(`"ssh session: Process exited with status "`) and the `fatal` tag matches
`FATAL_TAG`, so the recovery in the shipped code is reading exactly this.

Why it is needed is visible in `cmd/root.go`: `Execute` *does* try to propagate,
with `if sshExitErr, ok := err.(*ssh.ExitError); ok { os.Exit(sshExitErr.ExitStatus()) }`.
That is an unwrapped type assertion, and the error reaching it has been wrapped
twice (`tunnel to container:` then `run in container:`), so it fails and devpod
exits 1. The `Try using the --debug flag` line that `StderrFilter` holds back is
emitted a few lines further down the same function.

OpenSSH has none of this. Measured, remote status becomes ssh's own status,
identically with and without multiplexing:

| requested | fresh ssh | reused master | `devpod ssh` |
|---|---|---|---|
| 0 | 0 | 0 | 0 |
| 3 | 3 | 3 | 1 + stderr sentence |
| 7 | (n/a) | 7 | (n/a) |
| 130 | 130 | 130 | 1 + stderr sentence |

So a direct ssh path would not need `StderrFilter` or `interpret` at all, which
is what `clients/ssh.rs`'s own doc comment already claims ("OpenSSH exits with
the remote program's status, the thing devpod loses"). This measurement confirms
the claim and extends it to the multiplexed case.

The cost of that simplification is the ambiguity noted in 3(d): ssh's 255 means
both "transport failed" and "the remote command exited 255". devpod's stderr
protocol, for all its awkwardness, distinguishes them.

## 5. The thing nobody was looking for: concurrency

`devpod ssh --stdio` takes a **per-workspace lock**
(`acquired workspace lock`, `workspace_client.go:284` and `:289`), so
non-multiplexed trips to one workspace do not merely cost 1.8s each, they cost
1.8s each **in series**.

Eight concurrent `ssh` runs, each `sleep 8`, no multiplexing:

```
session 2   9861ms      session 7  17930ms
session 1  11852ms      session 6  19852ms
session 5  13857ms      session 3  21864ms
session 4  16115ms      session 8  23853ms
```

A staircase 2s apart, and devpod says why on stderr:
`Trying to lock workspace, seems like another process is running that blocks this workspace`
(`machine_client.go:311`).

The same eight over **one** master:

```
all eight: 8019ms to 8026ms
```

Perfectly parallel. This is the same shape as
[loft-sh/devpod#1929](https://github.com/loft-sh/devpod/issues/1929) ("only 4 can
be established, the 5th hangs"), which at v0.26.1 presents as serialization
rather than failure. Multiplexing is a mitigation for it: N logical sessions
collapse to one `devpod ssh --stdio`, so the lock is taken once.

## 6. Reference table

Quiet machine unless noted.

| | ms |
|---|---|
| `devpod ssh <ws> --command true` | 2014 to 2363 |
| raw `ssh -F <devpod config> <ws>.devpod true`, fresh connection | 1686 to 1980 |
| same, `-t` (pty) | 1950 to 2061 |
| same, second trip over `ControlMaster`/`ControlPersist` | **9 to 12** |
| same, reuse with `-t` | 10, and 17 to 22 loaded |
| `docker exec <cid> bash -lc true` | 125 to 140 |
| `docker exec <cid> true` | 65 to 74 |
| `devpod status <ws>` | 552 to 841 |
| `devpod --help` | 46 to 55 |
| `devpod version` (telemetry on) | 398 to 441 |
| `devpod version` (telemetry off) | 42 to 62 |
| in-container agent `devpod version` | 479 to 501 |
| in-container agent `devpod version`, telemetry off | 99 to 111 |
| `devpod up --recreate` on this one-line devcontainer | ~3400 |

Ratio that matters: a multiplexed reuse is **150x to 200x** cheaper than a fresh
trip, and `docker exec` is **13x to 27x** cheaper.

## What this does NOT settle

- **What `dl` should do.** That is [#390](https://github.com/blooop/devlaunch/issues/390),
  deliberately. Nothing here argues for multiplexing, for `docker exec`, or for
  leaving the trip alone.
- **Host numbers.** Every figure is nested DinD, and part of the run had three
  other agents competing for the same 8 CPUs. The decomposition and the ratios
  are what to carry forward. The absolute seconds are not the map's baseline.
- **Whether the telemetry cost can be reached.** ~0.78s of the trip is telemetry
  in two in-container agent invocations, measured. Neither
  `DEVPOD_DISABLE_TELEMETRY` on the host process nor context `TELEMETRY=false`
  propagates into those invocations, and one guess at an in-container config file
  did not either. It might be reachable by an upstream change to
  `buildExistsCheck`, by dropping the redundant second `version` exec, or by an
  env var the docker driver already forwards. Untested. **This is the single
  most valuable open thread here, because it is a 40% cut to the trip that
  requires nothing of `dl`'s architecture.**
- **Whether the recreate result holds on non-docker providers.** Fail-closed was
  measured only on the docker provider, and the mechanism is the ProxyCommand's
  `docker exec` dying with the container. A ssh or kubernetes provider's
  ProxyCommand dies for different reasons, or possibly does not.
- **Longer `ControlPersist` windows.** Everything here used 600s and reused
  within seconds. Nothing was measured about a master idle for minutes, about
  devpod's `--ssh-keepalive-interval 55s` interacting with an idle master, or
  about a laptop suspending under one.
- **The 4-session ceiling.** Section 5 reproduces serialization at 8 concurrent
  sessions on v0.26.1, not the hard hang loft-sh/devpod#1929 describes. Whether
  the ceiling still exists at some higher count, or was fixed on the fork's line,
  was not established.
- **`ForwardAgent` policy.** That a reused master pins agent forwarding to the
  first caller's agent is measured. Whether that is acceptable is a judgement
  about `dl`'s users, not a measurement.
- **Config-path assumptions in the shipped code.**
  `clients/ssh.rs::config_path()` hardcodes `~/.ssh/config`, while devpod v0.26.1
  will write to `DEVPOD_SSH_CONFIG` / `SSH_CONFIG_PATH` /
  `SSH_CONFIG_INCLUDE_PATH` when any is set. Under `DEVPOD_SSH_CONFIG` (which is
  what a scratch-scoped run of this repository's own test harness uses) devpod
  writes **only** there and `~/.ssh/config` is never created, so
  `devpod_host_configured` answers false and `dl` silently keeps every command on
  the devpod transport. Noticed while measuring, not investigated, and not this
  ticket's question.
