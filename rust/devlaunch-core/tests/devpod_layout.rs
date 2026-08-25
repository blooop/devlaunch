//! devpod's on-disk layout, spelled in one module and nowhere else.
//!
//! `clients::devpod` is the seam for devpod-the-*command*; `clients::devpod_home`
//! is the seam for devpod-the-*filesystem*. Everything under
//! `<devpod home>/contexts/<context>/workspaces/<id>/` is that second module's
//! knowledge: which directory a record is in, what the result beside it is called,
//! and how the contexts are walked to find either.
//!
//! This is a guard rather than a behaviour test, because the failure it catches is
//! not a wrong answer — it is a second copy of the layout that agrees with the
//! first until devpod changes it. Before this test there were eight copies inside
//! `devlaunch-core`: three in `flows::lifecycle`'s implementation, one in
//! `flows::provision::verdict_cache`, and four more in test fixtures, each
//! reproducing the convention literally. `flows/lifecycle.rs` carried a comment
//! claiming the layout was "spelled out in one place and not three" while it was
//! spelled in four.
//!
//! Scoped to `devlaunch-core/src`. `dl`'s end-to-end tests spell devpod's paths on
//! purpose: they stand outside the crate and check that what dl wrote landed where
//! devpod will look for it, which is exactly the assertion that must not be routed
//! through the code under test.

use std::path::{Path, PathBuf};

/// The module allowed to name devpod's record layout.
const ADAPTER: &str = "clients/devpod_home.rs";

/// How the layout gets spelled: a string literal whose first component is
/// `contexts`, either on its own (`.join("contexts")`) or as the head of a joined
/// path (`"contexts/default/workspaces/myws/workspace_result.json"`).
///
/// Prose is not a spelling — a doc comment describing the layout costs nothing
/// when devpod moves it, so neither pattern matches unquoted text. Nor does
/// devpod's `config.yaml`, whose top-level `contexts:` key is a different fact
/// about a different file.
const SPELLINGS: [&str; 2] = ["\"contexts\"", "\"contexts/"];

fn crate_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `src`, relative to it, sorted.
fn sources(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found.sort();
    found
}

fn walk(root: &Path, dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a readable source directory") {
        let path = entry.expect("a readable directory entry").path();
        if path.is_dir() {
            walk(root, &path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(
                path.strip_prefix(root)
                    .expect("a path under the source root")
                    .to_path_buf(),
            );
        }
    }
}

/// Which files spell the layout, and on which lines.
fn spellings_in(root: &Path) -> Vec<(PathBuf, usize)> {
    let mut found = Vec::new();
    for relative in sources(root) {
        let text = std::fs::read_to_string(root.join(&relative)).expect("a readable source file");
        for (index, line) in text.lines().enumerate() {
            if SPELLINGS.iter().any(|spelling| line.contains(spelling)) {
                found.push((relative.clone(), index + 1));
            }
        }
    }
    found
}

#[test]
fn only_the_devpod_home_adapter_spells_devpods_on_disk_layout() {
    let root = crate_src();
    let strays: Vec<String> = spellings_in(&root)
        .into_iter()
        .filter(|(file, _)| file != Path::new(ADAPTER))
        .map(|(file, line)| format!("  src/{}:{line}", file.display()))
        .collect();
    assert!(
        strays.is_empty(),
        "devpod's on-disk layout is spelled outside {ADAPTER}:\n{}",
        strays.join("\n")
    );
}

/// The other half, so the guard cannot pass by the layout having been spelled
/// nowhere at all — which is how a guard like this quietly stops guarding.
#[test]
fn the_devpod_home_adapter_does_spell_it() {
    let root = crate_src();
    let inside = spellings_in(&root)
        .into_iter()
        .filter(|(file, _)| file == Path::new(ADAPTER))
        .count();
    assert!(
        inside > 0,
        "no module spells devpod's on-disk layout, so this guard is guarding nothing"
    );
}
