#!/bin/sh
# Make sure every host path this devcontainer bind-mounts exists.
# This script runs on the HOST before the container is created.
# Everything here is idempotent - it only creates what is missing, and never
# clobbers or reconfigures anything the developer already has.

mkdir -p "$HOME/.claude"

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
