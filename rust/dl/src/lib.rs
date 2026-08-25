//! `dl` as a library: the whole of the binary except the process it runs in.
//!
//! The binary is [`main.rs`](../main.rs) and holds three things — the signal
//! dispositions, the process's argv, and `exit()`. Everything else is here, because
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

use devlaunch_core::clients::ssh;
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

/// `os.environ.get`, for the entry point that has environment variables of its
/// own: `aid` reads `DEVLAUNCH_AID_AGENT` through here rather than through
/// `std::env::var(..).ok()`, which reports a value that is not valid UTF-8 as
/// *unset* — so an undecodable agent name silently started the default agent
/// instead of being refused by name. The reading is core's, reached the same way
/// `shell` and `python_repr` are, because neither half of what makes it right —
/// the lossy decode, and treating present-but-undecodable as present — can be
/// spelled correctly outside `devlaunch_core::osext`.
pub use devlaunch_core::osext::env_str;

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

/// The code a `dl` cut short by signal `signal` exits with: **128 + the signal
/// number**, the convention a shell reports for a process a signal ended.
///
/// One rule for every signal in [`DRAINED`] rather than one constant each, because
/// the alternative — a code chosen per signal — is a table that has to be
/// remembered, and this one can be read off `kill -l`. It is also what a Ctrl-C
/// already exited with: Python's `except KeyboardInterrupt: sys.exit(130)` was
/// 128 + SIGINT written out long-hand.
///
/// The exit is reproduced rather than left to the default disposition because the
/// difference is observable: a process that dies *by* the signal has no exit code
/// at all, and a caller reading `Popen.returncode` sees `-2` where it used to see
/// `130`.
///
/// **This is not the same question as a child's status, and the repo answers the
/// two opposite ways on purpose.** `flows::launch::Session::exit_status` renders a
/// *remote program* killed by a signal as Python's negative `returncode`, and says
/// so in as many words — "rather than inventing 128+n". That is about reporting
/// what happened to somebody else's process, where Python had already fixed the
/// spelling and parity is the whole point. This is about what code `dl`'s own
/// death leaves behind, where Python had also already fixed the spelling — and
/// fixed it at 130. Each follows its own precedent; neither generalises to the
/// other.
const fn signalled(signal: i32) -> i32 {
    128 + signal
}

/// Whether a `SIG_IGN` disposition `dl` inherited at startup survives, for one
/// signal in [`DRAINED`].
///
/// Two named variants rather than a `bool`, because which one a signal gets is a
/// judgement about what an inherited ignore *means* coming from that signal, and
/// the two meanings are not two settings of one dial.
#[derive(Clone, Copy)]
enum InheritedIgnore {
    /// The inherited disposition wins: the signal is left ignored, reaches no
    /// handler, and ends nothing.
    Wins,
    /// The handler goes in regardless, so the signal drains even though it
    /// arrived already ignored.
    Loses,
}

/// The signals whose delivery runs the drain instead of ending `dl` where it
/// stands, and what an inherited `SIG_IGN` means for each. Every one of them says
/// "this run is over", and leaves `dl` holding a staged plaintext token and a live
/// `devpod up` child if nothing intervenes.
///
/// **SIGINT** is the terminal's Ctrl-C, and the one that was always handled. An
/// inherited ignore *loses* for it, which is to say Ctrl-C behaves exactly as it
/// did before SIGTERM and SIGHUP were added. That is not an oversight in the rule
/// below but the point of stating the rule per signal: a non-interactive shell
/// backgrounding a job hands its child an ignored SIGINT and SIGQUIT under POSIX
/// job control (measured: `SigIgn` `0x6`), and nobody typed anything to ask for
/// that. Honouring it would stop the drain for every `dl` launched as `… &` from a
/// script or a CI step — leaving the staged credential and the `devpod up` child
/// to outlive a run that was cancelled, in the case where the abandoned run is
/// least likely to be noticed.
///
/// **SIGTERM** is every orderly kill there is — a supervisor timing a run out, a
/// cancelled CI job, the shutdown sweep — and was the gap this closes: `kill <dl>`
/// leaked the exact pair the Ctrl-C handler exists to prevent. **SIGHUP** is the
/// terminal window closing, which leaked the same pair for the same reason,
/// unwatched: the window any complaint would have appeared in is the one that just
/// went away.
///
/// An inherited ignore *wins* for both, and there it is a statement rather than an
/// accident: something disarmed a signal this process had no handler for until
/// now, and `nohup dl …` disarms SIGHUP for the express purpose of outliving the
/// terminal. Draining on it would take that away.
///
/// SIGQUIT is deliberately absent: it means "die now and dump core", and a
/// handler that tidies up first is not what someone reaching for it asked for.
const DRAINED: [(libc::c_int, InheritedIgnore); 3] = [
    (libc::SIGINT, InheritedIgnore::Loses),
    (libc::SIGTERM, InheritedIgnore::Wins),
    (libc::SIGHUP, InheritedIgnore::Wins),
];

/// Install the signal disposition both binaries share: any of `DRAINED` exits
/// `signalled` with that signal's code after cleaning up, rather than killing
/// the process by signal.
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
/// (allocation, formatting, a lock) is safe here. For the same reason none of
/// these signals can run an `--rm` removal; see README's "How you exit decides
/// whether it fires".
///
/// **A SIGTERM or SIGHUP already ignored when `dl` started stays ignored; a SIGINT
/// does not.** `nohup dl …` sets SIGHUP to `SIG_IGN` and that disposition survives
/// the `exec` — it is how `nohup` works, and honouring it is the only way `dl` can
/// still outlive its terminal. The check is the classic POSIX idiom, applied to
/// exactly the two signals where an inherited ignore is a statement; `DRAINED`
/// has the argument for why SIGINT is not one of them, and gets read rather than
/// guessed at because the answer lives in that table.
///
/// Note what the idiom does *not* cover, since it is easy to over-credit: `disown`
/// and `setsid` set no `SIG_IGN` at all (measured). A disowned job survives its
/// terminal because the shell does not send it SIGHUP, and a `setsid` one because
/// it left the session — neither reaches this code, and neither needs to.
///
/// It lives in the library both entry points share rather than in either `main`
/// because `dl` and `aid` must install the *same* disposition: `aid` runs [`run`]
/// in-process, so a signal during an `aid` launch stages and orphans exactly what a
/// `dl` launch does. Two copies in two `main`s drifted once — aid's stayed a bare
/// `_exit` that left the token file on disk and the `up` child running — which is
/// the drift a single definition ends.
pub fn install_signal_handlers() {
    extern "C" fn drain(signal: libc::c_int) {
        // SAFETY: called only as a signal handler; `cleanup_and_exit` is
        // async-signal-safe and never returns. The code is derived from the
        // signal the kernel passed in, so one handler serves every signal in
        // `DRAINED` and none of them can be given the wrong code.
        unsafe { devlaunch_runner::interrupt::cleanup_and_exit(signalled(signal)) }
    }
    for (signal, inherited_ignore) in DRAINED {
        match inherited_ignore {
            // SAFETY: installing a handler before any thread is started. The
            // handler is `extern "C"` and does nothing but call the
            // async-signal-safe cleanup, which does not return.
            InheritedIgnore::Loses => unsafe {
                libc::signal(signal, drain as *const () as libc::sighandler_t);
            },
            // SAFETY: as above. The first call reads the inherited disposition —
            // `signal` returns the one it replaced — and the SIG_IGN it installs
            // to do so is the safe thing to be holding in the meantime, since the
            // only window it widens is one in which the signal is ignored.
            InheritedIgnore::Wins => unsafe {
                let inherited = libc::signal(signal, libc::SIG_IGN);
                if inherited != libc::SIG_IGN {
                    libc::signal(signal, drain as *const () as libc::sighandler_t);
                }
            },
        }
    }
}

/// Whether this process is talking to a person at a terminal.
///
/// stdin *and* stdout, because the question is asked before reading a line the
/// user is expected to see themselves type: a piped stdin has no one typing, and
/// a piped stdout gives them no echo to type against. `DEVLAUNCH_NO_TTY` is the
/// same escape hatch it is for the ssh transport — set to anything but a falsey
/// value it means "behave as if there were no terminal", so one variable turns
/// off everything dl and aid only do on one.
///
/// Exported for `aid`, whose interactive prompt is gated on it: aid's one
/// dependency is `dl`, so the isatty call lives here rather than giving aid a
/// libc of its own.
pub fn interactive_terminal() -> bool {
    // SAFETY: `isatty` reads a property of the descriptor and touches nothing.
    let tty = unsafe { libc::isatty(0) == 1 && libc::isatty(1) == 1 };
    // Core's reading, not a copy of it. The copy that used to live here answered
    // `std::env::var(..).ok()` and a bare `matches!` over the falsey words, so
    // `FALSE`, ` no ` and a non-UTF-8 value each meant one thing to the ssh
    // transport and the other to the prompt below.
    tty && !ssh::tty_disabled_by_environment()
}

/// Read one submission from a cooked-mode terminal: the line the user ends with
/// Enter, plus whatever input was already buffered at that moment — a multi-line
/// paste — joined as the newlines it arrived with. Empty on a bare Enter or an
/// immediate Ctrl-D.
///
/// The read is byte-by-byte from descriptor 0 rather than through
/// `std::io::stdin()`, and that is load-bearing twice over. First, `Stdin`'s
/// buffer would swallow the rest of a paste where nothing can see it — a
/// zero-timeout `poll` on the descriptor answers for the kernel's queue, not for
/// bytes a `BufRead` already took. Second, whatever this function does not
/// consume stays in the terminal's queue for the *next* process to inherit — the
/// agent session `aid` goes on to attach — so pasted lines that were not drained
/// here would land inside the agent as keystrokes.
///
/// Cooked mode is also why this is safe to call and abandon: no raw mode is
/// entered, so a Ctrl-C mid-read leaves the terminal exactly as it found it.
pub fn read_terminal_submission() -> String {
    let mut bytes: Vec<u8> = Vec::new();
    // The line itself: up to Enter, or EOF (Ctrl-D on an empty line reads 0).
    let mut ended_with_newline = false;
    loop {
        match read_stdin_byte() {
            None => break,
            Some(b'\n') => {
                ended_with_newline = true;
                break;
            }
            Some(byte) => bytes.push(byte),
        }
    }
    // The paste tail: the newline-terminated lines the terminal already holds.
    // Only what is *already* queued — the zero timeout is what keeps a person
    // who typed one line from being waited on for a second — and in cooked mode
    // that is only *completed* lines: a final fragment a paste left unterminated
    // is not yet readable, stays queued, and reaches the agent's session as
    // typed-ahead input. The Enter that ended the first line was consumed above,
    // so it is put back before the tail or the first two lines would be glued
    // into one word.
    if ended_with_newline && stdin_readable_now() {
        bytes.push(b'\n');
        while stdin_readable_now() {
            match read_stdin_byte() {
                None => break,
                Some(byte) => bytes.push(byte),
            }
        }
    }
    String::from_utf8_lossy(&bytes).trim_end().to_owned()
}

/// One byte from descriptor 0, or `None` on EOF or an unreadable stdin.
fn read_stdin_byte() -> Option<u8> {
    let mut byte: u8 = 0;
    loop {
        // SAFETY: reading one byte into a stack buffer of that size.
        let read = unsafe { libc::read(0, std::ptr::from_mut(&mut byte).cast(), 1) };
        match read {
            1 => return Some(byte),
            0 => return None,
            // A signal that did not kill the process (SIGWINCH, a stopped and
            // resumed job) interrupts the read without ending the input.
            _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {}
            _ => return None,
        }
    }
}

/// Whether descriptor 0 has bytes to read right now, without waiting for any.
fn stdin_readable_now() -> bool {
    let mut asked = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: polling one descriptor with a zero timeout; the struct outlives the
    // call.
    let ready = unsafe { libc::poll(&mut asked, 1, 0) };
    ready > 0 && (asked.revents & libc::POLLIN) != 0
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
    cli::resolve(parsed, argv).map_err(|grammar| {
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
        // Divergence row 32. Each names the spelling that replaced it, because both
        // moved on account of `--rm` changing meaning and neither person typing one
        // can be assumed to know that yet.
        cli::GrammarError::RetiredFlag(cli::RetiredFlag::Stop) => {
            "--stop is no longer a flag: the flag spellings now modify a session \
             (--rm deletes the workspace once one ends) rather than name a verb. Use \
             'dl <workspace> stop' to stop a workspace."
                .to_owned()
        }
        cli::GrammarError::RetiredFlag(cli::RetiredFlag::Autorm) => {
            "--autorm is now spelled --rm: 'dl <workspace> --rm' opens the workspace and \
             deletes it when the session ends, the way 'docker run --rm' does. Use \
             'dl <workspace> rm' to delete one now."
                .to_owned()
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
        // The two forms it *does* apply to are named, and so is what to type to
        // delete a workspace now: somebody who reached for `--rm` on another verb
        // wants the workspace gone at some point, and this sentence is where they
        // find out when. Deliberately no claim about *why* the verb refuses —
        // `restart`, `recreate` and `reset` do hand over a session, and the reason
        // they are out is that `--rm` is defined for the throwaway workspace rather
        // than as a modifier on every verb that ends in a shell.
        //
        // It has to read correctly for `command` = `rm`, which `dl <ws> rm --rm`
        // reaches, and it does: the flag is not the verb, and the verb alone is
        // already the answer the sentence points at.
        cli::GrammarError::RmNotAllowed { command } => format!(
            "--rm deletes the workspace when a session ends, so it applies to \
             'dl <workspace>' and 'dl <workspace> -- <command>', not to '{command}'. Use \
             'dl <workspace> rm' to delete a workspace now."
        ),
        cli::GrammarError::RmForced => {
            "--force does not apply to --rm: a removal that waits for the session always \
             stops at work that is nowhere else, which is what makes it safe to leave on a \
             line you recall. Use 'dl <workspace> rm --force' to delete one despite it."
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

#[cfg(test)]
mod drained_signals {
    //! A guard on the handled-signal set, and the honest limit of what it buys.
    //!
    //! Each signal's *behaviour* is proved at the binary boundary by
    //! `INHERITED_IGNORE` in `dl/tests/interrupt.rs` — which is a second,
    //! independent list. It has to be: an integration test is a separate crate, so
    //! it cannot see [`DRAINED`] at all (measured: `constant DRAINED is private`),
    //! and giving it sight would mean `pub`-ing an implementation detail of a crate
    //! whose surface is deliberately small. So nothing can machine-check that the
    //! two lists *agree*.
    //!
    //! What this guard buys instead is that the set cannot be extended in
    //! **silence**: a fourth signal added to [`DRAINED`] fails the assertion below,
    //! and the failure names the file whose table has to grow with it. That is the
    //! whole claim — "cannot be added without being told what else to edit", not
    //! "cannot disagree".
    //!
    //! What still slips through, said plainly so nobody reads more into it: an
    //! author who edits this expectation *and* [`DRAINED`] together and still leaves
    //! the boundary table alone. That is a deliberate act rather than an oversight —
    //! the class of mistake a tripwire cannot catch and review can — and accepting
    //! it is what keeps this at four lines instead of a `pub`.

    use super::{DRAINED, InheritedIgnore};

    #[test]
    fn the_handled_set_cannot_grow_without_the_boundary_table_growing_too() {
        let held: Vec<(libc::c_int, bool)> = DRAINED
            .iter()
            .map(|(signal, inherited)| (*signal, matches!(inherited, InheritedIgnore::Wins)))
            .collect();
        assert_eq!(
            held,
            vec![
                (libc::SIGINT, false),
                (libc::SIGTERM, true),
                (libc::SIGHUP, true),
            ],
            "the handled signals changed. Add or remove the matching row in \
             `INHERITED_IGNORE` in `dl/tests/interrupt.rs`, which proves each \
             one's behaviour at the binary boundary, and only then update this \
             expectation."
        );
    }
}
