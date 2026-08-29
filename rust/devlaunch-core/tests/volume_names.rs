//! Where a docker volume name can come from, and where it can be removed.
//!
//! devlaunch#325's rule is that **no volume name is ever synthesized from a
//! pattern**: every name handed to `docker volume rm` originates in a substitution
//! devpod performed and wrote down. devlaunch#451 asked for a test that no removal
//! is issued for a name from anywhere else, and asked for it *by construction* —
//! the absence of a path rather than a guard firing at runtime.
//!
//! That is what this is. Two spellings and one call site:
//!
//! - The two name **templates** — `<basename>-pixi` and
//!   `dind-var-lib-docker-<devcontainerId>` — are written in `flows::kept_copies`
//!   and nowhere else, so a name can only be built by
//!   `Substitutions::volume_names`, and a `Substitutions` can only be built by
//!   parsing bytes devpod wrote or devlaunch copied. There is no constructor for an
//!   inferred name to reach.
//! - `clients::docker::remove_volumes` is *called* from exactly one place,
//!   `flows::lifecycle::sweep_volumes`, so there is one door and it takes the names
//!   those two reads produce.
//!
//! Neither half is a behaviour test; both are guards, in `tests/devpod_layout.rs`'s
//! shape and for its reason. The failure they catch is not a wrong answer — it is a
//! second way to build a name, which agrees with the first until somebody points it
//! at `docker volume ls`.

use std::path::{Path, PathBuf};

/// The module allowed to spell a volume name's template.
const NAMER: &str = "flows/kept_copies.rs";

/// How a volume name gets built: the format templates themselves, with their
/// interpolation braces, so the whole names a *test fixture* writes out
/// (`"repo-main-ab12-pixi"`) do not match. A fixture asserting what devlaunch asked
/// docker for is the assertion, and routing it through the code under test is what
/// this guard is for elsewhere.
const TEMPLATES: [&str; 2] = ["{basename}-pixi", "dind-var-lib-docker-{"];

/// The one function that can remove a volume, and the one module that may call it.
const REMOVAL: &str = "remove_volumes(";
const DOOR: &str = "flows/lifecycle/delete.rs";
const DOORWAY: &str = "clients/docker.rs";

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

/// Which files hold `needles`, and on which lines.
fn mentions(root: &Path, needles: &[&str]) -> Vec<(PathBuf, usize)> {
    let mut found = Vec::new();
    for relative in sources(root) {
        let text = std::fs::read_to_string(root.join(&relative)).expect("a readable source file");
        for (index, line) in text.lines().enumerate() {
            if needles.iter().any(|needle| line.contains(needle)) {
                found.push((relative.clone(), index + 1));
            }
        }
    }
    found
}

#[test]
fn only_one_module_can_build_a_volume_name() {
    let root = crate_src();
    let strays: Vec<String> = mentions(&root, &TEMPLATES)
        .into_iter()
        .filter(|(file, _)| file != Path::new(NAMER))
        .map(|(file, line)| format!("  src/{}:{line}", file.display()))
        .collect();
    assert!(
        strays.is_empty(),
        "a volume name is built outside {NAMER}, so there is now more than one way to \
         make one:\n{}",
        strays.join("\n")
    );
}

/// The other half, so the guard cannot pass by the templates having moved out of
/// the crate entirely — which is how a guard like this quietly stops guarding.
#[test]
fn that_module_does_build_them() {
    let root = crate_src();
    let inside = mentions(&root, &TEMPLATES)
        .into_iter()
        .filter(|(file, _)| file == Path::new(NAMER))
        .count();
    assert!(
        inside > 0,
        "no module builds a volume name, so this guard is guarding nothing"
    );
}

/// One door to `docker volume rm`, so every name that reaches docker went through
/// the one function that turns docker's answer into a verdict.
///
/// Both reads — devpod's live create result at delete time, and devlaunch's kept
/// copy at prune time — arrive at that same door, which is what makes "no name from
/// anywhere else" checkable at all.
#[test]
fn only_one_module_can_remove_a_volume() {
    let root = crate_src();
    let strays: Vec<String> = mentions(&root, &[REMOVAL])
        .into_iter()
        .filter(|(file, _)| file != Path::new(DOOR) && file != Path::new(DOORWAY))
        .map(|(file, line)| format!("  src/{}:{line}", file.display()))
        .collect();
    assert!(
        strays.is_empty(),
        "`docker volume rm` is reachable outside {DOOR}:\n{}",
        strays.join("\n")
    );
}
