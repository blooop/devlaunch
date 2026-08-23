#!/usr/bin/env bash
# Re-record the README's demo GIFs from the tapes in docs/demo/.
#
#   pixi run demo             # all of them
#   pixi run demo 2-branches  # just one, by tape name
#
# A script rather than a pixi task body because it needs a loop, a `command -v`
# and a preflight -- and pixi's task shell has no `for` and no `if`.
#
# Easiest on a host: vhs films the RELEASED `dl`, which needs devpod and docker.
# `Require dl` in the tapes fails a machine without one in a line.
#
# This repo's devcontainer can also do it -- it has docker-in-docker and a
# devpod, and these GIFs were recorded in one -- but you have to add the
# released `dl` (`pixi run dl` is the working tree, not the shipped build), vhs
# with ttyd/ffmpeg/gifsicle, and the libraries vhs's headless chromium needs
# (libnss3, libgbm1, libasound2t64 and friends). That chromium won't start
# without a sandbox either, so put a `chrome` shim adding `--no-sandbox` on PATH.
set -euo pipefail

cd "$(dirname "$0")/../docs/demo"

# vhs resolves `Source` and `Output` against the working directory, not against
# the tape's own directory, so the cd above is load-bearing: run vhs from the
# repo root instead and `Source _common.tape` is a file-not-found and the GIFs
# land beside it.

for tool in vhs ttyd ffmpeg; do
  command -v "$tool" >/dev/null || {
    echo "record_demo: '$tool' is not on PATH." >&2
    echo "  vhs needs all three: ttyd runs the shell it films, ffmpeg encodes it." >&2
    echo "  brew install vhs ttyd ffmpeg   (or pacman/nix/scoop; see charmbracelet/vhs)" >&2
    exit 127
  }
done

command -v dl >/dev/null || {
  echo "record_demo: no 'dl' on PATH -- these tapes film the RELEASED dl." >&2
  echo "  pixi global install --channel conda-forge --channel https://prefix.dev/blooop devlaunch" >&2
  exit 127
}

# One tape by name, or all of them in order -- 4-cleanup deletes the workspaces
# the others attach to, so it goes last. Named rather than globbed because
# _common.tape is a fragment with no Output line, and vhs errors on those.
tapes=("${@:-}")
if [[ -z "${tapes[0]}" ]]; then
  tapes=(1-launch 2-branches 3-agent 4-cleanup)
fi

for name in "${tapes[@]}"; do
  echo "==> ${name}.tape"
  vhs "${name}.tape"
done

# GIF is the only format that autoplays in a GitHub README, and vhs has no
# colour or file-size setting -- so this is the only place the weight of the
# front page can be brought down. Optional on purpose: a missing gifsicle
# should cost bytes, not the recording.
if command -v gifsicle >/dev/null; then
  for name in "${tapes[@]}"; do
    gifsicle -O3 --lossy=60 --batch "${name}.gif"
  done
  echo "==> optimised with gifsicle"
else
  echo "note: gifsicle not found; GIFs are unoptimised (expect ~2-3x the size)." >&2
fi

ls -lh -- *.gif

# The tapes drive a UI that moves. Everything below has been wrong at least
# once, so look before you commit.
cat >&2 <<'CHECKS'

Watch each one back before committing:
  * nothing wraps mid-word -- the prompt inside a workspace is the long line,
    and a longer branch name is what breaks it
  * 2-branches: the query narrows the picker to ONE row, and Enter lands in
    main. skim does not re-rank, so two surviving rows means Enter takes the
    workspace you just left
  * 3-agent: it cuts while the agent is starting, before the model's answer
  * 4-cleanup: it ends on "No workspaces found."
CHECKS

# The README ships each <img> line commented out, so that a clone with no GIFs
# recorded yet never shows broken images on its front page. Recording one is
# what earns it its line, so uncommenting is this script's last step rather than
# a note asking you to do it.
#
# Per GIF and idempotent: only an entry whose file now exists is uncommented,
# and an already-live line has no comment left to match.
cd ../..
for name in "${tapes[@]}"; do
  [[ -f "docs/demo/${name}.gif" ]] || continue
  perl -0pi -e "s{<!-- Recorded by docs/demo/${name}\.tape[^\n]*\n(!\[[^\n]*\n)-->\n}{\$1}" README.md
done

git --no-pager diff --stat -- README.md docs/demo || true
