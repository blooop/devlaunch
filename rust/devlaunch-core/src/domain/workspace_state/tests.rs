//! What a clone holds, pinned against real git.
//!
//! These tests use real repositories with a local bare as their remote — a local
//! path is a real git remote, so push, fetch and the remote-tracking refs all
//! behave exactly as they do over ssh, with no network. Faking git here would only
//! prove this file agrees with itself, and the two bugs these tests exist for (an
//! argument order that reported every clone as safe to delete, and git's discovery
//! walking up into an ancestor repository) were both invisible to everything
//! except a real git.
//!
//! Ported from `test/test_workspace_state.py`'s five module-level classes:
//! `TestWhatACloneHolds`, `TestWhenGitCannotBeAsked`,
//! `TestGitIsPinnedToItsWorkTreeToo`, `TestADirectoryThatCannotBeLookedAt`,
//! `TestTheAnswersAreTotal` and `TestNamingWhatIsUnsaved`. Its remaining classes
//! (`TestTheJsonListing`, `TestReportingWhatAWorkspaceCostsOnDisk`,
//! `TestTheDeleteGuard`, `TestForcedRemoveIsEnsureAbsent`) are about the two
//! surfaces above this module and belong to the listing (M5) and lifecycle (M6)
//! flows; they are re-expressed at the binary boundary there, not here.
//!
//! One Python test has no analogue and needs none:
//! `test_an_arm_nobody_handles_is_refused_rather_than_rendered` fed
//! `unsaved_as_json` a string, which Rust's type system refuses to compile. Its
//! neighbour `test_would_lose_cannot_be_built_with_nothing_to_say` becomes
//! [`a_would_lose_with_nothing_to_say_has_no_representation`], which asserts the
//! absence rather than a raise.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;
use crate::runner::ProcessRunner;
use crate::testing::ScriptedRunner;
use devlaunch_test_support::Response;

// --------------------------------------------------------------- fixtures

/// Run real git, failing the test with git's own words if it refuses.
fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git is installed");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Commit everything in *work*, with an identity this test owns.
fn commit(work: &Path, message: &str) {
    git(work, &["add", "-A"]);
    git(
        work,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            message,
        ],
    );
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("a parent directory");
    }
    std::fs::write(path, text).expect("written");
}

/// A bare repository standing in for GitHub, with one commit on `main`.
fn remote_at(root: &Path) -> PathBuf {
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    git(root, &["init", "-q", "-b", "main", "seed"]);
    write(&seed.join("README.md"), "seed\n");
    commit(&seed, "seed");
    git(
        root,
        &[
            "clone",
            "-q",
            "--bare",
            seed.to_str().expect("utf-8"),
            origin.to_str().expect("utf-8"),
        ],
    );
    origin
}

/// A workspace clone on a pushed branch, as `dl` would leave one.
fn clone_on_a_pushed_branch(remote: &Path, work: &Path) -> PathBuf {
    let parent = work.parent().expect("a parent");
    std::fs::create_dir_all(parent).expect("a parent directory");
    git(
        parent,
        &[
            "clone",
            "-q",
            remote.to_str().expect("utf-8"),
            work.to_str().expect("utf-8"),
        ],
    );
    git(work, &["checkout", "-q", "-b", "feature"]);
    write(&work.join("feature.txt"), "work\n");
    commit(work, "feature");
    git(work, &["push", "-q", "-u", "origin", "feature"]);
    work.to_path_buf()
}

/// A temp directory holding a remote and a clone of it on a pushed branch.
struct Fixture {
    root: tempfile::TempDir,
    remote: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("a temp dir");
        let remote = remote_at(root.path());
        Self { root, remote }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    /// The clone at the path `dl` would put one, on a pushed `feature`.
    fn clone(&self) -> PathBuf {
        clone_on_a_pushed_branch(&self.remote, &self.path("ws"))
    }

    /// A repository that is clean, fully pushed, and ignores `.cache/`.
    ///
    /// The dotfiles-in-`$HOME` case, and the reason devlaunch#171 only shows
    /// itself on a tidy host: git's discovery walking up out of a broken clone
    /// lands here, and a repository with nothing to report answers "nothing to
    /// report".
    fn ancestor(&self) -> PathBuf {
        let host = self.path("host");
        std::fs::create_dir_all(&host).expect("a directory");
        git(&host, &["init", "-q", "-b", "main", "."]);
        write(&host.join(".gitignore"), ".cache/\n");
        commit(&host, "seed");
        let origin = self.path("host-origin.git");
        git(
            self.root.path(),
            &["init", "-q", "--bare", origin.to_str().expect("utf-8")],
        );
        git(
            &host,
            &["remote", "add", "origin", origin.to_str().expect("utf-8")],
        );
        git(&host, &["push", "-q", "-u", "origin", "main"]);
        // The premise, asserted rather than assumed: if this repository were
        // dirty or had an unpushed commit the guard would fire for the wrong
        // reason and the bug these tests are about would be invisible.
        assert_eq!(git(&host, &["status", "--porcelain"]), "");
        assert_eq!(
            git(&host, &["log", "--oneline", "main", "--not", "--remotes"]),
            ""
        );
        host
    }

    /// A clone whose `.git` is unusable, holding scratch work, nested in a
    /// repository. What an interrupted delete, a truncated write or a half-copied
    /// cache leaves behind: the directory is there and holds a file that exists
    /// nowhere else.
    fn broken_clone_under_ancestor(&self) -> PathBuf {
        let clone = self.ancestor().join(".cache/devlaunch/ws");
        write(&clone.join(".git/HEAD"), "garbage\n");
        write(&clone.join("scratch.md"), "half a plan\n");
        clone
    }
}

// ------------------------------------------------------------- the subject

/// [`read_clone`] against real git.
fn read(clone: &Path) -> CloneState {
    let runner = ProcessRunner::new();
    read_clone(&Git::new(&runner), clone)
}

/// [`holds_unsaved_work`] against real git.
fn held(clone: &Path) -> Unsaved {
    let runner = ProcessRunner::new();
    holds_unsaved_work(&Git::new(&runner), clone)
}

/// The description of a `WouldLose`, or a failure naming the arm that came back.
fn would_lose(unsaved: &Unsaved) -> String {
    match unsaved {
        Unsaved::WouldLose(losses) => losses.describe(),
        other => panic!("expected a WouldLose: {other:?}"),
    }
}

/// The description of a `CouldNotTell`, or a failure naming the arm that came back.
fn could_not_tell(unsaved: &Unsaved) -> String {
    match unsaved {
        Unsaved::CouldNotTell(cause) => cause.describe(),
        other => panic!("expected a CouldNotTell: {other:?}"),
    }
}

// ------------------------------------------------------ what a clone holds

#[test]
fn a_pushed_branch_with_a_clean_tree_holds_nothing_unsaved() {
    let fixture = Fixture::new();
    let clone = fixture.clone();

    assert_eq!(
        read(&clone),
        CloneState {
            branch: Some("feature".to_owned()),
            unsaved: Unsaved::NothingToLose,
        }
    );
}

#[test]
fn an_unpushed_commit_is_unsaved() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("more.txt"), "more\n");
    commit(&clone, "more");

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn several_unpushed_commits_are_counted() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    for n in 0..2 {
        write(&clone.join(format!("more{n}.txt")), "more\n");
        commit(&clone, &format!("more {n}"));
    }

    assert_eq!(would_lose(&held(&clone)), "2 unpushed commit(s)");
}

#[test]
fn uncommitted_changes_are_unsaved() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("feature.txt"), "edited\n");

    assert_eq!(
        would_lose(&held(&clone)),
        "1 uncommitted change(s) (feature.txt)"
    );
}

#[test]
fn untracked_files_are_unsaved_too() {
    // An agent's scratch notes are not less lost for never having been added.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("notes.md"), "half a plan\n");

    assert_eq!(
        would_lose(&held(&clone)),
        "1 uncommitted change(s) (notes.md)"
    );
}

#[test]
fn both_kinds_of_loss_are_reported_together() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("more.txt"), "more\n");
    commit(&clone, "more");
    write(&clone.join("dirty.txt"), "dirty\n");

    assert_eq!(
        would_lose(&held(&clone)),
        "1 uncommitted change(s) (dirty.txt) and 1 unpushed commit(s)",
        "the dirty tree first, joined with \" and \""
    );
}

#[test]
fn a_branch_whose_commits_are_on_the_remote_under_another_name_is_saved() {
    // Pushed under a second name: the commits exist elsewhere, so nothing would
    // be lost. Asking about *any* remote ref rather than this branch's upstream
    // is what gets this right.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["push", "-q", "origin", "feature:review/feature"]);
    git(&clone, &["branch", "-m", "feature", "renamed"]);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_commit_on_a_branch_that_is_not_checked_out_is_unsaved() {
    // The whole of #471, and it is a live data-loss path in shipped code: commit
    // on `wip`, switch back, and the clone reads as safe to delete while holding
    // the only copy of that commit. Asking about the checked-out branch alone is
    // what got this wrong — the question is what the *clone* holds, and a clone
    // holds every ref in it.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["checkout", "-q", "-b", "wip"]);
    write(&clone.join("wip.txt"), "an hour of work\n");
    commit(&clone, "wip");
    git(&clone, &["checkout", "-q", "feature"]);

    // The premise, asserted rather than assumed: the checked-out branch really is
    // clean and fully pushed, so the only thing left to find is on `wip`.
    assert_eq!(git(&clone, &["status", "--porcelain"]), "");
    assert_eq!(read(&clone).branch.as_deref(), Some("feature"));

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_stashed_change_is_unsaved_too() {
    // `refs/stash` is a ref in the clone, so reaching every ref reaches it. Not
    // incidental: a stash is work that exists nowhere else and the clone is the
    // only place it lives, so it belongs on the same side of the answer as an
    // unpushed commit. It is written to the clone's own `refs/stash` even from
    // inside a linked worktree, so there is one stash per clone to find.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("stashed.txt"), "half a plan\n");
    git(&clone, &["add", "-A"]);
    git(
        &clone,
        &["-c", "user.email=t@t", "-c", "user.name=t", "stash", "-q"],
    );

    assert_eq!(git(&clone, &["status", "--porcelain"]), "");
    // Two commits: git writes a stash as a commit plus its index parent, and both
    // are counted, because a count of commits is what this answer is.
    assert_eq!(would_lose(&held(&clone)), "2 unpushed commit(s)");
}

#[test]
fn a_clone_that_is_not_there_holds_nothing() {
    // A half-finished delete, or a directory removed by hand. There is no work in
    // it to lose, and nothing here may crash on it.
    let dir = tempfile::tempdir().expect("a temp dir");

    assert_eq!(
        read(&dir.path().join("absent")),
        CloneState {
            branch: None,
            unsaved: Unsaved::NothingToLose,
        }
    );
}

#[test]
fn a_clone_that_is_not_there_is_not_asked_about_either() {
    // git is never spawned for a directory that is not on disk: the stat is the
    // whole answer, and a `dl --ls --json` over many workspaces pays nothing for
    // the ones whose clones are gone.
    let dir = tempfile::tempdir().expect("a temp dir");
    let fake = ScriptedRunner::new();

    let state = read_clone(&Git::new(&fake), &dir.path().join("absent"));

    assert_eq!(state.unsaved, Unsaved::NothingToLose);
    assert_eq!(fake.call_count(), 0);
}

#[test]
fn a_path_with_a_file_at_it_rather_than_a_clone_also_holds_nothing() {
    // Neither a clone nor a directory: the same answer as nothing at all.
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("ws");
    write(&file, "not a clone\n");

    assert_eq!(
        read(&file),
        CloneState {
            branch: None,
            unsaved: Unsaved::NothingToLose,
        }
    );
}

#[test]
fn a_clone_under_something_that_is_not_a_directory_holds_nothing() {
    // ENOTDIR rather than ENOENT: a parent component is a file. Still no clone at
    // that path, so still nothing in it to lose.
    let dir = tempfile::tempdir().expect("a temp dir");
    let file = dir.path().join("file");
    write(&file, "not a directory\n");

    assert_eq!(
        read(&file.join("ws")),
        CloneState {
            branch: None,
            unsaved: Unsaved::NothingToLose,
        }
    );
}

#[test]
fn a_clone_nested_in_a_repository_still_answers_about_itself() {
    // The other half of devlaunch#171: pinning the clone down must not cost the
    // ordinary answer. dl's cache lives under `$XDG_CACHE_HOME`, which on a great
    // many machines is inside a dotfiles repository.
    let fixture = Fixture::new();
    let ancestor = fixture.ancestor();
    let work = ancestor.join(".cache/devlaunch/ws");
    std::fs::create_dir_all(work.parent().expect("a parent")).expect("a directory");
    git(
        &ancestor,
        &[
            "clone",
            "-q",
            fixture.remote.to_str().expect("utf-8"),
            work.to_str().expect("utf-8"),
        ],
    );
    git(&work, &["checkout", "-q", "-b", "feature"]);
    write(&work.join("mine.txt"), "mine\n");

    let state = read(&work);

    assert_eq!(state.branch.as_deref(), Some("feature"));
    assert_eq!(
        would_lose(&state.unsaved),
        "1 uncommitted change(s) (mine.txt)"
    );
}

#[test]
fn a_linked_worktree_answers_normally_too() {
    // Its `.git` is a gitfile, which git follows, so pinning the clone down costs
    // nothing here either — and a devcontainer that runs `git worktree add`
    // inside a workspace is an ordinary thing to do.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    let linked = fixture.path("linked");
    git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().expect("utf-8"),
            "-b",
            "side",
        ],
    );
    write(&linked.join("notes.md"), "half a plan\n");

    let state = read(&linked);

    assert_eq!(state.branch.as_deref(), Some("side"));
    assert_eq!(
        would_lose(&state.unsaved),
        "1 uncommitted change(s) (notes.md)"
    );
}

#[test]
fn a_clone_moved_off_its_branch_reports_the_branch_it_is_on() {
    // An agent moved off the branch the workspace was made for. Both are facts;
    // neither is made to stand for the other, which is why the listing prints
    // this beside the recorded branch.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["checkout", "-q", "-b", "sidequest"]);

    assert_eq!(read(&clone).branch.as_deref(), Some("sidequest"));
}

#[test]
fn a_repository_with_no_commits_yet_is_readable_and_names_no_branch() {
    // `git status` succeeds on it — that is why status is the repository probe —
    // and `rev-parse --abbrev-ref HEAD` refuses, so there is no branch. `git log`
    // is still asked, and answers 0 with no output on a clone with no refs, which
    // is why widening the probe needed no gate to protect this case. The work in
    // it is still work.
    let fixture = Fixture::new();
    let empty = fixture.path("empty");
    std::fs::create_dir_all(&empty).expect("a directory");
    git(&empty, &["init", "-q", "-b", "main", "."]);
    write(&empty.join("notes.md"), "half a plan\n");

    let state = read(&empty);

    assert_eq!(state.branch, None, "an unborn HEAD names no branch");
    assert_eq!(
        would_lose(&state.unsaved),
        "1 uncommitted change(s) (notes.md)"
    );
}

#[test]
fn an_unborn_head_does_not_hide_the_commits_on_the_other_branches() {
    // The second data-loss path the same blindness had, and the one the branch
    // gate rather than the branch *name* caused: `git checkout --orphan` leaves
    // HEAD naming no commit, so `rev-parse` refuses, so the old probe skipped the
    // unpushed question altogether — and reported `NothingToLose` for a clone
    // holding an unpushed commit on the branch it had just stepped off.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("more.txt"), "more\n");
    commit(&clone, "more");
    git(&clone, &["checkout", "-q", "--orphan", "fresh"]);
    git(&clone, &["rm", "-r", "-q", "--cached", "."]);

    let state = read(&clone);

    assert_eq!(state.branch, None, "an unborn HEAD names no branch");
    assert!(
        would_lose(&state.unsaved).contains("1 unpushed commit(s)"),
        "the commit on `feature` is still the only copy: {:?}",
        state.unsaved
    );
}

// -------------------------------------------------- when git cannot be asked

#[test]
fn an_unusable_git_is_could_not_tell_not_nothing_to_lose() {
    let fixture = Fixture::new();
    let clone = fixture.broken_clone_under_ancestor();

    let reason = could_not_tell(&held(&clone));

    // And it must say so of *this* directory, not of the one git wandered into:
    // the reason is what a person reads before deciding to force.
    assert!(
        reason.contains(&clone.display().to_string()),
        "the reason names the clone: {reason:?}"
    );
}

#[test]
fn it_does_not_borrow_the_ancestor_s_branch_either() {
    // The shipped bug reported `branch='main'` — the ancestor's checked-out
    // branch — and `dl --ls --json` printed it as this clone's `checkedOut`.
    let fixture = Fixture::new();
    let clone = fixture.broken_clone_under_ancestor();

    assert_eq!(read(&clone).branch, None);
}

#[test]
fn an_unusable_git_with_no_ancestor_at_all_is_also_could_not_tell() {
    // The same directory with nothing above it to walk into. git refuses either
    // way now, so the answer does not depend on what the machine happens to have
    // in a parent directory.
    let dir = tempfile::tempdir().expect("a temp dir");
    let clone = dir.path().join("ws");
    write(&clone.join(".git/HEAD"), "garbage\n");
    write(&clone.join("scratch.md"), "half a plan\n");

    could_not_tell(&held(&clone));
}

#[test]
fn every_shape_a_broken_clone_takes_is_a_refusal() {
    // The five shapes `_git`'s docstring enumerates, each built here rather than
    // asserted from the prose. Four of the five answered about the *ancestor*
    // under a plain working directory; the truncated gitfile did not, because git
    // treats an unreadable gitfile as a hard error rather than continuing
    // discovery upward — it is here because it is a shape a broken clone takes,
    // not because it was ever part of the bug.
    let fixture = Fixture::new();
    let ancestor = fixture.ancestor();
    let nest = |name: &str| ancestor.join(".cache/devlaunch").join(name);

    let garbage = nest("garbage");
    write(&garbage.join(".git/HEAD"), "garbage\n");

    let empty_git = nest("empty-git");
    std::fs::create_dir_all(empty_git.join(".git")).expect("a directory");

    let head_only = nest("head-only");
    write(&head_only.join(".git/HEAD"), "ref: refs/heads/main\n");

    let truncated_gitfile = nest("truncated-gitfile");
    std::fs::create_dir_all(&truncated_gitfile).expect("a directory");
    write(&truncated_gitfile.join(".git"), "gitdir: ");

    let objects_gone = nest("objects-gone");
    clone_on_a_pushed_branch(&fixture.remote, &objects_gone);
    std::fs::remove_dir_all(objects_gone.join(".git/objects")).expect("removed");

    for shape in [
        &garbage,
        &empty_git,
        &head_only,
        &truncated_gitfile,
        &objects_gone,
    ] {
        write(&shape.join("scratch.md"), "half a plan\n");
        let state = read(shape);
        let reason = could_not_tell(&state.unsaved);
        assert!(
            reason.contains(&shape.display().to_string()),
            "{}: {reason:?}",
            shape.display()
        );
        assert_eq!(state.branch, None, "{}", shape.display());
    }
}

#[test]
fn a_directory_that_is_not_a_repository_cannot_be_judged() {
    // A present directory that is not a repository is not an empty one. This used
    // to answer "nothing", documented as "a directory that is not there, or is
    // not a repository, holds nothing". Half of that is true and stays true; the
    // other half was the bug: a directory that *is* there and is not a repository
    // holds whatever files are in it, and git, having no repository to read,
    // cannot say whether they exist anywhere else. That is a refusal, and a
    // refusal is not permission.
    let dir = tempfile::tempdir().expect("a temp dir");
    let plain = dir.path().join("plain");
    write(&plain.join("file.txt"), "not a repo\n");

    let state = read(&plain);

    assert_eq!(state.branch, None);
    could_not_tell(&state.unsaved);
}

#[test]
fn a_half_removed_clone_is_could_not_tell() {
    // An interrupted delete: a real clone with its object store gone. Named
    // separately from the garbage-`.git` case because it is the one the issue
    // describes reaching in the wild, and because it is the shape where the
    // *files* are still all there to lose.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("scratch.md"), "half a plan\n");
    std::fs::remove_dir_all(clone.join(".git/objects")).expect("removed");

    could_not_tell(&held(&clone));
    assert!(clone.join("scratch.md").exists(), "the work is still there");
}

#[test]
fn a_readable_repo_whose_remote_refs_are_broken_is_could_not_tell() {
    // The second refusal, which the first would otherwise hide. `git status`
    // succeeds — it never looks at remote-tracking refs — so the repository probe
    // passes and the clone reads as clean right up until
    // `git log … --not --remotes` is asked, which refuses on a ref pointing at an
    // object that is not there. Answering "nothing to lose" on the strength of
    // the half that worked is the same bug in a narrower place: the unpushed
    // commits were never counted.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(
        &clone.join(".git/refs/remotes/origin/bogus"),
        "0123456789abcdef0123456789abcdef01234567\n",
    );

    let state = read(&clone);

    let reason = could_not_tell(&state.unsaved);
    assert!(
        reason.contains("unpushed commits") && reason.contains(&clone.display().to_string()),
        "the reason says which question refused, and about which clone: {reason:?}"
    );
    assert!(
        !reason.contains("feature"),
        "and names no branch, because the question named none: {reason:?}"
    );
    assert_eq!(
        state.branch.as_deref(),
        Some("feature"),
        "status answered, so the branch is known; the two facts are independent"
    );
}

#[test]
fn git_that_cannot_be_run_at_all_is_could_not_tell() {
    // The process-level refusal, which never reaches a return code to inspect.
    // Scripted rather than arranged by emptying PATH, which would be a
    // process-wide change in a threaded test binary.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    let fake = ScriptedRunner::new().with_script(["git"], Response::ProgramNotFound);

    let state = read_clone(&Git::new(&fake), &clone);

    let reason = could_not_tell(&state.unsaved);
    assert!(
        reason.contains(&clone.display().to_string()) && reason.contains("PATH"),
        "the reason names the clone and what stopped it: {reason:?}"
    );
}

#[test]
fn a_refused_status_is_never_read_as_a_clean_tree() {
    // The sentinel bug one layer down: `""` and a refusal are both falsey in
    // Python, so `if status:` read a refused `git status` as a clean tree. Here
    // the empty answer and the refusal are different arms, and only one of them
    // is permission.
    let clean = ScriptedRunner::new().with_script(["git"], Response::stdout(""));
    let refused = ScriptedRunner::new().with_script(["git"], Response::failed(128, "fatal: nope"));
    let dir = tempfile::tempdir().expect("a temp dir");

    assert_eq!(
        holds_unsaved_work(&Git::new(&clean), dir.path()),
        Unsaved::NothingToLose
    );
    could_not_tell(&holds_unsaved_work(&Git::new(&refused), dir.path()));
}

// ------------------------------------------- git is pinned to its work tree

/// Point *clone*'s work tree at another directory, optionally mirroring HEAD.
fn work_tree_pointed_elsewhere(clone: &Path, elsewhere: &Path, mirror_head: bool) {
    std::fs::create_dir_all(elsewhere).expect("a directory");
    if mirror_head {
        // A checkout of the same commit, built from what git says is tracked
        // rather than from a list written here, so it mirrors HEAD by
        // construction and stays mirrored if the fixture gains a file.
        for tracked in git(clone, &["ls-files"]).lines() {
            std::fs::copy(clone.join(tracked), elsewhere.join(tracked)).expect("copied");
        }
    }
    git(
        clone,
        &[
            "config",
            "core.worktree",
            elsewhere.to_str().expect("utf-8"),
        ],
    );
}

#[test]
fn a_clone_whose_other_work_tree_mirrors_head_still_reports_its_own_work() {
    // The fail-open, and the assertion that matters is the arm. Without
    // `--work-tree` git looks at the mirror, finds it identical to the index, and
    // says nothing at rc 0: "nothing to lose" on the clone below, which holds a
    // file that exists nowhere else. That is devlaunch#171's failure class
    // reached by a second route.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    work_tree_pointed_elsewhere(&clone, &fixture.path("elsewhere"), true);
    write(&clone.join("an-hour-of-work.md"), "half a plan\n");

    assert_eq!(
        would_lose(&held(&clone)),
        "1 uncommitted change(s) (an-hour-of-work.md)"
    );
}

#[test]
fn a_clean_clone_is_not_made_dirty_by_the_other_work_tree_s_absences() {
    // The other outcome, pinned so it cannot be mistaken for the one above: an
    // empty other work tree makes `--git-dir` alone report HEAD's files as
    // deleted, so this clone — which is clean — would be refused for work it does
    // not hold. Wrong, and a refusal rather than a fail-open, which is why the
    // test above is the one that carries the safety property.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    work_tree_pointed_elsewhere(&clone, &fixture.path("elsewhere"), false);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_clone_marked_bare_in_its_own_config_still_answers_about_its_files() {
    // `core.bare = true` is the neighbouring shape. With `--git-dir` alone it is
    // `fatal: this operation must be run in a work tree` — a refusal, and
    // therefore already safe — but with `--work-tree` given, git answers about
    // the real clone, which is the better answer for the same flags.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["config", "core.bare", "true"]);
    write(&clone.join("an-hour-of-work.md"), "half a plan\n");

    assert_eq!(
        would_lose(&held(&clone)),
        "1 uncommitted change(s) (an-hour-of-work.md)"
    );
}

// ------------------------------------- a directory that cannot be looked at

#[test]
fn a_clone_behind_a_closed_door_is_could_not_tell() {
    // Not knowing whether the clone is there is not knowing what it holds. This
    // was `Path.is_dir()` in Python, which gave two different wrong answers
    // depending on which interpreter ran it: a raise up to 3.13 (so `rm` failed
    // closed by crashing and `--ls --json` became a traceback for the whole
    // listing because of one workspace), and `False` on 3.14, which read as "not
    // there, so nothing to lose". One expression, two sentinels.
    // SAFETY: `geteuid` takes nothing, returns a uid and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        // Root is refused by nothing, so the closed door would open. Skipped
        // rather than inverted: what this asserts is true of every other user,
        // and CI runs as one.
        return;
    }
    let dir = tempfile::tempdir().expect("a temp dir");
    let parent = dir.path().join("locked");
    let clone = parent.join("ws");
    write(&clone.join("an-hour-of-work.md"), "half a plan\n");
    shut(&parent, 0o000);

    let state = read(&clone);

    shut(&parent, 0o700);
    let reason = could_not_tell(&state.unsaved);
    assert!(
        reason.contains(&clone.display().to_string()),
        "the reason names the clone: {reason:?}"
    );
}

fn shut(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

#[test]
fn a_recorded_path_that_is_not_a_path_at_all_is_could_not_tell() {
    // A NUL byte in the path is rejected before the syscall, and a hand-edited or
    // truncated `metadata.json` is how one gets into a record. Unhandled it takes
    // the whole of `dl --ls --json` down for one bad row, which is the harm this
    // guard was written to stop.
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = PathBuf::from(format!("{}\0truncated", dir.path().join("ws").display()));

    could_not_tell(&read(&path).unsaved);
}

// ---------------------------------------------------- the answers are total

#[test]
fn each_arm_renders_as_one_key_that_names_it() {
    // The exact wire format: wf parses this.
    assert_eq!(
        Unsaved::NothingToLose.as_json().to_string(),
        r#"{"nothingToLose":true}"#
    );
    assert_eq!(
        Unsaved::WouldLose(Losses::one(Loss::Unpushed(NonEmpty::one(
            "abc123 more".to_owned()
        ))))
        .as_json()
        .to_string(),
        r#"{"wouldLose":"1 unpushed commit(s)"}"#
    );
    assert_eq!(
        Unsaved::CouldNotTell(CouldNotTell::GitCouldNotRead {
            clone: PathBuf::from("/c"),
            reason: "git said no".to_owned(),
        })
        .as_json()
        .to_string(),
        r#"{"couldNotTell":"git could not read /c: git said no"}"#
    );
}

#[test]
fn a_would_lose_with_nothing_to_say_has_no_representation() {
    // Python raised from `WouldLose.__post_init__` because a description was a
    // string a caller could get wrong: "workspace holds ." reads as a bug in dl
    // rather than as a reason to stop. Here the arm carries losses that cannot be
    // empty, so the empty case *is* the other arm — asserted through the one
    // constructor that could produce it.
    assert_eq!(Losses::of(Vec::<Loss>::new()), None);
    assert_eq!(NonEmpty::<String>::of(Vec::new()), None);

    let nothing_changed = ScriptedRunner::new().with_script(["git"], Response::stdout(""));
    let dir = tempfile::tempdir().expect("a temp dir");
    assert_eq!(
        holds_unsaved_work(&Git::new(&nothing_changed), dir.path()),
        Unsaved::NothingToLose
    );
}

#[test]
fn every_description_says_something() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("dirty.txt"), "dirty\n");
    commit(&clone, "more");
    write(&clone.join("again.txt"), "again\n");

    for unsaved in [held(&clone), Unsaved::NothingToLose] {
        match unsaved {
            Unsaved::WouldLose(losses) => assert!(!losses.describe().is_empty()),
            Unsaved::CouldNotTell(cause) => assert!(!cause.describe().is_empty()),
            Unsaved::NothingToLose => {}
        }
    }
}

#[test]
fn two_of_the_three_answers_refuse_a_delete() {
    // devlaunch#171 in one assertion: "could not tell" refuses exactly as "would
    // lose" does, and only the first arm is permission. Written as the match a
    // guard writes rather than against a `may_delete()` helper, which this module
    // deliberately does not offer — see the note above `Unsaved`'s impl.
    for (unsaved, may_delete) in [
        (Unsaved::NothingToLose, true),
        (
            Unsaved::WouldLose(Losses::one(Loss::Unpushed(NonEmpty::one("abc".to_owned())))),
            false,
        ),
        (
            Unsaved::CouldNotTell(CouldNotTell::GitCouldNotRead {
                clone: PathBuf::from("/c"),
                reason: "git said no".to_owned(),
            }),
            false,
        ),
    ] {
        let permitted = match &unsaved {
            Unsaved::NothingToLose => true,
            Unsaved::WouldLose(_) | Unsaved::CouldNotTell(_) => false,
        };
        assert_eq!(permitted, may_delete, "{unsaved:?}");
    }
}

// ------------------------------------------------- naming what is unsaved

#[test]
fn the_changed_paths_are_named() {
    // The case this exists for is real and permanent: this repo's own
    // devcontainer runs `pixi install` in its postCreateCommand, which leaves the
    // tracked `pixi.lock` modified in *every* workspace it builds. Reported as "1
    // uncommitted change(s)", an untouched clone is indistinguishable from an
    // hour of someone's unsaved work, and a cleanup tool that believes the count
    // never cleans anything.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(
        &clone.join("pixi.lock"),
        "churned by the container's own build\n",
    );

    assert_eq!(
        would_lose(&held(&clone)),
        "1 uncommitted change(s) (pixi.lock)"
    );
}

#[test]
fn a_modified_tracked_file_keeps_its_first_letter() {
    // The regression that took real use to find. `git status --porcelain` writes
    // a *modified* tracked file as " M path" — leading space — and a full strip of
    // git's output ate it, so the path was reported one character short
    // ("ixi.lock"). Untracked files start "??" and were unharmed, which is why
    // every test passed while the feature was printing nonsense. Asserted on the
    // exact rendering, not on a substring.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(
        &clone.join("feature.txt"),
        "edited by the container's build\n",
    );

    assert_eq!(
        held(&clone),
        Unsaved::WouldLose(Losses::one(Loss::Uncommitted(NonEmpty::one(
            " M feature.txt".to_owned()
        )))),
        "the porcelain line is kept whole, status column and all"
    );
    assert_eq!(
        would_lose(&held(&clone)),
        "1 uncommitted change(s) (feature.txt)"
    );
}

#[test]
fn a_long_list_is_cut_short_rather_than_dumped() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    for n in 0..6 {
        write(&clone.join(format!("file{n}.txt")), "x\n");
    }

    let description = would_lose(&held(&clone));

    assert!(
        description.starts_with("6 uncommitted change(s) ("),
        "{description:?}"
    );
    // Three names and an ellipsis: enough to recognise, not a wall of text.
    assert_eq!(description.matches(',').count(), 3, "{description:?}");
    assert!(description.contains('…'), "{description:?}");
}

#[test]
fn exactly_the_limit_is_not_cut_short() {
    // The boundary either side of the ellipsis, which the Python suite pinned
    // only from above.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    for n in 0..3 {
        write(&clone.join(format!("file{n}.txt")), "x\n");
    }

    let description = would_lose(&held(&clone));

    assert_eq!(
        description,
        "3 uncommitted change(s) (file0.txt, file1.txt, file2.txt)"
    );
}

#[test]
fn a_renamed_path_keeps_both_halves() {
    // A rename reads `old -> new`, and the whole field is kept rather than split,
    // because both halves are the news.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["mv", "feature.txt", "renamed.txt"]);

    assert_eq!(
        would_lose(&held(&clone)),
        "1 uncommitted change(s) (feature.txt -> renamed.txt)"
    );
}

#[test]
fn a_porcelain_line_too_short_to_hold_a_path_names_nothing() {
    // Python's `if len(line) > 3` filter, which drops the line from the names
    // while still counting it. Unreachable from git as far as anyone knows, and
    // pinned because the alternative in Rust is a panic on a slice.
    let losses = Losses::one(Loss::Uncommitted(
        NonEmpty::of(vec!["??".to_owned(), "?? kept.md".to_owned()]).expect("two lines"),
    ));

    assert_eq!(losses.describe(), "2 uncommitted change(s) (kept.md)");
}

#[test]
fn a_multibyte_path_is_cut_at_the_third_character_not_the_third_byte() {
    // The porcelain columns are ASCII but a path need not be; slicing a byte
    // offset into the middle of a character would be a panic where Python had an
    // answer.
    let losses = Losses::one(Loss::Uncommitted(NonEmpty::one(
        "?? café/über.md".to_owned(),
    )));

    assert_eq!(losses.describe(), "1 uncommitted change(s) (café/über.md)");
}
