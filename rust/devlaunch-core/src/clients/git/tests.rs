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
        &|| git.unpushed_commits(dir.path(), "feature"),
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
    git.unpushed_commits(dir.path(), "feature");
    // Order is load-bearing: `--not` flips every ref after it, so the branch
    // comes first. `log --oneline --not --remotes feature` is silently always
    // empty, which would report every clone as safe to delete.
    assert_eq!(
        strs(&argv(&fake))[3..],
        ["log", "--oneline", "feature", "--not", "--remotes"]
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
fn the_broad_sweep_fetches_every_head_and_tag_and_prunes() {
    let fake = ScriptedRunner::new();

    Git::new(&fake).fetch_all(Path::new("/cache/o/r/.bare"), None);

    assert_eq!(
        strs(&argv(&fake)),
        [
            "git",
            "fetch",
            "origin",
            "+refs/heads/*:refs/heads/*",
            "--tags",
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
    git.unpushed_commits(Path::new("/ws"), "feature");

    assert_eq!(fake.call_count(), 27, "one spawn per verb, 27 verbs");
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
fn an_empty_listing_parses_to_nothing_rather_than_to_one_empty_name() {
    assert!(branches_in_ls_remote("").is_empty());
    assert!(lines("\n\n").is_empty());
    assert!(nul_separated("\0").is_empty());
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
