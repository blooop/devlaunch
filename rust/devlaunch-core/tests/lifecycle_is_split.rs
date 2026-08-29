//! `flows::lifecycle` is a family of modules, and its root is a table of
//! contents.
//!
//! Five unrelated commands lived in one 9,000-line file — stop, delete, purge,
//! prune and reconcile, plus the refresh latch, the fetch sweep and the on-disk
//! placement rules that serve the launch path rather than any of them. The file
//! carried thirteen banner-comment dividers marking exactly where the modules
//! were, and none of them had ever been made into one (devlaunch#314).
//!
//! This is a guard rather than a behaviour test, because the failure it catches
//! is not a wrong answer. Nothing breaks the day a new lifecycle flow is written
//! into whichever file is open; it costs a reader a little more each time, and
//! the file that has to be split is the one nobody can afford to touch. What is
//! asserted here is the shape that makes that impossible to do quietly:
//!
//! 1. The family is more than one module.
//! 2. The **root defines nothing**. It carries the family's documentation, the
//!    module declarations and the re-exports that keep every
//!    `flows::lifecycle::Thing` path where callers already have it — and no
//!    item of its own. That is what closes the road back: a lifecycle that is
//!    one file again has to be one file *here*, and here is the one place a
//!    body cannot go.
//! 3. No member is more than a third of the family. Rule 2 alone is satisfied by
//!    twelve stubs beside one module holding everything, which is the same file
//!    under a longer name.
//!
//! Scoped to this one family on purpose. `flows::launch` and `flows::provision`
//! carry banners of their own and are not split yet; a guard that failed on them
//! today would have to be born with an exemption list, and an exemption list is
//! how a guard stops meaning anything.

use std::path::{Path, PathBuf};

/// The module root, relative to `src`.
const ROOT: &str = "flows/lifecycle.rs";

/// The directory its members live in, relative to `src`.
const FAMILY: &str = "flows/lifecycle";

/// The share of the family one module may hold before the split has stopped
/// paying. A third rather than a half: half is "bigger than everything else put
/// together", which is already past the point where a reader can guess which
/// file something is in.
const LARGEST_SHARE: f64 = 1.0 / 3.0;

fn crate_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// The family's production modules, relative to `src`, sorted.
///
/// `tests.rs` is not one of them. It is the suite over the whole family and it
/// is bigger than the family is, so counting it would make every share
/// meaningless and rule 3 unfailable.
fn members(src: &Path) -> Vec<PathBuf> {
    let dir = src.join(FAMILY);
    let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|why| panic!("{FAMILY} should be a readable directory: {why}"))
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter(|path| path.file_name().is_some_and(|name| name != "tests.rs"))
        .map(|path| {
            path.strip_prefix(src)
                .expect("a path under the source root")
                .to_path_buf()
        })
        .collect();
    found.sort();
    found
}

fn line_count(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_else(|why| panic!("{} should be readable: {why}", path.display()))
        .lines()
        .count()
}

/// Whether a line of the root is a declaration rather than a definition.
///
/// Everything a table of contents is made of: its own documentation, ordinary
/// comments, attributes on a module declaration, imports, module declarations
/// and re-exports. A `mod name { .. }` with a body is not one of them, which is
/// why the `mod` arm insists on the semicolon.
fn is_declaration(line: &str) -> bool {
    let line = line.trim();
    line.is_empty()
        || line.starts_with("//")
        || line.starts_with("#[")
        || line.starts_with("use ")
        || line.starts_with("pub use ")
        || line.starts_with("pub(crate) use ")
        || ((line.starts_with("mod ")
            || line.starts_with("pub mod ")
            || line.starts_with("pub(crate) mod "))
            && line.ends_with(';'))
        // The continuation lines of a wrapped `use` or `pub use`, which rustfmt
        // writes one name per line inside braces.
        || line == "{"
        || line == "};"
        || line.ends_with(',')
        || line.ends_with("::{")
}

#[test]
fn the_lifecycle_family_is_more_than_one_module() {
    let members = members(&crate_src());
    assert!(
        members.len() > 1,
        "src/{FAMILY} holds {} production module(s): the family is one file again",
        members.len()
    );
}

#[test]
fn the_lifecycle_root_declares_and_defines_nothing() {
    let src = crate_src();
    let path = src.join(ROOT);
    let text = std::fs::read_to_string(&path).expect("the lifecycle root should be readable");
    let definitions: Vec<String> = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !is_declaration(line))
        .map(|(index, line)| format!("  src/{ROOT}:{}: {}", index + 1, line.trim()))
        .collect();
    assert!(
        definitions.is_empty(),
        "src/{ROOT} is the family's table of contents and defines nothing itself, \
         but {} line(s) are neither documentation, an import, a module declaration \
         nor a re-export:\n{}",
        definitions.len(),
        definitions.join("\n")
    );
}

#[test]
fn no_lifecycle_module_holds_a_third_of_the_family() {
    let src = crate_src();
    let members = members(&src);
    let sizes: Vec<(PathBuf, usize)> = members
        .into_iter()
        .map(|relative| {
            let lines = line_count(&src.join(&relative));
            (relative, lines)
        })
        .collect();
    let family: usize = sizes.iter().map(|(_, lines)| lines).sum();
    let ceiling = (family as f64 * LARGEST_SHARE) as usize;
    let overgrown: Vec<String> = sizes
        .iter()
        .filter(|(_, lines)| *lines > ceiling)
        .map(|(relative, lines)| format!("  src/{}: {lines} lines", relative.display()))
        .collect();
    assert!(
        overgrown.is_empty(),
        "the lifecycle family is {family} lines over {} modules, so no one of them \
         should be past {ceiling}:\n{}",
        sizes.len(),
        overgrown.join("\n")
    );
}
