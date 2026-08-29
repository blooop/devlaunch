//! The two files core's public-API snapshot splits into, and the invariant that
//! keeps them worth reading.
//!
//! `public-api.api.txt` is the promise: every row declares something
//! `devlaunch_core::api` re-exports, the surface an external consumer is
//! entitled to depend on, so a diff there is a change to that contract.
//! `public-api.rest.txt` is the tripwire: the binary surface,
//! reachable but never promised, regenerated freely whenever a refactor moves
//! it.
//!
//! One snapshot for both tiers is what the split replaced, and the reason is
//! the signal: a diff that is nine hundred rows of internal churn and one
//! removed `api` function reads as routine, and the one row that mattered goes
//! through review unremarked.
//!
//! The promise file holds behaviour and not only declarations, which took a
//! second rule to achieve (#352). `cargo public-api` renders inherent methods
//! and trait impls at a type's *canonical* path only, never at the path it is
//! re-exported under, so `api::Launch::{new, run}` are rendered
//! `flows::launch::Launch::{new, run}` and a path match on `api` cannot see
//! them. The classifier resolves each re-export back to the path it names and
//! claims that item's rows too, which is why `public-api.api.txt` is full of
//! `flows::` and `domain::` paths: they are the promise, not strays.
//!
//! The classification lives in `scripts/public-api-snapshots.sh` and nowhere
//! else — the CI job runs that script rather than re-implementing its filter,
//! and so does the fixed-point test below rather than restating the rule in
//! Rust, where the restatement is what would be under test. These tests are the
//! other half of that: they hold the *checked-in* files to the rule, so a
//! hand-edited snapshot, or a regeneration that put the promise in the file
//! nobody reads, fails here rather than passing quietly.

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

/// One side of the split, as the regeneration script itself decides it.
///
/// Shelling out rather than reimplementing: the rule is now two clauses over a
/// path list read out of `src/lib.rs`, and a copy of it here would be a second
/// definition of what is promised -- the exact thing the script exists to stop
/// there being. Needs no nightly toolchain and generates nothing; `--classify`
/// is a filter over rows on stdin.
fn classified(kind: &str, corpus: &str) -> String {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/public-api-snapshots.sh");
    let input = std::env::temp_dir().join(format!(
        "devlaunch-public-api-{kind}-{}.txt",
        std::process::id()
    ));
    std::fs::write(&input, corpus).expect("staging the corpus for the classifier");
    let done = std::process::Command::new(&script)
        .args(["--classify", kind])
        .stdin(std::fs::File::open(&input).expect("reopening the staged corpus"))
        .output()
        .expect("running the regeneration script's classifier");
    let _ = std::fs::remove_file(&input);
    assert!(
        done.status.success(),
        "{} --classify {kind} failed: {}",
        script.display(),
        String::from_utf8_lossy(&done.stderr)
    );
    String::from_utf8(done.stdout).expect("the classifier's output is the rows it was given")
}

#[test]
fn the_checked_in_split_is_the_one_the_classifier_makes() {
    // Feed the classifier everything the two files hold and it must hand back
    // the same two files: each row on the side it is already on, in the order
    // it is already in. That is a stronger statement than "every promised row
    // names the api path", which stopped being true when the classifier learned
    // to claim canonical paths -- and it covers what that one covered, since a
    // hand-added row on the wrong side changes which list it comes back in.
    let corpus = format!("{PROMISE}{REST}");
    assert_eq!(
        rows(&classified("api", &corpus)),
        rows(PROMISE),
        "public-api.api.txt is not what the classifier would put there; a row \
         was hand-edited in, or the two were regenerated by different rules"
    );
    assert_eq!(
        rows(&classified("rest", &corpus)),
        rows(REST),
        "public-api.rest.txt holds a row the classifier calls promised, where a \
         breaking change to it would read as routine churn"
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

/// Whether some row of a snapshot declares `Type::method`, wherever
/// `cargo public-api` chose to render it and whatever generics it carried.
///
/// A row is `pub fn <path>(<args>) -> <ret>`, so the declaration is everything
/// before the first `(`; matching against that and not the whole row is what
/// keeps an argument of type `Launch` from reading as a method on one.
fn declares(snapshot: &str, ty: &str, method: &str) -> bool {
    let owner = format!("::{ty}");
    let called = format!("::{method}");
    rows(snapshot).into_iter().any(|row| {
        let Some(rest) = row.strip_prefix("pub fn ") else {
            return false;
        };
        let path = rest.split('(').next().unwrap_or_default();
        path.ends_with(&called)
            && path
                .match_indices(&owner)
                .any(|(at, _)| matches!(path[at + owner.len()..].chars().next(), Some(':' | '<')))
    })
}

#[test]
fn the_promise_file_carries_the_promised_types_behaviour() {
    // Every one of these is reachable only through the promised tier, and
    // renaming any of them breaks a caller that has `api` and nothing else.
    let missing: Vec<String> = [
        ("Launch", "new"),
        ("Launch", "run"),
        ("CommandContext", "new"),
        ("DevcontainerPath", "as_str"),
    ]
    .into_iter()
    .filter(|(ty, method)| !declares(PROMISE, ty, method))
    .map(|(ty, method)| format!("{ty}::{method}"))
    .collect();
    assert!(
        missing.is_empty(),
        "public-api.api.txt is the frozen promise and does not declare {missing:?}; \
         renaming one of those is an unambiguous break that leaves the file \
         byte-identical"
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
