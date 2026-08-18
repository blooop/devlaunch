//! The `dl` binary: clap definitions -> one call into devlaunch-core's API ->
//! rendering of typed results to text and exit codes (plus interactive
//! selection and completion-cache writing). Nothing else is allowed to live
//! here (#251's invariant 1).
//!
//! Four modules, and the boundary between them is the invariant:
//!
//! - [`cli`] — the grammar. argv in, one `Command` out, pure.
//! - [`session`] — what one command holds: the runner, the cache directory, and
//!   the records when it needs them; [`cold`] is the lazily-opened records
//!   themselves, and [`target`] is which workspace a verb's target word names.
//! - [`commands`] — one `render_*` per command, and the exhaustive match;
//!   [`launch`] is the eight launch verbs' half of it.
//! - [`render`] — typed values in, bytes out. Every user-facing English word `dl`
//!   prints is written in this module or in [`commands`]; core holds none of it.

mod cli;
mod cold;
mod commands;
mod launch;
mod render;
mod session;
mod target;

use std::io::Write as _;

use devlaunch_core::flows::completion_cache;
use devlaunch_core::flows::lifecycle::{Refresh, RefreshReason};
use devlaunch_core::runner::ProcessRunner;
use devlaunch_core::timing;

/// The code a `dl` killed by Ctrl-C exits with.
///
/// 128 + SIGINT, which is what a shell reports for a process the terminal
/// interrupted, and what Python's `except KeyboardInterrupt: sys.exit(130)`
/// produced. Reproduced rather than left to the default disposition because the
/// difference is observable: a process that dies *by* the signal has no exit code
/// at all, and a caller reading `Popen.returncode` sees `-2` where it used to see
/// `130`.
const INTERRUPTED: i32 = 130;

fn main() {
    interrupt_exits_130();
    // Timing is per-command: begin() here so a second run in the same process
    // starts a fresh summary, and the report is written however the command ended.
    timing::begin();
    let ending = run();
    report_timing();
    // `process::exit` runs no destructors, and one of the things not run is the
    // flush of a stdout that ended without a newline.
    let _ = std::io::stdout().flush();
    std::process::exit(ending);
}

fn run() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // Before the command runs and before anything is parsed into: `dl --help` must
    // not pay for a refresh it has no use for, and the predicate is a pure
    // function of argv for exactly that reason. Asked here rather than after the
    // parse because Python asks it there too — a command line dl goes on to refuse
    // has still warmed the cache.
    let wanted =
        completion_cache::wants_startup_cache_refresh(&cli::argv_without_devcontainer(&argv));

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
            match command_line() {
                Err(ending) => ending,
                Ok(command) => {
                    commands::dispatch(&ProcessRunner, &cache, &mut refresh, command).code()
                }
            }
        }
        // No home directory and no `XDG_CACHE_HOME`: there is nowhere to warm and
        // nowhere to run most commands against, so the parse still happens (a
        // usage error is still a usage error) and the command says why.
        Err(_) => match command_line() {
            Err(ending) => ending,
            Ok(command) => commands::without_a_cache_directory(command).code(),
        },
    }
}

/// The command this argv asks for, or the exit code the refusal already printed.
fn command_line() -> Result<cli::Command, i32> {
    let parsed = match <cli::Cli as clap::Parser>::try_parse_from(std::env::args_os()) {
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
    cli::resolve(parsed).map_err(|grammar| {
        eprintln!("{}", grammar_refusal(&grammar));
        // Python's `logging.error(...); return 1` for every shape it refused after
        // parsing. Deliberately not clap's 2: these are the refusals Python also
        // made, and they keep its code.
        commands::Ending::Refused.code()
    })
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

/// Make Ctrl-C exit [`INTERRUPTED`] rather than killing this process by signal.
///
/// The handler cannot do anything but exit: almost nothing is safe to call from a
/// signal handler, `_exit` included in the little that is. The cost is the timing
/// summary of an interrupted run, which Python's unwinding `KeyboardInterrupt`
/// still managed to write — and which docs/rust-rewrite-plan.md row 5 says is not
/// a parity dimension.
fn interrupt_exits_130() {
    extern "C" fn interrupted(_signal: libc::c_int) {
        // SAFETY: `_exit` is async-signal-safe; it makes the exit-status syscall
        // and returns to no one.
        unsafe { libc::_exit(INTERRUPTED) }
    }
    // SAFETY: installing a handler for SIGINT before any thread is started. The
    // handler is `extern "C"` and does nothing but `_exit`.
    unsafe {
        libc::signal(libc::SIGINT, interrupted as *const () as libc::sighandler_t);
    }
}
