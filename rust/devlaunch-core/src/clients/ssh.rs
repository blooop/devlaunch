//! Carry a terminal into the workspace for commands that need one.
//!
//! Ported from `devlaunch/tty_session.py`; see docs/rust-rewrite-plan.md (M3).
//!
//! `devpod ssh --command` never asks its ssh session for a pty. It requests one
//! only for a bare interactive attach, so anything started through `--command`
//! runs with its three streams on pipes and `TERM=dumb`, and no devpod flag forces
//! the matter. One-shot commands do not care. Interactive ones do, and they fail
//! in a way that does not look like a missing terminal: `claude` reads the pipe as
//! "invoked non-interactively", switches to print mode and exits.
//!
//! devpod already publishes the way out. Every `devpod up` writes an ssh host
//! alias `<workspace>.devpod` into `~/.ssh/config` whose `ProxyCommand` tunnels
//! through `devpod ssh --stdio`, so OpenSSH can open the same session devpod
//! would and, with `-t`, ask it for a pty — which also hands the user OpenSSH's
//! terminal handling (raw mode, window size, SIGWINCH) rather than a pty proxy
//! devlaunch would have to reimplement.
//!
//! # What this module decides
//!
//! Only whether the transport is available and what the argv is. Which transport
//! a command actually takes is the launch flow's decision (M7), and it is
//! deliberately the same decision ssh itself makes: use a terminal when there is
//! a terminal to use, so a redirected `dl <ws> -- ls > out.txt` keeps the devpod
//! transport and keeps its output free of escape sequences. Three facts feed it,
//! and each is a function here: [`tty_disabled`], [`terminal_usable`] and
//! [`host_published`].
//!
//! Two things `dl.py` does around this module stay there: the wrapping of a
//! command into `bash -lc <quoted>` (shared with the devpod transport, so both
//! get the same PATH), and the `DEVLAUNCH_DOTFILES_ON_ATTACH` gate, which is not
//! in `tty_session.py` at all.

// The launch flow that chooses this transport lands in M7.
#![allow(dead_code)] // consumed from M5 on

use std::path::{Path, PathBuf};

use crate::runner::{EnvSpec, Exit, Invocation, OsFailure, Outcome, Runner, SpawnSpec};
use crate::shell;

/// OpenSSH, dl's other way into a workspace.
pub(crate) const PROGRAM: &str = "ssh";

/// devpod names each alias after the workspace.
pub(crate) const HOST_SUFFIX: &str = ".devpod";

/// devpod brackets each block with markers it recognises again on the next `up`.
pub(crate) const MARKER_PREFIX: &str = "# DevPod Start ";

/// Set this to keep every command on the devpod transport, whatever the terminal
/// says.
pub(crate) const DISABLE_VAR: &str = "DEVLAUNCH_NO_TTY";

/// The values that mean "no" rather than "set, therefore yes". The same list as
/// [`super::gh::forwarding_disabled`] reads, and `tty_session.py` and `gh_auth.py`
/// each keep their own copy for the same reason: two escape hatches that answer
/// to one shared constant are one edit away from becoming one escape hatch.
const FALSEY: [&str; 4] = ["", "0", "false", "no"];

/// ssh never ran.
///
/// Its own type rather than [`super::devpod::NotRun`], for the reason Python
/// gives `SshNotInstalled` its own class: telling someone to install devpod when
/// devpod is present and working would send them the wrong way. The way out that
/// needs no ssh at all — `DEVLAUNCH_NO_TTY=1` — is what the rendering names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotRun {
    NotInstalled,
    TimedOut,
    Blocked(OsFailure),
}

/// A terminal session that cannot be composed, and must not be approximated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnsafeRequest {
    /// The workspace id would reach ssh as an option.
    ///
    /// The alias goes in positionally and ssh has no reliable `--`, so an id
    /// beginning with a dash is read as a flag — and `-o ProxyCommand=…` is
    /// arbitrary command execution on the host. devpod's own ids cannot look like
    /// that (a workspace id starts with a word character) and neither can the
    /// `~/.ssh/config` entry this is gated on, so reaching here means something
    /// upstream is already wrong.
    OptionLikeWorkspaceId { workspace_id: String },
    /// The working directory cannot be made into a shell word, because it holds a
    /// NUL. Python has no such refusal — `shlex.quote` wraps it and the remote
    /// shell mangles it — and an argument that cannot mean what it says is better
    /// refused than sent.
    UnquotableWorkdir { workdir: String },
}

/// The ssh host name devpod publishes for a workspace.
pub(crate) fn host_alias(workspace_id: &str) -> String {
    format!("{workspace_id}{HOST_SUFFIX}")
}

/// Build the OpenSSH invocation that runs `command` under a pty.
///
/// `send_env` names variables only. OpenSSH reads their values from its own
/// environment, so a forwarded token never appears in argv where `ps` would show
/// it to every other user on the host — the same discipline
/// [`super::gh`] applies to the devpod transport.
///
/// `workdir` travels *inside* the payload, because ssh has no `--workdir`. An
/// empty one names no directory: landing in the `workspaceFolder` from
/// devcontainer.json is the right default, and `cd '' &&` would fail for no
/// reason.
pub(crate) fn command_args(
    workspace_id: &str,
    command: &str,
    send_env: &[String],
    workdir: Option<&str>,
) -> Result<Vec<String>, UnsafeRequest> {
    if workspace_id.starts_with('-') {
        return Err(UnsafeRequest::OptionLikeWorkspaceId {
            workspace_id: workspace_id.to_owned(),
        });
    }
    let mut args = vec![PROGRAM.to_owned(), "-t".to_owned()];
    for name in send_env {
        args.push("-o".to_owned());
        args.push(format!("SendEnv={name}"));
    }
    args.push(host_alias(workspace_id));
    args.push(payload(command, workdir)?);
    Ok(args)
}

/// The one remote argument: the command, under a `cd` when there is a directory.
///
/// The directory is quoted by [`shell::quote`] — Python's `shlex.quote`, byte for
/// byte — and not by the `shlex` crate, which spells two things differently: it
/// quotes a word Python leaves bare (`/srv/a@b`), and it switches to double quotes
/// for a directory holding an apostrophe (`/home/o'brien/src`) where Python writes
/// one single-quoted word with `'"'"'`. The payload travels in argv and argv is what
/// the parity harness compares, so a session that ran the same command in the same
/// directory would still have differed in bytes.
///
/// A NUL is the one thing quoting cannot fix, and it is refused rather than sent —
/// which is what the crate's `try_quote` was here for.
fn payload(command: &str, workdir: Option<&str>) -> Result<String, UnsafeRequest> {
    match workdir {
        None | Some("") => Ok(command.to_owned()),
        Some(workdir) if shell::holds_nul(workdir) => Err(UnsafeRequest::UnquotableWorkdir {
            workdir: workdir.to_owned(),
        }),
        Some(workdir) => Ok(format!("cd {} && {command}", shell::quote(workdir))),
    }
}

/// Whether the user opted this machine out of the pty transport.
pub(crate) fn tty_disabled(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(value) => !FALSEY.contains(&value.trim().to_lowercase().as_str()),
    }
}

/// Whether dl was run from a terminal it can hand to the workspace.
///
/// Both directions have to be a terminal. Python also has to defend against a
/// stream that is not a real one — pytest's capture object, a closed file — which
/// a raw file descriptor cannot be: `isatty` answers no for anything that is not
/// a terminal, including a descriptor that is closed.
pub(crate) fn terminal_usable(disable: Option<&str>, stdin_tty: bool, stdout_tty: bool) -> bool {
    !tty_disabled(disable) && stdin_tty && stdout_tty
}

/// [`terminal_usable`] for this process, as the launch flow asks it.
pub(crate) fn have_terminal() -> bool {
    terminal_usable(
        std::env::var(DISABLE_VAR).ok().as_deref(),
        is_a_terminal(libc::STDIN_FILENO),
        is_a_terminal(libc::STDOUT_FILENO),
    )
}

fn is_a_terminal(descriptor: std::ffi::c_int) -> bool {
    // SAFETY: `isatty` reads one descriptor and returns a flag; it cannot fail in
    // a way that matters here, since "not a terminal" and "no such descriptor"
    // are the same answer to this question.
    unsafe { libc::isatty(descriptor) == 1 }
}

/// Where devpod writes its host aliases.
pub(crate) fn config_path() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".ssh").join("config"))
}

/// Whether devpod has published an ssh alias for this workspace.
///
/// A config dl cannot read is a fallback, never a crash mid-launch: no file, no
/// permission, a directory where a file should be — all mean the same thing.
pub(crate) fn devpod_host_configured(config_path: &Path, workspace_id: &str) -> bool {
    match std::fs::read(config_path) {
        // Lossily decoded: what is in a user's ssh config is not devlaunch's
        // promise, and one bad byte elsewhere in the file must not hide an alias.
        Ok(bytes) => host_published(&String::from_utf8_lossy(&bytes), workspace_id),
        Err(_) => false,
    }
}

/// Whether `config` carries devpod's start marker for this workspace.
///
/// Matched on the marker as a whole line, not as a substring: workspace ids share
/// prefixes by construction (`devlaunch-main-abcdefgh` and
/// `devlaunch-main-ijklmnop`), so a substring test would route a command at a host
/// alias belonging to a different container.
pub(crate) fn host_published(config: &str, workspace_id: &str) -> bool {
    let marker = format!("{MARKER_PREFIX}{}", host_alias(workspace_id));
    config.lines().any(|line| line.trim() == marker)
}

/// Run OpenSSH, dl's other way into a workspace.
///
/// Takes the whole argv, `ssh` included, because [`command_args`] composes a
/// complete command rather than a tail of flags — unlike the devpod calls, whose
/// callers each build their own subcommand.
///
/// Nothing is captured: this is a terminal session, and the remote program's own
/// streams are the point. Its status comes back as it is, because OpenSSH exits
/// with the remote program's status — the thing devpod loses by wrapping its
/// `*ssh.ExitError` three times before type-asserting on it, and the reason this
/// transport needs none of that recovery machinery.
pub(crate) fn run(runner: &dyn Runner, args: &[String], env: EnvSpec) -> Result<Exit, NotRun> {
    let Some((program, rest)) = args.split_first() else {
        // An empty argv names no program, so nothing was ever going to run.
        return Err(NotRun::NotInstalled);
    };
    let spec = SpawnSpec::new(
        Invocation::new(program.clone())
            .with_args(rest.iter().cloned())
            .with_env(env),
    );
    match runner.passthrough(&spec) {
        Outcome::Ran { exit, .. } => Ok(exit),
        Outcome::ProgramNotFound => Err(NotRun::NotInstalled),
        Outcome::TimedOut => Err(NotRun::TimedOut),
        Outcome::NotStarted(failure) => Err(NotRun::Blocked(failure)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::devpod::tests::{Recorded, Reply, ScriptedRunner};

    /// An ssh config in the exact shape `devpod up` leaves behind.
    fn devpod_config(workspace_ids: &[&str]) -> String {
        workspace_ids
            .iter()
            .map(|id| {
                let host = format!("{id}{HOST_SUFFIX}");
                format!(
                    "# DevPod Start {host}\n\
                     Host {host}\n\
                     \x20 ForwardAgent yes\n\
                     \x20 StrictHostKeyChecking no\n\
                     \x20 ProxyCommand \"devpod\" ssh --stdio --context default --user vscode {id}\n\
                     \x20 User vscode\n\
                     # DevPod End {host}\n"
                )
            })
            .collect()
    }

    fn args_for(workspace_id: &str, command: &str) -> Vec<String> {
        command_args(workspace_id, command, &[], None).expect("a well-formed request")
    }

    // ------------------------------------------------- the OpenSSH invocation

    #[test]
    fn the_whole_argv_is_what_dl_hands_to_openssh() {
        // The argv *is* the contract: `-t` before the alias, the alias
        // positionally, one payload argument last.
        let args = command_args(
            "devlaunch-main-abcdefgh",
            "bash -lc claude",
            &["GH_TOKEN".to_owned()],
            Some("/workspaces/devlaunch"),
        )
        .expect("a well-formed request");

        assert_eq!(
            args,
            vec![
                "ssh".to_owned(),
                "-t".to_owned(),
                "-o".to_owned(),
                "SendEnv=GH_TOKEN".to_owned(),
                "devlaunch-main-abcdefgh.devpod".to_owned(),
                "cd /workspaces/devlaunch && bash -lc claude".to_owned(),
            ]
        );
    }

    #[test]
    fn it_forces_a_pty() {
        // Without -t ssh runs the command with no terminal, which is the whole
        // situation this transport exists to escape.
        let args = args_for("myws", "bash -lc claude");

        assert_eq!(args[0], PROGRAM);
        assert!(args.contains(&"-t".to_owned()), "{args:?}");
    }

    #[test]
    fn it_targets_the_host_alias_devpod_published() {
        let args = args_for("myws", "bash -lc claude");

        assert!(args.contains(&"myws.devpod".to_owned()), "{args:?}");
        assert_eq!(host_alias("myws"), "myws.devpod");
    }

    #[test]
    fn the_payload_is_the_final_argument() {
        // One argument, so the remote shell sees the command dl composed.
        let args = args_for("myws", "bash -lc 'claude do the thing'");

        assert_eq!(
            args.last().map(String::as_str),
            Some("bash -lc 'claude do the thing'")
        );
    }

    #[test]
    fn the_host_comes_before_the_payload() {
        let args = args_for("myws", "bash -lc claude");

        let host = args
            .iter()
            .position(|arg| arg == "myws.devpod")
            .expect("the alias is there");
        assert!(host < args.len() - 1, "{args:?}");
    }

    #[test]
    fn send_env_names_variables_without_their_values() {
        // The token must reach the container through the environment, not argv.
        let args = command_args("myws", "bash -lc claude", &["GH_TOKEN".to_owned()], None)
            .expect("a well-formed request");

        assert!(args.contains(&"SendEnv=GH_TOKEN".to_owned()), "{args:?}");
        assert!(!args.iter().any(|arg| arg.contains("secret")));
    }

    #[test]
    fn nothing_to_forward_means_no_send_env_at_all() {
        let args = args_for("myws", "bash -lc claude");

        assert!(
            !args.iter().any(|arg| arg.starts_with("SendEnv")),
            "{args:?}"
        );
    }

    #[test]
    fn a_workdir_becomes_a_cd_inside_the_payload() {
        // ssh has no --workdir, so a directory has to travel in the command.
        let args = command_args("myws", "bash -lc make", &[], Some("/workspaces/myws"))
            .expect("a well-formed request");

        let payload = args.last().expect("a payload");
        assert!(
            payload.starts_with("cd /workspaces/myws && "),
            "{payload:?}"
        );
        assert!(payload.ends_with("bash -lc make"), "{payload:?}");
    }

    #[test]
    fn a_workdir_with_spaces_is_quoted_for_the_remote_shell() {
        let args = command_args("myws", "bash -lc make", &[], Some("/a dir/with space"))
            .expect("a well-formed request");

        assert!(
            args.last()
                .expect("a payload")
                .contains("'/a dir/with space'"),
            "{args:?}"
        );
    }

    #[test]
    fn a_workdir_is_quoted_the_way_python_quotes_it_and_not_the_way_shlex_does() {
        // The two words the `shlex` crate spells differently, which is why this
        // module quotes with `shell::quote`: an apostrophe makes the crate switch to
        // double quotes, and `@`/`%` make it quote a word CPython leaves bare.
        for (workdir, expected) in [
            (
                "/home/o'brien/src",
                r#"cd '/home/o'"'"'brien/src' && bash -lc make"#,
            ),
            ("/srv/a@b%c", "cd /srv/a@b%c && bash -lc make"),
        ] {
            let args = command_args("myws", "bash -lc make", &[], Some(workdir))
                .expect("a well-formed request");

            assert_eq!(args.last().expect("a payload"), expected);
        }
    }

    #[test]
    fn a_workdir_that_names_nothing_adds_no_cd() {
        // Python's `if workdir:` reads an empty flag value as no workdir; landing
        // in the workspaceFolder from devcontainer.json is the right default, and
        // `cd '' &&` would be a payload that fails for no reason.
        let args = command_args("myws", "bash -lc make", &[], Some("")).expect("well-formed");

        assert_eq!(args.last().map(String::as_str), Some("bash -lc make"));
    }

    #[test]
    fn a_workspace_id_that_looks_like_an_option_is_refused() {
        // The alias goes in positionally and ssh has no reliable `--`, so a
        // leading dash would be read as a flag — and `-o ProxyCommand=...` is
        // arbitrary command execution on the host. Refused rather than quoted:
        // nothing upstream can legitimately produce it, so reaching here means
        // something is already wrong.
        for workspace_id in ["-oProxyCommand=touch /tmp/pwned", "-t", "--rubbish", "-"] {
            assert_eq!(
                command_args(workspace_id, "bash -lc claude", &[], None),
                Err(UnsafeRequest::OptionLikeWorkspaceId {
                    workspace_id: workspace_id.to_owned()
                }),
                "{workspace_id:?}"
            );
        }
    }

    #[test]
    fn an_ordinary_workspace_id_is_not_refused() {
        for workspace_id in ["devlaunch-main-abcdefgh", "my_ws.2", "ws-1"] {
            assert!(
                command_args(workspace_id, "true", &[], None).is_ok(),
                "{workspace_id:?}"
            );
        }
    }

    #[test]
    fn a_workdir_no_remote_shell_could_be_given_is_refused() {
        // A NUL cannot survive a shell word, so there is no payload to build.
        // Python has no such refusal — `shlex.quote` wraps it and the remote
        // shell mangles it — and an argument that cannot mean what it says is
        // better refused than sent.
        assert_eq!(
            command_args("myws", "true", &[], Some("/a\0dir")),
            Err(UnsafeRequest::UnquotableWorkdir {
                workdir: "/a\0dir".to_owned()
            })
        );
    }

    // ------------------------------------------------------- is there a tty

    #[test]
    fn both_streams_have_to_be_a_terminal() {
        // A pty on stdout with stdin redirected would give a TUI a screen it
        // cannot receive keystrokes on; a pty on stdin with stdout redirected
        // would fill the redirect with escape sequences.
        assert!(terminal_usable(None, true, true));
        assert!(!terminal_usable(None, true, false));
        assert!(!terminal_usable(None, false, true));
        assert!(!terminal_usable(None, false, false));
    }

    #[test]
    fn the_opt_out_forces_the_devpod_transport_whatever_the_terminal_says() {
        // An escape hatch for a machine where the ssh alias is stale or the
        // tunnel misbehaves, matching DEVLAUNCH_NO_GH_TOKEN in spirit.
        assert!(!terminal_usable(Some("1"), true, true));
        assert!(tty_disabled(Some("1")));
    }

    #[test]
    fn a_falsey_opt_out_still_allows_a_terminal() {
        for value in ["", "0", "false", "no", " NO "] {
            assert!(terminal_usable(Some(value), true, true), "{value:?}");
            assert!(!tty_disabled(Some(value)), "{value:?}");
        }
        assert!(!tty_disabled(None));
    }

    // ------------------------------------------ has devpod published an alias

    #[test]
    fn the_entry_devpod_wrote_is_found() {
        let config = devpod_config(&["devlaunch-main-abcdefgh"]);

        assert!(host_published(&config, "devlaunch-main-abcdefgh"));
    }

    #[test]
    fn another_workspaces_entry_is_not_this_workspaces_entry() {
        let config = devpod_config(&["some-other-workspace"]);

        assert!(!host_published(&config, "devlaunch-main-abcdefgh"));
    }

    #[test]
    fn a_prefix_of_another_workspace_does_not_count() {
        // Workspace ids share prefixes by construction (`devlaunch-main-abcdefgh`
        // and `devlaunch-main-ijklmnop`), so a substring test would route a
        // command at a host alias belonging to a different container.
        let config = devpod_config(&["devlaunch-main-abcdefgh"]);

        assert!(!host_published(&config, "devlaunch-main"));
    }

    #[test]
    fn one_of_several_entries_is_found() {
        let config = devpod_config(&["ws-one", "ws-two", "ws-three"]);

        assert!(host_published(&config, "ws-two"));
        assert!(!host_published(&config, "ws-four"));
    }

    #[test]
    fn the_marker_is_matched_as_a_whole_line() {
        // devpod's own start marker, not a mention of it inside another line.
        let mentioned = format!("#   {MARKER_PREFIX}myws.devpod is what we want\n");

        assert!(!host_published(&mentioned, "myws"));
        assert!(host_published(
            &format!("  {MARKER_PREFIX}myws.devpod  \n"),
            "myws"
        ));
    }

    #[test]
    fn a_config_that_cannot_be_read_is_a_fallback_and_never_a_failed_launch() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let written = scratch.path().join("config");
        std::fs::write(&written, devpod_config(&["myws"])).expect("a config");

        assert!(devpod_host_configured(&written, "myws"));
        // No config, no permission, a directory where a file should be — all mean
        // "fall back", never "fail the launch".
        assert!(!devpod_host_configured(
            &scratch.path().join("nope"),
            "myws"
        ));
        assert!(!devpod_host_configured(scratch.path(), "myws"));
    }

    // ----------------------------------------------------------- the spawn

    #[test]
    fn openssh_is_run_with_the_argv_it_was_given_and_the_environment_it_needs() {
        // The whole argv, `ssh` included, because this transport composes a
        // complete command rather than a tail of flags.
        let fake = ScriptedRunner::new();
        let args = command_args("myws", "bash -lc claude", &["GH_TOKEN".to_owned()], None)
            .expect("well-formed");
        let env = EnvSpec::inherited().and("GH_TOKEN", "gho_secret");

        let exit = run(&fake, &args, env.clone()).expect("ssh ran");

        assert_eq!(exit, Exit::Code(0));
        assert_eq!(fake.argvs(), vec![args]);
        let call = fake.only_call();
        assert!(matches!(call, Recorded::Passthrough(_)), "{call:?}");
        assert_eq!(call.invocation().env, env);
    }

    #[test]
    fn openssh_passes_the_remote_programs_own_status_through() {
        // Nothing to recover here, unlike `devpod ssh`: OpenSSH exits with the
        // remote program's status, which is the thing devpod loses by wrapping
        // its *ssh.ExitError three times before type-asserting on it.
        let fake = ScriptedRunner::new().with_script(["ssh"], Reply::exited(130));

        assert_eq!(
            run(
                &fake,
                &args_for("myws", "bash -lc claude"),
                EnvSpec::inherited()
            ),
            Ok(Exit::Code(130))
        );
    }

    #[test]
    fn an_openssh_that_is_not_installed_is_its_own_answer() {
        // Its own arm rather than devpod's: telling someone to install devpod
        // when devpod is present and working sends them the wrong way.
        let fake = ScriptedRunner::new().with_missing("ssh");

        assert_eq!(
            run(&fake, &args_for("myws", "true"), EnvSpec::inherited()),
            Err(NotRun::NotInstalled)
        );
    }
}
