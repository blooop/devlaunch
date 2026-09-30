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
//! Ported from the Python `test_workspace_state` (retired with the Python tree
//! in #267), whose five module-level classes were:
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

/// A bare repository standing in for GitHub, with one commit on `main`, its
/// objects named by *object_format*.
fn remote_at(root: &Path, object_format: &str) -> PathBuf {
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    let format = format!("--object-format={object_format}");
    git(root, &["init", "-q", "-b", "main", &format, "seed"]);
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
        Self::named_by("sha1")
    }

    /// A fixture whose repositories name their objects by *object_format*.
    fn named_by(object_format: &str) -> Self {
        let root = tempfile::tempdir().expect("a temp dir");
        let remote = remote_at(root.path(), object_format);
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

/// [`read_clone`] against real git, with no bare cache named.
///
/// [`BareCache::Unknown`] is the honest default for a clone built by hand in a
/// temp directory, and it is invisible to every test with no tags in its clone —
/// which is all of them but the four #485 and #487 are about. Those name the bare
/// through [`read_against`] / [`held_against`], because *which* bare is the whole
/// question there.
fn read(clone: &Path) -> CloneState {
    read_against(clone, BareCache::Unknown)
}

/// [`holds_unsaved_work`] against real git, with no bare cache named.
fn held(clone: &Path) -> Unsaved {
    read(clone).unsaved
}

/// [`read_clone`] against real git, told where dl's mirror of the remote is.
fn read_against(clone: &Path, bare: BareCache<'_>) -> CloneState {
    let runner = ProcessRunner::new();
    read_clone(&Git::new(&runner), clone, bare)
}

/// [`holds_unsaved_work`] against real git, told where dl's mirror of the remote
/// is.
fn held_against(clone: &Path, bare: &Path) -> Unsaved {
    let runner = ProcessRunner::new();
    holds_unsaved_work(&Git::new(&runner), clone, BareCache::At(bare))
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

// ------------------------------------ work on the remote as new commits

/// Run a git command that writes a commit, with an identity this test owns.
fn git_as_author(cwd: &Path, args: &[&str]) -> String {
    let mut argv = vec!["-c", "user.email=t@t", "-c", "user.name=t"];
    argv.extend(args);
    git(cwd, &argv)
}

/// A second clone of *remote*, for the teammate who rewrites it.
fn teammate(fixture: &Fixture) -> PathBuf {
    let mate = fixture.path("mate");
    git(
        fixture.root.path(),
        &[
            "clone",
            "-q",
            fixture.remote.to_str().expect("utf-8"),
            mate.to_str().expect("utf-8"),
        ],
    );
    mate
}

/// Move `main` on the remote by one commit that touches nothing *clone* has.
fn move_main(mate: &Path) {
    git(mate, &["checkout", "-q", "main"]);
    write(&mate.join("main.txt"), "main moved\n");
    commit(mate, "main moved");
    git(mate, &["push", "-q", "origin", "main"]);
}

/// Rebase the remote's `feature` onto its moved `main` and force-push it, so
/// every commit on it gets a new hash.
fn rebase_feature_on_the_remote(mate: &Path) {
    move_main(mate);
    git(mate, &["checkout", "-q", "feature"]);
    git_as_author(mate, &["rebase", "-q", "main"]);
    git(mate, &["push", "-q", "--force", "origin", "feature"]);
}

/// The commits on *branch* that no remote-tracking ref has, by hash.
fn by_sha(clone: &Path, branch: &str) -> usize {
    git(clone, &["log", "--oneline", branch, "--not", "--remotes"])
        .lines()
        .count()
}

#[test]
fn a_branch_the_remote_rebased_holds_nothing_unsaved() {
    // The kinisi_ros case that refused `rm` over 23 commits: the remote branch was
    // rebased, so the clone's old commits are on no remote-tracking ref, and every
    // one of them is on the remote in its new form.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    for name in ["a", "b"] {
        write(&clone.join(format!("{name}.txt")), "work\n");
        commit(&clone, name);
    }
    git(&clone, &["push", "-q", "origin", "feature"]);
    rebase_feature_on_the_remote(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);

    // The premise, asserted rather than assumed: by hash, all three commits are
    // unpushed now.
    assert_eq!(by_sha(&clone, "feature"), 3);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_commit_the_remote_rebased_in_a_sha256_repository_holds_nothing_unsaved() {
    // The copy rule replays the commit onto the copy's parent, which edits the
    // same file far from it, so the replay merges content. Told to read
    // attributes from the SHA-1 empty tree, git refused it, and the commit
    // stayed counted.
    let fixture = Fixture::named_by("sha256");
    let clone = fixture.clone();
    write(&clone.join("list.txt"), &numbered(&[]));
    commit(&clone, "list");
    git(&clone, &["push", "-q", "origin", "feature"]);
    write(&clone.join("list.txt"), &numbered(&[(18, "eighteen")]));
    commit(&clone, "eighteen");
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "feature"]);
    write(&mate.join("list.txt"), &numbered(&[(2, "two")]));
    commit(&mate, "two");
    write(
        &mate.join("list.txt"),
        &numbered(&[(2, "two"), (18, "eighteen")]),
    );
    commit(&mate, "eighteen");
    git(&mate, &["push", "-q", "origin", "feature"]);
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 1);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_rebased_copy_in_a_clone_with_info_attributes_stays_counted() {
    // No option switches `info/attributes` off, so the replay that confirms a
    // copy is not made, and the copies stay counted.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    for name in ["a", "b"] {
        write(&clone.join(format!("{name}.txt")), "work\n");
        commit(&clone, name);
    }
    git(&clone, &["push", "-q", "origin", "feature"]);
    rebase_feature_on_the_remote(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);
    write(&clone.join(".git/info/attributes"), "*.bin -diff\n");

    assert_eq!(would_lose(&held(&clone)), "3 unpushed commit(s)");
}

#[test]
fn a_copy_that_replays_cleanly_only_under_a_union_attribute_stays_counted() {
    // The local commit adds `L` after `b3`. The remote's parent of the copy
    // adds `new b1 b2 b3` at the same place, and the copy adds `L` after the
    // second `b3`, so the two patches are the same text. As text the replay
    // conflicts: both sides add at one place. `merge=union` keeps both sides
    // and gives the copy's tree, and the rule merges as text.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    let lines = |lines: &[&str]| {
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>()
    };
    let head = ["x1", "x2", "x3", "x4", "b1", "b2", "b3"];
    let tail = ["a1", "a2", "a3", "y1", "y2", "y3"];
    let again = ["new", "b1", "b2", "b3"];
    write(&clone.join(".gitattributes"), "N merge=union\n");
    write(&clone.join("N"), &lines(&[&head[..], &tail[..]].concat()));
    commit(&clone, "base");
    git(&clone, &["push", "-q", "origin", "feature"]);
    write(
        &clone.join("N"),
        &lines(&[&head[..], &["L"], &tail[..]].concat()),
    );
    commit(&clone, "add L");
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "feature"]);
    write(
        &mate.join("N"),
        &lines(&[&head[..], &again[..], &tail[..]].concat()),
    );
    commit(&mate, "add new");
    write(
        &mate.join("N"),
        &lines(&[&head[..], &again[..], &["L"], &tail[..]].concat()),
    );
    commit(&mate, "add L");
    git(&mate, &["push", "-q", "origin", "feature"]);
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(
        cherry_marked(&clone, "feature...origin/feature").len(),
        1,
        "the copy has the local commit's patch"
    );

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_branch_the_remote_rebased_holds_nothing_unsaved_in_colour_too() {
    // `color.ui=always` puts an escape code before every hash `log --oneline`
    // prints, and a line that starts with one names no commit.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["config", "color.ui", "always"]);
    rebase_feature_on_the_remote(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 1);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_backup_made_before_the_remote_rebase_holds_nothing_unsaved() {
    // The shape that kept most of the old commits on a real host: a backup branch
    // with no upstream, whose copies are on *another* local branch's upstream.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["branch", "backup/before-rebase"]);
    rebase_feature_on_the_remote(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);
    git_as_author(&clone, &["reset", "-q", "--hard", "origin/feature"]);
    assert_eq!(by_sha(&clone, "backup/before-rebase"), 1);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn only_the_commits_the_rebased_remote_has_a_copy_of_drop_out() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    rebase_feature_on_the_remote(&teammate(&fixture));
    write(&clone.join("unpushed.txt"), "an hour of work\n");
    commit(&clone, "unpushed");
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 2);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_commit_squash_merged_into_the_default_branch_is_saved() {
    // A one-commit PR squashed into `main` lands as a new commit with the same
    // patch. The branch's own remote ref never got the commit, and with no local
    // `main` tracking `origin/main`, only `origin/HEAD` can say the remote has it.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("fix.txt"), "the fix\n");
    commit(&clone, "fix");
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "main"]);
    write(&mate.join("fix.txt"), "the fix\n");
    commit(&mate, "fix (#1)");
    git(&mate, &["push", "-q", "origin", "main"]);
    git(&clone, &["fetch", "-q", "origin"]);
    git(&clone, &["branch", "-q", "-D", "main"]);
    assert_eq!(by_sha(&clone, "feature"), 1);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_branch_that_tracks_nothing_is_compared_with_its_namesake_on_the_remote() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    rebase_feature_on_the_remote(&teammate(&fixture));
    git(&clone, &["branch", "-q", "--unset-upstream", "feature"]);
    git(&clone, &["branch", "-q", "-D", "main"]);
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 1);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_squash_that_leaves_out_part_of_the_branch_leaves_every_commit_counted() {
    // No one commit's patch is the squash's, so the copy rule finds nothing. The
    // squash rule compares the branch's whole change since it left `main`, and
    // `feature.txt` is part of that change but not of this squash, so it finds
    // nothing either. Stricter than it has to be (`feature.txt` is on
    // `origin/feature`), and that is the safe side.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    for name in ["a", "b"] {
        write(&clone.join(format!("{name}.txt")), "work\n");
        commit(&clone, name);
    }
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "main"]);
    for name in ["a", "b"] {
        write(&mate.join(format!("{name}.txt")), "work\n");
    }
    commit(&mate, "a and b (#2)");
    git(&mate, &["push", "-q", "origin", "main"]);
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "2 unpushed commit(s)");
}

/// Three unpushed commits on `feature`, the second editing the file the first
/// wrote, so no one of them has the squash's patch.
fn three_commits(clone: &Path) {
    write(&clone.join("notes.txt"), "one\n");
    commit(clone, "a");
    write(&clone.join("notes.txt"), "one\ntwo\n");
    commit(clone, "b");
    write(&clone.join("other.txt"), "three\n");
    commit(clone, "c");
}

/// Squash `feature` into the remote's `main` as one commit, the way GitHub
/// writes a squash merge: the whole change of the branch since it left `main`.
fn squash_feature_into_main(mate: &Path) {
    git(mate, &["checkout", "-q", "main"]);
    write(&mate.join("feature.txt"), "work\n");
    write(&mate.join("notes.txt"), "one\ntwo\n");
    write(&mate.join("other.txt"), "three\n");
    commit(mate, "feature (#3)");
    git(mate, &["push", "-q", "origin", "main"]);
}

#[test]
fn a_branch_squashed_into_the_default_branch_holds_nothing_unsaved() {
    // kinisi_ros#12035's workspace, which `rm` refused over 3 commits: the PR was
    // squashed into `main`, and its whole change is on `origin/main` as one
    // commit that matches none of the three.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    squash_feature_into_main(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 3);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

/// Twenty numbered lines, with line *n* replaced by *text* for each edit.
fn numbered(edits: &[(usize, &str)]) -> String {
    (1..=20)
        .map(|n| {
            let edit = edits.iter().find(|(line, _)| *line == n);
            edit.map_or_else(|| format!("{n}\n"), |(_, text)| format!("{text}\n"))
        })
        .collect()
}

/// A SHA-256 clone that pushed `list.txt` on `feature`, then made three
/// unpushed commits that edit lines 5 to 7 of it; and a teammate's push to
/// `feature` of those three edits as one commit, with line 18 as *line_18*
/// and line 6 as *line_6*. Both sides change `list.txt`, so the merge that
/// compares them merges its content.
fn squashed_on_feature_in_sha256(fixture: &Fixture, line_6: &str, line_18: &str) -> PathBuf {
    let clone = fixture.clone();
    assert_eq!(
        git(&clone, &["rev-parse", "--show-object-format"]),
        "sha256"
    );
    write(&clone.join("list.txt"), &numbered(&[]));
    commit(&clone, "list");
    git(&clone, &["push", "-q", "origin", "feature"]);
    for (line, text) in [(5, "five"), (6, "six"), (7, "seven")] {
        let edits: Vec<(usize, &str)> = [(5, "five"), (6, "six"), (7, "seven")]
            .into_iter()
            .filter(|(done, _)| *done <= line)
            .collect();
        write(&clone.join("list.txt"), &numbered(&edits));
        commit(&clone, text);
    }
    let mate = teammate(fixture);
    git(&mate, &["checkout", "-q", "feature"]);
    write(
        &mate.join("list.txt"),
        &numbered(&[(5, "five"), (6, line_6), (7, "seven"), (18, line_18)]),
    );
    commit(&mate, "five to seven, squashed");
    git(&mate, &["push", "-q", "origin", "feature"]);
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 3);
    clone
}

#[test]
fn a_squash_in_a_sha256_repository_holds_nothing_unsaved() {
    // The empty tree has another name under SHA-256, and a content merge told to
    // read attributes from the SHA-1 name is refused.
    let fixture = Fixture::named_by("sha256");
    let clone = squashed_on_feature_in_sha256(&fixture, "six", "eighteen");

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_squash_that_conflicts_in_a_sha256_repository_leaves_every_commit_counted() {
    // The control for the one above: the remote's line 6 is not the branch's,
    // so the merge conflicts, and a conflict clears nothing.
    let fixture = Fixture::named_by("sha256");
    let clone = squashed_on_feature_in_sha256(&fixture, "SIX", "eighteen");

    assert_eq!(would_lose(&held(&clone)), "3 unpushed commit(s)");
}

#[test]
fn a_squash_the_default_branch_later_reverted_leaves_every_commit_counted() {
    // `main` no longer holds the change, so the merge puts it back, and the tree
    // it gives is not `main`'s. `main` moves on after the revert, so its tree is
    // not the merge base's, and the merge is made.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    let mate = teammate(&fixture);
    squash_feature_into_main(&mate);
    git_as_author(&mate, &["revert", "--no-edit", "HEAD"]);
    git(&mate, &["push", "-q", "origin", "main"]);
    move_main(&mate);
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "3 unpushed commit(s)");
}

#[test]
fn a_commit_made_after_the_squash_is_the_one_counted() {
    // The tip holds work `main` has not got, so the rule looks further down the
    // branch for the last commit whose change `main` does hold.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    squash_feature_into_main(&teammate(&fixture));
    write(&clone.join("later.txt"), "an hour of work\n");
    commit(&clone, "later");
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 4);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_squash_the_default_branch_then_edited_on_the_same_lines_leaves_every_commit_counted() {
    // The limit, pinned: `main` changed the lines the squash wrote, so the merge
    // conflicts, and a conflict clears nothing. The change is still in `main`'s
    // history, but this rule reads trees, not history.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    let mate = teammate(&fixture);
    squash_feature_into_main(&mate);
    write(&mate.join("notes.txt"), "one\nTWO\n");
    commit(&mate, "edit the squash");
    git(&mate, &["push", "-q", "origin", "main"]);
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "3 unpushed commit(s)");
}

#[test]
fn a_squashed_branch_that_merged_the_default_branch_holds_nothing_unsaved() {
    // A merge inside the branch moves where it left `main`, and the change the
    // rule compares is the one since that merge.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("notes.txt"), "one\n");
    commit(&clone, "a");
    let mate = teammate(&fixture);
    move_main(&mate);
    git(&clone, &["fetch", "-q", "origin"]);
    git_as_author(&clone, &["merge", "-q", "--no-edit", "origin/main"]);
    write(&clone.join("notes.txt"), "one\ntwo\n");
    commit(&clone, "b");
    squash_feature_into_main(&mate);
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 3);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_squashed_commit_another_branch_still_needs_stays_counted() {
    // `a` wrote `one`, and `b` on `feature` replaced it with `two`, which is
    // what the squash holds. So `feature` passes, and `a` is in its history
    // only. `side` grew from `a` and does not pass at any commit: nothing on the
    // remote holds `one`. `a` stays counted with `side`, and only `b` drops out.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("notes.txt"), "one\n");
    commit(&clone, "a");
    git(&clone, &["checkout", "-q", "-b", "side"]);
    write(&clone.join("side.txt"), "an hour of work\n");
    commit(&clone, "side");
    git(&clone, &["checkout", "-q", "feature"]);
    write(&clone.join("notes.txt"), "two\n");
    commit(&clone, "b");
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "main"]);
    write(&mate.join("feature.txt"), "work\n");
    write(&mate.join("notes.txt"), "two\n");
    commit(&mate, "feature (#3)");
    git(&mate, &["push", "-q", "origin", "main"]);
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "2 unpushed commit(s)");
}

#[test]
fn a_merge_that_dropped_a_deleted_side_branch_leaves_that_branch_counted() {
    // `merge -s ours` puts `side` in `feature`'s history and none of its change
    // in `feature`'s tree, so the squash of `feature` holds none of `secret.txt`.
    // `side` is gone, and its commit is reachable only through the merge's second
    // parent. The squash proves the first-parent line, and the one commit it
    // does not prove stays counted.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["checkout", "-q", "-b", "side"]);
    write(&clone.join("secret.txt"), "an hour of work\n");
    commit(&clone, "secret");
    git(&clone, &["checkout", "-q", "feature"]);
    three_commits(&clone);
    git_as_author(&clone, &["merge", "-q", "--no-edit", "-s", "ours", "side"]);
    git(&clone, &["branch", "-q", "-D", "side"]);
    squash_feature_into_main(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 5);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_commit_and_its_revert_are_not_cleared_by_a_remote_that_moved() {
    // The branch changes nothing since it left `origin/feature`, so any merge of
    // it into `origin/feature` gives `origin/feature`'s tree. An empty change
    // proves nothing, and the two commits stay counted.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("notes.txt"), "one\n");
    commit(&clone, "a");
    git_as_author(&clone, &["revert", "--no-edit", "HEAD"]);
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "feature"]);
    write(&mate.join("mate.txt"), "the teammate's\n");
    commit(&mate, "mate");
    git(&mate, &["push", "-q", "origin", "feature"]);
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "2 unpushed commit(s)");
}

#[test]
fn the_squash_rule_never_clears_a_stashed_commit() {
    // The stash reaches the branch's tip, so it holds the branch back as well:
    // the rule clears a commit only when every ref that reaches it is a branch
    // that passed. Two stash commits and the three under them.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    write(&clone.join("stashed.txt"), "half a plan\n");
    git(&clone, &["add", "-A"]);
    git_as_author(&clone, &["stash", "-q"]);
    squash_feature_into_main(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "5 unpushed commit(s)");
}

#[test]
fn a_detached_head_on_a_squashed_branch_holds_it_back() {
    // The same rule for a worktree on a detached HEAD, which is how an agent
    // gets a second checkout: it is not a branch, so it cannot pass.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    let linked = fixture.path("detached");
    git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().expect("utf-8"),
            "feature~1",
        ],
    );
    squash_feature_into_main(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "2 unpushed commit(s)");
}

#[test]
fn a_worktree_on_a_squashed_branch_does_not_hold_it_back() {
    // A worktree that has the branch checked out is that branch, not a ref of
    // its own.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    git(&clone, &["checkout", "-q", "--detach"]);
    let linked = fixture.path("linked");
    git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().expect("utf-8"),
            "feature",
        ],
    );
    git(&clone, &["checkout", "-q", "main"]);
    squash_feature_into_main(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

/// `feature` with `NOTES` under the built-in `union` driver, pushed; a local
/// commit that deletes `old line`; and a teammate's push that rewrites it.
/// Where the attribute lives is *attributes*' to say.
fn a_deletion_the_remote_rewrote(fixture: &Fixture, attributes: impl Fn(&Path)) -> PathBuf {
    let clone = fixture.clone();
    attributes(&clone);
    write(&clone.join("NOTES"), "keep\nold line\nkeep2\n");
    commit(&clone, "notes");
    git(&clone, &["push", "-q", "origin", "feature"]);
    write(&clone.join("NOTES"), "keep\nkeep2\n");
    commit(&clone, "drop the old line");
    let mate = teammate(fixture);
    git(&mate, &["checkout", "-q", "feature"]);
    write(&mate.join("NOTES"), "keep\nremote rewrite\nkeep2\n");
    commit(&mate, "rewrite the old line");
    git(&mate, &["push", "-q", "origin", "feature"]);
    git(&clone, &["fetch", "-q", "origin"]);
    clone
}

#[test]
fn a_union_merge_attribute_does_not_let_the_squash_rule_clear_a_commit() {
    // `merge=union` keeps both sides' lines and never conflicts, so the merge of
    // the deletion into `origin/feature` is clean and gives `origin/feature`'s
    // tree: the deletion is dropped, not held. The rule merges as plain text.
    let fixture = Fixture::new();
    let clone = a_deletion_the_remote_rewrote(&fixture, |clone| {
        write(&clone.join(".gitattributes"), "NOTES merge=union\n");
    });

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_merge_attribute_in_info_attributes_leaves_the_commit_counted() {
    // `--attr-source` does not reach `$GIT_DIR/info/attributes`, so a clone
    // with one cannot say how git would merge, and clears nothing.
    let fixture = Fixture::new();
    let clone = a_deletion_the_remote_rewrote(&fixture, |clone| {
        write(&clone.join(".git/info/attributes"), "NOTES merge=union\n");
    });

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn an_empty_info_attributes_still_lets_the_squash_rule_clear() {
    // An empty file names no merge driver, so the merge runs.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    squash_feature_into_main(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);
    write(&clone.join(".git/info/attributes"), "");

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_configured_merge_driver_does_not_let_the_squash_rule_clear_a_commit() {
    // A driver of `true` keeps the remote's side of every file it owns, so the
    // merge of the line added after the squash gives `main`'s tree.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["config", "merge.ours.driver", "true"]);
    write(&clone.join(".gitattributes"), "notes.txt merge=ours\n");
    write(&clone.join("notes.txt"), "one\n");
    commit(&clone, "a");
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "main"]);
    write(&mate.join("feature.txt"), "work\n");
    write(&mate.join(".gitattributes"), "notes.txt merge=ours\n");
    write(&mate.join("notes.txt"), "one\n");
    commit(&mate, "feature (#3)");
    git(&mate, &["push", "-q", "origin", "main"]);
    write(&clone.join("notes.txt"), "one\ntwo\n");
    commit(&clone, "b");
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

/// A clone at `/ws` with one branch, `feature`, and one counted commit whose
/// change merges cleanly into its remote refs when *merge_tree* says so.
fn scripted_squash(merge_tree: Response) -> ScriptedRunner {
    scripted_squash_with(Response::stdout("base\n"), Response::stdout(""), merge_tree)
}

/// [`scripted_squash`], with the answers to `merge-base` and to the
/// `rev-list` of the refs that are not branches as well.
fn scripted_squash_with(
    merge_base: Response,
    off_every_branch: Response,
    merge_tree: Response,
) -> ScriptedRunner {
    scripted_squash_after(
        ScriptedRunner::new(),
        merge_base,
        off_every_branch,
        merge_tree,
    )
}

/// [`scripted_squash_with`], after the scripts *first* holds, which answer
/// before any of its own.
fn scripted_squash_after(
    first: ScriptedRunner,
    merge_base: Response,
    off_every_branch: Response,
    merge_tree: Response,
) -> ScriptedRunner {
    const COUNTED: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
    fn at(verb: &'static str) -> [&'static str; 4] {
        ["git", "--git-dir=/ws/.git", "--work-tree=/ws", verb]
    }
    first
        .with_script(
            [
                "git",
                "--git-dir=/ws/.git",
                "--work-tree=/ws",
                "for-each-ref",
                "--format=%(refname)%00%(tree)",
            ],
            Response::stdout("refs/remotes/origin/feature\0remote-tree\n"),
        )
        .with_script(
            at("for-each-ref"),
            Response::stdout("refs/heads/feature\0refs/remotes/origin/feature\n"),
        )
        .with_script(
            [
                "git",
                "--git-dir=/ws/.git",
                "--work-tree=/ws",
                "rev-list",
                "--exclude=refs/heads/*",
            ],
            off_every_branch,
        )
        .with_script(at("rev-list"), Response::stdout(format!("{COUNTED}\n")))
        .with_script(at("merge-base"), merge_base)
        .with_script(
            [
                "git",
                "--git-dir=/ws/.git",
                "--work-tree=/ws",
                "rev-parse",
                "--git-path",
            ],
            Response::stdout("/ws/.git/info/attributes\nsha1\n"),
        )
        .with_script(
            at("rev-parse"),
            Response::stdout("tip-tree\nbase-tree\nremote-tree\n"),
        )
        .with_script(
            [
                "git",
                "--git-dir=/ws/.git",
                "--work-tree=/ws",
                "-c",
                "core.attributesFile=/dev/null",
                "-c",
                "merge.default=text",
                "--attr-source=4b825dc642cb6eb9a060e54bf8d69288fbee4904",
                "merge-tree",
                "--stdin",
            ],
            Response::stdout("1\0remote-tree\0\0"),
        )
        .with_script(
            ["git", "--git-dir=/ws/.git", "--work-tree=/ws", "-c"],
            merge_tree,
        )
        .with_script(at("worktree"), Response::stdout(""))
}

#[test]
fn the_squash_rule_makes_no_more_merges_than_its_budget() {
    // 32 branches, each on an upstream of its own with eight counted points:
    // each branch merges 8 points into 32 refs, 256 merges, so 16 branches fit
    // and the other 16 are not merged. The spawns grow with the branches, not
    // with the merges.
    let branches = 32;
    let at = |verb: &'static str| ["git", "--git-dir=/ws/.git", "--work-tree=/ws", verb];
    let listing: String = (0..branches)
        .map(|b| format!("refs/heads/b{b}\0refs/remotes/origin/b{b}\n"))
        .collect();
    let trees: String = (0..branches)
        .map(|b| format!("refs/remotes/origin/b{b}\0tree-{b}\n"))
        .collect();
    let points: String = (0..LOOK_BACK).map(|p| format!("{p:040x}\n")).collect();
    let fake = ScriptedRunner::new()
        .with_script(
            [
                "git",
                "--git-dir=/ws/.git",
                "--work-tree=/ws",
                "for-each-ref",
                "--format=%(refname)%00%(tree)",
            ],
            Response::stdout(trees),
        )
        .with_script(at("for-each-ref"), Response::stdout(listing))
        .with_script(
            [
                "git",
                "--git-dir=/ws/.git",
                "--work-tree=/ws",
                "rev-parse",
                "--git-path",
            ],
            Response::stdout("/ws/.git/info/attributes\nsha1\n"),
        )
        .with_script(at("rev-list"), Response::stdout(points))
        .with_script(at("-c"), Response::stdout(""));
    let counted = ["0000000 work".to_owned()];

    assert_eq!(
        squashed_onto_a_remote(&Git::new(&fake), Path::new("/ws"), &counted, &[]),
        Vec::<String>::new()
    );
    let calls = fake.calls();
    let merged = calls
        .iter()
        .filter(|call| call.argv().iter().any(|arg| arg == "--stdin"))
        .count();
    assert_eq!(merged, MERGE_BUDGET / (LOOK_BACK * branches));
    assert!(
        calls.len() <= 3 * branches + 3,
        "{} spawns for {branches} branches",
        calls.len()
    );
}

#[test]
fn a_merge_git_refuses_clears_nothing() {
    // A conflict, a timeout and a merge-tree too old to write a tree all arrive
    // as a refusal, and a refusal is no evidence that the remote holds anything.
    let counted = ["a1b2c3d squashed".to_owned()];
    let refused = scripted_squash(Response::failed(1, "CONFLICT (content)"));

    assert_eq!(
        squashed_onto_a_remote(&Git::new(&refused), Path::new("/ws"), &counted, &[]),
        Vec::<String>::new()
    );

    // The control: the same script with a clean merge that gives the remote's
    // tree clears the commit, so the refusal is what cleared nothing.
    let clean = scripted_squash(Response::stdout("remote-tree\n"));
    assert_eq!(
        squashed_onto_a_remote(&Git::new(&clean), Path::new("/ws"), &counted, &[]),
        vec!["a1b2c3d4e5f60718293a4b5c6d7e8f9012345678".to_owned()]
    );
}

#[test]
fn a_change_measured_from_two_merge_bases_clears_nothing() {
    // With two bases git would merge the bases first, and a clean merge from a
    // base git made up is no evidence. The same clean merge-tree as the control
    // in `a_merge_git_refuses_clears_nothing` clears the commit there.
    let counted = ["a1b2c3d squashed".to_owned()];
    let two_bases = scripted_squash_with(
        Response::stdout("base\nother-base\n"),
        Response::stdout(""),
        Response::stdout("remote-tree\n"),
    );

    assert_eq!(
        squashed_onto_a_remote(&Git::new(&two_bases), Path::new("/ws"), &counted, &[]),
        Vec::<String>::new()
    );
}

#[test]
fn refs_that_are_not_branches_git_will_not_list_clear_nothing() {
    // What was not read may reach the commit, so a passing branch clears
    // nothing when git refuses to say what the stash, the tags and the detached
    // HEADs reach.
    let counted = ["a1b2c3d squashed".to_owned()];
    let refused = scripted_squash_with(
        Response::stdout("base\n"),
        Response::failed(128, "fatal: nope"),
        Response::stdout("remote-tree\n"),
    );

    assert_eq!(
        squashed_onto_a_remote(&Git::new(&refused), Path::new("/ws"), &counted, &[]),
        Vec::<String>::new()
    );
}

#[test]
fn a_branch_git_will_not_list_after_one_passed_clears_nothing() {
    // `feature` passes. What `other` reaches was not read, and it may hold the
    // commit back, so nothing is cleared.
    let counted = ["a1b2c3d squashed".to_owned()];
    let two_branches = |other_reaches: Response| {
        let first = ScriptedRunner::new()
            .with_script(
                [
                    "git",
                    "--git-dir=/ws/.git",
                    "--work-tree=/ws",
                    "for-each-ref",
                    "--format=%(refname)%00%(upstream)",
                ],
                Response::stdout(
                    "refs/heads/feature\0refs/remotes/origin/feature\nrefs/heads/other\0\n",
                ),
            )
            .with_script(
                [
                    "git",
                    "--git-dir=/ws/.git",
                    "--work-tree=/ws",
                    "rev-list",
                    "refs/heads/other",
                ],
                other_reaches,
            );
        scripted_squash_after(
            first,
            Response::stdout("base\n"),
            Response::stdout(""),
            Response::stdout("remote-tree\n"),
        )
    };
    let refused = two_branches(Response::failed(128, "fatal: nope"));

    assert_eq!(
        squashed_onto_a_remote(&Git::new(&refused), Path::new("/ws"), &counted, &[]),
        Vec::<String>::new()
    );

    // The control: `other` reaches nothing unpushed, and `feature`'s pass
    // clears the commit.
    let read = two_branches(Response::stdout(""));
    assert_eq!(
        squashed_onto_a_remote(&Git::new(&read), Path::new("/ws"), &counted, &[]),
        vec!["a1b2c3d4e5f60718293a4b5c6d7e8f9012345678".to_owned()]
    );
}

#[test]
fn a_local_tag_on_a_squashed_commit_holds_it_back() {
    // The tag is not a branch, so it cannot pass, and what it reaches stays
    // counted: `a` and `b`, with `c` above it cleared.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    git(&clone, &["tag", "keep", "feature~1"]);
    squash_feature_into_main(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "2 unpushed commit(s)");
}

/// *count* commits on `feature` after the squashed three, each a file of its
/// own.
fn more_commits(clone: &Path, count: usize) {
    for n in 0..count {
        write(&clone.join(format!("later-{n}.txt")), "more work\n");
        commit(clone, &format!("later {n}"));
    }
}

#[test]
fn a_squash_seven_commits_under_the_tip_is_found() {
    // The eighth point down is the squashed tip, `c`, the last one looked at.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    squash_feature_into_main(&teammate(&fixture));
    more_commits(&clone, 7);
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "7 unpushed commit(s)");
}

#[test]
fn a_squash_eight_commits_under_the_tip_is_not_looked_for() {
    // The limit, pinned: every point looked at holds later work, so nothing
    // passes, and the squashed three stay counted with the eight above them.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    three_commits(&clone);
    squash_feature_into_main(&teammate(&fixture));
    more_commits(&clone, 8);
    git(&clone, &["fetch", "-q", "origin"]);

    assert_eq!(would_lose(&held(&clone)), "11 unpushed commit(s)");
}

#[test]
fn a_merge_of_the_default_branch_adds_nothing_of_its_own() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    move_main(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);
    git_as_author(&clone, &["merge", "-q", "--no-edit", "origin/main"]);
    assert_eq!(by_sha(&clone, "feature"), 1);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_merge_with_a_conflict_resolved_by_hand_is_still_unsaved() {
    // The resolution is work of the merge's own, and it may exist nowhere else.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("README.md"), "ours\n");
    commit(&clone, "ours");
    git(&clone, &["push", "-q", "origin", "feature"]);
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "main"]);
    write(&mate.join("README.md"), "theirs\n");
    commit(&mate, "theirs");
    git(&mate, &["push", "-q", "origin", "main"]);
    git(&clone, &["fetch", "-q", "origin"]);
    let conflicted = Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "merge",
            "-q",
            "origin/main",
        ])
        .current_dir(&clone)
        .output()
        .expect("git is installed");
    assert!(!conflicted.status.success(), "the premise: a conflict");
    // A merge that fails for any other reason, no identity on a CI runner say,
    // fails the same way and leaves no merge in progress.
    assert!(
        clone.join(".git/MERGE_HEAD").exists(),
        "the premise: a merge stopped on its conflict: {}",
        String::from_utf8_lossy(&conflicted.stderr)
    );
    write(&clone.join("README.md"), "resolved by hand\n");
    git(&clone, &["add", "README.md"]);
    git_as_author(&clone, &["commit", "-q", "--no-edit"]);
    assert_eq!(by_sha(&clone, "feature"), 1);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_change_of_indentation_alone_is_not_a_copy() {
    // git's patch id drops whitespace, and in Python, a Makefile or YAML the
    // indentation is the change.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("guard.py"), "if a:\n    x()\n");
    commit(&clone, "guard");
    git(&clone, &["push", "-q", "origin", "feature"]);
    write(&clone.join("guard.py"), "if a:\nx()\n");
    git(&clone, &["add", "-A"]);
    git_as_author(&clone, &["commit", "-q", "--amend", "--no-edit"]);
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 1);
    assert_eq!(cherry_marked(&clone, "feature...origin/feature").len(), 1);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

/// A clone whose pushed `guard.py` commit was amended to indent its body with
/// *indent* instead of *pushed*.
fn reindented_after_the_push(fixture: &Fixture, pushed: &str, indent: &str) -> PathBuf {
    let clone = fixture.clone();
    write(&clone.join("guard.py"), &format!("if a:\n{pushed}x()\n"));
    commit(&clone, "guard");
    git(&clone, &["push", "-q", "origin", "feature"]);
    write(&clone.join("guard.py"), &format!("if a:\n{indent}x()\n"));
    git(&clone, &["add", "-A"]);
    git_as_author(&clone, &["commit", "-q", "--amend", "--no-edit"]);
    git(&clone, &["fetch", "-q", "origin"]);
    clone
}

#[test]
fn indentation_changed_but_not_removed_is_not_a_copy() {
    // A run of whitespace that only changes its length, or a tab that turns
    // into spaces, is still the change.
    for (pushed, indent) in [("    ", "  "), ("\t", "    ")] {
        let fixture = Fixture::new();
        let clone = reindented_after_the_push(&fixture, pushed, indent);
        assert_eq!(by_sha(&clone, "feature"), 1);
        assert_eq!(
            cherry_marked(&clone, "feature...origin/feature").len(),
            1,
            "the premise: the patch id matches {pushed:?} with {indent:?}"
        );

        assert_eq!(
            would_lose(&held(&clone)),
            "1 unpushed commit(s)",
            "{pushed:?} became {indent:?}"
        );
    }
}

#[test]
fn a_mode_change_added_to_a_pushed_commit_is_not_a_copy() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    make_executable(&clone.join("feature.txt"));
    git(&clone, &["add", "-A"]);
    git_as_author(&clone, &["commit", "-q", "--amend", "--no-edit"]);
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 1);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_mode_change_the_remote_rebased_is_saved() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    make_executable(&clone.join("feature.txt"));
    commit(&clone, "executable");
    git(&clone, &["push", "-q", "origin", "feature"]);
    rebase_feature_on_the_remote(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 2);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[test]
fn a_binary_change_the_remote_rebased_is_saved() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    let blob = clone.join("blob.bin");
    std::fs::write(&blob, b"\x00\x01binary\xff\n").expect("written");
    commit(&clone, "binary");
    std::fs::write(&blob, b"\x00\x02binary, changed\xfe\n").expect("written");
    commit(&clone, "binary changed");
    git(&clone, &["push", "-q", "origin", "feature"]);
    rebase_feature_on_the_remote(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 3);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_rename_with_an_edit_the_remote_rebased_is_saved() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    let lines: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    write(&clone.join("notes.txt"), &lines);
    commit(&clone, "notes");
    git(&clone, &["mv", "notes.txt", "moved.txt"]);
    write(
        &clone.join("moved.txt"),
        &lines.replace("line 5", "line five"),
    );
    commit(&clone, "move the notes");
    git(&clone, &["push", "-q", "origin", "feature"]);
    rebase_feature_on_the_remote(&teammate(&fixture));
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 3);

    assert_eq!(held(&clone), Unsaved::NothingToLose);
}

#[test]
fn a_commit_reindented_after_the_remote_rebase_is_the_one_counted() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    for name in ["a", "b"] {
        write(&clone.join(format!("{name}.py")), "if a:\n    x()\n");
        commit(&clone, name);
    }
    git(&clone, &["push", "-q", "origin", "feature"]);
    rebase_feature_on_the_remote(&teammate(&fixture));
    write(&clone.join("b.py"), "if a:\n  x()\n");
    git(&clone, &["add", "-A"]);
    git_as_author(&clone, &["commit", "-q", "--amend", "--no-edit"]);
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 3);
    assert_eq!(cherry_marked(&clone, "feature...origin/feature").len(), 3);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

/// The commits on the left of *range* that `rev-list --cherry-mark` marks `=`,
/// the patch-id match the copy rule starts from.
fn cherry_marked(clone: &Path, range: &str) -> Vec<String> {
    git(
        clone,
        &[
            "rev-list",
            "--cherry-mark",
            "--left-only",
            "--no-merges",
            range,
        ],
    )
    .lines()
    .filter_map(|line| line.strip_prefix('='))
    .map(str::to_owned)
    .collect()
}

#[test]
fn the_same_edit_in_another_place_is_not_a_copy() {
    // Two blocks with the same context make the same hunk, line numbers aside,
    // and the local edit to the first block is on no remote.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    let block = |x: &str| format!("p\nq\nr\nx = {x}\ns\nt\nu\n");
    write(
        &clone.join("f.py"),
        &format!("{}\n\n\n\n{}", block("1"), block("1")),
    );
    commit(&clone, "two blocks");
    git(&clone, &["push", "-q", "origin", "feature"]);
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "feature"]);
    write(
        &mate.join("f.py"),
        &format!("{}\n\n\n\n{}", block("1"), block("2")),
    );
    commit(&mate, "the second block");
    git(&mate, &["push", "-q", "origin", "feature"]);
    write(
        &clone.join("f.py"),
        &format!("{}\n\n\n\n{}", block("2"), block("1")),
    );
    commit(&clone, "the first block");
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 1);
    assert_eq!(cherry_marked(&clone, "feature...origin/feature").len(), 1);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn an_empty_commit_is_never_a_copy() {
    // Its message is all it holds, and any empty commit on the remote would
    // otherwise replay as it.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "feature"]);
    git_as_author(
        &mate,
        &["commit", "-q", "--allow-empty", "-m", "ci: retrigger"],
    );
    git(&mate, &["push", "-q", "origin", "feature"]);
    git_as_author(
        &clone,
        &["commit", "-q", "--allow-empty", "-m", "the only record"],
    );
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "feature"), 1);
    assert_eq!(cherry_marked(&clone, "feature...origin/feature").len(), 1);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_root_commit_is_never_a_copy() {
    // `log.showRoot=false` prints a root commit with no patch at all, and even
    // with its patch printed there is no parent to replay it on.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["config", "log.showRoot", "false"]);
    let mate = teammate(&fixture);
    git(&mate, &["checkout", "-q", "feature"]);
    write(&mate.join("n.txt"), "x\n");
    commit(&mate, "n");
    git(&mate, &["push", "-q", "origin", "feature"]);
    git(&clone, &["checkout", "-q", "--orphan", "orphan"]);
    git(&clone, &["rm", "-q", "-r", "-f", "."]);
    write(&clone.join("n.txt"), "x\n");
    commit(&clone, "a root of its own");
    git(&clone, &["fetch", "-q", "origin"]);
    assert_eq!(by_sha(&clone, "orphan"), 1);
    assert_eq!(cherry_marked(&clone, "orphan...origin/feature").len(), 1);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_merged_parent_with_no_copy_is_still_unsaved() {
    // The merge adds nothing of its own, so it drops out, but the commit it
    // brought in is on no remote.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["checkout", "-q", "-b", "side"]);
    write(&clone.join("side.txt"), "side work\n");
    commit(&clone, "side");
    git(&clone, &["checkout", "-q", "feature"]);
    git_as_author(&clone, &["merge", "-q", "--no-ff", "--no-edit", "side"]);
    assert_eq!(by_sha(&clone, "feature"), 2);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn an_octopus_merge_with_an_edit_of_its_own_is_still_unsaved() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    for name in ["a", "b"] {
        git(&clone, &["checkout", "-q", "-b", name, "origin/main"]);
        write(&clone.join(format!("{name}.txt")), "pushed\n");
        commit(&clone, name);
        git(&clone, &["push", "-q", "origin", name]);
    }
    git(&clone, &["checkout", "-q", "feature"]);
    git_as_author(&clone, &["merge", "-q", "--no-ff", "--no-commit", "a", "b"]);
    write(&clone.join("feature.txt"), "edited in the merge\n");
    git(&clone, &["add", "-A"]);
    git_as_author(&clone, &["commit", "-q", "--no-edit"]);
    assert_eq!(by_sha(&clone, "feature"), 1);

    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
}

#[test]
fn a_copy_on_another_local_branch_is_not_a_copy_on_a_remote() {
    // Two local copies of one change are still the only two copies.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("fix.txt"), "the fix\n");
    commit(&clone, "fix");
    git(&clone, &["checkout", "-q", "-b", "other", "origin/main"]);
    git_as_author(&clone, &["cherry-pick", "feature"]);

    assert_eq!(would_lose(&held(&clone)), "2 unpushed commit(s)");
}

#[test]
fn a_local_branch_as_an_upstream_is_not_a_remote() {
    // `branch -u`, `--track` and `branch.autoSetupMerge=always` can all make a
    // local branch the upstream, and a copy on it is still only local.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("fix.txt"), "the fix\n");
    commit(&clone, "fix");
    git(&clone, &["checkout", "-q", "-b", "other", "origin/main"]);
    git(&clone, &["branch", "-q", "-u", "feature", "other"]);
    git_as_author(&clone, &["cherry-pick", "feature"]);

    assert_eq!(would_lose(&held(&clone)), "2 unpushed commit(s)");
}

#[test]
fn a_copy_that_git_will_not_look_for_leaves_every_commit_counted() {
    // Every question the copy rule asks can fail, and a failure must never clear
    // a commit: here git refuses them all, and nothing is found to be copied.
    let refused = ScriptedRunner::new().with_script(["git"], Response::failed(128, "fatal: nope"));

    assert_eq!(
        already_on_a_remote(&Git::new(&refused), Path::new("/ws"), &[]),
        Vec::<String>::new()
    );
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
fn a_tag_no_remote_branch_reaches_any_more_is_not_unsaved_work() {
    // devlaunch#485, and it is the shipped guard refusing every workspace of a
    // real repository: tag a release, delete the branch it was on, and the tag is
    // the only ref left reaching those commits. The remote carries the tag, the
    // clone fetched it, nothing about it is unsaved — but no `refs/remotes/*`
    // reaches it, so a ref set that spans `refs/tags` counts every commit under
    // it as work that exists nowhere else. kinisi_ros has 265 of them, which is the
    // floor under what six of eight workspaces on one host reported, all wrongly.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["checkout", "-q", "-b", "release"]);
    write(&clone.join("release.txt"), "shipped\n");
    commit(&clone, "release");
    git(&clone, &["push", "-q", "origin", "release"]);
    git(&clone, &["tag", "v1"]);
    git(&clone, &["push", "-q", "origin", "v1"]);
    // The branch goes, on the remote and here, exactly as a merged release
    // branch does. The tag stays on both sides.
    git(&clone, &["push", "-q", "origin", ":release"]);
    git(&clone, &["checkout", "-q", "feature"]);
    git(&clone, &["branch", "-qD", "release"]);
    git(&clone, &["remote", "prune", "origin"]);

    // The premise, asserted rather than assumed: the tag really is the only ref
    // left in the clone reaching that commit, and the remote really does have it.
    assert_eq!(
        git(&clone, &["tag", "--points-at", "v1^{commit}"]),
        "v1",
        "the tag is here"
    );
    assert!(
        !git(&clone, &["branch", "-a", "--contains", "v1^{commit}"]).contains("release"),
        "and no branch, local or remote-tracking, is"
    );

    assert_eq!(
        held_against(&clone, &fixture.remote),
        Unsaved::NothingToLose,
        "the bare has this tag at this object, so it is not work in danger"
    );
}

#[test]
fn a_commit_only_an_unpushed_local_tag_reaches_is_unsaved() {
    // devlaunch#487, and the reason it was a data-loss ticket rather than a
    // tidiness one: the blanket `--exclude=refs/tags/*` that answered #485 also
    // gave away this case, where a clone holds a commit that exists nowhere else
    // and still read as nothing to lose — so `dl rm` deleted it without asking and
    // `--prune` without printing.
    //
    // Reached by the ordinary backup-tag habit, which is why it had to be fixed
    // rather than recorded: tag before a rewrite, then move the branch out from
    // under the tag. Nothing in the clone but `refs/tags/backup` reaches the
    // commit, and the bare has never heard of it — which is exactly how the guard
    // now tells this case from #485's.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("an-hour.txt"), "an hour of work\n");
    commit(&clone, "about to be rewritten");
    git(&clone, &["tag", "backup"]);
    git(&clone, &["reset", "-q", "--hard", "origin/feature"]);

    // The premise, asserted rather than assumed: the tag is the only ref left
    // reaching that commit, and it is on no remote.
    assert_eq!(
        git(&clone, &["tag", "--points-at", "backup^{commit}"]),
        "backup"
    );
    assert_eq!(
        git(&clone, &["branch", "-a", "--contains", "backup^{commit}"]),
        ""
    );

    // And the bare, which is what the answer now turns on.
    assert_eq!(
        git(&fixture.remote, &["tag", "--list"]),
        "",
        "the mirror has no tag at all, so this one was typed here"
    );

    // The whole sentence, tag named. "1 unpushed commit(s)" alone tells the reader
    // to push or commit something they have already committed; the tag's name is
    // what turns the refusal into the thing that saves the work.
    assert_eq!(
        would_lose(&held_against(&clone, &fixture.remote)),
        "1 unpushed commit(s), 1 reachable only from local tag(s) (backup)"
    );
}

#[test]
fn a_rewrite_backup_ref_does_not_hide_a_tagged_commit_from_the_attribution() {
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["checkout", "-q", "-b", "feat/x"]);
    write(&clone.join("an-hour.txt"), "an hour of work\n");
    commit(&clone, "about to be rewritten");
    git(&clone, &["tag", "backup"]);
    git(
        &clone,
        &["update-ref", "refs/original/refs/heads/feat/x", "HEAD"],
    );
    git(
        &clone,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--amend",
            "-m",
            "reworded",
        ],
    );

    assert_eq!(
        git(&clone, &["rev-parse", "refs/original/refs/heads/feat/x"]),
        git(&clone, &["rev-parse", "backup^{commit}"])
    );
    assert_eq!(git(&fixture.remote, &["tag", "--list"]), "");

    assert_eq!(
        would_lose(&held_against(&clone, &fixture.remote)),
        "2 unpushed commit(s), 1 reachable only from local tag(s) (backup)"
    );
}

#[test]
fn a_local_tag_the_bare_holds_at_another_object_is_unsaved() {
    // The middle case, and the one a name-only comparison would get wrong: the
    // bare has a tag by this name, so "does the mirror have `v1`" says yes — but it
    // has it at the commit the remote published, and this clone moved it onto a
    // commit rewritten here. What the local `v1` reaches exists nowhere else, and
    // moving a tag is not a way to lose it.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["tag", "v1"]);
    git(&clone, &["push", "-q", "origin", "v1"]);
    write(&clone.join("rewritten.txt"), "an hour of work\n");
    commit(&clone, "rewritten");
    git(&clone, &["tag", "-f", "v1"]);
    git(&clone, &["reset", "-q", "--hard", "origin/feature"]);

    // The premise, asserted rather than assumed: both sides have `v1`, and they
    // disagree about what it names.
    assert_eq!(git(&fixture.remote, &["tag", "--list"]), "v1");
    assert_ne!(
        git(&clone, &["rev-parse", "refs/tags/v1"]),
        git(&fixture.remote, &["rev-parse", "refs/tags/v1"])
    );

    assert_eq!(
        would_lose(&held_against(&clone, &fixture.remote)),
        "1 unpushed commit(s), 1 reachable only from local tag(s) (v1)"
    );
}

#[test]
fn a_local_tag_on_a_commit_the_remote_already_has_is_not_a_loss() {
    // The other direction, and what keeps the fix from becoming #485 again by a
    // narrower route: a tag the bare has not got is *asked about*, not counted. Its
    // commits are on the remote, so there is nothing to lose, and the answer is
    // the same as if the tag were not there.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["tag", "reviewed"]);

    assert_eq!(git(&fixture.remote, &["tag", "--list"]), "");
    assert_eq!(
        held_against(&clone, &fixture.remote),
        Unsaved::NothingToLose
    );
}

#[test]
fn with_no_bare_to_compare_against_every_tag_counts() {
    // Principle 1 of map #444: where a check cannot prove safety, it fails towards
    // keeping. This is #485's own fixture — a released tag the remote carries, on a
    // branch both sides have deleted — asked with no mirror named, and the answer
    // has to be the refusal. A clone kept costs disk; the other direction costs the
    // only copy of somebody's work.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["checkout", "-q", "-b", "release"]);
    write(&clone.join("release.txt"), "shipped\n");
    commit(&clone, "release");
    git(&clone, &["push", "-q", "origin", "release"]);
    git(&clone, &["tag", "v1"]);
    git(&clone, &["push", "-q", "origin", "v1"]);
    git(&clone, &["push", "-q", "origin", ":release"]);
    git(&clone, &["checkout", "-q", "feature"]);
    git(&clone, &["branch", "-qD", "release"]);
    git(&clone, &["remote", "prune", "origin"]);

    assert_eq!(
        held_against(&clone, &fixture.remote),
        Unsaved::NothingToLose,
        "with the mirror named it is #485's answer"
    );
    // And it names the tag, which is what makes a mirror that is merely *behind*
    // diagnosable rather than baffling: a reader who recognises `v1` as a release
    // they pushed learns the cache is stale, where "push or commit it" tells them
    // to do a thing they have already done.
    assert_eq!(
        would_lose(&held(&clone)),
        "1 unpushed commit(s), 1 reachable only from local tag(s) (v1)",
        "and without one, the same clone is kept"
    );
}

#[test]
fn a_bare_that_is_not_a_repository_counts_every_tag_too() {
    // The same fail-towards-keeping, reached by the shape that actually happens:
    // the mirror is named but is gone, half-removed, or was never cloned. A
    // refusal from it is not an empty tag list — that reading would let a deleted
    // cache directory quietly authorise a delete.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["tag", "v1"]);
    git(&clone, &["push", "-q", "origin", "v1"]);
    write(&clone.join("later.txt"), "an hour of work\n");
    commit(&clone, "later");
    git(&clone, &["tag", "backup"]);
    git(&clone, &["reset", "-q", "--hard", "origin/feature"]);

    // Both tags are named, and that is the honest answer rather than a loose one:
    // with no mirror readable, nothing has established which of them the remote
    // has, so both are candidates for the commit nothing else reaches. Narrowing
    // the list would mean claiming knowledge this run does not have.
    assert_eq!(
        would_lose(&held_against(&clone, &fixture.path("no-such-bare"))),
        "1 unpushed commit(s), 1 reachable only from local tag(s) (backup, v1)"
    );
}

#[test]
fn a_branch_commit_is_not_blamed_on_a_tag_that_happens_to_be_there() {
    // The over-claim this could have shipped instead. The clone holds two unpushed
    // commits for two different reasons: one on a branch, which "push it" really
    // does clear, and one only a local tag reaches, which it does not. The sentence
    // has to carry both numbers, because a reader told "2 unpushed commit(s),
    // 2 reachable only from local tag(s)" would go looking for a tag that explains
    // the branch commit and find none.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    git(&clone, &["checkout", "-q", "-b", "wip"]);
    write(&clone.join("branch-work.txt"), "an hour of work\n");
    commit(&clone, "branch work");
    git(&clone, &["checkout", "-q", "feature"]);
    write(&clone.join("rewritten.txt"), "another hour\n");
    commit(&clone, "about to be rewritten");
    git(&clone, &["tag", "backup"]);
    git(&clone, &["reset", "-q", "--hard", "origin/feature"]);

    assert_eq!(
        would_lose(&held_against(&clone, &fixture.remote)),
        "2 unpushed commit(s), 1 reachable only from local tag(s) (backup)"
    );
}

#[test]
fn a_tag_that_reaches_nothing_of_its_own_is_not_named_in_the_refusal() {
    // The other half of the same honesty. A local tag sitting on a commit some
    // branch also holds explains nothing about why this clone is being kept, and
    // naming it would send the reader after the wrong ref. The unpushed commit here
    // is the branch's, and the sentence stays the plain one.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("more.txt"), "more\n");
    commit(&clone, "more");
    git(&clone, &["tag", "sits-on-the-branch"]);

    assert_eq!(
        would_lose(&held_against(&clone, &fixture.remote)),
        "1 unpushed commit(s)",
        "the tag reaches nothing the branch does not"
    );
}

#[test]
fn a_long_list_of_tags_is_cut_short_like_every_other_list() {
    // One truncation rule, shared with the changed-paths list rather than written
    // twice: a stale mirror on a repository that tags releases makes every tag
    // local, and a refusal that dumped four hundred names would be unreadable in
    // exactly the case a person most needs to read it.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    write(&clone.join("an-hour.txt"), "an hour of work\n");
    commit(&clone, "about to be rewritten");
    for n in 0..5 {
        git(&clone, &["tag", &format!("backup-{n}")]);
    }
    git(&clone, &["reset", "-q", "--hard", "origin/feature"]);

    let description = would_lose(&held_against(&clone, &fixture.remote));

    assert!(
        description.starts_with("1 unpushed commit(s), 1 reachable only from local tag(s) ("),
        "{description:?}"
    );
    assert!(
        description.ends_with("(backup-0, backup-1, backup-2, …)"),
        "three names and an ellipsis: {description:?}"
    );
}

#[test]
fn the_attribution_is_given_up_rather_than_the_refusal_when_git_will_not_say() {
    // The one place this module does *not* turn a refusal into `CouldNotTell`, and
    // the reason that is safe: by the time this is asked the loss is established
    // and the clone is being kept either way, so no failed question here can be
    // read as permission. Only the half of the sentence naming the tag is lost.
    // Asserted on the decision itself rather than through a fixture, because the
    // two `git log` calls share an argv prefix and cannot be scripted apart.
    let refused = ScriptedRunner::new().with_script(["git"], Response::failed(128, "fatal: nope"));

    assert_eq!(
        owed_to_tags(
            &Git::new(&refused),
            Path::new("/ws"),
            &["refs/tags/backup".to_owned()]
        ),
        None
    );
}

#[test]
fn no_local_tags_is_no_question_asked() {
    // The cheap path, pinned as a spawn count: a clone whose every tag the mirror
    // vouches for pays nothing for the sentence it does not need.
    let fake = ScriptedRunner::new();

    assert_eq!(owed_to_tags(&Git::new(&fake), Path::new("/ws"), &[]), None);
    assert_eq!(fake.call_count(), 0);
}

#[test]
fn a_clone_with_no_tags_never_asks_the_bare() {
    // The bare is one more spawn per clone in `dl --ls`, and a repository with no
    // tags has nothing to compare, so it is not asked. Scripted rather than
    // arranged with real git, because what this asserts is the argv that was never
    // built.
    let dir = tempfile::tempdir().expect("a temp dir");
    let fake = ScriptedRunner::new().with_script(["git"], Response::stdout(""));
    let bare = dir.path().join("mirror.git");

    let unsaved = holds_unsaved_work(&Git::new(&fake), dir.path(), BareCache::At(&bare));

    assert_eq!(unsaved, Unsaved::NothingToLose);
    let asked = fake.argvs();
    assert!(
        !asked
            .iter()
            .any(|argv| argv.iter().any(|arg| arg.contains("mirror.git"))),
        "the mirror was asked about anyway: {asked:?}"
    );
}

#[test]
fn a_commit_on_a_detached_worktree_head_is_still_unsaved() {
    // The tags come out of the ref set by `--exclude`, which drops the tags and
    // nothing else. This is the ref that would go with them if anybody ever
    // narrowed the question to `--branches` instead: a linked worktree on a
    // detached HEAD is on no branch at all, and `git worktree add --detach` is
    // how an agent gets a second checkout of a clone.
    let fixture = Fixture::new();
    let clone = fixture.clone();
    let linked = fixture.path("detached");
    git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            linked.to_str().expect("utf-8"),
            "HEAD",
        ],
    );
    write(&linked.join("agent.txt"), "an hour of work\n");
    commit(&linked, "agent work");

    // Asked of the clone, not of the worktree the commit was made in: one
    // workspace is what `dl rm` deletes, and the commit is on no branch in it.
    assert_eq!(git(&clone, &["status", "--porcelain"]), "");
    assert_eq!(would_lose(&held(&clone)), "1 unpushed commit(s)");
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

    let state = read_clone(
        &Git::new(&fake),
        &dir.path().join("absent"),
        BareCache::Unknown,
    );

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

    let state = read_clone(&Git::new(&fake), &clone, BareCache::Unknown);

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
        holds_unsaved_work(&Git::new(&clean), dir.path(), BareCache::Unknown),
        Unsaved::NothingToLose
    );
    could_not_tell(&holds_unsaved_work(
        &Git::new(&refused),
        dir.path(),
        BareCache::Unknown,
    ));
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
    // The exact wire format: wf parses this. The verdict is the internal answer
    // now, and this is its one flattening to the wire — a clone with no agent
    // worktrees reads exactly as it always did.
    use crate::flows::agent_worktrees::{Blank, Place, Reason, Verdict};
    assert_eq!(
        Verdict::test_collectable().unsaved_json().to_string(),
        r#"{"nothingToLose":true}"#
    );
    assert_eq!(
        Verdict::test_stands(vec![Reason::Holds {
            at: Place::TheCloneItself,
            losses: Box::new(Losses::one(Loss::Unpushed {
                commits: NonEmpty::one("abc123 more".to_owned()),
                by_tags: None,
            })),
        }])
        .unsaved_json()
        .to_string(),
        r#"{"wouldLose":"1 unpushed commit(s)"}"#
    );
    assert_eq!(
        Verdict::test_stands(vec![Reason::CouldNotProve {
            at: Place::TheCloneItself,
            blank: Blank::GitWouldNotSay(CouldNotTell::GitCouldNotRead {
                clone: PathBuf::from("/c"),
                reason: "git said no".to_owned(),
            }),
        }])
        .unsaved_json()
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
        holds_unsaved_work(&Git::new(&nothing_changed), dir.path(), BareCache::Unknown),
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
            Unsaved::WouldLose(Losses::one(Loss::Unpushed {
                commits: NonEmpty::one("abc".to_owned()),
                by_tags: None,
            })),
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
