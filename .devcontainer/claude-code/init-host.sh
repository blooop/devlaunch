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
