#!/usr/bin/env bash
#
# Regenerate the three checked-in `cargo public-api` snapshots -- and, wherever
# they are checked, be the one definition of how the surface is classified.
#
# There are three files because there are three different promises:
#
#   rust/devlaunch-core/public-api.api.txt   the promise: every row declares
#       something `devlaunch_core::api` re-exports, the tier an external
#       consumer is entitled to depend on. A diff here is a deliberate change to
#       that tier -- a removal or a changed signature is a break -- and wants a
#       reviewer who reads it that way. Most of its rows are written at a
#       `flows::` or `domain::` path rather than at `api`, which is not a bug in
#       the filter; see how it is filled, below.
#   rust/devlaunch-core/public-api.rest.txt  the tripwire. The binary surface --
#       `flows::`, `domain::`, `clients::` -- which is reachable but never
#       promised, so most of a diff here is routine and read for the accidental
#       `pub`. But see the limit below before reading one as routine.
#   rust/devlaunch-runner/public-api.txt     the process seam, as an external
#       `Runner` implementer sees it. It had no snapshot until the split: the
#       whole crate entered core's as one unexpanded glob row, so a removed
#       trait method moved nothing and passed CI.
#
# How the promise file is filled, measured rather than assumed. `cargo
# public-api` renders an item's own declaration at every path it is reachable
# by, so `api::Launch` gets a row -- but it renders inherent methods and trait
# impls at the type's *canonical* path only, never at the path it is re-exported
# under, so `api::Launch::run` is rendered
# `devlaunch_core::flows::launch::Launch::run`. Matching the `api` path alone
# kept 182 rows and left 631 in the rest file, `Launch::new` and `Launch::run`
# among them, so renaming `Launch::run` left the promise file byte-identical
# (#352). So the classifier resolves each `api` re-export back to the path it
# names and claims that item's rows too, which is what `promised_row_pattern`
# below does. Some rows are in the file twice, because the generator emits them
# twice: once under the `api` section and once under the module that owns them.
#
# The limit that is left, and it is not one type. A type `api` never re-exports
# but a promised signature names is reachable from outside and is classified as
# binary surface, so a break in it diffs public-api.rest.txt alone. Measured on
# the checked-in files rather than guessed at: 37 such types own close to six
# hundred rows over there. `--print-residual` lists them, needs no toolchain,
# and prints the exact row count; the type count above is the figure
# `test/test_public_api_snapshots_doc.py` diffs against it, because that is the
# one a reader calibrates on and the one that moves only when the residual
# really grows.
#
# The pointed one is `domain::spec::DevcontainerRefError`, because
# `pub fn devlaunch_core::api::resolve_devcontainer_ref(&str) -> Result<...,
# DevcontainerRefError>` is a promise-file row at the `api` path itself: rename
# one of that error's variants and every consumer matching on it breaks, while
# the only file that moves is the one this header calls routine. So is
# `domain::metadata::MetadataError`, which the promised `StartupError::Metadata`
# carries. `flows::launch::Launched`, which `Launch::run` returns, is the
# example this comment used to give as though it were the whole of it.
#
# What that means for reading a rest-file diff: it is routine for a row whose
# subject is nothing a promised signature names, and a contract change for a row
# whose subject is one of the 39. `--print-residual` is how you tell.
#
# Whether the tool does this already, since the obvious first question is why
# any of it is hand-rolled (#352 asked it explicitly). It does not, in the pin
# this script names. `cargo public-api 0.52.0` offers exactly four ways to
# select what is rendered -- `--omit blanket-impls|auto-trait-impls|
# auto-derived-impls` (and the `-s` shorthands), `--include
# function-parameter-names`, the feature and target flags, and `-p` for which
# package -- and not one of them is a path, module or reachability filter. There
# is no upstream notion of "the surface reachable from this module", so the
# choice is a filter over rendered rows or nothing. Re-check it when the pin
# moves; `cargo public-api --help` is the whole answer.
#
# The classification lives here, in the script CI runs, because the alternative
# is two copies of it -- one in the workflow, one in whatever regenerates the
# files -- drifting until the promise file quietly stops holding even what it
# does hold. `.github/workflows/ci.yml`'s `public-api` job runs this into a
# scratch tree and diffs the result against what the repo carries; a developer
# runs it with no argument to accept a deliberate change; and the tests over the
# checked-in files reach it through `--classify` rather than restating it.
#
# Usage:
#   scripts/public-api-snapshots.sh            # rewrite the checked-in files
#   scripts/public-api-snapshots.sh DEST       # write them under DEST instead
#   scripts/public-api-snapshots.sh --print-pin
#   scripts/public-api-snapshots.sh --print-files
#   scripts/public-api-snapshots.sh --print-promised
#   scripts/public-api-snapshots.sh --print-residual      # the limit, counted
#   scripts/public-api-snapshots.sh --classify api|rest   # rows on stdin
#
# Needs a nightly toolchain (cargo-public-api's rustdoc-JSON backend is
# nightly-only; the crates themselves still build on the stable pin) and the
# pinned cargo-public-api. This repository's devcontainer carries neither, so
# this is a host command:
#
#   rustup toolchain install nightly
#   cargo install cargo-public-api --locked \
#       --version "$(scripts/public-api-snapshots.sh --print-pin)"
#
set -euo pipefail

# Pinned, and pinned here so the workflow installs what this script demands
# rather than the two agreeing by coincidence: cargo-public-api's rendering
# moves between releases, and a snapshot generated by a different one is a
# whole-file diff that says nothing. Bump it together with regenerated files.
PIN=0.52.0

# `-ss` omits blanket and auto-trait impls. Those rows move when *rustdoc*
# moves -- `UnsafeUnpin` appeared with a nightly, not with a crate change -- and
# a tripwire that fires on toolchain drift teaches people to update snapshots
# unread. Derived impls (Clone, Debug, serde) stay in: losing one is a real
# break, and a promised type's are in the promise file with the rest of it.
FLAGS=(-ss)

# The boundary matters: `\b` is what keeps a future `devlaunch_core::apiary`
# out of the promise file. `rust/devlaunch-core/tests/public_api_snapshots.rs`
# holds the checked-in files to this same rule from the other side.
API_ROW='devlaunch_core::api\b'

# Every file this script writes, relative to `rust/` -- which is also every
# file CI diffs. Emitted rather than restated there, for the same reason the
# pin is: a fourth snapshot added here should not need a workflow edit to be
# checked, and a workflow that lists them itself is a list that can fall behind.
API_FILE=devlaunch-core/public-api.api.txt
REST_FILE=devlaunch-core/public-api.rest.txt
RUNNER_FILE=devlaunch-runner/public-api.txt
FILES=("$API_FILE" "$REST_FILE" "$RUNNER_FILE")

# Where the promised tier is declared, and so where the canonical paths behind
# it are read from. The `api` module is a wall of `pub use crate::...`, which is
# exactly the mapping the renderer throws away.
CORE_LIB=devlaunch-core/src/lib.rs

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The canonical path of every item the `api` module re-exports, spelled the way
# `cargo public-api` spells one.
#
# Reading Rust with awk is not free, and the alternative was worse: a list of
# paths kept here by hand is a second declaration of what is promised, and the
# day it falls behind `api` the promise file quietly stops covering whatever was
# added. This reads the one declaration there is.
#
# What it understands, exactly, because a form it half-understands is a path it
# gets wrong: `pub use crate::a::b::Name;`, `pub use crate::a::b::{X, Y};`, and
# either of those with an `as` rename on any name, all of them free to wrap
# across lines. `//` and `/* */` comments are removed first, so a commented-out
# re-export is not a promise. Anything else -- a glob, a path that is not an
# identifier chain -- is refused by the shape check below rather than guessed at.
#
# The `as` form matters more than it looks, and the previous version of this
# parser got it wrong *quietly*: it stripped whitespace before looking, so
# `pub use crate::domain::spec::WorkspaceSpec as Spec;` welded into
# `...::WorkspaceSpecasSpec`, which is a valid identifier chain and so passed
# the shape check on its way to matching nothing. The alias is not what to
# match: `cargo public-api` renders the item's methods at its canonical path,
# and the alias only ever appears on the `api`-path row that the first clause of
# the pattern already claims. So the rename is dropped and the canonical path
# kept, which is the right answer rather than merely a loud one.
promised_paths() {
  awk '
    /^pub mod api \{/ { inside = 1; next }
    inside && /^\}/   { inside = 0 }
    inside            { sub(/\/\/.*/, ""); body = body " " $0 }
    END {
      # Block comments only after the lines are joined: one can span them.
      gsub(/\/\*([^*]|\*+[^*\/])*\*+\//, " ", body)
      n = split(body, statements, ";")
      for (i = 1; i <= n; i++) {
        if (statements[i] !~ /pub[ \t]+use[ \t]+crate::/) continue
        item = statements[i]
        sub(/^.*pub[ \t]+use[ \t]+crate::/, "", item)
        # Before the whitespace goes, or `X as Y` becomes the identifier `XasY`.
        gsub(/[ \t]+as[ \t]+[A-Za-z_][A-Za-z0-9_]*/, "", item)
        gsub(/[ \t]/, "", item)
        if (match(item, /\{/)) {
          module = substr(item, 1, RSTART - 1)
          names = substr(item, RSTART + 1)
          sub(/\}.*$/, "", names)
          m = split(names, list, ",")
          for (j = 1; j <= m; j++) if (list[j] != "") print "devlaunch_core::" module list[j]
        } else {
          print "devlaunch_core::" item
        }
      }
    }
  ' "$repo_root/rust/$CORE_LIB" | while read -r path; do
    if [[ ! "$path" =~ ^devlaunch_core(::[A-Za-z_][A-Za-z0-9_]*)+$ ]]; then
      echo "cannot read '$path' as a path: the api module uses a re-export form this script does not parse." >&2
      exit 1
    fi
    echo "$path"
  done
}

# The rule that decides which file a row belongs in, as one extended regex.
#
# Two clauses, and the second is what #352 added. `cargo public-api` renders an
# item's own declaration at every path it is reachable by, so `api::Launch` gets
# a row -- but it renders inherent methods and trait impls at the type's
# *canonical* path only, so `api::Launch::run` is rendered
# `devlaunch_core::flows::launch::Launch::run` and the first clause cannot see
# it. The second clause claims those by resolving the re-export back to the path
# it names.
#
# The anchors are the whole difficulty. A promised type appears in a signature
# somewhere in most of this crate, so claiming any row that *mentions* one would
# drag the binary surface into the promise file. A row's subject is what is
# claimed: the path right after `pub` and its item keywords, or -- for an impl,
# where the subject is either side of `for` -- any path in the header that
# follows a space.
#
# That second anchor is an approximation and it is worth saying which way it is
# wrong, since a comment that oversells it is how the next person stops
# checking. `Vec<Promised>` and `From<Promised>` are safe: a path nested in a
# single-argument generic follows a `<`, never a space. A path nested in a
# *multi*-argument generic, a tuple, or a trait bound does follow a space, so
# `impl From<(u8, Promised)> for Internal` and `impl<T: Promised> Trait for
# Internal` would be claimed as promises. No row of that shape exists today (no
# `where` clause is rendered at all, and no promised path appears in a bound),
# which is why this is a note rather than a second clause: the fix is to parse
# the header rather than match it, and there is nothing yet to parse it for.
#
# Note what this costs, and it is not nothing: the canonical path is now part of
# the promise file, so moving a promised type between modules diffs it. That is
# churn in the file where churn is expensive. It is still the better trade,
# because the alternative is a promise file that a rename of the promise does
# not touch.
promised_row_pattern() {
  local paths
  paths="$(promised_paths | paste -sd'|' -)"
  if [[ -z "$paths" ]]; then
    echo "the api module re-exports nothing: rust/$CORE_LIB no longer declares 'pub mod api'?" >&2
    exit 1
  fi
  # `([a-z]+ )*`, not `?`: a row is `pub <keywords> <path>` and there can be
  # more than one keyword. With `?` a promised type's `pub const fn` or `pub
  # unsafe fn` fell to the tripwire file, which is an ordinary thing to add and
  # a silent hole. Widening cannot over-claim, because every rendered path
  # starts `devlaunch_core::` and no `[a-z]+ ` alternative can consume a segment
  # of one: there is no space inside a path. Measured: no row moves today.
  printf '%s|^pub ([a-z]+ )*(%s)\\b|^impl.* (%s)\\b' "$API_ROW" "$paths" "$paths"
}

# The limit, counted: every type the promise file names that is not itself
# promised and owns rows in the tripwire file, as `<rows>\t<path>` sorted by
# rows. Those are the types a promised signature hands you and the promise file
# does not cover, so a diff to one of them reads as routine churn and is not.
#
# Read off the checked-in snapshots, so this needs no toolchain and nothing is
# generated. It exists because the header used to name one such type as though
# it were the whole residual, and prose that carries a number nothing recomputes
# is prose that is wrong a release later: `test/test_public_api_snapshots_doc.py`
# diffs the figures in the three places the limit is described against this.
#
# A "type" is a path whose last segment is capitalised, which is what separates
# `domain::spec::DevcontainerRefError` from the module it lives in and from the
# free functions beside it. Ownership is the same anchored subject test the
# classifier uses, not a mention: a row that merely takes one as an argument
# belongs to whatever declares it.
residual_types() {
  local api_file="$repo_root/rust/$API_FILE" rest_file="$repo_root/rust/$REST_FILE"
  for file in "$api_file" "$rest_file"; do
    [[ -r "$file" ]] || {
      echo "--print-residual reads the checked-in snapshots and $file is not there." >&2
      exit 1
    }
  done
  local promised
  promised="$(promised_paths)"
  grep -oE 'devlaunch_core(::[A-Za-z_][A-Za-z0-9_]*)+' "$api_file" |
    grep -E '::[A-Z][A-Za-z0-9_]*$' |
    sort -u |
    grep -Fxv -f <(printf '%s\n' "$promised") |
    while read -r path; do
      local rows
      rows="$(grep -cE "^pub ([a-z]+ )?$path\\b|^impl.* $path\\b" "$rest_file" || true)"
      [[ "$rows" -gt 0 ]] && printf '%s\t%s\n' "$rows" "$path"
    done | sort -rn
}

# One side of the split, over rows on stdin. `grep` says "no match" with status
# 1, which is not an error for a filter -- an empty side is only wrong when a
# whole snapshot is being generated, and the generation path checks it there.
classify() {
  local status=0
  grep -E "$@" || status=$?
  [[ "$status" -le 1 ]] || exit "$status"
}

case "${1:-}" in
--print-pin)
  echo "$PIN"
  exit 0
  ;;
--print-files)
  printf '%s\n' "${FILES[@]}"
  exit 0
  ;;
--print-promised)
  promised_paths
  exit 0
  ;;
--print-residual)
  residual_types
  exit 0
  ;;
--classify)
  # The classification, reachable without a nightly toolchain and without
  # generating anything -- so the rule above can be exercised on rows chosen to
  # be awkward. `test/test_public_api_snapshots_doc.py` is what does that.
  pattern="$(promised_row_pattern)"
  case "${2:-}" in
  api) classify "$pattern" ;;
  rest) classify -v "$pattern" ;;
  *)
    echo "--classify takes 'api' or 'rest'" >&2
    exit 2
    ;;
  esac
  exit 0
  ;;
esac

dest="${1:-$repo_root/rust}"
for file in "${FILES[@]}"; do
  mkdir -p "$dest/$(dirname "$file")"
done
# Absolute from here on. Everything below runs after a `cd` into `rust/`, which
# would otherwise re-anchor a relative DEST: the `mkdir` above lands beside the
# caller and the writes land somewhere else entirely -- or, as it happened,
# fail at the redirect and report it as a missing `api` module.
dest="$(cd "$dest" && pwd)"

if ! installed="$(cargo public-api --version 2>/dev/null)"; then
  echo "cargo-public-api is not installed. cargo install cargo-public-api --locked --version $PIN" >&2
  exit 1
fi
if [[ "$installed" != "cargo-public-api $PIN" ]]; then
  echo "cargo-public-api is '$installed', and these snapshots are rendered by $PIN." >&2
  echo "Install the pin, or bump PIN in this script and regenerate everything with it." >&2
  exit 1
fi

# Refuse before generating rather than half-way through applying. Two minutes
# of rustdoc followed by "permission denied" on the third move is the worst
# version of this: measured, it leaves two files updated and one stale, and a
# mixed set satisfies every invariant the tests over these files can check --
# the partition holds on any complementary pair -- so only CI's
# regenerate-and-diff would notice.
for file in "${FILES[@]}"; do
  if [[ ! -w "$dest/$(dirname "$file")" ]]; then
    echo "cannot write $dest/$(dirname "$file"): nothing generated, nothing changed." >&2
    exit 1
  fi
done

# Stage every file, and move them into place only once all three exist. The
# shell truncates a redirect target *before* the command on its right runs, so
# writing the destinations directly means a failed generation -- or a guard
# below firing -- empties a checked-in snapshot on the way to reporting the
# problem. That is a script whose whole job is to write those files leaving
# them at zero bytes, and only one of the tests over them notices.
#
# Staged *inside* `$dest`, not in `/tmp`: a real checkout is a different
# filesystem from `/tmp` (measured: device 66306 against 81), which makes every
# `mv` a copy-and-unlink rather than a rename, so an interrupted move can leave
# a destination half-written. Same filesystem means each move is a rename, and
# a rename either happened or did not. The set of three is still not one atomic
# act -- a crash between renames leaves some new and some old, each file whole
# -- and CI's diff is what catches that.
staging="$(mktemp -d "$dest/.staging.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
for file in "${FILES[@]}"; do
  mkdir -p "$staging/$(dirname "$file")"
done

cd "$repo_root/rust"

core="$(cargo public-api -p devlaunch-core "${FLAGS[@]}")"
pattern="$(promised_row_pattern)"

# Two greps rather than one pass with a fallthrough, so the two files are
# complements by construction. Either coming out empty means the filter no
# longer matches the crate, which is a broken split rather than a small API.
if ! printf '%s\n' "$core" | grep -E "$pattern" >"$staging/$API_FILE"; then
  echo "no rows matched the promise: devlaunch-core no longer declares an 'api' module?" >&2
  exit 1
fi
if ! printf '%s\n' "$core" | grep -Ev "$pattern" >"$staging/$REST_FILE"; then
  echo "every row matched the promise: the split has nothing left to classify?" >&2
  exit 1
fi

# Every promised path has to claim something. Without this the parse above can
# drift silently: `api` grows a re-export form awk reads as a shorter path, or
# a rename leaves a path nothing is rendered at, and the promise file loses the
# rows for it while every test over the two files still passes -- they check the
# split is a partition, not that it is the right one.
#
# A here-string rather than a pipe into `grep -q`: `grep -q` stops reading at
# the first match, `printf` takes a SIGPIPE for the rest, and `pipefail` reports
# that as the pipeline failing -- so every path that *did* claim a row was
# reported as one that had not.
while read -r path; do
  if ! grep -qE "^pub ([a-z]+ )?$path\\b|^impl.* $path\\b" <<<"$core"; then
    echo "the api module re-exports $path and no rendered row declares it." >&2
    echo "Either that item is gone, or --print-promised is reading the module wrongly." >&2
    exit 1
  fi
done < <(promised_paths)

cargo public-api -p devlaunch-runner "${FLAGS[@]}" >"$staging/$RUNNER_FILE"

for file in "${FILES[@]}"; do
  mv "$staging/$file" "$dest/$file"
done
