# Claude Code CLI - Local Dev Container Feature

A local Dev Container Feature that installs the Claude Code CLI and gives the container your host machine's Claude configuration as a live bind of the directory, with the subdirectories holding executable instructions mounted read-only over it.

## What This Feature Does

This feature combines two capabilities:

1. **CLI Installation**: Installs the `@anthropic-ai/claude-code` npm package globally
2. **Configuration Mounting**: Mounts your host machine's Claude configuration directory into the container, with each subdirectory of executable instructions mounted read-only on top of it

## What Gets Installed

- **Claude Code CLI**: The `claude` command becomes available in your container
- **VS Code Extension**: Automatically installs the `anthropic.claude-code` extension
- **Configuration Directories**: Creates `.claude/` structure in the container

## What Gets Mounted

The following files and directories from your **host machine** are mounted into the container:

### Read-Only Mounts (Security-Protected)
- `~/.claude/agents/` → Custom agent configurations
- `~/.claude/commands/` → Command definitions
- `~/.claude/hooks/` → Event-driven shell hooks
- `~/.claude/skills/` → Agent Skills, and the links a skill installer leaves there
- `~/.claude/wf-skills/` → the skill bodies those links point at

These are **read-only** (`readonly` flag) to prevent:
- Prompt injection attacks that could modify your Claude configuration
- Accidental modification of shared configuration from within containers
- Security issues related to hook manipulation

Every one of them is a **directory**, and that is a constraint rather than a
coincidence — see "Why only directories are mounted" below. `CLAUDE.md` and
`settings.json` are the two instruction files this list does not cover, because
a read-only mount over either of them could not be kept.

Skills are two mounts because they are one thing. An installer that keeps the
prompt bodies in a sibling directory and leaves *relative* links behind —
`skills/wf -> ../wf-skills/wf`, which is what `wf skills install` from
blooop/wayfinder writes — needs both sides in here or every link arrives
dangling, and a container with dangling links has no skills at all rather than
stale ones. Read-only for the same reason `commands/` is: a skill is executable
instructions. That costs one thing, and it is a warning rather than a failure —
`wf` heals its own links on every launch, so a `wf` run *inside* a container
reports that it could not refresh them and carries on.

### Read-Write Mounts (Authentication & State)
- `~/.claude/.credentials.json` → OAuth access/refresh tokens
- `~/.claude/.claude.json` → Account info, user ID, workspace setup tracking

Neither has a mount of its own. Both are reached through the one bind of
`~/.claude` itself, which is mounted read-write, and that is what keeps them
**live**: the container reads whatever the host has now, not what it had when
the container was created.

That distinction is the whole point of the layout. These two files **must be
writable** to enable:
- OAuth authentication flow and token refresh
- Workspace setup state tracking (`projectOnboardingSeenCount`)
- Your account and its onboarding state surviving container rebuilds (session
  *transcripts* do not: see "What Is Not Mounted")

They must also be **current**, which is the harder half. Claude replaces both by
rename — a token refresh writes a new `.credentials.json` and moves it into
place — and a container mounting either of them *as a file* went on reading the
inode that existed when it was created, for as long as it lived. Changing your
Claude account on the host then reached no running container: it kept
authenticating as the account you had left. Mounting the directory is what fixes
that, because a directory mount resolves names on each access and so follows the
rename.

### Why only directories are mounted

A bind mount of a file does not survive its source being replaced by rename. The
mount is attached to the dentry; the rename puts a new one at that name and the
mount is dropped from the namespace altogether. The path then reads through
whatever the parent mount provides, and the flags that were on the file mount
are simply gone.

Measured both ways round, on Docker, against a host doing exactly what Claude
does (write a temporary file, `mv` it into place):

| arrangement | after the host replaces the file |
|---|---|
| read-write parent, **read-only file** mounts | the file is **writable** — the protection is gone, silently, from the first host edit |
| read-only parent, **read-write file** mounts | the file is **read-only** — a token refresh fails |
| either parent, **read-only directory** mounts | unchanged: still live, still read-only |

So a file mount is unsound in both directions, and the direction it fails in is
decided by the parent rather than by what the mount asked for. Only directory
mounts are kept, and every source in the manifest is a directory. A test asserts
that, because the tempting change — protecting `CLAUDE.md` and `settings.json`
by naming them in the mount list — is one that passes review, appears in
`docker inspect`, and stops being true the next time you edit either file.

The cost is stated plainly: `CLAUDE.md` and `settings.json` are writable from
the container. `settings.json` can name hook commands inline, so this is a real
hole and not only a theoretical one. It is smaller than the alternative, which
was the same hole with a manifest claiming it was closed.

### What Is Not Mounted Read-Only

Everything else under `~/.claude` — session transcripts, `projects/`,
`history.jsonl`, `shell-snapshots/`, `plugins/`, and the two instruction files
`CLAUDE.md` and `settings.json` — is inside the read-write directory mount. The
container sees the host's copy of each, live, and can write to all of them.

This is the part of the layout to weigh before using it. The read-only list is
an allow-list of *protection*, not of visibility: a directory Claude starts
writing to next month is visible and writable from the container the day it
appears, and only the five named directories are proof against a prompt
injection that tries to edit its own instructions.

What you get for that is a container whose Claude is the same Claude as the
host's — same account, same login, same settings, updated in place when you
change them — rather than one holding a snapshot of the moment it was built.
Sessions you start on the host do resume in the container, because `projects/`
is shared.

One consequence people go looking for after the fact: a `settings.json` entry
naming a script under `~/.claude/scripts/` — a `statusLine` command, say —
resolves in the container, because that directory is inside the mount too. The
same entry naming a script *outside* `~/.claude` does not.

### Why These Must Be Writable

**`.credentials.json`**: OAuth tokens need to be refreshed periodically. Claude writes updated tokens to this file.

**`.claude.json`**: Claude tracks per-workspace setup state here. The `projectOnboardingSeenCount` field must be writable so Claude doesn't show the setup wizard on every launch.

⚠️ **Security Note**: These files contain sensitive data and are reachable read-write by necessity. They are only accessible by the container user and stored with `600` permissions. Only use this feature with trusted repositories.

## Usage

### Setup

Add this feature to your `devcontainer.json`:

```json
{
  "features": {
    "./claude-code": {}
  }
}
```

**Note**: Node.js is automatically installed via the `installsAfter` dependency mechanism - you don't need to explicitly add it to your features.

### The host-side prerequisite

Every mounted path has to exist on the host before the container is created, and
a Feature cannot arrange that for itself: `initializeCommand` is a
`devcontainer.json` property, and the Feature specification has no equivalent. So
the consuming `devcontainer.json` wires up the script this feature ships beside
its manifest:

```json
{
  "initializeCommand": ".devcontainer/claude-code/init-host.sh",
  "features": {
    "./claude-code": {}
  }
}
```

It creates only what is missing and rewrites nothing you already have — which is
also what lets it run *inside* a container built this way, where the paths it
would create are the read-only mounts and a write would fail.

Skipping it does not degrade the container, it stops it. Measured on devpod
0.26.1 against a host with no `~/.claude`:

```
devcontainer up: runner run container: bind mount source path does not exist
  …/.claude/skills
```

Nothing is created on the host when that happens; the create is refused before
any container exists.

### The OAuth callback, and why host networking is not the answer

This feature used to tell you to add `"runArgs": ["--network=host"]`, on an
argument about the OAuth callback. The argument's mechanism is real; its
conclusion was wrong, and the flag has been removed.

**The mechanism, which is real:**
1. You run `claude` → starts the OAuth flow
2. Your browser opens → you click "Authorize"
3. The browser redirects to `http://localhost:<port>/callback`
4. The listener waiting for that callback is **inside the container**

Your browser runs on the host, so `localhost` in step 3 is the *host's*
loopback, and nothing is listening there. A `curl` from the host to that port
gets connection refused. The callback genuinely does not complete on its own.

**Why that is not a reason for host networking.** Two independent reasons:

- **The port can simply be forwarded.** `devpod ssh -L <port>:localhost:<port>`
  makes the host's loopback reach the container's, and the one-click callback
  works again. That is a per-session flag, not a property baked into every
  container this feature builds.
- **The shipped configuration never runs that flow anyway.** This feature
  bind-mounts your `.credentials.json`, so it is already populated when the
  container starts. The interactive OAuth flow is for a host that has never
  authenticated — and that host should authenticate itself, once, rather than
  every container it builds carrying a networking mode for the case.

**And host networking has a cost that is not hypothetical.** Sharing the host's
network namespace means a listener in the container *is* a listener on the host.
Two containers cannot both run the callback flow, and a container cannot run it
while anything on the host holds the port — demonstrated: `nc -l -p 54545` in a
host-networked container fails with `Address in use` against the host's own
listener. In this repo it is worse than a collision, because the container runs
a Docker daemon of its own: a second daemon in the host's namespace would
co-manage the host's `docker0` bridge and write its NAT rules into the host's
netfilter tables.

**If you have never authenticated on the host**: run `claude` there once. The
credentials land in `~/.claude/.credentials.json` and every container this
feature builds picks them up.

### Build the Container

With DevPod:
```bash
devpod up . --recreate
```

With VS Code:
- Open the folder in VS Code
- Run: "Dev Containers: Rebuild Container"

## Requirements

### Host Machine

These are the paths this feature mounts, and all of them have to exist before the
container is created:

```bash
~/.claude/
├── CLAUDE.md           # Global instructions
├── settings.json       # Claude settings
├── agents/             # Custom agents
├── commands/           # Custom commands
├── hooks/              # Event hooks
├── .credentials.json   # OAuth tokens
└── .claude.json        # Account and workspace state
```

**None of them is optional.** A missing one aborts the container create rather
than producing a warning — see "The host-side prerequisite" above, which is how
this is normally handled. By hand, it is:

```bash
mkdir -p ~/.claude/{agents,commands,hooks}
touch ~/.claude/CLAUDE.md
echo '{}' > ~/.claude/settings.json
```

(A host that has ever run `claude` already has `.credentials.json` and
`.claude.json`.)

### Container

- **Node.js 18+** and **npm** are automatically installed via the `installsAfter` dependency mechanism
- No manual configuration required

## Assumptions

1. **Container User**: This feature assumes the container user is `vscode` (standard for Dev Containers)
   - Configuration files are mounted to `/home/vscode/.claude/`
   - If your container uses a different user (e.g., `root`, `codespace`), you'll need to customize the mounts in your `devcontainer.json`

2. **HOME Environment Variable**: Must be set on the host machine (standard on Unix systems)

3. **Persistence**: Your host machine's `~/.claude/` directory should persist across container rebuilds

4. **Platform**: Designed for Linux/macOS hosts
   - Windows WSL2 should work
   - Windows native may require path adjustments

## How to Iterate Locally

### Quick Changes

1. Edit files in `.devcontainer/claude-code/`:
   - `devcontainer-feature.json` - Change mounts, extensions, or metadata
   - `install.sh` - Modify installation logic
   - `README.md` - Update documentation

2. Rebuild the container:
   ```bash
   devpod up . --recreate
   ```

### Testing Install Script

You can test the install script standalone:

```bash
cd .devcontainer/claude-code
sudo ./install.sh
```

### Debugging

Check if Claude is installed:
```bash
claude --version
```

Check mounted files:
```bash
ls -la ~/.claude/
```

Verify mounts are read-only:
```bash
echo "test" >> ~/.claude/CLAUDE.md  # Should fail with "Read-only file system"
```

## Authentication

### How It Works

1. **Already Authenticated on Host**: If you have Claude Code set up on your host machine, credentials are automatically shared with the container
2. **First-Time Setup**: Run `claude` in the container and follow the OAuth flow:
   - The CLI will provide an OAuth URL
   - Open the URL in your browser (on your host machine)
   - Click "Authorize"
   - The callback should complete automatically, or you may need to paste the code
   - Credentials are saved to `~/.claude/.credentials.json` on your host

### OAuth Callback Behavior

The OAuth flow opens a local callback server. In containers, this can behave differently:
- **VS Code Dev Containers**: Usually handles port forwarding automatically
- **DevPod**: May require manual code pasting if callback doesn't complete
- **SSH/Remote**: Callback URL opens in your local browser

### Troubleshooting Authentication

**"Paste code here" prompt hangs forever:**
- Check that `~/.claude/.credentials.json` exists on your host with proper permissions (`600`)
- Try authenticating on your host machine first, then rebuild the container
- If the callback fails, look for the authorization code in the URL after clicking "Authorize"

**Credentials not persisting:**
- Ensure the `.credentials.json` file exists on your host before rebuilding
- Check file permissions: `chmod 600 ~/.claude/.credentials.json`

**Setup wizard runs on every rebuild (theme selection, OAuth):**

This happens because Claude tracks setup completion **per-workspace**, not globally.

**Quick fix:**
```bash
# On your HOST machine:
# Set the onboarding flag for your workspace
jq '.projects["/workspaces/pythontemplate"].projectOnboardingSeenCount = 1' ~/.claude/.claude.json > ~/.claude/.claude.json.tmp
mv ~/.claude/.claude.json.tmp ~/.claude/.claude.json

# Also ensure themeMode is set (if needed)
jq '. + {themeMode: "dark"}' ~/.claude/.claude.json > ~/.claude/.claude.json.tmp
mv ~/.claude/.claude.json.tmp ~/.claude/.claude.json

# Rebuild container
devpod up . --recreate
```

**Root cause:** Claude tracks setup wizard completion per-workspace in `.claude.json` under `.projects["/workspaces/pythontemplate"].projectOnboardingSeenCount`. When this is `0`, the setup wizard runs. Set it to `1` to mark setup as complete.

**For future workspaces:** Replace `/workspaces/pythontemplate` with your actual container workspace path.

## Modifying Configuration

Every mounted configuration file except `.credentials.json` and `.claude.json` is read-only. You **cannot** modify Claude settings from within the container.

To change configuration:

1. Edit files on your **host machine**: `~/.claude/settings.json`, `~/.claude/CLAUDE.md`, etc.
2. Restart or rebuild the container to see changes

This is by design for security (prevents prompt injection attacks).

## What Would Change Before Publishing to GHCR

If you wanted to publish this feature to GitHub Container Registry later:

### 1. Repository Structure

Move from `.devcontainer/claude-code/` to a dedicated repo:

```
anthropics/devcontainer-features/
└── src/
    └── claude-code/
        ├── devcontainer-feature.json
        ├── install.sh
        └── README.md
```

### 2. Metadata Updates

In `devcontainer-feature.json`:

```json
{
  "id": "claude-code",
  "version": "1.0.0",  // Semantic versioning
  "documentationURL": "https://github.com/anthropics/devcontainer-features/tree/main/src/claude-code",
  // ... rest of config
}
```

### 3. Testing Infrastructure

Add GitHub Actions workflow (`.github/workflows/test.yaml`):

```yaml
- name: "Create test prerequisites"
  run: |
    mkdir -p ~/.claude/agents
    mkdir -p ~/.claude/commands
    mkdir -p ~/.claude/hooks
    touch ~/.claude/settings.json
    touch ~/.claude/CLAUDE.md
    touch ~/.claude/.credentials.json
    touch ~/.claude/.claude.json
```

### 4. Publishing Workflow

Add release workflow to build and push to `ghcr.io/anthropics/devcontainer-features/claude-code:1`

### 5. Reference Change

Users would then reference it as:

```json
{
  "features": {
    "ghcr.io/anthropics/devcontainer-features/claude-code:1": {}
  }
}
```

Instead of `"./claude-code": {}`

## Optional: Future Composition

### Splitting into Modular Features

This feature could be split into:

1. **`claude-code-core`**: Just CLI installation, no mounts
   ```json
   {
     "features": {
       "./claude-code-core": {}
     }
   }
   ```

2. **`claude-code-mounts`**: Just configuration mounts (requires `claude-code-core`)
   ```json
   {
     "features": {
       "./claude-code-core": {},
       "./claude-code-mounts": {}
     }
   }
   ```

Benefits:
- Users can install CLI without mounts (useful for Codespaces or CI)
- More flexible composition
- Easier to maintain and test separately

### Composition with Custom Features

You could create a personal feature that extends this:

```json
// .devcontainer/my-claude-setup/devcontainer-feature.json
{
  "id": "my-claude-setup",
  "installsAfter": ["./claude-code"],
  "customizations": {
    "vscode": {
      "settings": {
        "claude.someCustomSetting": "value"
      }
    }
  }
}
```

Then use both:

```json
{
  "features": {
    "./claude-code": {},
    "./my-claude-setup": {}
  }
}
```

## Troubleshooting

### OAuth callback hangs at "Paste code here"

**Problem**: Browser clicks "Authorize" but the container never receives the callback.

**Root cause**: The listener is on the container's loopback and your browser is
on the host's. Nothing is wrong with the container.

**Solution**: Authenticate on the host once — `claude` there, then rebuild — and
the mounted `~/.claude/.credentials.json` means the container never runs this
flow. If you must complete it from inside, forward the port for that session:

```bash
devpod ssh <workspace> -L <port>:localhost:<port>
```

See "The OAuth callback, and why host networking is not the answer" above.

### Interactive `claude` asks for authentication but `claude --print` works

**Problem**: You're authenticated (credentials mounted) but interactive mode prompts for login.

**Root cause**: Not networking. Check that `~/.claude/.credentials.json` is
actually mounted and readable in the container, and that `CLAUDE_CONFIG_DIR`
points at the mounted directory rather than a fresh one.

### `bind mount source path does not exist`

**Problem**: `devpod up` fails before any container exists, naming a path under
`~/.claude`.

**Root cause**: This feature mounts named paths, and a bind whose source is
missing is refused rather than created. Nothing was written to your host.

**Solution**: Wire up `initializeCommand` as shown in "The host-side
prerequisite" — that is what creates them, on every create, for everyone. To
unblock one machine now:

```bash
mkdir -p ~/.claude/{agents,commands,hooks}
touch ~/.claude/CLAUDE.md
echo '{}' > ~/.claude/settings.json
```

## Security Notes

This implementation makes conscious security trade-offs to enable OAuth authentication and persistent setup state:

### What's Protected (Read-Only Mounts)
- **CLAUDE.md**: Prevents prompt injection attacks that could modify your global instructions
- **settings.json**: Prevents config tampering
- **agents/**, **commands/**, **hooks/**: Prevents malicious code execution through modified hooks

### What's Writable (Necessary Trade-off)
- **`.credentials.json`**: OAuth tokens must be writable for token refresh to work
- **`.claude.json`**: Workspace state must be writable to persist `projectOnboardingSeenCount` and other setup tracking

### Security Mitigations
- Files have `600` permissions (user-only access)
- Only use this feature in **trusted repositories**
- Container user isolation provides some protection
- Writable files are limited to authentication/state only
- All configuration and code execution files remain read-only

### Known Risks
- A malicious process in the container could exfiltrate OAuth tokens from `.credentials.json`
- A malicious process could modify workspace state in `.claude.json`
- **Recommendation**: Only use in repositories you trust, as you would with any dev container configuration

See related security discussions:
- [anthropics/claude-code#4478](https://github.com/anthropics/claude-code/issues/4478)
- [anthropics/claude-code#2350](https://github.com/anthropics/claude-code/issues/2350)
- Original read-only approach: [PR #25](https://github.com/anthropics/devcontainer-features/pull/25)

## Reference

- **Dev Container Features Spec**: https://containers.dev/implementers/features/
- **Local Features**: https://containers.dev/implementers/features/#local-features
- **Based on PR**: https://github.com/anthropics/devcontainer-features/pull/25
- **Upstream Features**: https://github.com/anthropics/devcontainer-features

## License

Based on the Anthropic devcontainer-features repository (MIT License).
