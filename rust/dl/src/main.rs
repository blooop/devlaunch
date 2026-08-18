//! The `dl` binary: clap definitions -> one call into devlaunch-core's API ->
//! rendering of typed results to text and exit codes (plus interactive
//! selection and completion-cache writing). Nothing else is allowed to live
//! here (#251's invariant 1).

fn main() {
    // TODO(M5, docs/rust-rewrite-plan.md): the clap grammar and rendering
    // land with the first read-side flows. Until then the binary exists so
    // the workspace, parity plumbing, and CI have something to build.
}
