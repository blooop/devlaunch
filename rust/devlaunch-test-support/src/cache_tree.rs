//! A stable picture of the cache a `dl` run left behind — the whole tree, not one path.
//!
//! # What this is for
//!
//! A boundary test that names the paths it expects can only fail on those paths.
//! `assert!(!world.exists("cache/devlaunch"))` says the purge removed the cache; it
//! says nothing about a stale record, an orphaned clone directory, a lock file
//! nobody released, or a `metadata.json` a migration corrupted somewhere else in
//! the tree. Those are the on-disk defects a stdout comparison cannot see and the
//! ones that cost someone their workspace list.
//!
//! Until #267 the port's `compare.py --fingerprint` covered them, by running the
//! frozen Python build and the Rust build in identical worlds and diffing the whole
//! cache afterwards. That check was a *comparison*, so it retired with the second
//! implementation. This is the same walk, re-aimed at the two questions one
//! implementation can still answer:
//!
//! - **Did anything move that should not have?** [`cache_fingerprint`] before and
//!   after a command that must change nothing. There is no golden in that: the
//!   assertion is that the two listings are equal, so it cannot rot and cannot be
//!   updated into vacuity.
//! - **Is the shape after a mutation the shape we meant?** [`cache_shape`] against
//!   a listing in the test. That one *is* read off today's binary, deliberately and
//!   unlike every other golden in these suites — there is no Python left to capture
//!   it from. What it catches is a **change**, which is what the parity cases caught
//!   too (`parity_cases.txt`: "the regression tripwire"). Every line of it should be
//!   explainable at review; a line nobody can explain is the finding.
//!
//! # What is left out, and why
//!
//! - **`.git` and `.bare` are recorded by presence and not descended into.** A
//!   clone's object store shards by commit SHA, the scenario builders stamp each
//!   world's commits with the wall clock, and so two builds of the same world
//!   produce different SHAs and different sharding. That is nondeterminism, not a
//!   difference worth failing on, and what the on-disk question is actually about
//!   lives in the tree around those stores.
//! - **Volatile files are dropped** — see [`VOLATILE`]. The detached refresh child
//!   races every one of them.
//! - **`last_fetched` is blanked inside `metadata.json`** before its contents are
//!   hashed, for the same reason: it is the timestamp of a fetch, not a fact about
//!   the records, and the refresh child rewrites it if it wins the race before the
//!   run exits.

use std::path::Path;

use sha2::{Digest, Sha256};

/// Files the detached refresh child races or that carry a per-run nonce, so their
/// presence and contents are never part of a picture two runs must agree on.
///
/// Matched against the file name alone, `*` allowed at either end and nowhere else
/// — the patterns the port used, kept literal rather than pulling in a glob crate
/// for four of them.
pub const VOLATILE: [&str; 4] = ["completions.*", "*.lock", "*.tmp", "last_fetched"];

/// The key the refresh child stamps a wall clock into, blanked before hashing.
const VOLATILE_JSON_KEY: &str = "last_fetched";

/// Every path under `<root>/cache`, sorted, with nothing machine-specific in it.
///
/// Directories carry a trailing `/`; symlinks are recorded as such without being
/// followed; `.git` and `.bare` are recorded by presence. The paths are relative to
/// `root`, so two scratch worlds in two different temporary directories compare
/// equal.
///
/// This is the view to assert a mutation's result against, because a line of it is
/// a path a reader can check — see the module docs on what that golden does and
/// does not promise.
pub fn cache_shape(root: &Path) -> Vec<String> {
    walk(root, Contents::Ignored)
}

/// [`cache_shape`], with each file's contents hashed into its line.
///
/// The hash is of the file's bytes with `root` replaced by `{ROOT}`, so it too is
/// the same in any scratch directory, and `metadata.json` goes through
/// [`without_the_fetch_timestamp`] first.
///
/// This is the view for a before-and-after comparison — a command that must leave
/// the cache alone, answered `no` or refused. It catches the rewrite that leaves
/// the tree the same shape, which is most of them.
pub fn cache_fingerprint(root: &Path) -> Vec<String> {
    walk(root, Contents::Hashed)
}

/// Whether a file's contents are part of the picture.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Contents {
    Ignored,
    Hashed,
}

fn walk(root: &Path, contents: Contents) -> Vec<String> {
    let cache = root.join("cache");
    if !cache.exists() {
        // A distinct line rather than an empty listing: "the cache is gone" and
        // "the cache is there and empty" are different results, and a `--purge`
        // test asserts the first.
        return vec!["(no cache)".to_owned()];
    }
    let mut entries = Vec::new();
    descend(&cache, root, contents, &mut entries);
    entries.sort();
    entries
}

fn descend(directory: &Path, root: &Path, contents: Contents, entries: &mut Vec<String>) {
    entries.push(format!("{}/", relative(directory, root)));
    let mut children: Vec<_> = match std::fs::read_dir(directory) {
        Ok(children) => children.filter_map(Result::ok).collect(),
        // Unreadable is a fact about the tree worth recording, not a panic: a
        // partial-removal test makes exactly this by taking a directory's mode
        // away, and the listing should say so rather than fail the harness.
        Err(error) => {
            entries.push(format!(
                "{}/ (unreadable: {})",
                relative(directory, root),
                error.kind()
            ));
            return;
        }
    };
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let name = child.file_name().to_string_lossy().into_owned();
        // Checked before the file type, because a symlink to a directory must be
        // recorded as a symlink rather than descended into.
        if path.is_symlink() {
            entries.push(format!("{} -> symlink", relative(&path, root)));
        } else if path.is_dir() {
            if name == ".git" || name == ".bare" {
                entries.push(format!(
                    "{}/ (git store, contents omitted)",
                    relative(&path, root)
                ));
            } else {
                descend(&path, root, contents, entries);
            }
        } else if !is_volatile(&name) {
            entries.push(match contents {
                Contents::Ignored => relative(&path, root),
                Contents::Hashed => {
                    format!("{} {}", relative(&path, root), hash(&path, root, &name))
                }
            });
        }
    }
}

/// `path` under `root`, in `/`-joined form so a listing reads the same anywhere.
fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// The first 16 hex of the file's SHA-256, with the scratch root templated out.
///
/// Short because it is only ever compared with another one taken the same way, in
/// the same process, minutes apart: what a reader needs from it is "these differ",
/// and 64 characters of that is 48 characters of noise in a failure message.
fn hash(path: &Path, root: &Path, name: &str) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        // Same reasoning as the unreadable directory above.
        return "(unreadable)".to_owned();
    };
    let templated = replace(&bytes, root.to_string_lossy().as_bytes(), b"{ROOT}");
    let blob = if name == "metadata.json" {
        without_the_fetch_timestamp(&templated)
    } else {
        templated
    };
    let digest = Sha256::digest(&blob);
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `metadata.json` with every `last_fetched` blanked, so the refresh child's clock
/// is not mistaken for a change to the records.
///
/// Everything else in the document — the records, their `local_path`, the
/// worktrees, a corruption a bad migration left — is exactly what this picture is
/// for and is kept. A file that will not parse is returned as it stands: the
/// malformed-ness is itself state worth comparing, and a fixture that corrupts it
/// on purpose is a test that means to.
fn without_the_fetch_timestamp(blob: &[u8]) -> Vec<u8> {
    let Ok(mut document) = serde_json::from_slice::<serde_json::Value>(blob) else {
        return blob.to_vec();
    };
    scrub(&mut document);
    serde_json::to_vec(&document).unwrap_or_else(|_| blob.to_vec())
}

fn scrub(node: &mut serde_json::Value) {
    match node {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields.iter_mut() {
                if key == VOLATILE_JSON_KEY {
                    *value = serde_json::Value::Null;
                } else {
                    scrub(value);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(scrub),
        _ => {}
    }
}

/// `needle` -> `replacement` throughout `haystack`.
fn replace(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut rest = haystack;
    while let Some(at) = rest
        .windows(needle.len())
        .position(|window| window == needle)
    {
        out.extend_from_slice(&rest[..at]);
        out.extend_from_slice(replacement);
        rest = &rest[at + needle.len()..];
    }
    out.extend_from_slice(rest);
    out
}

/// Whether `name` matches any of [`VOLATILE`].
fn is_volatile(name: &str) -> bool {
    VOLATILE.iter().any(|pattern| match pattern.as_bytes() {
        [b'*', ..] => name.ends_with(&pattern[1..]),
        [.., b'*'] => name.starts_with(&pattern[..pattern.len() - 1]),
        _ => name == *pattern,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One world, built by hand, so each rule above is visible in one listing.
    fn world() -> tempfile::TempDir {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let root = scratch.path();
        let cache = root.join("cache/devlaunch");
        let clone = cache.join("repos/owner/repo/repo-main");
        std::fs::create_dir_all(clone.join(".git/objects/ab")).expect("a clone");
        // The names the implementation uses: `repo_manager.rs`'s `BARE_DIR_NAME`
        // is `.bare`, a sibling of the clones. `almost.bare` beside it is not a
        // git store and has to be descended into -- the rule is the exact name,
        // and a directory whose name merely ends that way is an ordinary one.
        std::fs::create_dir_all(cache.join("repos/owner/repo/.bare/objects")).expect("a bare");
        std::fs::create_dir_all(cache.join("repos/owner/repo/almost.bare")).expect("a lookalike");
        std::fs::write(cache.join("repos/owner/repo/almost.bare/kept"), "")
            .expect("in the lookalike");
        std::fs::write(clone.join("README.md"), "hello\n").expect("a file in the clone");
        std::fs::write(clone.join(".git/objects/ab/cdef"), "an object").expect("an object");
        std::fs::write(cache.join("metadata.json"), r#"{"version": 3}"#).expect("metadata");
        std::fs::write(cache.join("metadata.json.lock"), "").expect("a lock");
        std::fs::write(cache.join("completions.json"), "[]").expect("a completions cache");
        std::fs::write(cache.join("last_fetched"), "1970").expect("a sidecar");
        std::os::unix::fs::symlink(cache.join("metadata.json"), cache.join("current"))
            .expect("a symlink");
        scratch
    }

    #[test]
    fn the_shape_is_every_directory_and_file_and_nothing_volatile() {
        let scratch = world();

        assert_eq!(
            cache_shape(scratch.path()),
            [
                "cache/",
                "cache/devlaunch/",
                "cache/devlaunch/current -> symlink",
                "cache/devlaunch/metadata.json",
                "cache/devlaunch/repos/",
                "cache/devlaunch/repos/owner/",
                "cache/devlaunch/repos/owner/repo/",
                "cache/devlaunch/repos/owner/repo/.bare/ (git store, contents omitted)",
                "cache/devlaunch/repos/owner/repo/almost.bare/",
                "cache/devlaunch/repos/owner/repo/almost.bare/kept",
                "cache/devlaunch/repos/owner/repo/repo-main/",
                "cache/devlaunch/repos/owner/repo/repo-main/.git/ (git store, contents omitted)",
                "cache/devlaunch/repos/owner/repo/repo-main/README.md",
            ],
            "the shape names something volatile, or descended into a git store"
        );
    }

    #[test]
    fn a_file_that_changed_changes_its_line_and_a_volatile_one_does_not() {
        let scratch = world();
        let cache = scratch.path().join("cache/devlaunch");
        let before = cache_fingerprint(scratch.path());

        // A volatile file may be rewritten freely.
        std::fs::write(cache.join("last_fetched"), "2026").expect("the sidecar moves");
        std::fs::write(cache.join("completions.json"), r#"["ws"]"#).expect("the cache moves");
        assert_eq!(
            cache_fingerprint(scratch.path()),
            before,
            "a volatile file reached the fingerprint"
        );

        // A record may not.
        std::fs::write(
            cache.join("metadata.json"),
            r#"{"version": 3, "worktrees": {}}"#,
        )
        .expect("metadata moves");
        assert_ne!(
            cache_fingerprint(scratch.path()),
            before,
            "a rewritten metadata.json left the fingerprint unchanged"
        );
    }

    /// The refresh child's timestamp is not a change to the records.
    ///
    /// Nested rather than top-level, because that is where it lives in the real
    /// document — per repository, under the record it belongs to.
    #[test]
    fn a_fetch_timestamp_moving_inside_metadata_is_not_a_change() {
        let scratch = world();
        let metadata = scratch.path().join("cache/devlaunch/metadata.json");
        std::fs::write(
            &metadata,
            r#"{"repos": {"owner/repo": {"last_fetched": "1970-01-01T00:00:00", "branch": "main"}}}"#,
        )
        .expect("metadata");
        let before = cache_fingerprint(scratch.path());

        std::fs::write(
            &metadata,
            r#"{"repos": {"owner/repo": {"last_fetched": "2026-08-20T12:00:00", "branch": "main"}}}"#,
        )
        .expect("metadata, refetched");
        assert_eq!(
            cache_fingerprint(scratch.path()),
            before,
            "the fetch timestamp is being read as a change to the records"
        );

        std::fs::write(
            &metadata,
            r#"{"repos": {"owner/repo": {"last_fetched": "2026-08-20T12:00:00", "branch": "next"}}}"#,
        )
        .expect("metadata, re-pointed");
        assert_ne!(
            cache_fingerprint(scratch.path()),
            before,
            "a branch that moved beside the timestamp was blanked with it"
        );
    }

    /// A purge's result: not an empty listing, a said-so one.
    #[test]
    fn a_cache_that_is_gone_says_that_rather_than_listing_nothing() {
        let scratch = tempfile::tempdir().expect("a scratch directory");

        assert_eq!(cache_shape(scratch.path()), ["(no cache)"]);
        assert_eq!(cache_fingerprint(scratch.path()), ["(no cache)"]);

        std::fs::create_dir(scratch.path().join("cache")).expect("an empty cache");
        assert_eq!(
            cache_shape(scratch.path()),
            ["cache/"],
            "an empty cache and an absent one are the same listing"
        );
    }

    /// The scratch root is templated out, so the same world in two directories
    /// fingerprints the same — which is what makes a before-and-after honest even
    /// when a file records where it lives.
    #[test]
    fn two_worlds_in_two_directories_fingerprint_alike() {
        let (one, two) = (world(), world());
        for scratch in [&one, &two] {
            let path = scratch.path().join("cache/devlaunch/metadata.json");
            let content = format!(r#"{{"local_path": "{}"}}"#, scratch.path().display());
            std::fs::write(path, content).expect("metadata naming its own root");
        }

        assert_eq!(
            cache_fingerprint(one.path()),
            cache_fingerprint(two.path()),
            "a path inside a file is reaching the fingerprint untemplated"
        );
    }

    #[test]
    fn the_volatile_patterns_match_at_the_end_they_are_written_at() {
        for volatile in [
            "completions.json",
            "completions.bash",
            "metadata.json.lock",
            "repo.lock",
            "scratch.tmp",
            "last_fetched",
        ] {
            assert!(is_volatile(volatile), "{volatile} should be volatile");
        }
        for kept in [
            "metadata.json",
            "config.toml",
            "completions",
            "lock",
            "last_fetched.json",
        ] {
            assert!(!is_volatile(kept), "{kept} should not be volatile");
        }
    }
}
