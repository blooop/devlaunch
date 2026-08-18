//! Dev-only test support (#251 §2): the single fake `Runner` (call recorder +
//! argv->response table + devpod workspace state machine — devpod_mock.py's
//! design re-homed), git fixtures, and scenario builders. A separate crate so
//! nothing test-only can leak into shipped builds; importable by core's
//! tests, dl's tests, and wf's contract tests.

#[cfg(test)]
mod tests {
    #[test]
    fn versioned_with_the_workspace() {
        assert_eq!(env!("CARGO_PKG_VERSION"), "0.1.0");
    }
}
