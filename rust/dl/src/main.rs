//! The `dl` binary: three things the library cannot do for itself.
//!
//! The command line, the rendering and the flows are [`dl`]'s — the same library
//! `aid` runs, so the two entry points cannot drift. What is left here is what
//! belongs to a *process*: the SIGINT disposition, the argv it was started with, and
//! the exit code it leaves behind.

use std::io::Write as _;

fn main() {
    interrupt_exits_130();
    // `args_os` and a lossy decode, not `args`: `std::env::args()` panics on an
    // argument that is not valid UTF-8, which would end `dl $'\xff'` with an exit
    // 101 and a traceback. Python decoded argv lossily and carried on, and
    // docs/rust-rewrite-plan.md row 4 forbids the traceback; row 12 licenses the
    // lossy render.
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let ending = dl::run(&argv);
    // `process::exit` runs no destructors, and one of the things not run is the
    // flush of a stdout that ended without a newline.
    let _ = std::io::stdout().flush();
    std::process::exit(ending);
}

/// Make Ctrl-C exit [`dl::INTERRUPTED`] rather than killing this process by signal.
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
        unsafe { libc::_exit(dl::INTERRUPTED) }
    }
    // SAFETY: installing a handler for SIGINT before any thread is started. The
    // handler is `extern "C"` and does nothing but `_exit`.
    unsafe {
        libc::signal(libc::SIGINT, interrupted as *const () as libc::sighandler_t);
    }
}
