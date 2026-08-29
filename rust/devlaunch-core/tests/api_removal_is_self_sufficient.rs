//! A workspace removal driven from `devlaunch_core::api` and nothing else, over a
//! clone holding the only copy of somebody's work.
//!
//! This is devlaunch#410 as a test. The promised tier used to carry
//! `workspace_delete` — the delete **without** the unsaved-work guard — while the
//! probe and the guard that make it safe were two further exported functions the
//! caller had to run first, in the right order, with the right arguments. So a
//! second consumer following the promise exactly deleted the clone. There is one
//! exported removal now, and the guard is inside it.
//!
//! Two things are asserted, and the import list is the first of them. Every name
//! comes through `api`: no `flows::`, `domain::` or `clients::` path appears below,
//! because a promise a caller cannot reach the parameter types of is not a promise.
//! The runner is the exception `api_launch_is_self_sufficient.rs` already makes —
//! it is its own crate and its own promised seam — and it is not a parameter of
//! `workspace_remove` either.
//!
//! The second is behavioural, and it is what "the guard cannot be skipped" means:
//! a recorded clone with an uncommitted change refuses, **devpod is never asked**,
//! and the refusal carries what would have been lost rather than a sentence about
//! it. The world these tests drive is a real cache directory on disk with a real
//! git repository in it, built the way `dl` leaves one, because the point is that
//! the promised call reads the machine and not a fixture handed to it.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use devlaunch_core::api::{
    ColdPath, CommandContext, DeleteStalled, DevpodHome, KeptCopies, LifecycleNotice, Notices,
    Records, RecordsNotice, Refresh, Removal, RemovalRefused, RemoveOutcome, SelfInvocation,
    workspace_remove,
};
use devlaunch_core::runner::{
    CapturedText, DetachOutcome, Invocation, Outcome, ProcessRunner, Runner, SpawnSpec,
};
use devlaunch_test_support::{FakeRunner, WorkspaceState};

const OWNER: &str = "blooop";
const REPO: &str = "devlaunch";
const BRANCH: &str = "main";
const WORKSPACE: &str = "devlaunch-main-aa11";

// ===========================================================================
// the tests
// ===========================================================================

/// The finding, at the surface it destroys things from.
///
/// `Insistence` is not a parameter of this call and there is no argument that
/// reaches the delete without the guard: `Removal::Guarded` is `dl <ws> rm`, and a
/// clone holding an uncommitted change ends it before devpod is asked. The refusal
/// is read rather than merely matched — its `losses` field is bound and its words
/// are the ones a caller would print.
#[test]
fn a_guarded_removal_refuses_over_unsaved_work_and_never_asks_devpod() {
    let machine = Machine::new();
    machine.a_recorded_clone_holding("an-hour-of-work.md");

    let outcome = machine.remove(Removal::Guarded);

    let RemoveOutcome::Refused(RemovalRefused::WouldLose {
        workspace_id,
        losses,
    }) = outcome
    else {
        panic!("expected a refusal that names what would be lost, got {outcome:?}");
    };
    assert_eq!(workspace_id, WORKSPACE);
    assert!(
        losses.describe().contains("an-hour-of-work.md"),
        "the refusal has to carry what would be lost, not a count: {}",
        losses.describe()
    );
    assert!(
        machine.devpod_argvs().is_empty(),
        "a refused removal must not have asked devpod for anything: {:?}",
        machine.devpod_argvs()
    );
    assert!(
        machine.clone_dir().exists(),
        "the clone is what the refusal was protecting"
    );
}

/// `rm --force` removes it anyway, and pays for no probe on the way.
///
/// The conditional probe is a `git status` and a `git log` per clone, and running
/// it unconditionally is the one-line accident the fold invites: it costs nothing
/// visible and is only wrong in the bill. `Removal::Insisted` has said in advance
/// that it will not act on the answer, so it must not ask the question.
#[test]
fn an_insisted_removal_deletes_the_same_clone_and_runs_no_probe() {
    let machine = Machine::new();
    machine.a_recorded_clone_holding("an-hour-of-work.md");

    let outcome = machine.remove(Removal::Insisted);

    assert!(
        matches!(outcome, RemoveOutcome::Deleted { .. }),
        "expected the workspace to go, got {outcome:?}"
    );
    assert_eq!(machine.deleted_workspaces(), [WORKSPACE]);
    assert!(!machine.clone_dir().exists(), "the clone goes with it");
    let probed: Vec<Vec<String>> = machine
        .argvs_to("git")
        .into_iter()
        .filter(|argv| argv.iter().any(|word| word == "status" || word == "log"))
        .collect();
    assert!(
        probed.is_empty(),
        "`rm --force` acts on no finding, so it should look for none: {probed:?}"
    );
}

/// The ordering the delete's own comment claims, as an assertion.
///
/// devpod's record of the workspace is the only place the substituted volume names
/// live, and `devpod delete` takes that record away with the workspace. Named
/// afterwards, the sweep would find nothing every time and look like a working
/// cleanup: the workspace still goes, the exit status is still zero, and the
/// volumes stay on the disk for ever.
///
/// So the fake devpod here removes its own record when it is asked to delete, the
/// way the real one does, and what the test reads is whether docker was asked about
/// the two names that record held. Read after the delete, there would be nothing to
/// ask about.
#[test]
fn the_volumes_are_named_before_devpod_is_asked_to_delete() {
    let machine = Machine::new();
    machine.a_recorded_clone_holding("an-hour-of-work.md");
    machine.a_devcontainer_that_created_a_volume();

    let outcome = machine.remove(Removal::Insisted);

    assert!(
        matches!(outcome, RemoveOutcome::Deleted { .. }),
        "expected the workspace to go, got {outcome:?}"
    );
    let swept: Vec<String> = machine.argvs_to("docker").into_iter().flatten().collect();
    for volume in [
        format!("{WORKSPACE}-pixi"),
        "dind-var-lib-docker-f00d".to_owned(),
    ] {
        assert!(
            swept.contains(&volume),
            "{volume} was named in devpod's record and had to be swept: {swept:?}"
        );
    }
}

/// The copy devlaunch keeps of those names goes with them.
///
/// `workspace_remove` takes the copy store because the removal is where a copy
/// stops being worth keeping: devlaunch#456 drops it on the proof that docker
/// removed the volumes it named. The store is a parameter rather than something
/// the removal resolves, so the way to get this wrong is to accept one and never
/// reach it, which nothing above would notice: the workspace still goes, the exit
/// is still zero, and the next `--prune` reports reclaiming volumes that left with
/// the workspace.
#[test]
fn the_kept_copy_of_the_volumes_goes_with_the_workspace() {
    let machine = Machine::new();
    machine.a_recorded_clone_holding("an-hour-of-work.md");
    machine.a_devcontainer_that_created_a_volume();
    machine.a_kept_copy_naming(&[&format!("{WORKSPACE}-pixi")]);

    let outcome = machine.remove(Removal::Insisted);

    assert!(
        matches!(outcome, RemoveOutcome::Deleted { .. }),
        "expected the workspace to go, got {outcome:?}"
    );
    assert!(
        !machine.kept_copy_path().exists(),
        "docker removed the volumes the copy named, so the copy names nothing: {}",
        machine.kept_copy_path().display()
    );
}

// ===========================================================================
// the machine the removal reads
// ===========================================================================

/// The environment these tests point devlaunch at, and the lock that makes that
/// sound.
///
/// `HOME` and the two XDG variables are process-wide, so every test here takes this
/// for its whole body: the removal resolves its cache directory from the
/// environment while it runs, and a second test rewriting those variables
/// underneath it would be reading a directory this one is deleting. Serialised
/// rather than shared, so each test still gets a cache of its own.
static THE_ENVIRONMENT: Mutex<()> = Mutex::new(());

/// A cache directory as `dl` leaves one, with the devpod that answers about it.
///
/// devpod and docker are faked and everything else is really run — git above all,
/// because what the guard reads is a real repository's real answer about what is in
/// it. The two that are faked are faked for the same reason: a unit test that
/// reached the developer's own devpod or docker daemon would be deleting their
/// workspaces and their volumes.
struct Machine {
    fake: FakeRunner,
    processes: ProcessRunner,
    argvs: Mutex<Vec<Vec<String>>>,
    dir: tempfile::TempDir,
    cache: PathBuf,
    /// Held for the test's lifetime. Last field, so it is dropped last.
    _environment: MutexGuard<'static, ()>,
}

impl Machine {
    fn new() -> Self {
        let environment = THE_ENVIRONMENT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("a temporary directory");
        let home = dir.path().to_path_buf();
        let cache = home.join("cache").join("devlaunch");
        std::fs::create_dir_all(&cache).expect("the cache directory");
        // Safety: every reader of these variables in this binary is a test body,
        // and every test body holds the lock this guard is. Nothing else in the
        // process reads the environment while they are being written.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_CACHE_HOME", home.join("cache"));
            std::env::set_var("XDG_CONFIG_HOME", home.join("config"));
            std::env::set_var("DEVPOD_HOME", home.join(".devpod"));
        }
        let fake = FakeRunner::new();
        fake.add_workspace(WORKSPACE, WorkspaceState::Running);
        Self {
            fake,
            processes: ProcessRunner,
            argvs: Mutex::new(Vec::new()),
            dir,
            cache,
            _environment: environment,
        }
    }

    fn clone_dir(&self) -> PathBuf {
        self.cache
            .join("repos")
            .join(OWNER)
            .join(REPO)
            .join(WORKSPACE)
    }

    /// A real git repository under the cache, holding `file` and nothing that has
    /// been committed or pushed, plus the `metadata.json` record naming it.
    ///
    /// Written as a file rather than through a builder because that is what it is:
    /// the record is what a previous `dl` run left on disk, and the removal has to
    /// read it from there.
    fn a_recorded_clone_holding(&self, file: &str) {
        let clone = self.clone_dir();
        std::fs::create_dir_all(&clone).expect("the clone directory");
        git(&clone, &["init", "-b", BRANCH]);
        std::fs::write(clone.join(file), "half a plan\n").expect("their work");

        let record = serde_json::json!({
            "version": 3,
            "repositories": {},
            "worktrees": {
                format!("{OWNER}/{REPO}/{BRANCH}"): {
                    "owner": OWNER,
                    "repo": REPO,
                    "branch": BRANCH,
                    "local_path": clone.display().to_string(),
                    "workspace_id": WORKSPACE,
                    "created_at": "2026-01-01T00:00:00",
                    "last_used": "2026-01-01T00:00:00",
                    "devpod_workspace_id": null,
                },
            },
        });
        std::fs::write(
            self.cache.join("metadata.json"),
            serde_json::to_string(&record).expect("a metadata document"),
        )
        .expect("the metadata file");
    }

    /// devpod's record of a finished `up`, naming what it substituted into the
    /// devcontainer — which is the only place the volume names live.
    fn a_devcontainer_that_created_a_volume(&self) {
        let workspace = self.devpod_workspace_dir(WORKSPACE);
        std::fs::create_dir_all(&workspace).expect("devpod's workspace directory");
        // Both files, because both are read: the record is what says which context
        // holds this id, and the result is what says what was substituted into it.
        std::fs::write(
            workspace.join("workspace.json"),
            serde_json::json!({ "id": WORKSPACE }).to_string(),
        )
        .expect("devpod's workspace record");
        let result = serde_json::json!({
            "SubstitutionContext": {
                "LocalWorkspaceFolder": format!("/host/clones/{WORKSPACE}"),
                "DevContainerID": "f00d",
            },
        });
        std::fs::write(workspace.join("workspace_result.json"), result.to_string())
            .expect("devpod's create result");
    }

    /// devlaunch's own copy of what devpod substituted, as a completed `up` leaves
    /// one under the cache directory.
    ///
    /// Written as a file for `a_recorded_clone_holding`'s reason: the store's write
    /// verb is internal to the crate, and what this test is about is the copy a
    /// previous run left on disk.
    fn a_kept_copy_naming(&self, volumes: &[&str]) {
        let path = self.kept_copy_path();
        std::fs::create_dir_all(path.parent().expect("the copies directory"))
            .expect("the copies directory");
        let copy = serde_json::json!({ "volumes": volumes });
        std::fs::write(&path, copy.to_string()).expect("the kept copy");
    }

    /// Where `KeptCopies::under(&self.cache)` keeps this workspace's copy.
    fn kept_copy_path(&self) -> PathBuf {
        self.cache
            .join("workspace-copies")
            .join(format!("{WORKSPACE}.json"))
    }

    /// Open devlaunch's records the way a command does, and remove the workspace
    /// through the one call the promise carries.
    fn remove(&self, removal: Removal) -> RemoveOutcome {
        let mut opening: Vec<RecordsNotice> = Vec::new();
        let mut cold = ColdPath::new(self, &mut opening);
        let records = cold.records().expect("devlaunch's records open");
        let Records {
            storage, clones, ..
        } = records;

        let mut context = CommandContext::new(self);
        let updater = SelfInvocation::new("dl".to_owned());
        let completions = self.cache.join("completions.json");
        let mut refresh = Refresh::new(&updater, &completions);
        // `Vec<T>` is core's own sink for `T`, so a consumer that only wants to
        // collect a removal's notices needs nothing of its own.
        let mut collected: Vec<LifecycleNotice> = Vec::new();
        let said: &mut dyn Notices<LifecycleNotice> = &mut collected;
        let devpod_home = DevpodHome::locate();

        workspace_remove(
            &mut context,
            &mut refresh,
            clones,
            storage,
            &self.cache,
            devpod_home.as_ref(),
            // The same cache directory the records came out of, which is where a
            // launch of this workspace would have written the copy this removal
            // drops.
            &KeptCopies::under(&self.cache),
            WORKSPACE,
            removal,
            &mut |DeleteStalled::OnTheLock| {},
            said,
        )
        .expect("devpod ran")
    }

    // ------------------------------------------------------------ what ran

    fn argvs(&self) -> Vec<Vec<String>> {
        self.argvs.lock().expect("the call log").clone()
    }

    fn argvs_to(&self, program: &str) -> Vec<Vec<String>> {
        self.argvs()
            .into_iter()
            .filter(|argv| argv[0] == program)
            .collect()
    }

    fn devpod_argvs(&self) -> Vec<Vec<String>> {
        self.argvs_to("devpod")
    }

    /// Every id `devpod delete` was called about, in order.
    fn deleted_workspaces(&self) -> Vec<String> {
        self.devpod_argvs()
            .into_iter()
            .filter(|argv| argv.get(1).map(String::as_str) == Some("delete"))
            .filter_map(|argv| argv.get(2).cloned())
            .collect()
    }

    fn record(&self, spec: &SpawnSpec) {
        self.argvs
            .lock()
            .expect("the call log")
            .push(spec.invocation.argv());
    }

    /// What devpod does to its own records on the way out of a delete, which the
    /// fake devpod does not do for itself.
    ///
    /// Modelled because it is the whole hazard: the substituted volume names live
    /// only in this directory, so a sweep that reads them after the delete reads
    /// nothing. Without this, both orders pass.
    fn devpod_forgets_what_it_deleted(&self, spec: &SpawnSpec) {
        let argv = spec.invocation.argv();
        if argv.len() >= 3 && argv[0] == "devpod" && argv[1] == "delete" {
            let _ = std::fs::remove_dir_all(self.devpod_workspace_dir(&argv[2]));
        }
    }

    fn devpod_workspace_dir(&self, workspace_id: &str) -> PathBuf {
        self.dir
            .path()
            .join(".devpod")
            .join("contexts")
            .join("default")
            .join("workspaces")
            .join(workspace_id)
    }
}

/// Which programs this machine answers for itself.
fn faked(program: &str) -> bool {
    program == "devpod" || program == "docker"
}

impl Runner for Machine {
    fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
        self.record(spec);
        if faked(&spec.invocation.program) {
            self.devpod_forgets_what_it_deleted(spec);
            self.fake.capture(spec)
        } else {
            self.processes.capture(spec)
        }
    }

    fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
        self.record(spec);
        if faked(&spec.invocation.program) {
            self.devpod_forgets_what_it_deleted(spec);
            self.fake.passthrough(spec)
        } else {
            self.processes.passthrough(spec)
        }
    }

    fn session(&self, spec: &SpawnSpec, on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
        self.record(spec);
        if faked(&spec.invocation.program) {
            self.devpod_forgets_what_it_deleted(spec);
            self.fake.session(spec, on_stderr_line)
        } else {
            self.processes.session(spec, on_stderr_line)
        }
    }

    /// Recorded and never started: the refresh a removal re-arms is a whole second
    /// `dl` run, and a test that really forked one would be running an unrelated
    /// program against the developer's own cache.
    fn detach(&self, what: &Invocation) -> DetachOutcome {
        self.fake.detach(what)
    }
}

/// git, really run, in `at`.
fn git(at: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(at)
        .env("GIT_AUTHOR_NAME", "dl")
        .env("GIT_AUTHOR_EMAIL", "dl@example.invalid")
        .env("GIT_COMMITTER_NAME", "dl")
        .env("GIT_COMMITTER_EMAIL", "dl@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        at.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}
