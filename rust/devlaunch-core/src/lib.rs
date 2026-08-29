//! devlaunch's engine: every operation behind the `dl` CLI, as a typed
//! library that `wf` links directly.
//!
//! Four layers, dependencies strictly downward (#251):
//!
//! 1. **runner** — the one seam to the outside: a spawn-spec in, an outcome
//!    out. The only place a process is started.
//! 2. **tool clients** — `devpod`, `devpod_home`, `git`, `gh`, `ssh`: typed
//!    argv builders and output parsers over the runner, plus `devpod_home` for
//!    devpod-the-filesystem where `devpod` is devpod-the-command.
//! 3. **domain** — `workspace_id`, `spec`, `model`, `metadata`, `config`,
//!    `xdg`, `locks`: the data model, written once.
//! 4. **flows** — `launch`, `lifecycle`, `listing`, `provision`, `records`,
//!    `completion`, `disk_usage`, `timing`: the operations. Dependencies run
//!    strictly downward, so a flow or a domain type may name a tool client
//!    (`workspace_state` reads `clients::git`); a client never names a flow.
//!
//! No user-facing English lives here — text and exit codes are the `dl`
//! binary's rendering.
//!
//! # Two tiers of public surface, not one
//!
//! The crate exposes two distinct kinds of `pub`, and they must not be
//! conflated:
//!
//! 1. **The frozen wf API** — what `wf` may link against, re-exported from
//!    [`api`] and nothing else. It is the day-one consumption surface #250
//!    charted, frozen by #251 §7 at the end of M6: `list`/`remove`/`up`
//!    equivalents, the spec and branch helpers, and the hand-off constants.
//!    `wf` should import from [`api`] alone; adding to it is a deliberate PR,
//!    removing from it is a breaking change.
//! 2. **The binary surface** — the four layer modules are `pub` so the `dl` and
//!    `aid` binaries, which live in separate crates, can name the typed results
//!    they render: a rendering layer that cannot name what it renders is not a
//!    rendering layer. Every item reached that way carries the note **binary
//!    surface — not part of the frozen wf API (#251 §7)**. It is *not* frozen
//!    and *not* for `wf`; it is an artefact of the crate split, and everything
//!    the binaries do not need stays `pub(crate)`.
//!
//! So a `pub` item here is not automatically part of the promised API. Only
//! what [`api`] re-exports is. The distinction is enforced by keeping [`api`]
//! the single re-export point, by the doc note above on every binary-surface
//! item, and by two `cargo public-api` snapshots that CI diffs on every pull
//! request: any change to the crate's public surface is a committed, reviewed
//! diff or a red tick. The two tiers get a file each — `public-api.api.txt`
//! for the promised tier, `public-api.rest.txt` for the rest — so that a change
//! to what [`api`] promises is a diff in a 521-row file rather than one row
//! inside two thousand.
//!
//! **How the promise file is filled, because the `api` path is not the whole
//! answer.** `cargo public-api` renders an item's own declaration at every path
//! it is reachable by, so [`api::Launch`] gets a row — but it renders inherent
//! methods and trait impls at the type's *canonical* path only, never at the
//! path it is re-exported under, so `api::Launch::run` is rendered
//! `flows::launch::Launch::run`. Matching the `api` path alone therefore kept
//! the promised types' names and dropped all of their behaviour, and renaming
//! `api::Launch::run` — an unambiguous break — left the file byte-identical.
//! So the classifier resolves each of [`api`]'s re-exports back to the path it
//! names and claims that item's rows too (#352), which moved 395 rows out of
//! the rest file: `Launch::{new, run}`, `CommandContext::new`,
//! `DevcontainerPath::as_str` and every derived `Clone`/`Debug`/`PartialEq` on
//! a promised type. A `flows::` or `domain::` path inside `public-api.api.txt`
//! is the promise and not a stray, and the cost of that is real: moving a
//! promised type between modules now churns the file where churn is expensive.
//!
//! What it still does not reach is a type [`api`] never re-exports but a
//! promised signature hands back — `flows::launch::Launched`, which
//! `Launch::run` returns. That is reachable from outside and classified as
//! binary surface, so a break in it diffs `public-api.rest.txt` alone.
//!
//! `scripts/public-api-snapshots.sh` regenerates both; see "The public-API
//! snapshots" in docs/development.md.

// `runner` is pub so `devlaunch-test-support` can implement the trait; that
// crate is dev-only and never shipped.
pub mod runner {
    //! Re-export of the `devlaunch-runner` leaf crate, kept at its original
    //! path so nothing above this line knows the runner moved crates. It moved
    //! so `devlaunch-test-support` (which implements [`Runner`]) need not
    //! depend on this crate — a dev-dependency back-edge that made core's own
    //! unit tests see two different `Runner` traits.
    pub use devlaunch_runner::*;
}

// A leaf below everything: the process boundary (env vars, home, temp dir)
// ported to Python's `os`/`posixpath` semantics rather than std's defaults. It
// depends on nothing in the crate and every layer above reads the host through
// it.
//
// `pub` only for [`osext::env_str`], the way `json` below is `pub` only for
// `JsonKind`: the binaries have environment variables of their own, and one of
// them -- `aid`'s `DEVLAUNCH_AID_AGENT` -- was read with the `std::env::var(..)
// .ok()` this module exists to replace. Everything else here stays
// `pub(crate)`, so this is one reader promoted rather than the module opened;
// whether the rest should follow is #406's open question and wants a second
// caller before it is answered.
//
// binary surface -- not part of the frozen wf API (#251 section 7)
pub mod osext;

// Leaf like `runner`: the env-gated span registry everything above may use
// (even `locks` spans a contended wait), depending on nothing itself.
//
// `json` is pub only for [`json::JsonKind`], which the typed refusals above
// carry and the `dl` binary renders; the Python-spelling writers stay
// `pub(crate)`.
//
// binary surface — not part of the frozen wf API (#251 §7)
pub mod json;
pub mod timing;

// Also a leaf, and for the same reason: the sink every flow reports its notices
// through. It depends on none of them, and all of them name it in their
// signatures.
//
// binary surface — not part of the frozen wf API (#251 §7)
pub mod notices;

// A leaf too: `shlex.quote`, which every remote payload in the crate is composed
// out of — and which the `aid` binary composes a dl command line out of.
//
// binary surface — not part of the frozen wf API (#251 §7)
pub mod shell;

// binary surface — not part of the frozen wf API (#251 §7)
pub mod clients;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod domain;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod flows;

/// The frozen wf consumption surface — and only this.
///
/// `wf` links against these re-exports and nothing else: the day-one surface
/// #250 charted, frozen by #251 §7. Everything else the crate marks `pub` is
/// binary-rendering surface (see the crate docs), reachable but not promised.
///
/// The names are re-exports of the items that already implement each verb, not
/// new wrappers: the crate split forced their promotion out of `pub(crate)`, and
/// gathering them behind one door is what turns "reachable from outside" into
/// "part of the promise". The hand-off constants are promoted from `pub(crate)`
/// here, which is the only place they are meant to be reached from.
pub mod api {
    // up: start or attach a workspace.
    pub use crate::flows::launch::{Launch, LaunchVerb};

    // …and everything `Launch::new` asks for, so that a caller with this module
    // and nothing else can build one that goes cold (#340). Five of the seven
    // parameter types used to live outside here, and the two implementations that
    // decide whether a launch can go cold at all lived in the `dl` binary — so the
    // promised tier named a launcher nobody outside `dl` could construct.
    //
    // The traits come with them: an external caller may lend its own machinery
    // (`NoColdPath`'s shape, for a caller that has established the workspace is
    // warm) instead of the implementations here, and it cannot do that without
    // naming what it is implementing.
    pub use crate::flows::launch::{
        Cold, ColdMachinery, ColdPath, ColdRefused, Host, LaunchNotice, Provision, ToolProvisioning,
    };
    pub use crate::flows::lifecycle::{Refresh, SelfInvocation};
    pub use crate::flows::provision::ProvisionEvent;
    pub use crate::flows::records::{RecordsNotice, StartupError};
    // The sink itself, which is how every one of those vocabularies is taken:
    // `Vec<T>` implements it, so a caller that only wants to collect needs nothing
    // of its own.
    pub use crate::notices::Notices;

    // list: the workspace listing, and the two shapes wf renders it in.
    pub use crate::flows::listing::{CommandContext, enriched_listing, json_document};

    // remove / stop: the two lifecycle verbs that take a workspace away.
    pub use crate::flows::lifecycle::{workspace_delete, workspace_stop};

    // spec and branch helpers: parsing `owner/repo@branch` and friends, the
    // identity a safe name derives, and the `--devcontainer` reference.
    pub use crate::domain::spec::{
        DevcontainerPath, SpecIdentity, WorkspaceSpec, identity, parse, resolve_devcontainer_ref,
    };

    // hand-off constants: the environment-variable names the launch seam writes
    // and reads across the process boundary.
    pub use crate::timing::{HANDOFF_VAR, PREWARM_VAR};
}

// The one fake runner every unit test in this crate drives, wrapped in the one
// thing `devlaunch-test-support` cannot reach from below: the timing exclusion.
#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests {
    #[test]
    fn the_workspace_compiles_and_tests_run() {
        // A smoke assertion, not a pin: the M0 scaffold hardcoded "0.1.0" here,
        // which made every release bump fail this test. The version's one source
        // is `[workspace.package]` in rust/Cargo.toml; this only checks the
        // crates actually inherit it.
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
