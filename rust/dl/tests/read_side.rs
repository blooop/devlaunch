//! The read-side commands, judged at the binary boundary.
//!
//! Every expectation in this file was captured by running
//! `python -m devlaunch.dl` — the frozen Python build — against
//! `tests/scenario.py`'s world with `test/fixtures/devpod_shim.py` on PATH as
//! `devpod`, under a scratch `HOME`/`XDG_CACHE_HOME`/`XDG_CONFIG_HOME`/
//! `DEVPOD_HOME`, and pasting what it printed. Nothing here was read off the Rust
//! implementation.
//!
//! The two surfaces pinned byte for byte are the ones the plan grades A: the
//! `--ls --json` document (`wf` parses it) and the exit codes. The `--size`
//! numbers are not pinned, because what a clone costs on disk depends on whether
//! `git clone` could hardlink its objects, which is a property of the filesystem
//! and not of dl.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The repository root, from the crate this test is compiled into.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root")
}

/// One scratch world, and the `dl` runs against it.
struct World {
    root: PathBuf,
    /// Kept so the directory outlives every run in the test.
    _scratch: tempfile::TempDir,
}

impl World {
    /// The full scenario: five workspaces, a metadata record, real clones.
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

    /// The same world with no workspaces in it: the shim's state emptied.
    fn empty() -> Self {
        let world = Self::full();
        std::fs::write(
            world.root.join("shim-state.json"),
            r#"{"workspaces": {}, "providers": {}}"#,
        )
        .expect("an empty state file");
        world
    }

    /// A world with a `devpod` that behaves as `script` says, or none at all.
    fn with_devpod(script: Option<&str>) -> Self {
        let scratch = scratch_dir();
        let root = scratch.path().to_path_buf();
        for directory in ["bin", "home", "cache", "config", "devpod"] {
            std::fs::create_dir_all(root.join(directory)).expect("a scratch directory");
        }
        if let Some(script) = script {
            let devpod = root.join("bin/devpod");
            std::fs::write(&devpod, script).expect("a fake devpod");
            make_executable(&devpod);
        }
        World {
            root,
            _scratch: scratch,
        }
    }

    fn dl(&self, args: &[&str]) -> Run {
        self.dl_with(args, &[])
    }

    fn dl_with(&self, args: &[&str], extra: &[(&str, &str)]) -> Run {
        let root = self.root.display().to_string();
        let mut command = Command::new(env!("CARGO_BIN_EXE_dl"));
        command
            .args(args)
            .env_clear()
            // /usr/bin and /bin for git, which the listing really runs; the fake
            // devpod is first, under its real name.
            .env("PATH", format!("{root}/bin:/usr/bin:/bin"))
            .env("HOME", format!("{root}/home"))
            .env("XDG_CACHE_HOME", format!("{root}/cache"))
            .env("XDG_CONFIG_HOME", format!("{root}/config"))
            .env("DEVPOD_HOME", format!("{root}/devpod"))
            .env("DEVPOD_SHIM_STATE", format!("{root}/shim-state.json"))
            // No network in a test: `git ls-remote` over ssh fails at once, so the
            // branch half of the completion cache comes off the local bare clone
            // and the answer is the same on every machine.
            .env("GIT_SSH_COMMAND", "false")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env(
                "DEVLAUNCH_COMPLETION_FILE",
                format!("{root}/home/completions.sh"),
            );
        for (name, value) in extra {
            command.env(name, value);
        }
        let output = command.output().expect("the dl binary runs");
        Run::of(&output, &self.root)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path(relative)).unwrap_or_default()
    }
}

/// A scratch directory whose path is always the same *length*.
///
/// The table's column widths are measured from the strings in it, and the widest
/// of those is a clone path — so a golden captured under one scratch root only
/// lines up under another of the same length. `/tmp/dltXXXXXX` is what the
/// golden-capture harness makes too (`mktemp -d /tmp/dltXXXXXX`), which is the
/// whole reason the directory is not simply `tempfile::tempdir()`.
fn scratch_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("dlt")
        .rand_bytes(6)
        .tempdir_in("/tmp")
        .expect("a scratch directory under /tmp")
}

/// One table line's cells.
///
/// Cells are separated by at least two spaces (the gap, plus whatever padding the
/// column needed), and no cell holds two spaces in a row — which is what makes
/// this the whole of the parsing.
fn columns(line: &str) -> Vec<&str> {
    line.split("  ")
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .collect()
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("an executable fake");
}

/// What one run printed and how it ended, with the scratch root templated out so
/// the expectations are the same on every machine.
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

    fn succeeded(&self) -> &Self {
        assert_eq!(self.code, Some(0), "expected exit 0; stderr: {}", self.err);
        self
    }

    fn exited(&self, code: i32) -> &Self {
        assert_eq!(
            self.code,
            Some(code),
            "expected exit {code}; stdout: {}; stderr: {}",
            self.out,
            self.err
        );
        self
    }

    fn lines(&self) -> Vec<&str> {
        self.out.lines().collect()
    }
}

// ===========================================================================
// --ls, the table
// ===========================================================================

/// Python's `dl --ls` for the scenario, verbatim.
const LS_TABLE: &str = "\
WORKSPACE                       TYPE     SOURCE                                                                                LAST USED
-------------------------------------------------------------------------------------------------------------------------------------------------------
blooop-devlaunch-main-4f3a2b1c  local    {ROOT}/cache/devlaunch/repos/blooop/devlaunch/blooop-devlaunch-main-4f3a2b1c  2026-08-01 10:11:12
blooop-other-feature-9e8d7c6b   local    {ROOT}/cache/devlaunch/repos/blooop/other/blooop-other-feature-9e8d7c6b       2026-07-30 09:08:07
someones-project                local    {ROOT}/foreign/proj                                                           never
devpod-upstream                 git      https://github.com/loft-sh/devpod.git                                                 2026-08-01 10:11:12
an-image-workspace              unknown  {\"image\": \"ubuntu:22.04\"}                                                             2026-07-30 09:08:07
";

#[test]
fn the_table_is_the_one_python_printed() {
    let world = World::full();
    let run = world.dl(&["--ls"]);
    run.succeeded();
    assert_eq!(run.out, LS_TABLE);
    assert_eq!(run.err, "");
}

#[test]
fn a_machine_with_no_workspaces_gets_one_sentence() {
    let world = World::empty();
    let run = world.dl(&["--ls"]);
    run.succeeded();
    assert_eq!(run.out, "No workspaces found.\n");
    // The same sentence with `--size`: there is no table to put a column in.
    let sized = world.dl(&["--ls", "--size"]);
    sized.succeeded();
    assert_eq!(sized.out, "No workspaces found.\n");
}

#[test]
fn the_size_column_holds_a_size_for_dls_own_clones_and_a_dash_for_everyone_elses() {
    // The sizes themselves are not pinned: what a clone costs depends on whether
    // `git clone` could hardlink its objects. The shape is.
    let world = World::full();
    let run = world.dl(&["--ls", "--size"]);
    run.succeeded();
    let lines = run.lines();
    assert!(
        lines[0].ends_with("  SIZE  LAST USED"),
        "expected a SIZE column: {:?}",
        lines[0]
    );
    let sizes: Vec<&str> = lines[2..].iter().map(|line| columns(line)[3]).collect();
    // Rows one and two are dl's own clones; the last three are not.
    assert!(
        sizes[0].ends_with("KiB") || sizes[0].ends_with("MiB"),
        "expected a measured size, got {:?}",
        sizes[0]
    );
    assert_eq!(
        &sizes[2..],
        ["-", "-", "-"],
        "a workspace dl did not make is not measured"
    );
}

// ===========================================================================
// --ls --json, the wire format
// ===========================================================================

/// Python's `dl --ls --json` for the scenario, verbatim. Grade A: `wf` parses it.
const LS_JSON: &str = r#"[
  {
    "id": "blooop-devlaunch-main-4f3a2b1c",
    "devlaunch": true,
    "repo": "blooop/devlaunch",
    "branch": "main",
    "checkedOut": "main",
    "path": "{ROOT}/cache/devlaunch/repos/blooop/devlaunch/blooop-devlaunch-main-4f3a2b1c",
    "state": "Running",
    "lastUsed": "2026-08-01T10:11:12+0000",
    "unsaved": {
      "nothingToLose": true
    }
  },
  {
    "id": "blooop-other-feature-9e8d7c6b",
    "devlaunch": true,
    "repo": null,
    "branch": null,
    "checkedOut": "feature",
    "path": "{ROOT}/cache/devlaunch/repos/blooop/other/blooop-other-feature-9e8d7c6b",
    "state": "Stopped",
    "lastUsed": "2026-07-30T09:08:07+0000",
    "unsaved": {
      "wouldLose": "1 uncommitted change(s) (scratch.txt)"
    }
  },
  {
    "id": "someones-project",
    "devlaunch": false,
    "repo": null,
    "branch": null,
    "checkedOut": null,
    "path": null,
    "state": "Stopped",
    "lastUsed": "",
    "unsaved": null
  },
  {
    "id": "devpod-upstream",
    "devlaunch": false,
    "repo": null,
    "branch": null,
    "checkedOut": null,
    "path": null,
    "state": "Running",
    "lastUsed": "2026-08-01T10:11:12+0000",
    "unsaved": null
  },
  {
    "id": "an-image-workspace",
    "devlaunch": false,
    "repo": null,
    "branch": null,
    "checkedOut": null,
    "path": null,
    "state": "Stopped",
    "lastUsed": "2026-07-30T09:08:07+0000",
    "unsaved": null
  }
]
"#;

#[test]
fn the_json_document_is_the_one_python_printed() {
    let world = World::full();
    let run = world.dl(&["--ls", "--json"]);
    run.succeeded();
    assert_eq!(run.out, LS_JSON);
}

#[test]
fn an_empty_listing_is_an_empty_json_array() {
    let world = World::empty();
    let run = world.dl(&["--ls", "--json"]);
    run.succeeded();
    assert_eq!(run.out, "[]\n");
}

#[test]
fn the_disk_field_appears_only_when_asked_and_is_null_where_nothing_is_dls() {
    let world = World::full();
    let run = world.dl(&["--ls", "--json", "--size"]);
    run.succeeded();
    // The document Python printed, with the measured byte counts stood down: they
    // are the one part of it that is a property of the filesystem.
    let normalized = normalize_bytes(&run.out);
    let expected = normalize_bytes(&LS_JSON.replace(
        "      \"nothingToLose\": true\n    }\n  }",
        "      \"nothingToLose\": true\n    },\n    \"disk\": {\n      \"exclusiveBytes\": 0\n    }\n  }",
    ));
    let expected = expected.replace(
        "      \"wouldLose\": \"1 uncommitted change(s) (scratch.txt)\"\n    }\n  }",
        "      \"wouldLose\": \"1 uncommitted change(s) (scratch.txt)\"\n    },\n    \"disk\": {\n      \"exclusiveBytes\": 0\n    }\n  }",
    );
    let expected = expected.replace(
        "    \"unsaved\": null\n  }",
        "    \"unsaved\": null,\n    \"disk\": null\n  }",
    );
    assert_eq!(normalized, expected);
}

/// Every measured byte count as a zero, so a document can be compared without its
/// filesystem-dependent half.
fn normalize_bytes(document: &str) -> String {
    document
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("\"exclusiveBytes\"") {
                "      \"exclusiveBytes\": 0".to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

// ===========================================================================
// the completion commands
// ===========================================================================

/// The sentence a workspace whose source dl cannot read gets, on stderr.
const UNREADABLE_SOURCE: &str = "Not looking for a repo in workspace 'an-image-workspace': \
     devpod describes its source as {\"image\": \"ubuntu:22.04\"}, which devlaunch cannot read.\n";

#[test]
fn the_repos_are_the_ones_discovered_from_the_workspaces_devpod_lists() {
    // No completion cache in a fresh world, so this is the ask-devpod path — and
    // the answer is only what the *workspaces* say, not what the cache directory
    // holds, which is `--refresh`'s wider question.
    let world = World::full();
    let run = world.dl(&["--repos"]);
    run.succeeded();
    assert_eq!(run.out, "loft-sh/devpod\n");
    assert_eq!(run.err, UNREADABLE_SOURCE);
}

#[test]
fn a_completion_cache_missing_the_repos_key_reads_as_no_repos() {
    // Divergence row 11. Python's `"repos" in cache` check fell through to asking
    // devpod, so the same file made it print `loft-sh/devpod`; here a boundary
    // document is read typed, and a document with no repos in it has no repos.
    // The distinguishing input is a `completions.json` dl did not write.
    let world = World::full();
    std::fs::write(
        world.path("cache/devlaunch/completions.json"),
        r#"{"workspaces": ["x"]}"#,
    )
    .expect("a cache file");
    let run = world.dl(&["--repos"]);
    run.succeeded();
    assert_eq!(run.out, "");
    assert_eq!(run.err, "");
}

#[test]
fn a_cached_listing_of_repos_is_printed_one_per_line() {
    let world = World::full();
    std::fs::write(
        world.path("cache/devlaunch/completions.json"),
        r#"{"workspaces": [], "repos": ["a/b", "c/d"], "owners": ["a", "c"], "branches": []}"#,
    )
    .expect("a cache file");
    let run = world.dl(&["--repos"]);
    run.succeeded();
    assert_eq!(run.out, "a/b\nc/d\n");
}

#[test]
fn the_completion_data_is_one_json_line_python_would_have_written() {
    let world = World::full();
    let run = world.dl(&["--completion-data"]);
    run.succeeded();
    assert_eq!(
        run.out,
        "{\"workspaces\": [\"blooop-devlaunch-main-4f3a2b1c\", \
         \"blooop-other-feature-9e8d7c6b\", \"someones-project\", \"devpod-upstream\", \
         \"an-image-workspace\"], \"repos\": [\"blooop/devlaunch\", \"loft-sh/devpod\"], \
         \"owners\": [\"blooop\", \"loft-sh\"], \"branches\": [\"blooop/devlaunch@main\"]}\n"
    );
    assert_eq!(run.err, UNREADABLE_SOURCE);
}

#[test]
fn refresh_says_what_it_found() {
    let world = World::full();
    let run = world.dl(&["--refresh"]);
    run.succeeded();
    assert_eq!(
        run.out,
        "Refreshing completion cache...\nCache updated: 5 workspaces found\n"
    );
    assert_eq!(run.err, UNREADABLE_SOURCE);
    // And it wrote both caches, which is what makes the next keystroke fast.
    assert!(
        world
            .read("cache/devlaunch/completions.json")
            .contains("blooop/devlaunch"),
        "the JSON cache was not written"
    );
    assert!(
        world
            .read("cache/devlaunch/completions.bash")
            .contains("DL_REPOS="),
        "the shell cache was not written"
    );
}

#[test]
fn a_refresh_that_cannot_reach_devpod_still_completes_what_it_can_see() {
    // Python catches the unreadable listing here and nowhere else: the workspace
    // names are one of four things being collected, and the other three come off
    // the local disk. Refusing would stop `--install` installing completions at
    // all.
    let world = World::with_devpod(Some("#!/bin/sh\necho boom >&2\nexit 1\n"));
    let run = world.dl(&["--refresh"]);
    run.succeeded();
    assert_eq!(
        run.out,
        "Refreshing completion cache...\nCache updated: 0 workspaces found\n"
    );
    assert!(
        run.err
            .starts_with("Completing without workspace names: `devpod list` exited 1:"),
        "expected the refusal to be named, got {:?}",
        run.err
    );
}

// ===========================================================================
// --version
// ===========================================================================

#[test]
fn version_prints_the_binarys_own_version_and_asks_devpod_nothing() {
    // No devpod at all: `--version` must not need one. Python's version line
    // carries the install's provenance for an editable install; a compiled binary
    // has none to carry.
    let world = World::with_devpod(None);
    let run = world.dl(&["--version"]);
    run.succeeded();
    assert_eq!(run.out, format!("dl {}\n", env!("CARGO_PKG_VERSION")));
    assert_eq!(run.err, "");
}

// ===========================================================================
// the exit codes
// ===========================================================================

const DEVPOD_MISSING: &str = "devpod not found on PATH: dl cannot manage workspaces without it. \
     Install devpod from https://devpod.sh/docs/getting-started/install (pixi/conda installs of \
     devlaunch include it; pip installs do not).\n";

#[test]
fn a_missing_devpod_is_exit_127_on_every_command_that_needs_one() {
    let world = World::with_devpod(None);
    for args in [
        vec!["--ls"],
        vec!["--ls", "--json"],
        vec!["--repos"],
        vec!["--refresh"],
        vec!["--completion-data"],
        vec!["--install"],
    ] {
        let run = world.dl(&args);
        run.exited(127);
        assert!(
            run.err.contains(DEVPOD_MISSING.trim()),
            "dl {args:?} said {:?}",
            run.err
        );
    }
}

#[test]
fn a_devpod_that_refuses_the_listing_is_exit_1_and_one_line() {
    let world = World::with_devpod(Some(
        "#!/bin/sh\necho \"context not found: default\" >&2\nexit 1\n",
    ));
    for args in [vec!["--ls"], vec!["--ls", "--json"], vec!["--repos"]] {
        let run = world.dl(&args);
        run.exited(1);
        assert_eq!(
            run.err, "error: `devpod list` exited 1: 'context not found: default'\n",
            "dl {args:?}"
        );
    }
}

#[test]
fn output_that_is_not_a_listing_is_exit_1_and_says_what_arrived() {
    let world = World::with_devpod(Some("#!/bin/sh\necho \"not json at all\"\n"));
    let run = world.dl(&["--ls"]);
    run.exited(1);
    assert_eq!(
        run.err,
        "error: devpod's workspace listing is not JSON: 'not json at all\\n'\n"
    );
}

#[test]
fn silence_from_devpod_is_exit_1_and_named_as_silence() {
    let world = World::with_devpod(Some("#!/bin/sh\nexit 0\n"));
    let run = world.dl(&["--ls"]);
    run.exited(1);
    assert_eq!(
        run.err,
        "error: devpod said nothing when asked to list workspaces; it prints `[]` when there \
         are none\n"
    );
}

#[test]
fn a_command_line_python_also_refused_keeps_pythons_exit_1_and_pythons_words() {
    let world = World::full();
    let run = world.dl(&["some-workspace", "wat"]);
    run.exited(1);
    assert_eq!(
        run.err,
        "Unknown command 'wat'. Use 'dl some-workspace -- wat' to run a shell command.\n"
    );
}

#[test]
fn a_usage_error_is_claps_exit_2() {
    // Divergence rows 2 and 3: an unknown flag is refused rather than read as a
    // workspace name, and it exits with clap's usage code. Python read `--nope` as
    // a spec and exited 1 saying the workspace was unknown.
    let world = World::full();
    let run = world.dl(&["--nope"]);
    run.exited(2);
    assert!(run.err.contains("--nope"), "{:?}", run.err);
    let help = world.dl(&["--help"]);
    help.succeeded();
    assert!(help.out.contains("Usage: dl"), "{:?}", help.out);
}

// ===========================================================================
// --install
// ===========================================================================

#[test]
fn install_writes_the_script_and_the_rc_block_and_says_so_once() {
    let world = World::full();
    let run = world.dl(&["--install"]);
    run.succeeded();
    assert_eq!(
        run.out,
        "[devlaunch] Autocomplete has been updated. Run 'source {ROOT}/home/.bashrc' or restart \
         your terminal to enable completion.\n"
    );
    assert!(
        world.read("home/completions.sh").contains("_dl_completion"),
        "the completion script was not written"
    );
    let rc = world.read("home/.bashrc");
    assert!(
        rc.contains("# >>> devlaunch completions >>>")
            && rc.contains(&format!(
                "source \"{}/home/completions.sh\"",
                world.root.display()
            )),
        "the rc block was not written: {rc:?}"
    );

    // Divergence row 10: a second run over an already-current install rewrites
    // nothing and reports that, where Python rewrote byte-identical files and
    // touched the rc file's mtime every time.
    let again = world.dl(&["--install"]);
    again.succeeded();
    assert_eq!(
        again.out,
        "[devlaunch] Autocomplete is already installed and current. Run 'source \
         {ROOT}/home/.bashrc' or restart your terminal if completion is not working yet.\n"
    );
    assert!(
        again.err.contains("already current") && again.err.contains("already sources it"),
        "expected both files reported as untouched, got {:?}",
        again.err
    );
}

#[test]
fn install_edits_the_rc_file_it_is_given() {
    let world = World::full();
    let run = world.dl(&["--install", "~/.zshrc"]);
    run.succeeded();
    assert!(
        world.read("home/.zshrc").contains("devlaunch completions"),
        "the named rc file was not edited"
    );
    assert_eq!(world.read("home/.bashrc"), "", "the default was edited too");
}

// ===========================================================================
// the migration's wiring
// ===========================================================================

#[test]
fn the_json_listing_migrates_the_cache_and_the_table_does_not() {
    // `test_worktree_migration.py`'s TestWiring, at the boundary: the migration
    // runs from the one place the clone manager is built, so the commands that
    // never build one never migrate. `--ls` reads devpod and nothing else;
    // `--ls --json` reads the records, so it is the one that migrates.
    let world = World::full();
    let v1 = world
        .read("cache/devlaunch/metadata.json")
        .replace("\"version\": 2", "\"version\": 1");
    std::fs::write(world.path("cache/devlaunch/metadata.json"), &v1).expect("a v1 document");

    world.dl(&["--ls"]).succeeded();
    assert!(
        world
            .read("cache/devlaunch/metadata.json")
            .contains("\"version\": 1"),
        "the table command migrated the cache"
    );

    world.dl(&["--ls", "--json"]).succeeded();
    assert!(
        world
            .read("cache/devlaunch/metadata.json")
            .contains("\"version\": 2"),
        "the json listing did not migrate the cache"
    );
}

// ===========================================================================
// timing
// ===========================================================================

#[test]
fn the_timing_summary_is_asked_for_by_the_environment_and_lands_on_stderr() {
    let world = World::full();
    let off = world.dl(&["--ls"]);
    off.succeeded();
    assert!(!off.err.contains("dl-timing"), "{:?}", off.err);

    let prose = world.dl_with(&["--ls"], &[("DEVLAUNCH_TIMING", "1")]);
    prose.succeeded();
    assert!(
        prose.err.contains("dl-timing: total "),
        "expected a prose summary, got {:?}",
        prose.err
    );
    assert_eq!(prose.out, LS_TABLE, "the summary must not reach stdout");

    let document = world.dl_with(&["--ls"], &[("DEVLAUNCH_TIMING", "json")]);
    document.succeeded();
    assert!(
        document.err.contains("dl-timing-json: {"),
        "expected a document, got {:?}",
        document.err
    );
}

// ===========================================================================
// the seams
// ===========================================================================

#[test]
fn a_command_whose_flow_is_not_ported_refuses_and_names_the_milestone() {
    // The alternative — succeeding silently — is what would let a mid-port build
    // look like it had stopped a workspace.
    let world = World::full();
    for (args, expected) in [
        (vec!["--purge"], "--purge is not in this build yet"),
        (vec!["--prune"], "--prune is not in this build yet"),
        (vec!["--reconcile"], "--reconcile is not in this build yet"),
        (
            vec!["some-workspace", "stop"],
            "`dl <workspace> stop` is not in this build yet",
        ),
        (
            vec!["some-workspace"],
            "`dl <workspace> attach` is not in this build yet",
        ),
        (
            vec!["stop"],
            "the interactive workspace selector is not in this build yet",
        ),
    ] {
        let run = world.dl(&args);
        run.exited(1);
        assert!(run.err.contains(expected), "dl {args:?} said {:?}", run.err);
    }
}
