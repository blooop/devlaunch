//! The two files core's public-API snapshot splits into, and the invariant that
//! keeps them worth reading.
//!
//! `public-api.api.txt` is the promise as a path match can see it: every row is
//! a declaration written *at* `devlaunch_core::api`, the surface an external
//! consumer is entitled to depend on, so a diff there is a change to that
//! contract. `public-api.rest.txt` is the tripwire: the binary surface,
//! reachable but never promised, regenerated freely whenever a refactor moves
//! it.
//!
//! One snapshot for both tiers is what the split replaced, and the reason is
//! the signal: a diff that is nine hundred rows of internal churn and one
//! removed `api` function reads as routine, and the one row that mattered goes
//! through review unremarked.
//!
//! The tests below hold the partition, which is not the same as holding the
//! promise. `cargo public-api` renders methods and impls only at a type's
//! canonical path, so `api::Launch::{new, run}` and every derived impl on a
//! promised type are in the rest file — 42 of the 79 rows the generator emits
//! for the `api` section — and renaming `Launch::run` diffs neither of these
//! two files in the place a reader would look. Deliberately not asserted here:
//! <https://github.com/blooop/devlaunch/issues/352> widens the classifier, and
//! its red is that rename, so a test pinning today's classification of those
//! rows would have to be deleted to let the fix land.
//!
//! The classification lives in `scripts/public-api-snapshots.sh` and nowhere
//! else — the CI job runs that script rather than re-implementing its filter.
//! These tests are the other half of that: they hold the *checked-in* files to
//! the rule, so a hand-edited snapshot, or a regeneration that put the promise
//! in the file nobody reads, fails here rather than passing quietly.

const PROMISE: &str = include_str!("../public-api.api.txt");
const REST: &str = include_str!("../public-api.rest.txt");

/// The rows of a snapshot: `cargo public-api` writes one declaration per line
/// and nothing else, so a blank line is noise from an editor rather than API.
fn rows(snapshot: &str) -> Vec<&str> {
    snapshot
        .lines()
        .map(str::trim_end)
        .filter(|row| !row.is_empty())
        .collect()
}

/// Whether a row declares something under `devlaunch_core::api`.
///
/// The path has to end there or continue with `::` — the same boundary the
/// regeneration script's `devlaunch_core::api\b` means, spelled out because a
/// future `devlaunch_core::apiary` must not be mistaken for the promise.
fn names_api(row: &str) -> bool {
    const PATH: &str = "devlaunch_core::api";
    row.match_indices(PATH).any(|(at, _)| {
        !row[at + PATH.len()..]
            .chars()
            .next()
            .is_some_and(|next| next.is_alphanumeric() || next == '_')
    })
}

#[test]
fn every_promised_row_is_an_api_declaration() {
    let strays: Vec<&str> = rows(PROMISE)
        .into_iter()
        .filter(|row| !names_api(row))
        .collect();
    assert!(
        strays.is_empty(),
        "public-api.api.txt is the frozen promise and holds only \
         devlaunch_core::api declarations; these rows are something else: {strays:#?}"
    );
}

#[test]
fn no_row_of_the_rest_is_an_api_declaration() {
    let promised: Vec<&str> = rows(REST)
        .into_iter()
        .filter(|row| names_api(row))
        .collect();
    assert!(
        promised.is_empty(),
        "these devlaunch_core::api declarations are in the freely regenerated \
         file, where a breaking change to them would read as routine churn: {promised:#?}"
    );
}

#[test]
fn the_two_files_share_no_row() {
    let promise = rows(PROMISE);
    let duplicated: Vec<&str> = rows(REST)
        .into_iter()
        .filter(|row| promise.contains(row))
        .collect();
    assert!(
        duplicated.is_empty(),
        "the split is a partition: a row belongs to exactly one file, and these \
         are in both: {duplicated:#?}"
    );
}

#[test]
fn each_file_is_anchored_on_a_row_every_generation_produces() {
    // Two rows `cargo public-api -p devlaunch-core` cannot omit while the crate
    // exists and still declares `pub mod api`. Their absence means a truncated
    // or misdirected snapshot -- an empty file passes every filter test above.
    assert!(
        rows(PROMISE).contains(&"pub mod devlaunch_core::api"),
        "the promise file does not contain the api module's own row"
    );
    assert!(
        rows(REST).contains(&"pub mod devlaunch_core"),
        "the rest file does not contain the crate root's row"
    );
}
