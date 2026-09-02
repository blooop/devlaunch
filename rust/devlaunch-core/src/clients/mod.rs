// Crate-private, like `docker` and unlike the rest: nothing outside core names
// the Claude login, and what it produces reaches a caller as flags and an
// environment through `flows::launch`.
pub(crate) mod claude;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod devpod;
// devpod-the-filesystem, where `devpod` above is devpod-the-command.
// binary surface — not part of the frozen wf API (#251 §7)
pub mod devpod_home;
// Crate-private, unlike its siblings: nothing outside core names docker, and the
// three things devlaunch asks it — remove these volumes, list what one compose
// project has running, kill those containers — are reported in the vocabulary of
// the flow that asked (`flows::lifecycle`, `flows::kill`) rather than docker's.
pub(crate) mod docker;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod gh;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod git;
// Crate-private, like `claude` and unlike the rest, but for one list: nothing
// outside core builds the environment a session manager reads, while
// `AGENT_NAMES` is the single copy of which agents devlaunch knows by name and
// `aid`'s richer table is held against it.
pub(crate) mod herdr;
// Crate-private for docker's reason: the two signals `dl <ws> kill` sends are
// the whole of what devlaunch asks of `kill(1)`, and what they came to is
// reported in the kill flow's vocabulary.
pub(crate) mod kill;
// Crate-private for docker's reason: nothing outside core reads the host's
// process table, and what `dl <ws> kill` found there is reported in the kill
// flow's vocabulary rather than in `ps`'s columns.
pub(crate) mod ps;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod ssh;
