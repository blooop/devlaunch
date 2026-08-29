//! Dev-only test support (#251 §2): the single fake `Runner` (call recorder +
//! argv->response table + devpod workspace state machine — the design of the
//! Python `devpod_mock` fixture, re-homed before that tree retired in #267),
//! git fixtures, and scenario builders. A separate crate so
//! nothing test-only can leak into shipped builds; importable by core's
//! tests, dl's tests, and wf's contract tests.
//!
//! ```no_run
//! use devlaunch_runner::{Invocation, Runner, SpawnSpec};
//! use devlaunch_test_support::{FakeRunner, Response, WorkspaceState};
//!
//! let fake = FakeRunner::new()
//!     .with_stopped("owner-repo-main")
//!     .with_script(["devpod", "up", "--id", "owner-repo-broken"], Response::failed(1, "boom\n"));
//!
//! // …hand `&fake` to the code under test, then read it back:
//! let spec = SpawnSpec::from(Invocation::new("devpod").with_args(["ssh", "owner-repo-main"]));
//! fake.capture(&spec);
//!
//! assert_eq!(fake.args_to("devpod"), [["ssh", "owner-repo-main"]]);
//! assert_eq!(fake.state_of("owner-repo-main"), Some(WorkspaceState::Running));
//! ```

mod cache_tree;
mod coverage;
mod devpod;
mod fake_runner;
mod response;

pub use cache_tree::{VOLATILE, cache_fingerprint, cache_shape};
pub use coverage::KeepingCoverage;
pub use devpod::{DEFAULT_STAMP, DevpodMachine, FakeWorkspace, Source, WorkspaceState};
pub use fake_runner::{Call, FakeRunner, Scripted, Unscripted};
pub use response::Response;

#[cfg(test)]
mod tests {
    /// This crate carries the workspace's version rather than one of its own.
    ///
    /// Asserted against `Cargo.toml` rather than against a literal. A literal is
    /// the same claim written in a way that has to be hand-edited every release
    /// and fails nothing until someone runs the one command that reaches it:
    /// `version.workspace = true` is what makes `CARGO_PKG_VERSION` the
    /// workspace's, so comparing that variable to a hardcoded string only ever
    /// tests whether the string was updated. It was not, at 0.3.1 --
    /// `cargo test --workspace` failed on a clean tree, and CI never saw it,
    /// because its rust job names the crates it tests one at a time and this is
    /// not among them.
    #[test]
    fn versioned_with_the_workspace() {
        let manifest = include_str!("../Cargo.toml");
        assert!(
            manifest
                .lines()
                .any(|line| line.split_whitespace().collect::<String>() == "version.workspace=true"),
            "devlaunch-test-support must take the workspace version, not name one of its own:\n{manifest}"
        );
    }
}
