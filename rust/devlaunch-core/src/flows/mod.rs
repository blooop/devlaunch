// binary surface — not part of the frozen wf API (#251 §7)
pub mod agent_worktrees;
// binary surface — not part of the frozen wf API (#251 §7): the branch decision's
// refusal travels inside a launch refusal, and the words for it are the binary's.
pub mod branch_manager;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod completion;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod completion_cache;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod disk_usage;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod kept_copies;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod kill;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod launch;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod lifecycle;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod listing;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod migration;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod provision;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod records;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod repo_manager;
// Crate-private, like `clients::herdr` whose decisions it carries out: nothing
// outside core opens a session manager's socket, and what a launch made of it
// reaches the caller as a notice.
pub(crate) mod session_manager;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod workspace_clone;
