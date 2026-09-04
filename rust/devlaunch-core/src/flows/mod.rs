// binary surface — not part of the frozen wf API (#251 §7)
pub mod agent_worktrees;
// binary surface — not part of the frozen wf API (#251 §7): the branch decision's
// refusal travels inside a launch refusal, and the words for it are the binary's.
pub mod branch_manager;
// binary surface — not part of the frozen wf API (#251 §7)
/// The Claude logins this host can forward, with the account behind each name.
pub mod claude_profiles;

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
pub mod launch_locks;
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
// binary surface — not part of the frozen wf API (#251 §7), and only just: the
// reporting half is crate-private like `clients::herdr` whose decisions it
// carries out, because nothing outside core opens a session manager's socket and
// what a launch made of one reaches the caller as a notice. `pane_destination` is
// the exception and the reason this is `pub` — `dl --herdr-shell` is a command of
// its own rather than part of a launch, and what crosses the crate boundary for
// it is one decision and no socket.
pub mod session_manager;
// binary surface — not part of the frozen wf API (#251 §7)
pub mod workspace_clone;
