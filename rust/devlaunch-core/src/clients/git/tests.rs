//! What argv each verb builds, and how a spawn outcome becomes an answer.
//!
//! Argv is what parity is judged on — the Python sites for git, the shim log for
//! devpod — so every verb has an assertion here naming its whole argv, its
//! working directory, its environment and its bound. Those are the facts a
//! rewrite of a body cannot preserve by accident.
//!
//! The verbs' *sequencing* is not tested here: nothing above this layer exists
//! yet, and a client that never retries and never falls back has no sequence of
//! its own to pin.
//!
//! # The fake runner
//!
//! [`ScriptedRunner`](crate::testing::ScriptedRunner) is the workspace's one fake
//! — `devlaunch-test-support`'s recorder, its argv-prefix response table and a
//! default of quiet success — wrapped in the timing exclusion this crate owns. A
//! smaller copy used to live here, because `devlaunch-test-support` depended back
//! on `devlaunch-core` and a unit-test build therefore saw two different `Runner`
//! traits: the `cfg(test)` core being tested, and the plain one the fake was
//! compiled against. The trait moved down to the `devlaunch-runner` leaf crate,
//! so the copy went with it and `clients::devpod`, `clients::gh` and
//! `clients::ssh` share the one fake. `domain::workspace_state`'s tests, which
//! reached in here for the local copy so one git spawn could fail without
//! touching this process's PATH, take it from `crate::testing` too — so this
//! module is private again.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use crate::runner::{EnvBase, Exit, OsFailure};
use crate::testing::ScriptedRunner;
use devlaunch_test_support::{Call, Response};

// ----------------------------------------------------------------- helpers

/// The whole argv of the one recorded call.
fn argv(fake: &ScriptedRunner) -> Vec<String> {
    fake.only_call().argv()
}

fn cwd(fake: &ScriptedRunner) -> Option<PathBuf> {
    fake.only_call().invocation().cwd.clone()
}

fn timeout(fake: &ScriptedRunner) -> Option<Duration> {
    fake.only_call()
        .spec()
        .expect("git is never detached")
        .timeout
}

fn env_entries(fake: &ScriptedRunner) -> Vec<(String, String)> {
    fake.only_call()
        .invocation()
        .env
        .entries
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn env_base(fake: &ScriptedRunner) -> EnvBase {
    fake.only_call().invocation().env.base
}

/// A directory to be about. Canonicalized, because the pinned family resolves
/// the path it is given and the assertion has to name the same string.
fn a_clone() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical");
    (dir, root)
}

fn strs(argv: &[String]) -> Vec<&str> {
    argv.iter().map(String::as_str).collect()
}

const C_LOCALE: [(&str, &str); 2] = [("LANGUAGE", "C"), ("LC_ALL", "C")];

fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

// ------------------------------------------------------- the pinned family

#[test]
fn every_pinned_verb_names_its_repository_twice_and_keeps_the_clone_as_cwd() {
    // devlaunch#171: --git-dir switches discovery off, --work-tree stops
    // core.worktree pointing the answer at another directory, and the cwd is
    // what keeps `git status` printing paths relative to the clone root.
    let (dir, root) = a_clone();
    let fake = ScriptedRunner::new();
    let git = Git::new(&fake);

    let verbs: [&dyn Fn() -> GitAnswer<String>; 3] = [
        &|| git.head_branch(dir.path()),
        &|| git.status_porcelain(dir.path()),
        &|| git.unpushed_commits(dir.path(), &[]),
    ];
    for verb in verbs {
        fake.forget_calls();
        verb();
        let argv = argv(&fake);
        assert_eq!(argv[0], "git");
        assert_eq!(
            argv[1],
            format!("--git-dir={}", root.join(".git").display())
        );
        assert_eq!(argv[2], format!("--work-tree={}", root.display()));
        assert_eq!(cwd(&fake).as_deref(), Some(dir.path()));
        assert_eq!(timeout(&fake), Some(Duration::from_secs(30)));
    }
}

#[test]
fn the_pinned_verbs_ask_exactly_what_python_asked() {
    let (dir, _root) = a_clone();
    let fake = ScriptedRunner::new();
    let git = Git::new(&fake);

    git.head_branch(dir.path());
    assert_eq!(
        strs(&argv(&fake))[3..],
        ["rev-parse", "--abbrev-ref", "HEAD"]
    );

    fake.forget_calls();
    git.status_porcelain(dir.path());
    assert_eq!(strs(&argv(&fake))[3..], ["status", "--porcelain"]);

    fake.forget_calls();
    git.unpushed_commits(dir.path(), &[]);
    // Order is load-bearing twice over. `--not` flips every ref after it, so
    // `--all` comes first: `log --oneline --not --remotes --all` is silently
    // always empty, which would report every clone as safe to delete. And
    // `--exclude` binds to the *next* ref-set option, so it has to sit
    // immediately before `--all` to take the tags out of it (#485), and
    // filter-branch's `refs/original` backups with them.
    assert_eq!(
        strs(&argv(&fake))[3..],
        [
            "log",
            "--oneline",
            "--no-color",
            "--exclude=refs/tags/*",
            "--exclude=refs/original/*",
            "--all",
            "--not",
            "--remotes"
        ]
    );

    // And a local tag goes back in by name, between the ref set it was excluded
    // from and the `--not` that would flip it (#487). Full refnames, so neither
    // git nor a branch of the same name can read them as anything else.
    fake.forget_calls();
    git.unpushed_commits(dir.path(), &["refs/tags/backup".to_owned()]);
    assert_eq!(
        strs(&argv(&fake))[3..],
        [
            "log",
            "--oneline",
            "--no-color",
            "--exclude=refs/tags/*",
            "--exclude=refs/original/*",
            "--all",
            "refs/tags/backup",
            "--not",
            "--remotes"
        ]
    );
}

#[test]
fn the_per_branch_unpushed_query_asks_for_its_hashes_without_colour() {
    let (dir, _root) = a_clone();
    let fake = ScriptedRunner::new();

    Git::new(&fake).unpushed_commits_from(dir.path(), "refs/heads/feature");

    assert_eq!(
        strs(&argv(&fake))[3..],
        [
            "log",
            "--oneline",
            "--no-color",
            "refs/heads/feature",
            "--not",
            "--remotes"
        ]
    );
}

#[test]
fn the_attribution_query_is_the_unpushed_one_with_the_sides_swapped() {
    // The tags are the whole positive set and everything else is subtracted, so a
    // commit it returns is on no remote, no branch, no worktree HEAD and no stash.
    // `--exclude` still binds to the `--all` that follows it, which is why that
    // pair stays adjacent on the negative side rather than being split up.
    let (dir, _root) = a_clone();
    let fake = ScriptedRunner::new();
    let git = Git::new(&fake);

    git.commits_only_tags_reach(dir.path(), &["refs/tags/backup".to_owned()]);

    assert_eq!(
        strs(&argv(&fake))[3..],
        [
            "log",
            "--oneline",
            "--no-color",
            "refs/tags/backup",
            "--not",
            "--remotes",
            "--exclude=refs/tags/*",
            "--exclude=refs/original/*",
            "--all"
        ]
    );
}

#[test]
fn the_two_tag_queries_ask_the_same_question_of_each_side() {
    // The clone's tags and the bare's are compared to each other, so the two
    // spellings have to be one: a format that drifted would read as "every tag is
    // local", which is safe but permanently noisy (#485's shape).
    let (dir, root) = a_clone();
    let fake = ScriptedRunner::new();
    let git = Git::new(&fake);

    git.tags_in_clone(dir.path());
    assert_eq!(
        strs(&argv(&fake))[1..],
        [
            format!("--git-dir={}", root.join(".git").display()).as_str(),
            format!("--work-tree={}", root.display()).as_str(),
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            "refs/tags/",
        ]
    );

    // The bare is named by `--git-dir` alone: it has no work tree, and no cwd is
    // set, so a directory that is not there refuses rather than letting git
    // discover whatever repository dl happens to be standing in.
    fake.forget_calls();
    git.tags_in_bare(Path::new("/cache/.bare"));
    assert_eq!(
        strs(&argv(&fake))[1..],
        [
            "--git-dir=/cache/.bare",
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            "refs/tags/",
        ]
    );
    assert_eq!(cwd(&fake), None);
    assert_eq!(timeout(&fake), Some(Duration::from_secs(30)));
}

#[test]
fn a_tag_is_its_object_as_well_as_its_name() {
    // `vouched_for` in the delete guard is `fetched.iter().any(|cached| cached ==
    // tag)`, so this equality *is* the safety rule: derived on the name alone it
    // would degrade to "the mirror has heard of this name", which silently excuses
    // a tag the clone moved onto a commit that exists nowhere else. That failure
    // has no test of its own that could catch it, because every fixture where the
    // two agree passes either way. Hence one here, on the equality itself.
    let same = TagRef {
        name: "refs/tags/v1".to_owned(),
        object: "aaa".to_owned(),
    };
    let moved = TagRef {
        name: "refs/tags/v1".to_owned(),
        object: "bbb".to_owned(),
    };
    let renamed = TagRef {
        name: "refs/tags/v2".to_owned(),
        object: "aaa".to_owned(),
    };

    assert_eq!(same, same.clone());
    assert_ne!(
        same, moved,
        "the same name at another object is another tag"
    );
    assert_ne!(
        same, renamed,
        "and the same object under another name is too"
    );
}

#[test]
fn a_tag_listing_is_read_as_the_pairs_it_is() {
    let fake = ScriptedRunner::new().with_script(
        ["git"],
        Response::stdout(concat!(
            "aaa refs/tags/v1\n",
            "bbb refs/tags/release/2\n",
            "\n",
            "not-a-tag-line\n",
            "ccc refs/heads/main\n",
        )),
    );
    let git = Git::new(&fake);

    let GitAnswer::Said(tags) = git.tags_in_bare(Path::new("/cache/.bare")) else {
        panic!("git answered");
    };

    // The two tag lines, and nothing else: a blank line, a line in another shape
    // and a ref that is not a tag are all dropped rather than guessed at.
    assert_eq!(
        tags,
        [
            TagRef {
                name: "refs/tags/v1".to_owned(),
                object: "aaa".to_owned(),
            },
            TagRef {
                name: "refs/tags/release/2".to_owned(),
                object: "bbb".to_owned(),
            },
        ]
    );
}

#[test]
fn a_pinned_answer_keeps_its_leading_status_column() {
    // The ` M pixi.lock` regression: a full trim ate the status column and the
    // path was then reported one character short.
    let (dir, _root) = a_clone();
    let fake =
        ScriptedRunner::new().with_script(["git"], Response::stdout(" M pixi.lock\n?? notes.md\n"));

    let answer = Git::new(&fake).status_porcelain(dir.path());

    assert_eq!(
        answer,
        GitAnswer::Said(" M pixi.lock\n?? notes.md".to_owned())
    );
}

#[test]
fn a_pinned_answer_trims_every_trailing_newline_and_no_other_whitespace() {
    let (dir, _root) = a_clone();
    let fake = ScriptedRunner::new().with_script(["git"], Response::stdout("feature\n\n"));

    assert_eq!(
        Git::new(&fake).head_branch(dir.path()),
        GitAnswer::Said("feature".to_owned())
    );
}

#[test]
fn an_empty_pinned_answer_is_an_answer() {
    // A clean tree and a refused status are different facts; `""` is the first.
    let (dir, _root) = a_clone();
    let fake = ScriptedRunner::new().with_script(["git"], Response::stdout(""));

    let answer = Git::new(&fake).status_porcelain(dir.path());

    assert!(answer.is_said());
    assert_eq!(answer, GitAnswer::Said(String::new()));
}

#[test]
fn a_silent_pinned_refusal_is_named_by_the_whole_argument_list() {
    // workspace_state._git's fallback spells out what was asked, where every
    // other site names the subcommand alone.
    let (dir, _root) = a_clone();
    let fake = ScriptedRunner::new().with_script(["git"], Response::exited(128));

    let answer = Git::new(&fake).status_porcelain(dir.path());

    let refused = answer.refusal().expect("refused");
    assert_eq!(refused.reason(), "git status --porcelain exited 128");
    assert_eq!(refused.how, Failure::Exited(Exit::Code(128)));
}

// ------------------------------------------------------------- refusals

#[test]
fn git_s_own_words_are_what_a_refusal_carries() {
    let fake = ScriptedRunner::new().with_script(
        ["git"],
        Response::failed(128, "fatal: not a git repository\n"),
    );

    let answer = Git::new(&fake).fetch_ref(Path::new("/cache/.bare"), "feature");

    assert_eq!(
        answer.refusal().map(GitRefused::reason),
        Some("fatal: not a git repository")
    );
}

#[test]
fn a_silent_failure_is_named_by_its_verb_and_its_status() {
    let fake = ScriptedRunner::new().with_script(["git"], Response::exited(1));

    let answer = Git::new(&fake).push_branch(Path::new("/cache/.bare"), "origin", "feature", None);

    assert_eq!(
        answer.refusal().map(GitRefused::reason),
        Some("git push exited 1")
    );
}

#[test]
fn a_signal_is_spelled_the_way_python_spells_a_returncode() {
    // `subprocess` reports a child killed by SIGTERM as -15, and this text is
    // compared as text.
    let fake = ScriptedRunner::new().with_script(["git"], Response::signalled(15));

    let answer = Git::new(&fake).clone_bare("url", Path::new("/cache/.bare"));

    let refused = answer.refusal().expect("refused");
    assert_eq!(refused.reason(), "git clone exited -15");
    assert_eq!(refused.how, Failure::Exited(Exit::Signal(15)));
}

#[test]
fn a_git_that_is_not_installed_is_its_own_refusal() {
    // Not an exit status: a caller that branched on the status would carry on as
    // though git had answered.
    let fake = ScriptedRunner::new().with_script(["git"], Response::ProgramNotFound);

    let answer = Git::new(&fake).status_porcelain(Path::new("/nowhere"));

    let refused = answer.refusal().expect("refused");
    assert_eq!(refused.how, Failure::GitNotInstalled);
    assert!(!refused.reason().is_empty());
}

#[test]
fn a_bound_that_elapsed_says_what_the_bound_was() {
    let (dir, _root) = a_clone();
    let fake = ScriptedRunner::new().with_script(["git"], Response::TimedOut);

    let answer = Git::new(&fake).status_porcelain(dir.path());

    let refused = answer.refusal().expect("refused");
    assert_eq!(refused.how, Failure::TimedOut);
    assert_eq!(
        refused.reason(),
        "git status --porcelain timed out after 30s"
    );
}

#[test]
fn an_os_refusal_carries_the_os_s_own_words() {
    let failure = OsFailure {
        kind: std::io::ErrorKind::PermissionDenied,
        errno: Some(13),
    };
    let fake = ScriptedRunner::new().with_script(["git"], Response::NotStarted(failure));

    let answer = Git::new(&fake).status_porcelain(Path::new("/locked/ws"));

    let refused = answer.refusal().expect("refused");
    assert_eq!(refused.how, Failure::NotStarted(failure));
    assert!(
        refused.reason().contains("Permission denied"),
        "the OS said it: {:?}",
        refused.reason()
    );
}

#[test]
fn a_refusal_never_has_nothing_to_say() {
    // git_errors.py's whole point: interpolated raw, an absent stderr reads
    // "…: " with nothing after the colon, which tells the reader only that
    // something went wrong.
    for reply in [
        Response::exited(1),
        Response::failed(1, "   \n"),
        Response::TimedOut,
        Response::ProgramNotFound,
        Response::NotStarted(OsFailure {
            kind: std::io::ErrorKind::Other,
            errno: None,
        }),
    ] {
        let fake = ScriptedRunner::new().with_script(["git"], reply.clone());
        let answer = Git::new(&fake).checkout(Path::new("/ws"), "feature");
        let refused = answer.refusal().expect("refused");
        assert!(!refused.reason().is_empty(), "{reply:?}");
    }
}

// ----------------------------------------------- reading git's own words

/// How `git clone --bare` read a refusal, given what git wrote to stderr.
///
/// Through the verb rather than through the reader directly: which reader is
/// wired to which verb is half of what these assertions are for.
fn cloning_read(stderr: &str) -> Option<Failure> {
    let fake = ScriptedRunner::new().with_script(["git"], Response::failed(128, stderr));
    Git::new(&fake)
        .clone_bare("git@github.com:o/r.git", Path::new("/cache/o/r/.bare"))
        .refusal()
        .map(GitRefused::how)
}

#[test]
fn a_branch_that_is_already_there_is_read_where_git_says_so() {
    // Both wordings git has used: `A branch named '%s' already exists.` up to
    // v2.34.0 (`branch.c:208`), lowercase and unstopped from v2.35.0
    // (`branch.c:307`). The tail of the sentence carries both.
    for said in [
        "fatal: a branch named 'feature' already exists\n",
        "fatal: A branch named 'feature' already exists.\n",
    ] {
        let fake = ScriptedRunner::new().with_script(["git"], Response::failed(128, said));

        let answer = Git::new(&fake).create_branch(Path::new("/cache/o/r"), "feature", "main");

        assert_eq!(
            answer.refusal().map(GitRefused::how),
            Some(Failure::BranchAlreadyExists),
            "git said {said:?}"
        );
    }
}

#[test]
fn a_ref_namespace_collision_is_not_a_branch_that_is_already_there() {
    // git's other `git branch` refusal, recorded from v2.51.1: creating `feat/x`
    // where `feat` is already a branch. It says `exists`, it is not the branch
    // being there, and its caller must not swallow it.
    let said = "fatal: cannot lock ref 'refs/heads/feat/x': 'refs/heads/feat' exists; \
                cannot create 'refs/heads/feat/x'\n";
    let fake = ScriptedRunner::new().with_script(["git"], Response::failed(128, said));

    let answer = Git::new(&fake).create_branch(Path::new("/cache/o/r"), "feat/x", "main");

    assert_eq!(
        answer.refusal().map(GitRefused::how),
        Some(Failure::Exited(Exit::Code(128)))
    );
}

#[test]
fn a_ref_the_remote_has_not_got_is_read_whatever_case_git_used() {
    // Up to v2.20.0 git wrote this one through a bare `die()` — `Couldn't find
    // remote ref %s` (`remote.c:1785`), neither lowercase nor translatable, so the
    // pinned C locale does not reach it. From v2.21.0 it is lowercase and goes
    // through `die(_())` (`remote.c:1840`).
    for said in [
        "fatal: couldn't find remote ref refs/heads/nosuch\n",
        "fatal: Couldn't find remote ref refs/heads/nosuch\n",
    ] {
        let fake = ScriptedRunner::new().with_script(["git"], Response::failed(128, said));

        let answer = Git::new(&fake).fetch_ref(Path::new("/cache/o/r/.bare"), "nosuch");

        assert_eq!(
            answer.refusal().map(GitRefused::how),
            Some(Failure::RefMissingOnRemote),
            "git said {said:?}"
        );
    }
}

#[test]
fn the_hosts_not_found_wordings_are_told_from_a_clone_s_other_refusals() {
    // The three hosts' own wordings, and git's own line for an HTTP 404.
    for said in [
        "ERROR: Repository not found.",
        "remote: Repository not found.\nfatal: repository 'https://x/y.git' not found",
        "GitLab: The project you were looking for could not be found or you don't have \
         permission to view it.",
        "conq: repository does not exist.",
        // Recorded from Codeberg (Forgejo) with git 2.51.1: its 404 body carries
        // none of the three phrases above, so git's own line is all there is.
        "Cloning into bare repository '/cache/acme/widgets'...\nremote: Not found.\n\
         fatal: repository 'https://codeberg.org/acme/widgets.git/' not found",
    ] {
        assert_eq!(
            cloning_read(said),
            Some(Failure::RepositoryNotFound),
            "a host said {said:?}"
        );
    }
}

#[test]
fn a_clone_that_failed_some_other_way_is_left_as_an_exit() {
    // The near misses. The last line of git's stock ssh advice — "and the
    // repository exists" — rides along with *every* ssh failure, refused keys
    // included, and is one word from GitHub's wording. git's own complaint about a
    // missing local directory is not a host's answer at all. `could not be found`
    // on its own is generic English rather than anything GitLab said. And git has
    // other `'%s' not found` lines that must not read as the repository's.
    for said in [
        "git@github.com:o/r.git: Permission denied (publickey).\nfatal: Could not read from \
         remote repository.\n\nPlease make sure you have the correct access rights\nand the \
         repository exists.",
        "ssh: Could not resolve hostname github.com: Temporary failure in name resolution",
        "fatal: repository '/home/someone/not-there' does not exist",
        "error: object file .git/objects/ab/cdef could not be found",
        "Cloning into bare repository '/cache/acme/widgets'...\nfatal: branch 'main' not found",
    ] {
        assert_eq!(
            cloning_read(said),
            Some(Failure::Exited(Exit::Code(128))),
            "git said {said:?}"
        );
    }
}

#[test]
fn a_phrase_belongs_to_the_verb_that_says_it() {
    // `git clone` says "already exists" about its *destination directory*, and
    // `git branch` says it about the branch. Naming the reader at the call site is
    // what keeps the two apart: the clone lands on an exit, not on a branch.
    assert_eq!(
        cloning_read(
            "fatal: destination path 'widgets' already exists and is not an empty \
                      directory.\n"
        ),
        Some(Failure::Exited(Exit::Code(128)))
    );
}

// ------------------------------------------------------------ the cache

#[test]
fn cloning_the_cache_is_bare_and_runs_nowhere_in_particular() {
    let fake = ScriptedRunner::new();

    Git::new(&fake).clone_bare("git@github.com:o/r.git", Path::new("/cache/o/r/.bare"));

    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "clone",
            "--bare",
            "git@github.com:o/r.git",
            "/cache/o/r/.bare"
        ]
    );
    assert_eq!(cwd(&fake), None, "the destination is absolute");
    assert_eq!(timeout(&fake), None);
}

#[test]
fn the_broad_sweep_forces_every_head_and_tag_refspec_and_prunes() {
    // The tags refspec is spelled out rather than left to `--tags`, which is the
    // same refspec unforced, and `--prune` prunes per refspec. Unforced, a tag the
    // remote retracted was never pruned and a tag it moved refused the whole
    // fetch, permanently.
    let fake = ScriptedRunner::new();

    Git::new(&fake).fetch_all(Path::new("/cache/o/r/.bare"), None);

    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "fetch",
            "origin",
            "+refs/heads/*:refs/heads/*",
            "+refs/tags/*:refs/tags/*",
            "--prune"
        ]
    );
    assert_eq!(cwd(&fake).as_deref(), Some(Path::new("/cache/o/r/.bare")));
    assert_eq!(
        timeout(&fake),
        None,
        "a watched launch waits as long as it takes"
    );
}

#[test]
fn the_background_sweep_s_bound_reaches_the_spawn() {
    // A detached fetch that never returns is a repository wedged until reboot.
    let fake = ScriptedRunner::new();

    Git::new(&fake).fetch_all(Path::new("/cache/o/r/.bare"), Some(Duration::from_secs(60)));

    assert_eq!(timeout(&fake), Some(Duration::from_secs(60)));
}

#[test]
fn the_guard_s_fetch_is_pinned_bounded_and_cannot_prompt() {
    // devlaunch#638. Pinned, because a fetch into an ancestor repository is a
    // write to somebody's dotfiles. `--no-prune`, because pruning a merged branch
    // puts its commits back into the unpushed count and a host's `fetch.prune`
    // would otherwise do it. `--no-tags`, because a moved tag fails the whole
    // fetch. The refspec on the command line with an empty `--refmap=`, so the
    // clone's own `remote.origin.fetch` cannot aim the fetch at local branches.
    // The bound is the caller's, and no prompt, because a prompt under a bound
    // eats it.
    let fake = ScriptedRunner::new();

    Git::new(&fake).fetch_origin(Path::new("/ws"), Duration::from_secs(30));

    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "--git-dir=/ws/.git",
            "--work-tree=/ws",
            "fetch",
            "--no-tags",
            "--no-prune",
            "--refmap=",
            "origin",
            "+refs/heads/*:refs/remotes/origin/*"
        ]
    );
    assert_eq!(cwd(&fake).as_deref(), Some(Path::new("/ws")));
    assert_eq!(timeout(&fake), Some(Duration::from_secs(30)));
    assert!(
        env_entries(&fake).contains(&("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned())),
        "{:?}",
        env_entries(&fake)
    );
}

#[test]
fn packing_collapses_every_loose_ref_in_the_bare_under_a_bound() {
    // `--all` and not the default, which packs tags alone and would leave every
    // head the sweep just fetched sitting loose. The bound is there because this
    // touches no network: thirty seconds of `pack-refs` is a stuck filesystem, and
    // the caller is a detached child nobody is watching.
    let fake = ScriptedRunner::new();

    Git::new(&fake).pack_refs(Path::new("/cache/o/r/.bare"));

    assert_eq!(strs(&argv(&fake)), ["git", "pack-refs", "--all"]);
    assert_eq!(cwd(&fake).as_deref(), Some(Path::new("/cache/o/r/.bare")));
    assert_eq!(timeout(&fake), Some(Duration::from_secs(30)));
}

#[test]
fn fetching_one_ref_moves_exactly_that_ref_in_the_c_locale() {
    let fake = ScriptedRunner::new();

    Git::new(&fake).fetch_ref(Path::new("/cache/o/r/.bare"), "release/1.0");

    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "fetch",
            "origin",
            "+refs/heads/release/1.0:refs/heads/release/1.0"
        ]
    );
    // The caller classifies the failure from git's stderr text, which git
    // translates; LANGUAGE is pinned too because under gettext it outranks a
    // non-C LC_ALL.
    assert_eq!(env_entries(&fake), pairs(&C_LOCALE));
    assert_eq!(
        env_base(&fake),
        EnvBase::Parent,
        "layered on the environment, not substituted for it"
    );
}

#[test]
fn a_symbolic_ref_is_asked_for_by_name_and_comes_back_trimmed() {
    let fake = ScriptedRunner::new().with_script(["git"], Response::stdout("refs/heads/main\n"));

    let answer = Git::new(&fake).symbolic_ref(Path::new("/cache/o/r/.bare"), "HEAD");

    assert_eq!(strs(&argv(&fake)), ["git", "symbolic-ref", "HEAD"]);
    assert_eq!(answer, GitAnswer::Said("refs/heads/main".to_owned()));
}

#[test]
fn the_remote_branch_listing_is_left_as_text_for_its_caller_to_search() {
    let fake = ScriptedRunner::new().with_script(
        ["git"],
        Response::stdout("  origin/HEAD -> origin/main\n  origin/main\n"),
    );

    let answer = Git::new(&fake).remote_branch_listing(Path::new("/cache/o/r/.bare"));

    assert_eq!(strs(&argv(&fake)), ["git", "branch", "-r"]);
    assert_eq!(
        answer.said().as_deref(),
        Some("origin/HEAD -> origin/main\n  origin/main")
    );
}

#[test]
fn asking_a_remote_for_its_head_is_bounded_at_ten_seconds() {
    let fake = ScriptedRunner::new();

    Git::new(&fake).ls_remote_symref_head("git@github.com:o/r.git");

    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "ls-remote",
            "--symref",
            "git@github.com:o/r.git",
            "HEAD"
        ]
    );
    assert_eq!(cwd(&fake), None);
    assert_eq!(timeout(&fake), Some(Duration::from_secs(10)));
}

#[test]
fn the_local_branches_of_the_cache_are_read_off_disk_under_a_short_bound() {
    let fake = ScriptedRunner::new().with_script(["git"], Response::stdout("main\nfeature\n\n"));

    let answer = Git::new(&fake).local_branches(Path::new("/cache/o/r/.bare"));

    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/"
        ]
    );
    assert_eq!(timeout(&fake), Some(Duration::from_secs(2)));
    assert_eq!(
        answer,
        GitAnswer::Said(vec!["main".to_owned(), "feature".to_owned()])
    );
}

// ----------------------------------------------------------- branches

#[test]
fn creating_a_branch_names_its_start_point_in_the_c_locale() {
    let fake = ScriptedRunner::new();

    Git::new(&fake).create_branch(Path::new("/cache/o/r/.bare"), "feature", "main");

    assert_eq!(strs(&argv(&fake)), ["git", "branch", "feature", "main"]);
    // The caller swallows this failure when the reason says "already exists".
    assert_eq!(env_entries(&fake), pairs(&C_LOCALE));
}

#[test]
fn tracking_is_set_with_one_flag_carrying_the_remote_and_the_branch() {
    let fake = ScriptedRunner::new();

    Git::new(&fake).set_upstream(Path::new("/cache/o/r/.bare"), "feature", "origin");

    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "branch",
            "--set-upstream-to=origin/feature",
            "feature"
        ]
    );
    assert!(env_entries(&fake).is_empty());
}

#[test]
fn a_ref_is_verified_by_its_full_name() {
    let fake = ScriptedRunner::new();
    let git = Git::new(&fake);

    git.verify_ref(Path::new("/cache/o/r/.bare"), &refs_heads("release/1.0"));
    assert_eq!(
        strs(&argv(&fake)),
        ["git", "show-ref", "--verify", "refs/heads/release/1.0"]
    );

    fake.forget_calls();
    git.verify_ref(Path::new("/ws"), &refs_remotes("origin", "feature"));
    assert_eq!(
        strs(&argv(&fake)),
        ["git", "show-ref", "--verify", "refs/remotes/origin/feature"]
    );
}

#[test]
fn a_verify_that_exits_non_zero_is_a_refusal_rather_than_a_false() {
    // show-ref --verify exits non-zero both for an absent ref and for a
    // directory that is not a repository; collapsing those to one bool is the
    // caller's decision to keep or to reconsider, not this layer's to make.
    let fake = ScriptedRunner::new().with_script(["git"], Response::exited(1));

    let answer = Git::new(&fake).verify_ref(Path::new("/ws"), &refs_heads("gone"));

    assert!(!answer.is_said());
}

#[test]
fn remote_heads_can_be_asked_about_one_branch_or_all_of_them() {
    let listing = "abc123\trefs/heads/main\ndef456\trefs/heads/feature\n";
    let fake = ScriptedRunner::new().with_script(["git"], Response::stdout(listing));
    let git = Git::new(&fake);

    let all = git.ls_remote_heads(Path::new("/cache/o/r/.bare"), "origin", None);
    assert_eq!(
        strs(&argv(&fake)),
        ["git", "ls-remote", "--heads", "origin"]
    );
    // A network round trip on the refresh path: bounded like every remote
    // ls-remote dl.py issued (timeout=5), never left to hang (R9).
    assert_eq!(timeout(&fake), Some(Duration::from_secs(5)));
    assert_eq!(
        all,
        GitAnswer::Said(vec!["main".to_owned(), "feature".to_owned()])
    );

    fake.forget_calls();
    git.ls_remote_heads(Path::new("/cache/o/r/.bare"), "origin", Some("feature"));
    assert_eq!(
        strs(&argv(&fake)),
        ["git", "ls-remote", "--heads", "origin", "feature"]
    );
    assert_eq!(cwd(&fake).as_deref(), Some(Path::new("/cache/o/r/.bare")));
}

#[test]
fn a_push_sets_upstream_and_names_no_key_unless_it_is_given_one() {
    let fake = ScriptedRunner::new();

    Git::new(&fake).push_branch(Path::new("/cache/o/r/.bare"), "origin", "feature", None);

    assert_eq!(
        strs(&argv(&fake)),
        ["git", "push", "-u", "origin", "feature"]
    );
    assert!(
        env_entries(&fake).is_empty(),
        "no key, no GIT_SSH_COMMAND at all"
    );
}

#[test]
fn a_named_key_is_quoted_because_git_ssh_command_is_a_shell_string() {
    // A key under a directory with a space in it would otherwise be split, and
    // ssh would get a truncated -i and the remainder as a hostname.
    let fake = ScriptedRunner::new();

    Git::new(&fake).push_branch(
        Path::new("/cache"),
        "origin",
        "feature",
        Some(Path::new("/home/a b/.ssh/id_ed25519")),
    );

    assert_eq!(
        env_entries(&fake),
        pairs(&[(
            "GIT_SSH_COMMAND",
            "ssh -i '/home/a b/.ssh/id_ed25519' -o IdentitiesOnly=yes"
        )])
    );
    assert_eq!(
        env_base(&fake),
        EnvBase::Parent,
        "a push with no PATH cannot find the ssh it was told to run"
    );
}

// -------------------------------------------------------- workspace clones

#[test]
fn a_workspace_is_cloned_from_the_cache_by_plain_path_with_smudge_off() {
    // Plain paths are what makes git hardlink the pack files; a `file://` source
    // or a --no-hardlinks would lose that silently.
    let fake = ScriptedRunner::new();

    Git::new(&fake).clone_from_cache(Path::new("/cache/o/r/.bare"), Path::new("/cache/o/r/ws-1"));

    assert_eq!(
        strs(&argv(&fake)),
        ["git", "clone", "/cache/o/r/.bare", "/cache/o/r/ws-1"]
    );
    assert_eq!(env_entries(&fake), pairs(&[("GIT_LFS_SKIP_SMUDGE", "1")]));
    assert_eq!(cwd(&fake), None);
}

#[test]
fn the_clone_s_remote_is_pointed_at_the_forge() {
    let fake = ScriptedRunner::new();

    Git::new(&fake).set_remote_url(Path::new("/ws"), "origin", "git@github.com:o/r.git");

    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "remote",
            "set-url",
            "origin",
            "git@github.com:o/r.git"
        ]
    );
    assert_eq!(cwd(&fake).as_deref(), Some(Path::new("/ws")));
}

#[test]
fn an_existing_workspace_is_checked_out_plainly_and_a_new_one_is_reset() {
    let fake = ScriptedRunner::new();
    let git = Git::new(&fake);

    git.checkout(Path::new("/ws"), "feature");
    assert_eq!(strs(&argv(&fake)), ["git", "checkout", "feature"]);

    fake.forget_calls();
    git.checkout_reset(Path::new("/ws"), "feature", "origin/feature");
    assert_eq!(
        strs(&argv(&fake)),
        ["git", "checkout", "-B", "feature", "origin/feature"]
    );
}

#[test]
fn tracked_files_are_the_union_of_head_and_the_index_nul_separated() {
    let fake = ScriptedRunner::new()
        .with_script(["git"], Response::stdout("a.bin\0dir/b with\nnewline\0"));

    let answer = Git::new(&fake).tracked_files(Path::new("/ws"));

    assert_eq!(
        strs(&argv(&fake)),
        ["git", "ls-files", "-z", "--with-tree=HEAD"]
    );
    assert_eq!(
        answer,
        GitAnswer::Said(vec!["a.bin".to_owned(), "dir/b with\nnewline".to_owned()]),
        "-z is what keeps a path with a newline in it whole"
    );
}

// ---------------------------------------------------------------- LFS

#[test]
fn the_lfs_file_list_is_asked_for_by_name_only() {
    let fake = ScriptedRunner::new().with_script(["git"], Response::stdout("big.bin\nother.bin\n"));

    let answer = Git::new(&fake).lfs_tracked_files(Path::new("/ws"));

    assert_eq!(
        strs(&argv(&fake)),
        ["git", "lfs", "ls-files", "--name-only"]
    );
    assert_eq!(
        answer,
        GitAnswer::Said(vec!["big.bin".to_owned(), "other.bin".to_owned()])
    );
}

#[test]
fn filling_the_cache_s_lfs_store_runs_in_the_bare_with_recency_zeroed() {
    let fake = ScriptedRunner::new();

    Git::new(&fake).lfs_fetch_into_cache(Path::new("/cache/o/r/.bare"), "feature");

    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "-c",
            "lfs.fetchrecentrefsdays=0",
            "-c",
            "lfs.fetchrecentcommitsdays=0",
            "lfs",
            "fetch",
            "origin",
            "feature"
        ]
    );
    assert_eq!(
        cwd(&fake).as_deref(),
        Some(Path::new("/cache/o/r/.bare")),
        "cwd is the only thing that decides where the objects land"
    );
}

#[test]
fn the_lfs_verbs_leave_their_output_on_the_user_s_terminal() {
    // A multi-gigabyte fetch has to look like progress rather than a hang.
    let fake = ScriptedRunner::new();
    let git = Git::new(&fake);

    git.lfs_fetch_into_cache(Path::new("/cache/o/r/.bare"), "feature");
    git.lfs_pull_from_cache(Path::new("/ws"), Path::new("/cache/o/r/.bare"));
    git.lfs_pull_origin(Path::new("/ws"));

    for call in fake.calls() {
        assert!(
            matches!(call, Call::Passthrough(_)),
            "captured an LFS transfer: {call:?}"
        );
    }
}

#[test]
fn the_cache_is_named_as_a_file_url_on_the_command_line() {
    // Not a configured remote: `.bare` is not bind-mounted into the container,
    // so a host path persisted into the clone names a directory that is not
    // there. An argument is gone when the command is.
    let fake = ScriptedRunner::new();

    Git::new(&fake).lfs_pull_from_cache(Path::new("/ws"), Path::new("/cache/o/r/.bare"));

    assert_eq!(
        strs(&argv(&fake)),
        ["git", "lfs", "pull", "file:///cache/o/r/.bare"]
    );
    assert_eq!(cwd(&fake).as_deref(), Some(Path::new("/ws")));
}

#[test]
fn the_network_phase_pulls_from_origin() {
    let fake = ScriptedRunner::new();

    Git::new(&fake).lfs_pull_origin(Path::new("/ws"));

    assert_eq!(strs(&argv(&fake)), ["git", "lfs", "pull", "origin"]);
}

#[test]
fn an_uncaptured_verb_that_refuses_reports_what_ran_and_how_it_ended() {
    // There is no stderr to quote, so naming the command and its status is all
    // there is — which is what Python's CalledProcessError message says too.
    let fake = ScriptedRunner::new().with_script(["git"], Response::exited(2));

    let answer = Git::new(&fake).lfs_pull_origin(Path::new("/ws"));

    assert_eq!(
        answer.refusal().map(GitRefused::reason),
        Some("git lfs pull exited 2")
    );
}

#[test]
fn an_uncaptured_verb_that_succeeds_carries_no_output_at_all() {
    let fake = ScriptedRunner::new();

    assert_eq!(
        Git::new(&fake).lfs_pull_origin(Path::new("/ws")),
        GitAnswer::Said(())
    );
}

// ------------------------------------------------------- remotes, outside

#[test]
fn the_origin_url_is_asked_for_with_dash_c_rather_than_a_cwd() {
    let fake =
        ScriptedRunner::new().with_script(["git"], Response::stdout("git@github.com:o/r.git\n"));

    let answer = Git::new(&fake).origin_url_at(Path::new("/projects/mine"));

    assert_eq!(
        strs(&argv(&fake)),
        ["git", "-C", "/projects/mine", "remote", "get-url", "origin"]
    );
    assert_eq!(cwd(&fake), None);
    assert_eq!(answer, GitAnswer::Said("git@github.com:o/r.git".to_owned()));
}

#[test]
fn the_two_ls_remote_spellings_are_kept_apart() {
    // Both exist in Python — `--heads <url>` from nowhere, and `<url> <args…>`
    // with the URL first — and parity is judged on argv.
    let fake = ScriptedRunner::new();
    let git = Git::new(&fake);

    git.ls_remote_heads_of("git@github.com:o/r.git");
    assert_eq!(
        strs(&argv(&fake)),
        ["git", "ls-remote", "--heads", "git@github.com:o/r.git"]
    );
    assert_eq!(timeout(&fake), Some(Duration::from_secs(5)));

    fake.forget_calls();
    git.ls_remote("git@github.com:o/r.git", &["--heads", "feature"]);
    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "ls-remote",
            "git@github.com:o/r.git",
            "--heads",
            "feature"
        ]
    );
    assert_eq!(timeout(&fake), Some(Duration::from_secs(5)));
}

#[test]
fn nothing_here_spawns_more_than_once_per_verb() {
    // A verb that retried or fell back would be a flow, and flows are M4b's.
    let fake = ScriptedRunner::new();
    let git = Git::new(&fake);

    git.clone_bare("url", Path::new("/cache/.bare"));
    git.fetch_all(Path::new("/cache/.bare"), None);
    git.pack_refs(Path::new("/cache/.bare"));
    git.fetch_ref(Path::new("/cache/.bare"), "feature");
    git.symbolic_ref(Path::new("/cache/.bare"), "HEAD");
    git.remote_branch_listing(Path::new("/cache/.bare"));
    git.ls_remote_symref_head("url");
    git.local_branches(Path::new("/cache/.bare"));
    git.create_branch(Path::new("/cache/.bare"), "feature", "main");
    git.set_upstream(Path::new("/cache/.bare"), "feature", "origin");
    git.verify_ref(Path::new("/cache/.bare"), &refs_heads("feature"));
    git.ls_remote_heads(Path::new("/cache/.bare"), "origin", None);
    git.push_branch(Path::new("/cache/.bare"), "origin", "feature", None);
    git.clone_from_cache(Path::new("/cache/.bare"), Path::new("/ws"));
    git.set_remote_url(Path::new("/ws"), "origin", "url");
    git.checkout(Path::new("/ws"), "feature");
    git.checkout_reset(Path::new("/ws"), "feature", "origin/feature");
    git.tracked_files(Path::new("/ws"));
    git.lfs_tracked_files(Path::new("/ws"));
    git.lfs_fetch_into_cache(Path::new("/cache/.bare"), "feature");
    git.lfs_pull_from_cache(Path::new("/ws"), Path::new("/cache/.bare"));
    git.lfs_pull_origin(Path::new("/ws"));
    git.origin_url_at(Path::new("/ws"));
    git.ls_remote_heads_of("url");
    git.ls_remote("url", &[]);
    git.head_branch(Path::new("/ws"));
    git.status_porcelain(Path::new("/ws"));
    git.unpushed_commits(Path::new("/ws"), &[]);
    git.tags_in_clone(Path::new("/ws"));
    git.tags_in_bare(Path::new("/cache/.bare"));
    git.commits_only_tags_reach(Path::new("/ws"), &[]);
    git.fetch_origin(Path::new("/ws"), Duration::from_secs(30));

    assert_eq!(fake.call_count(), 32, "one spawn per verb, 32 verbs");
    assert!(
        fake.calls()
            .iter()
            .all(|call| call.invocation().program == "git")
    );
}

// ------------------------------------------------------------- parsing

#[test]
fn ls_remote_lines_that_are_not_head_refs_are_dropped_rather_than_guessed_at() {
    let output = concat!(
        "abc123\trefs/heads/main\n",
        "def456\trefs/tags/v1\n",
        "no-tab-here\n",
        "ghi789\trefs/heads/release/1.0\n",
        "\n",
    );

    assert_eq!(
        branches_in_ls_remote(output),
        ["main".to_owned(), "release/1.0".to_owned()]
    );
}

#[test]
fn a_symref_answer_names_the_branch_head_points_at() {
    let output = "ref: refs/heads/release/1.0\tHEAD\nabc123\tHEAD\n";

    assert_eq!(
        head_branch_in_symref(output),
        Some("release/1.0".to_owned()),
        "the prefix is stripped, not the last path segment"
    );
}

#[test]
fn a_symref_answer_with_no_ref_line_names_nothing() {
    for output in ["", "abc123\tHEAD\n", "ref: refs/heads/\tHEAD\n"] {
        assert_eq!(head_branch_in_symref(output), None, "{output:?}");
    }
}

#[test]
fn a_left_right_count_reads_heads_own_commits_first() {
    // The order is the contract: `HEAD...<ref>` puts HEAD on the left, so the
    // left column is `ahead`. Swap the two and every report of a stale checkout
    // says the opposite of what happened.
    assert_eq!(
        ahead_behind_in_counts("3\t37\n"),
        Some(AheadBehind {
            ahead: 3,
            behind: 37
        })
    );
    assert_eq!(
        ahead_behind_in_counts("0\t0"),
        Some(AheadBehind::default()),
        "the same commit both sides is an answer, and it is the ordinary one"
    );
}

#[test]
fn a_count_in_a_shape_this_does_not_recognise_is_no_answer() {
    // Not a refusal and not a zero: a reader that guessed here would put an
    // invented number in front of somebody, which is worse than saying nothing.
    for output in ["", "37", "0 37", "a\tb", "-1\t2", "0\t"] {
        assert_eq!(ahead_behind_in_counts(output), None, "{output:?}");
    }
}

#[test]
fn ahead_behind_asks_for_the_symmetric_difference_and_names_the_repository() {
    let (dir, root) = a_clone();
    let fake = ScriptedRunner::new().with_script(["git"], Response::stdout("2\t5\n"));

    let answer = Git::new(&fake).ahead_behind(dir.path(), &refs_remotes("origin", "main"));

    assert_eq!(
        answer,
        GitAnswer::Said(Some(AheadBehind {
            ahead: 2,
            behind: 5
        }))
    );
    let argv = argv(&fake);
    assert_eq!(
        argv[1],
        format!("--git-dir={}", root.join(".git").display()),
        "one of the pinned family: discovery stays off (devlaunch#171)"
    );
    assert_eq!(
        strs(&argv)[3..],
        [
            "rev-list",
            "--left-right",
            "--count",
            "HEAD...refs/remotes/origin/main"
        ],
        "three dots, so a diverged checkout is two counts and not one"
    );
}

#[test]
fn an_empty_listing_parses_to_nothing_rather_than_to_one_empty_name() {
    assert!(branches_in_ls_remote("").is_empty());
    assert!(lines("\n\n").is_empty());
    assert!(nul_separated("\0").is_empty());
}

const LOCAL: &str = "1111111111111111111111111111111111111111";
const COPY: &str = "2222222222222222222222222222222222222222";
const TREE: &str = "3333333333333333333333333333333333333333";

/// A clone where *LOCAL* and *COPY* have the same patch, and the replay of
/// *LOCAL* on *COPY*'s parent answers *replayed*.
fn a_patch_match(root: &Path, replayed: Response, copy_tree: Response) -> ScriptedRunner {
    let pinned = |verb: &[&str]| {
        let mut argv = vec![
            "git".to_owned(),
            format!("--git-dir={}", root.join(".git").display()),
            format!("--work-tree={}", root.display()),
        ];
        argv.extend(verb.iter().map(|arg| (*arg).to_owned()));
        argv
    };
    let patch = "\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n";
    ScriptedRunner::new()
        .with_script(
            pinned(&["rev-list", "--cherry-mark", "--left-only"]),
            Response::stdout(format!("={LOCAL}\n")),
        )
        .with_script(
            pinned(&["rev-list", "--cherry-mark", "--right-only"]),
            Response::stdout(format!("={COPY}\n")),
        )
        .with_script(
            pinned(&["log"]),
            Response::stdout(format!("\0{LOCAL}{patch}\0{COPY}{patch}")),
        )
        .with_script(pinned(&AS_TEXT), replayed)
        .with_script(
            pinned(&["rev-parse", "--git-path"]),
            Response::stdout(format!(
                "{}\nsha1\n",
                root.join(".git/info/attributes").display()
            )),
        )
        .with_script(pinned(&["rev-parse"]), copy_tree)
}

#[test]
fn a_patch_match_is_a_copy_only_when_git_replays_it_as_the_copy() {
    let (dir, root) = a_clone();
    let tree = || Response::stdout(format!("{TREE}\n"));

    let fake = a_patch_match(&root, tree(), tree());
    assert_eq!(
        Git::new(&fake)
            .patches_already_on(
                dir.path(),
                "refs/heads/feature",
                &RemoteRef::of("origin", "feature")
            )
            .said(),
        Some(vec![LOCAL.to_owned()])
    );
    let log = fake
        .calls()
        .iter()
        .map(Call::argv)
        .find(|argv| argv.get(3).map(String::as_str) == Some("log"))
        .expect("the patches are read");
    assert_eq!(
        strs(&log)[3..],
        [
            "log",
            "--no-walk",
            "--no-merges",
            "--root",
            "-p",
            "--binary",
            "--no-renames",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--format=%x00%H",
            LOCAL,
            COPY,
        ],
        "`--root`, or `log.showRoot=false` prints a root commit with no patch"
    );
    let replay = fake
        .calls()
        .into_iter()
        .find(|call| call.argv().get(8).map(String::as_str) == Some("merge-tree"))
        .expect("the pair is replayed");
    assert_eq!(
        replay.invocation().env.entries.get("GIT_ATTR_NOSYSTEM"),
        Some(&"1".to_owned()),
        "a system gitattributes file could name a merge driver that `-c` does not reach"
    );
    let replay = replay.argv();
    assert_eq!(strs(&replay)[3..7], AS_TEXT, "every path merged as text");
    assert_eq!(
        strs(&replay)[7],
        "--attr-source=4b825dc642cb6eb9a060e54bf8d69288fbee4904",
        "the attributes of the empty tree, which is no attributes"
    );
    assert_eq!(
        strs(&replay)[8..],
        [
            "merge-tree",
            "--write-tree",
            &format!("--merge-base={LOCAL}^"),
            &format!("{COPY}^"),
            LOCAL,
        ]
    );

    for (replayed, copy_tree, why) in [
        (Response::exited(1), tree(), "a conflict"),
        (tree(), Response::exited(128), "no tree for the copy"),
        (
            Response::stdout("4444444444444444444444444444444444444444\n"),
            tree(),
            "another tree",
        ),
    ] {
        let fake = a_patch_match(&root, replayed, copy_tree);
        assert_eq!(
            Git::new(&fake)
                .patches_already_on(
                    dir.path(),
                    "refs/heads/feature",
                    &RemoteRef::of("origin", "feature")
                )
                .said(),
            Some(vec![]),
            "{why} clears nothing"
        );
    }
}

#[test]
fn two_patches_pair_when_only_their_blobs_and_line_numbers_differ() {
    let patch = |index: &str, hunk: &str, added: &str| {
        format!(
            "\ndiff --git a/f b/f\nindex {index} 100644\n--- a/f\n+++ b/f\n\
             @@ {hunk} @@ def f():\n     a\n-    b\n+{added}\n"
        )
    };
    assert_eq!(
        normalized(&patch("1111111..2222222", "-1,2 +1,2", "    c")),
        normalized(&patch("3333333..4444444", "-10,2 +12,2", "    c")),
    );
    assert_ne!(
        normalized(&patch("1111111..2222222", "-1,2 +1,2", "    c")),
        normalized(&patch("1111111..2222222", "-1,2 +1,2", "   c")),
        "one space is a different change"
    );
    let last = patch("1111111..2222222", "-1,2 +1,2", "    c");
    assert_eq!(
        normalized(&last),
        normalized(last.trim_end_matches('\n')),
        "the last patch in the output, with its trailing newline trimmed away"
    );
}

#[test]
fn a_merge_with_a_remerge_diff_is_not_one_that_adds_nothing() {
    assert_eq!(
        merges_without_a_diff_in(&format!("\0{LOCAL}\n\0{COPY}\ndiff --git x\n")),
        [LOCAL.to_owned()]
    );
    assert_eq!(
        merges_without_a_diff_in(&format!("\0{TREE}")),
        [TREE.to_owned()],
        "the last merge, with its trailing newline trimmed away"
    );
    assert!(merges_without_a_diff_in("").is_empty());
}

/// A hash in SHA-256's length, which git prints in a repository made with
/// `--object-format=sha256`.
const SHA256: &str = "5555555555555555555555555555555555555555555555555555555555555555";

#[test]
fn a_nul_inside_a_diff_leaves_the_merges_unread() {
    // A `diff` gitattribute makes git print a file that holds a NUL as text,
    // so the NUL between entries can also turn up inside one.
    assert!(
        merges_without_a_diff_in(&format!("\0{LOCAL}\ndiff --git a/f b/f\n+a\0  y\n")).is_empty()
    );
    assert!(merges_without_a_diff_in(&format!("\0{LOCAL}\ndiff --git a/f b/f\n+a\0y")).is_empty());
    assert_eq!(
        merges_without_a_diff_in(&format!("\0{SHA256}\n")),
        [SHA256.to_owned()]
    );
    for hash in [
        "1111111",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        " 1111111111111111111111111111111111111111",
        "g111111111111111111111111111111111111111",
    ] {
        assert!(
            merges_without_a_diff_in(&format!("\0{hash}\n")).is_empty(),
            "{hash:?} is not a full hash"
        );
    }
}

#[test]
fn a_nul_inside_a_patch_leaves_the_patches_unread() {
    let patch = "\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n";
    assert_eq!(
        patches_in(&format!("\0{LOCAL}{patch}\0{SHA256}{patch}"))
            .into_iter()
            .map(|(hash, _)| hash)
            .collect::<Vec<_>>(),
        [LOCAL.to_owned(), SHA256.to_owned()]
    );
    assert!(patches_in(&format!("\0{LOCAL}{patch}+a\0  y\n")).is_empty());
    assert!(patches_in(&format!("\0{LOCAL}{patch}+a\0y")).is_empty());
    assert!(
        patches_in(&format!(
            "\0AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA{patch}"
        ))
        .is_empty()
    );
    assert!(
        patches_in(&format!("{LOCAL}{patch}")).is_empty(),
        "no NUL before the first"
    );
}

#[test]
fn a_branch_with_an_empty_upstream_field_tracks_nothing() {
    let output = concat!(
        "refs/heads/backup\0\n",
        "refs/heads/feature\0refs/remotes/origin/feature\n",
        "refs/heads/other\0refs/heads/feature\n",
        "no-nul-here\n",
    );

    assert_eq!(
        local_branches_in(output),
        [
            LocalBranch {
                name: "refs/heads/backup".to_owned(),
                upstream: None,
            },
            LocalBranch {
                name: "refs/heads/feature".to_owned(),
                upstream: RemoteRef::parse("refs/remotes/origin/feature"),
            },
            LocalBranch {
                name: "refs/heads/other".to_owned(),
                upstream: None,
            },
        ],
        "a local upstream is no remote, and a copy on it is one more local copy"
    );
}

#[test]
fn only_a_remote_tracking_refname_is_a_remote_ref() {
    assert_eq!(
        RemoteRef::parse("refs/remotes/origin/feature").map(|remote| remote.as_str().to_owned()),
        Some("refs/remotes/origin/feature".to_owned())
    );
    assert_eq!(
        RemoteRef::of("origin", "HEAD").as_str(),
        "refs/remotes/origin/HEAD"
    );
    for not_one in ["refs/heads/feature", "origin/feature", "refs/remotes/", ""] {
        assert_eq!(RemoteRef::parse(not_one), None, "{not_one:?}");
    }
}

// ------------------------------------------------------- the pointer sniff

#[test]
fn a_pointer_file_is_recognised_by_its_first_bytes() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let pointer = dir.path().join("big.bin");
    std::fs::write(
        &pointer,
        "version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 1\n",
    )
    .expect("written");

    assert!(is_lfs_pointer(&pointer));
}

#[test]
fn nothing_else_is_a_pointer_and_a_path_that_will_not_open_least_of_all() {
    // Every ordinary workspace has several unopenable paths — a deleted file, a
    // dangling symlink, a submodule's directory. Answering true for them drives
    // an unbounded `git lfs pull origin` on every launch, forever.
    let dir = tempfile::tempdir().expect("a temp dir");
    let real = dir.path().join("real.bin");
    std::fs::write(&real, "not a pointer, just bytes").expect("written");
    let short = dir.path().join("short");
    std::fs::write(&short, "version").expect("written");
    let empty = dir.path().join("empty");
    std::fs::write(&empty, "").expect("written");

    assert!(!is_lfs_pointer(&real));
    assert!(!is_lfs_pointer(&short), "shorter than the prefix");
    assert!(!is_lfs_pointer(&empty));
    assert!(!is_lfs_pointer(&dir.path().join("absent")));
    assert!(!is_lfs_pointer(dir.path()), "a directory is not a pointer");
}

#[test]
fn git_lfs_is_looked_for_on_path_rather_than_forked_for() {
    // The answer gates a fork, so paying a fork to learn it defeats the point.
    // Asserted against a PATH this test hands over rather than one it sets, so
    // it cannot race the other tests in this process.
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = std::ffi::OsString::from(dir.path());

    assert!(!lfs_is_installed_along(&path), "nothing in this PATH");

    let binary = dir.path().join("git-lfs");
    std::fs::write(&binary, "#!/bin/sh\n").expect("written");
    assert!(
        !lfs_is_installed_along(&path),
        "there, but not something that can be executed"
    );

    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(lfs_is_installed_along(&path));
}

#[test]
fn a_symbolic_ref_keeps_a_branch_name_that_has_slashes_in_it() {
    // `split("/")[-1]` turned a default branch of `release/1.0` into `1.0` — a
    // ref the repository does not have, recorded as the one every later
    // operation targets.
    assert_eq!(
        branch_in_symbolic_ref("refs/heads/release/1.0"),
        "release/1.0"
    );
    assert_eq!(
        branch_in_symbolic_ref("refs/remotes/origin/feature/auth"),
        "feature/auth"
    );
    assert_eq!(branch_in_symbolic_ref("refs/heads/main"), "main");
    assert_eq!(
        branch_in_symbolic_ref("refs/tags/v1"),
        "v1",
        "neither namespace: the last segment, where Python left it"
    );
    assert_eq!(branch_in_symbolic_ref("main"), "main");
}

#[test]
fn clean_merge_trees_are_read_from_merge_tree_stdin_output() {
    // A clean merge, a conflict with two paths, and a clean merge again, in
    // the shape `merge-tree --stdin -z --name-only --no-messages` prints.
    let output = "1\0aaa\0\x000\0bbb\0f\0g/h\0\x001\0ccc\0\0";
    assert_eq!(
        clean_merge_trees_in(output, 3),
        Some(vec![Some("aaa".to_owned()), None, Some("ccc".to_owned())])
    );

    // Fewer records than merges, more, a status that is neither, and a
    // record cut short all read as no answer.
    assert_eq!(clean_merge_trees_in(output, 4), None);
    assert_eq!(clean_merge_trees_in(output, 2), None);
    assert_eq!(clean_merge_trees_in("2\0aaa\0\0", 1), None);
    assert_eq!(clean_merge_trees_in("1\0aaa\0", 1), None);
    assert_eq!(clean_merge_trees_in("", 1), None);
}
