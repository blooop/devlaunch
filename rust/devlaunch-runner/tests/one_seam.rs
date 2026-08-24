//! `ProcessRunner` is the only [`Runner`] outside test code, asserted rather than
//! asked for.
//!
//! The trait's doc used to say it was "implemented once for real processes and
//! once for tests", and by the time anyone read that sentence there were nine
//! impls. Rewriting it as a count would rot the same way, so the doc now states a
//! shape — one production impl, one shared fake, wrappers over those two — and
//! this test holds the half of that shape which is load-bearing. A second
//! implementation doing real work is not a doc that has gone stale: it is a second
//! way for devlaunch to start a process, and the seam has stopped being one.
//!
//! The scan is deliberately dumb — an `impl ... for` line in a source file,
//! classified by where it sits — because the alternative is a hand-written list of
//! the wrappers, which is the artefact that rotted in the first place. Everything
//! it cannot see for itself is [`TEST_ONLY_FILES`], where dropping a file is a
//! visible edit rather than an omission.
//!
//! [`Runner`]: devlaunch_runner::Runner

use std::fs;
use std::path::{Path, PathBuf};

/// Files whose every impl is test code but which do not say so in the file: the
/// module is declared `#[cfg(test)] mod ...` somewhere else. One entry today, and
/// a new one arrives as a failure here rather than as silence.
const TEST_ONLY_FILES: &[&str] = &["devlaunch-core/src/testing.rs"];

/// The one implementation that may do real work.
const PRODUCTION: &str = "ProcessRunner";

/// What an implementation of the seam opens with, once the line is trimmed.
const OPENER: &str = "impl Runner for ";

/// The cargo workspace root: this crate's directory, one level up.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the cargo workspace above this crate")
        .to_path_buf()
}

fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|why| panic!("reading {}: {why}", dir.display()));
    for entry in entries {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|kind| kind == "rs") {
            found.push(path);
        }
    }
}

/// The type named by an implementation line, without its lifetimes or generics.
fn implementer(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix(OPENER)?;
    let end = rest.find(['<', '{', ' ']).unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// Whether the file opens a top-level `#[cfg(test)]` module inline at `at`.
///
/// The attribute alone is not the signal: `devlaunch-runner/src/lib.rs` carries
/// `#[cfg(test)] mod tests;` near the top, and the production impl is four hundred
/// lines below it. An *inline* module is the one that swallows what follows.
fn opens_a_test_module(lines: &[&str], at: usize) -> bool {
    if lines[at] != "#[cfg(test)]" {
        return false;
    }
    lines
        .get(at + 1)
        .is_some_and(|next| next.contains("mod ") && next.ends_with('{'))
}

/// Whether the implementation at `at` is test code.
///
/// Three ways to be: living in a test-support crate, living under a `tests/`
/// directory, or sitting below the `#[cfg(test)] mod tests {` its file ends with —
/// which is where every wrapper a unit test defines sits.
fn is_test_code(relative: &str, lines: &[&str], at: usize) -> bool {
    // Read from the workspace-relative path, not the absolute one: whatever
    // directories this checkout happens to live under are not evidence about the
    // code.
    let test_tree = relative
        .split('/')
        .any(|part| part.ends_with("-test-support") || part == "tests");
    test_tree
        || TEST_ONLY_FILES.contains(&relative)
        || (0..at).any(|line| opens_a_test_module(lines, line))
}

/// Every implementation of the seam in the workspace: relative path, type, and
/// whether it is test code.
fn implementations() -> Vec<(String, String, bool)> {
    let root = workspace();
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    sources.sort();

    let mut found = Vec::new();
    for path in sources {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|why| panic!("reading {}: {why}", path.display()));
        let lines: Vec<&str> = text.lines().collect();
        let relative = path
            .strip_prefix(&root)
            .expect("a path under the workspace")
            .to_string_lossy()
            .into_owned();
        for (at, line) in lines.iter().enumerate() {
            if let Some(name) = implementer(line) {
                let test = is_test_code(&relative, &lines, at);
                found.push((relative.clone(), name.to_string(), test));
            }
        }
    }
    found
}

#[test]
fn the_scan_finds_the_implementations_it_is_meant_to_judge() {
    let found = implementations();
    assert!(
        found
            .iter()
            .any(|(path, name, _)| path.contains("devlaunch-runner") && name == PRODUCTION),
        "the scan did not find {PRODUCTION}, so it is not reading this workspace: \
         {found:#?}"
    );
    assert!(
        found.iter().filter(|(_, _, test)| *test).count() >= 2,
        "the scan found no test implementations, which no version of this tree has \
         been true of: {found:#?}"
    );
}

#[test]
fn only_process_runner_does_real_work() {
    let production: Vec<_> = implementations()
        .into_iter()
        .filter(|(_, _, test)| !*test)
        .map(|(path, name, _)| format!("{name} ({path})"))
        .collect();
    assert_eq!(
        production.len(),
        1,
        "Runner is the one seam onto the OS, and {PRODUCTION} is meant to be the \
         only implementation of it outside test code. Found: {production:#?}. A \
         second real implementation is a second way to start a process; if that is \
         deliberate, the trait's doc in src/lib.rs says otherwise and needs the \
         edit first."
    );
    assert!(
        production[0].starts_with(PRODUCTION),
        "the one production Runner is no longer {PRODUCTION}: {production:#?}"
    );
}
