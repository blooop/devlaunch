//! Keeping an instrumented child's coverage counters when its environment is cleared.
//!
//! Every suite that judges `dl` or `aid` at the binary boundary builds the child
//! with `env_clear()`, and it has to: what those tests pin is what the binary does
//! given exactly the `HOME`, `PATH` and `XDG_*` the world under test provides, and
//! a variable leaking in from the developer's shell is a test that passes on one
//! machine. `LLVM_PROFILE_FILE` is the one exception, and this is the seam that
//! makes it one.
//!
//! # What it costs to forget
//!
//! A binary built with `-C instrument-coverage` writes its counters at exit to the
//! path in `LLVM_PROFILE_FILE`, and to `default_*.profraw` in its working
//! directory when that variable is unset. `cargo llvm-cov` only reduces the
//! `.profraw` files under its own target directory, so an `env_clear()`ed child
//! writes a file nothing reads, in a directory nobody cleans, and contributes
//! *nothing* to the report.
//!
//! That is not a small loss and it does not look like one. Measured on this tree
//! before this seam existed: `cargo llvm-cov -p dl --test grammar` ran five tests
//! that each spawn the real `dl`, passed, and reported `render.rs`, `commands.rs`
//! and `cli.rs` at **0.00%** — a number that reads as "the binary's whole front end
//! is untested" when what actually happened is that the tests ran it and the
//! counters were thrown away. A coverage report that under-counts the suite it is
//! measuring is worse than no report: it invites someone to write tests for lines
//! that already have them.
//!
//! # Why forwarding it is safe
//!
//! `cargo llvm-cov` sets the variable to a pattern, not a filename —
//! `<target>/rust-%p-%32m.profraw`, where `%p` is the writing process's pid. So a
//! parent and every child it spawns can share the value and still each write their
//! own file; there is nothing to allocate per child and nothing to collide over.
//!
//! Outside a coverage run the variable is unset, [`KeepingCoverage`] adds nothing,
//! and the child's environment is exactly the cleared one the test asked for.

use std::process::Command;

/// The one variable this adds back, and the only one it is allowed to.
const PROFILE_FILE: &str = "LLVM_PROFILE_FILE";

/// Re-admit the coverage instrumentation to a child whose environment was cleared.
pub trait KeepingCoverage {
    /// Forward `LLVM_PROFILE_FILE` if this process has one, and nothing otherwise.
    ///
    /// Call it *after* `env_clear()` and before the variables the test is setting
    /// deliberately — it is one `env` among them, and order only matters relative
    /// to the clear:
    ///
    /// ```no_run
    /// use std::process::Command;
    /// use devlaunch_test_support::KeepingCoverage;
    ///
    /// Command::new("dl")
    ///     .env_clear()
    ///     .keeping_coverage()
    ///     .env("HOME", "/tmp/scratch/home")
    ///     .status()
    ///     .expect("dl runs");
    /// ```
    fn keeping_coverage(&mut self) -> &mut Self;
}

impl KeepingCoverage for Command {
    fn keeping_coverage(&mut self) -> &mut Self {
        match std::env::var_os(PROFILE_FILE) {
            Some(destination) => self.env(PROFILE_FILE, destination),
            // Not under a coverage run. Adding an empty value here would be worse
            // than adding nothing: LLVM reads `LLVM_PROFILE_FILE=""` as a request
            // to write to a file with no name.
            None => self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both halves, in one test on purpose.
    ///
    /// `LLVM_PROFILE_FILE` is process-global, so two tests that each set and unset
    /// it are two tests that cannot run beside each other — and `cargo test`
    /// parallelises within a binary by default. One test that walks both cases in
    /// order has no such window, and needs no `--test-threads=1` from whoever runs
    /// it to be true.
    ///
    /// Both halves are asserted through a real spawn rather than by reading the
    /// `Command` back, because a `Command`'s environment cannot be read back — and
    /// the question is what the child sees, which is what a spawn answers.
    #[test]
    fn a_cleared_child_gets_the_profile_destination_and_nothing_else() {
        let destination = "/tmp/devlaunch-test-support-%p.profraw";
        // Under a coverage run this variable is already set, to the pattern this
        // whole module exists to forward. Put it back before returning: the
        // profile runtime reads it once at process start, so moving it cannot
        // cost this binary its own counters, but a test that leaves a global
        // where it did not find it is a test that breaks the next one.
        let ours = std::env::var_os(PROFILE_FILE);

        // Safety: `std::env::set_var` is unsound only against a concurrent reader
        // in another thread. This test holds the only window in which the variable
        // moves, and the crate's other tests never read it.
        unsafe { std::env::set_var(PROFILE_FILE, destination) };
        let under_coverage = cleared_child_environment();
        unsafe { std::env::remove_var(PROFILE_FILE) };
        let without = cleared_child_environment();
        if let Some(ours) = ours {
            unsafe { std::env::set_var(PROFILE_FILE, ours) };
        }

        assert_eq!(
            under_coverage,
            format!("{PROFILE_FILE}={destination}\n"),
            "the cleared child's environment is not exactly the forwarded variable"
        );
        // The negative half matters as much as the positive one: these suites pin
        // what a binary does in a known environment, so a helper that put a
        // variable there unconditionally would be changing the thing under test.
        assert_eq!(
            without, "",
            "something reached a child whose environment was cleared"
        );
    }

    /// What a child sees, given nothing but [`KeepingCoverage`].
    fn cleared_child_environment() -> String {
        let mut command = Command::new("/usr/bin/env");
        command.env_clear().keeping_coverage();
        let seen = command.output().expect("/usr/bin/env runs");
        String::from_utf8_lossy(&seen.stdout).into_owned()
    }
}
