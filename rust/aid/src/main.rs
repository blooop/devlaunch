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
    dl::run(&dl_args)
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
