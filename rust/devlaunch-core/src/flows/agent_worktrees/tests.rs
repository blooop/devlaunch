//! What the agent-worktree sweep decides, at the seam the whole ticket turns on.
//!
//! **Real git, real filesystem, real registrations.** Every fact this module
//! acts on comes out of `git worktree list --porcelain` and out of `git status`
//! run through an admin directory, and a faked spawn answers a clean exit with
//! empty output — which reads as "git named no registrations" and "this worktree
//! is clean", the two answers that used to delete. So these build a clone with
//! real worktrees in it and then rewrite the registrations to container paths,
//! which is the shape a host sees and the shape none of this can be tested
//! without.
//!
//! Where a decision says a state is **unrepresentable**, the test here asserts
//! the absence of a path — a plan that contains no unit for it, a spawn log that
//! contains no invocation naming it — rather than a guard firing.

use std::cell::RefCell;

use devlaunch_runner::{
    CapturedText, DetachOutcome, Invocation, Outcome, ProcessRunner, Runner, SpawnSpec,
};

use super::*;
use crate::flows::repo_manager::tests::run_git;

/// A cache holding one bare repository and one clone of it, with real worktrees
/// under the clone's `.claude/worktrees/`.
///
/// The layout is the one `--prune` scans: `<repos>/<owner>/<repo>/.bare` beside
/// `<repos>/<owner>/<repo>/<leaf>`, so the sibling cache the reachability probe
/// consults is where the real thing puts it.
struct Clone {
    dir: tempfile::TempDir,
    bare: PathBuf,
    clone: PathBuf,
}

const OWNER: &str = "o";
const REPO: &str = "r";

impl Clone {
    /// A clone of a one-commit repository, with nothing under `.claude/` yet.
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let root = dir.path().to_path_buf();
        let seed = root.join("seed");
        std::fs::create_dir_all(&seed).expect("a seed directory");
        run_git(&root, &["init", "-b", "main", &seed.display().to_string()]);
        std::fs::write(seed.join("README.md"), "seed\n").expect("a README");
        commit(&seed, "seed");

        // The forge stands in for GitHub; the bare cache is what devlaunch
        // fetches into and what the reachability probe asks.
        let forge = root.join("forge.git");
        clone_bare(&root, &seed, &forge);
        let repo_dir = root.join("repos").join(OWNER).join(REPO);
        std::fs::create_dir_all(&repo_dir).expect("the repository directory");
        let bare = repo_dir.join(".bare");
        clone_bare(&root, &forge, &bare);

        let clone = repo_dir.join("ws-one");
        run_git(
            &root,
            &[
                "clone",
                &bare.display().to_string(),
                &clone.display().to_string(),
            ],
        );
        run_git(
            &clone,
            &["remote", "set-url", "origin", &forge.display().to_string()],
        );
        Self { dir, bare, clone }
    }

    fn tmp(&self) -> &Path {
        self.dir.path()
    }

    /// One real agent worktree at `<clone>/.claude/worktrees/<leaf>` on its own
    /// branch, pushed to the forge and fetched into the cache — the ordinary
    /// finished-task shape, holding nothing.
    fn worktree(&self, leaf: &str) -> PathBuf {
        let path = worktrees_dir(&self.clone).join(leaf);
        std::fs::create_dir_all(worktrees_dir(&self.clone)).expect("the worktrees directory");
        run_git(
            &self.clone,
            &["worktree", "add", "-b", leaf, &path.display().to_string()],
        );
        run_git(&self.clone, &["push", "origin", leaf]);
        self.fetch();
        path
    }

    /// A worktree nested inside another one, the way an agent session running in
    /// a worktree creates one.
    fn nested(&self, inside: &Path, leaf: &str) -> PathBuf {
        let path = worktrees_dir(inside).join(leaf);
        std::fs::create_dir_all(worktrees_dir(inside)).expect("the nested worktrees directory");
        run_git(
            &self.clone,
            &["worktree", "add", "-b", leaf, &path.display().to_string()],
        );
        run_git(&self.clone, &["push", "origin", leaf]);
        self.fetch();
        path
    }

    /// Bring the cache up to date with the forge, the way the fetch sweep does.
    fn fetch(&self) {
        run_git(
            &self.bare,
            &["fetch", "origin", "+refs/heads/*:refs/heads/*", "--prune"],
        );
    }

    /// Rewrite every registration to the container path it would really carry,
    /// which is what a host actually sees.
    fn containerise(&self) {
        let admin = self.clone.join(".git").join("worktrees");
        for entry in std::fs::read_dir(&admin).expect("the admin directory") {
            let gitdir = entry.expect("an admin entry").path().join("gitdir");
            let registered = std::fs::read_to_string(&gitdir).expect("a gitdir file");
            std::fs::write(
                &gitdir,
                registered.replace(
                    &self.clone.display().to_string(),
                    "/workspaces/devlaunch-container",
                ),
            )
            .expect("the rewritten gitdir");
        }
    }

    /// The sweep, as `--prune`'s planning pass would take it.
    fn sweep(&self, insistence: Insistence) -> Option<CloneWorktrees> {
        let runner = ProcessRunner::new();
        let git = Git::new(&runner);
        sweep_clone(&git, &self.clone, OWNER, REPO, Some(&self.bare), insistence)
    }

    /// The sweep, insisting on nothing.
    fn plan(&self) -> CloneWorktrees {
        self.sweep(Insistence::NotInsisted)
            .expect("a sweep of a clone that has worktrees")
    }

    /// Carry `plan` out, over a recording runner, and hand back what happened
    /// and every argv the pass spawned.
    fn act(&self, plan: &CloneWorktrees) -> (WorktreeReport, Vec<Vec<String>>) {
        self.act_expecting_absent_forgets(plan, true)
    }

    fn act_expecting_absent_forgets(
        &self,
        plan: &CloneWorktrees,
        forgets_must_be_absent: bool,
    ) -> (WorktreeReport, Vec<Vec<String>>) {
        let calls = RefCell::new(Vec::new());
        let runner = Recording {
            real: ProcessRunner::new(),
            calls: &calls,
            forgets_must_be_absent,
        };
        let git = Git::new(&runner);
        let mut report = WorktreeReport::default();
        reclaim(&git, plan, Some(&self.bare), &mut report);
        (report, calls.into_inner())
    }

    fn listing(&self) -> String {
        run_git(&self.clone, &["worktree", "list", "--porcelain"])
    }
}

/// A second, unrelated repository, for the cross-repository fixtures
/// (devlaunch#463). Its worktrees are made *inside* the first clone's worktrees
/// place, which takes nothing more than a `git -C <other> worktree add <path>`.
struct OtherRepository {
    repo: PathBuf,
}

impl OtherRepository {
    fn new(root: &Path) -> Self {
        let repo = root.join("other-repo");
        std::fs::create_dir_all(&repo).expect("the other repository");
        run_git(root, &["init", "-b", "main", &repo.display().to_string()]);
        std::fs::write(repo.join("THEIRS.md"), "theirs\n").expect("their file");
        commit(&repo, "theirs");
        Self { repo }
    }

    /// A live worktree of this repository at `at`, on its own branch, holding
    /// uncommitted work.
    fn worktree_at(&self, at: &Path, branch: &str) -> PathBuf {
        std::fs::create_dir_all(at.parent().expect("a parent")).expect("the nesting directory");
        run_git(
            &self.repo,
            &["worktree", "add", "-b", branch, &at.display().to_string()],
        );
        std::fs::write(at.join("UNSAVED.txt"), "an afternoon nobody else has\n")
            .expect("their unsaved work");
        at.to_path_buf()
    }

    fn listing(&self) -> String {
        run_git(&self.repo, &["worktree", "list", "--porcelain"])
    }
}

/// A wrapper that does real work and keeps every argv, so a test can assert an
/// invocation was never made — the absence half of T2 and of the foreign-site
/// rules — and that the forget's argument does not exist at the moment the
/// forget is invoked, which is P2 asserted directly (devlaunch#462).
struct Recording<'a> {
    real: ProcessRunner,
    calls: &'a RefCell<Vec<Vec<String>>>,
    /// Assert P2 at every forget: the argument must not exist when the spawn
    /// happens. Off for the one fixture whose recorded path deliberately
    /// resolves into another repository, where the point is git's refusal.
    forgets_must_be_absent: bool,
}

impl Runner for Recording<'_> {
    fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
        let argv = spec.invocation.argv();
        if self.forgets_must_be_absent
            && let Some(at) = argv.iter().position(|arg| arg == "remove")
            && argv.iter().any(|arg| arg == "worktree")
        {
            let target = argv[at..]
                .iter()
                .skip(1)
                .find(|arg| !arg.starts_with("--"))
                .expect("worktree remove names a path");
            assert!(
                !Path::new(target).exists(),
                "P2: the forget's argument must not exist at the moment the forget is \
                 invoked, and {target} does"
            );
        }
        self.calls.borrow_mut().push(argv);
        self.real.capture(spec)
    }

    fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
        self.real.passthrough(spec)
    }

    fn session(&self, spec: &SpawnSpec, on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
        self.real.session(spec, on_stderr_line)
    }

    fn detach(&self, what: &Invocation) -> DetachOutcome {
        self.real.detach(what)
    }
}

fn clone_bare(cwd: &Path, from: &Path, to: &Path) {
    run_git(
        cwd,
        &[
            "clone",
            "--bare",
            &from.display().to_string(),
            &to.display().to_string(),
        ],
    );
}

fn commit(work: &Path, message: &str) {
    run_git(work, &["add", "-A"]);
    run_git(work, &["commit", "-m", message]);
}

// ---------------------------------------------------------------------------
// the tagged derivatives (devlaunch#468)
// ---------------------------------------------------------------------------

/// The Cache Directory Tagging Specification's file, as a real writer emits it.
fn cachedir_tag(at: &Path) {
    std::fs::write(
        at.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n\
         # This file is a cache directory tag created by a build tool.\n",
    )
    .expect("a cache tag");
}

/// An installed pixi environment inside `site`, with the lockfile that
/// re-derives it beside it — the shape 18 of the 72 directories on the
/// reference host carried, and the difference between 104 GB and about 10.
///
/// `.pixi/config.toml` is here because it is the one path `.pixi/.gitignore`
/// un-ignores and the one human-writable file in there: the tag sits one level
/// below it, which is what makes the unit's edge pixi's own rather than
/// devlaunch's.
fn installed_env(site: &Path, environment: &str) -> PathBuf {
    let pixi = site.join(".pixi");
    let env = pixi.join("envs").join(environment);
    std::fs::create_dir_all(env.join("conda-meta")).expect("an installed environment");
    std::fs::write(pixi.join("config.toml"), "[repodata-config]\n").expect("pixi's own config");
    std::fs::write(pixi.join(".gitignore"), "*\n!config.toml\n").expect("pixi's own ignore");
    cachedir_tag(&env);
    std::fs::write(
        env.join("conda-meta").join("pixi"),
        format!(
            "{{\"manifest_path\": \"/workspaces/devlaunch-container/pyproject.toml\", \
             \"environment_name\": \"{environment}\"}}"
        ),
    )
    .expect("pixi's own record");
    std::fs::create_dir_all(env.join("lib")).expect("the environment's lib");
    std::fs::write(env.join("lib").join("libthing.so"), "a great many bytes\n")
        .expect("environment bytes");
    std::fs::write(
        site.join("pixi.lock"),
        format!("version: 7\nenvironments:\n  {environment}:\n    channels: []\npackages: []\n"),
    )
    .expect("the lockfile that re-derives it");
    env
}

/// Commit and push everything in a site except the environment, and gitignore
/// the environment the way every pixi project gitignores it.
///
/// The ordinary finished-task shape: the lockfile and the ignore file are on the
/// forge, so the only bytes in the site that exist nowhere else are the ones
/// under the tag — which is exactly the population devlaunch#468 is about.
fn commit_the_project(world: &Clone, worktree: &Path, branch: &str) {
    std::fs::write(worktree.join(".gitignore"), ".pixi/\n").expect("an ignore file");
    commit(worktree, "the project and its lockfile");
    run_git(&world.clone, &["push", "origin", branch]);
    world.fetch();
}

/// `git worktree lock` by hand, for a registration whose recorded path no longer
/// resolves — which is every registration a host sees, and which git's own
/// `worktree lock` will not take as an argument.
fn lock_by_hand(world: &Clone, leaf: &str) {
    std::fs::write(
        world
            .clone
            .join(".git")
            .join("worktrees")
            .join(leaf)
            .join("locked"),
        "",
    )
    .expect("the lock git's own listing reads");
}

/// The derivatives a plan will reclaim, by the place each sits at.
fn reclaiming(found: &CloneWorktrees) -> Vec<String> {
    found
        .derivatives()
        .iter()
        .filter(|it| it.derivable().is_some())
        .map(|it| it.at().as_str().to_owned())
        .collect()
}

/// The derivatives a plan names and will not reclaim, with why.
fn standing_derivatives(found: &CloneWorktrees) -> Vec<(String, String)> {
    found
        .derivatives()
        .iter()
        .filter_map(|it| Some((it.at().as_str().to_owned(), it.standing()?)))
        .collect()
}

/// The directories a plan removes, in the order it reports them.
fn going_dirs(found: &CloneWorktrees) -> Vec<PathBuf> {
    found
        .going()
        .iter()
        .filter_map(|going| match going.what() {
            Collectable::Directory(directory) => Some(directory.at().to_path_buf()),
            Collectable::Registration(_) => None,
        })
        .collect()
}

/// The registration-only forgets a plan carries.
fn going_places(found: &CloneWorktrees) -> Vec<String> {
    found
        .going()
        .iter()
        .filter_map(|going| match going.what() {
            Collectable::Directory(_) => None,
            Collectable::Registration(registration) => {
                Some(registration.place().as_str().to_owned())
            }
        })
        .collect()
}

/// The paths of the standing sites, own reasons only.
fn standing_paths(found: &CloneWorktrees) -> Vec<PathBuf> {
    found
        .standing()
        .iter()
        .map(|site| site.at().to_path_buf())
        .collect()
}

/// The reasons `path`'s site stands on, failing loudly when it is not reported
/// at all — because "it is still on disk" is true of a site kept for the right
/// reason and of one no verdict ever looked at.
fn reasons_at<'a>(found: &'a CloneWorktrees, path: &Path) -> Vec<&'a Reason> {
    let mut matched: Vec<&StandingSite> = found
        .standing()
        .iter()
        .filter(|site| site.at() == path)
        .collect();
    assert_eq!(
        matched.len(),
        1,
        "expected exactly one standing line for {}: {:?}",
        path.display(),
        found.standing()
    );
    matched.remove(0).reasons().iter().collect()
}

// =======================================================================
// a clone with nothing in it costs nothing
// =======================================================================

#[test]
fn a_clone_with_no_worktrees_directory_is_not_swept_at_all() {
    // The answer for nearly every clone, and the reason this is affordable on
    // the prune path: no `git worktree list`, no disk walk.
    let world = Clone::new();

    assert!(world.sweep(Insistence::NotInsisted).is_none());
}

// =======================================================================
// the ordinary shapes: registered, finished, and their refusals
// =======================================================================

#[test]
fn a_registered_worktree_clean_and_pushed_is_collectable() {
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    world.containerise();

    let plan = world.plan();

    assert_eq!(going_dirs(&plan), [worktree]);
    assert!(plan.standing().is_empty(), "{:?}", plan.standing());
    let Collectable::Directory(directory) = plan.going()[0].what() else {
        panic!("expected a directory unit");
    };
    assert_eq!(
        directory.forgets().len(),
        1,
        "the one removal accounts for the one registration"
    );
}

#[test]
fn an_uncommitted_edit_stands_a_worktree_that_is_otherwise_collectable() {
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    std::fs::write(worktree.join("notes.md"), "an afternoon\n").expect("a note");
    world.containerise();

    let plan = world.plan();

    assert!(going_dirs(&plan).is_empty(), "{:?}", plan.going());
    let reasons = reasons_at(&plan, &worktree);
    assert!(matches!(reasons[0], Reason::Holds { .. }), "{reasons:?}");
}

#[test]
fn a_site_whose_only_content_is_gitignored_is_collectable_as_a_clone_would_be() {
    // **The limit, pinned as behaviour rather than left to prose.** Ignored
    // content is not weighed here, because it is not weighed of a whole clone
    // either — `dl <ws> rm` and `--prune`'s orphan arm both `rm -rf` past a
    // clone's own ignored bytes — and one conjunction wants one definition of
    // dirty. Weighing it at this scope alone was tried and reverted: an
    // installed `.pixi/envs/default` is exactly this shape and is the whole
    // reason these directories are worth reclaiming, so it put the sweep's
    // entire yield behind `--force-worktrees`, the flag that also carries past
    // a lock and past another repository's worktree.
    //
    // The .gitignore itself is committed and pushed, so the ignored file really
    // is the only thing here that exists nowhere else. That is the honest
    // statement of what goes.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    std::fs::write(worktree.join(".gitignore"), "scratch.db\n").expect("an ignore file");
    commit(&worktree, "ignore scratch");
    run_git(&world.clone, &["push", "origin", "agent-one"]);
    world.fetch();
    std::fs::write(worktree.join("scratch.db"), "bytes nowhere else\n").expect("ignored bytes");
    world.containerise();

    let plan = world.plan();

    assert_eq!(going_dirs(&plan), std::slice::from_ref(&worktree));
    assert!(plan.standing().is_empty(), "{:?}", plan.standing());
}

#[test]
fn an_installed_environment_does_not_need_the_flag_that_disables_the_guards() {
    // The shape the whole ticket exists for: 18 of the 72 directories on the
    // reference host carried a whole `.pixi/envs/default`, and they are the
    // difference between 104 GB and about 10. `.pixi/` is gitignored the way
    // every pixi project gitignores it.
    //
    // Asserted as the absence of a flag rather than as a byte count: the
    // failure this pins is not "the env stays", it is "reclaiming the env costs
    // you the lock guard, the foreign-repository guard and the unpushed guard,
    // all at once, because one flag carries past all of them".
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    std::fs::write(worktree.join(".gitignore"), ".pixi/\n").expect("an ignore file");
    commit(&worktree, "ignore the env");
    run_git(&world.clone, &["push", "origin", "agent-one"]);
    world.fetch();
    let env = worktree
        .join(".pixi")
        .join("envs")
        .join("default")
        .join("lib");
    std::fs::create_dir_all(&env).expect("an installed environment");
    std::fs::write(env.join("libthing.so"), "a great many bytes\n").expect("env bytes");
    world.containerise();

    let plan = world.plan();

    assert_eq!(
        going_dirs(&plan),
        std::slice::from_ref(&worktree),
        "an env must not need --force-worktrees: {:?}",
        plan.standing()
    );
    let WorktreePromotion::Unopposed = plan.going()[0].promotion() else {
        panic!("nothing objected, so nothing was insisted past");
    };
}

#[test]
fn a_commit_that_was_never_pushed_stands_the_worktree() {
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    std::fs::write(worktree.join("work.md"), "work\n").expect("work");
    commit(&worktree, "work nowhere else");
    world.containerise();

    let plan = world.plan();

    assert!(going_dirs(&plan).is_empty());
    let reasons = reasons_at(&plan, &worktree);
    assert!(
        reasons.iter().any(|reason| matches!(
            reason,
            Reason::Holds { losses, .. }
                if losses.describe().contains("unpushed commit")
        )),
        "{reasons:?}"
    );
}

#[test]
fn a_detached_head_that_is_ahead_stands_and_one_the_cache_reaches_goes() {
    let world = Clone::new();
    let reached = world.worktree("agent-one");
    let head = run_git(&world.clone, &["rev-parse", "HEAD"]);
    let detached = worktrees_dir(&world.clone).join("agent-two");
    run_git(
        &world.clone,
        &[
            "worktree",
            "add",
            "--detach",
            &detached.display().to_string(),
            head.trim(),
        ],
    );
    // Turn agent-one detached too, onto a commit the cache never saw.
    run_git(&reached, &["checkout", "--detach"]);
    std::fs::write(reached.join("ahead.md"), "ahead\n").expect("ahead");
    commit(&reached, "detached and ahead");
    world.containerise();

    let plan = world.plan();

    assert_eq!(going_dirs(&plan), [detached]);
    let reasons = reasons_at(&plan, &reached);
    assert!(matches!(reasons[0], Reason::Holds { .. }), "{reasons:?}");
}

#[test]
fn work_pushed_after_the_clone_was_cut_is_found_in_the_cache() {
    // The stale-ref trap: the clone is never fetched into, so asking it alone
    // reports pushed-and-merged branches as unpushed forever. The bare next
    // door is the thing that gets fetched, and it is asked first.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    std::fs::write(worktree.join("late.md"), "late\n").expect("late work");
    commit(&worktree, "pushed after the clone was cut");
    run_git(&world.clone, &["push", "origin", "agent-one"]);
    world.fetch();
    world.containerise();

    let plan = world.plan();

    assert_eq!(going_dirs(&plan), [worktree]);
}

// =======================================================================
// T1: a worktree nested inside one that is going (devlaunch#442, review
// 5019431339). Three probes, each asserting the absence of a unit rather
// than a guard firing: the plan contains nothing that removes the parent.
// =======================================================================

#[test]
fn a_nested_worktree_holding_an_uncommitted_note_stands_everything_above_it() {
    let world = Clone::new();
    let outer = world.worktree("agent-outer");
    let inner = world.nested(&outer, "agent-inner");
    std::fs::write(inner.join("UNSAVED.txt"), "an afternoon nobody else has\n").expect("the note");
    world.containerise();

    let plan = world.plan();

    assert!(
        going_dirs(&plan).is_empty(),
        "no unit in this plan may contain the outer worktree: {:?}",
        plan.going()
    );
    // The printed line names the child, not the parent it pins.
    assert_eq!(standing_paths(&plan), std::slice::from_ref(&inner));
    let reasons = reasons_at(&plan, &inner);
    assert!(matches!(reasons[0], Reason::Holds { .. }), "{reasons:?}");
}

#[test]
fn a_nested_worktree_holding_an_unpushed_commit_stands_everything_above_it() {
    let world = Clone::new();
    let outer = world.worktree("agent-outer");
    let inner = world.nested(&outer, "agent-inner");
    std::fs::write(inner.join("work.md"), "work\n").expect("work");
    commit(&inner, "nested work nowhere else");
    world.containerise();

    let plan = world.plan();

    assert!(going_dirs(&plan).is_empty(), "{:?}", plan.going());
    assert_eq!(standing_paths(&plan), [inner]);
}

#[test]
fn a_locked_nested_worktree_cannot_be_inside_anything_that_goes() {
    // devlaunch#426 Ask 2, made structural: with no flag typed, a locked
    // worktree cannot be inside any unit the plan carries.
    let world = Clone::new();
    let outer = world.worktree("agent-outer");
    let inner = world.nested(&outer, "agent-inner");
    run_git(
        &world.clone,
        &[
            "worktree",
            "lock",
            "--reason",
            "claude session",
            &inner.display().to_string(),
        ],
    );
    world.containerise();

    let plan = world.plan();

    assert!(going_dirs(&plan).is_empty(), "{:?}", plan.going());
    let reasons = reasons_at(&plan, &inner);
    assert!(
        matches!(
            reasons[0],
            Reason::CouldNotProve {
                blank: Blank::ThirdPartyClaim(Some(reason)),
                ..
            } if reason == "claude session"
        ),
        "a lock is an unproved with the reason git printed, never a loss: {reasons:?}"
    );

    // The one flag carries the whole subtree past it, with the claim on the
    // plan line rather than swallowed.
    let insisted = world
        .sweep(Insistence::Insisted)
        .expect("a sweep of a clone that has worktrees");
    assert_eq!(going_dirs(&insisted), [outer]);
    let WorktreePromotion::Insisted { despite } = insisted.going()[0].promotion() else {
        panic!("insisting past a lock must say so");
    };
    assert!(despite.describe().contains("claude session"));
}

// =======================================================================
// the conjunction the other way round, and the byte recursion
// =======================================================================

#[test]
fn a_collectable_worktree_inside_a_kept_one_is_reclaimed_on_its_own() {
    let world = Clone::new();
    let outer = world.worktree("agent-outer");
    let inner = world.nested(&outer, "agent-inner");
    std::fs::write(outer.join("notes.md"), "outer work\n").expect("outer note");
    world.containerise();

    let plan = world.plan();

    assert_eq!(going_dirs(&plan), [inner]);
    let reasons = reasons_at(&plan, &outer);
    assert!(matches!(reasons[0], Reason::Holds { .. }), "{reasons:?}");
}

#[test]
fn a_collectable_subtree_is_one_unit_with_every_registration_riding_on_it() {
    // The byte recursion stops at the outermost thing that goes — one unit,
    // one figure, no double count — while the verdict recursion descended into
    // everything to earn it.
    let world = Clone::new();
    let outer = world.worktree("agent-outer");
    let inner = world.nested(&outer, "agent-inner");
    world.containerise();

    let plan = world.plan();

    assert_eq!(going_dirs(&plan), std::slice::from_ref(&outer));
    let Collectable::Directory(directory) = plan.going()[0].what() else {
        panic!("expected a directory unit");
    };
    assert_eq!(
        directory.forgets().len(),
        2,
        "the one removal accounts for both registrations"
    );

    let (report, _) = world.act(&plan);
    assert_eq!(report.removed.len(), 1);
    assert_eq!(report.forgotten, 2);
    assert!(!outer.exists());
    assert!(!inner.exists());
    let listing = world.listing();
    assert!(
        !listing.contains("agent-outer") && !listing.contains("agent-inner"),
        "both registrations are gone: {listing}"
    );
}

// =======================================================================
// T2: a registration created after the plan was printed. The metadata
// operation is per registration by name, and a name exists only because a
// listing printed it — so the new registration is never named. The spawn
// log is the absence assertion.
// =======================================================================

#[test]
fn a_registration_created_after_the_plan_is_never_named() {
    let world = Clone::new();
    let finished = world.worktree("agent-finished");
    world.containerise();
    let plan = world.plan();
    assert_eq!(going_dirs(&plan), std::slice::from_ref(&finished));

    // The container registers a new worktree while the question is on screen.
    let fresh = worktrees_dir(&world.clone).join("agent-fresh");
    run_git(
        &world.clone,
        &[
            "worktree",
            "add",
            "-b",
            "agent-fresh",
            &fresh.display().to_string(),
        ],
    );
    std::fs::write(fresh.join("notes.md"), "live work\n").expect("live work");

    let (report, calls) = world.act(&plan);

    assert_eq!(report.removed.len(), 1);
    assert!(!finished.exists());
    assert!(fresh.exists(), "the fresh worktree is untouched");
    assert!(
        world.listing().contains("agent-fresh"),
        "its registration is untouched too"
    );
    // The absence itself: no git invocation names a registration the pass did
    // not read from a listing, and the fresh worktree was approved by nobody.
    let fresh_spelled = fresh.display().to_string();
    assert!(
        !calls
            .iter()
            .filter(|argv| argv.iter().any(|arg| arg == "remove"))
            .any(|argv| argv.iter().any(|arg| arg.contains(&fresh_spelled))),
        "no removal invocation may name the fresh registration: {calls:?}"
    );
}

#[test]
fn a_clean_site_nested_into_an_approved_one_after_the_plan_is_not_absorbed() {
    // The half the sibling case above does not reach, and the one that was
    // wrong: matching the two passes by *identity* let a site created inside an
    // approved parent ride out on that parent's subtree removal. The unit count
    // was unchanged, the root matched, and a registration the plan never named
    // was handed to `git worktree remove`.
    //
    // Clean and nested are the two properties that make it slip through: dirty
    // would stand it, and a sibling would be a unit of its own with no approval.
    // So the unit is compared by its blast radius, not its root.
    let world = Clone::new();
    let outer = world.worktree("agent-outer");
    world.containerise();
    let plan = world.plan();
    assert_eq!(going_dirs(&plan), std::slice::from_ref(&outer));
    assert_eq!(
        plan.going()[0].forgets().len(),
        1,
        "the plan names one registration"
    );

    // The container makes a worktree *inside* the approved one while the
    // question is on screen. Nothing is written into it: it is finished work.
    let fresh = world.nested(&outer, "agent-fresh");

    let (report, calls) = world.act(&plan);

    assert!(report.removed.is_empty(), "{report:?}");
    assert_eq!(report.withheld.len(), 1, "{report:?}");
    assert_eq!(report.forgotten, 0, "{report:?}");
    assert!(outer.exists() && fresh.exists());
    // The absence: no *forget* names the registration the plan did not. The
    // fresh site is deliberately probed — it is classified like any other, which
    // is what makes it stand its parent — so the assertion is over the removal
    // invocations rather than over every call.
    let fresh_spelled = fresh.display().to_string();
    assert!(
        !calls
            .iter()
            .filter(|argv| argv.iter().any(|arg| arg == "remove"))
            .any(|argv| argv.iter().any(|arg| arg.contains(&fresh_spelled))),
        "{calls:?}"
    );
    // And the next run offers the whole subtree, which is now in the plan
    // somebody reads.
    let again = world.plan();
    assert_eq!(going_dirs(&again), std::slice::from_ref(&outer));
    assert_eq!(again.going()[0].forgets().len(), 2);
}

#[test]
fn what_the_report_says_was_freed_is_what_the_plan_measured() {
    // The two halves of one report have to mean the same thing. The acting pass
    // re-measures, so a subtree that grew between the two would be reported at
    // its new size against a plan somebody read at the old one. The clone arm
    // beside this one has always reported the plan's figure.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    // Ignored, so these bytes are in the figure without making the tree dirty —
    // which is exactly the shape a build output has.
    std::fs::write(worktree.join(".gitignore"), "big.bin\n").expect("an ignore file");
    commit(&worktree, "ignore the build output");
    run_git(&world.clone, &["push", "origin", "agent-one"]);
    world.fetch();
    std::fs::write(worktree.join("big.bin"), vec![7u8; 200_000]).expect("build output");
    world.containerise();
    let plan = world.plan();
    let Collectable::Directory(approved) = plan.going()[0].what() else {
        panic!("expected a directory unit");
    };
    let planned_bytes = approved.usage().known_bytes();
    assert!(planned_bytes > 100_000, "the plan measured the bytes");

    // A container tidies up after itself while the question is on screen, so
    // the acting pass would measure a much smaller directory.
    std::fs::remove_file(worktree.join("big.bin")).expect("tidied away");

    let (report, _) = world.act(&plan);

    assert_eq!(report.removed.len(), 1, "{report:?}");
    assert_eq!(
        report.freed().known_bytes(),
        planned_bytes,
        "the figure reported is the one that was said yes to"
    );
}

#[test]
fn a_worktree_that_went_dirty_while_the_question_was_open_is_withheld() {
    // The re-check is the same weighing as the plan, run again under the lock.
    // And with `git worktree prune` deleted there is no second operation for
    // the withheld worktree's registration to be lost to: the next run reads
    // it as registered-and-dirty, not as forgotten.
    let world = Clone::new();
    let finished = world.worktree("agent-finished");
    let unsaved = world.worktree("agent-unsaved");
    world.containerise();
    let plan = world.plan();
    assert_eq!(going_dirs(&plan).len(), 2, "{:?}", plan.going());

    // The write the plan on screen could not have known about.
    std::fs::write(unsaved.join("notes.md"), "an afternoon\n").expect("a note");

    let (report, _) = world.act(&plan);

    assert_eq!(report.removed.len(), 1);
    assert_eq!(report.withheld.len(), 1);
    assert!(!finished.exists(), "nothing objected to that one");
    assert!(unsaved.exists(), "it holds a note nowhere else");
    assert!(
        world.listing().contains("agent-unsaved"),
        "the registration is untouched: it is what tells the next run this is not \
         an unaccounted-for directory"
    );

    // And the run after it: the site stands, offered to nobody.
    let again = world.plan();
    assert!(going_dirs(&again).is_empty(), "{:?}", again.going());
    assert_eq!(
        std::fs::read_to_string(unsaved.join("notes.md")).expect("the note is still here"),
        "an afternoon\n"
    );
}

// =======================================================================
// locks at the top level, and what insisting means
// =======================================================================

#[test]
fn a_locked_worktree_is_never_removed_implicitly_and_the_flag_carries_it() {
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    run_git(
        &world.clone,
        &["worktree", "lock", &worktree.display().to_string()],
    );
    world.containerise();

    let plan = world.plan();
    assert!(going_dirs(&plan).is_empty());
    let reasons = reasons_at(&plan, &worktree);
    assert!(
        matches!(
            reasons[0],
            Reason::CouldNotProve {
                blank: Blank::ThirdPartyClaim(None),
                ..
            }
        ),
        "a lock taken with no reason carries no invented one: {reasons:?}"
    );

    let insisted = world
        .sweep(Insistence::Insisted)
        .expect("a sweep of a clone that has worktrees");
    assert_eq!(going_dirs(&insisted), std::slice::from_ref(&worktree));
    let (report, _) = world.act(&insisted);
    assert_eq!(report.removed.len(), 1);
    assert_eq!(report.forgotten, 1, "{:?}", report.forget_refused);
    assert!(!worktree.exists());
    assert!(!world.listing().contains("agent-one"));
}

#[test]
fn a_locked_and_dirty_worktree_reports_both_reasons() {
    // A report naming one of two reasons is telling half the truth; the
    // standing accumulates rather than short-circuits.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    std::fs::write(worktree.join("notes.md"), "work\n").expect("work");
    run_git(
        &world.clone,
        &["worktree", "lock", &worktree.display().to_string()],
    );
    world.containerise();

    let plan = world.plan();

    let reasons = reasons_at(&plan, &worktree);
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(reasons.iter().any(|it| matches!(it, Reason::Holds { .. })));
    assert!(
        reasons
            .iter()
            .any(|it| matches!(it, Reason::CouldNotProve { .. }))
    );
}

// =======================================================================
// registrations with nothing at their place (devlaunch#446 §5a)
// =======================================================================

#[test]
fn a_registration_with_nothing_here_is_forgotten_once_its_commits_are_reached() {
    let world = Clone::new();
    let worktree = world.worktree("agent-gone");
    world.containerise();
    std::fs::remove_dir_all(&worktree).expect("removed by hand");

    let plan = world.plan();
    assert_eq!(going_places(&plan), [".claude/worktrees/agent-gone"]);

    let (report, _) = world.act(&plan);
    assert_eq!(report.forgotten, 1);
    assert!(!world.listing().contains("agent-gone"));

    // And the run after that has nothing to offer.
    let again = world.plan();
    assert!(again.going().is_empty(), "{:?}", again.going());
    assert!(again.standing().is_empty(), "{:?}", again.standing());
}

#[test]
fn emptiness_alone_never_carries_a_registered_site() {
    // Measured on devlaunch#446: a registration can be the last ref reaching a
    // detached worktree's commits, so forgetting it on emptiness alone hands
    // the commits to the next gc. The reachability question is asked whether
    // or not there are bytes.
    let world = Clone::new();
    let worktree = world.worktree("agent-gone");
    run_git(&worktree, &["checkout", "--detach"]);
    std::fs::write(worktree.join("late.md"), "work\n").expect("work");
    commit(&worktree, "reachable only from this registration");
    world.containerise();
    std::fs::remove_dir_all(&worktree).expect("removed by hand");

    let plan = world.plan();

    assert!(plan.going().is_empty(), "{:?}", plan.going());
    let reasons = reasons_at(&plan, &worktree);
    assert!(matches!(reasons[0], Reason::Holds { .. }), "{reasons:?}");
    assert!(
        world.listing().contains("agent-gone"),
        "the registration is what keeps those commits alive"
    );
}

// =======================================================================
// sites this clone cannot account for
// =======================================================================

#[test]
fn a_plain_directory_in_the_worktrees_place_stands_and_is_reported() {
    let world = Clone::new();
    world.worktree("agent-one");
    let plain = worktrees_dir(&world.clone).join("not-a-worktree");
    std::fs::create_dir_all(&plain).expect("a plain directory");
    std::fs::write(plain.join("keep.txt"), "keep\n").expect("their content");
    world.containerise();

    let plan = world.plan();

    let reasons = reasons_at(&plan, &plain);
    assert!(
        matches!(
            reasons[0],
            Reason::CouldNotProve {
                blank: Blank::NotThisClonesToAccountFor(Unaccountable::PlainDirectory),
                ..
            }
        ),
        "{reasons:?}"
    );
    let (report, _) = world.act(&plan);
    assert!(plain.exists(), "not devlaunch's to remove: {report:?}");
}

#[test]
fn a_gitfile_that_does_not_normalise_is_not_a_worktree_and_stands() {
    // devlaunch#442 review S6, carried: a `..` tail would name the clone's own
    // `.git`. There is no deleting arm for it to land on any more — the site
    // stands, and the reason names what could not be read.
    let world = Clone::new();
    world.worktree("agent-one");
    let doctored = worktrees_dir(&world.clone).join("doctored");
    std::fs::create_dir_all(&doctored).expect("a directory");
    std::fs::write(doctored.join(".git"), "gitdir: /x/.git/worktrees/..\n")
        .expect("a doctored gitfile");
    world.containerise();

    let plan = world.plan();

    let reasons = reasons_at(&plan, &doctored);
    assert!(
        matches!(
            reasons[0],
            Reason::CouldNotProve {
                blank: Blank::NotThisClonesToAccountFor(Unaccountable::GitfileUnreadable),
                ..
            }
        ),
        "{reasons:?}"
    );
}

#[test]
fn a_symlink_in_the_worktrees_place_is_never_followed() {
    let world = Clone::new();
    world.worktree("agent-one");
    let elsewhere = world.tmp().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("a directory outside the clone");
    std::fs::write(elsewhere.join("keep.txt"), "keep\n").expect("content");
    let link = worktrees_dir(&world.clone).join("a-link");
    std::os::unix::fs::symlink(&elsewhere, &link).expect("a symlink");
    world.containerise();

    let plan = world.plan();

    let reasons = reasons_at(&plan, &link);
    assert!(
        matches!(
            reasons[0],
            Reason::CouldNotProve {
                blank: Blank::NotThisClonesToAccountFor(Unaccountable::SymlinkInThePlace),
                ..
            }
        ),
        "{reasons:?}"
    );
    assert!(elsewhere.join("keep.txt").exists());
}

// =======================================================================
// a worktree of a different repository (devlaunch#463)
// =======================================================================

#[test]
fn a_foreign_worktree_stands_its_ancestors_and_is_never_probed_or_named() {
    // The measured defect: this exact shape was offered for removal unopposed
    // under "git has already forgotten it", and answering `y` destroyed the
    // other repository's uncommitted work and left it unable to check out its
    // branch. Ownership is a registration join now, so the site stands, every
    // ancestor stands, and no git invocation of ours ever names it.
    let world = Clone::new();
    let outer = world.worktree("agent-outer");
    let other = OtherRepository::new(world.tmp());
    let theirs = other.worktree_at(&worktrees_dir(&outer).join("nested2"), "feat");
    world.containerise();

    let plan = world.plan();

    assert!(
        going_dirs(&plan).is_empty(),
        "the ancestor is pinned: {:?}",
        plan.going()
    );
    // One line, attributed to the foreign site rather than to each ancestor.
    assert_eq!(standing_paths(&plan), std::slice::from_ref(&theirs));
    let reasons = reasons_at(&plan, &theirs);
    assert!(
        matches!(
            reasons[0],
            Reason::CouldNotProve {
                blank: Blank::NotThisClonesToAccountFor(Unaccountable::RegisteredElsewhere),
                ..
            }
        ),
        "{reasons:?}"
    );

    let (report, calls) = world.act(&plan);
    assert!(
        report.removed.is_empty() && report.forgotten == 0,
        "{report:?}"
    );
    assert!(theirs.join("UNSAVED.txt").exists());
    // No invocation names the foreign registration — probe or forget.
    let theirs_spelled = theirs.display().to_string();
    assert!(
        !calls
            .iter()
            .any(|argv| argv.iter().any(|arg| arg.contains(&theirs_spelled))),
        "{calls:?}"
    );
    // The other repository is untouched: still listing its worktree, with no
    // prunable line, its work intact.
    let their_listing = other.listing();
    assert!(their_listing.contains("nested2"), "{their_listing}");
    assert!(!their_listing.contains("prunable"), "{their_listing}");
}

#[test]
fn a_foreign_leaf_colliding_with_our_admin_name_is_not_probed_through_our_index() {
    // devlaunch#446 §8's suffix-join hazard, closed structurally: the admin
    // directory is derived from the joined registration's recorded path, so a
    // foreign worktree whose leaf name matches one of our admin directories can
    // never be handed our index. Two worktrees cut from the same commit with no
    // edits would read as a clean Q2 through a borrowed index — the fail-safe
    // the old shape leaned on does not exist here to lean on.
    let world = Clone::new();
    let outer = world.worktree("agent-outer");
    let other = OtherRepository::new(world.tmp());
    // The foreign worktree's leaf is `agent-outer`, so its own admin directory
    // over in the other repository is named `agent-outer` — the same name as
    // ours.
    let theirs = other.worktree_at(&worktrees_dir(&outer).join("agent-outer"), "agent-outer");
    world.containerise();

    let calls = RefCell::new(Vec::new());
    let runner = Recording {
        real: ProcessRunner::new(),
        calls: &calls,
        forgets_must_be_absent: true,
    };
    let git = Git::new(&runner);
    let plan = sweep_clone(
        &git,
        &world.clone,
        OWNER,
        REPO,
        Some(&world.bare),
        Insistence::NotInsisted,
    )
    .expect("a sweep");

    let reasons = reasons_at(&plan, &theirs);
    assert!(
        matches!(
            reasons[0],
            Reason::CouldNotProve {
                blank: Blank::NotThisClonesToAccountFor(Unaccountable::RegisteredElsewhere),
                ..
            }
        ),
        "{reasons:?}"
    );
    // The absence: no status probe was pointed at the foreign directory
    // through anything of ours.
    let theirs_spelled = format!("--work-tree={}", theirs.display());
    assert!(
        !calls
            .borrow()
            .iter()
            .any(|argv| argv.iter().any(|arg| arg == &theirs_spelled)),
        "the foreign site must never be probed: {:?}",
        calls.borrow()
    );
}

#[test]
fn a_recorded_path_resolving_into_another_clone_is_refused_and_contained() {
    // The suffix join pairs a registration with the directory at its place —
    // and the forget's argument is the recorded path, which git resolves by
    // path, not by registration identity. Doctored to point into another
    // repository's live worktree, the forget is refused by git, nothing else is
    // forgotten, and the other repository's bytes are untouched.
    let world = Clone::new();
    let ours = world.worktree("agent-one");
    world.containerise();
    // Another repository, holding a live worktree whose path carries the same
    // `.claude/worktrees/agent-one` suffix as ours.
    let other = OtherRepository::new(world.tmp());
    let their_place = other
        .repo
        .join(".claude")
        .join("worktrees")
        .join("agent-one");
    let theirs = other.worktree_at(&their_place, "agent-one");
    // The doctored gitdir: our registration now records their path.
    let gitdir = world
        .clone
        .join(".git")
        .join("worktrees")
        .join("agent-one")
        .join("gitdir");
    std::fs::write(&gitdir, format!("{}/.git\n", theirs.display())).expect("doctored");

    let plan = world.plan();
    // The join still pairs the registration with our directory at that place,
    // and the site reads collectable — the hazard is all in the forget.
    assert_eq!(going_dirs(&plan), std::slice::from_ref(&ours));

    let (report, _) = world.act_expecting_absent_forgets(&plan, false);

    assert!(!ours.exists(), "our directory went");
    assert_eq!(
        report.forget_refused.len(),
        1,
        "git refused the forget whose argument resolves into another repository: {report:?}"
    );
    assert!(
        theirs.join("UNSAVED.txt").exists(),
        "the other repository's work is untouched"
    );
    let their_listing = other.listing();
    assert!(their_listing.contains("agent-one"), "{their_listing}");
}

// =======================================================================
// the resolving fixture (devlaunch#462): registrations that record paths
// which really do resolve to worktrees of this clone
// =======================================================================

#[test]
fn a_resolving_site_with_uncommitted_work_stands() {
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    std::fs::write(worktree.join("notes.md"), "work\n").expect("work");
    // No containerise: the recorded paths resolve.

    let plan = world.plan();

    assert!(going_dirs(&plan).is_empty(), "{:?}", plan.going());
    let reasons = reasons_at(&plan, &worktree);
    assert!(matches!(reasons[0], Reason::Holds { .. }), "{reasons:?}");
}

#[test]
fn a_collected_resolving_site_is_forgotten_only_after_its_directory_is_gone() {
    // P2 asserted directly: the recorded path does not resolve at the moment
    // the forget is invoked, even though it resolved a moment earlier. The
    // recording runner asserts it at the spawn itself.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");

    let plan = world.plan();
    assert_eq!(going_dirs(&plan), std::slice::from_ref(&worktree));

    let (report, calls) = world.act(&plan);

    assert_eq!(report.removed.len(), 1);
    assert_eq!(report.forgotten, 1, "{:?}", report.forget_refused);
    assert!(!worktree.exists());
    assert!(!world.listing().contains("agent-one"));
    assert!(
        calls
            .iter()
            .any(|argv| argv.iter().any(|arg| arg == "remove")),
        "the forget really was a per-registration git invocation: {calls:?}"
    );
}

// =======================================================================
// parsing details that carry the join
// =======================================================================

#[test]
fn the_clones_own_entry_and_worktrees_elsewhere_are_not_candidates() {
    let world = Clone::new();
    world.worktree("agent-one");
    // A worktree of this clone parked outside any worktrees place.
    let elsewhere = world.tmp().join("parked");
    run_git(
        &world.clone,
        &[
            "worktree",
            "add",
            "-b",
            "parked",
            &elsewhere.display().to_string(),
        ],
    );
    world.containerise();

    let plan = world.plan();

    assert_eq!(going_dirs(&plan).len(), 1);
    assert!(plan.standing().is_empty(), "{:?}", plan.standing());
    assert!(elsewhere.exists());
}

#[test]
fn a_nested_registration_keeps_its_whole_place_inside_the_clone() {
    // The join key is the suffix from the first `.claude/worktrees` onwards, so
    // a nested worktree cannot be confused with a same-named one at the top.
    let world = Clone::new();
    let outer = world.worktree("agent-a");
    let inner = world.nested(&outer, "agent-a-nested");
    world.containerise();

    let plan = world.plan();

    assert_eq!(going_dirs(&plan), [outer]);
    let Collectable::Directory(directory) = plan.going()[0].what() else {
        panic!("expected a directory unit");
    };
    let places: Vec<String> = directory
        .forgets()
        .iter()
        .map(|recorded| recorded.as_path().display().to_string())
        .collect();
    assert!(
        places
            .iter()
            .any(|place| place.ends_with("agent-a/.claude/worktrees/agent-a-nested")),
        "{places:?}"
    );
    drop(inner);
}

// =======================================================================
// the clone as the root of the same forest
// =======================================================================

#[test]
fn a_clean_clone_with_a_dirty_gitignored_worktree_stands_on_the_sites_account() {
    // The nested half of the dirt blindness (devlaunch#459 by way of #446):
    // `.claude/` is gitignored, so the clone's own status says nothing, and
    // the shipped guard read this exact shape as nothing to lose.
    let world = Clone::new();
    std::fs::write(world.clone.join(".gitignore"), ".claude/\n").expect("gitignore");
    commit(&world.clone, "ignore the agent worktrees");
    run_git(&world.clone, &["push", "origin", "main"]);
    world.fetch();
    let worktree = world.worktree("agent-one");
    std::fs::write(worktree.join("UNSAVED.txt"), "an afternoon\n").expect("the note");
    world.containerise();

    let runner = ProcessRunner::new();
    let git = Git::new(&runner);
    let verdict = clone_verdict(&git, &world.clone, BareCache::At(&world.bare));

    let Verdict::Stands(standing) = &verdict else {
        panic!("the clone must stand on the site's account, got {verdict:?}");
    };
    let json = verdict.unsaved_json();
    let would_lose = json["wouldLose"].as_str().expect("a wouldLose key");
    assert!(
        would_lose.contains("agent-one"),
        "the loss is attributed to the site holding it: {would_lose}"
    );
    assert!(standing.would_lose().is_some());
}

#[test]
fn a_standing_that_holds_both_kinds_emits_both_wire_keys() {
    // The flattening is additive, never lossy: a dirty clone with a locked
    // worktree in it says both things, in both keys, so no reader keyed on key
    // presence breaks and no answer is dropped to fit the shape.
    let world = Clone::new();
    std::fs::write(world.clone.join(".gitignore"), ".claude/\n").expect("gitignore");
    commit(&world.clone, "ignore the agent worktrees");
    run_git(&world.clone, &["push", "origin", "main"]);
    world.fetch();
    let worktree = world.worktree("agent-one");
    run_git(
        &world.clone,
        &["worktree", "lock", &worktree.display().to_string()],
    );
    std::fs::write(world.clone.join("dirty.md"), "clone-level work\n").expect("clone dirt");
    world.containerise();

    let runner = ProcessRunner::new();
    let git = Git::new(&runner);
    let json = clone_verdict(&git, &world.clone, BareCache::At(&world.bare)).unsaved_json();

    assert!(json["wouldLose"].is_string(), "{json}");
    assert!(json["couldNotTell"].is_string(), "{json}");
    assert!(json["nothingToLose"].is_null(), "{json}");
}

#[test]
fn a_workspace_an_agent_built_in_does_not_start_refusing_dl_rm() {
    // The ordinary loop: open a workspace, an agent works in it and installs an
    // environment, `dl <ws> rm`. The clone verdict is the `rm` guard, so a site
    // that stood for its build output would make the daily command refuse, and
    // the only way past would be `dl <ws> rm --force` -- which also carries past
    // the clone's own unpushed commits, which is the #171 guard this must not
    // teach anybody to type.
    //
    // Pinned against the shipped answer rather than against a shape: this reads
    // the same as the build before the sweep existed.
    let world = Clone::new();
    std::fs::write(world.clone.join(".gitignore"), ".claude/\n.pixi/\n").expect("gitignore");
    commit(&world.clone, "ignore the agent's working directories");
    run_git(&world.clone, &["push", "origin", "main"]);
    world.fetch();
    let worktree = world.worktree("agent-one");
    let env = worktree.join(".pixi").join("envs").join("default");
    std::fs::create_dir_all(&env).expect("an installed environment");
    std::fs::write(env.join("libthing.so"), "a great many bytes\n").expect("env bytes");
    world.containerise();

    let runner = ProcessRunner::new();
    let git = Git::new(&runner);
    let json = clone_verdict(&git, &world.clone, BareCache::At(&world.bare)).unsaved_json();

    assert_eq!(json, serde_json::json!({ "nothingToLose": true }), "{json}");
}

#[test]
fn a_clean_clone_with_collectable_worktrees_reads_as_nothing_to_lose() {
    // Collectable sites do not stand the clone: the verdict conjoins standing
    // reasons, and a finished worktree contributes none.
    let world = Clone::new();
    std::fs::write(world.clone.join(".gitignore"), ".claude/\n").expect("gitignore");
    commit(&world.clone, "ignore the agent worktrees");
    run_git(&world.clone, &["push", "origin", "main"]);
    world.fetch();
    world.worktree("agent-one");
    world.containerise();

    let runner = ProcessRunner::new();
    let git = Git::new(&runner);
    let json = clone_verdict(&git, &world.clone, BareCache::At(&world.bare)).unsaved_json();

    assert_eq!(json, serde_json::json!({ "nothingToLose": true }));
}

// ---------------------------------------------------------------------------
// reclaiming the tagged derivative subtrees (devlaunch#468, devlaunch#472)
// ---------------------------------------------------------------------------

#[test]
fn an_environment_inside_a_standing_worktree_is_reclaimed_and_the_worktree_stands() {
    // The fixture devlaunch#468 asks for: a site that must stand, holding both
    // a `.pixi/envs/default` and a file a human wrote. The first goes, the
    // second stays, and the site still stands.
    //
    // What makes that legitimate rather than the thin end of the wedge: the
    // site stands on `Holds { Uncommitted }`, which is git's account of its
    // content, and nothing under `.pixi/envs/` has ever been in that account —
    // pixi's own `.pixi/.gitignore` puts it outside every index, every status
    // and every commit. The two sets of bytes are disjoint by construction, and
    // the construction is a file the installer wrote.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    let env = installed_env(&worktree, "default");
    commit_the_project(&world, &worktree, "agent-one");
    std::fs::write(worktree.join("NOTES.md"), "an afternoon nobody else has\n")
        .expect("the human's own file");
    world.containerise();

    let plan = world.plan();

    assert!(going_dirs(&plan).is_empty(), "the site must stand");
    assert_eq!(
        reclaiming(&plan),
        vec![".claude/worktrees/agent-one/.pixi/envs/default"],
    );

    let (report, _) = world.act(&plan);

    assert_eq!(
        report
            .reclaimed
            .iter()
            .map(|it| it.path.clone())
            .collect::<Vec<_>>(),
        vec![env.clone()],
    );
    assert!(!env.exists(), "the tagged directory is what goes");
    assert!(
        worktree.join(".pixi").join("config.toml").is_file(),
        "`.pixi` holds config.toml and is never what goes"
    );
    assert!(
        worktree.join("NOTES.md").is_file(),
        "the site still stands, and so does what it holds"
    );
    assert!(worktree.is_dir(), "the site still stands");
}

#[test]
fn reclaiming_an_environment_needs_no_flag_and_no_second_question() {
    // devlaunch#459 refused one flag carrying two consents, and this is a
    // removal with a proof rather than a force. It rides `--prune`'s own y/N:
    // the plan says which directory and how big, and nothing else is typed.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    installed_env(&worktree, "default");
    commit_the_project(&world, &worktree, "agent-one");
    std::fs::write(worktree.join("NOTES.md"), "unsaved\n").expect("the human's own file");
    world.containerise();

    let plan = world
        .sweep(Insistence::NotInsisted)
        .expect("a sweep of a clone that has worktrees");

    assert_eq!(reclaiming(&plan).len(), 1, "no flag was typed");
    assert!(
        !plan.nothing_to_do(),
        "a run with a derivative to reclaim has something to do, so the question is asked"
    );
}

#[test]
fn the_plan_names_each_derivative_and_its_size_before_the_question() {
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    installed_env(&worktree, "default");
    commit_the_project(&world, &worktree, "agent-one");
    std::fs::write(worktree.join("NOTES.md"), "unsaved\n").expect("the human's own file");
    world.containerise();

    let plan = world.plan();

    let [tagged] = plan.derivatives() else {
        panic!("one derivative: {:?}", plan.derivatives());
    };
    assert!(
        tagged.usage().known_bytes() > 0,
        "a plan line with no figure is a y/N answering a total nobody can decompose"
    );
    let Some(derivative) = tagged.derivable() else {
        panic!("derivable: {tagged:?}");
    };
    let Recipe::PixiEnvironment { environment, lock } = derivative.recipe();
    assert_eq!(environment, "default");
    assert_eq!(lock.as_str(), ".claude/worktrees/agent-one/pixi.lock");
}

#[test]
fn a_locked_worktrees_environment_is_named_and_never_reclaimed() {
    // A lock is a claim over the directory by a party this pass cannot
    // interrogate, and it makes no distinction between the directory's parts.
    // It may also mean *running right now*. devlaunch#426 Ask 2 holds at the
    // subtree level for the same reason it holds at the site.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    installed_env(&worktree, "default");
    run_git(
        &world.clone,
        &["worktree", "lock", &worktree.display().to_string()],
    );
    world.containerise();

    let plan = world.plan();

    assert!(reclaiming(&plan).is_empty(), "a claim pins the subtree");
    let [(at, why)] = &standing_derivatives(&plan)[..] else {
        panic!("named with its bytes: {:?}", plan.derivatives());
    };
    assert_eq!(at, ".claude/worktrees/agent-one/.pixi/envs/default");
    assert!(why.contains("locked"), "{why}");
}

#[test]
fn an_environment_inside_a_worktree_that_is_going_is_not_reported_twice() {
    // devlaunch#446 §6's two-recursions rule, extended one artifact over: the
    // byte recursion stops at the outermost thing that goes, so a derivative
    // inside a site that is itself going rides on the site's own figure and is
    // never a unit of its own. Two lines for one set of bytes is the double
    // count R3 forbids.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    installed_env(&worktree, "default");
    commit_the_project(&world, &worktree, "agent-one");
    world.containerise();

    let plan = world.plan();

    assert_eq!(going_dirs(&plan), std::slice::from_ref(&worktree));
    assert!(
        plan.derivatives().is_empty(),
        "the site's own removal accounts for it: {:?}",
        plan.derivatives()
    );
}

#[test]
fn a_planted_file_inside_an_environment_goes_with_it_and_the_row_says_so() {
    // The honest row, recorded rather than hidden. pixi does not defend its own
    // declaration: a planted `my-notes.txt` and a hand-written
    // `site-packages/mypkg` both survived `pixi install --frozen` unmentioned.
    // So the tag is a claim about the directory's purpose, not a proof about
    // its current contents, and the case for removal rests on the tagged
    // subtree being outside what the site's verdict is about.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    let env = installed_env(&worktree, "default");
    commit_the_project(&world, &worktree, "agent-one");
    std::fs::write(env.join("my-notes.txt"), "planted by hand\n").expect("a planted file");
    std::fs::write(worktree.join("NOTES.md"), "unsaved\n").expect("the human's own file");
    world.containerise();

    let plan = world.plan();
    let (report, _) = world.act(&plan);

    assert_eq!(report.reclaimed.len(), 1);
    assert!(
        !env.join("my-notes.txt").exists(),
        "everything under the tag goes, and the plan's own words are what warn about it"
    );
}

#[test]
fn a_node_modules_and_a_stdlib_venv_beside_an_environment_are_untouched() {
    // The row that proves the predicate never reads a name, at the level a
    // whole run decides. Both of these are shaped exactly like the population a
    // name-matcher would take — measured, npm writes no tag anywhere beneath
    // `node_modules` and `python -m venv` writes none at all — and the tagged
    // environment beside them goes.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    let env = installed_env(&worktree, "default");
    commit_the_project(&world, &worktree, "agent-one");
    std::fs::write(worktree.join("NOTES.md"), "unsaved\n").expect("the human's own file");
    let modules = worktree.join("node_modules").join("lodash");
    std::fs::create_dir_all(&modules).expect("an npm install");
    std::fs::write(modules.join("index.js"), "module.exports = {}\n").expect("a package");
    let venv = worktree.join(".venv").join("lib");
    std::fs::create_dir_all(&venv).expect("a stdlib venv");
    std::fs::write(
        worktree.join(".venv").join("pyvenv.cfg"),
        "home = /usr/bin\n",
    )
    .expect("a pyvenv.cfg");
    world.containerise();

    let plan = world.plan();

    assert_eq!(
        reclaiming(&plan),
        vec![".claude/worktrees/agent-one/.pixi/envs/default"],
        "only the one that declared itself: {:?}",
        plan.derivatives()
    );

    world.act(&plan);

    assert!(!env.exists());
    assert!(
        modules.join("index.js").is_file(),
        "npm never made the claim"
    );
    assert!(
        worktree.join(".venv").join("pyvenv.cfg").is_file(),
        "the stdlib venv never made the claim"
    );
}

#[test]
fn another_repositorys_environment_is_still_derivable() {
    // Named explicitly on devlaunch#468 §6 because a reviewer will ask. A
    // foreign worktree stands, and it stands on `NotThisClonesToAccountFor`,
    // which is git's account of content being out of scope rather than a
    // claimant's assertion. Whose repository the environment belongs to was
    // never part of the argument: the tag and the lockfile beside it say what
    // they say either way.
    let world = Clone::new();
    let theirs = OtherRepository::new(world.tmp());
    let at = worktrees_dir(&world.clone).join("theirs");
    theirs.worktree_at(&at, "their-branch");
    installed_env(&at, "default");

    let plan = world.plan();

    assert!(going_dirs(&plan).is_empty(), "a foreign site always stands");
    assert_eq!(
        reclaiming(&plan),
        vec![".claude/worktrees/theirs/.pixi/envs/default"],
    );
}

#[test]
fn an_environment_whose_lockfile_went_away_is_withheld_by_the_second_read() {
    // The acting pass re-reads both records, and it re-reads them with the same
    // weighing the plan ran, so the two passes cannot answer different
    // questions. The approved set shrinks here and can never grow.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    let env = installed_env(&worktree, "default");
    commit_the_project(&world, &worktree, "agent-one");
    std::fs::write(worktree.join("NOTES.md"), "unsaved\n").expect("the human's own file");
    world.containerise();

    let plan = world.plan();
    assert_eq!(reclaiming(&plan).len(), 1);

    std::fs::remove_file(worktree.join("pixi.lock")).expect("the lockfile going away");
    let (report, _) = world.act(&plan);

    assert!(report.reclaimed.is_empty(), "nothing was re-derivable");
    let [withheld] = &report.withheld_derivatives[..] else {
        panic!("one withheld: {report:?}");
    };
    assert_eq!(withheld.path, env);
    assert!(env.is_dir(), "and it is still there");
    assert!(
        withheld.because.describe().contains("lockfile"),
        "{}",
        withheld.because.describe()
    );
}

#[test]
fn an_environment_claimed_between_the_plan_and_the_act_is_withheld() {
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    let env = installed_env(&worktree, "default");
    commit_the_project(&world, &worktree, "agent-one");
    std::fs::write(worktree.join("NOTES.md"), "unsaved\n").expect("the human's own file");
    world.containerise();

    let plan = world.plan();
    assert_eq!(reclaiming(&plan).len(), 1);

    lock_by_hand(&world, "agent-one");
    let (report, _) = world.act(&plan);

    assert!(report.reclaimed.is_empty());
    assert_eq!(report.withheld_derivatives.len(), 1, "{report:?}");
    assert!(env.is_dir());
}

#[test]
fn the_listing_path_never_costs_a_derivative() {
    // `dl --ls` is a read-only command people run casually, and costing one
    // derivative is a full walk of a site plus an `exclusive_usage` over a
    // 12000-file environment. The clone's verdict is the same either way — a
    // derivative is not a reason a site stands — so the listing asks for none.
    let world = Clone::new();
    let worktree = world.worktree("agent-one");
    installed_env(&worktree, "default");
    commit_the_project(&world, &worktree, "agent-one");
    std::fs::write(worktree.join("NOTES.md"), "unsaved\n").expect("the human's own file");
    world.containerise();

    let runner = ProcessRunner::new();
    let git = Git::new(&runner);
    let picture = ClonePicture::of(&git, &world.clone).expect("git's own listing");
    let asked = weigh_clone(
        &git,
        &world.clone,
        Some(&world.bare),
        &picture,
        Derivatives::Weighed,
        |_| Insistence::NotInsisted,
    );
    let not_asked = weigh_clone(
        &git,
        &world.clone,
        Some(&world.bare),
        &picture,
        Derivatives::NotAsked,
        |_| Insistence::NotInsisted,
    );

    assert_eq!(asked.derivatives.len(), 1, "the prune path asks");
    assert!(
        not_asked.derivatives.is_empty(),
        "the listing path does not"
    );
    assert_eq!(
        asked.standing, not_asked.standing,
        "and the verdict is the same either way: a derivative is never a reason a site \
         stands, so the listing loses nothing by not costing one"
    );
    assert_eq!(asked.going, not_asked.going);
}
