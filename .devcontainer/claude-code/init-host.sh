#!/bin/sh
# Make sure every host path this devcontainer bind-mounts exists.
# This script runs on the HOST before the container is created.
# Everything here is idempotent - it only creates what is missing, and never
# clobbers or reconfigures anything the developer already has.

# The claude-code feature mounts the developer's Claude configuration as the
# directory itself, plus a read-only mount over each subdirectory holding
# *executable instructions* -- agents/, commands/, hooks/, skills/ and
# wf-skills/. Every one of those sources has to exist before the container is
# created, and the cost of a missing one is not a warning: the create is refused
# outright with `bind mount source path does not exist`, measured on devpod
# 0.26.1 -- and nothing is written to the host when it happens, which is why
# creating them here is the whole fix.
#
# Only directories are mounted, and that is the load-bearing part rather than an
# accident of which paths happen to be directories. A bind mount of a *file*
# does not survive its source being replaced by rename: the mount is attached to
# the dentry, the rename puts a new one at that name, and the mount is dropped
# from the namespace entirely. Measured both ways round, because the direction
# of the failure follows the parent mount and neither direction is safe:
#
#   rw parent + read-only file mounts -> after the rename the file is WRITABLE,
#       so a protection the manifest still advertises is silently gone from the
#       first host edit onwards
#   ro parent + read-write file mounts -> after the rename the file is READ-ONLY,
#       so a token refresh fails
#
# A mount of a *directory* survives the same rename with its flags intact, which
# is why the read-only list is exactly the five instruction directories and why
# CLAUDE.md and settings.json are no longer mounted at all: under a writable
# parent their read-only mounts were enforceable only until the developer next
# edited them, which is worse than not claiming the protection.
#
# The same rename is what used to make these paths go stale in the other
# direction. A container holds the mount it was created with, so a file mount
# pinned the pre-rename inode and the container went on reading it forever --
# an account switch on the host reached no running container, which is the
# regression this layout exists to end. The directory mount resolves names per
# access, so it follows the rename and the container reads what the host has.
#
# A Feature cannot declare a host-side hook, which is why this list lives here
# rather than beside the mounts it serves; the consuming devcontainer wires this
# script up as its initializeCommand.
#
# Before anything is created: a container built by an *earlier* version of this
# feature mounted CLAUDE.md, settings.json, .credentials.json and .claude.json
# one file at a time, and those mounts go stale exactly as described above --
# the container reads them fine, which is why nothing notices, until it creates
# a container of its own: a Docker daemon refuses a deleted-inode mount as a
# bind source (runc: `no such file or directory`), which is `dl <repo>` in there
# failing for every repo that mounts the same paths (devlaunch#326).
#
# Nothing this feature mounts today can reach that state, so on a host, and in
# any container built since, every path below is an ordinary file, no mountinfo
# entry matches and none of this runs. It stays because the containers that
# *do* have those mounts are still running: they are healed in place rather than
# having to be rebuilt, and the list can be deleted once they are gone.
#
# This hook is the one thing that runs host-side before every create, so it is
# where the stale mount is caught: detach it and leave an ordinary file holding
# the same bytes and mode, so the nested bind has a real inode to pin. Only a
# mount whose root the kernel marks deleted is touched -- a live mount is the
# container's working connection to the host file, and detaching one would sever
# every container this repo builds from its own configuration. The `sudo` that
# detaching needs when the caller is not root is only ever reached inside a
# container, where this repo's images give the user passwordless root, which is
# why it can be asked for here at all. A stale mount that still cannot be
# detached costs the heal and not the launch: the path stays readable, only a
# *nested* create was ever going to trip on it, and aborting `devpod up` here
# would turn that maybe into a certainty.
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
# it a Docker daemon -- the five instruction directories are the read-only
# mounts, and a write to one fails with EROFS. A non-zero initializeCommand
# aborts `devpod up` outright. `mkdir -p` on a directory that already exists
# writes nothing and is safe there.
#
# Only the mounted directories are created. The files that used to be created
# here -- CLAUDE.md, settings.json, .credentials.json, .claude.json -- are no
# longer mounted one at a time, so a missing one can no longer refuse the
# create, and Claude makes each of them itself on first use. Seeding an empty
# `{}` over a credentials file was never anything but a way to satisfy a bind
# source, and on a host that has never run Claude it is indistinguishable from
# a logged-out session.
mkdir -p "$HOME/.claude" "$HOME/.claude/agents" "$HOME/.claude/commands" \
    "$HOME/.claude/hooks" "$HOME/.claude/skills" "$HOME/.claude/wf-skills"

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
