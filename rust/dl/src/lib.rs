//! `dl` as a library: the whole of the binary except the process it runs in.
//!
//! The binary is [`main.rs`](../main.rs) and holds three things — the SIGINT
//! disposition, the process's argv, and `exit()`. Everything else is here, because
//! `aid` is a second entry point into the *same* command line rather than a second
//! launcher: Python's `aid.py` rewrites its argv and calls `dl.main(dl_args)`
//! in-process, and [`run`] is that function. An `aid` that spawned `dl` instead
//! would be a second process, a second `devpod` resolution and a second timing
//! summary; an `aid` with launch logic of its own is the drift the Python module was
//! written to end.
//!
//! Five modules, and the boundary between them is the invariant (#251's invariant 1:
//! the binary holds nothing beyond parsing, rendering and interactive selection):
//!
//! - [`cli`] — the grammar. argv in, one `Command` out, pure.
//! - [`session`] — what one command holds: the runner, the cache directory, and
//!   the records when it needs them; [`cold`] is the lazily-opened records
//!   themselves, and [`target`] is which workspace a verb's target word names.
//! - [`commands`] — one `render_*` per command, and the exhaustive match;
//!   [`launch`] is the eight launch verbs' half of it, and [`select`] is the
//!   embedded picker that supplies the workspace when the command line named none.
//! - [`render`] — typed values in, bytes out. Every user-facing English word `dl`
//!   prints is written in that module or in [`commands`]; core holds none of it.

mod cli;
mod cold;
mod commands;
mod launch;
mod render;
mod select;
mod session;
mod target;

use devlaunch_core::flows::completion_cache;
use devlaunch_core::flows::lifecycle::{Refresh, RefreshReason};
use devlaunch_core::runner::ProcessRunner;
use devlaunch_core::timing;

/// `shlex.quote`, for the entry point that builds a `dl` command line out of its
/// own: `aid` reaches it through here rather than through `devlaunch-core`, so the
/// only thing it can see of devlaunch is `dl`'s command line and the quoting that
/// composes one.
pub use devlaunch_core::shell;

/// Python's `repr()`, for the entry point that quotes an untrusted name the way
/// Python did: `aid` names a bad `DEVLAUNCH_AID_AGENT` value with `{name!r}`, and
/// reaches the one renderer through here rather than carrying a second copy, so a
/// name holding a quote or a control byte is spelled the same as everywhere else
/// `dl` quotes what a tool or an environment said.
pub use render::python_repr;

/// The sentence a `--rm`/`--stop` appended to a line prints about what it
/// overrode: `aid` swallows the *prompt* when the suffix wins, so it owes the same
/// notice `dl` owes for a swallowed positional word, in the same words. See
/// [`render::overridden_notice`].
pub use render::overridden_notice;

/// The version both binaries print, single-sourced from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What both binaries append to [`VERSION`] to say which build this is.
///
/// Empty for everything that ships. `-dev` for a build of somebody's working
/// tree, which is what `./dev.sh` installs as `dl-next`/`aid-next` beside the
/// released pair (#268): two names on one PATH are only worth having if the
/// builds behind them are told apart by their output too, and a compiled binary
/// has no editable-install metadata to describe the way Python's did
/// (**divergence row 16**).
///
/// Gated on a cargo feature, so cargo rebuilds when it moves. It is deliberately
/// not read from the environment: `option_env!` is resolved at compile time
/// without cargo knowing, so a marker flipped that way leaves the previous
/// binary standing and mislabelled.
pub const BUILD_MARKER: &str = if cfg!(feature = "dev-build") {
    "-dev"
} else {
    ""
};

/// The code a `dl` killed by Ctrl-C exits with.
///
/// 128 + SIGINT, which is what a shell reports for a process the terminal
/// interrupted, and what Python's `except KeyboardInterrupt: sys.exit(130)`
/// produced. Reproduced rather than left to the default disposition because the
/// difference is observable: a process that dies *by* the signal has no exit code
/// at all, and a caller reading `Popen.returncode` sees `-2` where it used to see
/// `130`.
pub const INTERRUPTED: i32 = 130;

/// Install the SIGINT disposition both binaries share: a Ctrl-C exits
/// [`INTERRUPTED`] after cleaning up, rather than killing the process by signal.
///
/// The handler does the little a signal handler safely may:
/// [`devlaunch_runner::interrupt::cleanup_and_exit`] kills the foreground child's
/// process group (so a `devpod up` cannot outlive the run), `unlink`s the temp
/// files registered at their creation — chiefly the plaintext GitHub-token file a
/// Ctrl-C mid-`up` used to leave on disk — and then `_exit`s. Everything it calls
/// is async-signal-safe; see that module. The one thing still not done is the
/// timing summary of an interrupted run, which Python's unwinding
/// `KeyboardInterrupt` managed to write and which docs/rust-rewrite-plan.md row 5
/// says is not a parity dimension — it cannot be, as none of what it needs
/// (allocation, formatting, a lock) is safe here.
///
/// It lives in the library both entry points share rather than in either `main`
/// because `dl` and `aid` must install the *same* disposition: `aid` runs [`run`]
/// in-process, so a Ctrl-C during an `aid` launch stages and orphans exactly what a
/// `dl` launch does. Two copies in two `main`s drifted once — aid's stayed a bare
/// `_exit` that left the token file on disk and the `up` child running — which is
/// the drift a single definition ends.
pub fn install_interrupt_handler() {
    extern "C" fn interrupted(_signal: libc::c_int) {
        // SAFETY: called only as a signal handler; `cleanup_and_exit` is
        // async-signal-safe and never returns.
        unsafe { devlaunch_runner::interrupt::cleanup_and_exit(INTERRUPTED) }
    }
    // SAFETY: installing a handler for SIGINT before any thread is started. The
    // handler is `extern "C"` and does nothing but call the async-signal-safe
    // cleanup, which does not return.
    unsafe {
        libc::signal(libc::SIGINT, interrupted as *const () as libc::sighandler_t);
    }
}

/// Run one `dl` command line — the words after the program name — and say how it
/// ended.
///
/// Python's `dl.main(argv)`, and a parameter for its reason: a sibling entry point
/// hands dl a command line it built and gets dl's behaviour rather than a second
/// copy of it. The timing summary begins and ends inside this call, as Python's
/// `main()` does, so a second command in the same process gets a summary of its own.
pub fn run(argv: &[String]) -> i32 {
    // Timing is per-command: begin() here so a second run in the same process
    // starts a fresh summary, and the report is written however the command ended.
    timing::begin();
    let ending = one_command(argv);
    report_timing();
    ending
}

/// The command itself, between the timing summary's two ends.
fn one_command(argv: &[String]) -> i32 {
    // Before the command runs and before anything is parsed into: `dl --help` must
    // not pay for a refresh it has no use for, and the predicate is a pure
    // function of argv for exactly that reason. Asked here rather than after the
    // parse because Python asks it there too — a command line dl goes on to refuse
    // has still warmed the cache.
    let wanted =
        completion_cache::wants_startup_cache_refresh(&cli::argv_without_devcontainer(argv));

    // One process, one cache directory, and one background refresh. Both are
    // resolved out here rather than per command: two halves of one run that
    // resolved the cache separately could disagree about where it is, and two
    // `Refresh` values would spawn the child Python's process-wide latch spawns
    // once.
    let updater = session::self_invocation();
    match session::cache_dir() {
        Ok(cache) => {
            let cache_path = completion_cache::cache_path(&cache);
            let mut refresh = Refresh::new(&updater, &cache_path);
            if wanted {
                // Nothing is printed either way: a refresh nobody asked to see is
                // not news, and a child that could not be started costs
                // completions their freshness and nothing else.
                refresh.ask(&ProcessRunner, RefreshReason::IfStale);
            }
            match command_line(argv) {
                Err(ending) => ending,
                Ok(command) => {
                    commands::dispatch(&ProcessRunner, &cache, &mut refresh, command).code()
                }
            }
        }
        // No home directory and no `XDG_CACHE_HOME`: there is nowhere to warm and
        // nowhere to run most commands against, so the parse still happens (a
        // usage error is still a usage error) and the command says why.
        Err(_) => match command_line(argv) {
            Err(ending) => ending,
            Ok(command) => commands::without_a_cache_directory(command).code(),
        },
    }
}

/// The command this argv asks for, or the exit code the refusal already printed.
fn command_line(argv: &[String]) -> Result<cli::Command, i32> {
    // The program name clap wants at argv[0] is written here rather than taken from
    // the process: for `aid` the process is `aid`, and the usage line a refused `dl`
    // command line prints has to name the grammar it was refused by.
    let words = std::iter::once("dl".to_owned()).chain(argv.iter().cloned());
    let parsed = match <cli::Cli as clap::Parser>::try_parse_from(words) {
        Ok(parsed) => parsed,
        Err(usage) => {
            // clap's own exit codes: 0 when it printed help or a version, 2 for a
            // usage error. Printed through clap so `--help` reaches stdout and an
            // error reaches stderr, and handled here rather than by clap's
            // `parse()` so the timing summary still lands.
            let _ = usage.print();
            return Err(usage.exit_code());
        }
    };
    let resolved = cli::resolve(parsed, argv).map_err(|grammar| {
        eprintln!("{}", grammar_refusal(&grammar));
        // Python's `logging.error(...); return 1` for every shape it refused after
        // parsing. Deliberately not clap's 2: these are the refusals Python also
        // made, and they keep its code.
        commands::Ending::Refused.code()
    })?;
    // Printed before the command runs, not after: what it names is the reason a
    // person may want to hit Ctrl-C, and a notice that arrives once the container
    // is already gone is a receipt rather than a warning.
    if let Some(overridden) = &resolved.overridden {
        let words: Vec<String> = overridden.words.iter().cloned().collect();
        eprintln!("{}", overridden_notice(overridden.flag, &words));
    }
    Ok(resolved.command)
}

/// What a command line clap accepted but `dl` could not make a command of.
///
/// The two Python also refused keep Python's words, because they are the ones a
/// user has seen and a script may match on.
fn grammar_refusal(refused: &cli::GrammarError) -> String {
    use devlaunch_core::domain::spec::DevcontainerRefError;

    match refused {
        cli::GrammarError::UnknownVerb { target, word } => {
            format!("Unknown command '{word}'. Use 'dl {target} -- {word}' to run a shell command.")
        }
        // Both halves of the collision that retired the word, because a person who
        // typed one of them was reaching for one of the two and the sentence cannot
        // tell which. Divergence row 31.
        cli::GrammarError::RetiredVerb(retired) => format!(
            "'{}' is no longer a workspace verb. Use 'dl <workspace> {}' to delete a \
             workspace, or 'dl --prune' to remove the clone directories no workspace \
             opens any more.",
            retired.word(),
            retired.instead()
        ),
        cli::GrammarError::TargetNotAllowed { command } => {
            format!("{command} takes no workspace: it is not a workspace command.")
        }
        cli::GrammarError::ModifierNotAllowed { modifier, command } => {
            format!("{modifier} means nothing for {command}.")
        }
        cli::GrammarError::CommandNotAllowed { verb } => format!(
            "A shell command can only be run by 'dl <workspace> -- <command>', not with \
             '{verb}'."
        ),
        cli::GrammarError::DevcontainerNotAllowed { command } => {
            format!("--devcontainer means nothing for {command}: it opens no workspace.")
        }
        // The two forms it *does* apply to are named, and so is what to type to
        // delete a workspace now: somebody who reached for `--autorm` on another
        // verb wants the workspace gone at some point, and this sentence is where
        // they find out when. Deliberately no claim about *why* the verb refuses —
        // `restart`, `recreate` and `reset` do hand over a session, and the reason
        // they are out is that `--autorm` is defined for the throwaway workspace
        // rather than as a modifier on every verb that ends in a shell.
        cli::GrammarError::AutormNotAllowed { command } => format!(
            "--autorm applies to 'dl <workspace>' and 'dl <workspace> -- <command>', not to \
             {command}. Use 'dl <workspace> rm' to delete a workspace now."
        ),
        cli::GrammarError::AutormForced => {
            "--force does not apply to --autorm: an automatic removal always stops at work \
             that is nowhere else, which is what makes it safe to leave on a line you recall. \
             Use 'dl <workspace> rm --force' to delete one despite it."
                .to_owned()
        }
        cli::GrammarError::Devcontainer {
            raw,
            why: DevcontainerRefError::Missing,
        } => {
            let _ = raw;
            "--devcontainer requires a variant name or path".to_owned()
        }
        cli::GrammarError::Devcontainer {
            raw,
            why: DevcontainerRefError::FlagLike,
        } => format!(
            "--devcontainer needs a value, got the flag {}",
            render::python_repr(raw)
        ),
    }
}

/// Write the timing summary, if `DEVLAUNCH_TIMING` asked for one.
///
/// stderr, because stdout is parsed by the completion machinery (`--repos`,
/// `--completion-data`) and by `wf` (`--ls --json`).
fn report_timing() {
    if let Some(report) = timing::emit() {
        for line in report.lines() {
            eprintln!("{line}");
        }
    }
}

#[cfg(test)]
mod build_marker {
    //! What [`BUILD_MARKER`] is, asserted once per build rather than once.
    //!
    //! The two cases cannot both be compiled at once — the marker is a `cfg`, so a
    //! single build has exactly one value — and that is the point: the ordinary
    //! `cargo test` the gate runs proves the released half, and `./dev.sh`'s
    //! feature-on build proves the other. Neither half is asserted by a test that
    //! could not have failed.

    use super::{BUILD_MARKER, VERSION};

    #[test]
    #[cfg(not(feature = "dev-build"))]
    fn a_released_build_appends_nothing() {
        assert_eq!(
            BUILD_MARKER, "",
            "a build without `dev-build` is what ships, and it must print the bare version"
        );
    }

    #[test]
    #[cfg(feature = "dev-build")]
    fn a_working_tree_build_says_which_build_it_is() {
        assert_eq!(
            BUILD_MARKER, "-dev",
            "`./dev.sh` builds with `dev-build` so that `dl-next --version` differs from `dl`'s"
        );
    }

    #[test]
    fn the_version_itself_never_carries_a_marker() {
        // Otherwise the two could be conflated: a `-dev` written into
        // `rust/Cargo.toml` would mark every released artifact, and the assertion
        // above it would still pass.
        assert!(
            !VERSION.contains("-dev"),
            "the marker is the build's, not the version's; `rust/Cargo.toml` says {VERSION:?}"
        );
    }
}
