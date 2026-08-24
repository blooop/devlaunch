// binary surface — not part of the frozen wf API (#251 §7)
pub mod devpod;
// devpod-the-filesystem, where `devpod` above is devpod-the-command.
// binary surface — not part of the frozen wf API (#251 §7)
pub mod devpod_home;
// Crate-private, unlike its siblings: nothing outside core names docker, and the
// one thing devlaunch asks it (remove these volumes) is reported through
// `flows::lifecycle`'s own vocabulary rather than docker's.
pub(crate) mod docker;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod gh;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod git;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod ssh;
