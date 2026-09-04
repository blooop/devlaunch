#!/bin/sh
set -eu

# Claude Code CLI Local Feature Install Script
# Installs Claude Code via pixi and sets up configuration directories

# Global variables set by resolve_target_home
TARGET_USER=""
TARGET_HOME=""

# Function to resolve target user and home directory with validation
# Sets TARGET_USER and TARGET_HOME global variables
resolve_target_home() {
    TARGET_USER="${_REMOTE_USER:-vscode}"
    TARGET_HOME="${_REMOTE_USER_HOME:-}"

    # If _REMOTE_USER_HOME is not set, try to infer from current user or /home/<user>
    if [ -z "${TARGET_HOME}" ]; then
        if [ "$(id -un 2>/dev/null)" = "${TARGET_USER}" ] && [ -n "${HOME:-}" ]; then
            TARGET_HOME="${HOME}"
        elif [ -d "/home/${TARGET_USER}" ]; then
            TARGET_HOME="/home/${TARGET_USER}"
        fi
    fi

    # If TARGET_HOME is set but doesn't exist, try fallbacks
    if [ -n "${TARGET_HOME}" ] && [ ! -d "${TARGET_HOME}" ]; then
        if [ -n "${HOME:-}" ] && [ -d "$HOME" ]; then
            echo "Warning: TARGET_HOME '${TARGET_HOME}' does not exist, falling back to \$HOME: $HOME" >&2
            TARGET_HOME="$HOME"
        elif [ -d "/home/${TARGET_USER}" ]; then
            echo "Warning: TARGET_HOME '${TARGET_HOME}' does not exist, falling back to /home/${TARGET_USER}" >&2
            TARGET_HOME="/home/${TARGET_USER}"
        fi
    fi

    # Ensure we ended up with a valid, existing home directory
    if [ -z "${TARGET_HOME}" ] || [ ! -d "${TARGET_HOME}" ]; then
        echo "Error: could not determine a valid home directory for user '${TARGET_USER}'." >&2
        echo "Checked _REMOTE_USER_HOME ('${_REMOTE_USER_HOME:-}'), \$HOME ('${HOME:-}'), and /home/${TARGET_USER}." >&2
        exit 1
    fi
}

# Function to install pixi if not found
install_pixi() {
    echo "Installing pixi..."

    # Detect architecture
    case "$(uname -m)" in
        x86_64|amd64) ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
    esac

    # Download and install pixi
    curl -fsSL "https://github.com/prefix-dev/pixi/releases/latest/download/pixi-${ARCH}-unknown-linux-musl" -o /usr/local/bin/pixi
    chmod +x /usr/local/bin/pixi

    echo "pixi installed successfully"
    pixi --version
}

# Function to install Claude Code CLI via pixi
install_claude_code() {
    echo "Installing Claude Code CLI via pixi..."

    # Install pixi if not available
    if ! command -v pixi >/dev/null; then
        install_pixi
    fi

    # Resolve target user and home (sets TARGET_USER and TARGET_HOME)
    resolve_target_home

    # Install with pixi global from blooop channel
    # Run as target user so it installs to their home directory
    #
    # Deliberately unversioned, which is the opposite of what this file's
    # position suggests. This script sits inside the prebuild's build context
    # (.devcontainer, see devcontainer.json), so devpod's prebuild tag is a hash
    # of these bytes -- and a floating package spec is a published image whose
    # contents can move while every hashed byte stays identical. The instinct is
    # to pin, because bumping a pin is a visible diff that republishes.
    #
    # What that argument misses is what the package is. claude-shim is 21KB of
    # bash -- `claude`, `cld`, `cldr` -- and carries no claude at all: the first
    # run downloads the current stable binary from Anthropic's GCS bucket into
    # ~/.claude/cache and re-checks stable hourly after that. The image cannot
    # serve a stale claude because there is no claude in it, so a pin would
    # freeze the fetcher and nothing that gets fetched.
    #
    # It would also freeze it *harder* than not pinning does. Unversioned, the
    # shim is refreshed by every republish, which is every commit that touches
    # this directory; pinned, it stops at whatever was current the day the pin
    # was written, until somebody remembers to bump it. And on the launch path
    # this container is actually opened by, the baked shim is never executed:
    # devlaunch's probe reads a shim-provided claude as `lendable` and the
    # transfer puts the host's real binary at ~/.local/bin/claude with a profile
    # prepend that wins the PATH (README, "What to bake so a launch does no work
    # at all"). What it is baked for is that `command -v claude` answers at all.
    #
    # Measured 2026-08-20: the published prebuild carries claude-shim 0.7.0 --
    # the newest of the channel's 14 releases, unmoved since 2026-04-06 -- so
    # the drift a pin would have prevented is currently zero. The spec here is
    # also the one devlaunch/tools.py installs into every workspace `dl` opens
    # (REQUIRED_TOOLS), the copy that reaches users rather than us; a unit test
    # asserts the two still match, so pinning one side alone fails there. See
    # "What the prebuild tag does not promise" in docs/development.md.
    if [ "$(id -u)" -eq 0 ] && [ "$TARGET_USER" != "root" ]; then
        su - "$TARGET_USER" -c "pixi global install --channel https://prefix.dev/blooop claude-shim"
    else
        pixi global install --channel https://prefix.dev/blooop claude-shim
    fi

    # Put pixi's bin directory on the user's login PATH, and the claude-shim
    # env's bin beside it (the pixi trampoline fails for packages that ship a
    # shell script -- the same workaround devlaunch's provision script
    # carries). Each line goes in under a `# devlaunch:` mark, and the guard
    # is an exact-line match on that mark: this file is the *user's* profile,
    # not this feature's, and a guard that asks about a directory name reads
    # a base image's own PATH block as work already done -- the misread that
    # cost devlaunch a re-paid transfer on every launch (#164).
    #
    # Both edits are devlaunch's _profile_prepend output, verbatim. The marks
    # are content hashes of the lines they guard, which sh cannot rederive,
    # so they are pasted rather than computed -- and matching them is what
    # lets this installer (at image build) and devlaunch's provision script
    # (at `up`) recognise each other's work instead of each appending its own
    # copy of the same line. A test pins these fragments byte-for-byte;
    # regenerate with devlaunch.tools._profile_prepend if a line changes.
    #
    # Which file to edit is devlaunch.tools._profile_resolution, pasted the
    # same way and with $TARGET_HOME for $HOME -- this installer edits a home
    # it is not running in. bash sources only the first of ~/.bash_profile,
    # ~/.bash_login and ~/.profile that exists, so on an image shipping a
    # ~/.bash_profile this used to write both lines into a file nothing ever
    # reads -- and to look for its marks there too, missing the provision
    # script's identical lines in the file bash does read (#191).
    local PROFILE
    if [ -f "$TARGET_HOME/.bash_profile" ]; then PROFILE="$TARGET_HOME/.bash_profile"
    elif [ -f "$TARGET_HOME/.bash_login" ]; then PROFILE="$TARGET_HOME/.bash_login"
    else PROFILE="$TARGET_HOME/.profile"
    fi
    grep -qxF '# devlaunch: 6b593c3a6327' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 6b593c3a6327' 'export PATH="$HOME/.pixi/bin:$PATH"' >> "$PROFILE"
    grep -qxF '# devlaunch: 12897b113bea' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 12897b113bea' '[ -d "$HOME/.pixi/envs/claude-shim/bin" ] && export PATH="$HOME/.pixi/envs/claude-shim/bin:$PATH"' >> "$PROFILE"
    chown "$TARGET_USER:$TARGET_USER" "$PROFILE" 2>/dev/null || true

    # Verify installation by checking the trampoline exists (don't run it - that triggers download)
    local pixi_bin_path="$TARGET_HOME/.pixi/bin"
    local claude_bin="$pixi_bin_path/claude"
    if [ -x "$claude_bin" ]; then
        echo "Claude Code CLI installed successfully!"
        echo "(Claude binary will be downloaded on first run)"
        return 0
    else
        echo "ERROR: Claude Code CLI installation failed! Binary not found at $claude_bin"
        return 1
    fi
}

# Function to create Claude configuration directories
create_claude_directories() {
    echo "Creating Claude configuration directories..."

    # Resolve target user and home (sets TARGET_USER and TARGET_HOME)
    resolve_target_home

    echo "Target home directory: $TARGET_HOME"
    echo "Target user: $TARGET_USER"

    # Create the main .claude directory and subdirectories
    mkdir -p "$TARGET_HOME/.claude"
    mkdir -p "$TARGET_HOME/.claude/agents"
    mkdir -p "$TARGET_HOME/.claude/commands"
    mkdir -p "$TARGET_HOME/.claude/hooks"

    # No empty config files are seeded, and `.credentials.json` is the reason.
    #
    # This used to write `{}` into `.credentials.json` and `.claude.json` when they were
    # missing. Both existed only to give a bind mount a source to cover, from the layout
    # where this feature mounted nine individual paths under `~/.claude` -- and the
    # host-side hook already retired its half of that on the same reasoning: "Seeding an
    # empty {} over a credentials file was never anything but a way to satisfy a bind
    # source, and on a host that has never run Claude it is indistinguishable from a
    # logged-out session" (init-host.sh).
    #
    # In the container that stub is worse than useless, because it wins. `dl` forwards a
    # profile's login as CLAUDE_CODE_OAUTH_TOKEN, Claude Code reads the credentials file
    # first, and an empty one is a logged-out session: the agent asks the operator to log
    # in while a valid token sits in its environment. It stayed hidden for as long as this
    # feature mounted the host's real credentials file *over* the stub -- so the bug was
    # invisible in exactly the configuration that could not use the token anyway, and
    # surfaced the moment a container was given the forwarded login as its only one.
    #
    # Measured both ways in one container: with the stub, `claude` prompts for a login;
    # with it removed and nothing else changed, `claude -p` answers on the forwarded
    # token. Claude Code creates both files itself on first use, so there is nothing to
    # replace this with.
    #
    # Guarded by test_the_feature_seeds_no_empty_credential.

    # Set proper ownership
    if [ "$(id -u)" -eq 0 ]; then
        chown -R "$TARGET_USER:$TARGET_USER" "$TARGET_HOME/.claude" || true
    fi

    echo "Claude directories created successfully"
}

# Main script
main() {
    echo "========================================="
    echo "Activating feature 'claude-code' (local)"
    echo "========================================="

    # Resolve target user and home (sets TARGET_USER and TARGET_HOME)
    resolve_target_home

    local claude_bin="$TARGET_HOME/.pixi/bin/claude"

    # Install Claude Code CLI
    if [ -x "$claude_bin" ]; then
        echo "Claude Code CLI is already installed"
    else
        install_claude_code || exit 1
    fi

    # Create Claude configuration directories
    create_claude_directories

    echo "========================================="
    echo "Claude Code feature activated successfully!"
    echo "========================================="
    echo ""
    echo "Host config (CLAUDE.md, settings.json, agents/, commands/, hooks/)"
    echo "is bind-mounted read-only; credentials are mounted read-write."
    echo "Sessions and history are container-local and do not persist."
    echo ""
    echo "To authenticate, run 'claude' and follow the OAuth flow."
    echo ""
}

main
