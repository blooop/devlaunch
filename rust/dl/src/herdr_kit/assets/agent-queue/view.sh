#!/usr/bin/env bash
# Make F7 walk a queue of things that need you, rather than a list of everything.
#
#   view.sh set      blocked, done, then idle -- never working    ("needs me")
#   view.sh clear    the Agents panel shows every agent           ("all")
#   view.sh toggle   flip between the two
#
# `keys.next_agent` is documented as "focus the next agent shown in the agent
# panel". It is *positional* -- one row from wherever you already are -- not
# priority-seeking. `ui.agent_panel_sort = "priority"` fixes the panel's ORDER,
# so row 1 is the most blocked, but F7 from row 5 still goes to row 6. That gap
# between "the queue is sorted" and "the key walks the queue" is the whole
# reason this plugin exists.
#
# It is made worse by `done` being defined as "idle and not yet seen": focusing
# the ticked agent marks it seen, it collapses to `idle`, and the panel re-sorts
# underneath you, so the *next* F7 lands somewhere unrelated.
#
# `agent.view.set` is the lever, because it "controls the expanded and collapsed
# sidebar, mobile Agents list, mouse targets, indexed focus, and next/previous
# Agent navigation" -- navigation included. Filter the panel to blocked+done and
# F7 cannot physically land on something that does not need you.
#
# There is no CLI for it, only the socket, which is why this is raw JSON rather
# than a `herdr` invocation like the rest of the plugin API.
#
# `idle` is in the filter too, sorted last. It was blocked+done only at first,
# which made the panel an inbox: `done` means "idle and not yet seen", so an
# agent that finished while its tab was focused went straight to `idle`, never
# entered the panel, and could not be found again by F7 once forgotten. With
# idle included and `attention desc` first, blocked and done still head the
# list -- prefix+alt+1 is still "most urgent", F7 from the top still walks the
# real queue -- and F7 then continues into idle agents, most recently changed
# first, instead of wrapping. The trade is that F7 can once again land on an
# agent that needs nothing; what it buys is never losing one.
#
# The cost, stated plainly: `working` agents leave the Agents panel while they
# work, and reappear the moment they finish or block. herdr has exactly one
# Agents panel and one projection on it, so there is no second sidebar showing
# everything -- the Spaces panel is that view at workspace granularity, prefix+w
# is the picker over all of them, and `toggle` (prefix+a) is the way to look at
# the full list for a moment and come back.
#
# The projection is transient and dies with the server, so `set` is also what
# the manifest's [[startup]] hook runs. State lives in a file because there is
# an `agent.view.set` and an `agent.view.clear` but no `agent.view.get`.

set -euo pipefail

sock="${HERDR_SOCKET_PATH:-${XDG_CONFIG_HOME:-$HOME/.config}/herdr/herdr.sock}"
# Deliberately not HERDR_PLUGIN_STATE_DIR: that is only set when herdr invokes
# the script, and `dl --herdr-setup` calls it directly after linking, so
# honouring it would leave two state files that disagree about which view is on.
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/herdr-agent-queue"
state="$state_dir/view"

[ -S "$sock" ] || {
    echo "agent-queue: no herdr socket at $sock" >&2
    exit 0
}
mkdir -p "$state_dir"

# One line of JSON each: the socket frames requests by newline, so an embedded
# newline would be a second request.
set_request='{"id":"agent-queue-set","method":"agent.view.set","params":{"source":"local.agent-queue","label":"needs me","filter":{"op":"in","field":"status","values":["blocked","done","idle"]},"sort":[{"field":"attention","order":"desc"},{"field":"state_change_seq","order":"desc"}]}}'
clear_request='{"id":"agent-queue-clear","method":"agent.view.clear","params":{"source":"local.agent-queue"}}'

# An alternative filter, the docs' own worked example: everything in the
# workspace you are looking at, plus anything needing attention elsewhere.
# Keeps working agents visible at the cost of F7 walking your current
# workspace's idle agents again.
#   {"op":"any","filters":[
#     {"op":"eq","field":"workspace_id","value":{"context":"current_workspace_id"}},
#     {"op":"in","field":"status","values":["blocked","done"]}]}

send() {
    if command -v socat >/dev/null 2>&1; then
        printf '%s\n' "$1" | socat -T5 - "UNIX-CONNECT:$sock"
    else
        printf '%s\n' "$1" | SOCK="$sock" python3 -c '
import os, socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(5)
s.connect(os.environ["SOCK"])
s.sendall(sys.stdin.buffer.read())
s.shutdown(socket.SHUT_WR)
sys.stdout.write(s.recv(65536).decode())
'
    fi
}

mode="${1:-set}"
if [ "$mode" = toggle ]; then
    if [ "$(cat "$state" 2>/dev/null || true)" = needs-me ]; then
        mode=clear
    else
        mode=set
    fi
fi

case "$mode" in
    set)
        response=$(send "$set_request")
        # A `set` that did not take is worth one line on stderr and nothing
        # more -- a startup hook that fails does not stop the server, and a
        # full agent panel is a degraded queue, not a broken session.
        case "$response" in
            *'"active":true'*)
                echo needs-me >"$state"
                echo "agent-queue: needs me -- F7 walks blocked, done, then idle"
                ;;
            *) echo "agent-queue: agent.view.set did not take: $response" >&2 ;;
        esac
        ;;
    clear)
        response=$(send "$clear_request")
        case "$response" in
            *'"active":false'*)
                echo all >"$state"
                echo "agent-queue: all -- F7 walks every agent"
                ;;
            *) echo "agent-queue: agent.view.clear did not take: $response" >&2 ;;
        esac
        ;;
    *)
        echo "usage: view.sh [set|clear|toggle]" >&2
        exit 2
        ;;
esac
