//! devlaunch's engine: every operation behind the `dl` CLI, as a typed
//! library that `wf` links directly.
//!
//! Four layers, dependencies strictly downward (#251):
//!
//! 1. **runner** — the one seam to the outside: a spawn-spec in, an outcome
//!    out. The only place a process is started.
//! 2. **tool clients** — `devpod`, `git`, `gh`, `ssh`: typed argv builders
//!    and output parsers over the runner.
//! 3. **domain** — `workspace_id`, `spec`, `model`, `metadata`, `config`,
//!    `xdg`, `locks`: the data model, written once.
//! 4. **flows** — `launch`, `lifecycle`, `listing`, `provision`,
//!    `completion`, `disk_usage`, `timing`: the operations.
//!
//! The public API is the day-one wf consumption surface (#250) and nothing
//! else; no user-facing English lives here — text and exit codes are the
//! `dl` binary's rendering.

// The four layers. Everything is crate-private until the day-one public API
// (#250's closure) is frozen at the end of M6; publishing later is free,
// un-publishing is a break. `runner` is pub so devlaunch-test-support can
// implement the trait; that crate is dev-only and never shipped.
//
// # The binary surface
//
// From M5c the four layer modules are `pub`, because the `dl` binary is a
// separate crate and every string a user sees is written there — a rendering
// layer that cannot name the typed results it renders is not a rendering layer.
// The items reached that way carry the note **binary surface — not part of the
// frozen wf API (#250 §7)** so that the day-one public API and the binary's
// working set stay distinguishable: only the former is frozen at the end of M6,
// and only the former is what `wf` may link against. Everything the binary does
// not need stays `pub(crate)`.
pub mod runner {
    //! Re-export of the `devlaunch-runner` leaf crate, kept at its original
    //! path so nothing above this line knows the runner moved crates. It moved
    //! so `devlaunch-test-support` (which implements [`Runner`]) need not
    //! depend on this crate — a dev-dependency back-edge that made core's own
    //! unit tests see two different `Runner` traits.
    pub use devlaunch_runner::*;
}

// Leaf like `runner`: the env-gated span registry everything above may use
// (even `locks` spans a contended wait), depending on nothing itself.
//
// binary surface — not part of the frozen wf API (#250 §7)
pub mod timing;

// Also a leaf, and for the same reason: the sink every flow reports its notices
// through. It depends on none of them, and all of them name it in their
// signatures.
//
// binary surface — not part of the frozen wf API (#250 §7)
pub mod notices;

// A leaf too: `shlex.quote`, which every remote payload in the crate is composed
// out of — and which the `aid` binary composes a dl command line out of.
//
// binary surface — not part of the frozen wf API (#250 §7)
pub mod shell;

// binary surface — not part of the frozen wf API (#250 §7)
pub mod clients;
// binary surface — not part of the frozen wf API (#250 §7)
pub mod domain;
// binary surface — not part of the frozen wf API (#250 §7)
pub mod flows;

#[cfg(test)]
mod tests {
    #[test]
    fn the_workspace_compiles_and_tests_run() {
        assert_eq!(env!("CARGO_PKG_VERSION"), "0.1.0");
    }
}
