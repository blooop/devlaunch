// binary surface — not part of the frozen wf API (#251 §7)
pub mod devpod;
// devpod-the-filesystem, where `devpod` above is devpod-the-command.
// binary surface — not part of the frozen wf API (#251 §7)
pub mod devpod_home;
// Crate-private, unlike its siblings: nothing outside core names docker, and the
// one thing devlaunch asks it (remove these volumes) is reported through
// `flows::lifecycle`'s own vocabulary rather than docker's.
pub(crate) mod docker;
// Crate-private for the same reason: the two signals `dl <ws> kill` sends are
// the whole of what devlaunch asks of `kill(1)`, and what they came to is
// reported in the kill flow's vocabulary.
pub(crate) mod kill;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod gh;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod git;
// Crate-private for docker's reason: nothing outside core reads the host's
// process table, and what `dl <ws> kill` found there is reported in the kill
// flow's vocabulary rather than in `ps`'s columns.
pub(crate) mod ps;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod ssh;
