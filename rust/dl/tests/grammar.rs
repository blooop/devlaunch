//! Grammar refusals that guard against a destructive misparse, at the binary
//! boundary.
//!
//! Every expectation was captured by running `python -m devlaunch.dl` — the frozen
//! Python build — against `tests/scenario.py`'s world with
//! `test/fixtures/devpod_shim.py` on PATH as `devpod`, under a scratch
//! `HOME`/`XDG_*`/`DEVPOD_HOME`, and pasting what it printed. Nothing here was read
//! off the Rust implementation.
//!
//! Two divergences the port had regressed, both landing on `exit 0` where Python
//! refused:
//!
//! - `--force` before the verb (`dl <ws> --force rm`, `dl --force <ws> rm`). clap
//!   accepts `--force` in any position and strips it; Python read it positionally
//!   (`"--force" in args[2:]`, dl.py:4726), so a `--force` in the workspace or verb
//!   slot was an unknown name and the delete never happened. The port deleted.
//! - a path spec normalising to an empty leaf (`dl /`, `//`, `/.`): the derived id
//!   was empty, handed to devpod as `--id ""`, and run to a reported success. The
//!   empty derived id is now refused (its wording is the port's, since Python
//!   reached exit 1 through devpod rather than through a refusal).

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

fn scratch_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("dlt")
        .rand_bytes(6)
        .tempdir_in("/tmp")
        .expect("a scratch directory under /tmp")
}

struct World {
    root: PathBuf,
    _scratch: tempfile::TempDir,
}

impl World {
    fn full() -> Self {
        let scratch = scratch_dir();
        let root = scratch.path().to_path_buf();
        let built = Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scenario.py"))
            .arg(&root)
            .arg(repo_root().join("test/fixtures/devpod_shim.py"))
            .output()
            .expect("python3 is installed");
        assert!(
            built.status.success(),
            "scenario.py failed: {}",
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

    fn clone_is_there(&self) -> bool {
        self.root
            .join("cache/devlaunch/repos/blooop/devlaunch/blooop-devlaunch-main-4f3a2b1c")
            .is_dir()
    }
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

#[test]
fn force_before_the_verb_is_refused_and_deletes_nothing() {
    let world = World::full();
    let run = world.dl(&["blooop/devlaunch@main", "--force", "rm"]);

    assert_eq!(run.code, Some(1), "stderr: {}", run.err);
    assert_eq!(run.out, "");
    // Python reads `--force` in the verb slot as an unknown command.
    assert_eq!(
        run.err,
        "Unknown command '--force'. Use 'dl blooop/devlaunch@main -- --force' to run a shell \
         command.\n"
    );
    assert!(
        world.clone_is_there(),
        "the clone must not have been deleted"
    );
}

#[test]
fn force_before_the_workspace_is_refused_and_deletes_nothing() {
    let world = World::full();
    let run = world.dl(&["--force", "blooop/devlaunch@main", "rm"]);

    assert_eq!(run.code, Some(1), "stderr: {}", run.err);
    assert_eq!(run.out, "");
    // Python reads `--force` in the workspace slot as an unknown workspace.
    assert_eq!(
        run.err,
        "Unknown workspace '--force'. Use 'dl --ls' to list workspaces, or specify owner/repo or \
         ./path\n"
    );
    assert!(
        world.clone_is_there(),
        "the clone must not have been deleted"
    );
}

#[test]
fn force_after_the_verb_still_deletes() {
    // The valid position, unchanged: `--force` in args[2:] is the force it always
    // was, so the guard above cannot have cost the flag its one real use.
    let world = World::full();
    let run = world.dl(&["blooop/devlaunch@main", "rm", "--force"]);

    assert_eq!(run.code, Some(0), "stderr: {}", run.err);
    assert_eq!(
        run.out,
        "Successfully deleted workspace blooop-devlaunch-main-4f3a2b1c\n"
    );
    assert!(!world.clone_is_there(), "the clone was deleted");
}

/// The refusals `docs/cli.md` quotes, held to the ones the binary prints.
///
/// The "Where `--force` may sit" section writes both into a code block, cut at the
/// end of the first sentence. That is a hand-maintained copy of a string owned by
/// `render.rs` and `lib.rs`, and this repository allows a second copy only with a
/// test beside it that diffs it against the first. The section names the tests that
/// pin the *behaviour*, which is not the same promise: those would still pass with
/// the message reworded and the page left quoting the old one.
///
/// Matched as a prefix, because the block drops the `Use '...'` suggestion that
/// follows on purpose. It is quoting the refusal, not transcribing the session.
#[test]
fn the_force_placement_section_quotes_the_refusals_it_says_it_does() {
    let doc = std::fs::read_to_string(repo_root().join("docs/cli.md")).expect("docs/cli.md");
    let section = force_placement_section(&doc);

    for (spelling, args) in [
        (
            "dl <ws> --force rm",
            ["blooop/devlaunch@main", "--force", "rm"],
        ),
        (
            "dl --force <ws> rm",
            ["--force", "blooop/devlaunch@main", "rm"],
        ),
    ] {
        let quoted = quoted_refusal(&section, spelling);
        let world = World::full();
        let run = world.dl(&args);

        assert_eq!(run.code, Some(1), "stderr: {}", run.err);
        assert!(
            run.err.starts_with(&quoted),
            "docs/cli.md says `{spelling}` prints {quoted:?}; it prints {:?}",
            run.err
        );
        assert!(
            world.clone_is_there(),
            "`{spelling}` must not have deleted the clone"
        );
    }
}

/// The text under the `--force` placement heading, up to the next heading.
///
/// Matched on the heading rather than on a phrase under it, so the prose stays free
/// to be rewritten while this test keeps pointing at one span. A missing heading
/// says so rather than yielding an empty section that every assertion passes over.
fn force_placement_section(document: &str) -> String {
    const HEADING: &str = "### Where `--force` may sit";
    let start = document
        .find(HEADING)
        .unwrap_or_else(|| panic!("docs/cli.md no longer has a '{HEADING}' section"));
    let rest = &document[start + HEADING.len()..];
    let end = rest
        .find(
            "
## ",
        )
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// The refusal `docs/cli.md` writes for one spelling: what follows the `->` arrow,
/// up to the run of spaces before the `(exit 1, ...)` annotation.
fn quoted_refusal(section: &str, spelling: &str) -> String {
    let line = section
        .lines()
        .find(|line| line.starts_with(spelling))
        .unwrap_or_else(|| panic!("docs/cli.md no longer shows `{spelling}`"));
    let (_, printed) = line
        .split_once("->")
        .unwrap_or_else(|| panic!("`{spelling}` in docs/cli.md no longer names what it prints"));
    printed
        .trim_start()
        .split("  ")
        .next()
        .expect("split always yields one part")
        .trim_end()
        .to_string()
}

#[test]
fn a_path_that_names_no_workspace_is_refused() {
    let world = World::full();
    let run = world.dl(&["/"]);

    assert_eq!(run.code, Some(1), "stderr: {}", run.err);
    assert_eq!(run.out, "");
    assert_eq!(
        run.err,
        "'/' does not name a workspace: its path has no final component to name one after.\n"
    );
}

#[test]
fn a_non_utf8_argument_is_refused_cleanly_and_never_panics() {
    // Row 4 forbids a traceback. `std::env::args()` panics on an argument that is
    // not valid UTF-8 — the entry points now decode with `args_os` +
    // `to_string_lossy`, so a `\xff` argument reaches the grammar as the
    // replacement character rather than aborting the process with a traceback
    // (exit 101). What the command then does with the decoded word is an ordinary
    // ending; the guarantee row 4 makes is only that it is one, not a panic.
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let output = Command::new(env!("CARGO_BIN_EXE_dl"))
        .arg(OsStr::from_bytes(b"\xffowner/repo"))
        .env_clear()
        .keeping_coverage()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .output()
        .expect("the dl binary runs");
    let code = output.status.code();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_ne!(code, None, "killed by a signal: {stderr}");
    assert_ne!(
        code,
        Some(101),
        "a Rust panic reached the process exit: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "a traceback that row 4 forbids: {stderr}"
    );
}
