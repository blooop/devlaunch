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
//! dl owner/repo@branch -- claude --dangerously-skip-permissions 'fix the flaky test'
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
    dl::install_interrupt_handler();
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

    let parsed = match rewrite::parse_aid_args(
        argv,
        std::env::var(rewrite::AGENT_ENV_VAR).ok().as_deref(),
    ) {
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
    // Read here rather than in `rewrite`, which is deliberately a pure
    // string-to-strings module: the pane is a fact about this process.
    let pane = dl::herdr::Session::from_env();
    let Some(dl_args) = rewrite::build_dl_args(&parsed, pane.as_ref()) else {
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
    --help, -h                       Show this help
    --version                        Show version

Environment:
    {variable}=<agent>       Change the default agent

Examples:
    aid blooop/devlaunch                       # Start {default} in the workspace
    aid blooop/devlaunch@fix/42 fix the bug    # Open the branch, hand over the prompt
    aid --gemini ./my-project explain this     # Pick a different agent
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
