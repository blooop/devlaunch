#!/usr/bin/env bash
# Installed by `dl --herdr-setup`, which rewrites this file when devlaunch ships
# a new version of it. Copy it under another name before editing it.
#
# Name the Herdr tab after Claude Code's own session title, and stop the moment
# a human disagrees.
#
# Claude Code already writes a one-line summary of the session to the terminal
# title; Herdr captures it per pane as terminal_title_stripped (the raw one
# carries a spinner glyph). So this is a copy, not a generation step -- no model
# call, no token cost. All it adds is `herdr tab rename`.
#
# Three things it must never fight:
#
#   - A name you typed. Herdr's auto label is either "" or a bare integer (the
#     tab's position in the row), and nothing else ever is, so the integer test
#     alone is enough to claim an unnamed tab. Keeping it after that is what the
#     state file is for: we only overwrite a name we ourselves last wrote. Rename
#     a tab by hand and this goes quiet on it permanently -- and clearing the
#     name in the UI hands it back, because the label reads as auto again.
#
#   - devlaunch. `dl` and `aid` name a tab after its workspace when they launch
#     into it. That name is neither a bare integer nor one this hook wrote, so the
#     hook leaves it alone like any other name a person chose. Inside a
#     devcontainer there is no herdr binary unless DEVLAUNCH_HERDR=1 lends one, and
#     this hook is installed in the host's Claude settings, not the container's.
#     The guards below keep it quiet outside Herdr.
#
#   - Another agent in the same tab. Two Claudes in a split would each write
#     their own title every turn and the tab would flicker between them, so a
#     multi-pane tab keeps its number.
#
# Installed as a Stop hook, so the name tracks the conversation as it drifts
# rather than freezing on whatever the first turn happened to be about. Set
# ONCE_ONLY=1 below for the freeze-on-first-turn behaviour instead.

set -uo pipefail

ONCE_ONLY=0
MAX_LEN=40

# Drain the hook JSON even when we are about to bail, so Claude Code never
# writes into a closed pipe.
hook_input="$(cat 2>/dev/null || true)"

[ "${HERDR_ENV:-}" = "1" ] || exit 0
[ -n "${HERDR_TAB_ID:-}" ] || exit 0
[ -n "${HERDR_PANE_ID:-}" ] || exit 0
command -v herdr >/dev/null 2>&1 || exit 0
command -v jq >/dev/null 2>&1 || exit 0

# A subagent finishing is not the session changing topic.
[ -n "$hook_input" ] &&
    [ "$(printf '%s' "$hook_input" | jq -r '.agent_id // empty' 2>/dev/null)" != "" ] &&
    exit 0

hd() { timeout 2 herdr "$@" 2>/dev/null; }

tab=$(hd tab get "$HERDR_TAB_ID") || exit 0
[ -n "$tab" ] || exit 0
label=$(printf '%s' "$tab" | jq -r '.result.tab.label // ""')
panes=$(printf '%s' "$tab" | jq -r '.result.tab.pane_count // 0')

[ "$panes" = "1" ] || exit 0

state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/herdr-claude-tab-title"
state="$state_dir/${HERDR_TAB_ID//[^A-Za-z0-9]/_}"
last=$(cat "$state" 2>/dev/null || true)

if [[ "$label" =~ ^[0-9]*$ ]]; then
    :                                   # unnamed: ours to claim
elif [ "$label" = "$last" ]; then
    [ "$ONCE_ONLY" = "1" ] && exit 0    # ours already, and we only write once
else
    exit 0                              # somebody named it: hands off
fi

title=$(hd pane get "$HERDR_PANE_ID" |
    jq -r '.result.pane.terminal_title_stripped // empty' |
    tr -d '\r\n' |
    sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')

[ -n "$title" ] || exit 0
[ "$title" = "$label" ] && exit 0
[ "${#title}" -gt "$MAX_LEN" ] && title="${title:0:MAX_LEN-1}…"

hd tab rename "$HERDR_TAB_ID" "$title" >/dev/null || exit 0

mkdir -p "$state_dir" 2>/dev/null || exit 0
printf '%s' "$title" >"$state"

# Tab ids are never reused, so abandoned state files only ever accumulate.
find "$state_dir" -maxdepth 1 -type f -mtime +30 -delete 2>/dev/null

exit 0
