#!/bin/sh
# Make sure every host path this devcontainer bind-mounts exists.
# This script runs on the HOST before the container is created.
# Everything here is idempotent - it only creates what is missing, and never
# clobbers or reconfigures anything the developer already has.

# The claude-code feature mounts the developer's Claude configuration a path at
# a time rather than as one directory, so that the files holding *executable
# instructions* -- CLAUDE.md, settings.json, agents/, commands/, hooks/, skills/
# and wf-skills/ -- can be read-only while credentials and onboarding state stay
# writable. Every one of
# those sources has to exist before the container is created, and the cost of a
# missing one is not a warning: the create is refused outright with `bind mount
# source path does not exist`, measured on devpod 0.26.1. That is the same
# failure the single whole-directory mount had on a host with no ~/.claude, so
# this list is longer than it was rather than newly load-bearing -- and nothing
# is written to the host when it happens, which is why creating these here is
# the whole fix.
#
# A Feature cannot declare a host-side hook, which is why this list lives here
# rather than beside the mounts it serves; the consuming devcontainer wires this
# script up as its initializeCommand.
#
# Before anything is created: every file mounted one at a time is a file its
# owner replaces *by rename* -- Claude rewrites .claude.json on nearly every
# host session, a token refresh rewrites .credentials.json, ssh rewrites
# known_hosts when a key rotates. A rename swaps the inode and a bind mount
# pins the old one, so run from inside a container created before the rename,
# these paths are mounts of **deleted** inodes. The container itself reads them
# fine, which is why nothing notices -- until it creates a container of its
# own: a Docker daemon refuses a deleted-inode mount as a bind source (runc:
# `no such file or directory`), which is `dl <repo>` in here failing for every
# repo that mounts these same files (devlaunch#326). This hook is the one
# thing that runs host-side before every create, so it is where the stale
# mount is caught: detach it and leave an ordinary file holding the same bytes
# and mode, so the nested bind has a real inode to pin. Only a mount whose
# root the kernel marks deleted is touched -- a live mount is the container's
# working connection to the host file, and detaching one would sever every
# container this repo builds from its own configuration. On a real host these
# paths are ordinary files, no mountinfo entry matches, and none of this runs
# -- including the `sudo` that detaching needs when the caller is not root,
# which is why it can be asked for here at all: it is only reached inside a
# container, where this repo's images give the user passwordless root. A stale
# mount that still cannot be detached costs the heal and not the launch: the
# path stays readable, only a *nested* create was ever going to trip on it,
# and aborting `devpod up` here would turn that maybe into a certainty.
as_root() {
    if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi
}

heal_stale_file_mount() {
    mounted="$1"
    # mountinfo is Linux; a macOS host runs devpod too, and five awk
    # complaints per create is this heal charging a platform it cannot
    # even be needed on.
    [ -r /proc/self/mountinfo ] || return 0
    case "$(awk -v p="$mounted" '$5 == p { r = $4 } END { print r }' /proc/self/mountinfo)" in
    *"//deleted") ;;
    *) return 0 ;;
    esac
    # The agent socket goes stale the same way and blocks a nested create the
    # same way, but has no bytes to carry over -- a deleted socket inode is a
    # dead endpoint however it is mounted -- so its whole heal is the detach.
    # The test matters beyond that economy: *reading* a stale FIFO blocks
    # forever, and this runs inside initializeCommand, where hanging is worse
    # than any failure being healed.
    if [ ! -f "$mounted" ]; then
        as_root umount "$mounted" 2>/dev/null || as_root umount -l "$mounted" 2>/dev/null || :
        return 0
    fi
    mode=$(stat -c '%a' "$mounted" 2>/dev/null) || return 0
    saved=$(mktemp) || return 0
    # Read through the mount before detaching it: the deleted inode's bytes
    # are unreachable afterwards, and they are the developer's live state.
    if ! cat "$mounted" > "$saved" 2>/dev/null; then
        rm -f "$saved"
        return 0
    fi
    if as_root umount "$mounted" 2>/dev/null || as_root umount -l "$mounted" 2>/dev/null; then
        # Written into the file the mount was covering rather than renamed
        # over it -- a rename is what made the inode stale in the first place,
        # and the point here is a plain file at a stable inode.
        if as_root cp "$saved" "$mounted" 2>/dev/null; then
            as_root chown "$(id -u):$(id -g)" "$mounted" 2>/dev/null || :
            as_root chmod "$mode" "$mounted" 2>/dev/null || :
        else
            # The mount is gone and the write-back failed, so the temporary
            # copy is now the only copy: keep it and say where it is.
            echo "init-host.sh: detached the stale mount at $mounted but could not write its content back; the bytes are kept at $saved" >&2
            return 0
        fi
    else
        echo "init-host.sh: $mounted is a mount of a deleted inode and could not be detached; containers created from here may fail to bind it" >&2
    fi
    rm -f "$saved"
}

for mounted_file in "$HOME/.claude/CLAUDE.md" "$HOME/.claude/settings.json" \
    "$HOME/.claude/.credentials.json" "$HOME/.claude/.claude.json" \
    "$HOME/.ssh/known_hosts" "$HOME/.ssh/agent.sock"; do
    heal_stale_file_mount "$mounted_file"
done
#
# Every line is guarded on absence, and that is load-bearing rather than tidy.
# Run from *inside* a container this repo built -- which is the point of giving
# it a Docker daemon -- these paths are the read-only mounts, and a write to one
# fails with EROFS. A non-zero initializeCommand aborts `devpod up` outright.
# `mkdir -p` on a directory that already exists writes nothing and is safe there;
# `touch` on a file that already exists is not, so files are created only when
# absent. The two JSON files are seeded with an empty object rather than left
# zero-length because Claude parses them, and the pair holding secrets is created
# 600 -- applied only when this script is the one creating the file, so a
# developer's existing permissions are never rewritten.
mkdir -p "$HOME/.claude" "$HOME/.claude/agents" "$HOME/.claude/commands" \
    "$HOME/.claude/hooks" "$HOME/.claude/skills" "$HOME/.claude/wf-skills"
[ -e "$HOME/.claude/CLAUDE.md" ] || touch "$HOME/.claude/CLAUDE.md"
[ -e "$HOME/.claude/settings.json" ] || echo '{}' > "$HOME/.claude/settings.json"
[ -e "$HOME/.claude/.credentials.json" ] || {
    touch "$HOME/.claude/.credentials.json"
    chmod 600 "$HOME/.claude/.credentials.json"
    echo '{}' > "$HOME/.claude/.credentials.json"
}
[ -e "$HOME/.claude/.claude.json" ] || {
    touch "$HOME/.claude/.claude.json"
    chmod 600 "$HOME/.claude/.claude.json"
    echo '{}' > "$HOME/.claude/.claude.json"
}

# known_hosts is mounted as a *file*, and Docker creates nothing for a file
# source: if it is missing the container does not start degraded, it does not
# start at all ("bind source path does not exist"). A developer who has only
# ever pushed over HTTPS has no such file, so this is not a hypothetical.
#
# -m 700 applies only to a directory this actually creates, so an existing
# ~/.ssh keeps whatever permissions the developer gave it.
#
# The existence test is not a tidiness habit, it is what lets this container
# build itself. Run from inside it -- which is the whole point of giving it a
# Docker daemon -- $HOME/.ssh/known_hosts *is* the read-only mount, so a bare
# touch fails with EROFS. This is the last command in the script, so the
# script's exit status is its exit status, and a non-zero initializeCommand
# aborts `devpod up` outright:
#
#   fatal run agent command failed: exit status 1
#   devcontainer up: exit status 1
mkdir -m 700 -p "$HOME/.ssh"
[ -e "$HOME/.ssh/known_hosts" ] || touch "$HOME/.ssh/known_hosts"
