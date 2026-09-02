//! `aid` — AI Develop: `dl`, with a coding agent started for you.
//!
//! aid is a shortcut, not a second launcher. It rewrites its own command line into a
//! `dl` one and hands that to [`dl::run`], so
//!
//! ```text
//! aid owner/repo@branch fix the flaky test
//! ```
//!
//! is exactly
//!
//! ```text
//! dl owner/repo@branch -- claude --dangerously-skip-permissions \
//!     --remote-control=owner/repo@branch 'fix the flaky test'
//! ```
//!
//! Everything that decides how a workspace is obtained — the bare repo cache, the
//! worktree clone, the workspace id, the devpod container, the fast attach to one
//! that is already running, the forwarded gh login — happens inside dl, once. There
//! is no container machinery in this binary, deliberately: an aid that built its own
//! would drift from dl and start rebuilding containers dl would have reused.
//!
//! **In-process, as Python's `aid.py` is.** `dl::run` is called, not spawned: one
//! process, one `devpod` resolved from one PATH, one timing summary, and one exit
//! code. Spawning `dl` would also mean finding it — the two binaries are installed
//! together but nothing guarantees `dl` is on PATH of a run that reached `aid`
//! through an absolute path.

mod interactive;
mod rewrite;

use std::io::Write as _;

use rewrite::UsageError;

/// The variable a session manager reads to learn which agent is behind a wrapper.
///
/// herdr's name for it, and the one manager-specific word in either binary. It is
/// here because of what devlaunch#549 measured on hardware: the *rules* half of
/// screen detection already works through a dl workspace and only the
/// *identification* half does not. A herdr pane running aid has `aid`, `ssh` and
/// two `devpod`s as its foreground processes and no `claude` anywhere, so herdr
/// never decides which agent's manifest applies and never runs the rules that
/// would have classified the pane. The rules themselves are fine: the pane holds
/// the agent's real screen, because `dl <ws> -- <agent>` pipes the agent's own TUI
/// through it. Name the agent and the same pane reports idle, working and blocked
/// correctly, with nothing else changed — which is the whole of devlaunch#548.
///
/// Written unconditionally, and written over a value the environment already
/// holds. Both follow from aid being the thing that *decides* which agent starts:
/// a `HERDR_AGENT=codex` left in a profile is wrong the moment `aid --claude`
/// runs, and a machine with no herdr pays one `setenv` for a name nothing reads.
/// Detecting herdr first (`HERDR_ENV=1`) was the alternative and buys nothing —
/// the detection is a second thing to be wrong about, and being wrong about it
/// fails in the direction this whole issue is about.
///
/// The name is spelled here and once more in core, which writes the same variable
/// for a `dl <ws> -- <agent>` whose command is an agent by name
/// (`devlaunch-core/src/clients/herdr.rs`). Two copies because core's is
/// `pub(crate)` and this is a different binary; `test/unit/test_session_manager.py`
/// diffs the two spellings, which is what the standing rule in CLAUDE.md asks of a
/// second copy of a fact.
const SESSION_MANAGER_AGENT_VAR: &str = "HERDR_AGENT";

fn main() {
    dl::install_signal_handlers();
    // `args_os` and a lossy decode, not `args`: `std::env::args()` panics on an
    // argument that is not valid UTF-8, which would end `aid $'\xff'` with an exit
    // 101 and a traceback. Python decoded argv lossily and carried on
    // (docs/rust-rewrite-plan.md rows 4 and 12).
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let ending = run(&argv);
    // `process::exit` runs no destructors, and one of the things not run is the
    // flush of a stdout that ended without a newline.
    let _ = std::io::stdout().flush();
    std::process::exit(ending);
}

/// One `aid` command line: the three answers aid gives on its own, and dl for the
/// rest.
fn run(argv: &[String]) -> i32 {
    // The internal re-entries, before anything user-facing. `--boot-up` is the
    // background boot the interactive flow spawns — aid's argv, because the one
    // binary aid can find without guessing at PATH is itself. `--update-cache` is
    // the detached completion refresh dl re-spawns through `current_exe`, which
    // under aid *is* aid: without this arm every refresh an aid launch fired died
    // as "aid needs a workspace", and completions silently never refreshed.
    if argv
        .first()
        .is_some_and(|word| word == interactive::BOOT_WORD)
    {
        return dl::run(&argv[1..]);
    }
    if argv.first().is_some_and(|word| word == "--update-cache") {
        return dl::run(argv);
    }
    // No arguments is the help *and* a failure, which is Python's pair of endings for
    // one body: somebody who typed `aid` asked for a workspace and named none, and
    // somebody who typed `aid --help` got what they asked for.
    let asked_for_help = argv
        .first()
        .is_some_and(|word| word == "--help" || word == "-h");
    if argv.is_empty() || asked_for_help {
        print!("{}", help());
        return if argv.is_empty() { 1 } else { 0 };
    }
    if argv[0] == "--version" {
        // `aid <version>`, the version dl prints under aid's name, and the same
        // build marker: `dl::BUILD_MARKER` is empty in a released build and `-dev`
        // in a working-tree one, so `aid-next` says which build it is exactly as
        // `dl-next` does (#268). **Divergence row 16**: what Python appended here
        // was an editable install's provenance, which a compiled binary has none of.
        println!("aid {}{}", dl::VERSION, dl::BUILD_MARKER);
        return 0;
    }

    // `dl::env_str`, not `std::env::var(..).ok()`: that call reports a value which
    // is not valid UTF-8 as *unset*, so `DEVLAUNCH_AID_AGENT=$'\xff'` used to name
    // no agent at all and quietly start the default one instead of being refused
    // by name. Core's reading is reached through dl, like `shell` and
    // `python_repr`, so aid still sees nothing of devlaunch but dl.
    //
    // Read here rather than inside `rewrite`, which is one function from strings to
    // strings and stays that way: an environment read down there is a fact its tests
    // could not vary.
    let agent = dl::env_str(rewrite::AGENT_ENV_VAR);
    let remote_control = dl::env_str(rewrite::REMOTE_CONTROL_ENV_VAR);
    let environment = rewrite::Environment {
        agent: agent.as_deref(),
        remote_control: remote_control.as_deref(),
    };
    let parsed = match rewrite::parse_aid_args(argv, environment) {
        Ok(parsed) => parsed,
        Err(refused) => {
            eprintln!("{}", refusal(&refused));
            return 1;
        }
    };
    // The interactive default: an agent line with no prompt, on a terminal, boots
    // the workspace in the background while the prompt is typed here instead of in
    // a shell. Anything else — an inline prompt, a verb line, a pipe — comes back
    // unchanged with no boot, and takes the path it always took.
    let (parsed, boot) = interactive::collect_prompt(parsed);
    let Some(dl_args) = rewrite::build_dl_args(&parsed) else {
        // Unreachable by a command line: the parse only ever answers with an agent
        // from the table, and a line that starts no agent cannot fail to build one.
        // Reported rather than panicked on, in the words the refusal for an invented
        // name would have used.
        eprintln!(
            "{}",
            refusal(&UsageError::UnknownAgentInEnvironment {
                name: parsed.agent().unwrap_or_default().to_owned()
            })
        );
        return 1;
    };
    // Python's `logging.info("aid -> dl %s", shlex.join(dl_args))`, which lands on
    // stderr as the bare message. It is how a person sees what aid actually asked
    // for, and the quoting is what makes it a line they can paste.
    eprintln!(
        "aid -> dl {}",
        dl::shell::join(dl_args.iter().map(String::as_str))
    );
    // The boot the interactive flow started is waited out *after* the echo names
    // what will run, with its parked output replayed as it lands — so by the time
    // dl runs, the workspace is up and the launch is the fast attach.
    if let Some(boot) = boot {
        boot.finish();
    }
    name_agent_for_session_manager(parsed.agent());
    dl::run(&dl_args)
}

/// Put the agent's name in the environment dl's ssh child will inherit.
///
/// Here and not in dl, because dl is not what knows: `dl <ws> -- claude` somebody
/// typed themselves is their command, and reading an agent back out of a command
/// tail would be a guess. aid picked the agent from its own table, so aid is what
/// can say so — the same reasoning that puts `IS_SANDBOX` in that table rather
/// than in dl.
///
/// **It reaches a session manager through the ssh child, not through this
/// process.** A `setenv` after start does not rewrite `/proc/<pid>/environ`, which
/// the kernel fixes at exec, so aid's own row still shows no agent however early
/// this is called. What carries it is the `ssh` that [`dl::run`] spawns below,
/// which builds its environment from this one at its own exec; herdr walks the
/// pane's foreground processes rather than reading only the shell, and that ssh is
/// one of them.
///
/// **No test in this tree holds that**, and none can: it is a fact about a running
/// herdr, a pty and a container. It was measured instead, that way round and after
/// the change rather than before it, and the `/proc` reading is posted on
/// devlaunch#548 so the claim has a record and not just this paragraph. The
/// plausible-looking version of this function is a silent no-op, which is why it
/// was checked rather than argued.
///
/// A line that starts no agent names none. [`AidArgs::agent`] answers `None` for a
/// retired spelling, which dl is about to refuse, and refusals have no session for
/// anyone to classify.
fn name_agent_for_session_manager(agent: Option<&str>) {
    let Some(agent) = agent else { return };
    // Safety: `set_var` is unsound only against a concurrent reader in another
    // thread. aid is single-threaded here — the interactive boot is a *process*,
    // already finished above — and the reader this is for is a later `execve`.
    unsafe { std::env::set_var(SESSION_MANAGER_AGENT_VAR, agent) };
}

/// Why the command line could not be understood.
///
/// aid's own words, and Python's: the first is what somebody who typed `aid` alone
/// sees, and the second names the variable that has to be fixed.
fn refusal(refused: &UsageError) -> String {
    match refused {
        UsageError::NoWorkspace => {
            "aid needs a workspace: aid <user/repo>[@branch] [prompt]".to_owned()
        }
        // `{name!r}` in Python: `dl::python_repr` is that repr, reached through dl
        // rather than hand-quoted here, so a name holding a quote or a control byte
        // is spelled the way Python's `repr` spelled it and not merely wrapped in
        // single quotes.
        UsageError::UnknownAgentInEnvironment { name } => format!(
            "{}={} is not a known agent. Choose one of: {}.",
            rewrite::AGENT_ENV_VAR,
            dl::python_repr(name),
            rewrite::agent_names().join(", ")
        ),
        // The agent is named because the flag is not always what picked it: with
        // `DEVLAUNCH_AID_AGENT=codex` set, nothing on the command line says codex,
        // and a sentence that only said "pick --claude" would be answering a
        // question the person did not ask.
        UsageError::RemoteControlUnsupported { agent } => format!(
            "{} starts Claude Code's Remote Control, which only the claude agent has, not {agent}. \
             Drop the flag or pick --claude.",
            rewrite::REMOTE_CONTROL_FLAG
        ),
        // The values are listed for the same reason the agent names are: the
        // variable is in somebody's profile, and the sentence has to be enough to
        // fix it without reading the help.
        UsageError::UnknownRemoteControlInEnvironment { value } => format!(
            "{}={} is not a yes or a no. Choose one of: {}.",
            rewrite::REMOTE_CONTROL_ENV_VAR,
            dl::python_repr(value),
            rewrite::remote_control_values().join(", ")
        ),
    }
}

/// aid's usage text.
///
/// Hand-written rather than generated, and unlike `dl`'s (**divergence row 3**) it is
/// Python's own text: aid's grammar is not a clap grammar and could not be one — an
/// unknown leading flag is *passed through to dl* and everything after the workspace
/// is the prompt, flags and all, which a parser that rejects unknown flags would
/// refuse. What generates a `--help` here would have to be the parser that cannot
/// exist, so the text is the interface.
fn help() -> String {
    let agents = rewrite::agent_names()
        .iter()
        .map(|name| format!("--{name}"))
        .collect::<Vec<String>>()
        .join(", ");
    let default = rewrite::DEFAULT_AGENT;
    let variable = rewrite::AGENT_ENV_VAR;
    let remote_variable = rewrite::REMOTE_CONTROL_ENV_VAR;
    let manager_variable = SESSION_MANAGER_AGENT_VAR;
    format!(
        "\
aid - AI Develop: start a coding agent in a devlaunch workspace

aid is a shortcut for `dl <workspace> -- <agent> '<prompt>'`. The workspace is
opened by dl itself, so it is the same workspace, container and clone that
`dl <workspace>` gives you — started if it is stopped, attached to if it is
already running, and never rebuilt just because aid asked for it.

Usage:
    aid <user/repo>[@branch] [prompt...]   Open the workspace and start the agent
    aid <workspace> [prompt...]            Same, for an existing workspace or ./path

With no prompt on a terminal, aid boots the workspace in the background and
asks for the prompt while it does: type it free of shell quoting and press
Enter to launch. An empty Enter (or Ctrl-D) starts the agent's plain session.
Piping stdin or setting DEVLAUNCH_NO_TTY=1 skips the question and launches
one-shot, as a prompt on the command line always has.

Options:
    {agents}
                                     Pick the agent (default: {default})
    --devcontainer <variant|path>    Passed through to dl
    --rm                             Delete the workspace once the agent's session
                                     ends, the way docker run --rm does. Appendable:
                                     recall the line and type it at the end, prompt
                                     and all — the agent still runs, and the workspace
                                     goes when it is done. Stops at work that is
                                     nowhere else and says so, leaving the workspace
                                     standing. To delete one now instead, that is
                                     dl's rm verb: dl <workspace> rm.
    --no-remote-control, --no-remote
                                     Start a plain local session. Remote Control is
                                     on by default: claude is started under the
                                     workspace you typed as the session name, so the
                                     session in this terminal is also readable and
                                     steerable from claude.ai/code and the Claude
                                     app. It is claude only, and it needs a
                                     claude.ai (Pro/Max/Team) login inside the
                                     workspace. The agent runs with permissions
                                     skipped, so the account signed in there can
                                     drive it.
    --remote-control, --remote       Ask for Remote Control by name, which is what
                                     turns one launch back on when the variable
                                     below has turned the default off. Beside
                                     --codex or --gemini it says they have not got
                                     it and stops, where the default is silently
                                     absent.
    --help, -h                       Show this help
    --version                        Show version

Environment:
    {variable}=<agent>       Change the default agent
    {remote_variable}=0
                                     Turn the Remote Control default off for every
                                     launch. Takes 1/true/on/yes or 0/false/off/no,
                                     and refuses anything else
    {manager_variable}=<agent>
                                     Set, not read: every launch names its agent
                                     here so a session manager can tell what the
                                     pane is doing. Overwrites whatever you set

Examples:
    aid blooop/devlaunch                       # Start {default} in the workspace
    aid blooop/devlaunch@fix/42 fix the bug    # Open the branch, hand over the prompt
    aid --gemini ./my-project explain this     # Pick a different agent
    aid --no-remote blooop/devlaunch           # Nothing but the session in front of you
    aid blooop/devlaunch@fix/42 fix the bug --rm
                                               # The line above, recalled, with the
                                               # workspace deleted once the agent is
                                               # done with it

Everything else — listing, stopping, deleting, VS Code — is dl's job:
    dl --help\n\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The process environment, and the lock that makes writing it sound.
    ///
    /// `HERDR_AGENT` is process-wide, so the tests that write it take this for
    /// their whole body and put it back on the way out. Nothing else in this
    /// binary's test run reads the variable, but two of these tests read what the
    /// other wrote if they overlap.
    static THE_ENVIRONMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The value as it was found, put back when this leaves scope.
    ///
    /// A `Drop` and not a line after the body, because a failed assertion unwinds
    /// past a line: the test that failed would leak its value into every test that
    /// ran after it, and the second failure would be the confusing one.
    struct Restore {
        before: Option<std::ffi::OsString>,
        /// Dropped after the restore above, so the lock still covers it.
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for Restore {
        fn drop(&mut self) {
            // Safety: every reader is a test body, and this still holds the lock
            // every test body takes.
            unsafe {
                match self.before.take() {
                    Some(value) => std::env::set_var(SESSION_MANAGER_AGENT_VAR, value),
                    None => std::env::remove_var(SESSION_MANAGER_AGENT_VAR),
                }
            }
        }
    }

    /// Run `body` with the variable saved and restored, whatever it does.
    fn with_the_environment(body: impl FnOnce()) {
        let guard = THE_ENVIRONMENT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _restore = Restore {
            before: std::env::var_os(SESSION_MANAGER_AGENT_VAR),
            _guard: guard,
        };
        body();
    }

    /// A body that fails still puts the environment back.
    ///
    /// The reason [`Restore`] is a `Drop` and not a line after the call. An
    /// assertion that fails unwinds past a line, so the failing test would leak
    /// its value into every test that ran after it and the *second* failure would
    /// be the one nobody could explain.
    #[test]
    fn a_body_that_panics_still_restores_the_environment() {
        const LEAKED: &str = "a-value-no-environment-holds";

        let panicked = std::panic::catch_unwind(|| {
            with_the_environment(|| {
                name_agent_for_session_manager(Some(LEAKED));
                panic!("as a failing assertion would");
            });
        });

        assert!(panicked.is_err(), "the body was supposed to panic");
        // Under the lock, which is also what shows the guard was released rather
        // than left held for the rest of the run.
        with_the_environment(|| {
            assert_ne!(
                std::env::var(SESSION_MANAGER_AGENT_VAR).ok().as_deref(),
                Some(LEAKED),
                "a failed test leaked its value into the tests after it"
            );
        });
    }

    /// Every name in the table reaches the environment **verbatim**.
    ///
    /// Verbatim is the whole of what this pins, and it is worth pinning: herdr
    /// matches the value against a manifest id, so `claude` classifies a pane
    /// where `Claude` or `claude-code` identifies nothing and runs no rules. A
    /// transformation applied here would therefore fail at the far end, in
    /// silence, which is the failure devlaunch#548 is about.
    ///
    /// What this cannot check is that the names *are* ids herdr knows: that list
    /// lives in herdr and not in this tree. The convention is stated on
    /// [`rewrite`]'s agent table instead, where a new row is written.
    #[test]
    fn aid_names_the_agent_it_starts() {
        for name in rewrite::agent_names() {
            with_the_environment(|| {
                name_agent_for_session_manager(Some(name));

                assert_eq!(
                    std::env::var(SESSION_MANAGER_AGENT_VAR).ok().as_deref(),
                    Some(name),
                    "{name} is not the name a session manager would read"
                );
            });
        }
    }

    /// A stale value in somebody's profile does not survive the agent aid picked.
    ///
    /// This is the whole reason the write is unconditional. `HERDR_AGENT=codex`
    /// exported for some other wrapper is *wrong* about an `aid --claude` launch,
    /// and a session manager that believed it would run codex's rules against
    /// Claude's screen — which classifies nothing, silently, the way everything in
    /// devlaunch#548 fails.
    #[test]
    fn a_name_already_in_the_environment_is_replaced() {
        with_the_environment(|| {
            // Safety: the guard `with_the_environment` holds is the lock every
            // reader in this binary takes.
            unsafe { std::env::set_var(SESSION_MANAGER_AGENT_VAR, "codex") };

            name_agent_for_session_manager(Some("claude"));

            assert_eq!(
                std::env::var(SESSION_MANAGER_AGENT_VAR).ok().as_deref(),
                Some("claude")
            );
        });
    }

    /// A line that starts no agent names none, and disturbs nothing.
    ///
    /// A retired spelling is about to become dl's refusal. There is no session for
    /// anyone to classify, so the environment is left exactly as it was found —
    /// including a value somebody set themselves, which aid has no launch to
    /// contradict.
    #[test]
    fn a_line_that_starts_no_agent_leaves_the_environment_alone() {
        with_the_environment(|| {
            // Safety: as above.
            unsafe { std::env::set_var(SESSION_MANAGER_AGENT_VAR, "theirs") };

            name_agent_for_session_manager(None);

            assert_eq!(
                std::env::var(SESSION_MANAGER_AGENT_VAR).ok().as_deref(),
                Some("theirs")
            );
        });

        with_the_environment(|| {
            // Safety: as above.
            unsafe { std::env::remove_var(SESSION_MANAGER_AGENT_VAR) };

            name_agent_for_session_manager(None);

            assert!(std::env::var_os(SESSION_MANAGER_AGENT_VAR).is_none());
        });
    }

    #[test]
    fn the_help_names_every_agent_and_the_default() {
        let help = help();

        for name in rewrite::agent_names() {
            assert!(help.contains(&format!("--{name}")), "{name} is not offered");
        }
        assert!(help.contains("(default: claude)"));
        // Python's `print(f"""…\n""")`: the text ends with a blank line.
        assert!(help.ends_with("dl --help\n\n"), "{help:?}");
    }

    #[test]
    fn the_help_names_the_interactive_default_and_both_ways_out() {
        let help = help();

        assert!(
            help.contains("boots the workspace in the background"),
            "{help}"
        );
        assert!(help.contains("empty Enter"), "{help}");
        assert!(help.contains("DEVLAUNCH_NO_TTY=1"), "{help}");
    }

    #[test]
    fn the_help_names_remote_control_and_what_it_needs() {
        let help = help();

        assert!(help.contains("--remote-control"), "{help}");
        // The two things somebody finds out the hard way otherwise: it is claude's
        // alone, and it wants a login the container may not have.
        assert!(help.contains("claude only"), "{help}");
        assert!(help.contains("claude.ai (Pro/Max/Team)"), "{help}");
    }

    #[test]
    fn the_help_says_remote_control_is_on_and_names_both_ways_out() {
        // It is on without anybody typing anything now, so the help's job here is
        // the opposite of what it was: not how to ask for it, but that it is
        // happening and how to stop it. Both switches are named because they turn
        // it off at different scopes.
        let help = help();

        assert!(help.contains("on by default"), "{help}");
        assert!(help.contains("--no-remote-control"), "{help}");
        assert!(help.contains("--no-remote"), "{help}");
        assert!(help.contains("DEVLAUNCH_AID_REMOTE_CONTROL"), "{help}");
        // And the consequence of the default, said once: the agent already runs
        // with permissions skipped, so a drivable session is a drivable agent.
        assert!(help.contains("permissions"), "{help}");
    }

    #[test]
    fn a_remote_control_variable_that_is_neither_a_yes_nor_a_no_lists_both() {
        assert_eq!(
            refusal(&UsageError::UnknownRemoteControlInEnvironment {
                value: "maybe".to_owned()
            }),
            "DEVLAUNCH_AID_REMOTE_CONTROL='maybe' is not a yes or a no. \
             Choose one of: 1, true, on, yes, 0, false, off, no."
        );
    }

    #[test]
    fn remote_control_beside_an_agent_that_has_none_says_which_agent() {
        // The agent is named because the flag is not always what chose it: with
        // DEVLAUNCH_AID_AGENT set there is nothing on the command line saying codex.
        assert_eq!(
            refusal(&UsageError::RemoteControlUnsupported {
                agent: "codex".to_owned()
            }),
            "--remote-control starts Claude Code's Remote Control, which only the claude agent \
             has, not codex. Drop the flag or pick --claude."
        );
    }

    #[test]
    fn a_command_line_with_no_workspace_says_what_one_looks_like() {
        assert_eq!(
            refusal(&UsageError::NoWorkspace),
            "aid needs a workspace: aid <user/repo>[@branch] [prompt]"
        );
    }

    #[test]
    fn an_agent_the_environment_invented_is_named_with_the_ones_that_exist() {
        assert_eq!(
            refusal(&UsageError::UnknownAgentInEnvironment {
                name: "nope".to_owned()
            }),
            "DEVLAUNCH_AID_AGENT='nope' is not a known agent. Choose one of: claude, codex, \
             gemini."
        );
    }
}
