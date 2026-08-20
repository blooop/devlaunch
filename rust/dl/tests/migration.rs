//! The id-scheme cache migration, judged at the binary boundary.
//!
//! Every expectation in this file was captured by running `python -m devlaunch.dl`
//! — the frozen Python build — against `tests/lifecycle_scenario.py --v1-cache`'s
//! v1 world (the same fixture the `rust-parity` compare step's v1 case builds)
//! with `test/fixtures/devpod_shim.py` on PATH as `devpod`, under a scratch
//! `HOME`/`XDG_CACHE_HOME`/`XDG_CONFIG_HOME`/`DEVPOD_HOME`, and pasting what it
//! printed. Nothing here was read off the Rust implementation.
//!
//! The read-side and lifecycle worlds are both schema 2, so this is the one place
//! the migration's stderr notices — Python's `migration.py::_announce`, the only
//! pointer a user gets to `dl --reconcile`/`recreate` for the containers the
//! rename orphans — are pinned against the binary. `--ls --json` is the command
//! chosen because it builds the clone manager (and so runs the migration) in both
//! builds, and its stdout is a deterministic empty array while the notices are the
//! whole of stderr.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use devlaunch_test_support::KeepingCoverage;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// A scratch directory whose path is always the same *length*, so a golden
/// captured under one `/tmp/dltXXXXXX` lines up under another. The golden-capture
/// harness makes its root the same way (`mktemp -d /tmp/dltXXXXXX`).
fn scratch_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("dlt")
        .rand_bytes(6)
        .tempdir_in("/tmp")
        .expect("a scratch directory under /tmp")
}

/// One v1 cache, and the `dl` runs against it.
struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    fn v1() -> Self {
        let scratch = scratch_dir();
        let root = scratch.path().to_path_buf();
        let built = Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lifecycle_scenario.py"))
            .arg(&root)
            .arg(repo_root().join("test/fixtures/devpod_shim.py"))
            .arg("--v1-cache")
            .output()
            .expect("python3 is installed");
        assert!(
            built.status.success(),
            "lifecycle_scenario.py --v1-cache failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        World {
            root,
            _scratch: scratch,
        }
    }

    fn dl(&self, args: &[&str]) -> Run {
        let root = self.root.display().to_string();
        let output = Command::new(env!("CARGO_BIN_EXE_dl"))
            .args(args)
            .env_clear()
            .keeping_coverage()
            .env("PATH", format!("{root}/bin:/usr/bin:/bin"))
            .env("HOME", format!("{root}/home"))
            .env("XDG_CACHE_HOME", format!("{root}/cache"))
            .env("XDG_CONFIG_HOME", format!("{root}/config"))
            .env("DEVPOD_HOME", format!("{root}/devpod"))
            .env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"))
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env(
                "DEVLAUNCH_COMPLETION_FILE",
                format!("{root}/home/completions.sh"),
            )
            .output()
            .expect("the dl binary runs");
        Run::of(&output, &self.root)
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.root.join(relative)).unwrap_or_default()
    }

    fn leaves(&self, relative: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.root.join(relative))
            .expect("a listing")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

/// Every path the cache holds, so a migration can be judged by what it moved
/// rather than by the one directory this test happens to list.
///
/// See `devlaunch_test_support::cache_shape`. This is the case the port's
/// harness-honesty review named as its untested blind spot -- a v1 cache the first
/// command migrates in place -- and the `--fingerprint` half of it retired with the
/// implementation it compared against.
fn cache_shape(root: &Path) -> Vec<String> {
    devlaunch_test_support::cache_shape(root)
}

struct Run {
    out: String,
    err: String,
    code: Option<i32>,
}

impl Run {
    fn of(output: &Output, root: &Path) -> Self {
        let template = |bytes: &[u8]| {
            String::from_utf8_lossy(bytes).replace(&root.display().to_string(), "{ROOT}")
        };
        Run {
            out: template(&output.stdout),
            err: template(&output.stderr),
            code: output.status.code(),
        }
    }
}

/// The two lines Python's migration wrote to stderr, verbatim. `main` is the old
/// flattened-branch leaf; `devlaunch-main-zovomobo` is the id this build derives;
/// the orphaned-container line is the only place a user is told about
/// `dl --reconcile` and `dl <workspace> recreate`.
const MIGRATION_NOTICES: &str = "\
dl: migrated 1 workspace clone directory to the new id scheme (e.g. main -> devlaunch-main-zovomobo)
dl: 1 devpod container(s) still carry the old workspace ids and are now orphaned; dl --reconcile re-points them at the renamed clones, and dl <workspace> recreate finishes each repair -- that restores the clone association and the workspace, not state that lived only inside the old container, and only until the branch is launched again (a fresh launch claims the clone, and reconcile never re-points a clone a live container holds). dl deletes nothing for you; for the ones you are finished with: xargs -r -n1 devpod delete < {ROOT}/cache/devlaunch/orphaned-workspaces.txt
";

#[test]
fn the_migration_notices_are_the_ones_python_printed() {
    let world = World::v1();
    let before = cache_shape(&world.root);
    let run = world.dl(&["--ls", "--json"]);

    assert_eq!(run.code, Some(0), "stderr: {}", run.err);
    // The listing itself is empty: devpod knows no workspaces, and the migration
    // reads no devpod. Stdout is the whole of the read side; the notices are the
    // whole of stderr.
    assert_eq!(run.out, "[]\n");
    assert_eq!(run.err, MIGRATION_NOTICES);

    // The migration did the work the notices describe: the old-scheme clone is
    // renamed onto the derived id, and the orphaned old container id is listed,
    // one per line, for the cleanup command the notice names.
    assert_eq!(
        world.leaves("cache/devlaunch/repos/blooop/devlaunch"),
        vec![".bare".to_owned(), "devlaunch-main-zovomobo".to_owned()],
    );
    assert_eq!(
        world.read("cache/devlaunch/orphaned-workspaces.txt"),
        "devlaunch-main\n",
    );

    // And the rest of the cache, which the two assertions above do not reach: a
    // migration that renamed the right directory and left a v1 remnant somewhere
    // else -- a half-moved clone, a sidecar under the old name -- passes both of
    // them. Stated as the difference, in both directions, so an addition cannot
    // hide behind a removal.
    //
    // Read the two lists together and they are mirror images below the leaf name,
    // which is the whole claim: on disk this migration is a *rename* of one clone,
    // plus the one file the second notice tells the user to feed to
    // `devpod delete`. Nothing was copied and nothing was rebuilt.
    //
    // It says nothing about `metadata.json`, which appears in neither list because
    // it exists on both sides -- and whose *contents* the migration does rewrite,
    // since rewriting them is what a schema migration is. This is `cache_shape`
    // and not `cache_fingerprint` for exactly that reason: the records are pinned
    // by the notices above and by `flows/migration.rs`, and a hash of them here
    // would be a golden nobody could read.
    let after = cache_shape(&world.root);
    assert_eq!(
        only_in(&before, &after),
        [
            "cache/devlaunch/repos/blooop/devlaunch/main/",
            "cache/devlaunch/repos/blooop/devlaunch/main/.git/ (git store, contents omitted)",
            "cache/devlaunch/repos/blooop/devlaunch/main/README.md",
        ],
        "the migration moved a directory it did not report, or left the old one"
    );
    assert_eq!(
        only_in(&after, &before),
        [
            "cache/devlaunch/orphaned-workspaces.txt",
            "cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-zovomobo/",
            "cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-zovomobo/.git/ (git store, contents omitted)",
            "cache/devlaunch/repos/blooop/devlaunch/devlaunch-main-zovomobo/README.md",
        ],
        "the migration wrote something the notices do not mention"
    );
}

/// The entries `listing` has and `other` does not, in `listing`'s order. The twin
/// of `lifecycle.rs`'s, kept here because these two files share no harness.
fn only_in(listing: &[String], other: &[String]) -> Vec<String> {
    listing
        .iter()
        .filter(|entry| !other.contains(entry))
        .cloned()
        .collect()
}
