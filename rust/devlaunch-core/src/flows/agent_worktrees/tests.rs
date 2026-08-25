//! What the agent-worktree sweep classifies, at the seam the whole ticket turns
//! on.
//!
//! **Real git, real filesystem, real registrations.** Every fact this module acts
//! on comes out of `git worktree list --porcelain` and out of `git status` run
//! through an admin directory, and a faked spawn answers a clean exit with empty
//! output — which reads as "git named no registrations" and "this worktree is
//! clean", the two answers that delete. So these build a clone with real
//! worktrees in it and then rewrite the registrations to container paths, which is
//! the shape a host sees and the shape none of this can be tested without.

use std::path::{Path, PathBuf};

use devlaunch_runner::ProcessRunner;

use super::*;
use crate::domain::workspace_state::Unsaved;
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

        // The forge stands in for GitHub; the bare cache is what devlaunch fetches
        // into and what the reachability probe asks.
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
    /// which is what makes git call them prunable and what a host actually sees.
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

    /// The sweep, as `--prune` would take it.
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

/// The paths a sweep would remove, in the order it reports them.
fn removing(found: &CloneWorktrees) -> Vec<PathBuf> {
    found.removing().iter().map(|it| it.path.clone()).collect()
}

/// The paths it would leave.
fn keeping(found: &CloneWorktrees) -> Vec<PathBuf> {
    found.keeping().iter().map(|it| it.path.clone()).collect()
}

/// Why it keeps `path`, failing loudly if it is not in the sweep at all.
///
/// Every assertion about a directory *surviving* goes through here rather than
/// through an existence check, because "it is still there" is true of a worktree
/// kept for the right reason and of one no guard ever looked at.
fn kept_because(found: &CloneWorktrees, path: &Path) -> WorktreeKept {
    let mut matched: Vec<&KeptWorktree> = found
        .keeping()
        .iter()
        .filter(|kept| kept.path == path)
        .collect();
    assert_eq!(
        matched.len(),
        1,
        "expected exactly one report line for {}: {:?}",
        path.display(),
        found.keeping()
    );
    matched.remove(0).because.clone()
}

// =======================================================================
// a clone with nothing in it costs nothing
// =======================================================================

#[test]
fn a_clone_with_no_worktrees_directory_is_not_swept_at_all() {
    // The answer for nearly every clone, and the reason this is affordable on the
    // prune path: no `git worktree list`, no disk walk.
    let world = Clone::new();

    assert_eq!(world.sweep(Insistence::NotInsisted), None);
}

// =======================================================================
// the four categories (devlaunch#426)
// =======================================================================

#[test]
fn a_worktree_git_still_holds_is_left_alone() {
    // The registrations resolve, so git calls this worktree live. This is the arm
    // that stops a run inside a container collecting its own worktrees.
    let world = Clone::new();
    let live = world.worktree("agent-live");

    let found = world.plan();

    assert!(removing(&found).is_empty(), "{:?}", removing(&found));
    assert!(matches!(
        kept_because(&found, &live),
        WorktreeKept::StillHeld { .. }
    ));
}

#[test]
fn a_registration_git_calls_prunable_is_reclaimed() {
    // The host's ordinary case: registered from inside the container, so the path
    // git holds resolves to nothing and git itself says the registration can go.
    let world = Clone::new();
    let finished = world.worktree("agent-finished");
    world.containerise();

    let found = world.plan();

    assert_eq!(removing(&found), [finished]);
    assert_eq!(found.removing()[0].seen_as, SeenAs::Prunable);
    assert_eq!(
        found.removing()[0].promotion,
        WorktreePromotion::Unopposed,
        "nothing objected, so nothing was insisted past"
    );
}

#[test]
fn a_directory_git_has_already_forgotten_is_reclaimed() {
    // devlaunch#426's category 1: `git worktree prune` has already dropped the
    // metadata and the directory is the whole of what is left.
    let world = Clone::new();
    let left_behind = world.worktree("agent-forgotten");
    world.containerise();
    run_git(&world.clone, &["worktree", "prune"]);

    let found = world.plan();

    assert_eq!(removing(&found), [left_behind]);
    assert_eq!(found.removing()[0].seen_as, SeenAs::Forgotten);
}

#[test]
fn a_locked_worktree_is_never_removed_without_being_asked_for() {
    // A harness locks a worktree so it is not collected mid-run, so a lock may
    // mean in use right now or may be what a killed session left behind.
    let world = Clone::new();
    let locked = world.worktree("agent-locked");
    run_git(
        &world.clone,
        &["worktree", "lock", &locked.display().to_string()],
    );
    world.containerise();

    let found = world.plan();

    assert!(removing(&found).is_empty(), "{:?}", removing(&found));
    let because = kept_because(&found, &locked);
    let WorktreeKept::Objected(objections) = &because else {
        panic!("expected an objection, got {because:?}");
    };
    assert!(
        objections
            .iter()
            .any(|it| matches!(it, WorktreeObjection::Locked { .. })),
        "{objections:?}"
    );
}

#[test]
fn insisting_carries_a_locked_worktree_past_its_lock() {
    let world = Clone::new();
    let locked = world.worktree("agent-locked");
    run_git(
        &world.clone,
        &["worktree", "lock", &locked.display().to_string()],
    );
    world.containerise();

    let found = world
        .sweep(Insistence::Insisted)
        .expect("a sweep of a clone that has worktrees");

    assert_eq!(removing(&found), [locked]);
    assert_eq!(found.removing()[0].seen_as, SeenAs::Locked);
    // What was insisted past travels with the directory it was insisted past for,
    // so the report can say it on that line.
    assert!(matches!(
        &found.removing()[0].promotion,
        WorktreePromotion::Insisted { .. }
    ));
}

// =======================================================================
// what a worktree holds (the sharpened Ask 3)
// =======================================================================

#[test]
fn an_uncommitted_edit_keeps_a_worktree_that_is_otherwise_collectable() {
    // Reachability says nothing about the working tree. A worktree on a
    // fully-merged branch with unstaged edits in it reads as safe to delete under
    // a commits-only rule, and deleting it loses the edits.
    let world = Clone::new();
    let dirty = world.worktree("agent-dirty");
    std::fs::write(dirty.join("README.md"), "an afternoon of edits\n").expect("an edit");
    world.containerise();

    let found = world.plan();

    assert!(removing(&found).is_empty(), "{:?}", removing(&found));
    assert!(matches!(
        kept_because(&found, &dirty),
        WorktreeKept::Objected(_)
    ));
}

#[test]
fn an_untracked_file_keeps_it_too() {
    // An agent's scratch notes are not less lost for never having been added.
    let world = Clone::new();
    let dirty = world.worktree("agent-notes");
    std::fs::write(dirty.join("notes.md"), "what I was about to do\n").expect("a note");
    world.containerise();

    let found = world.plan();

    assert!(removing(&found).is_empty(), "{:?}", removing(&found));
}

#[test]
fn a_commit_that_was_never_pushed_keeps_the_worktree() {
    let world = Clone::new();
    let ahead = world.worktree("agent-ahead");
    std::fs::write(ahead.join("work.md"), "committed and nowhere else\n").expect("a file");
    commit(&ahead, "ahead");
    world.containerise();

    let found = world.plan();

    assert!(removing(&found).is_empty(), "{:?}", removing(&found));
    assert!(matches!(
        kept_because(&found, &ahead),
        WorktreeKept::Objected(_)
    ));
}

#[test]
fn a_detached_head_that_is_ahead_keeps_the_worktree() {
    // An agent worktree need not be on a branch, and a check keyed on a branch
    // name finds no branch and therefore nothing to protect.
    let world = Clone::new();
    let detached = world.worktree("agent-detached");
    std::fs::write(detached.join("work.md"), "committed on no branch\n").expect("a file");
    commit(&detached, "ahead");
    run_git(&detached, &["checkout", "--detach"]);
    world.containerise();

    let found = world.plan();

    assert!(removing(&found).is_empty(), "{:?}", removing(&found));
    assert!(matches!(
        kept_because(&found, &detached),
        WorktreeKept::Objected(_)
    ));
}

#[test]
fn a_detached_head_on_a_commit_the_cache_has_is_collectable() {
    // Detached is not by itself a reason to keep anything: the question is whether
    // what it points at is anywhere else.
    let world = Clone::new();
    let detached = world.worktree("agent-detached");
    run_git(&detached, &["checkout", "--detach"]);
    world.containerise();

    let found = world.plan();

    assert_eq!(removing(&found), [detached]);
}

// =======================================================================
// the stale-ref trap
// =======================================================================

#[test]
fn work_pushed_after_the_clone_was_cut_is_found_in_the_cache() {
    // The clone's own `refs/remotes/origin/*` is as of clone time and never
    // fetched again, so asking it alone reports pushed-and-merged branches as
    // unpushed -- which keeps every byte forever and makes the flag the only way
    // to reclaim anything. The sibling cache is the thing that gets fetched.
    let world = Clone::new();
    let pushed = world.worktree("agent-pushed");
    std::fs::write(pushed.join("work.md"), "pushed after the clone was cut\n").expect("a file");
    commit(&pushed, "pushed");
    run_git(&pushed, &["push", "origin", "agent-pushed"]);
    world.fetch();
    // The clone's own view of the remote goes back to what it was when the clone
    // was cut. That is the state the module header describes -- a clone that is
    // never fetched into, whose `refs/remotes/origin/*` is as of clone time and
    // can be absent -- reached here in one step instead of by waiting for a
    // container to be rebuilt.
    run_git(
        &world.clone,
        &["update-ref", "-d", "refs/remotes/origin/agent-pushed"],
    );
    world.containerise();

    // The clone alone now cannot see it. This is the trap, asserted so the
    // fixture cannot quietly stop reproducing it.
    let runner = ProcessRunner::new();
    let git = Git::new(&runner);
    let seen_by_the_clone = git
        .unpushed_commits(&world.clone, "refs/heads/agent-pushed")
        .said()
        .expect("the clone answers");
    assert!(
        !seen_by_the_clone.trim().is_empty(),
        "the clone should still think this branch is unpushed"
    );

    let found = world.plan();

    assert_eq!(removing(&found), [pushed]);
}

// =======================================================================
// nesting (55 GB in one clone)
// =======================================================================

#[test]
fn a_worktree_inside_a_kept_worktree_is_reclaimed_on_its_own() {
    // An agent session running inside an agent worktree makes its worktrees under
    // that one. Scanning only the top level is how one clone reached 55 GB.
    let world = Clone::new();
    let outer = world.worktree("agent-outer");
    let inner = world.nested(&outer, "agent-inner");
    // The outer one is dirty for a reason of its own, so it stays and the scan has
    // to descend into it to find the inner one.
    std::fs::write(outer.join("notes.md"), "keep me\n").expect("a note");
    world.containerise();

    let found = world.plan();

    assert_eq!(removing(&found), [inner]);
    assert_eq!(keeping(&found), [outer]);
}

#[test]
fn a_worktree_inside_one_that_is_going_is_not_reported_twice() {
    // The outer directory takes everything inside it, so reporting the inner one
    // as well would count the same bytes twice and offer a directory that will not
    // be there.
    let world = Clone::new();
    let outer = world.worktree("agent-outer");
    world.nested(&outer, "agent-inner");
    world.containerise();

    let found = world.plan();

    assert_eq!(removing(&found), [outer]);
}

// =======================================================================
// what is and is not devlaunch's to delete
// =======================================================================

#[test]
fn a_plain_directory_sitting_in_the_worktrees_place_is_not_touched() {
    // Confirmed by the `.git` gitfile naming an admin directory, not by the path
    // shape: a directory that happens to be here is not devlaunch's to remove.
    let world = Clone::new();
    let intruder = worktrees_dir(&world.clone).join("not-a-worktree");
    std::fs::create_dir_all(&intruder).expect("a plain directory");
    std::fs::write(intruder.join("a-file"), "mine\n").expect("a file");

    let found = world
        .sweep(Insistence::Insisted)
        .expect("a clone with a `.claude/worktrees/` is swept");

    assert!(
        removing(&found).is_empty() && keeping(&found).is_empty(),
        "nothing here is a worktree, so there is nothing to say: {found:?}"
    );
    assert!(intruder.exists());
}

#[test]
fn a_symlink_in_the_worktrees_place_is_stepped_over() {
    // Following one would walk a removal out of the cache directory `--prune` is
    // scoped to, and that scoping is what makes a scratch-cache run harmless.
    let world = Clone::new();
    let outside = world.tmp().join("somewhere-else");
    std::fs::create_dir_all(&outside).expect("a directory outside the cache");
    std::fs::create_dir_all(worktrees_dir(&world.clone)).expect("the worktrees directory");
    let link = worktrees_dir(&world.clone).join("agent-link");
    std::os::unix::fs::symlink(&outside, &link).expect("a symlink");

    let found = world
        .sweep(Insistence::Insisted)
        .expect("a clone with a `.claude/worktrees/` is swept");

    assert!(
        removing(&found).is_empty() && keeping(&found).is_empty(),
        "a symlink is not a candidate: {found:?}"
    );
    assert!(outside.exists());
}

// =======================================================================
// what the dirty check sees under a `.claude/worktrees/` (devlaunch#442, S4)
// =======================================================================

#[test]
fn plain_content_under_a_candidates_worktrees_place_is_work() {
    // The dangerous residue of excluding the place instead of the thing. This
    // directory is not a linked worktree, so the sweep never reports it -- and
    // with `.claude/worktrees/` excluded from the dirty check wholesale, nothing
    // protected it either. It went when its parent did, unreported.
    let world = Clone::new();
    let finished = world.worktree("agent-finished");
    let scratch = worktrees_dir(&finished).join("scratch");
    std::fs::create_dir_all(&scratch).expect("a plain directory");
    std::fs::write(scratch.join("notes.md"), "an afternoon\n").expect("a note");
    world.containerise();

    let found = world.plan();

    assert!(removing(&found).is_empty(), "{:?}", removing(&found));
    assert!(
        matches!(kept_because(&found, &finished), WorktreeKept::Objected(_)),
        "content nothing else accounts for has to object"
    );
}

#[test]
fn a_tracked_file_modified_under_the_worktrees_place_is_still_work() {
    // Contrived -- the harness's directory is normally ignored -- but it is real
    // uncommitted work, and a pathspec that hides a whole path hides this too.
    let world = Clone::new();
    let finished = world.worktree("agent-tracked");
    let tracked = worktrees_dir(&finished).join("keep.md");
    std::fs::create_dir_all(tracked.parent().expect("a parent")).expect("the directory");
    std::fs::write(&tracked, "committed\n").expect("the file");
    commit(&finished, "a tracked file where the sweep looks");
    run_git(&finished, &["push", "origin", "agent-tracked"]);
    world.fetch();
    std::fs::write(&tracked, "edited, and nowhere else\n").expect("the edit");
    world.containerise();

    let found = world.plan();

    assert!(removing(&found).is_empty(), "{:?}", removing(&found));
    assert!(
        matches!(kept_because(&found, &finished), WorktreeKept::Objected(_)),
        "an edit to a file the repository knows about is work"
    );
}

// =======================================================================
// the arm that deletes is the hardest one to reach (devlaunch#442, S1)
// =======================================================================

#[test]
fn a_directory_whose_admin_directory_is_here_is_asked_what_it_holds() {
    // git has let go of the name -- nothing in the listing joins to this
    // directory any more -- but the admin directory it wrote is still here, so
    // there is an index and a HEAD to ask through. A classification that lands on
    // the deleting arm with a probe available must take the probe: neither git
    // dropping a name nor this module's suffix join missing one should cost
    // somebody an afternoon.
    let world = Clone::new();
    let unsaved = world.worktree("agent-unsaved");
    std::fs::write(unsaved.join("notes.md"), "an afternoon\n").expect("a note");
    let gitdir = world
        .clone
        .join(".git")
        .join("worktrees")
        .join("agent-unsaved")
        .join("gitdir");
    std::fs::write(&gitdir, "/workspaces/a-container/elsewhere/.git\n")
        .expect("a registration that no longer looks like a worktrees path");

    let found = world.plan();

    assert!(removing(&found).is_empty(), "{:?}", removing(&found));
    assert!(
        matches!(kept_because(&found, &unsaved), WorktreeKept::Objected(_)),
        "an unjoinable registration is not a licence to delete"
    );
}

#[test]
fn a_gitfile_naming_the_clones_own_admin_directory_is_not_a_worktree() {
    // The tail of a gitfile is file content, and file content is not trusted to
    // be a name: `..` would name the clone's own `.git` and have this module
    // probe, and then remove, something that is not one worktree.
    let world = Clone::new();
    let liar = worktrees_dir(&world.clone).join("agent-liar");
    std::fs::create_dir_all(&liar).expect("a directory");
    std::fs::write(
        liar.join(".git"),
        "gitdir: /workspaces/x/.git/worktrees/..\n",
    )
    .expect("a gitfile naming no worktree");
    std::fs::write(liar.join("a-file"), "mine\n").expect("a file");

    let found = world
        .sweep(Insistence::Insisted)
        .expect("a clone with a `.claude/worktrees/` is swept");

    assert!(
        removing(&found).is_empty() && keeping(&found).is_empty(),
        "nothing here names one worktree, so there is nothing to say: {found:?}"
    );
    assert!(liar.exists());
}

// =======================================================================
// the prune-metadata guard
// =======================================================================

#[test]
fn metadata_is_pruned_when_nothing_was_held_back_for_what_it_holds() {
    let world = Clone::new();
    world.worktree("agent-finished");
    world.containerise();

    let found = world.plan();

    assert!(found.metadata_gate().open());
}

#[test]
fn metadata_is_not_pruned_while_a_worktree_is_kept_for_what_it_holds() {
    // `git worktree prune` is all-or-nothing across a clone and would drop the
    // registration of the dirty one too -- turning it into a forgotten directory,
    // which the next run removes outright. The guard would protect it once and
    // hand it over the second time.
    let world = Clone::new();
    world.worktree("agent-finished");
    let dirty = world.worktree("agent-dirty");
    std::fs::write(dirty.join("notes.md"), "unsaved\n").expect("a note");
    world.containerise();

    let found = world.plan();

    assert_eq!(removing(&found).len(), 1);
    assert!(!found.metadata_gate().open());
}

#[test]
fn a_lock_does_not_hold_the_metadata_prune_back() {
    // git skips locked registrations itself, so a locked worktree keeps its
    // registration through a prune and goes on being protected by it.
    let world = Clone::new();
    world.worktree("agent-finished");
    let locked = world.worktree("agent-locked");
    run_git(
        &world.clone,
        &["worktree", "lock", &locked.display().to_string()],
    );
    world.containerise();

    let found = world.plan();

    assert_eq!(removing(&found).len(), 1);
    assert!(found.metadata_gate().open());
}

// =======================================================================
// registrations with nothing behind them
// =======================================================================

#[test]
fn a_registration_whose_directory_is_gone_is_counted_and_frees_nothing() {
    // Prunable metadata with no host bytes behind it: either a container path
    // that never resolved here or a directory somebody removed by hand.
    let world = Clone::new();
    let removed_by_hand = world.worktree("agent-gone");
    world.worktree("agent-here");
    world.containerise();
    std::fs::remove_dir_all(&removed_by_hand).expect("a directory removed by hand");

    let found = world.plan();

    assert_eq!(found.registrations_with_nothing_here().container_paths(), 1);
    assert_eq!(
        found.registrations_with_nothing_here().deleted(),
        0,
        "the registration named a container path, not a path in this clone"
    );
    assert_eq!(removing(&found).len(), 1, "{:?}", removing(&found));
}

#[test]
fn a_registration_naming_a_path_in_this_clone_is_told_apart_from_a_container_one() {
    // The sharpened spec asks the two apart. Neither has bytes behind it, so the
    // metadata prune is the whole of the work either way -- but a container path
    // is the ordinary shape of every worktree an agent made inside a
    // devcontainer, and a path in this clone with nothing at it is somebody's own
    // removal or a run interrupted between the removal and the prune. One number
    // said neither (devlaunch#442 review, S5).
    let world = Clone::new();
    let deleted = world.worktree("agent-deleted");
    world.worktree("agent-here");
    // No `containerise` for this one: the registration keeps the host path it was
    // made with, and the directory it names is gone.
    std::fs::remove_dir_all(&deleted).expect("a directory removed by hand");

    let found = world.plan();

    let nothing_here = found.registrations_with_nothing_here();
    assert_eq!(nothing_here.deleted(), 1);
    assert_eq!(nothing_here.container_paths(), 0);
}

// =======================================================================
// the porcelain parse
// =======================================================================

#[test]
fn the_clones_own_entry_and_worktrees_elsewhere_are_not_candidates() {
    // Only registrations under a `.claude/worktrees/` have a directory here to be
    // joined to; the clone's own entry is not one of them.
    let parsed = registrations(
        "worktree /cache/repos/o/r/ws-one\n\
         HEAD 1111111111111111111111111111111111111111\n\
         branch refs/heads/main\n\
         \n\
         worktree /somewhere/else/entirely\n\
         HEAD 2222222222222222222222222222222222222222\n\
         branch refs/heads/other\n\
         \n\
         worktree /workspaces/ws/.claude/worktrees/agent-a\n\
         HEAD 3333333333333333333333333333333333333333\n\
         branch refs/heads/task\n\
         prunable gitdir file points to non-existent location\n",
    );

    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].inside, ".claude/worktrees/agent-a");
    assert!(parsed[0].prunable);
}

#[test]
fn a_nested_registration_keeps_its_whole_place_inside_the_clone() {
    // The join key has to carry the nesting, or a nested worktree and a top-level
    // one of the same leaf name would be joined to each other.
    let parsed = registrations(
        "worktree /workspaces/ws/.claude/worktrees/outer/.claude/worktrees/inner\n\
         HEAD 4444444444444444444444444444444444444444\n\
         detached\n\
         locked\n",
    );

    assert_eq!(parsed.len(), 1);
    assert_eq!(
        parsed[0].inside,
        ".claude/worktrees/outer/.claude/worktrees/inner"
    );
    assert!(matches!(parsed[0].head, WorktreeHead::Detached { .. }));
    assert_eq!(parsed[0].locked, Some(Lock { reason: None }));
}

#[test]
fn a_lock_reason_is_carried_and_an_absent_one_is_absent() {
    let parsed = registrations(
        "worktree /workspaces/ws/.claude/worktrees/agent-a\n\
         HEAD 5555555555555555555555555555555555555555\n\
         branch refs/heads/task\n\
         locked an agent is working in here\n",
    );

    assert_eq!(
        parsed[0].locked,
        Some(Lock {
            reason: Some("an agent is working in here".to_owned())
        })
    );
}

// =======================================================================
// the decision is total
// =======================================================================

#[test]
fn a_worktree_git_holds_is_never_promoted_by_insisting() {
    // `--force-worktrees` is not a general override: git saying a worktree is
    // live is not a refusal for a person to insist past.
    let held = WorktreeStatus::Held {
        head: WorktreeHead::Branch {
            reference: "refs/heads/task".to_owned(),
            commit: "6666666666666666666666666666666666666666".to_owned(),
        },
    };

    assert!(matches!(
        decide(held, Insistence::Insisted),
        WorktreeDecision::Keep(WorktreeKept::StillHeld { .. })
    ));
}

#[test]
fn a_lock_and_what_it_holds_are_both_reported() {
    // A locked worktree that is also dirty has two things wrong with it, and a
    // report naming one would be telling half the truth.
    let status = WorktreeStatus::Locked {
        lock: Lock { reason: None },
        head: WorktreeHead::Branch {
            reference: "refs/heads/task".to_owned(),
            commit: "7777777777777777777777777777777777777777".to_owned(),
        },
        holds: Unsaved::WouldLose(
            Losses::of([Loss::Uncommitted(
                NonEmpty::of(["?? notes.md".to_owned()]).expect("one line"),
            )])
            .expect("one loss"),
        ),
        usage: DiskUsage::measured(4096),
    };

    let WorktreeDecision::Keep(WorktreeKept::Objected(objections)) =
        decide(status, Insistence::NotInsisted)
    else {
        panic!("expected two objections");
    };

    assert_eq!(objections.len(), 2);
}
