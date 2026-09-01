//! What the lifecycle commands do, at three seams.
//!
//! **The argv seam.** Every devpod call's whole argv, through the fake runner,
//! because a rewritten body cannot preserve `devpod delete <id>
//! --ignore-not-found` by accident and the flags are what devpod acts on.
//!
//! **Real filesystem, real git.** The prune classification, the delete guard
//! and the partial-removal walk are all decided by the state of real
//! directories and by a real `git status` / `git log --not --remotes`. A faked
//! spawn answers a clean exit with empty output, which reads as "this clone
//! holds nothing" — the answer that deletes. Written the other way round,
//! Python's own central guard passed while guarding nothing and the clone with
//! two unpushed commits in it was removed, so these tests build a local bare
//! repository standing in for GitHub (a local path is a real git remote) and
//! let git run.
//!
//! **Permissions, verified rather than assumed.** Every refusal case goes
//! through `refusing_writes`/`refusing_reads`, which apply the mode and then
//! *try the write*: root is refused by nothing, and a stored-but-ignored mode
//! is ordinary on bind and overlay mounts. Where the filesystem does not deny,
//! the test steps aside instead of asserting something it cannot reproduce.
//!
//! Ported from these Python suites, all of which retired with the Python tree
//! (#267) — the names are what to grep the history for, not files to open:
//! `test_purge_partial_removal`, `test_purge_ownership`'s purge-action
//! classes, `test_workspace_listing::TestPurgeWillNotActOnAListItCouldNotRead`,
//! `test_workspace_state`'s `TestTheDeleteGuard` and
//! `TestForcedRemoveIsEnsureAbsent`, `test_dl`'s
//! `TestBackgroundRefreshSpawning`, `TestRefreshChildRechecksFreshness` and
//! `TestWorkspaceCommandsRefreshOnceAfterwards`,
//! `test_updater_fetch_sweep`, `test_stored_workspace_id`,
//! `test_prune_orphaned_clones`, `test_workspace_source_placement` and
//! `test_reconcile_orphaned_workspaces`.
//!
//! `remove_tree_as_far_as_it_goes` lives in `flows::repo_manager` beside the
//! cleanup it is the counterpart of, and is tested from here: `dl --purge` is
//! its only caller and the three-armed answer exists for the purge's three
//! headlines.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use devlaunch_runner::{
    CapturedText, DetachOutcome, Invocation as RawInvocation, Outcome, ProcessRunner, SpawnSpec,
};
use devlaunch_test_support::{FakeRunner, Response};

use super::*;

use crate::clients::devpod::{
    ContainerState, ListingUnreadable, NotRun, Patience, Workspace, WorkspaceSource,
};
use crate::clients::devpod_home::{DevpodHome, RepointFailure, ScratchHome, devpod_home_with};
use crate::clients::git::Git;
use crate::clients::{devpod, docker};
use crate::domain::locks;
use crate::domain::metadata::{self, MetadataStorage, WorktreeFilter};
use crate::domain::model::{BaseRepository, SweepNote, SweepTrouble, Timestamp, WorktreeInfo};
use crate::domain::workspace_id::WorkspaceId;
use crate::domain::workspace_state::{self, NonEmpty};
use crate::flows::agent_worktrees::{self, Standing, Verdict};
use crate::flows::completion_cache;
use crate::flows::kept_copies::KeptCopies;
use crate::flows::listing::CommandContext;
use crate::flows::repo_manager::tests::{refusing_reads, refusing_writes, run_git};
use crate::flows::repo_manager::{
    BACKGROUND_FETCH_TIMEOUT, CacheNotice, LazyFetchError, RefusalReason, RemoveTreeError,
    RepositoryManager, TreeSweep, bare_dir, present, remove_tree_as_far_as_it_goes,
};
use crate::flows::workspace_clone::{GitLfs, RemoveWorkspaceError, Removed, WorkspaceCloneManager};
use crate::runner::{Exit, Runner};
use crate::timing;

/// A devpod home whose create result for `workspace_id` records what devpod
/// substituted into that workspace's devcontainer.
///
/// The shape is devpod's own: `SubstitutionContext` beside `ContainerDetails`
/// and `MergedConfig` in `workspace_result.json`, with the field spellings
/// devpod's `config.SubstitutionContext` serialises. Read off the pinned
/// devpod binary's struct tags rather than assumed, because every volume name
/// below is built from these two strings.
fn devpod_home_recording(
    workspace_id: &str,
    local_workspace_folder: &str,
    devcontainer_id: &str,
) -> ScratchHome {
    let home = devpod_home_with(&[("default", workspace_id, Some(()))]);
    let result = home.result("default", workspace_id);
    std::fs::write(
        &result,
        serde_json::json!({
            "ContainerDetails": { "Id": "container-id" },
            "MergedConfig": {},
            "SubstitutionContext": {
                "LocalWorkspaceFolder": local_workspace_folder,
                "ContainerWorkspaceFolder": "/workspaces/whatever",
                "DevContainerID": devcontainer_id,
            },
        })
        .to_string(),
    )
    .expect("a create result");
    home
}

#[test]
fn a_devpod_that_cannot_be_run_fails_the_stage_but_one_that_refuses_does_not() {
    // Python's `@timing.staged("devpod-up") get_workspace_state` returns None
    // for a devpod that ran and refused, gave non-JSON, or omitted `state`, so
    // the stage stays `ok`; only a devpod that could not be run at all raises
    // (`DevpodNotInstalled`) and the decorator marks the stage `failed`.
    // Rust's `NotRun` is that case and nothing else (P12/C8).
    let _serialized = timing::exclusive();

    fn devpod_up_outcome(runner: &dyn devlaunch_runner::Runner) -> &'static str {
        timing::install(Some(timing::Registry::start(
            timing::Mode::Document,
            timing::Seam::default(),
            0.0,
        )));
        let _ = workspace_state(runner, "dl-ws", Patience::AsLongAsItTakes);
        let report = timing::emit().expect("a report");
        let document = report.document().expect("a document");
        document
            .stages
            .iter()
            .find(|stage| stage.stage == "devpod-up")
            .expect("a devpod-up stage")
            .outcome
    }

    let missing = FakeRunner::new();
    missing.script(["devpod"], Response::ProgramNotFound);
    assert_eq!(
        devpod_up_outcome(&missing),
        "failed",
        "a devpod that could not be run must fail the stage"
    );

    let refused = FakeRunner::new();
    refused.script(["devpod"], Response::failed(1, "no such workspace\n"));
    assert_eq!(
        devpod_up_outcome(&refused),
        "ok",
        "a devpod that ran and refused is Python's None return — the stage stays ok"
    );

    timing::install(None);
}

// ------------------------------------------------------------ test doubles

/// devpod from the fake, everything else from real processes.
///
/// The shim's arrangement, in-process. git has to be real here for the reason
/// the module docs give; devpod has to be fake because the listing is the
/// fixture, and because the two passes of `--prune` are meant to be able to see
/// different worlds.
struct Devpod {
    fake: FakeRunner,
    processes: ProcessRunner,
    /// See [`timing::exclusive`]. Last field, so it is dropped last.
    _serialized: timing::Exclusive,
}

impl Devpod {
    /// The runner, and the timing exclusion for as long as it lives.
    ///
    /// Every devpod call through it is spanned against the **process-global**
    /// registry (`clients::devpod` names each round trip), and
    /// [`workspace_state`] opens the `devpod-up` stage — so a test holding one
    /// of these would otherwise write into whatever document a concurrent
    /// measured test had installed. In the fixture rather than per test, so a
    /// new test cannot forget it.
    fn new() -> Self {
        Self {
            fake: FakeRunner::new(),
            processes: ProcessRunner,
            _serialized: timing::exclusive(),
        }
    }

    /// What `devpod list --output json` answers from now on.
    fn lists(&self, entries: &[serde_json::Value]) {
        let listing = serde_json::Value::Array(entries.to_vec()).to_string();
        self.fake.clear_scripts();
        self.fake
            .script(["devpod", "list"], Response::stdout(listing));
    }

    /// devpod has a workspace of this name, so `stop` and `delete` address
    /// something. The scripted listing is what `--ls` reads; this is the state
    /// machine underneath it that the other verbs act on.
    fn knows(&self, workspace_id: &str) {
        self.fake.add_workspace(
            workspace_id,
            devlaunch_test_support::WorkspaceState::Running,
        );
    }

    /// devpod refuses the listing outright.
    fn cannot_list(&self, stderr: &str) {
        self.fake.clear_scripts();
        self.fake
            .script(["devpod", "list"], Response::failed(1, stderr));
    }

    fn devpod_argvs(&self) -> Vec<Vec<String>> {
        self.fake.args_to(devpod::PROGRAM)
    }

    /// Every id `devpod delete` was called about, in order.
    fn deleted(&self) -> Vec<String> {
        self.devpod_argvs()
            .into_iter()
            .filter(|argv| argv.first().map(String::as_str) == Some("delete"))
            .filter_map(|argv| argv.get(1).cloned())
            .collect()
    }

    /// Every docker call's argv tail, in order.
    fn docker_argvs(&self) -> Vec<Vec<String>> {
        self.fake.args_to(docker::PROGRAM)
    }

    fn detached(&self) -> Vec<Vec<String>> {
        self.fake
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                devlaunch_test_support::Call::Detach(invocation) => Some(invocation.argv()),
                _ => None,
            })
            .collect()
    }
}

/// Which programs this fixture answers for itself. Everything else — git,
/// above all — is really run, which is what the repo_manager fixtures need.
///
/// `docker` is on the list for the reason `devpod` is: a delete spawns it now,
/// and a unit test that reached the developer's own docker daemon would be
/// removing real volumes named after a fixture.
fn faked(program: &str) -> bool {
    program == devpod::PROGRAM || program == docker::PROGRAM
}

impl Runner for Devpod {
    fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
        if faked(&spec.invocation.program) {
            self.fake.capture(spec)
        } else {
            self.processes.capture(spec)
        }
    }

    fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
        if faked(&spec.invocation.program) {
            self.fake.passthrough(spec)
        } else {
            self.processes.passthrough(spec)
        }
    }

    fn session(&self, spec: &SpawnSpec, on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
        if faked(&spec.invocation.program) {
            self.fake.session(spec, on_stderr_line)
        } else {
            self.processes.session(spec, on_stderr_line)
        }
    }

    /// Every detached spawn is recorded and never started: the refresh child is
    /// a whole second `dl` run, and a unit test that really forked one would be
    /// running an unrelated program against the developer's own cache.
    fn detach(&self, what: &RawInvocation) -> DetachOutcome {
        self.fake.detach(what)
    }
}

// ---------------------------------------------------------------- helpers

fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

fn commit(work: &Path, message: &str) {
    run_git(work, &["add", "-A"]);
    run_git(work, &["commit", "-m", message]);
}

/// One `devpod list --output json` element, sourced at a local folder.
fn listed(workspace_id: &str, source: &Path) -> serde_json::Value {
    serde_json::json!({
        "id": workspace_id,
        "source": { "localFolder": source.display().to_string() },
        "provider": { "name": "docker" },
        "ide": { "name": "none" },
        "context": "default",
        "lastUsed": "2026-08-08T11:43:27Z",
    })
}

/// One element whose source is whatever devpod happened to write.
fn listed_with(workspace_id: &str, source: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": workspace_id,
        "source": source,
        "provider": { "name": "docker" },
        "ide": { "name": "none" },
        "context": "default",
        "lastUsed": "2026-08-08T11:43:27Z",
    })
}

fn one_workspace(id: &str, source: serde_json::Value) -> Workspace {
    devpod::parse_workspaces(&serde_json::json!([listed_with(id, source)]).to_string())
        .expect("a listing")
        .remove(0)
}

/// A completion cache file with a chosen age, so freshness is a fixture rather
/// than a race with the clock.
fn a_completion_cache(dir: &Path, age: Duration) -> PathBuf {
    let path = dir.join("completions.json");
    std::fs::write(&path, "{}").expect("a completion cache");
    let when = SystemTime::now() - age;
    let times = std::fs::FileTimes::new().set_modified(when);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("the cache file")
        .set_times(times)
        .expect("an mtime");
    path
}

fn fresh_cache(dir: &Path) -> PathBuf {
    a_completion_cache(dir, Duration::from_secs(1))
}

fn stale_cache(dir: &Path) -> PathBuf {
    a_completion_cache(
        dir,
        completion_cache::COMPLETION_CACHE_TTL + Duration::from_secs(60),
    )
}

fn ignoring() -> Vec<LifecycleNotice> {
    Vec::new()
}

/// A delete nothing was blocking, for the tests that are about something else.
fn unblocked() -> impl FnMut(DeleteStalled) {
    |stalled| panic!("the delete reported {stalled:?} and this test expected none")
}

/// The cache `--prune` and `--reconcile` are pointed at, and the devpod that
/// answers about it.
///
/// One real clone of each kind the classification has an arm for, plus the bare
/// cache every one of them was made from. Everything lives under one temp
/// directory, so the directories scanned, the metadata file and `repos_dir` all
/// agree without being patched into agreement — a fixture whose clones sit
/// outside the directory under test is how a guard comes to run zero times.
struct World {
    dir: tempfile::TempDir,
    cache: PathBuf,
    repos_dir: PathBuf,
    repo_dir: PathBuf,
    origin: PathBuf,
    bare: PathBuf,
    storage: MetadataStorage,
    devpod: Devpod,
}

const OWNER: &str = "o";
const REPO: &str = "r";

impl World {
    /// A cache with a bare clone of a one-commit remote and nothing else.
    fn empty() -> Self {
        let dir = temp_dir();
        let root = dir.path().to_path_buf();
        let cache = root.join("cache").join("devlaunch");
        let repos_dir = cache.join("repos");
        let repo_dir = repos_dir.join(OWNER).join(REPO);
        std::fs::create_dir_all(&repo_dir).expect("the repo directory");

        let seed = root.join("seed");
        std::fs::create_dir_all(&seed).expect("the seed directory");
        run_git(&root, &["init", "-b", "main", &seed.display().to_string()]);
        std::fs::write(seed.join("README.md"), "seed\n").expect("a README");
        commit(&seed, "seed");
        let origin = root.join("origin.git");
        run_git(
            &root,
            &[
                "clone",
                "--bare",
                &seed.display().to_string(),
                &origin.display().to_string(),
            ],
        );
        let bare = repo_dir.join(".bare");
        run_git(
            &root,
            &[
                "clone",
                "--bare",
                &origin.display().to_string(),
                &bare.display().to_string(),
            ],
        );
        let (storage, _) =
            MetadataStorage::open(cache.join("metadata.json")).expect("a metadata store");
        let devpod = Devpod::new();
        devpod.lists(&[]);
        Self {
            dir,
            cache,
            repos_dir,
            repo_dir,
            origin,
            bare,
            storage,
            devpod,
        }
    }

    fn tmp(&self) -> &Path {
        self.dir.path()
    }

    /// One real workspace clone, fully pushed, at `leaf`, on `branch`.
    fn clone_at(&self, leaf: &str, branch: &str) -> PathBuf {
        let clone = self.repo_dir.join(leaf);
        run_git(
            self.tmp(),
            &[
                "clone",
                &self.bare.display().to_string(),
                &clone.display().to_string(),
            ],
        );
        run_git(
            &clone,
            &[
                "remote",
                "set-url",
                "origin",
                &self.origin.display().to_string(),
            ],
        );
        // `-B` rather than `-b`: a clone of a `main`-headed remote already has
        // `main`, and the fixture asks for that branch by name like any other.
        run_git(&clone, &["checkout", "-B", branch]);
        std::fs::write(clone.join(format!("{branch}.txt")), "work\n").expect("a tracked file");
        commit(&clone, branch);
        run_git(&clone, &["push", "-u", "origin", branch]);
        clone
    }

    /// A worktree record naming `clone`, with `leaf` as its workspace id.
    fn record(&mut self, leaf: &str, branch: &str, clone: &Path) -> WorktreeInfo {
        let record = WorktreeInfo::as_an_older_dl_recorded_it(
            OWNER,
            REPO,
            branch,
            clone.to_path_buf(),
            leaf,
        );
        self.storage
            .add_worktree(record.clone())
            .expect("the record is written");
        record
    }

    /// devlaunch's copies of what devpod substituted, under this world's cache.
    fn copies(&self) -> KeptCopies {
        KeptCopies::under(&self.cache)
    }

    fn branches_on_record(&self) -> Vec<String> {
        let mut branches: Vec<String> = self
            .storage
            .list_worktrees(WorktreeFilter::All)
            .into_iter()
            .map(|record| record.branch.clone())
            .collect();
        branches.sort();
        branches
    }
}

/// The clone manager these tests drive.
///
/// A free function over the two fields it needs rather than a method on the
/// fixture, because a method borrows the whole fixture for the manager's
/// lifetime — and every caller then also needs `storage` mutably.
fn clones_for<'r>(repos_dir: &Path, runner: &'r dyn Runner) -> WorkspaceCloneManager<'r> {
    WorkspaceCloneManager::new(
        repos_dir,
        Duration::from_secs(3600),
        Git::new(runner),
        GitLfs::NotInstalled,
    )
}

/// The plan `--prune` would print.
fn plan_for(world: &World, insistence: Insistence) -> PrunePlan {
    plan_insisting(
        world,
        Insisted {
            clones: insistence,
            worktrees: Insistence::NotInsisted,
        },
    )
}

/// The plan, with both insistences named.
fn plan_insisting(world: &World, insisted: Insisted) -> PrunePlan {
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let workspaces = context.workspaces().expect("a listing");
    let placement = ClonePlacement::resolve(&clones, &workspaces);
    prune_plan(
        &clones,
        &world.storage,
        &workspaces,
        &world.copies(),
        &placement,
        insisted,
        &mut ignoring(),
    )
    .expect("a plan")
}

/// The paths the plan would remove, in the order it would report them.
fn removing(plan: &PrunePlan) -> Vec<PathBuf> {
    plan.removing.iter().map(|it| it.path.clone()).collect()
}

/// Why the plan keeps `path`, or a failure saying it is not in the plan at all.
///
/// Every assertion about a directory *surviving* goes through here rather than
/// through an existence check, because "it is still there" is true of a clone
/// kept for the right reason and of one kept by a guard that was never asked.
fn kept_because(plan: &PrunePlan, path: &Path) -> KeptBecause {
    let mut found: Vec<&Kept> = plan
        .keeping
        .iter()
        .filter(|kept| kept.path == path)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one report line for {}: {:?}",
        path.display(),
        plan.keeping
    );
    found.remove(0).because.clone()
}

// =======================================================================
// a refused path does not stop the rest (devlaunch#131, #182)
// =======================================================================

/// A devlaunch cache with a clone in it the container's user would have
/// written: `stuck` holds a file, and sealing `stuck` makes that file
/// impossible for us to unlink.
struct SealableCache {
    dir: tempfile::TempDir,
    root: PathBuf,
    completions: PathBuf,
    metadata: PathBuf,
    other_clone: PathBuf,
    stuck: PathBuf,
}

fn a_sealable_cache() -> SealableCache {
    let dir = temp_dir();
    let root = dir.path().join("devlaunch");
    let other_clone = root
        .join("repos")
        .join("blooop")
        .join("bencher")
        .join("bencher-main-ii41");
    let stuck = root
        .join("repos")
        .join("blooop")
        .join("e2e-repo")
        .join("e2e-purge-devlaunchs");
    std::fs::create_dir_all(&other_clone).expect("a clone that will go");
    std::fs::write(other_clone.join("README.md"), "a clone that will go\n").expect("a README");
    let completions = root.join("completions.json");
    let metadata = root.join("metadata.json");
    std::fs::write(&completions, "{}").expect("a completion cache");
    std::fs::write(&metadata, "{}").expect("a metadata file");
    std::fs::create_dir_all(&stuck).expect("the stuck clone");
    std::fs::write(stuck.join("pixi.lock"), "written by the container's user\n")
        .expect("a file we will not be able to unlink");
    SealableCache {
        dir,
        root,
        completions,
        metadata,
        other_clone,
        stuck,
    }
}

/// The refusals of a removal, whichever arm carries them.
fn refused_paths(removal: &TreeSweep) -> Vec<PathBuf> {
    match removal {
        TreeSweep::Everything => Vec::new(),
        TreeSweep::WhatItCould(refused) | TreeSweep::Nothing(refused) => {
            refused.iter().map(|it| it.path.clone()).collect()
        }
    }
}

/// Whether `path` is on disk, where "cannot tell" counts as there — the same
/// distinction the code under test makes, because both would be reporting
/// "gone" about something present.
fn still_there(path: &Path) -> bool {
    present(path)
}

#[test]
fn a_cache_nothing_refuses_goes_completely() {
    let cache = a_sealable_cache();
    assert_eq!(
        remove_tree_as_far_as_it_goes(&cache.root),
        TreeSweep::Everything
    );
    assert!(!cache.root.exists());
}

#[test]
fn a_tree_that_was_never_there_is_a_clean_sweep_not_a_refusal() {
    // A purge run twice is not a failure the second time, and is not a removal
    // that refused nothing while removing nothing either: there is nothing left
    // under that name, which is what the first arm means.
    let dir = temp_dir();
    assert_eq!(
        remove_tree_as_far_as_it_goes(&dir.path().join("never-existed")),
        TreeSweep::Everything
    );
}

#[test]
fn everything_removable_is_removed_and_only_the_obstruction_is_named() {
    // The fault devlaunch#131 measured: one EACCES abandoned the entire cache.
    let cache = a_sealable_cache();
    let Some(_sealed) = refusing_writes(&cache.stuck) else {
        return; // this filesystem does not deny; nothing here can be reproduced
    };
    let removal = remove_tree_as_far_as_it_goes(&cache.root);

    assert!(
        !cache.completions.exists(),
        "a completion cache is removable"
    );
    assert!(!cache.metadata.exists(), "metadata.json is removable");
    assert!(!cache.other_clone.exists(), "another clone is removable");
    assert!(
        cache.stuck.join("pixi.lock").exists(),
        "the sealed file stays"
    );
    assert_eq!(
        refused_paths(&removal),
        [cache.stuck.as_path()],
        "every directory from the cache root down to the sealed one also fails \
         to go, and saying so five times buries the one fact"
    );
    assert!(
        matches!(removal, TreeSweep::WhatItCould(_)),
        "the partial arm has to mean something went: {removal:?}"
    );
}

#[test]
fn the_directory_is_blamed_rather_than_each_file_in_it() {
    // Unlinking needs write permission on the *directory*, not on the file, so a
    // clone owned by the container's user refuses every one of its children
    // separately — and none of them is an ancestor of another, so ancestor
    // suppression alone catches none of them.
    let cache = a_sealable_cache();
    for name in ["README.md", "pyproject.toml", "config"] {
        std::fs::write(cache.stuck.join(name), "also written by the container\n")
            .expect("another file");
    }
    std::fs::create_dir(cache.stuck.join("objects")).expect("an objects directory");
    let Some(_sealed) = refusing_writes(&cache.stuck) else {
        return;
    };
    assert_eq!(
        refused_paths(&remove_tree_as_far_as_it_goes(&cache.root)),
        [cache.stuck.as_path()]
    );
}

#[test]
fn two_separate_obstructions_are_both_listed() {
    // Suppressing ancestors must not suppress siblings.
    let cache = a_sealable_cache();
    let second = cache
        .root
        .join("repos")
        .join("blooop")
        .join("other")
        .join("clone");
    std::fs::create_dir_all(&second).expect("a second clone");
    std::fs::write(second.join("held"), "also stuck\n").expect("a held file");
    let (Some(_one), Some(_two)) = (refusing_writes(&second), refusing_writes(&cache.stuck)) else {
        return;
    };
    let mut refused = refused_paths(&remove_tree_as_far_as_it_goes(&cache.root));
    refused.sort();
    let mut expected = vec![cache.stuck.clone(), second];
    expected.sort();
    assert_eq!(refused, expected);
}

#[test]
fn a_separately_sealed_ancestor_is_reported_as_well() {
    // Where "ancestors are not listed" stops being the right rule. The outer one
    // does not fail *because* of the inner one — clearing the inner would leave
    // the outer exactly as stuck — so each is a separate piece of work, and a
    // person told only about the inner one would fix it and find the purge still
    // refusing.
    let cache = a_sealable_cache();
    let outer = cache.root.join("repos").join("outer");
    let inner = outer.join("middle").join("inner");
    std::fs::create_dir_all(&inner).expect("the inner directory");
    std::fs::write(inner.join("file"), "x").expect("a file");
    // Deepest first: sealing a parent would make sealing its child fail.
    let (Some(_in), Some(_out)) = (refusing_writes(&inner), refusing_writes(&outer)) else {
        return;
    };
    let mut refused = refused_paths(&remove_tree_as_far_as_it_goes(&cache.root));
    refused.sort();
    let mut expected = vec![inner, outer];
    expected.sort();
    assert_eq!(refused, expected);
}

#[test]
fn a_path_whose_parent_is_writable_is_blamed_itself() {
    // Attribution walks up only as far as the permissions justify: without this,
    // a refusal in a perfectly writable directory would be blamed on an ancestor
    // that has nothing wrong with it.
    let cache = a_sealable_cache();
    let held = cache.root.join("repos").join("blooop").join("held-open");
    std::fs::create_dir_all(&held).expect("the held directory");
    std::fs::write(held.join("inner"), "x\n").expect("a file");
    let Some(_sealed) = refusing_writes(&held) else {
        return;
    };
    assert_eq!(
        refused_paths(&remove_tree_as_far_as_it_goes(&cache.root)),
        [held]
    );
}

#[test]
fn a_cache_root_that_refuses_everything_reports_that_nothing_went() {
    // devlaunch#182's case: the root itself is what will not let go. Nothing
    // under it can be unlinked either, since unlinking an entry needs write
    // permission on the directory holding it — so the whole cache is standing
    // afterwards and the honest answer names no removal at all.
    let dir = temp_dir();
    let root = dir.path().join("devlaunch");
    std::fs::create_dir_all(root.join("repos")).expect("an empty repos directory");
    std::fs::write(root.join("metadata.json"), "{}").expect("a metadata file");
    std::fs::write(root.join("completions.json"), "{}").expect("a completion cache");
    let Some(_sealed) = refusing_writes(&root) else {
        return;
    };
    let removal = remove_tree_as_far_as_it_goes(&root);
    assert!(
        matches!(removal, TreeSweep::Nothing(_)),
        "nothing came away: {removal:?}"
    );
    assert_eq!(refused_paths(&removal), [root.as_path()]);
    assert_eq!(
        std::fs::read_to_string(root.join("metadata.json")).expect("still there"),
        "{}"
    );
    assert!(root.join("repos").is_dir());
}

#[test]
fn a_sealed_root_over_clones_that_did_go_is_still_a_partial_success() {
    // The arm is decided by what moved, not by where the obstruction is. A
    // sealed root refuses its own entries and nothing deeper, so the clones
    // under it go. Reading "the root refused" as "nothing came away" would tell
    // somebody their clones survived when they did not — the same class of lie
    // as devlaunch#182, pointed the other way.
    let dir = temp_dir();
    let root = dir.path().join("devlaunch");
    let clone = root
        .join("repos")
        .join("blooop")
        .join("bencher")
        .join("bencher-main-ii41");
    std::fs::create_dir_all(&clone).expect("a clone");
    std::fs::write(clone.join("README.md"), "a clone that will go\n").expect("a README");
    let Some(_sealed) = refusing_writes(&root) else {
        return;
    };
    let removal = remove_tree_as_far_as_it_goes(&root);
    assert!(
        matches!(removal, TreeSweep::WhatItCould(_)),
        "the clones under a sealed root are still removable: {removal:?}"
    );
    assert!(!clone.exists());
}

#[test]
fn a_root_that_cannot_even_be_looked_at_removed_nothing() {
    // "Cannot tell" is not a partial success either: the lstat is refused before
    // a single path is attempted, so there is nothing this could have removed.
    let dir = temp_dir();
    let home = dir.path().join("cachehome");
    let root = home.join("devlaunch");
    std::fs::create_dir_all(&root).expect("the cache");
    std::fs::write(root.join("metadata.json"), "still here").expect("a metadata file");
    let Some(_sealed) = refusing_reads(&home) else {
        return;
    };
    let removal = remove_tree_as_far_as_it_goes(&root);
    assert!(
        matches!(removal, TreeSweep::Nothing(_)),
        "nothing was attempted: {removal:?}"
    );
    assert_eq!(refused_paths(&removal), [root]);
}

#[test]
fn a_symlinked_root_is_refused_and_left_where_it_is() {
    // Refused, not followed and not quietly unlinked. Unlinking only the link
    // reports a clean sweep over clones that are still on disk on another
    // volume, and following it empties a directory the caller never named. A
    // cache root is a symlink because somebody moved their cache, so both
    // answers cost them the same thing by opposite routes.
    //
    // Needs no permissions, so it holds as root too — which matters, because it
    // is the arm a container running as root would otherwise never exercise.
    let dir = temp_dir();
    let target = dir.path().join("elsewhere");
    std::fs::create_dir_all(target.join("repos")).expect("somebody's cache");
    std::fs::write(target.join("metadata.json"), "somebody's cache").expect("their metadata");
    std::fs::write(target.join("repos").join("work.txt"), "somebody's work").expect("their work");
    let link = dir.path().join("cache").join("devlaunch");
    std::fs::create_dir_all(link.parent().expect("a parent")).expect("the cache parent");
    std::os::unix::fs::symlink(&target, &link).expect("a symlink");

    let removal = remove_tree_as_far_as_it_goes(&link);

    let TreeSweep::Nothing(refused) = &removal else {
        panic!("expected a removal that removed nothing, got {removal:?}");
    };
    assert_eq!(refused.len(), 1);
    let refusal = refused.iter().next().expect("one refusal");
    assert_eq!(refusal.path, link);
    // The advice a report gives is `sudo rm -rf <cache>`, which would remove the
    // link and nothing else, so the reason has to carry the real location.
    assert_eq!(
        refusal.reason,
        RefusalReason::RootIsSymlink {
            points_at: Some(target.clone())
        }
    );
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("the link")
            .file_type()
            .is_symlink(),
        "the link is left where it is, not silently removed"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("metadata.json")).expect("their metadata"),
        "somebody's cache"
    );
    assert!(target.join("repos").join("work.txt").exists());
}

#[test]
fn a_symlink_inside_the_tree_is_unlinked_not_followed() {
    let cache = a_sealable_cache();
    let outside = cache.dir.path().join("outside");
    std::fs::create_dir_all(&outside).expect("a directory outside the tree");
    std::fs::write(outside.join("precious.txt"), "not devlaunch's").expect("their file");
    std::os::unix::fs::symlink(&outside, cache.root.join("repos").join("link"))
        .expect("a link to a directory");
    std::os::unix::fs::symlink(
        outside.join("precious.txt"),
        cache.root.join("repos").join("file-link"),
    )
    .expect("a link to a file");

    assert_eq!(
        remove_tree_as_far_as_it_goes(&cache.root),
        TreeSweep::Everything
    );
    assert!(!cache.root.exists());
    assert_eq!(
        std::fs::read_to_string(outside.join("precious.txt")).expect("still there"),
        "not devlaunch's"
    );
}

#[test]
fn a_dangling_symlink_is_removed_without_complaint() {
    let cache = a_sealable_cache();
    std::os::unix::fs::symlink(
        cache.root.join("never-existed"),
        cache.root.join("repos").join("broken"),
    )
    .expect("a dangling link");
    assert_eq!(
        remove_tree_as_far_as_it_goes(&cache.root),
        TreeSweep::Everything
    );
    assert!(!cache.root.exists());
}

#[test]
fn an_unreadable_directory_is_reported_rather_than_skipped() {
    // A directory that cannot even be listed must not pass for empty: without
    // reporting the scan failure the tree would be walked as though it held
    // nothing, the rmdir would fail on it, and the contents would be neither
    // removed nor mentioned.
    let cache = a_sealable_cache();
    let opaque = cache.root.join("repos").join("blooop").join("opaque");
    std::fs::create_dir_all(&opaque).expect("the opaque directory");
    std::fs::write(opaque.join("inside"), "unreadable\n").expect("a file inside");
    let Some(_sealed) = refusing_reads(&opaque) else {
        return;
    };
    let refused = refused_paths(&remove_tree_as_far_as_it_goes(&cache.root));
    assert!(
        refused.contains(&opaque),
        "the directory it could not read must be named: {refused:?}"
    );
}

#[test]
fn an_unlistable_but_empty_directory_is_not_reported() {
    // The other half of the case above, and it goes the opposite way: the scan
    // fails, but the directory is empty so the rmdir afterwards succeeds and
    // there is nothing left to report. Treating the scan failure as the refusal
    // named a path that is not there, and — through the ancestor rule — could
    // have silenced a genuine refusal above it.
    let cache = a_sealable_cache();
    let opaque = cache.root.join("repos").join("blooop").join("opaque");
    std::fs::create_dir_all(&opaque).expect("the opaque directory");
    let Some(_sealed) = refusing_reads(&opaque) else {
        return;
    };
    assert_eq!(
        remove_tree_as_far_as_it_goes(&cache.root),
        TreeSweep::Everything
    );
    assert!(!cache.root.exists());
}

#[test]
fn every_refused_path_is_still_on_disk_afterwards() {
    let cache = a_sealable_cache();
    let Some(_sealed) = refusing_writes(&cache.stuck) else {
        return;
    };
    let refused = refused_paths(&remove_tree_as_far_as_it_goes(&cache.root));
    assert!(!refused.is_empty(), "the sealed directory must refuse");
    for path in refused {
        assert!(
            still_there(&path),
            "{} was reported as refused but is gone",
            path.display()
        );
    }
}

#[test]
fn the_two_invariants_hold_over_randomised_trees() {
    // Hand-built cases check the shapes somebody thought of. This checks the
    // rest, and two invariants are the whole contract:
    //
    // - **nothing survives unsaid** — a tree still on disk with an empty refusal
    //   list is a purge claiming a clean sweep it did not have, and is the only
    //   failure here that costs anybody anything;
    // - **nothing is said that is not there** — naming a path the user then
    //   cannot find is how a report stops being believed.
    //
    // A third, which only symlinks can break: **nothing outside the tree is
    // touched.** Every trial plants links to a canary directory alongside the
    // tree, and the canary's contents are checked afterwards.
    //
    // Seeded, so a failure here is reproducible rather than a rumour.
    let dir = temp_dir();
    let canary = dir.path().join("canary");
    std::fs::create_dir_all(&canary).expect("the canary");
    std::fs::write(canary.join("precious"), "outside the tree").expect("the canary's file");
    let mut rng = Seeded::new(20260808);

    for trial in 0..60 {
        let root = dir.path().join(format!("tree{trial}"));
        std::fs::create_dir_all(&root).expect("a tree root");
        let mut made = vec![root.clone()];
        for _ in 0..rng.upto(11) {
            let parent = made[rng.upto(made.len())].clone();
            let child = parent.join(format!("d{}", rng.upto(4)));
            let _ = std::fs::create_dir(&child);
            if !made.contains(&child) {
                made.push(child);
            }
        }
        for directory in made.clone() {
            for _ in 0..rng.upto(3) {
                let _ = std::fs::write(directory.join(format!("f{}", rng.upto(4))), "x");
            }
            let roll = rng.upto(100);
            let link = directory.join(format!("l{}", rng.upto(3)));
            if roll < 15 {
                let _ = std::os::unix::fs::symlink(&canary, &link);
            } else if roll < 25 {
                let _ = std::os::unix::fs::symlink(canary.join("precious"), &link);
            } else if roll < 30 {
                let _ = std::os::unix::fs::symlink(dir.path().join("nowhere"), &link);
            }
        }
        // Deepest first: sealing a parent would make sealing its child fail.
        let mut deepest = made.clone();
        deepest.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        let mut sealed = Vec::new();
        for directory in deepest {
            if rng.upto(4) == 0 {
                let mode = [0o000u32, 0o100, 0o300, 0o400, 0o500][rng.upto(5)];
                if let Some(denied) = denying(&directory, mode) {
                    sealed.push(denied);
                }
            }
        }

        let refused = refused_paths(&remove_tree_as_far_as_it_goes(&root));
        let survived = still_there(&root);
        let complaint = format!("trial {trial}: survives={survived} refused={refused:?}");
        // Permissions restored before the assertions, so a failure does not also
        // wreck the temp directory's cleanup.
        drop(sealed);
        assert_eq!(survived, !refused.is_empty(), "{complaint}");
        for path in &refused {
            assert!(
                still_there(path),
                "trial {trial}: reported {}, which is not there",
                path.display()
            );
        }
        let unique: std::collections::HashSet<&PathBuf> = refused.iter().collect();
        assert_eq!(unique.len(), refused.len(), "trial {trial}: duplicates");
        assert_eq!(
            std::fs::read_to_string(canary.join("precious")).expect("the canary"),
            "outside the tree",
            "trial {trial}: a symlink was followed out of the tree"
        );
        let _ = std::process::Command::new("chmod")
            .args(["-R", "u+rwx", &root.display().to_string()])
            .status();
    }
}

/// A directory whose mode this test tightened, restored when this drops.
///
/// `repo_manager`'s `refusing_writes` verifies one specific mode; the randomised
/// trial needs five of them, and needs the restore to happen even when the
/// assertion under it fails.
struct Denying {
    path: PathBuf,
    was: u32,
}

impl Drop for Denying {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.was));
    }
}

fn denying(path: &Path, mode: u32) -> Option<Denying> {
    use std::os::unix::fs::PermissionsExt as _;
    // SAFETY: a bare `geteuid` syscall, which cannot fail and touches nothing.
    if unsafe { libc::geteuid() } == 0 {
        return None;
    }
    let was = std::fs::metadata(path).ok()?.permissions().mode();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).ok()?;
    Some(Denying {
        path: path.to_path_buf(),
        was,
    })
}

/// A tiny deterministic generator, so a failed trial is reproducible.
struct Seeded(u64);

impl Seeded {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn upto(&mut self, bound: usize) -> usize {
        // xorshift64*, which is plenty for choosing directory shapes.
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        if bound == 0 {
            0
        } else {
            (self.0 % bound as u64) as usize
        }
    }
}

// =======================================================================
// purge: only what devlaunch made, and it names what it leaves
// =======================================================================

/// The recorded six-workspace listing, rehomed under `cache_dir`.
///
/// The two foreign workspaces are interleaved with the four clones rather than
/// appended, because a split that happened to keep listing order would pass a
/// test where they were not.
fn six_workspaces(cache_dir: &Path) -> Vec<serde_json::Value> {
    let repos = cache_dir.join("repos").join("blooop");
    vec![
        listed(
            "bencher-test1-mxvm",
            &repos.join("bencher").join("bencher-test1-mxvm"),
        ),
        listed(
            "bencher-main-ii41",
            &repos.join("bencher").join("bencher-main-ii41"),
        ),
        listed("devlaunch", Path::new("/home/dev/projects/devlaunch")),
        listed(
            "devlaunch-main-3j1t",
            &repos.join("devlaunch").join("devlaunch-main-3j1t"),
        ),
        listed(
            "devlaunch-t1-d7bw",
            &repos.join("devlaunch").join("devlaunch-t1-d7bw"),
        ),
        listed(
            "pythontemplate",
            Path::new("/home/dev/projects/python_template"),
        ),
    ]
}

const CLONED_BY_DEVLAUNCH: [&str; 4] = [
    "bencher-test1-mxvm",
    "bencher-main-ii41",
    "devlaunch-main-3j1t",
    "devlaunch-t1-d7bw",
];

/// A cache directory with something in it worth removing.
fn a_cache_directory(dir: &Path) -> PathBuf {
    let cache = dir.join("devlaunch");
    std::fs::create_dir_all(cache.join("repos")).expect("a repos directory");
    std::fs::write(cache.join("completions.json"), "{}").expect("a completion cache");
    cache
}

fn purge(devpod: &Devpod, cache_dir: &Path) -> PurgeOutcome {
    let mut context = CommandContext::new(devpod);
    let plan = purge_plan(&mut context, cache_dir).expect("a plan");
    purge_all_data(&mut context, &plan, None, &mut |_| {}).expect("devpod ran")
}

/// `--purge` does not share `workspace_delete` — it issues its own captured
/// `devpod delete --force` per workspace — so the volumes have to be wired here
/// too rather than inherited. This is the test that says so.
#[test]
fn a_purge_removes_the_volumes_of_every_workspace_it_deleted() {
    let dir = temp_dir();
    let cache = a_cache_directory(dir.path());
    let clone = cache.join("repos").join("o").join("r").join("r-main-aa");
    let devpod = Devpod::new();
    devpod.lists(&[listed("r-main-aa", &clone)]);
    devpod.knows("r-main-aa");
    let home = devpod_home_recording("r-main-aa", "/host/clones/opened-as", "dc9a8b7c");
    let mut context = CommandContext::new(&devpod);
    let plan = purge_plan(&mut context, &cache).expect("a plan");

    purge_all_data(&mut context, &plan, Some(&home), &mut |_| {}).expect("devpod ran");

    assert_eq!(
        devpod.docker_argvs(),
        [[
            "volume",
            "rm",
            "--force",
            "opened-as-pixi",
            "dind-var-lib-docker-dc9a8b7c",
        ]]
    );
}

#[test]
fn a_purge_leaves_the_volumes_of_a_workspace_devpod_would_not_delete() {
    // The container is still there holding them, so removing its volumes would
    // fail anyway — and a purge that reported a removal it never made would be
    // worse than one that says the delete failed and stops there.
    let dir = temp_dir();
    let cache = a_cache_directory(dir.path());
    let clone = cache.join("repos").join("o").join("r").join("r-main-aa");
    let devpod = Devpod::new();
    devpod.lists(&[listed("r-main-aa", &clone)]);
    devpod.fake.script(
        ["devpod", "delete"],
        Response::failed(1, "container is busy\n"),
    );
    // devpod *has* the workspace, so the scripted refusal is the only reason the
    // delete fails: a workspace the fake never heard of would refuse anyway and
    // the test would pass without saying anything.
    devpod.knows("r-main-aa");
    let home = devpod_home_recording("r-main-aa", "/host/clones/opened-as", "dc9a8b7c");
    let mut context = CommandContext::new(&devpod);
    let plan = purge_plan(&mut context, &cache).expect("a plan");

    purge_all_data(&mut context, &plan, Some(&home), &mut |_| {}).expect("devpod ran");

    assert_eq!(devpod.docker_argvs(), Vec::<Vec<String>>::new());
}

#[test]
fn a_purge_says_which_workspaces_volumes_it_could_not_remove() {
    let dir = temp_dir();
    let cache = a_cache_directory(dir.path());
    let clone = cache.join("repos").join("o").join("r").join("r-main-aa");
    let devpod = Devpod::new();
    devpod.lists(&[listed("r-main-aa", &clone)]);
    devpod.fake.script(
        ["docker", "volume", "rm"],
        Response::failed(1, "volume is in use\n"),
    );
    devpod.knows("r-main-aa");
    let home = devpod_home_recording("r-main-aa", "/host/clones/opened-as", "dc9a8b7c");
    let mut steps = Vec::new();
    let mut context = CommandContext::new(&devpod);
    let plan = purge_plan(&mut context, &cache).expect("a plan");

    purge_all_data(&mut context, &plan, Some(&home), &mut |step| {
        steps.push(step)
    })
    .expect("devpod ran");

    assert_eq!(
        steps,
        vec![
            PurgeStep::Deleting {
                workspace_id: "r-main-aa".to_owned(),
            },
            PurgeStep::VolumesNotRemoved {
                workspace_id: "r-main-aa".to_owned(),
                occasion: SweepOccasion::DevpodResult,
                refusal: VolumeRefusal::Docker {
                    exit: Exit::Code(1),
                    stderr: "volume is in use\n".to_owned(),
                },
            },
        ]
    );
}

/// A purge on a machine with no docker is a purge that behaves exactly as it did
/// before this existed: nothing added here may fail on a host that never had a
/// volume to leak.
#[test]
fn a_purge_on_a_machine_with_no_docker_says_nothing_about_it() {
    let dir = temp_dir();
    let cache = a_cache_directory(dir.path());
    let clone = cache.join("repos").join("o").join("r").join("r-main-aa");
    let devpod = Devpod::new();
    devpod.lists(&[listed("r-main-aa", &clone)]);
    devpod.fake.script_missing("docker");
    devpod.knows("r-main-aa");
    let home = devpod_home_recording("r-main-aa", "/host/clones/opened-as", "dc9a8b7c");
    let mut steps = Vec::new();
    let mut context = CommandContext::new(&devpod);
    let plan = purge_plan(&mut context, &cache).expect("a plan");

    let outcome = purge_all_data(&mut context, &plan, Some(&home), &mut |step| {
        steps.push(step)
    })
    .expect("devpod ran");

    assert_eq!(outcome, PurgeOutcome::Removed { cache_dir: cache });
    assert_eq!(
        steps,
        vec![PurgeStep::Deleting {
            workspace_id: "r-main-aa".to_owned(),
        }]
    );
}

#[test]
fn a_purge_deletes_the_clones_devlaunch_made_and_leaves_the_rest() {
    let dir = temp_dir();
    let cache = a_cache_directory(dir.path());
    let devpod = Devpod::new();
    devpod.lists(&six_workspaces(&cache));

    let outcome = purge(&devpod, &cache);

    assert_eq!(devpod.deleted(), CLONED_BY_DEVLAUNCH);
    assert!(!cache.exists(), "the cache goes too");
    assert_eq!(outcome, PurgeOutcome::Removed { cache_dir: cache });
}

#[test]
fn a_purge_asks_devpod_to_delete_with_force_and_nothing_else() {
    // argv-exact. `--force` here is devpod's: the directory the workspace opens
    // is about to be deleted, so a container devpod cannot reach cleanly must
    // not leave a record behind.
    let dir = temp_dir();
    let cache = a_cache_directory(dir.path());
    let clone = cache
        .join("repos")
        .join("blooop")
        .join("r")
        .join("r-main-aa");
    let devpod = Devpod::new();
    devpod.lists(&[listed("r-main-aa", &clone)]);

    purge(&devpod, &cache);

    assert_eq!(
        devpod.devpod_argvs(),
        [
            vec!["list", "--output", "json"],
            vec!["delete", "r-main-aa", "--force"],
        ]
    );
}

#[test]
fn the_plan_counts_only_what_will_be_deleted_and_names_the_survivors() {
    // It used to say six DevPod workspaces and mean six, two of them somebody
    // else's; it now says four and means four — and the survivors are named
    // while saying no is still an option.
    let dir = temp_dir();
    let cache = a_cache_directory(dir.path());
    let devpod = Devpod::new();
    devpod.lists(&six_workspaces(&cache));
    let mut context = CommandContext::new(&devpod);

    let plan = purge_plan(&mut context, &cache).expect("a plan");

    assert_eq!(
        plan.ownership
            .mine
            .iter()
            .map(|it| it.id.as_str())
            .collect::<Vec<_>>(),
        CLONED_BY_DEVLAUNCH
    );
    assert_eq!(
        plan.ownership
            .foreign
            .iter()
            .map(|it| it.id.as_str())
            .collect::<Vec<_>>(),
        ["devlaunch", "pythontemplate"]
    );
}

#[test]
fn pointing_the_cache_elsewhere_makes_a_purge_recognise_nothing() {
    // The scratch-XDG recipe protects `--purge` for real: XDG_CACHE_HOME does not
    // scope `devpod list`, so a scratch run used to see — and delete — every real
    // workspace.
    let dir = temp_dir();
    let real_cache = a_cache_directory(&dir.path().join("real"));
    let scratch = dir.path().join("scratch").join("devlaunch");
    let devpod = Devpod::new();
    devpod.lists(&six_workspaces(&real_cache));

    let outcome = purge(&devpod, &scratch);

    assert_eq!(devpod.deleted(), Vec::<String>::new());
    assert_eq!(outcome, PurgeOutcome::NothingToPurge);
    assert!(real_cache.exists());
}

#[test]
fn a_purge_with_nothing_of_its_own_and_no_cache_has_nothing_to_purge() {
    let dir = temp_dir();
    let devpod = Devpod::new();
    devpod.lists(&[listed(
        "pythontemplate",
        Path::new("/home/dev/projects/python_template"),
    )]);

    let outcome = purge(&devpod, &dir.path().join("never-made"));

    assert_eq!(outcome, PurgeOutcome::NothingToPurge);
    assert_eq!(devpod.deleted(), Vec::<String>::new());
}

#[test]
fn a_purge_that_deleted_workspaces_but_had_no_cache_is_not_nothing_to_purge() {
    // Python reached the same exit code by a branch that printed neither
    // sentence, so the distinction had no representation. Four workspaces went;
    // that is not nothing.
    let dir = temp_dir();
    let cache = dir.path().join("devlaunch");
    let devpod = Devpod::new();
    devpod.lists(&[listed(
        "r-main-aa",
        &cache
            .join("repos")
            .join("blooop")
            .join("r")
            .join("r-main-aa"),
    )]);

    let outcome = purge(&devpod, &cache);

    assert_eq!(outcome, PurgeOutcome::NoCacheDirectory);
    assert_eq!(devpod.deleted(), ["r-main-aa"]);
}

#[test]
fn a_purge_will_not_act_on_a_list_it_could_not_read() {
    // The caller the ticket is named for: a purge that quietly did nothing used
    // to look exactly like a purge that had nothing to do.
    let dir = temp_dir();
    let cache = a_cache_directory(dir.path());
    let devpod = Devpod::new();
    devpod.cannot_list("context not found\n");
    let mut context = CommandContext::new(&devpod);

    let refused = purge_plan(&mut context, &cache);

    match refused {
        Err(ListingUnreadable::Failed { stderr, .. }) => {
            assert!(stderr.contains("context not found"), "{stderr}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(devpod.deleted(), Vec::<String>::new());
    assert!(
        cache.exists(),
        "a purge that could not read the list must not half-run"
    );
}

#[test]
fn a_purge_forgets_the_workspace_list_once_it_has_deleted_something() {
    let dir = temp_dir();
    let cache = a_cache_directory(dir.path());
    let clone = cache
        .join("repos")
        .join("blooop")
        .join("r")
        .join("r-main-aa");
    let devpod = Devpod::new();
    devpod.lists(&[listed("r-main-aa", &clone)]);
    let mut context = CommandContext::new(&devpod);
    let plan = purge_plan(&mut context, &cache).expect("a plan");

    purge_all_data(&mut context, &plan, None, &mut |_| {}).expect("devpod ran");
    devpod.lists(&[]);
    assert_eq!(
        context.workspaces().expect("a listing"),
        Vec::new(),
        "the snapshot the plan was built from must not answer a later read"
    );
}

#[test]
fn a_workspace_devpod_would_not_delete_is_reported_and_the_cache_still_goes() {
    // One failed delete must not cost the rest of the cache its removal — and it
    // must not pass in silence either.
    let dir = temp_dir();
    let cache = a_cache_directory(dir.path());
    let clone = cache
        .join("repos")
        .join("blooop")
        .join("r")
        .join("r-main-aa");
    let devpod = Devpod::new();
    devpod.lists(&[listed("r-main-aa", &clone)]);
    devpod.fake.script(
        ["devpod", "delete"],
        Response::failed(1, "container is busy\n"),
    );
    let mut steps = Vec::new();
    let mut context = CommandContext::new(&devpod);
    let plan = purge_plan(&mut context, &cache).expect("a plan");

    let outcome = purge_all_data(&mut context, &plan, None, &mut |step| steps.push(step))
        .expect("devpod ran");

    assert_eq!(outcome, PurgeOutcome::Removed { cache_dir: cache });
    // The step is the failure's one report — no notice doubles it.
    assert!(matches!(
        steps.as_slice(),
        [
            PurgeStep::Deleting { .. },
            PurgeStep::NotDeleted { workspace_id, stderr, .. },
        ] if workspace_id == "r-main-aa" && stderr.contains("container is busy")
    ));
}

#[test]
fn a_purge_says_which_of_the_three_removals_happened() {
    // devlaunch#182: the exit status deliberately stays two-valued, so the arm is
    // the only place the difference between "one clone stayed" and "nothing
    // moved" is carried.
    let cache = a_sealable_cache();
    let devpod = Devpod::new();
    devpod.lists(&[]);
    let Some(_sealed) = refusing_writes(&cache.stuck) else {
        return;
    };

    let outcome = purge(&devpod, &cache.root);

    let PurgeOutcome::RemovedWhatItCould { refused, .. } = &outcome else {
        panic!("expected a partial removal, got {outcome:?}");
    };
    assert_eq!(
        refused
            .iter()
            .map(|it| it.path.as_path())
            .collect::<Vec<_>>(),
        [cache.stuck.as_path()]
    );
    assert!(
        !outcome.finished(),
        "a clone the user was told would go is still there"
    );
    assert!(
        !cache.metadata.exists(),
        "the partial arm has to mean something went"
    );
}

#[test]
fn a_purge_of_a_symlinked_cache_does_not_report_success() {
    let dir = temp_dir();
    let target = dir.path().join("elsewhere");
    std::fs::create_dir_all(&target).expect("somebody's cache");
    std::fs::write(target.join("metadata.json"), "somebody's cache").expect("their metadata");
    let root = dir.path().join("cache").join("devlaunch");
    std::fs::create_dir_all(root.parent().expect("a parent")).expect("the cache parent");
    std::os::unix::fs::symlink(&target, &root).expect("a symlink");
    let devpod = Devpod::new();
    devpod.lists(&[]);

    let outcome = purge(&devpod, &root);

    assert!(
        matches!(outcome, PurgeOutcome::RemovedNothing { .. }),
        "{outcome:?}"
    );
    assert!(!outcome.finished());
    assert_eq!(
        std::fs::read_to_string(target.join("metadata.json")).expect("their metadata"),
        "somebody's cache"
    );
}

#[test]
fn a_cache_that_cannot_be_looked_at_is_not_mistaken_for_absent() {
    // A cache whose *parent* cannot be traversed used to come out as "No data to
    // purge." and exit 0 with the cache fully intact — a clean sweep reported
    // over untouched data, which is the one failure the whole change prevents.
    let dir = temp_dir();
    let home = dir.path().join("cachehome");
    let root = home.join("devlaunch");
    std::fs::create_dir_all(&root).expect("the cache");
    std::fs::write(root.join("metadata.json"), "still here").expect("a metadata file");
    let devpod = Devpod::new();
    devpod.lists(&[]);
    let Some(_sealed) = refusing_reads(&home) else {
        return;
    };

    let outcome = purge(&devpod, &root);

    assert!(
        matches!(outcome, PurgeOutcome::RemovedNothing { .. }),
        "{outcome:?}"
    );
    assert_ne!(outcome, PurgeOutcome::NothingToPurge);
}

// =======================================================================
// stop and delete: argv-exact, and the clone follows the workspace
// =======================================================================

struct Stopping {
    dir: tempfile::TempDir,
    devpod: Devpod,
    updater: SelfInvocation,
    cache_path: PathBuf,
}

fn a_stopping_world() -> Stopping {
    let dir = temp_dir();
    let cache_path = fresh_cache(dir.path());
    let devpod = Devpod::new();
    devpod.lists(&[]);
    devpod.knows("myws");
    Stopping {
        dir,
        devpod,
        updater: SelfInvocation::new("dl"),
        cache_path,
    }
}

#[test]
fn a_stop_asks_devpod_to_stop_that_workspace_and_nothing_else() {
    let world = a_stopping_world();
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &world.cache_path);

    let outcome = workspace_stop(&mut context, &mut refresh, "myws").expect("devpod ran");

    assert_eq!(outcome, StopOutcome::Stopped);
    assert_eq!(world.devpod.devpod_argvs(), [vec!["stop", "myws"]]);
}

#[test]
fn a_stop_forces_exactly_one_refresh_and_forgets_the_listing() {
    // The cache is wrong regardless of age, and a *stale* cache buys no second
    // sweep: the one refresh a stop gets is the one that runs after the stop.
    let world = a_stopping_world();
    let stale = stale_cache(world.dir.path());
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &stale);

    workspace_stop(&mut context, &mut refresh, "myws").expect("devpod ran");
    workspace_stop(&mut context, &mut refresh, "myws").expect("devpod ran");

    assert_eq!(
        world.devpod.detached(),
        [vec!["dl", "--update-cache", "--force"]]
    );
}

#[test]
fn a_stop_devpod_refused_says_so() {
    let world = a_stopping_world();
    world
        .devpod
        .fake
        .script(["devpod", "stop"], Response::exited(1));
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &world.cache_path);

    assert_eq!(
        workspace_stop(&mut context, &mut refresh, "myws").expect("devpod ran"),
        StopOutcome::DevpodRefused {
            exit: Exit::Code(1)
        }
    );
}

#[test]
fn a_plain_delete_names_the_workspace_and_no_flags() {
    let world = a_stopping_world();
    let mut world_cache = World::empty();
    let clones = clones_for(&world_cache.repos_dir, &world_cache.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &world.cache_path);

    let copies = world_cache.copies();
    let outcome = workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world_cache.storage,
        None,
        &copies,
        "myws",
        Insistence::NotInsisted,
        Persistence::Ordinary,
        &mut unblocked(),
        &mut ignoring(),
    )
    .expect("devpod ran");

    assert_eq!(
        outcome,
        RemoveOutcome::Deleted {
            clone: Ok(Removed::NothingRecorded),
            volumes: VolumeSweep::NothingNamed,
        }
    );
    assert_eq!(world.devpod.devpod_argvs(), [vec!["delete", "myws"]]);
}

#[test]
fn a_forced_delete_passes_devpods_own_ignore_not_found() {
    // `rm -f` semantics: the contract is the state afterwards, not the work done.
    // A cold-launch bench reset runs this before *every* timed run, including the
    // first, where there is nothing to remove yet.
    let world = a_stopping_world();
    let mut world_cache = World::empty();
    let clones = clones_for(&world_cache.repos_dir, &world_cache.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &world.cache_path);

    let copies = world_cache.copies();
    workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world_cache.storage,
        None,
        &copies,
        "myws",
        Insistence::Insisted,
        Persistence::Ordinary,
        &mut unblocked(),
        &mut ignoring(),
    )
    .expect("devpod ran");

    assert_eq!(
        world.devpod.devpod_argvs(),
        [vec!["delete", "myws", "--ignore-not-found"]]
    );
}

/// `kill`'s delete, argv-exact, and it is the flags rather than the count that
/// matter: `--ignore-not-found` is dl's verdict about absence and `--force` is
/// devpod's about reach, and a wedged workspace routinely needs both. This is
/// also the flag dl's own refusal has always told people to type by hand.
#[test]
fn a_wedged_delete_asks_devpod_to_force_it() {
    let world = a_stopping_world();
    let mut world_cache = World::empty();
    let clones = clones_for(&world_cache.repos_dir, &world_cache.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &world.cache_path);

    let copies = world_cache.copies();
    workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world_cache.storage,
        None,
        &copies,
        "myws",
        Insistence::Insisted,
        Persistence::Wedged,
        &mut unblocked(),
        &mut ignoring(),
    )
    .expect("devpod ran");

    assert_eq!(
        world.devpod.devpod_argvs(),
        [vec!["delete", "myws", "--ignore-not-found", "--force"]]
    );
}

/// And it carries a deadline where no other delete does. The asymmetry is the
/// whole robustness claim: `devpod delete` takes the workspace's flock with a
/// *blocking* acquire and nothing behind it, so a holder that arrives between
/// the sweep and the delete would otherwise put `kill` back in the five second
/// loop somebody typed it to escape. `rm`'s delete keeps its patience, because
/// a container that is slow to come down is a container that is coming down.
#[test]
fn only_the_wedged_delete_gives_devpod_a_deadline() {
    assert_eq!(
        deadline_on_a_delete(Persistence::Wedged),
        Some(WEDGED_DELETE)
    );
    assert_eq!(deadline_on_a_delete(Persistence::Ordinary), None);
}

/// The timeout the spawned `devpod delete` actually carried, read off the call
/// the runner recorded rather than off the [`Call`] builder: what is being
/// pinned is that the bound survives the whole path to the child, which is
/// where an earlier version of this could have dropped it silently.
fn deadline_on_a_delete(persistence: Persistence) -> Option<Duration> {
    let world = a_stopping_world();
    let mut world_cache = World::empty();
    let clones = clones_for(&world_cache.repos_dir, &world_cache.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &world.cache_path);

    let copies = world_cache.copies();
    workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world_cache.storage,
        None,
        &copies,
        "myws",
        Insistence::Insisted,
        persistence,
        &mut unblocked(),
        &mut ignoring(),
    )
    .expect("devpod ran");

    match world.devpod.fake.calls().first() {
        Some(devlaunch_test_support::Call::Session(spec)) => spec.timeout,
        other => panic!("the delete was not a passthrough: {other:?}"),
    }
}

/// The failure the whole verb was reached for, and the one shape of it dl
/// could not see. `devpod delete` takes the workspace's flock with a blocking
/// acquire and logs this line every five seconds behind it, forever. It is not
/// a non-zero exit and it is not a timeout on `rm`'s delete, which has no
/// deadline: it is a command that never returns, so nothing downstream of the
/// call can report on it. Reading devpod's stderr as it arrives is the only
/// place the fact exists.
#[test]
fn a_delete_blocked_on_the_workspace_lock_says_so_while_it_is_blocked() {
    let world = a_stopping_world();
    world.devpod.fake.script(
        ["devpod", "delete"],
        Response::exited(0).and_stderr(
            "info Trying to lock workspace, seems like another process is running that \
             blocks this workspace machine_client.go:311\n",
        ),
    );
    let mut world_cache = World::empty();
    let clones = clones_for(&world_cache.repos_dir, &world_cache.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &world.cache_path);
    let mut stalls = 0;

    let copies = world_cache.copies();
    workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world_cache.storage,
        None,
        &copies,
        "myws",
        Insistence::NotInsisted,
        Persistence::Ordinary,
        &mut |DeleteStalled::OnTheLock| stalls += 1,
        &mut ignoring(),
    )
    .expect("devpod ran");

    assert_eq!(stalls, 1);
}

/// And it is said once however long devpod goes on saying it. The line repeats
/// every five seconds for as long as the holder lives, so forwarding each one
/// would bury the advice under the log it is advice about.
#[test]
fn a_delete_that_stays_blocked_says_it_once() {
    let world = a_stopping_world();
    let blocked = "info Trying to lock workspace, seems like another process is running that \
                   blocks this workspace machine_client.go:311\n";
    world.devpod.fake.script(
        ["devpod", "delete"],
        Response::exited(0).and_stderr(format!("{blocked}{blocked}{blocked}")),
    );
    let mut world_cache = World::empty();
    let clones = clones_for(&world_cache.repos_dir, &world_cache.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &world.cache_path);
    let mut stalls = 0;

    let copies = world_cache.copies();
    workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world_cache.storage,
        None,
        &copies,
        "myws",
        Insistence::NotInsisted,
        Persistence::Ordinary,
        &mut |DeleteStalled::OnTheLock| stalls += 1,
        &mut ignoring(),
    )
    .expect("devpod ran");

    assert_eq!(stalls, 1);
}

/// The deadline firing is a devpod that *ran* — for a minute, and was then
/// SIGKILLed by the runner — so it may have got far enough to unlink the
/// workspace record before it went. The two lines that answer for that are
/// marked "Unconditionally" and sit below an early return that this deadline
/// made reachable: before `Persistence::Wedged` there was no timeout on this
/// call, so `NotRun` here could only mean devpod never started.
///
/// Left unfixed, the completion cache goes on offering a workspace that is
/// gone until some later command happens to refresh it.
#[test]
fn a_delete_killed_at_its_deadline_still_invalidates_the_listing() {
    let world = a_stopping_world();
    world
        .devpod
        .fake
        .script(["devpod", "delete"], Response::TimedOut);
    let mut world_cache = World::empty();
    let clones = clones_for(&world_cache.repos_dir, &world_cache.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &world.cache_path);

    let copies = world_cache.copies();
    let outcome = workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world_cache.storage,
        None,
        &copies,
        "myws",
        Insistence::Insisted,
        Persistence::Wedged,
        &mut unblocked(),
        &mut ignoring(),
    );

    assert_eq!(outcome, Err(NotRun::TimedOut));
    assert_eq!(
        world.devpod.detached(),
        [vec!["dl", "--update-cache", "--force"]],
        "the completion cache was left offering a workspace that may be gone"
    );
}

/// And a devpod that never started leaves the listing alone, which is the
/// distinction the arm above turns on: nothing ran, so nothing changed, and a
/// forced refresh would be a background `devpod list` bought for nothing.
#[test]
fn a_delete_devpod_would_not_run_at_all_invalidates_nothing() {
    let world = a_stopping_world();
    world.devpod.fake.script_missing("devpod");
    let mut world_cache = World::empty();
    let clones = clones_for(&world_cache.repos_dir, &world_cache.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&world.updater, &world.cache_path);

    let copies = world_cache.copies();
    let outcome = workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world_cache.storage,
        None,
        &copies,
        "myws",
        Insistence::Insisted,
        Persistence::Wedged,
        &mut unblocked(),
        &mut ignoring(),
    );

    assert_eq!(outcome, Err(NotRun::NotInstalled));
    assert_eq!(world.devpod.detached(), Vec::<Vec<String>>::new());
}

#[test]
fn a_delete_devpod_refused_keeps_the_local_clone() {
    // devpod re-parses the workspace's devcontainer.json to tear the container
    // down, so removing the clone regardless strands the workspace for good.
    let mut world = World::empty();
    let clone = world.clone_at("r-main-aa", "main");
    world.record("r-main-aa", "main", &clone);
    world
        .devpod
        .fake
        .script(["devpod", "delete"], Response::exited(1));
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let updater = SelfInvocation::new("dl");
    let cache_path = fresh_cache(world.tmp());
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&updater, &cache_path);

    let copies = world.copies();
    let outcome = workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world.storage,
        None,
        &copies,
        "r-main-aa",
        Insistence::NotInsisted,
        Persistence::Ordinary,
        &mut unblocked(),
        &mut ignoring(),
    )
    .expect("devpod ran");

    assert_eq!(
        outcome,
        RemoveOutcome::DevpodRefused {
            exit: Exit::Code(1)
        }
    );
    assert!(
        clone.exists(),
        "the clone stays so the delete stays retryable"
    );
    assert_eq!(world.branches_on_record(), ["main"]);
}

#[test]
fn a_delete_devpod_allowed_takes_the_clone_and_its_record() {
    let mut world = World::empty();
    let clone = world.clone_at("r-main-aa", "main");
    world.record("r-main-aa", "main", &clone);
    world.devpod.knows("r-main-aa");
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let updater = SelfInvocation::new("dl");
    let cache_path = fresh_cache(world.tmp());
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&updater, &cache_path);
    let mut notices = Vec::new();

    let copies = world.copies();
    let outcome = workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world.storage,
        None,
        &copies,
        "r-main-aa",
        Insistence::NotInsisted,
        Persistence::Ordinary,
        &mut unblocked(),
        &mut notices,
    )
    .expect("devpod ran");

    assert_eq!(
        outcome,
        RemoveOutcome::Deleted {
            clone: Ok(Removed::Clone),
            volumes: VolumeSweep::NothingNamed,
        }
    );
    assert!(!clone.exists());
    assert_eq!(world.branches_on_record(), Vec::<String>::new());
    assert!(notices.contains(&LifecycleNotice::CloneRemoved {
        workspace_id: "r-main-aa".to_owned()
    }));
}

#[test]
fn a_clone_that_could_not_be_removed_reports_the_refusal_and_not_a_rendering_of_it() {
    // The workspace is gone whatever happened to the clone, so this is a notice
    // and the delete still succeeds. What the notice carries is the refusal
    // itself: a symlinked root has a `points_at` worth naming, and choosing the
    // words for it is the binary's job, not core's.
    let mut world = World::empty();
    let elsewhere = world.tmp().join("moved-clone");
    let clone = world.repo_dir.join("r-main-aa");
    std::fs::create_dir_all(&elsewhere).expect("the real directory");
    std::os::unix::fs::symlink(&elsewhere, &clone).expect("a symlinked clone");
    world.record("r-main-aa", "main", &clone);
    world.devpod.knows("r-main-aa");
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let updater = SelfInvocation::new("dl");
    let cache_path = fresh_cache(world.tmp());
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&updater, &cache_path);
    let mut notices = Vec::new();

    let copies = world.copies();
    let outcome = workspace_delete(
        &mut context,
        &mut refresh,
        &clones,
        &mut world.storage,
        None,
        &copies,
        "r-main-aa",
        Insistence::NotInsisted,
        Persistence::Ordinary,
        &mut unblocked(),
        &mut notices,
    )
    .expect("devpod ran");

    // The removal errored, so the clone outcome is the refusal itself — its own
    // channel now, not a `Removed::Nothing` that could not tell an error from a
    // no-op.
    assert!(
        matches!(
            &outcome,
            RemoveOutcome::Deleted {
                clone: Err(RemoveWorkspaceError::DirectoryLeft(
                    RemoveTreeError::RootIsSymlink { .. }
                )),
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(
        notices,
        vec![LifecycleNotice::CloneNotRemoved {
            workspace_id: "r-main-aa".to_owned(),
            refusal: RemoveWorkspaceError::DirectoryLeft(RemoveTreeError::RootIsSymlink {
                path: clone,
                points_at: Some(elsewhere),
            }),
        }]
    );
}

#[test]
fn a_record_that_could_not_be_dropped_reports_the_step_that_refused() {
    // Python's line is `Could not drop the record for {path}: {e}`, and the
    // `{e}` is the binary's to write: a temp file that could not be made, a
    // lock that could not be taken and a rename that failed read differently
    // to whoever has to fix them, so the notice carries which one it was.
    let mut world = World::empty();
    let clone = world.repo_dir.join("r-main-aa");
    let record = world.record("r-main-aa", "main", &clone);
    let cache = world.cache.clone();
    let Some(_denied) = refusing_writes(&cache) else {
        // Root is refused by nothing, and a mode this filesystem ignores is
        // ordinary on bind and overlay mounts.
        return;
    };
    let mut notices = Vec::new();

    forget_clone(&mut world.storage, &record, &mut notices);

    assert!(
        matches!(
            notices.as_slice(),
            [LifecycleNotice::RecordNotDropped {
                path,
                refusal: metadata::MetadataError::CreateTemp { directory, .. },
            }] if path == &clone && directory == &cache
        ),
        "{notices:?}"
    );
}

// =======================================================================
// delete: the volumes the workspace's devcontainer created
// =======================================================================

/// A world with a recorded clone for `r-main-aa`, ready to be deleted, plus a
/// devpod home recording what devpod substituted into its devcontainer.
///
/// The clone directory and the recorded `LocalWorkspaceFolder` are deliberately
/// *different* leaves: the pixi volume is named after what devpod recorded, and
/// a test that made them the same could not tell the two sources apart.
struct Deleting {
    world: World,
    home: ScratchHome,
    updater: SelfInvocation,
    cache_path: PathBuf,
}

fn a_world_ready_to_delete() -> Deleting {
    let mut world = World::empty();
    let clone = world.clone_at("r-main-aa", "main");
    world.record("r-main-aa", "main", &clone);
    world.devpod.knows("r-main-aa");
    let home = devpod_home_recording("r-main-aa", "/host/clones/opened-as", "dc9a8b7c");
    let cache_path = fresh_cache(world.tmp());
    Deleting {
        world,
        home,
        updater: SelfInvocation::new("dl"),
        cache_path,
    }
}

impl Deleting {
    /// Delete `r-main-aa`, collecting the notices it produced.
    fn delete(&mut self) -> (RemoveOutcome, Vec<LifecycleNotice>) {
        let clones = clones_for(&self.world.repos_dir, &self.world.devpod);
        let mut context = CommandContext::new(&self.world.devpod);
        let mut refresh = Refresh::new(&self.updater, &self.cache_path);
        let mut notices = Vec::new();
        let copies = self.world.copies();
        let outcome = workspace_delete(
            &mut context,
            &mut refresh,
            &clones,
            &mut self.world.storage,
            Some(&self.home),
            &copies,
            "r-main-aa",
            Insistence::NotInsisted,
            Persistence::Ordinary,
            &mut unblocked(),
            &mut notices,
        )
        .expect("devpod ran");
        (outcome, notices)
    }
}

/// **The test that would have caught devlaunch#324.** Nothing in devlaunch ever
/// ran a volume command, so every removal path left both of these behind — 39
/// orphans and 37 GB on the machine the leak was measured on.
#[test]
fn a_delete_removes_both_volumes_the_workspaces_devcontainer_created() {
    let mut deleting = a_world_ready_to_delete();

    let (outcome, notices) = deleting.delete();

    assert_eq!(
        deleting.world.devpod.docker_argvs(),
        [[
            "volume",
            "rm",
            "--force",
            // `${localWorkspaceFolderBasename}-pixi`, from the basename devpod
            // recorded opening — not from the clone directory, which is
            // `r-main-aa`.
            "opened-as-pixi",
            // `dind-var-lib-docker-${devcontainerId}`, from the id devpod
            // recorded deriving.
            "dind-var-lib-docker-dc9a8b7c",
        ]]
    );
    assert_eq!(
        outcome,
        RemoveOutcome::Deleted {
            clone: Ok(Removed::Clone),
            volumes: VolumeSweep::Removed,
        }
    );
    // Silent: a removal that worked has nothing to tell anybody.
    assert!(
        !notices
            .iter()
            .any(|notice| matches!(notice, LifecycleNotice::VolumesNotRemoved { .. })),
        "{notices:?}"
    );
}

/// The order is the fix, not a detail: `devpod delete` takes devpod's record of
/// the workspace away with the workspace, so the names have to be read first.
#[test]
fn the_volumes_are_removed_after_devpod_has_let_go_of_the_workspace() {
    let mut deleting = a_world_ready_to_delete();

    deleting.delete();

    let argvs: Vec<Vec<String>> = deleting
        .world
        .devpod
        .fake
        .calls()
        .into_iter()
        .filter(|call| !matches!(call, devlaunch_test_support::Call::Detach(_)))
        .map(|call| call.argv())
        .collect();
    assert_eq!(
        argvs,
        [
            vec!["devpod", "delete", "r-main-aa"],
            vec![
                "docker",
                "volume",
                "rm",
                "--force",
                "opened-as-pixi",
                "dind-var-lib-docker-dc9a8b7c",
            ],
        ]
    );
}

#[test]
fn a_machine_with_no_docker_still_deletes_the_workspace_and_its_clone_cleanly() {
    // The whole reason the sweep is best-effort: nothing added here may fail on
    // a host that never had a volume to leak.
    let mut deleting = a_world_ready_to_delete();
    deleting.world.devpod.fake.script_missing("docker");
    let clone = deleting.world.repo_dir.join("r-main-aa");

    let (outcome, notices) = deleting.delete();

    assert_eq!(
        outcome,
        RemoveOutcome::Deleted {
            clone: Ok(Removed::Clone),
            volumes: VolumeSweep::NoDocker,
        }
    );
    assert!(!clone.exists(), "the clone went with the workspace");
    assert_eq!(deleting.world.branches_on_record(), Vec::<String>::new());
    // Not a word about docker: this machine never made these volumes. The two
    // lines it *does* say are the clone's, in the order they happened.
    assert_eq!(
        notices,
        vec![
            LifecycleNotice::Cache(CacheNotice::WorkspaceCloneRemoved { path: clone }),
            LifecycleNotice::CloneRemoved {
                workspace_id: "r-main-aa".to_owned(),
            },
        ]
    );
}

#[test]
fn a_docker_that_would_not_remove_them_is_a_notice_and_not_a_failed_delete() {
    // A volume another container still holds is the case this exists for. The
    // workspace is gone regardless, so reporting failure would send the caller
    // looking for a workspace that is not there.
    let mut deleting = a_world_ready_to_delete();
    deleting.world.devpod.fake.script(
        ["docker", "volume", "rm"],
        Response::failed(1, "volume is in use\n"),
    );
    let refusal = VolumeRefusal::Docker {
        exit: Exit::Code(1),
        stderr: "volume is in use\n".to_owned(),
    };

    let (outcome, notices) = deleting.delete();

    assert_eq!(
        outcome,
        RemoveOutcome::Deleted {
            clone: Ok(Removed::Clone),
            volumes: VolumeSweep::Refused(refusal.clone()),
        }
    );
    // The refusal itself, not a rendering of it: docker's words are docker's and
    // the sentence around them is the binary's.
    assert!(
        notices.contains(&LifecycleNotice::VolumesNotRemoved {
            workspace_id: "r-main-aa".to_owned(),
            occasion: SweepOccasion::DevpodResult,
            refusal,
        }),
        "{notices:?}"
    );
}

#[test]
fn a_workspace_whose_up_never_finished_names_nothing_and_runs_no_docker() {
    // devpod writes its create result on the way *out* of a successful `up`, so
    // an `up` that died in its lifecycle hooks leaves the record with no result
    // beside it. Nothing to name is not the same as removing nothing, and it is
    // certainly not a docker call with a made-up name in it.
    let mut deleting = a_world_ready_to_delete();
    deleting.home = devpod_home_with(&[("default", "r-main-aa", None)]);

    let (outcome, notices) = deleting.delete();

    assert_eq!(
        outcome,
        RemoveOutcome::Deleted {
            clone: Ok(Removed::Clone),
            volumes: VolumeSweep::NothingNamed,
        }
    );
    assert_eq!(
        deleting.world.devpod.docker_argvs(),
        Vec::<Vec<String>>::new()
    );
    assert!(
        !notices
            .iter()
            .any(|notice| matches!(notice, LifecycleNotice::VolumesNotRemoved { .. })),
        "{notices:?}"
    );
}

/// One id under two contexts: devpod's ids are unique per context, so a record
/// found twice cannot say which workspace's volumes these are — and guessing
/// would remove the other one's. The ambiguity is the answer, as it is for
/// [`create_record`].
///
/// Both shapes, because the ambiguity is read off the record devpod writes on
/// the way *in* and not off whose `up` finished. Keying it on the create result
/// instead answers with the one context that completed — while `devpod delete`
/// resolves the id against the *current* context, so the volumes named would be
/// the other, living workspace's.
#[test]
fn one_id_in_two_contexts_names_nothing() {
    for second_up_finished in [false, true] {
        let home = devpod_home_recording("myws", "/host/clones/opened-as", "dc9a8b7c");
        let record = home.record("work", "myws");
        std::fs::create_dir_all(record.parent().expect("a parent")).expect("a record directory");
        std::fs::write(&record, "{}").expect("a record");
        if second_up_finished {
            std::fs::write(home.result("work", "myws"), "{}").expect("a create result");
        }

        assert!(
            devcontainer_volumes(Some(&home), "myws").is_none(),
            "second context finished its up: {second_up_finished}"
        );
    }
}

// The two tests that used to sit here — each recorded substitution naming its
// own volume, and a create result of another shape naming nothing — moved with
// the parse itself into `flows::kept_copies`, which is now the one module that
// turns a devpod record into a volume name. There is one parser, so the live
// read at delete time and the kept copy's read agree by construction.

// =======================================================================
// the delete guard
// =======================================================================

fn losses_of(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Stands(standing) => standing
            .would_lose()
            .unwrap_or_else(|| panic!("expected losses, got {standing:?}")),
        other => panic!("expected losses, got {other:?}"),
    }
}

/// A standing built from one proved loss, for the guard's unit tests.
fn stands_holding(losses: workspace_state::Losses) -> Standing {
    Standing::test_of(vec![agent_worktrees::Reason::Holds {
        at: agent_worktrees::Place::TheCloneItself,
        losses: Box::new(losses),
    }])
}

#[test]
fn a_collectable_verdict_is_the_only_answer_that_is_permission() {
    assert_eq!(
        guard_removal("ws", Verdict::test_collectable(), Insistence::NotInsisted),
        Guarded::MayRemove
    );
}

#[test]
fn work_saved_nowhere_else_stops_the_delete_and_names_what_it_is() {
    let standing = stands_holding(workspace_state::Losses::one(
        workspace_state::Loss::Unpushed {
            commits: NonEmpty::one("abc1234 later".to_owned()),
            by_tags: None,
        },
    ));
    let guarded = guard_removal(
        "ws",
        Verdict::Stands(standing.clone()),
        Insistence::NotInsisted,
    );
    assert_eq!(
        guarded,
        Guarded::Refused(RemovalRefused {
            workspace_id: "ws".to_owned(),
            because: refusal_from(&standing)
        })
    );
}

#[test]
fn an_answer_git_would_not_give_stops_the_delete_too() {
    // devlaunch#171: "could not be proved" refuses exactly as "would lose"
    // does. The files are still on disk and nothing has established that
    // they exist anywhere else.
    let cause = workspace_state::CouldNotTell::GitCouldNotRead {
        clone: PathBuf::from("/x"),
        reason: "not a repository".to_owned(),
    };
    let standing = Standing::test_of(vec![agent_worktrees::Reason::CouldNotProve {
        at: agent_worktrees::Place::TheCloneItself,
        blank: agent_worktrees::Blank::GitWouldNotSay(cause),
    }]);
    let guarded = guard_removal(
        "ws",
        Verdict::Stands(standing.clone()),
        Insistence::NotInsisted,
    );
    assert_eq!(
        guarded,
        Guarded::Refused(RemovalRefused {
            workspace_id: "ws".to_owned(),
            because: refusal_from(&standing)
        })
    );
}

#[test]
fn force_gets_past_both_refusals() {
    // The caller who means it is not blocked by dl declining to guess.
    for verdict in [
        Verdict::Stands(stands_holding(workspace_state::Losses::one(
            workspace_state::Loss::Uncommitted(NonEmpty::one("?? scratch.md".to_owned())),
        ))),
        Verdict::test_stands(vec![agent_worktrees::Reason::CouldNotProve {
            at: agent_worktrees::Place::TheCloneItself,
            blank: agent_worktrees::Blank::GitWouldNotSay(
                workspace_state::CouldNotTell::DirectoryUnknown {
                    workspace_id: "ws".to_owned(),
                },
            ),
        }]),
    ] {
        assert_eq!(
            guard_removal("ws", verdict, Insistence::Insisted),
            Guarded::MayRemove
        );
    }
}

/// The guard's answer about `workspace_id`, over a real cache and real git.
fn guard_reads(world: &World, workspace_id: &str) -> Verdict {
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let git = Git::new(&world.devpod);
    unsaved_work_in(
        &clones,
        &world.storage,
        &git,
        &world.cache,
        workspace_id,
        &mut ignoring(),
    )
}

#[test]
fn a_clean_recorded_clone_has_nothing_to_lose() {
    let mut world = World::empty();
    let clone = world.clone_at("r-main-aa", "main");
    world.record("r-main-aa", "main", &clone);
    assert!(matches!(
        guard_reads(&world, "r-main-aa"),
        Verdict::Collectable(_)
    ));
}

#[test]
fn an_unpushed_commit_in_a_recorded_clone_would_be_lost() {
    let mut world = World::empty();
    let clone = world.clone_at("r-main-aa", "main");
    world.record("r-main-aa", "main", &clone);
    std::fs::write(clone.join("more.txt"), "more\n").expect("a file");
    commit(&clone, "more");

    assert!(losses_of(&guard_reads(&world, "r-main-aa")).contains("unpushed commit"));
}

#[test]
fn a_commit_on_a_branch_the_clone_is_not_on_would_be_lost_too() {
    // The reader half of #471: the widened probe has to reach the guard that
    // actually destroys things, not only the function that answers. An agent
    // that committed on a side branch and checked the main one back out leaves
    // exactly this clone, and `rm` used to take it without asking.
    let mut world = World::empty();
    let clone = world.clone_at("r-main-aa", "main");
    world.record("r-main-aa", "main", &clone);
    run_git(&clone, &["checkout", "-b", "wip"]);
    std::fs::write(clone.join("wip.txt"), "an hour of work\n").expect("a file");
    commit(&clone, "wip");
    run_git(&clone, &["checkout", "main"]);

    assert!(losses_of(&guard_reads(&world, "r-main-aa")).contains("unpushed commit"));
}

#[test]
fn a_clone_dl_has_no_record_for_answers_nothing_to_lose() {
    // What the guard does *not* cover, pinned so no README can overstate it. A
    // clone under dl's cache with no metadata record: `--ls --json` reports what
    // it holds, but `rm` does not refuse, because "dl has no record of a clone
    // here" is `NothingToLose`. No work is destroyed — the delete reads the same
    // absent record and removes nothing — but it is not a refusal either.
    let world = World::empty();
    let clone = world.clone_at("r-main-aa", "main");
    std::fs::write(clone.join("an-hour-of-work.md"), "half a plan\n").expect("their work");

    assert!(matches!(
        guard_reads(&world, "r-main-aa"),
        Verdict::Collectable(_)
    ));
    assert!(clone.join("an-hour-of-work.md").exists());
}

#[test]
fn a_stale_record_does_not_let_the_delete_past_the_guard() {
    // devlaunch#174, at the surface it destroys things from. The guard read the
    // recorded path; the delete fell back to the derived one when that path was
    // not on disk. So a record pointing somewhere stale had the guard answering
    // `NothingToLose` about an absent directory — correctly, nothing absent holds
    // anything — while the delete removed the derived directory, which held an
    // unpushed commit. Exit 0, no `--force`, nothing logged.
    //
    // The only assertion that pins it is one where those two would differ.
    let mut world = World::empty();
    let derived_id = WorkspaceId::new(OWNER, REPO, "feature")
        .expect("a safe triple")
        .value()
        .to_owned();
    let derived = world.clone_at(&derived_id, "feature");
    std::fs::write(derived.join("more.txt"), "more\n").expect("a file");
    commit(&derived, "more");
    let stale = world.repo_dir.join("moved-away");
    assert!(!stale.exists(), "the premise is that the record is stale");
    world.record("r-feature-aaa", "feature", &stale);

    assert!(
        losses_of(&guard_reads(&world, "r-feature-aaa")).contains("unpushed commit"),
        "the guard has to inspect the directory the delete will remove"
    );
}

#[test]
fn a_record_no_directory_can_be_derived_from_stops_the_delete() {
    // A record holding a ref the id validator refuses — a hand-edited or
    // truncated metadata.json — resolves to no directory at all. That is not
    // `NothingToLose`: dl has established nothing about it, which is
    // devlaunch#171's rule one layer further out.
    let mut world = World::empty();
    let stale = world.repo_dir.join("not-on-disk");
    world.record("r-evil-aaa", "--evil", &stale);

    let verdict = guard_reads(&world, "r-evil-aaa");

    let Verdict::Stands(standing) = &verdict else {
        panic!("expected a refusal, got {verdict:?}");
    };
    let words = standing.could_not_tell().expect("an unproved, not a loss");
    assert!(words.contains("r-evil-aaa"), "{words}");
}

#[test]
fn a_clone_git_cannot_read_stops_the_delete_too() {
    // devlaunch#171 itself. A directory that is there and is not a repository git
    // can read holds whatever files are in it, and with no repository to consult
    // nothing has established that they exist anywhere else.
    let mut world = World::empty();
    let broken = world.repo_dir.join("r-broken-aa");
    std::fs::create_dir_all(broken.join(".git")).expect("a directory that is not a clone");
    std::fs::write(broken.join("scratch.md"), "an agent's notes\n").expect("their work");
    world.record("r-broken-aa", "broken", &broken);

    let verdict = guard_reads(&world, "r-broken-aa");

    let Verdict::Stands(standing) = &verdict else {
        panic!("expected a refusal, got {verdict:?}");
    };
    assert!(standing.could_not_tell().is_some(), "{standing:?}");
    assert!(broken.join("scratch.md").exists());
}

#[test]
fn a_clone_already_removed_by_hand_is_still_deletable() {
    // The reason the "not there" arm answers `NothingToLose` rather than
    // refusing: clearing up after a half-finished delete must not need `--force`.
    let mut world = World::empty();
    let clone = world.clone_at("r-main-aa", "main");
    world.record("r-main-aa", "main", &clone);
    std::fs::remove_dir_all(&clone).expect("removed by hand");

    assert!(matches!(
        guard_reads(&world, "r-main-aa"),
        Verdict::Collectable(_)
    ));
}

// =======================================================================
// the detached refresh child
// =======================================================================

#[test]
fn a_fresh_cache_costs_no_subprocess() {
    let dir = temp_dir();
    let cache = fresh_cache(dir.path());
    let devpod = Devpod::new();
    let updater = SelfInvocation::new("dl");
    let mut refresh = Refresh::new(&updater, &cache);

    assert_eq!(
        refresh.ask(&devpod, RefreshReason::IfStale),
        RefreshSpawn::CacheStillFresh
    );
    assert_eq!(devpod.detached(), Vec::<Vec<String>>::new());
    assert!(!refresh.spawned());
}

#[test]
fn a_stale_cache_spawns_one_refresh_with_the_update_cache_flag() {
    let dir = temp_dir();
    let cache = stale_cache(dir.path());
    let devpod = Devpod::new();
    let updater = SelfInvocation::new("dl");
    let mut refresh = Refresh::new(&updater, &cache);

    assert!(matches!(
        refresh.ask(&devpod, RefreshReason::IfStale),
        RefreshSpawn::Spawned { .. }
    ));
    assert_eq!(devpod.detached(), [vec!["dl", "--update-cache"]]);
}

#[test]
fn a_cache_that_is_not_there_at_all_spawns_a_refresh() {
    let dir = temp_dir();
    let devpod = Devpod::new();
    let updater = SelfInvocation::new("dl");
    let never_written = dir.path().join("never-written.json");
    let mut refresh = Refresh::new(&updater, &never_written);

    assert!(matches!(
        refresh.ask(&devpod, RefreshReason::IfStale),
        RefreshSpawn::Spawned { .. }
    ));
}

#[test]
fn a_forced_refresh_ignores_the_ttl_and_tells_the_child_so() {
    let dir = temp_dir();
    let cache = fresh_cache(dir.path());
    let devpod = Devpod::new();
    let updater = SelfInvocation::new("dl");
    let mut refresh = Refresh::new(&updater, &cache);

    refresh.ask(&devpod, RefreshReason::Forced);

    assert_eq!(devpod.detached(), [vec!["dl", "--update-cache", "--force"]]);
}

#[test]
fn only_one_refresh_is_spawned_per_command() {
    let dir = temp_dir();
    let cache = stale_cache(dir.path());
    let devpod = Devpod::new();
    let updater = SelfInvocation::new("dl");
    let mut refresh = Refresh::new(&updater, &cache);

    refresh.ask(&devpod, RefreshReason::IfStale);
    assert_eq!(
        refresh.ask(&devpod, RefreshReason::IfStale),
        RefreshSpawn::AlreadySpawned
    );
    assert_eq!(
        refresh.ask(&devpod, RefreshReason::Forced),
        RefreshSpawn::AlreadySpawned
    );
    assert_eq!(devpod.detached().len(), 1);
}

#[test]
fn skipping_on_freshness_does_not_use_up_the_one_spawn() {
    // A TTL skip means "not needed yet", not "already done".
    let dir = temp_dir();
    let cache = fresh_cache(dir.path());
    let devpod = Devpod::new();
    let updater = SelfInvocation::new("dl");
    let mut refresh = Refresh::new(&updater, &cache);

    refresh.ask(&devpod, RefreshReason::IfStale);
    refresh.ask(&devpod, RefreshReason::Forced);

    assert_eq!(devpod.detached().len(), 1);
}

#[test]
fn two_commands_do_not_share_one_refresh_latch() {
    // Python held it in a module-level dict and reset it in `main()`; a second
    // command is a second value here.
    let dir = temp_dir();
    let cache = fresh_cache(dir.path());
    let devpod = Devpod::new();
    let updater = SelfInvocation::new("dl");

    Refresh::new(&updater, &cache).ask(&devpod, RefreshReason::Forced);
    Refresh::new(&updater, &cache).ask(&devpod, RefreshReason::Forced);

    assert_eq!(devpod.detached().len(), 2);
}

#[test]
fn a_spawn_the_os_refuses_is_survivable_and_not_retried() {
    let dir = temp_dir();
    let cache = stale_cache(dir.path());
    let devpod = Devpod::new();
    devpod.fake.script_missing("dl");
    let updater = SelfInvocation::new("dl");
    let mut refresh = Refresh::new(&updater, &cache);

    assert_eq!(
        refresh.ask(&devpod, RefreshReason::IfStale),
        RefreshSpawn::NotStarted(SpawnRefused::ProgramNotFound)
    );
    assert_eq!(
        refresh.ask(&devpod, RefreshReason::Forced),
        RefreshSpawn::AlreadySpawned,
        "whatever refused the fork will refuse the next one too"
    );
}

#[test]
fn the_child_argv_is_whatever_the_binary_says_it_is() {
    // Core never asks the OS who it is: `current_exe()` inside a library answers
    // `wf` when wf links it. Python's build spells its own re-invocation
    // `[sys.executable, "-m", "devlaunch.dl", "--update-cache"]`, which the
    // leading arguments are for.
    let python = SelfInvocation::new("/usr/bin/python3").with_leading_args(["-m", "devlaunch.dl"]);
    assert_eq!(
        python.refresh_child(RefreshReason::IfStale).argv(),
        ["/usr/bin/python3", "-m", "devlaunch.dl", "--update-cache"]
    );
    assert_eq!(
        python.refresh_child(RefreshReason::Forced).argv(),
        [
            "/usr/bin/python3",
            "-m",
            "devlaunch.dl",
            "--update-cache",
            "--force"
        ]
    );
}

#[test]
fn the_refresh_child_rechecks_freshness_for_itself() {
    // Two parents can both see a stale cache before either child has written one,
    // and the second sweep would be pure waste.
    let dir = temp_dir();
    let fresh = fresh_cache(dir.path());
    assert_eq!(
        child_work(&fresh, RefreshReason::IfStale),
        ChildWork::NothingToDo
    );
    assert_eq!(
        child_work(&fresh, RefreshReason::Forced),
        ChildWork::RefreshAndSweep,
        "a forced refresh follows a workspace change: age says nothing about it"
    );
    let stale = stale_cache(dir.path());
    assert_eq!(
        child_work(&stale, RefreshReason::IfStale),
        ChildWork::RefreshAndSweep
    );
}

// =======================================================================
// the fetch sweep
// =======================================================================

/// `git fetch origin +refs/heads/*:refs/heads/* +refs/tags/*:refs/tags/* --prune`
/// — the broad sweep, and the sweep's alone.
const BROAD_FETCH: [&str; 6] = [
    "git",
    "fetch",
    "origin",
    "+refs/heads/*:refs/heads/*",
    "+refs/tags/*:refs/tags/*",
    "--prune",
];

fn hours_ago(hours: i64) -> Timestamp {
    let now = jiff::Zoned::now();
    let then = now
        .checked_sub(jiff::Span::new().hours(hours))
        .expect("a time two hours ago");
    Timestamp::from_civil(then.datetime())
}

/// A world whose git calls are faked, so a fetch's argv is observable without a
/// remote to reach.
struct Sweeping {
    /// Held for its `Drop`: the whole world below lives under it.
    _dir: tempfile::TempDir,
    repos_dir: PathBuf,
    bare: PathBuf,
    storage: MetadataStorage,
    fake: FakeRunner,
}

fn a_sweeping_cache() -> Sweeping {
    let dir = temp_dir();
    let repos_dir = dir.path().join("repos");
    let bare = bare_dir(&repos_dir, OWNER, REPO);
    std::fs::create_dir_all(&bare).expect("the bare directory");
    std::fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").expect("a HEAD");
    let (storage, _) =
        MetadataStorage::open(dir.path().join("metadata.json")).expect("a metadata store");
    Sweeping {
        _dir: dir,
        repos_dir,
        bare,
        storage,
        fake: FakeRunner::new(),
    }
}

impl Sweeping {
    fn recorded(&mut self, owner: &str, repo: &str, last_fetched: Option<Timestamp>) -> PathBuf {
        let bare = bare_dir(&self.repos_dir, owner, repo);
        std::fs::create_dir_all(&bare).expect("the bare directory");
        std::fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").expect("a HEAD");
        let mut recorded = BaseRepository::new(
            owner,
            repo,
            &format!("https://github.com/{owner}/{repo}.git"),
            bare.clone(),
        );
        recorded.last_fetched = last_fetched;
        self.storage.add_repository(recorded).expect("recorded");
        bare
    }

    fn last_fetched(&self, owner: &str, repo: &str) -> Option<Timestamp> {
        self.storage
            .get_repository(owner, repo)
            .expect("a record")
            .last_fetched
            .clone()
    }

    /// What the record says the last sweep of this repository had to say.
    fn last_sweep(&self, owner: &str, repo: &str) -> Option<SweepNote> {
        self.storage
            .get_repository(owner, repo)
            .expect("a record")
            .last_sweep
            .clone()
    }

    fn fetches(&self) -> Vec<devlaunch_test_support::Call> {
        self.fake
            .calls()
            .into_iter()
            .filter(|call| call.args().first().map(String::as_str) == Some("fetch"))
            .collect()
    }
}

#[test]
fn a_repository_past_its_interval_gets_the_broad_fetch_under_a_deadline() {
    let mut cache = a_sweeping_cache();
    cache.recorded(OWNER, REPO, Some(hours_ago(2)));
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    let report = sweep_repo_fetches(&manager, &mut cache.storage);

    assert_eq!(
        report.repos,
        [SweptRepo::Fetched {
            owner: OWNER.to_owned(),
            repo: REPO.to_owned()
        }]
    );
    let fetches = cache.fetches();
    assert_eq!(fetches.len(), 1, "{fetches:?}");
    let devlaunch_test_support::Call::Capture(spec) = &fetches[0] else {
        panic!("a fetch is captured, not inherited: {:?}", fetches[0]);
    };
    assert_eq!(spec.invocation.argv(), BROAD_FETCH);
    assert_eq!(spec.invocation.cwd.as_deref(), Some(cache.bare.as_path()));
    assert_eq!(
        spec.timeout,
        Some(BACKGROUND_FETCH_TIMEOUT),
        "the fetch the sweep runs under the lock is given a deadline"
    );
}

#[test]
fn fetching_advances_the_shared_fetch_clock() {
    // `last_fetched` is shared with the launch path, so a sweep is what stops a
    // launch reaching for the same fetch a second time.
    let mut cache = a_sweeping_cache();
    let stale = hours_ago(2);
    cache.recorded(OWNER, REPO, Some(stale.clone()));
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    sweep_repo_fetches(&manager, &mut cache.storage);

    assert_ne!(cache.last_fetched(OWNER, REPO), Some(stale));
}

#[test]
fn a_repository_within_its_interval_is_left_alone() {
    // The interval is the whole point: this is not a fetch-every-command.
    let mut cache = a_sweeping_cache();
    cache.recorded(OWNER, REPO, Some(Timestamp::now()));
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    let report = sweep_repo_fetches(&manager, &mut cache.storage);

    assert_eq!(
        report.repos,
        [SweptRepo::NotDue {
            owner: OWNER.to_owned(),
            repo: REPO.to_owned()
        }]
    );
    assert_eq!(cache.fetches().len(), 0);
}

#[test]
fn a_repository_another_run_is_holding_is_skipped_rather_than_queued_for() {
    // A launch holds the repo lock while it clones. The sweep must neither wait
    // for it nor fetch behind its back — it comes back next hour.
    let mut cache = a_sweeping_cache();
    let stale = hours_ago(2);
    cache.recorded(OWNER, REPO, Some(stale.clone()));
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));
    let lock_path = manager.lock_path(OWNER, REPO);
    let held = locks::hold_lock(&lock_path).expect("the lock");

    let report = sweep_repo_fetches(&manager, &mut cache.storage);
    drop(held);

    assert_eq!(
        report.repos,
        [SweptRepo::Contended {
            owner: OWNER.to_owned(),
            repo: REPO.to_owned()
        }]
    );
    assert_eq!(cache.fetches().len(), 0);
    assert_eq!(
        cache.last_fetched(OWNER, REPO),
        Some(stale),
        "nothing was fetched, so nothing may claim it was"
    );
}

#[test]
fn the_prune_scan_blocks_on_a_repository_another_process_is_holding() {
    // `dl --prune`'s scan takes each repo's lock, blocking, before it weighs
    // the clones under it (prune_plan → hold_repo_lock), so it never walks a
    // directory a cold launch is still cloning into. Unlike the hourly sweep —
    // which declines a held lock (`run_if_lock_free`) and comes back later —
    // the scan must WAIT, the way Python's
    // test_it_blocks_while_another_process_holds_the_repository_lock proves.
    // Pinned at the acquisition itself, against a real second process, because
    // driving prune_plan across a thread would carry the fake runner (which is
    // neither Send nor free of the process-global timing lock).
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    const BOUND: Duration = Duration::from_secs(10);

    fn spawn_holder(lock: &Path) -> Child {
        Command::new("sh")
            .arg("-c")
            .arg(r#"exec 9>"$1"; flock --exclusive 9; exec sleep 300"#)
            .arg("sh")
            .arg(lock)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a shell and flock(1) from util-linux")
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the condition never held within {BOUND:?}");
    }

    let cache = a_sweeping_cache();
    let repos_dir = cache.repos_dir.clone();
    let lock_path = {
        let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));
        manager.lock_path(OWNER, REPO)
    };
    std::fs::create_dir_all(lock_path.parent().expect("a parent")).expect("the repo directory");

    let mut holder = spawn_holder(&lock_path);
    wait_until(|| {
        locks::run_if_lock_free(&lock_path, || ())
            .expect("no lock error")
            .is_none()
    });

    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        // A fresh manager over the same repos_dir; the acquisition is the scan's.
        let runner = ProcessRunner::new();
        let manager = RepositoryManager::new(&repos_dir, Git::new(&runner));
        let _lock = manager.hold_repo_lock(OWNER, REPO).expect("acquired");
        tx.send(()).expect("the parent listens");
    });

    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "the scan acquired the repo lock while another process held it"
    );

    holder.kill().expect("kill the holder");
    holder.wait().expect("reap the holder");

    rx.recv_timeout(BOUND)
        .expect("the scan acquired the lock once it was free");
    worker.join().expect("the worker finished");
}

#[test]
fn one_bad_repository_does_not_cost_the_next_one_its_refresh() {
    // A detached child has nobody to complain to, so it complains to nobody — and
    // a failure that ended the loop would give the first slow remote every other
    // repository's refresh.
    let mut cache = a_sweeping_cache();
    let stale = hours_ago(2);
    let first = cache.recorded(OWNER, "first", Some(stale.clone()));
    cache.recorded(OWNER, "second", Some(stale.clone()));
    cache.fake.script(
        ["git", "fetch"],
        Response::failed(128, "fatal: no route to host\n"),
    );
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    let report = sweep_repo_fetches(&manager, &mut cache.storage);

    assert_eq!(
        cache.fetches().len(),
        2,
        "the second repository was still swept"
    );
    assert!(
        report
            .repos
            .iter()
            .all(|swept| matches!(swept, SweptRepo::Failed { .. })),
        "{:?}",
        report.repos
    );
    assert_eq!(
        cache.last_fetched(OWNER, "first"),
        Some(stale.clone()),
        "a fetch that failed must not claim the clock"
    );
    assert_eq!(cache.last_fetched(OWNER, "second"), Some(stale));
    assert!(first.exists(), "the clone is left where it is");
}

#[test]
fn a_fetch_that_hits_its_deadline_is_one_more_thing_to_step_over() {
    let mut cache = a_sweeping_cache();
    cache.recorded(OWNER, "first", Some(hours_ago(2)));
    cache.recorded(OWNER, "second", Some(hours_ago(2)));
    cache.fake.script(["git", "fetch"], Response::TimedOut);
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    let report = sweep_repo_fetches(&manager, &mut cache.storage);

    assert_eq!(cache.fetches().len(), 2);
    assert!(
        report.repos.iter().all(|swept| matches!(
            swept,
            SweptRepo::Failed {
                error: LazyFetchError::Fetch(
                    crate::flows::repo_manager::FetchRepoError::TimedOut { .. }
                ),
                ..
            }
        )),
        "{:?}",
        report.repos
    );
}

#[test]
fn a_cache_with_no_repositories_sweeps_nothing() {
    let mut cache = a_sweeping_cache();
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));
    let report = sweep_repo_fetches(&manager, &mut cache.storage);
    assert!(report.repos.is_empty());
    assert_eq!(cache.fetches().len(), 0);
}

// -----------------------------------------------------------------------
// what the sweep leaves in the record (devlaunch#480)
// -----------------------------------------------------------------------

#[test]
fn a_pack_that_refused_is_left_in_the_record_in_gits_own_words() {
    // The notice this reads was already raised (#470) and already went nowhere:
    // the sweep is a detached child with all three descriptors on /dev/null. The
    // record is where it survives to be read.
    let mut cache = a_sweeping_cache();
    cache.recorded(OWNER, REPO, Some(hours_ago(2)));
    cache.fake.script(
        ["git", "pack-refs"],
        Response::failed(
            1,
            "fatal: unable to create 'packed-refs.lock': Permission denied\n",
        ),
    );
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    let report = sweep_repo_fetches(&manager, &mut cache.storage);

    assert_eq!(
        report.repos,
        [SweptRepo::Fetched {
            owner: OWNER.to_owned(),
            repo: REPO.to_owned()
        }],
        "a pack that refused is not a fetch that failed"
    );
    assert_eq!(
        cache.last_sweep(OWNER, REPO),
        Some(SweepNote {
            trouble: SweepTrouble::RefsNotPacked,
            said: Some("fatal: unable to create 'packed-refs.lock': Permission denied".to_owned()),
        })
    );
    assert!(
        cache.last_fetched(OWNER, REPO).is_some(),
        "the stamp is about the fetch, and the fetch happened"
    );
}

#[test]
fn a_sweep_that_went_cleanly_takes_the_last_ones_note_away() {
    // Overwritten, not accumulated: one note per repository is what makes the
    // record able to hold this at all, and a cache whose trouble has been fixed
    // has to stop complaining without anybody clearing it by hand.
    let mut cache = a_sweeping_cache();
    cache.recorded(OWNER, REPO, Some(hours_ago(2)));
    let (_written, _) = cache
        .storage
        .update_repository(OWNER, REPO, |recorded| {
            recorded.last_sweep = Some(SweepNote {
                trouble: SweepTrouble::RefsNotPacked,
                said: Some("something that no longer happens".to_owned()),
            });
        })
        .expect("a note from the pass before");
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    sweep_repo_fetches(&manager, &mut cache.storage);

    assert_eq!(cache.last_sweep(OWNER, REPO), None);
}

#[test]
fn a_pass_that_attempted_nothing_leaves_the_last_note_standing() {
    // Three ways to attempt nothing and one rule for all of them: silence about
    // a repository this pass never touched must not read as a clean sweep of it.
    // Here the interval has not elapsed; contended and lock-unavailable take the
    // same arm.
    let mut cache = a_sweeping_cache();
    cache.recorded(OWNER, REPO, Some(Timestamp::now()));
    let outstanding = SweepNote {
        trouble: SweepTrouble::FetchRefused,
        said: Some("fatal: no route to host".to_owned()),
    };
    let (_written, _) = cache
        .storage
        .update_repository(OWNER, REPO, |recorded| {
            recorded.last_sweep = Some(outstanding.clone());
        })
        .expect("a note from the pass before");
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    let report = sweep_repo_fetches(&manager, &mut cache.storage);

    assert_eq!(
        report.repos,
        [SweptRepo::NotDue {
            owner: OWNER.to_owned(),
            repo: REPO.to_owned()
        }]
    );
    assert_eq!(cache.last_sweep(OWNER, REPO), Some(outstanding));
}

#[test]
fn a_fetch_that_failed_leaves_what_git_said_about_it() {
    let mut cache = a_sweeping_cache();
    cache.recorded(OWNER, REPO, Some(hours_ago(2)));
    cache.fake.script(
        ["git", "fetch"],
        Response::failed(128, "fatal: no route to host\n"),
    );
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    sweep_repo_fetches(&manager, &mut cache.storage);

    assert_eq!(
        cache.last_sweep(OWNER, REPO),
        Some(SweepNote {
            trouble: SweepTrouble::FetchRefused,
            said: Some("fatal: no route to host".to_owned()),
        })
    );
}

#[test]
fn a_fetch_killed_at_the_bound_says_so_and_quotes_nobody() {
    // Nothing spoke: the child was killed by dl, so `said` is None rather than
    // an empty string standing in for words that were never written.
    let mut cache = a_sweeping_cache();
    cache.recorded(OWNER, REPO, Some(hours_ago(2)));
    cache.fake.script(["git", "fetch"], Response::TimedOut);
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    sweep_repo_fetches(&manager, &mut cache.storage);

    assert_eq!(
        cache.last_sweep(OWNER, REPO),
        Some(SweepNote {
            trouble: SweepTrouble::FetchTimedOut,
            said: None,
        })
    );
}

#[test]
fn each_repository_gets_its_own_note_out_of_the_one_pass() {
    // The sweep walks a whole cache in one detached pass, so a note that landed
    // on the wrong record would be worse than none: it would name a repository
    // that is fine and clear the one that is not. Two repositories, two
    // different troubles, one pass.
    let mut cache = a_sweeping_cache();
    cache.recorded(OWNER, "first", Some(hours_ago(2)));
    let gone = cache.recorded(OWNER, "second", Some(hours_ago(2)));
    std::fs::remove_dir_all(&gone).expect("the second clone is deleted under the record");
    cache.fake.script(
        ["git", "pack-refs"],
        Response::failed(1, "fatal: refusing to pack\n"),
    );
    let manager = RepositoryManager::new(&cache.repos_dir, Git::new(&cache.fake));

    sweep_repo_fetches(&manager, &mut cache.storage);

    assert_eq!(
        cache.last_sweep(OWNER, "first"),
        Some(SweepNote {
            trouble: SweepTrouble::RefsNotPacked,
            said: Some("fatal: refusing to pack".to_owned()),
        })
    );
    assert_eq!(
        cache.last_sweep(OWNER, "second"),
        Some(SweepNote {
            trouble: SweepTrouble::CloneMissing,
            said: None,
        }),
        "the repository whose clone is gone never reached a pack to refuse"
    );
}

// =======================================================================
// which workspace a triple is (devlaunch#88, #145)
// =======================================================================

/// The triple these tests resolve, parsed.
fn a_triple(branch: &str) -> WorkspaceId {
    WorkspaceId::new(OWNER, REPO, branch).expect("a safe triple")
}

#[test]
fn the_id_probed_is_the_one_the_triple_itself_derives() {
    // The derived id used to arrive as a second argument beside the triple, so
    // this function could be handed a triple and an id that were not each
    // other's -- and it goes on to *name both* in the notice it emits when they
    // differ, which is a sentence about a workspace nobody has. There is one
    // argument now, and the id is read off it.
    let workspace = a_triple("main");
    let devpod = devpod_knowing(&[(workspace.value(), "Running")]);

    let resolved = resolve_known_workspace(
        &devpod,
        &workspace,
        || None,
        &mut ignoring(),
        Patience::AsLongAsItTakes,
    );

    assert_eq!(
        resolved,
        Ok(KnownWorkspace::Known {
            workspace_id: "r-main-znkz".to_owned(),
            state: ContainerState::Running,
        })
    );
    assert_eq!(workspace.value(), "r-main-znkz");
}

/// A devpod that knows exactly these workspaces, in these states.
fn devpod_knowing(known: &[(&str, &str)]) -> FakeRunner {
    let fake = FakeRunner::new();
    for (id, state) in known {
        fake.script(
            ["devpod", "status", id],
            Response::stdout(format!("{{\"state\": \"{state}\"}}")),
        );
    }
    fake.script(
        ["devpod", "status"],
        Response::failed(1, "workspace not found\n"),
    );
    fake
}

#[test]
fn a_workspace_devpod_knows_answers_from_the_derivation_and_reads_no_record() {
    // #145's promise: a launch of a workspace devpod already knows must not load
    // metadata.json. Here the closure is the record read, and it is never called.
    let workspace = a_triple("main");
    let devpod = devpod_knowing(&[(workspace.value(), "Running")]);
    let mut asked = false;

    let resolved = resolve_known_workspace(
        &devpod,
        &workspace,
        || {
            asked = true;
            Some("something-else".to_owned())
        },
        &mut ignoring(),
        Patience::AsLongAsItTakes,
    );

    assert_eq!(
        resolved,
        Ok(KnownWorkspace::Known {
            workspace_id: workspace.value().to_owned(),
            state: ContainerState::Running
        })
    );
    assert!(!asked, "the warm path reads no metadata");
}

#[test]
fn a_workspace_created_under_the_old_scheme_is_still_addressable() {
    // The regression PR #81 caused: the record was written by a dl whose
    // derivation produced a different id, and following the derivation reaches a
    // workspace devpod has never heard of.
    let workspace = a_triple("main");
    let devpod = devpod_knowing(&[("r-main-old", "Stopped")]);
    let mut notices = Vec::new();

    let resolved = resolve_known_workspace(
        &devpod,
        &workspace,
        || Some("r-main-old".to_owned()),
        &mut notices,
        Patience::AsLongAsItTakes,
    );

    assert_eq!(
        resolved,
        Ok(KnownWorkspace::Known {
            workspace_id: "r-main-old".to_owned(),
            state: ContainerState::Stopped
        })
    );
    // The sentence names the recorded id, the derived id and the triple, and the
    // derived id is now read off the triple rather than handed in beside it -- so
    // the three cannot be about two different workspaces.
    assert_eq!(
        notices,
        [LifecycleNotice::AddressingRecordedWorkspace {
            recorded: "r-main-old".to_owned(),
            derived: workspace.value().to_owned(),
            owner: OWNER.to_owned(),
            repo: REPO.to_owned(),
            branch: "main".to_owned(),
        }]
    );
}

#[test]
fn a_record_that_agrees_with_the_derivation_changes_nothing() {
    let workspace = a_triple("main");
    let devpod = devpod_knowing(&[]);
    let resolved = resolve_known_workspace(
        &devpod,
        &workspace,
        || Some(workspace.value().to_owned()),
        &mut ignoring(),
        Patience::AsLongAsItTakes,
    );
    assert_eq!(
        resolved,
        Ok(KnownWorkspace::Unknown {
            derived: workspace.value().to_owned()
        })
    );
}

#[test]
fn a_stored_id_devpod_also_denies_is_not_used() {
    // metadata.json is append-mostly, so a record naming a workspace deleted
    // months ago is ordinary. The answer has to be the derived id — the one a
    // create would use — not a workspace that is doubly gone.
    let workspace = a_triple("main");
    let devpod = devpod_knowing(&[]);
    let resolved = resolve_known_workspace(
        &devpod,
        &workspace,
        || Some("r-main-old".to_owned()),
        &mut ignoring(),
        Patience::AsLongAsItTakes,
    );
    assert_eq!(
        resolved,
        Ok(KnownWorkspace::Unknown {
            derived: workspace.value().to_owned()
        })
    );
}

#[test]
fn no_record_at_all_falls_back_to_the_derivation() {
    // Also the answer for a cache dl could not read: a lookup that failed must not
    // stop a command that would otherwise have worked.
    let workspace = a_triple("main");
    let devpod = devpod_knowing(&[]);
    let resolved = resolve_known_workspace(
        &devpod,
        &workspace,
        || None,
        &mut ignoring(),
        Patience::AsLongAsItTakes,
    );
    let resolved = resolved.expect("devpod ran and denied it");
    assert_eq!(
        resolved,
        KnownWorkspace::Unknown {
            derived: workspace.value().to_owned()
        }
    );
    assert!(resolved.state().is_none());
    assert_eq!(resolved.workspace_id(), workspace.value());
    assert!(!resolved.is_running());
}

#[test]
fn a_devpod_nobody_can_run_is_not_a_workspace_nobody_knows() {
    // Python's `get_workspace_state` folds a non-zero exit into `None` but
    // *raises* `DevpodNotInstalled`, so this is the point a devpod-less host's
    // command ends. Answering `Unknown` instead sends a launch down the cold
    // path, which clones a repository the host cannot open a container from
    // and leaves it and its record behind (dl/tests/launch.rs pins the
    // observable half).
    let missing = FakeRunner::new().with_missing("devpod");
    let mut asked = false;

    let resolved = resolve_known_workspace(
        &missing,
        &a_triple("main"),
        || {
            asked = true;
            Some("r-main-old".to_owned())
        },
        &mut ignoring(),
        Patience::AsLongAsItTakes,
    );

    assert_eq!(resolved, Err(NotRun::NotInstalled));
    assert!(
        !asked,
        "a devpod that cannot be run makes the record irrelevant, so it is not read"
    );
}

#[test]
fn a_devpod_that_refused_the_derived_id_is_a_denial_and_not_a_failure() {
    // The other side of the line above: devpod ran, said it has no such
    // workspace, and that is the cold path -- exit code and all.
    let workspace = a_triple("main");
    let devpod = devpod_knowing(&[]);
    let resolved = resolve_known_workspace(
        &devpod,
        &workspace,
        || None,
        &mut ignoring(),
        Patience::AsLongAsItTakes,
    );
    assert_eq!(
        resolved,
        Ok(KnownWorkspace::Unknown {
            derived: workspace.value().to_owned()
        })
    );
}

#[test]
fn the_recorded_id_comes_off_the_record_for_the_triple() {
    let mut world = World::empty();
    let clone = world.clone_at("r-main-aa", "main");
    let mut record = world.record("r-main-aa", "main", &clone);
    assert_eq!(
        recorded_devpod_workspace_id(&world.storage, OWNER, REPO, "main"),
        None,
        "every record written before the field existed has it empty"
    );
    record.devpod_workspace_id = Some("r-main-old".to_owned());
    world.storage.add_worktree(record).expect("rewritten");
    assert_eq!(
        recorded_devpod_workspace_id(&world.storage, OWNER, REPO, "main"),
        Some("r-main-old".to_owned())
    );
    assert_eq!(
        recorded_devpod_workspace_id(&world.storage, OWNER, REPO, "other"),
        None
    );
}

#[test]
fn asking_devpod_for_a_state_charges_the_devpod_up_stage() {
    // Python's `@timing.staged("devpod-up")`, which is what stops a warm attach
    // showing a gap where its one round trip was.
    let devpod = devpod_knowing(&[("ws", "Running")]);
    assert_eq!(
        workspace_state(&devpod, "ws", Patience::AsLongAsItTakes),
        Ok(ContainerState::Running)
    );
    assert_eq!(
        devpod.args_to("devpod"),
        [["status", "ws", "--output", "json"]]
    );
}

// =======================================================================
// where a workspace is on this disk
// =======================================================================

#[test]
fn a_url_scheme_and_an_scp_remote_name_somewhere_else() {
    for remote in [
        "https://github.com/o/r.git",
        "http://github.com/o/r",
        "ssh://git@github.com/o/r",
        "git://example.com/o/r",
        "file:///srv/repos/o/r",
        "git@github.com:o/r.git",
        "user@host:path",
    ] {
        assert!(names_a_remote(remote), "{remote}");
    }
}

#[test]
fn text_that_is_also_a_perfectly_good_relative_path_does_not() {
    // The two mistakes are not equal. A path read as a remote drops a directory
    // out of the referenced set, which is how `--prune` would come to call a live
    // clone unreferenced — wrong, and toward loss. So only the shapes that are
    // never also written as a directory count.
    for path in [
        "github.com/o/r",
        "./some-repo",
        "/srv/repos/o/r",
        "some-repo",
        "a/b://c",
        "./a@b:c",
        "b:c",
        "",
    ] {
        assert!(!names_a_remote(path), "{path}");
    }
}

#[test]
fn a_git_source_carrying_a_path_still_places_itself() {
    // `devpod up <path-to-a-repo>` records the gitRepository arm with a path in
    // it, and a path this does not return is a directory `--prune` will call
    // unreferenced (devlaunch#224 is the other direction).
    assert_eq!(
        source_places(&WorkspaceSource::GitRepository("/srv/repos/o/r".to_owned())),
        SourcePlaces::Placeable(vec!["/srv/repos/o/r".to_owned()])
    );
    assert_eq!(
        source_places(&WorkspaceSource::GitRepository(
            "https://github.com/o/r.git".to_owned()
        )),
        SourcePlaces::Placeable(Vec::new())
    );
}

#[test]
fn a_source_that_opens_no_folder_here_is_not_the_same_as_one_dl_cannot_read() {
    // Reading them alike is how a live workspace contributed no path *and* no
    // alarm while the command printed that it stops for exactly that.
    let image = one_workspace("ws", serde_json::json!({ "image": "ubuntu:24.04" }));
    assert_eq!(
        source_places(&image.source),
        SourcePlaces::Placeable(Vec::new())
    );
    let unreadable = one_workspace("ws", serde_json::json!({ "localFolder": 7 }));
    assert!(matches!(
        source_places(&unreadable.source),
        SourcePlaces::Unplaceable { .. }
    ));
}

#[test]
fn where_a_source_sits_is_read_off_its_position_under_the_root() {
    let dir = temp_dir();
    let root = dir.path().join("repos");
    let clone = root.join("o").join("r").join("r-main-aa");
    std::fs::create_dir_all(clone.join(".git")).expect("a clone with a .git");

    assert_eq!(site_of(Path::new("/elsewhere"), &root), SourceSite::Outside);
    assert_eq!(site_of(&root, &root), SourceSite::TooShallow);
    assert_eq!(site_of(&root.join("o"), &root), SourceSite::TooShallow);
    assert_eq!(
        site_of(&root.join("o").join("r"), &root),
        SourceSite::InARepositoryOnly {
            owner: "o".to_owned(),
            repo: "r".to_owned()
        }
    );
    assert_eq!(
        site_of(&clone, &root),
        SourceSite::InAClone {
            clone: clone.clone()
        }
    );
    // `devpod up <clone>/subproject`: the clone is what answers for it.
    assert_eq!(
        site_of(&clone.join("subproject"), &root),
        SourceSite::InAClone {
            clone: clone.clone()
        }
    );
    // devlaunch#88's shape: a folder that is gone, or the config-only stub devpod
    // rebuilds from cache. Neither is a clone, so which clone of the repository
    // the workspace wants is unanswerable.
    assert_eq!(
        site_of(&root.join("o").join("r").join("old-leaf"), &root),
        SourceSite::InARepositoryOnly {
            owner: "o".to_owned(),
            repo: "r".to_owned()
        }
    );
}

#[test]
fn a_clone_whose_git_is_a_file_is_still_a_clone() {
    // `git clone --separate-git-dir` is a layout git supports, and it leaves
    // `.git` as a file. Asking whether a *directory* is there would read it as a
    // place a clone used to be.
    let dir = temp_dir();
    let clone = dir.path().join("clone");
    std::fs::create_dir_all(&clone).expect("the clone");
    std::fs::write(clone.join(".git"), "gitdir: /elsewhere/r.git\n").expect("a gitfile");
    assert!(is_populated_clone(&clone));
}

#[test]
fn a_path_that_is_not_there_still_canonicalises_as_far_as_it_goes() {
    // Which is the ordinary case here: on devlaunch#88's host most devpod records
    // named a folder that had been deleted.
    let dir = temp_dir();
    let real = std::fs::canonicalize(dir.path()).expect("a real temp directory");
    assert_eq!(
        canonical(&dir.path().join("gone").join("deeper").to_string_lossy()),
        Some(real.join("gone").join("deeper"))
    );
}

#[test]
fn a_symlinked_cache_root_and_its_clones_resolve_to_the_same_place() {
    // A lexical comparison here says that *no* clone is referenced, which is a
    // total-loss bug in the one direction that cannot be undone.
    let dir = temp_dir();
    let real = dir.path().join("real");
    let clone = real.join("o").join("r").join("r-main-aa");
    std::fs::create_dir_all(&clone).expect("the clone");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("a symlink");

    assert_eq!(
        canonical(&link.join("o").join("r").join("r-main-aa").to_string_lossy()),
        canonical(&clone.to_string_lossy())
    );
}

#[test]
fn text_no_filesystem_call_will_accept_cannot_be_followed() {
    assert_eq!(canonical(""), None);
    assert_eq!(canonical("has\0a\0nul"), None);
}

#[test]
fn a_live_workspace_opening_part_of_a_clone_still_holds_the_clone() {
    // `devpod up <clone>/subproject` records the subdirectory, and deleting the
    // clone takes the workspace with it. Equality answered no and deleted the
    // parent.
    let dir = temp_dir();
    let root = std::fs::canonicalize(dir.path()).expect("a real directory");
    let clone = root.join("o").join("r").join("r-main-aa");
    std::fs::create_dir_all(clone.join(".git")).expect("a clone");
    std::fs::create_dir_all(clone.join("subproject")).expect("a subproject");
    let workspaces = vec![one_workspace(
        "ws",
        serde_json::json!({ "localFolder": clone.join("subproject").display().to_string() }),
    )];

    let locations = workspace_locations(&workspaces, &root);

    assert_eq!(locations.holder(&clone), Some("ws"));
    // A lexical prefix test would answer yes here, and this is not under it.
    let sibling = root.join("o").join("r").join("r-main-aa-scratch");
    assert_eq!(locations.holder(&sibling), None);
}

#[test]
fn a_source_that_cannot_be_followed_is_named_rather_than_dropped() {
    let dir = temp_dir();
    let root = std::fs::canonicalize(dir.path()).expect("a real directory");
    let workspaces = vec![
        one_workspace("unreadable", serde_json::json!({ "localFolder": [] })),
        one_workspace("nul", serde_json::json!({ "localFolder": "has\0a\0nul" })),
        one_workspace(
            "shallow",
            serde_json::json!({ "localFolder": root.display().to_string() }),
        ),
    ];

    let locations = workspace_locations(&workspaces, &root);
    let unlocatable = locations.unlocatable().expect("three of them");

    assert_eq!(
        unlocatable
            .iter()
            .map(|it| it.workspace_id.clone())
            .collect::<Vec<_>>(),
        ["unreadable", "nul", "shallow"]
    );
}

// =======================================================================
// prune: which clone directories go
// =======================================================================

/// A cache holding one clone directory of every kind the classification has an
/// arm for, all of them real repositories.
///
/// `referenced` is sourced by a live workspace. `orphan-clean` is sourced by
/// nobody and holds nothing unpushed. `orphan-dirty` is sourced by nobody and
/// holds both an unpushed commit and an uncommitted file — 13 of the reference
/// host's 37 stale clones were in that state, two of them with real work in them.
/// `disputed` has a metadata record naming a workspace devpod still lists but
/// sources somewhere else entirely, which is devlaunch#88's shape.
struct Four {
    world: World,
    referenced: PathBuf,
    orphan_clean: PathBuf,
    orphan_dirty: PathBuf,
    disputed: PathBuf,
}

fn four_clones() -> Four {
    let mut world = World::empty();
    let mut made = Vec::new();
    for (leaf, branch) in [
        ("referenced", "ref"),
        ("orphan-clean", "clean"),
        ("orphan-dirty", "dirty"),
        ("disputed", "disp"),
    ] {
        let clone = world.clone_at(leaf, branch);
        world.record(leaf, branch, &clone);
        made.push(clone);
    }
    let dirty = made[2].clone();
    std::fs::write(dirty.join("later.txt"), "later\n").expect("a file");
    commit(&dirty, "later"); // committed, never pushed
    std::fs::write(dirty.join("scratch.md"), "an agent's notes\n").expect("never even added");

    world.devpod.lists(&[
        listed("referenced", &made[0]),
        listed("disputed", &world.tmp().join("somewhere").join("else")),
    ]);
    Four {
        referenced: made[0].clone(),
        orphan_clean: made[1].clone(),
        orphan_dirty: made[2].clone(),
        disputed: made[3].clone(),
        world,
    }
}

#[test]
fn the_four_arms_are_classified_from_the_disk_and_from_devpods_own_listing() {
    let four = four_clones();
    let plan = plan_for(&four.world, Insistence::NotInsisted);

    assert_eq!(
        removing(&plan),
        [four.orphan_clean.as_path()],
        "only the clone nothing references and nothing would lose"
    );
    assert_eq!(
        kept_because(&plan, &four.referenced),
        KeptBecause::StillOpened {
            workspace_id: "referenced".to_owned()
        }
    );
    match kept_because(&plan, &four.orphan_dirty) {
        KeptBecause::Objected(standing) => {
            assert!(standing.would_lose().is_some(), "{standing:?}");
        }
        other => panic!("expected a standing, got {other:?}"),
    }
    assert!(matches!(
        kept_because(&plan, &four.disputed),
        KeptBecause::RecordsDisagree { .. }
    ));
}

#[test]
fn an_orphan_whose_only_work_is_on_a_branch_it_is_not_on_is_kept() {
    // #471 at the reader that removes clones by the dozen rather than one at a
    // time, which is where the blindness cost the most: `orphan-clean` is the
    // arm this plan removes, and a commit sitting on a branch the clone is not
    // checked out on has to move it off that arm on its own.
    let four = four_clones();
    run_git(&four.orphan_clean, &["checkout", "-b", "wip"]);
    std::fs::write(four.orphan_clean.join("wip.txt"), "an hour of work\n").expect("a file");
    commit(&four.orphan_clean, "wip");
    run_git(&four.orphan_clean, &["checkout", "clean"]);

    let plan = plan_for(&four.world, Insistence::NotInsisted);

    assert_eq!(
        removing(&plan),
        [] as [&Path; 0],
        "nothing is safe to remove"
    );
    match kept_because(&plan, &four.orphan_clean) {
        KeptBecause::Objected(standing) => {
            assert!(standing.would_lose().is_some(), "{standing:?}");
        }
        other => panic!("expected a standing, got {other:?}"),
    }
}

#[test]
fn the_bare_cache_is_never_a_candidate_and_never_reported() {
    // Nothing sources it and no record names it, so every rule would call it an
    // orphan — and it is the copy every clone hardlinks its git objects out of.
    let four = four_clones();
    let plan = plan_for(&four.world, Insistence::NotInsisted);
    let bare = canonical(&four.world.bare.to_string_lossy()).expect("the bare directory");
    assert!(!removing(&plan).contains(&bare));
    assert!(
        plan.keeping.iter().all(|kept| kept.path != bare),
        "not reported either: {:?}",
        plan.keeping
    );
}

#[test]
fn force_promotes_the_clone_holding_work_and_nothing_else() {
    // `--force` is not a general override: Referenced and Disputed are devlaunch
    // saying the directory is still in use or that its own records disagree, and
    // there is nothing for a user to mean by insisting.
    let four = four_clones();
    let plan = plan_for(&four.world, Insistence::Insisted);

    let mut going = removing(&plan);
    going.sort();
    let mut expected = vec![four.orphan_clean.clone(), four.orphan_dirty.clone()];
    expected.sort();
    assert_eq!(going, expected);
    assert!(matches!(
        kept_because(&plan, &four.referenced),
        KeptBecause::StillOpened { .. }
    ));
    assert!(matches!(
        kept_because(&plan, &four.disputed),
        KeptBecause::RecordsDisagree { .. }
    ));
}

#[test]
fn what_force_is_answering_rides_on_the_directory_it_answers_for() {
    // Without it the plan reads the same for a clone holding an afternoon's
    // uncommitted work as for an empty one, and the confirmation cannot say what
    // it costs.
    let four = four_clones();
    let plan = plan_for(&four.world, Insistence::Insisted);

    let promotions: Vec<(PathBuf, Promotion)> = plan
        .removing
        .iter()
        .map(|it| (it.path.clone(), it.promotion.clone()))
        .collect();
    let clean = promotions
        .iter()
        .find(|(path, _)| *path == four.orphan_clean)
        .expect("the clean orphan");
    assert_eq!(clean.1, Promotion::Unopposed);
    let dirty = promotions
        .iter()
        .find(|(path, _)| *path == four.orphan_dirty)
        .expect("the dirty orphan");
    match &dirty.1 {
        Promotion::Insisted { despite } => {
            assert!(despite.would_lose().is_some(), "{despite:?}");
        }
        other => panic!("expected an insisted promotion, got {other:?}"),
    }
}

#[test]
fn a_clone_git_will_not_answer_about_is_kept_with_nothing_typed() {
    // Since devlaunch#171 a directory git cannot read as a repository is a
    // `CouldNotTell` rather than "holds nothing", so it objects — and `--force` is
    // what removes it.
    let world = World::empty();
    let broken = world.repo_dir.join("was-never-a-clone");
    std::fs::create_dir_all(&broken).expect("a directory that is not a clone");
    std::fs::write(broken.join("something.txt"), "x\n").expect("a file in it");

    let kept = plan_for(&world, Insistence::NotInsisted);
    match kept_because(&kept, &broken) {
        KeptBecause::Objected(standing) => {
            assert!(standing.could_not_tell().is_some(), "{standing:?}");
        }
        other => panic!("expected a standing, got {other:?}"),
    }
    assert!(removing(&kept).is_empty());

    let forced = plan_for(&world, Insistence::Insisted);
    assert_eq!(removing(&forced), [broken]);
}

#[test]
fn a_symlink_standing_where_a_clone_would_be_is_left_alone() {
    // Following one would put a candidate outside the cache entirely, and
    // unlinking the link instead would report a clone as reclaimed while it sat on
    // another volume.
    let world = World::empty();
    let outside = world.tmp().join("outside");
    std::fs::create_dir_all(&outside).expect("somebody else's directory");
    std::os::unix::fs::symlink(&outside, world.repo_dir.join("a-link")).expect("a symlink");

    let plan = plan_for(&world, Insistence::Insisted);

    assert!(removing(&plan).is_empty());
    assert!(plan.keeping.is_empty(), "{:?}", plan.keeping);
    assert!(outside.exists());
}

#[test]
fn a_machine_with_no_clone_directories_has_nothing_to_prune() {
    let dir = temp_dir();
    let devpod = Devpod::new();
    devpod.lists(&[]);
    let clones = WorkspaceCloneManager::new(
        dir.path().join("repos"),
        Duration::from_secs(3600),
        Git::new(&devpod),
        GitLfs::NotInstalled,
    );
    let (storage, _) = MetadataStorage::open(dir.path().join("metadata.json")).expect("a store");
    let mut context = CommandContext::new(&devpod);
    let workspaces = context.workspaces().expect("a listing");
    let placement = ClonePlacement::resolve(&clones, &workspaces);

    let plan = prune_plan(
        &clones,
        &storage,
        &workspaces,
        &KeptCopies::under(dir.path()),
        &placement,
        Insisted::nothing(),
        &mut ignoring(),
    )
    .expect("a plan");

    assert!(plan.nothing_to_do());
}

#[test]
fn a_workspace_devpod_records_at_a_stub_disputes_that_repositorys_clones() {
    // devlaunch#88's shape, and the reason `--prune` does not wait on it: read as
    // an orphan the healthy clone would be deleted, read as referenced it would
    // silently hide disk.
    let mut world = World::empty();
    let clone = world.clone_at("r-clean-aa", "clean");
    world.record("r-clean-aa", "clean", &clone);
    let stub = world.repo_dir.join("old-leaf");
    std::fs::create_dir_all(&stub).expect("a config-only stub with no .git");
    world.devpod.lists(&[listed("stale", &stub)]);

    let plan = plan_for(&world, Insistence::Insisted);

    assert!(removing(&plan).is_empty(), "{:?}", removing(&plan));
    assert!(matches!(
        kept_because(&plan, &clone),
        KeptBecause::RecordsDisagree { workspace_id, .. } if workspace_id == "stale"
    ));
}

#[test]
fn only_that_repositorys_clones_are_disputed() {
    // What keeps this command usable on devlaunch#88's host rather than merely
    // safe on it.
    let mut world = World::empty();
    let clone = world.clone_at("r-clean-aa", "clean");
    world.record("r-clean-aa", "clean", &clone);
    let other_repo = world.repos_dir.join(OWNER).join("other");
    std::fs::create_dir_all(&other_repo).expect("a second repository");
    let elsewhere = other_repo.join("other-clean-aa");
    std::fs::create_dir_all(elsewhere.join(".git")).expect("a clone of the other repository");
    let stub = world.repo_dir.join("old-leaf");
    std::fs::create_dir_all(&stub).expect("a stub in the first repository");
    world.devpod.lists(&[listed("stale", &stub)]);

    let plan = plan_for(&world, Insistence::Insisted);

    assert_eq!(
        removing(&plan),
        [elsewhere],
        "the other repository is still prunable"
    );
}

#[test]
fn a_source_that_cannot_be_followed_stops_the_whole_command() {
    // While one exists there is no directory this command can honestly call
    // unreferenced.
    let mut world = World::empty();
    let clone = world.clone_at("r-clean-aa", "clean");
    world.record("r-clean-aa", "clean", &clone);
    world.devpod.lists(&[listed_with(
        "unreadable",
        serde_json::json!({ "localFolder": 7 }),
    )]);
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let root = clone_root(&clones);
    let mut context = CommandContext::new(&world.devpod);
    let workspaces = context.workspaces().expect("a listing");

    let locations = workspace_locations(&workspaces, &root);

    let unlocatable = locations.unlocatable().expect("one of them");
    assert_eq!(unlocatable.len(), 1);
    assert_eq!(
        unlocatable.iter().next().expect("it").workspace_id,
        "unreadable"
    );
}

#[test]
fn a_source_that_is_simply_gone_does_not_stop_the_command() {
    let world = World::empty();
    world
        .devpod
        .lists(&[listed("stale", &world.tmp().join("deleted-long-ago"))]);
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let root = clone_root(&clones);
    let mut context = CommandContext::new(&world.devpod);
    let workspaces = context.workspaces().expect("a listing");

    assert!(
        workspace_locations(&workspaces, &root)
            .unlocatable()
            .is_none()
    );
}

#[test]
fn the_biggest_reclaim_is_reported_first() {
    let world = World::empty();
    let small = world.clone_at("small", "small");
    let big = world.clone_at("big", "big");
    // Two megabytes nothing else links to, so the reclaimed figure is a number a
    // test can name. Excluded, because an untracked file is unsaved work and the
    // clone would be kept rather than reclaimed.
    std::fs::write(
        big.join(".git").join("info").join("exclude"),
        "payload.bin\n",
    )
    .expect("an exclude file");
    std::fs::write(big.join("payload.bin"), vec![0u8; 2 * 1024 * 1024]).expect("a payload");

    let plan = plan_for(&world, Insistence::NotInsisted);

    assert_eq!(removing(&plan), [big, small]);
    assert!(plan.clones_freed().known_bytes() > 2 * 1024 * 1024);
}

#[test]
fn a_record_for_a_directory_that_is_already_gone_is_dropped() {
    let mut world = World::empty();
    let gone = world.repo_dir.join("r-gone-aaa");
    world.record("r-gone-aaa", "gone", &gone);

    let plan = plan_for(&world, Insistence::NotInsisted);

    assert_eq!(
        plan.stale_records
            .iter()
            .map(|it| it.branch.clone())
            .collect::<Vec<_>>(),
        ["gone"]
    );
    assert!(
        !plan.nothing_to_do(),
        "a run whose only work is the records"
    );
}

#[test]
fn a_record_whose_directory_cannot_be_looked_at_is_kept() {
    // "dl could not look" is not "this is not there", and only the second is a
    // reason to forget a record — it is the only note of where a clone lives.
    let mut world = World::empty();
    let hidden = world.repo_dir.join("hidden");
    std::fs::create_dir_all(hidden.join("r-main-aa")).expect("a clone inside");
    world.record("r-main-aa", "main", &hidden.join("r-main-aa"));
    let Some(_sealed) = refusing_reads(&hidden) else {
        return;
    };

    let plan = plan_for(&world, Insistence::NotInsisted);

    assert!(plan.stale_records.is_empty(), "{:?}", plan.stale_records);
}

#[test]
fn the_acting_pass_removes_the_directories_and_forgets_their_records() {
    let mut four = four_clones();
    let plan = plan_for(&four.world, Insistence::NotInsisted);
    let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
    let mut context = CommandContext::new(&four.world.devpod);

    let copies = four.world.copies();
    let outcome = prune_clones(
        &mut context,
        &clones,
        &mut four.world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    let PruneOutcome::Acted(report) = &outcome else {
        panic!("expected the pass to act, got {outcome:?}");
    };
    assert_eq!(
        report
            .removed
            .iter()
            .map(|it| it.path.clone())
            .collect::<Vec<_>>(),
        [four.orphan_clean.clone()]
    );
    assert!(report.finished());
    assert!(!four.orphan_clean.exists());
    assert!(four.referenced.exists());
    assert!(four.orphan_dirty.exists());
    assert!(four.disputed.exists());
    assert_eq!(four.world.branches_on_record(), ["dirty", "disp", "ref"]);
}

#[test]
fn work_written_while_the_question_was_open_is_not_destroyed() {
    // The report a user answered was taken before they answered it.
    let mut four = four_clones();
    let plan = plan_for(&four.world, Insistence::NotInsisted);
    assert_eq!(removing(&plan), [four.orphan_clean.clone()]);
    std::fs::write(four.orphan_clean.join("just-typed.md"), "half a plan\n")
        .expect("work written while the question was open");
    let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
    let mut context = CommandContext::new(&four.world.devpod);

    let copies = four.world.copies();
    let outcome = prune_clones(
        &mut context,
        &clones,
        &mut four.world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    let PruneOutcome::Acted(report) = &outcome else {
        panic!("expected the pass to act, got {outcome:?}");
    };
    assert!(report.removed.is_empty());
    assert_eq!(
        report
            .withheld
            .iter()
            .map(|it| it.path.clone())
            .collect::<Vec<_>>(),
        [four.orphan_clean.clone()]
    );
    assert!(four.orphan_clean.join("just-typed.md").exists());
}

#[test]
fn a_clone_a_launch_registered_since_the_plan_is_not_removed_even_under_force() {
    // The clone path for `(owner, repo, branch)` is deterministic, so a concurrent
    // launch reuses the very directory in the plan. Re-asking only "has it grown
    // unsaved work" caught the other case and not this one, and the difference was
    // somebody's running workspace.
    let mut four = four_clones();
    let plan = plan_for(&four.world, Insistence::Insisted);
    assert!(removing(&plan).contains(&four.orphan_dirty));
    four.world.devpod.lists(&[
        listed("referenced", &four.referenced),
        listed("disputed", &four.world.tmp().join("somewhere").join("else")),
        listed("just-launched", &four.orphan_dirty),
    ]);
    let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
    let mut context = CommandContext::new(&four.world.devpod);

    let copies = four.world.copies();
    let outcome = prune_clones(
        &mut context,
        &clones,
        &mut four.world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    let PruneOutcome::Acted(report) = &outcome else {
        panic!("expected the pass to act, got {outcome:?}");
    };
    assert!(
        report.withheld.iter().any(|it| it.path == four.orphan_dirty
            && matches!(it.because, KeptBecause::StillOpened { .. })),
        "{:?}",
        report.withheld
    );
    assert!(four.orphan_dirty.exists());
}

#[test]
fn a_directory_that_refuses_is_named_and_its_siblings_still_go() {
    let mut world = World::empty();
    let stuck = world.clone_at("r-stuck-aa", "stuck");
    let goes = world.clone_at("r-goes-aaa", "goes");
    let plan = plan_for(&world, Insistence::NotInsisted);
    assert_eq!(plan.removing.len(), 2);
    let Some(_sealed) = refusing_writes(&stuck) else {
        return;
    };
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let mut context = CommandContext::new(&world.devpod);

    let copies = world.copies();
    let outcome = prune_clones(
        &mut context,
        &clones,
        &mut world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    let PruneOutcome::Acted(report) = &outcome else {
        panic!("expected the pass to act, got {outcome:?}");
    };
    assert!(!report.finished());
    assert_eq!(
        report
            .removed
            .iter()
            .map(|it| it.path.clone())
            .collect::<Vec<_>>(),
        [goes]
    );
    assert_eq!(
        report
            .refused
            .iter()
            .map(|it| it.path.clone())
            .collect::<Vec<_>>(),
        [stuck]
    );
}

#[test]
fn the_acting_pass_stops_when_a_workspace_appeared_that_it_cannot_place() {
    let mut four = four_clones();
    let plan = plan_for(&four.world, Insistence::NotInsisted);
    four.world.devpod.lists(&[listed_with(
        "unreadable",
        serde_json::json!({ "localFolder": {} }),
    )]);
    let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
    let mut context = CommandContext::new(&four.world.devpod);

    let copies = four.world.copies();
    let outcome = prune_clones(
        &mut context,
        &clones,
        &mut four.world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    assert!(
        matches!(outcome, PruneOutcome::Unlocatable(_)),
        "{outcome:?}"
    );
    assert!(four.orphan_clean.exists(), "nothing was removed");
}

#[test]
fn a_second_run_finds_nothing_left_to_do() {
    let mut four = four_clones();
    let plan = plan_for(&four.world, Insistence::NotInsisted);
    let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
    {
        let mut context = CommandContext::new(&four.world.devpod);
        let copies = four.world.copies();
        prune_clones(
            &mut context,
            &clones,
            &mut four.world.storage,
            &copies,
            &plan,
            &mut ignoring(),
        )
        .expect("the pass ran");
    }
    drop(clones);

    let again = plan_for(&four.world, Insistence::NotInsisted);

    assert!(again.nothing_to_do());
}

#[test]
fn the_acting_pass_pays_a_second_devpod_list() {
    // It is the one question whose answer cannot be re-derived from disk, and it is
    // paid only after a user has said yes to a deletion.
    let mut four = four_clones();
    let plan = plan_for(&four.world, Insistence::NotInsisted);
    let before = four.world.devpod.devpod_argvs().len();
    let clones = clones_for(&four.world.repos_dir, &four.world.devpod);
    let mut context = CommandContext::new(&four.world.devpod);

    let copies = four.world.copies();
    prune_clones(
        &mut context,
        &clones,
        &mut four.world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    assert_eq!(
        four.world.devpod.devpod_argvs()[before..],
        [vec![
            "list".to_owned(),
            "--output".to_owned(),
            "json".to_owned()
        ]]
    );
}

// =======================================================================
// prune: reclaiming from devlaunch's kept copy (devlaunch#456)
// =======================================================================

/// devlaunch's kept copy of what devpod substituted for `workspace_id`, written
/// the way a completed `up` writes it and then left without the devpod record
/// it came from.
///
/// The scratch devpod home is dropped before this returns, which is the whole
/// point: what a bare `devpod delete` outside `dl` leaves behind is a copy in
/// devlaunch's cache and no record anywhere else.
fn a_copy_of(world: &World, workspace_id: &str, folder: &str, devcontainer_id: &str) {
    let home = devpod_home_recording(workspace_id, folder, devcontainer_id);
    world.copies().keep(workspace_id, Some(&home));
    drop(home);
}

/// What the plan would reclaim, as `(workspace, names)` pairs.
fn reclaiming(plan: &PrunePlan) -> Vec<(String, Vec<String>)> {
    plan.reclaiming()
        .iter()
        .map(|it| {
            (
                it.workspace_id.clone(),
                it.names.iter().cloned().collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// The plan enumerates the **copies**, not the clone directories, and a copy
/// carries its own names — so the plan and the acting pass cannot disagree
/// about which volumes belonged to which entry.
#[test]
fn a_prune_plans_the_volumes_of_a_workspace_devpod_has_forgotten() {
    let world = World::empty();
    a_copy_of(&world, "r-main-aa", "/repos/o/r/r-main-aa", "abcdef");

    let plan = plan_for(&world, Insistence::NotInsisted);

    assert_eq!(
        reclaiming(&plan),
        [(
            "r-main-aa".to_owned(),
            vec![
                "r-main-aa-pixi".to_owned(),
                "dind-var-lib-docker-abcdef".to_owned()
            ]
        )]
    );
    assert!(
        !plan.nothing_to_do(),
        "a run whose only work is the volumes"
    );
}

/// The precondition, per copy: no workspace devpod lists carries that id. A
/// live workspace's volumes are not leftovers, and removing them would be the
/// worst thing in this file.
#[test]
fn a_prune_plans_no_volumes_for_a_workspace_devpod_still_lists() {
    let world = World::empty();
    a_copy_of(&world, "r-main-aa", "/repos/o/r/r-main-aa", "abcdef");
    let clone = world.repo_dir.join("r-main-aa");
    std::fs::create_dir_all(&clone).expect("a clone directory");
    world.devpod.lists(&[listed("r-main-aa", &clone)]);

    let plan = plan_for(&world, Insistence::NotInsisted);

    assert_eq!(reclaiming(&plan), []);
    assert!(plan.nothing_to_do());
}

/// The map's hard constraint, and this is what makes it hold by construction
/// rather than by a guard: a run pointed at a scratch cache reads copies that
/// are not there, so it names no volume and runs no docker command at all.
#[test]
fn a_prune_over_a_cache_with_no_copies_names_no_volume() {
    let mut world = World::empty();
    let plan = plan_for(&world, Insistence::NotInsisted);
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let copies = world.copies();
    let mut context = CommandContext::new(&world.devpod);

    prune_clones(
        &mut context,
        &clones,
        &mut world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    assert_eq!(reclaiming(&plan), []);
    assert_eq!(world.devpod.docker_argvs(), Vec::<Vec<String>>::new());
}

/// The reason the domain is the set of copies and not a clone walk
/// (devlaunch#445): a copy whose clone the user deleted by hand names volumes no
/// clone-shaped enumeration will ever reach.
#[test]
fn a_copy_whose_clone_was_deleted_by_hand_is_still_in_the_plan() {
    let world = World::empty();
    a_copy_of(&world, "r-main-aa", "/repos/o/r/r-main-aa", "abcdef");
    assert!(
        !world.repo_dir.join("r-main-aa").exists(),
        "no clone directory for this copy at all"
    );

    let plan = plan_for(&world, Insistence::NotInsisted);

    assert_eq!(
        reclaiming(&plan)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        ["r-main-aa"]
    );
}

/// **The whole regression in one run.** A workspace deleted by a bare `devpod
/// delete` outside `dl` leaves the volumes and no record; pruned, both volumes
/// go, in one `docker volume rm --force`, and the copy that named them is
/// dropped because the removal proved it pointless.
#[test]
fn a_prune_reclaims_the_volumes_of_a_workspace_devpod_forgot_and_drops_the_copy() {
    let mut world = World::empty();
    a_copy_of(&world, "r-main-aa", "/repos/o/r/r-main-aa", "abcdef");
    let plan = plan_for(&world, Insistence::NotInsisted);
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let copies = world.copies();
    let mut context = CommandContext::new(&world.devpod);

    let outcome = prune_clones(
        &mut context,
        &clones,
        &mut world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    let PruneOutcome::Acted(report) = &outcome else {
        panic!("expected the pass to act, got {outcome:?}");
    };
    assert_eq!(
        world.devpod.docker_argvs(),
        [[
            "volume",
            "rm",
            "--force",
            "r-main-aa-pixi",
            "dind-var-lib-docker-abcdef"
        ]]
    );
    assert_eq!(
        report
            .reclaimed
            .iter()
            .map(|it| it.workspace_id.clone())
            .collect::<Vec<_>>(),
        ["r-main-aa"]
    );
    assert_eq!(
        world.copies().copied(),
        Vec::<String>::new(),
        "the copy is dropped on proof"
    );
}

/// The first of the two ways a copy can be wrong, and docker is what catches
/// it: a volume some container still holds is refused, nothing is removed, and
/// the copy is **kept** so the retry survives.
#[test]
fn a_copy_naming_a_held_volume_is_a_reported_refusal_and_the_copy_stays() {
    let mut world = World::empty();
    a_copy_of(&world, "r-main-aa", "/repos/o/r/r-main-aa", "abcdef");
    world.devpod.fake.script(
        ["docker", "volume", "rm"],
        Response::failed(1, "volume is in use\n"),
    );
    let plan = plan_for(&world, Insistence::NotInsisted);
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let copies = world.copies();
    let mut context = CommandContext::new(&world.devpod);

    let outcome = prune_clones(
        &mut context,
        &clones,
        &mut world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    let PruneOutcome::Acted(report) = &outcome else {
        panic!("expected the pass to act, got {outcome:?}");
    };
    assert!(report.reclaimed.is_empty());
    assert_eq!(
        report.volumes_kept,
        [VolumesKept {
            workspace_id: "r-main-aa".to_owned(),
            because: VolumesKeptBecause::Refused(VolumeRefusal::Docker {
                exit: Exit::Code(1),
                stderr: "volume is in use\n".to_owned(),
            }),
        }]
    );
    assert_eq!(
        world.copies().copied(),
        ["r-main-aa"],
        "kept on a refusal, so the retry stays possible"
    );
}

/// The precondition is re-asked under the second listing the acting pass
/// already pays for. A workspace that came back between the plan and the act is
/// a live workspace again, and its volumes are not leftovers.
#[test]
fn a_workspace_back_in_the_listing_between_the_plan_and_the_act_keeps_its_volumes() {
    let mut world = World::empty();
    a_copy_of(&world, "r-main-aa", "/repos/o/r/r-main-aa", "abcdef");
    let plan = plan_for(&world, Insistence::NotInsisted);
    assert_eq!(reclaiming(&plan).len(), 1);
    let clone = world.repo_dir.join("r-main-aa");
    std::fs::create_dir_all(&clone).expect("a clone directory");
    world.devpod.lists(&[listed("r-main-aa", &clone)]);
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let copies = world.copies();
    let mut context = CommandContext::new(&world.devpod);

    let outcome = prune_clones(
        &mut context,
        &clones,
        &mut world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    let PruneOutcome::Acted(report) = &outcome else {
        panic!("expected the pass to act, got {outcome:?}");
    };
    assert_eq!(world.devpod.docker_argvs(), Vec::<Vec<String>>::new());
    assert!(report.reclaimed.is_empty());
    assert_eq!(
        report.volumes_kept,
        [VolumesKept {
            workspace_id: "r-main-aa".to_owned(),
            because: VolumesKeptBecause::ListedAgain,
        }]
    );
    assert_eq!(world.copies().copied(), ["r-main-aa"]);
}

/// A host with no docker never made these volumes, so there is nothing here to
/// have failed and nothing to say — the same silence the delete path keeps.
/// The copy stays, because nothing proved it pointless.
#[test]
fn a_prune_on_a_machine_with_no_docker_says_nothing_about_the_volumes() {
    let mut world = World::empty();
    a_copy_of(&world, "r-main-aa", "/repos/o/r/r-main-aa", "abcdef");
    world.devpod.fake.script_missing("docker");
    let plan = plan_for(&world, Insistence::NotInsisted);
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let copies = world.copies();
    let mut context = CommandContext::new(&world.devpod);

    let outcome = prune_clones(
        &mut context,
        &clones,
        &mut world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    let PruneOutcome::Acted(report) = &outcome else {
        panic!("expected the pass to act, got {outcome:?}");
    };
    assert!(report.reclaimed.is_empty());
    assert!(report.volumes_kept.is_empty());
    assert_eq!(world.copies().copied(), ["r-main-aa"]);
}

/// A delete has already swept these volumes from devpod's own live record, so
/// the copy that named them is pointless — dropped on the same proof the
/// reclaim drops one on, and for the same reason. Left standing, it would have
/// the next `--prune` report reclaiming volumes that went with the workspace.
#[test]
fn a_delete_drops_the_copy_of_the_volumes_it_swept() {
    let mut deleting = a_world_ready_to_delete();
    a_copy_of(
        &deleting.world,
        "r-main-aa",
        "/host/clones/opened-as",
        "dc9a8b7c",
    );

    let (outcome, _) = deleting.delete();

    assert!(
        matches!(
            outcome,
            RemoveOutcome::Deleted {
                volumes: VolumeSweep::Removed,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(deleting.world.copies().copied(), Vec::<String>::new());
}

/// Kept on a refusal here too, and for the reclaim's reason: the volumes are
/// still on the machine, so the copy is still the only thing that names them.
#[test]
fn a_delete_docker_refused_keeps_the_copy() {
    let mut deleting = a_world_ready_to_delete();
    a_copy_of(
        &deleting.world,
        "r-main-aa",
        "/host/clones/opened-as",
        "dc9a8b7c",
    );
    deleting.world.devpod.fake.script(
        ["docker", "volume", "rm"],
        Response::failed(1, "volume is in use\n"),
    );

    deleting.delete();

    assert_eq!(deleting.world.copies().copied(), ["r-main-aa"]);
}

// =======================================================================
// agent worktrees inside the clones a prune keeps (devlaunch#426, #454)
// =======================================================================

/// One real agent git worktree inside `clone`, on its own pushed branch.
///
/// The directory the harness makes, made the way the harness makes it,
/// because the classification is git's own `worktree list` and a stub would
/// report no registrations at all -- which used to read as "git has
/// forgotten these", the answer that deleted.
fn an_agent_worktree(clone: &Path, leaf: &str) -> PathBuf {
    let path = clone.join(".claude").join("worktrees").join(leaf);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the worktrees directory");
    run_git(
        clone,
        &["worktree", "add", "-b", leaf, &path.display().to_string()],
    );
    run_git(clone, &["push", "-u", "origin", leaf]);
    path
}

/// A worktree nested inside another, the way an agent session running in a
/// worktree makes one.
fn a_nested_worktree(clone: &Path, inside: &Path, leaf: &str) -> PathBuf {
    let path = inside.join(".claude").join("worktrees").join(leaf);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the worktrees directory");
    run_git(
        clone,
        &["worktree", "add", "-b", leaf, &path.display().to_string()],
    );
    run_git(clone, &["push", "-u", "origin", leaf]);
    path
}

/// Rewrite `clone`'s worktree registrations to the container paths they
/// really carry, which is what a host sees.
fn as_a_host_sees_them(clone: &Path) {
    let admin = clone.join(".git").join("worktrees");
    for entry in std::fs::read_dir(&admin).expect("the admin directory") {
        let gitdir = entry.expect("an admin entry").path().join("gitdir");
        let registered = std::fs::read_to_string(&gitdir).expect("a gitdir file");
        std::fs::write(
            &gitdir,
            registered.replace(&clone.display().to_string(), "/workspaces/a-container"),
        )
        .expect("the rewritten gitdir");
    }
}

/// A live workspace whose clone holds one collectable agent worktree.
fn a_live_clone_with_an_agent_worktree() -> (World, PathBuf, PathBuf) {
    let mut world = World::empty();
    let clone = world.clone_at("r-live-aa", "live");
    world.record("r-live-aa", "live", &clone);
    let worktree = an_agent_worktree(&clone, "agent-one");
    as_a_host_sees_them(&clone);
    world.devpod.lists(&[listed("live", &clone)]);
    (world, clone, worktree)
}

/// Every directory the sweep would remove, across every clone in the plan.
fn sweeping(plan: &PrunePlan) -> Vec<PathBuf> {
    plan.worktrees()
        .clones()
        .iter()
        .flat_map(|clone| clone.going())
        .filter_map(|going| match going.what() {
            agent_worktrees::Collectable::Directory(directory) => {
                Some(directory.at().to_path_buf())
            }
            agent_worktrees::Collectable::Registration(_) => None,
        })
        .collect()
}

/// Every site the sweep would leave standing, across every clone.
fn sweep_standing(plan: &PrunePlan) -> Vec<PathBuf> {
    plan.worktrees()
        .clones()
        .iter()
        .flat_map(|clone| clone.standing())
        .map(|site| site.at().to_path_buf())
        .collect()
}

#[test]
fn the_plan_reaches_inside_a_clone_it_is_keeping() {
    // The whole of devlaunch#426: every one of the 72 directories measured
    // was inside a clone belonging to a *live* workspace, so the orphan rule
    // not only missed them, it must never fire on them.
    let (world, clone, worktree) = a_live_clone_with_an_agent_worktree();

    let plan = plan_for(&world, Insistence::NotInsisted);

    assert!(
        removing(&plan).is_empty(),
        "the clone itself is staying: {:?}",
        removing(&plan)
    );
    assert!(matches!(
        kept_because(&plan, &clone),
        KeptBecause::StillOpened { .. }
    ));
    assert_eq!(sweeping(&plan), [worktree]);
    assert!(
        !plan.nothing_to_do(),
        "a plan with worktrees to reclaim has something to do"
    );
}

#[test]
fn a_worktree_inside_a_clone_that_is_going_is_not_swept_separately() {
    // Its bytes are already in the clone's own figure, so sweeping it would
    // count them twice and offer a directory that will not be there.
    let world = World::empty();
    let orphan = world.clone_at("r-orphan-aa", "orphan");
    an_agent_worktree(&orphan, "agent-one");
    as_a_host_sees_them(&orphan);
    world.devpod.lists(&[]);

    // `--force` is what carries the clone itself past the standing the
    // worktrees put in its way: the clone-level verdict conjoins every site
    // in it, and an untracked `.claude/` is uncommitted work besides. That
    // guard is right to count them -- removing the clone destroys whatever
    // they hold -- so the insistence is the fixture, not a workaround.
    let plan = plan_for(&world, Insistence::Insisted);

    assert_eq!(removing(&plan), [orphan]);
    assert!(plan.worktrees().nothing_to_say(), "{:?}", plan.worktrees());
}

#[test]
fn the_acting_pass_removes_the_worktree_and_forgets_its_registration() {
    let (mut world, clone, worktree) = a_live_clone_with_an_agent_worktree();
    let plan = plan_for(&world, Insistence::NotInsisted);
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let copies = world.copies();

    let outcome = prune_clones(
        &mut context,
        &clones,
        &mut world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    let PruneOutcome::Acted(report) = &outcome else {
        panic!("expected the pass to act, got {outcome:?}");
    };
    assert!(report.finished());
    assert_eq!(report.worktrees.removed.len(), 1);
    assert_eq!(report.worktrees.forgotten, 1);
    assert!(!worktree.exists());
    assert!(clone.exists(), "the clone itself is untouched");
    let listing = run_git(&clone, &["worktree", "list", "--porcelain"]);
    assert!(
        !listing.contains("agent-one"),
        "git still lists it: {listing}"
    );
}

#[test]
fn a_nested_worktree_holding_work_stands_the_whole_subtree_end_to_end() {
    // T1 through the real command's two passes: the plan offers nothing
    // containing the outer worktree, and the acting pass removes nothing.
    let mut world = World::empty();
    let clone = world.clone_at("r-live-aa", "live");
    world.record("r-live-aa", "live", &clone);
    let outer = an_agent_worktree(&clone, "agent-outer");
    let inner = a_nested_worktree(&clone, &outer, "agent-inner");
    std::fs::write(inner.join("UNSAVED.txt"), "an afternoon\n").expect("the note");
    as_a_host_sees_them(&clone);
    world.devpod.lists(&[listed("live", &clone)]);

    let plan = plan_for(&world, Insistence::NotInsisted);
    assert!(sweeping(&plan).is_empty(), "{:?}", plan.worktrees());
    assert_eq!(sweep_standing(&plan), std::slice::from_ref(&inner));

    let clones = clones_for(&world.repos_dir, &world.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let copies = world.copies();
    prune_clones(
        &mut context,
        &clones,
        &mut world.storage,
        &copies,
        &plan,
        &mut ignoring(),
    )
    .expect("the pass ran");

    assert!(outer.exists(), "the outer worktree is untouched");
    assert_eq!(
        std::fs::read_to_string(inner.join("UNSAVED.txt")).expect("the note is still here"),
        "an afternoon\n"
    );
}

#[test]
fn what_a_worktree_holds_is_reported_and_kept_until_the_flag_says_otherwise() {
    let (world, clone, worktree) = a_live_clone_with_an_agent_worktree();
    std::fs::write(worktree.join("notes.md"), "an afternoon\n").expect("a note");

    let kept = plan_for(&world, Insistence::Insisted);
    assert!(
        sweeping(&kept).is_empty(),
        "--force is not --force-worktrees: {:?}",
        kept.worktrees()
    );
    assert_eq!(sweep_standing(&kept), std::slice::from_ref(&worktree));

    let removed = plan_insisting(
        &world,
        Insisted {
            clones: Insistence::NotInsisted,
            worktrees: Insistence::Insisted,
        },
    );
    assert_eq!(sweeping(&removed), [worktree]);
    assert!(clone.exists());
}

#[test]
fn an_orphan_clone_whose_gitignored_worktrees_hold_work_is_kept() {
    // The nested half of the dirt blindness, at the surface that destroys
    // things: `.claude/` is gitignored, so the clone's own status says
    // nothing, and the shipped orphan rule removed the clone outright.
    let world = World::empty();
    let orphan = world.clone_at("r-orphan-aa", "orphan");
    std::fs::write(orphan.join(".gitignore"), ".claude/\n").expect("a gitignore");
    commit(&orphan, "ignore the agent worktrees");
    run_git(&orphan, &["push", "origin", "orphan"]);
    let worktree = an_agent_worktree(&orphan, "agent-one");
    std::fs::write(worktree.join("UNSAVED.txt"), "an afternoon\n").expect("the note");
    as_a_host_sees_them(&orphan);
    world.devpod.lists(&[]);

    let plan = plan_for(&world, Insistence::NotInsisted);

    assert!(
        removing(&plan).is_empty(),
        "the clone holds work one level in: {:?}",
        removing(&plan)
    );
    match kept_because(&plan, &orphan) {
        KeptBecause::Objected(standing) => {
            let words = standing.describe();
            assert!(words.contains("agent-one"), "{words}");
        }
        other => panic!("expected a standing, got {other:?}"),
    }
}

// =======================================================================
// reconcile (devlaunch#88)
// =======================================================================

/// A plain clone directory: `.git` present, nothing else.
///
/// Not the corner-cutting it would be in the prune tests. `--prune` guards a
/// deletion, so its guard is a real `git status` and a stub would answer "holds
/// nothing" — the reply that deletes. This command asks git nothing at all: what
/// it needs to know is whether a directory is a checkout, which is `.git`'s
/// presence and devlaunch#88's own published diagnostic.
fn a_bare_clone_directory(at: &Path) -> PathBuf {
    std::fs::create_dir_all(at.join(".git")).expect("a clone directory");
    at.to_path_buf()
}

/// devpod's own record for a workspace, in devpod's on-disk shape.
fn devpod_record(devpod_home: &DevpodHome, workspace_id: &str, source: &Path) -> PathBuf {
    let path = devpod_home.record("default", workspace_id);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the record directory");
    std::fs::write(
        &path,
        serde_json::json!({
            "id": workspace_id,
            "provider": { "name": "docker" },
            "ide": { "name": "none" },
            "source": { "localFolder": source.display().to_string() },
            "uid": "keep-me",
            "creationTimestamp": "2026-03-01T18:39:40Z",
            "context": "default",
        })
        .to_string(),
    )
    .expect("devpod's record");
    path
}

fn sourced_at(record: &Path) -> String {
    let text = std::fs::read_to_string(record).expect("devpod's record");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("it is JSON");
    parsed["source"]["localFolder"]
        .as_str()
        .expect("a source folder")
        .to_owned()
}

/// The plan `--reconcile` would print.
fn reconcile_for(world: &World) -> ReconcilePlan {
    let clones = clones_for(&world.repos_dir, &world.devpod);
    let mut context = CommandContext::new(&world.devpod);
    let workspaces = context.workspaces().expect("a listing");
    let placement = ClonePlacement::resolve(&clones, &workspaces);
    reconcile_plan(
        &clones,
        &world.storage,
        &workspaces,
        &placement,
        &mut ignoring(),
    )
}

#[test]
fn the_legacy_leaf_is_the_branch_flattened_for_a_path_component() {
    assert_eq!(legacy_leaf("feature/auth"), "feature-auth");
    assert_eq!(legacy_leaf("feature auth"), "feature-auth");
    assert_eq!(legacy_leaf("feature:auth"), "feature-auth");
    assert_eq!(legacy_leaf("main"), "main");
    assert_eq!(legacy_leaf("v1.2-rc_3"), "v1.2-rc_3");
    assert_eq!(legacy_leaf("/lead/"), "lead");
}

#[test]
fn an_orphan_whose_clone_answers_to_its_old_leaf_is_adopted() {
    // The join is by path and never by id: the id is what the scheme change moved,
    // and the source path devpod kept still names owner, repo and branch.
    let mut world = World::empty();
    let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
    let clone = a_bare_clone_directory(&world.repo_dir.join("r-feature-auth-aaa"));
    world.record("r-feature-auth-aaa", "feature/auth", &clone);
    let old = world.repo_dir.join("feature-auth");
    let record = devpod_record(&devpod_home, "ws-old", &old);
    world.devpod.lists(&[listed("ws-old", &old)]);

    let plan = reconcile_for(&world);

    assert_eq!(plan.adopting.len(), 1, "{plan:?}");
    let adoptable = &plan.adopting[0];
    assert_eq!(adoptable.workspace_id, "ws-old");
    assert_eq!(adoptable.context, "default");
    assert_eq!(
        adoptable.clone,
        canonical(&clone.to_string_lossy()).expect("the clone")
    );
    assert!(plan.reporting.is_empty());
    assert!(!plan.nothing_to_do());
    assert!(record.exists());
}

#[test]
fn applying_a_plan_repoints_devpods_own_record_and_keeps_its_other_keys() {
    let mut world = World::empty();
    let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
    let clone = a_bare_clone_directory(&world.repo_dir.join("r-feature-auth-aaa"));
    world.record("r-feature-auth-aaa", "feature/auth", &clone);
    let old = world.repo_dir.join("feature-auth");
    let record = devpod_record(&devpod_home, "ws-old", &old);
    world.devpod.lists(&[listed("ws-old", &old)]);
    let plan = reconcile_for(&world);
    let updater = SelfInvocation::new("dl");
    let cache_path = fresh_cache(world.tmp());
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&updater, &cache_path);

    let report = apply_reconciliation(
        &mut context,
        &mut refresh,
        &mut world.storage,
        &devpod_home,
        &plan,
        &mut ignoring(),
    );

    assert!(report.finished());
    assert_eq!(report.repointed().count(), 1);
    assert_eq!(
        sourced_at(&record),
        canonical(&clone.to_string_lossy())
            .expect("the clone")
            .display()
            .to_string()
    );
    let text = std::fs::read_to_string(&record).expect("devpod's record");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("it is JSON");
    assert_eq!(
        parsed["uid"], "keep-me",
        "every key devpod knows about and dl does not survives"
    );
    assert_eq!(
        recorded_devpod_workspace_id(&world.storage, OWNER, REPO, "feature/auth"),
        Some("ws-old".to_owned()),
        "the second copy of the id, which stops this happening again"
    );
    assert!(
        !record.with_extension("dl-tmp").exists(),
        "the temp file is renamed, not left behind"
    );
}

#[test]
fn a_record_removed_while_the_plan_sat_there_is_not_reported_as_re_pointed() {
    // The confirmation prompt is an unbounded wait, and `dl <ws> rm` in
    // another terminal is what walks through it. devpod's record is
    // re-pointed either way — that write is done before metadata is
    // reloaded — but the id is not written, because writing it would put
    // back a row the other run deleted. What must not happen is the run
    // reporting an adoption that landed anyway.
    let mut world = World::empty();
    let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
    let clone = a_bare_clone_directory(&world.repo_dir.join("r-feature-auth-aaa"));
    world.record("r-feature-auth-aaa", "feature/auth", &clone);
    let old = world.repo_dir.join("feature-auth");
    let record = devpod_record(&devpod_home, "ws-old", &old);
    world.devpod.lists(&[listed("ws-old", &old)]);
    let plan = reconcile_for(&world);
    world
        .storage
        .remove_worktree(OWNER, REPO, "feature/auth")
        .expect("the other run's delete");
    let updater = SelfInvocation::new("dl");
    let cache_path = fresh_cache(world.tmp());
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&updater, &cache_path);

    let report = apply_reconciliation(
        &mut context,
        &mut refresh,
        &mut world.storage,
        &devpod_home,
        &plan,
        &mut ignoring(),
    );

    assert_eq!(
        report.adoptions(),
        [Adoption::Unrecorded {
            workspace_id: "ws-old".to_owned()
        }],
        "the ending the report carries is the one that happened"
    );
    assert_eq!(report.repointed().count(), 0, "nothing was recorded");
    assert!(
        !report.finished(),
        "an adoption that wrote nothing is not an adoption that landed"
    );
    assert!(
        world
            .storage
            .get_worktree(OWNER, REPO, "feature/auth")
            .is_none(),
        "the other run's delete stands"
    );
    assert_eq!(
        sourced_at(&record),
        canonical(&clone.to_string_lossy())
            .expect("the clone")
            .display()
            .to_string(),
        "devpod's record was re-pointed before the reload found the row gone"
    );
}

#[test]
fn reconciling_never_reaches_devpod_delete() {
    // A wrongly-adopted workspace costs a rebuild; a wrongly-deleted one costs
    // whatever was in it.
    let mut world = World::empty();
    let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
    let old = world.repo_dir.join("no-such-clone");
    devpod_record(&devpod_home, "ws-old", &old);
    world.devpod.lists(&[listed("ws-old", &old)]);
    let plan = reconcile_for(&world);
    let updater = SelfInvocation::new("dl");
    let cache_path = fresh_cache(world.tmp());
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&updater, &cache_path);

    apply_reconciliation(
        &mut context,
        &mut refresh,
        &mut world.storage,
        &devpod_home,
        &plan,
        &mut ignoring(),
    );

    assert_eq!(world.devpod.deleted(), Vec::<String>::new());
    assert_eq!(
        plan.reporting
            .iter()
            .map(|it| it.because.clone())
            .collect::<Vec<_>>(),
        [NotAdopted::NoCloneAnswers]
    );
}

#[test]
fn a_name_two_clones_answer_to_is_claimed_by_neither() {
    // The legacy spelling is not injective: `feature/auth` and `feature:auth` were
    // both the directory `feature-auth`, so one devpod record can name two
    // branches' clones and a map would hand it whichever was written last.
    let mut world = World::empty();
    let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
    let slash = a_bare_clone_directory(&world.repo_dir.join("r-feature-auth-aaa"));
    let colon = a_bare_clone_directory(&world.repo_dir.join("r-feature-auth-bbb"));
    world.record("r-feature-auth-aaa", "feature/auth", &slash);
    world.record("r-feature-auth-bbb", "feature:auth", &colon);
    let old = world.repo_dir.join("feature-auth");
    devpod_record(&devpod_home, "ws-old", &old);
    world.devpod.lists(&[listed("ws-old", &old)]);

    let plan = reconcile_for(&world);

    assert!(plan.adopting.is_empty(), "{plan:?}");
    let NotAdopted::NameAnsweredByManyClones(answers) = &plan.reporting[0].because else {
        panic!("expected a contested name, got {:?}", plan.reporting);
    };
    assert_eq!(answers.len(), 2);
}

#[test]
fn a_clone_two_orphans_both_match_is_claimed_by_neither() {
    // Picking one would be a coin flip decided by listing order, and the loser
    // would still be broken with nothing said about why.
    let mut world = World::empty();
    let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
    let clone = a_bare_clone_directory(&world.repo_dir.join("r-main-aaa"));
    world.record("r-main-aaa", "main", &clone);
    let old = world.repo_dir.join("main");
    devpod_record(&devpod_home, "ws-one", &old);
    devpod_record(&devpod_home, "ws-two", &old);
    world
        .devpod
        .lists(&[listed("ws-one", &old), listed("ws-two", &old)]);

    let plan = reconcile_for(&world);

    assert!(plan.adopting.is_empty(), "{plan:?}");
    assert_eq!(plan.reporting.len(), 2);
    for unadoptable in &plan.reporting {
        assert!(
            matches!(
                unadoptable.because,
                NotAdopted::CloneWantedByManyWorkspaces { workspaces: 2, .. }
            ),
            "{unadoptable:?}"
        );
    }
}

#[test]
fn a_clone_a_live_workspace_already_opens_is_not_a_candidate() {
    // Adopting it would point two workspaces at one directory and leave the working
    // one sharing its checkout with a dead one.
    let mut world = World::empty();
    let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
    let clone = a_bare_clone_directory(&world.repo_dir.join("r-main-aaa"));
    world.record("r-main-aaa", "main", &clone);
    let old = world.repo_dir.join("main");
    devpod_record(&devpod_home, "ws-old", &old);
    world
        .devpod
        .lists(&[listed("ws-old", &old), listed("ws-live", &clone)]);

    let plan = reconcile_for(&world);

    assert!(plan.adopting.is_empty(), "{plan:?}");
    assert_eq!(
        plan.reporting
            .iter()
            .map(|it| it.because.clone())
            .collect::<Vec<_>>(),
        [NotAdopted::NoCloneAnswers]
    );
}

#[test]
fn running_it_twice_changes_nothing() {
    let mut world = World::empty();
    let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
    let clone = a_bare_clone_directory(&world.repo_dir.join("r-main-aaa"));
    world.record("r-main-aaa", "main", &clone);
    let old = world.repo_dir.join("main");
    let record = devpod_record(&devpod_home, "ws-old", &old);
    world.devpod.lists(&[listed("ws-old", &old)]);
    let updater = SelfInvocation::new("dl");
    let cache_path = fresh_cache(world.tmp());
    {
        let plan = reconcile_for(&world);
        let mut context = CommandContext::new(&world.devpod);
        let mut refresh = Refresh::new(&updater, &cache_path);
        apply_reconciliation(
            &mut context,
            &mut refresh,
            &mut world.storage,
            &devpod_home,
            &plan,
            &mut ignoring(),
        );
    }
    // devpod now sources the workspace at the clone, which is what a second run
    // reads.
    world.devpod.lists(&[listed("ws-old", &clone)]);

    let again = reconcile_for(&world);

    assert!(again.nothing_to_do(), "{again:?}");
    assert_eq!(
        sourced_at(&record),
        canonical(&clone.to_string_lossy())
            .expect("the clone")
            .display()
            .to_string()
    );
}

#[test]
fn a_repair_that_cannot_be_made_is_not_half_made() {
    // devpod's record is re-pointed first and metadata's id written second, and the
    // failure of the first must leave the second alone.
    let mut world = World::empty();
    let devpod_home = DevpodHome::at(world.tmp().join("devpod"));
    let clone = a_bare_clone_directory(&world.repo_dir.join("r-main-aaa"));
    world.record("r-main-aaa", "main", &clone);
    let old = world.repo_dir.join("main");
    // Listed, with no workspace.json written for it: a run that decided to adopt
    // this workspace has nothing to rewrite and must say so.
    world.devpod.lists(&[listed("ws-old", &old)]);
    let plan = reconcile_for(&world);
    assert_eq!(plan.adopting.len(), 1, "{plan:?}");
    let updater = SelfInvocation::new("dl");
    let cache_path = fresh_cache(world.tmp());
    let mut context = CommandContext::new(&world.devpod);
    let mut refresh = Refresh::new(&updater, &cache_path);

    let report = apply_reconciliation(
        &mut context,
        &mut refresh,
        &mut world.storage,
        &devpod_home,
        &plan,
        &mut ignoring(),
    );

    assert!(!report.finished());
    assert!(matches!(
        report.adoptions(),
        [Adoption::Refused {
            failure: RepointFailure::Unreadable { .. },
            ..
        }]
    ));
    assert_eq!(
        recorded_devpod_workspace_id(&world.storage, OWNER, REPO, "main"),
        None,
        "the id is written only once devpod's record says the same thing"
    );
}

#[test]
fn a_subtree_that_would_not_come_away_leaves_the_run_unfinished() {
    use crate::flows::agent_worktrees::WorktreeReport;
    use crate::flows::repo_manager::{Refusal, RefusalReason};

    // The exit contract. `finished()` decides `Ending::Done` against
    // `Ending::Unfinished`, and its own caller says why: a directory the user
    // was told would go is still on disk. A derivative that refused leaves an
    // environment part-removed, which is more than "still on disk" -- and it
    // is the ordinary case for a container-written tree, not the exotic one.
    let mut report = PruneReport {
        removed: Vec::new(),
        withheld: Vec::new(),
        refused: Vec::new(),
        reclaimed: Vec::new(),
        volumes_kept: Vec::new(),
        worktrees: WorktreeReport::default(),
    };

    assert!(report.finished(), "nothing was refused");

    report.worktrees.refused_derivatives.push(Refusal {
        path: PathBuf::from("/c/.claude/worktrees/agent-one/.pixi/envs/default/lib"),
        reason: RefusalReason::System("Permission denied (os error 13)".to_owned()),
    });

    assert!(
        !report.finished(),
        "a part-removed environment is not a finished run, and exiting 0 tells \
         every script that called dl that it was"
    );
}
