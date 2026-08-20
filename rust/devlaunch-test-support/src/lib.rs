//! Dev-only test support (#251 §2): the single fake `Runner` (call recorder +
//! argv->response table + devpod workspace state machine — devpod_mock.py's
//! design re-homed), git fixtures, and scenario builders. A separate crate so
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

mod devpod;
mod fake_runner;
mod response;

pub use devpod::{DEFAULT_STAMP, DevpodMachine, FakeWorkspace, Source, WorkspaceState};
pub use fake_runner::{Call, FakeRunner, Scripted, Unscripted};
pub use response::Response;

#[cfg(test)]
mod tests {
    #[test]
    fn versioned_with_the_workspace() {
        assert_eq!(env!("CARGO_PKG_VERSION"), "0.3.0");
    }
}
