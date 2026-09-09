//! The file name `docs/workspace-tools.md` publishes for an interrupted pass, held
//! against the path the cache actually writes.
//!
//! Beside `lending_contract` and `zellij_contract` for their reason, and reading the
//! same section splitter. What it guards is narrower than either: the page tells a
//! reader that a killed launch leaves `<workspace>.pass` beside the marker, and a
//! reader who believes that goes looking for the file -- to see whether a workspace
//! is stuck, or to clear one by hand. A page naming a file that is not there is
//! worse than a page that names none.
//!
//! Two facts, because the sentence makes two claims: what the file is called, and
//! that it sits beside the marker rather than somewhere of its own. Both are read
//! out of [`VerdictCache`] rather than out of a constant, so a rename that moves the
//! path moves the assertion with it.
//!
//! Everything else in the section is explanation and gets no assertions, for the
//! reason `lending_contract` gives about the trip-by-trip narrative.

use super::lending_contract::{CONTRACT_DOC, contract_doc, section};
use super::verdict_cache::VerdictCache;

/// The section carrying what this file guards, matched on its heading for
/// `lending_contract`'s reason.
const HEADING: &str = "### A pass that was interrupted";

/// The two paths this workspace's records live at, as the cache spells them.
fn recorded(workspace_id: &str) -> (String, String) {
    let dir = tempfile::tempdir().expect("a scratch cache directory");
    let verdicts = VerdictCache::under(dir.path(), None);
    let named = |path: std::path::PathBuf| {
        path.strip_prefix(dir.path())
            .expect("a path under the scratch cache")
            .display()
            .to_string()
    };
    (
        named(verdicts.in_flight(workspace_id)),
        named(verdicts.marker(workspace_id)),
    )
}

#[test]
fn the_page_names_the_file_a_killed_launch_leaves() {
    let (in_flight, _) = recorded("myws");
    let extension = in_flight
        .rsplit_once('.')
        .expect("the record carries an extension")
        .1
        .to_owned();

    assert!(
        section(&contract_doc(), HEADING).contains(&format!("<workspace>.{extension}")),
        "{CONTRACT_DOC} no longer names the record an interrupted pass leaves, which is \
         `<workspace>.{extension}`; a reader told to look for it would find nothing"
    );
}

#[test]
fn the_record_really_is_beside_the_marker() {
    // "beside the marker" is the page's own word for where to look, and it is the
    // half a reader uses to find the directory at all: the marker's own path is
    // published one subsection up, and this sentence hangs off it rather than
    // repeating it.
    let (in_flight, marker) = recorded("myws");
    let directory = |path: &str| {
        path.rsplit_once('/')
            .map(|(dir, _)| dir.to_owned())
            .unwrap_or_default()
    };

    assert_eq!(
        directory(&in_flight),
        directory(&marker),
        "{CONTRACT_DOC} says the record sits beside the marker and it no longer does"
    );
    assert!(
        section(&contract_doc(), HEADING).contains("beside the marker"),
        "{CONTRACT_DOC} no longer says where the record is, so nothing tells a reader \
         which directory to look in"
    );
}
