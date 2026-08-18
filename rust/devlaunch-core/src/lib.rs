//! devlaunch's engine: every operation behind the `dl` CLI, as a typed
//! library that `wf` links directly.
//!
//! Four layers, dependencies strictly downward (#251):
//!
//! 1. **runner** — the one seam to the outside: a spawn-spec in, an outcome
//!    out. The only place a process is started.
//! 2. **tool clients** — `devpod`, `git`, `gh`, `ssh`: typed argv builders
//!    and output parsers over the runner.
//! 3. **domain** — `workspace_id`, `spec`, `model`, `metadata`, `config`,
//!    `xdg`, `locks`: the data model, written once.
//! 4. **flows** — `launch`, `lifecycle`, `listing`, `provision`,
//!    `completion`, `disk_usage`, `timing`: the operations.
//!
//! The public API is the day-one wf consumption surface (#250) and nothing
//! else; no user-facing English lives here — text and exit codes are the
//! `dl` binary's rendering.

#[cfg(test)]
mod tests {
    #[test]
    fn the_workspace_compiles_and_tests_run() {
        assert_eq!(env!("CARGO_PKG_VERSION"), "0.1.0");
    }
}
