//! This crate's public surface is checked in, because it is the one an
//! external `Runner` implementer actually sees.
//!
//! Until the split it had no snapshot of its own: the whole crate entered
//! `devlaunch-core`'s snapshot as a single unexpanded row -- the glob re-export
//! `pub use devlaunch_core::runner::<<devlaunch_runner::*>>` -- so removing a
//! trait method, or changing what `Outcome` can be, moved nothing in any
//! checked-in file and passed CI silently.
//!
//! These tests do not police the whole snapshot -- the CI job's diff against a
//! fresh `cargo public-api` run does that. They hold the file to being *this*
//! crate's seam: present, and naming the declarations a caller implements
//! against, so a truncated or misdirected regeneration fails here instead of
//! being committed as the new truth.

const SNAPSHOT: &str = include_str!("../public-api.txt");

fn rows(snapshot: &str) -> Vec<&str> {
    snapshot
        .lines()
        .map(str::trim_end)
        .filter(|row| !row.is_empty())
        .collect()
}

fn has_row_starting(snapshot: &str, prefix: &str) -> bool {
    rows(snapshot)
        .into_iter()
        .any(|row| row.starts_with(prefix))
}

#[test]
fn the_seam_carries_a_snapshot_of_its_own() {
    assert!(
        has_row_starting(SNAPSHOT, "pub mod devlaunch_runner"),
        "devlaunch-runner's snapshot does not describe devlaunch-runner: {:#?}",
        rows(SNAPSHOT).first()
    );
}

#[test]
fn the_snapshot_pins_the_trait_an_implementer_writes_against() {
    // The whole row, supertraits included, because a supertrait is a promise to
    // whoever implements this trait exactly as a method is: `Sync` says a runner
    // may be handed to several threads at once, which is what lets `flows::listing`
    // ask its status round trips together, and dropping it would break every
    // implementation that had come to rely on being shareable. So it is pinned
    // here rather than tolerated by a prefix match, and changing it is a
    // deliberate edit to this line.
    assert!(
        rows(SNAPSHOT).contains(&"pub trait devlaunch_runner::Runner: core::marker::Sync"),
        "the Runner trait is missing from the snapshot, or no longer requires Sync"
    );
    for method in ["capture", "passthrough", "session", "detach"] {
        assert!(
            has_row_starting(
                SNAPSHOT,
                &format!("pub fn devlaunch_runner::Runner::{method}(")
            ),
            "Runner::{method} is missing from the snapshot; every method of this \
             trait is a promise to whoever implements it"
        );
    }
}

#[test]
fn the_snapshot_pins_what_a_run_can_turn_out_to_be() {
    assert!(
        has_row_starting(SNAPSHOT, "pub enum devlaunch_runner::Outcome"),
        "the Outcome enum is missing from the snapshot"
    );
    for variant in ["Ran", "ProgramNotFound", "TimedOut", "NotStarted"] {
        assert!(
            has_row_starting(
                SNAPSHOT,
                &format!("pub devlaunch_runner::Outcome::{variant}")
            ),
            "Outcome::{variant} is missing from the snapshot; the set of ways a \
             run can end is what a caller matches exhaustively on"
        );
    }
}
