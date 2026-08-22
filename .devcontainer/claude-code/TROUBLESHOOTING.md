# Claude Code Dev Container Feature - Troubleshooting Guide

## Quick Reference

### Files That Must Exist on Host

```bash
~/.claude/              # bound read-write: everything below is live
├── .credentials.json    # OAuth tokens (writable, no mount of its own)
├── .claude.json         # Account info, setup state (writable, no mount of its own)
├── CLAUDE.md           # Global instructions (writable — see below)
├── settings.json       # Settings (writable — see below)
├── agents/             # Custom agents (read-only mount)
├── commands/           # Custom commands (read-only mount)
├── hooks/              # Event hooks (read-only mount)
├── skills/             # Agent Skills (read-only mount)
└── wf-skills/          # Skill bodies (read-only mount)
```

Only the directories are mounted. `~/.claude` itself is one read-write bind, and
the five instruction directories are read-only binds on top of it; the files are
reached *through* the directory rather than bound one at a time. That is what
keeps them live — a bind mount of a file does not survive the host replacing it
by rename, which is what Claude does on every token refresh — and it is why
`CLAUDE.md` and `settings.json` are writable from the container: a read-only
mount over either of them could not be kept for longer than one host edit.

### Critical Configuration in devcontainer.json

```json
{
  "features": {
    "ghcr.io/devcontainers/features/node:1": {},
    "./claude-code": {}
  },
  "containerEnv": {
    "CLAUDE_CONFIG_DIR": "/home/vscode/.claude",
    "XDG_CONFIG_HOME": "/home/vscode/.config",
    "XDG_CACHE_HOME": "/home/vscode/.cache",
    "XDG_DATA_HOME": "/home/vscode/.local/share"
  }
}
```

## Common Issues and Solutions

### Issue 1: Setup Wizard Runs on Every Container Rebuild

**Symptoms:**
- Interactive `claude` shows theme selection screen
- After selecting theme, asks for OAuth authentication
- Happens every time you rebuild the container

**Root Cause:**
Claude tracks setup completion per-workspace in `.claude.json`:
```json
{
  "projects": {
    "/workspaces/pythontemplate": {
      "projectOnboardingSeenCount": 0  // ← This!
    }
  }
}
```

**Solution:**
```bash
# On HOST machine, set a high count to skip wizard
jq '.projects["/workspaces/pythontemplate"].projectOnboardingSeenCount = 999' \
  ~/.claude/.claude.json > ~/.claude/.claude.json.tmp
mv ~/.claude/.claude.json.tmp ~/.claude/.claude.json

# Also ensure themeMode is set (global setting)
jq '. + {themeMode: "dark"}' ~/.claude/.claude.json > ~/.claude/.claude.json.tmp
mv ~/.claude/.claude.json.tmp ~/.claude/.claude.json

# Rebuild container
devpod up . --recreate
```

**Why 999?** The field is `projectOnboardingSeenCount` - it increments each time you see the wizard. Setting it high tells Claude "this workspace has been onboarded many times, skip the wizard."

**Verification:**
```bash
# In container
devpod ssh pythontemplate
claude  # Should go straight to interactive mode without wizard
```

### Issue 2: OAuth Callback Hangs at "Paste code here"

**Symptoms:**
- Browser opens, you click "Authorize"
- CLI shows "Paste code here >" and waits forever
- Browser callback URL fails to connect

**Root Cause:**
The OAuth callback listener runs inside the container, on the container's loopback (e.g. `localhost:54545`). Your browser runs on the host, so it connects to the *host's* loopback, where nothing is listening. A `curl` from the host to that port gets connection refused.

**Solution — authenticate on the host, once:**
Run `claude` on your host machine and complete the flow there. The feature bind-mounts your `.credentials.json`, so it arrives already populated and no container ever needs to run this flow. This is what the shipped configuration does, and it is why the problem does not normally appear.

**If you must complete the flow from inside the container:**
Forward the port for that one session, rather than changing how every container is built:

```bash
devpod ssh <workspace> -L 54545:localhost:54545
```

**Why not `--network=host`:**
Earlier versions of this guide recommended it. It does work, and it costs more than it is worth. Sharing the host's network namespace means a listener in the container *is* a listener on the host: two containers cannot both run the callback flow, and neither can one while anything on the host holds the port (`nc -l -p 54545` in a host-networked container fails with `Address in use` against the host's own listener). It also breaks nesting a Docker daemon, since a second daemon in the host's namespace co-manages the host's `docker0` bridge and writes its NAT rules into the host's netfilter tables.

### Issue 3: `claude --print` Works But Interactive `claude` Asks for Login

**Symptoms:**
- `echo "test" | claude --print` works without authentication
- Running just `claude` shows setup wizard or login prompt

**Root Cause:**
Two different issues:
1. **Setup wizard** (theme/onboarding) - see Issue 1
2. **Print mode skips workspace trust dialogs** - expected behavior

**Solution:**
- For setup wizard: See Issue 1
- For workspace trust: Use `--dangerously-skip-permissions` in trusted containers

### Issue 4: Authentication Doesn't Persist After Container Rebuild

**Symptoms:**
- You authenticate in the container
- Rebuild the container
- Have to authenticate again

**Root Cause:**
`~/.claude` is not mounted, or is mounted read-only, so the two state files
reached through it are missing or unwritable.

**Solution:**

1. **Verify mounts in container:**
   ```bash
   devpod ssh pythontemplate
   mount | grep claude
   ```

   Should show the directory bind read-write, and the instruction
   directories read-only on top of it:
   ```
   /dev/... on /home/vscode/.claude type ext4 (rw,...)
   /dev/... on /home/vscode/.claude/agents type ext4 (ro,...)
   ```

   The state files have no line of their own, and should not: they are inside
   the read-write directory mount. A line naming `.credentials.json` means an
   older layout that pins the file to a dead inode on the next token refresh.

2. **Check files exist on host:**
   ```bash
   ls -la ~/.claude/.credentials.json ~/.claude/.claude.json
   ```

3. **Verify files are writable (not ro):**
   The mounts MUST be read-write for auth to persist.

### Issue 5: "Read-only file system" Error

**Symptoms:**
- Error when trying to write to `~/.claude/CLAUDE.md` or similar
- Operations fail with "Read-only file system"

**Expected Behavior:**
This is intentional! The instruction directories are mounted read-only:
- `agents/`, `commands/`, `hooks/`, `skills/`, `wf-skills/` → Read-only

`CLAUDE.md` and `settings.json` are **not** protected, and a write to either
succeeds and reaches the host. See "Why only directories are mounted" in the
README: under the read-write parent this feature needs, a read-only mount over
a file survives only until the host next replaces that file by rename.

**Why?**
Prevents prompt injection attacks that could modify your Claude configuration.

**Solution:**
Edit these files on your HOST machine, then restart/rebuild the container.

Everything else under `~/.claude` is read-write, including `.credentials.json`
and `.claude.json` (needed for auth and state).

### Issue 6: File Permission Errors (600 vs 664)

**Symptoms:**
- Cannot read credentials file
- Permission denied errors

**Solution:**
```bash
# On HOST
chmod 600 ~/.claude/.credentials.json
chmod 600 ~/.claude/.claude.json
```

These files contain sensitive data and should only be readable by you.

### Issue 7: Nested `docker run` Fails to Bind a `~/.claude` File

**Symptoms:**
- Creating a container from *inside* this container (docker-in-docker, `dl <repo>`) fails with:
  `error mounting "/home/vscode/.claude/.claude.json" ... no such file or directory`
- The file it names exists and reads fine in this container

**Why?**
The container was built by a version of this feature that mounted those files
one at a time. The host replaces them by rename (Claude rewrites `.claude.json`
on nearly every session), which swaps the inode, and a file mount stays pinned
to the old, now-deleted one — readable in here, but a Docker daemon refuses a
deleted-inode mount as a bind source.

The same pinning is why such a container never saw a host account switch: it
went on reading the credentials that existed when it was created.

**Solution:**
Rebuild the container. The current layout mounts no files, so it cannot reach
this state. Until you do, `init-host.sh` heals it before every container
create: a mount whose inode the kernel marks deleted is detached and replaced
with an ordinary file holding the same bytes and mode. If the error still
appears, the repo being launched carries an older `init-host.sh` without the
heal.

## Debugging Commands

### Check Authentication Status

```bash
# In container
cat ~/.claude/.credentials.json | jq '.claudeAiOauth.accessToken' | head -c 30
# Should show: sk-ant-oat01-...

cat ~/.claude/.claude.json | jq '.oauthAccount.emailAddress'
# Should show your email
```

### Verify Mounts

```bash
# In container
mount | grep claude
# Should show all mounted files/directories

ls -la ~/.claude/
# Should show files from your host
```

### Check Environment Variables

```bash
# In container
env | grep -E "(CLAUDE|XDG)" | sort
```

Should show:
```
CLAUDE_CONFIG_DIR=/home/vscode/.claude
XDG_CACHE_HOME=/home/vscode/.cache
XDG_CONFIG_HOME=/home/vscode/.config
XDG_DATA_HOME=/home/vscode/.local/share
```

### Test Claude Without Authentication

```bash
# This should work if you're authenticated
echo "what is 2+2" | claude --print
```

### Check Setup State

```bash
# On HOST
cat ~/.claude/.claude.json | jq '.projects["/workspaces/pythontemplate"]'
```

Look for:
- `projectOnboardingSeenCount`: Should be > 0 (e.g., 999)
- Check your actual workspace path matches

### Verify Network Mode

```bash
# On HOST
docker inspect <container-id> | jq '.[0].HostConfig.NetworkMode'
# Should NOT show "host" -- this container runs on a bridge network
```

## Complete Setup Checklist

When setting up a new workspace:

- [ ] Node.js feature added to devcontainer.json
- [ ] `./claude-code` feature added
- [ ] Environment variables added (CLAUDE_CONFIG_DIR, XDG_*)
- [ ] Files exist on host: `.credentials.json`, `.claude.json`
- [ ] File permissions: `chmod 600` on sensitive files
- [ ] `projectOnboardingSeenCount` set to 999 in `.claude.json`
- [ ] `themeMode` set (e.g., "dark") in `.claude.json`
- [ ] Container rebuilt: `devpod up . --recreate`
- [ ] Test: `claude --print "test"` works
- [ ] Test: `claude` goes to interactive mode without wizard

## File Explanation

### `.credentials.json`
Contains OAuth access and refresh tokens. Format:
```json
{
  "claudeAiOauth": {
    "accessToken": "sk-ant-oat01-...",
    "refreshToken": "sk-ant-ort01-...",
    "expiresAt": 1234567890000
  }
}
```

**Why writable:** Tokens need to be refreshed periodically.

### `.claude.json`
Contains account info, feature flags, and per-workspace state. Key fields:
```json
{
  "oauthAccount": { ... },
  "userID": "...",
  "themeMode": "dark",
  "projects": {
    "/workspaces/pythontemplate": {
      "projectOnboardingSeenCount": 999,
      "hasTrustDialogAccepted": false,
      ...
    }
  }
}
```

**Why writable:** Claude updates `projectOnboardingSeenCount` and other workspace state.

## Advanced Debugging

### Capture Complete Claude Startup

```bash
# In container
script -qec "timeout 3 claude 2>&1" /tmp/claude-startup.log
cat /tmp/claude-startup.log
```

### Compare Config Before/After

```bash
# Before operation
cp ~/.claude/.claude.json ~/.claude/.claude.json.before

# Do operation (e.g., run claude)

# After
diff <(jq -S . ~/.claude/.claude.json.before) <(jq -S . ~/.claude/.claude.json)
```

### Check What Changed on Host

```bash
# On HOST, monitor file changes
watch -n 1 'stat ~/.claude/.claude.json | grep Modify'
```

## Security Considerations

### What's Protected (Read-Only)
- `CLAUDE.md` - Prevents prompt injection
- `settings.json` - Prevents config tampering
- `agents/`, `commands/`, `hooks/` - Prevents malicious modifications

### What's Writable (Necessary Risk)
- `.credentials.json` - OAuth tokens (necessary for auth)
- `.claude.json` - Setup state (necessary to skip wizard)

### Mitigation
- Only use in trusted repositories
- Files have `600` permissions (user-only access)
- Container user isolation
- Regular review of `.claude.json` changes

## Known Limitations

1. **Per-workspace setup tracking**
   - Each workspace path needs its own `projectOnboardingSeenCount`
   - Renaming workspace requires updating the flag

2. **No credential isolation**
   - All containers share same host credentials
   - Can't use different Claude accounts per container

3. **OAuth callback browser routing**
   - The in-container callback needs a forwarded port, or the code pasted by hand
   - Moot in the shipped configuration, which mounts credentials from the host

## Getting Help

If issues persist:

1. Check `/tmp/claude/debug.log` in container
2. Run `claude --debug` for verbose output
3. Review this guide with an AI agent:
   - Share: `.devcontainer/claude-code/TROUBLESHOOTING.md`
   - Include: Output of debugging commands above
   - Describe: Exact symptoms and when they occur

## References

- Dev Container Features: https://containers.dev/implementers/features/
- Claude Code Docs: https://code.claude.com/docs/
- deps_rocker reference: https://github.com/blooop/deps_rocker
- OAuth callback issue: https://github.com/anthropics/claude-code/issues/1529
