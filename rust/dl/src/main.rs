//! The `dl` binary: three things the library cannot do for itself.
//!
//! The command line, the rendering and the flows are [`dl`]'s — the same library
//! `aid` runs, so the two entry points cannot drift. What is left here is what
//! belongs to a *process*: the signal dispositions, the argv it was started with,
//! and the exit code it leaves behind.

use std::io::Write as _;

fn main() {
    dl::install_signal_handlers();
    // `args_os` and a lossy decode, not `args`: `std::env::args()` panics on an
    // argument that is not valid UTF-8, which would end `dl $'\xff'` with an exit
    // 101 and a traceback. Python decoded argv lossily and carried on, and
    // docs/rust-rewrite-plan.md row 4 forbids the traceback; row 12 licenses the
    // lossy render.
    let mut args = std::env::args_os();
    // argv[0] is a fact about the process rather than about the command line, so
    // it is read here and turned into an ordinary flag: `dl-herdr-shell` is the
    // name herdr's `default_shell` can hold, and `--herdr-shell` is what dl
    // understands. One code path below this line, whichever name was typed.
    let program = args.next();
    let mut argv: Vec<String> = args.map(|arg| arg.to_string_lossy().into_owned()).collect();
    if dl::invoked_as_pane_shell(program.as_deref()) {
        argv.insert(0, "--herdr-shell".to_owned());
    }
    let ending = dl::run(&argv);
    // `process::exit` runs no destructors, and one of the things not run is the
    // flush of a stdout that ended without a newline.
    let _ = std::io::stdout().flush();
    std::process::exit(ending);
}
