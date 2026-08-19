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

mod rewrite;

use std::io::Write as _;

use rewrite::UsageError;

fn main() {
    interrupt_exits_130();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let ending = run(&argv);
    // `process::exit` runs no destructors, and one of the things not run is the
    // flush of a stdout that ended without a newline.
    let _ = std::io::stdout().flush();
    std::process::exit(ending);
}

/// One `aid` command line: the three answers aid gives on its own, and dl for the
/// rest.
fn run(argv: &[String]) -> i32 {
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
        // `aid <version>`, the version dl prints under aid's name. **Divergence row
        // 16**: Python appended an editable install's provenance, which a compiled
        // binary has none of.
        println!("aid {}", dl::VERSION);
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
    let Some(dl_args) = rewrite::build_dl_args(&parsed) else {
        // Unreachable by a command line: the parse only ever answers with an agent
        // from the table. Reported rather than panicked on, in the words the refusal
        // for an invented name would have used.
        eprintln!(
            "{}",
            refusal(&UsageError::UnknownAgentInEnvironment {
                name: parsed.agent.clone()
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
        // `{name!r}` in Python, which single-quotes a string: written out here
        // rather than reached for through dl's renderer, because aid's only view of
        // dl is its command line.
        UsageError::UnknownAgentInEnvironment { name } => format!(
            "{}='{name}' is not a known agent. Choose one of: {}.",
            rewrite::AGENT_ENV_VAR,
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

Options:
    {agents}
                                     Pick the agent (default: {default})
    --devcontainer <variant|path>    Passed through to dl
    --help, -h                       Show this help
    --version                        Show version

Environment:
    {variable}=<agent>       Change the default agent

Examples:
    aid blooop/devlaunch                       # Start {default} in the workspace
    aid blooop/devlaunch@fix/42 fix the bug    # Open the branch, hand over the prompt
    aid --gemini ./my-project explain this     # Pick a different agent

Everything else — listing, stopping, deleting, VS Code — is dl's job:
    dl --help\n\n"
    )
}

/// Make Ctrl-C exit 130 rather than killing this process by signal.
///
/// `dl`'s own disposition, under aid's name and for aid's reason: `python -m
/// devlaunch.aid` caught `KeyboardInterrupt` and exited 130, and an agent session a
/// user interrupts is the commonest way this binary ends.
fn interrupt_exits_130() {
    extern "C" fn interrupted(_signal: libc::c_int) {
        // SAFETY: `_exit` is async-signal-safe; it makes the exit-status syscall and
        // returns to no one.
        unsafe { libc::_exit(dl::INTERRUPTED) }
    }
    // SAFETY: installing a handler for SIGINT before any thread is started. The
    // handler is `extern "C"` and does nothing but `_exit`.
    unsafe {
        libc::signal(libc::SIGINT, interrupted as *const () as libc::sighandler_t);
    }
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
