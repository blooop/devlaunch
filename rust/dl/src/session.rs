//! The two answers only *this* process can give: where devlaunch keeps its
//! things, and how to re-run this build.
//!
//! The records themselves — the config, `metadata.json`, the cache migration and
//! the clone manager — used to live here too, and moved to
//! [`devlaunch_core::flows::records`] in #340. They were never dl's knowledge: the
//! whole module was core types plumbed together, and keeping the plumbing in the
//! binary meant `devlaunch_core::api` promised a launcher that only `dl` could
//! build. What is left is what genuinely belongs to a running program rather than
//! to the library: `current_exe()`, and the cache path everything else is handed.

use std::path::PathBuf;

use devlaunch_core::domain::xdg::{self, NoHomeDirectory};
use devlaunch_core::flows::lifecycle::SelfInvocation;

/// Where devlaunch keeps everything: the directory ownership is decided by and
/// `--purge` removes.
///
/// The answer comes from `xdg` so that this, the clone root and `metadata.json`'s
/// default path cannot drift apart — ownership decides what `--purge` may delete
/// by asking whether a workspace's source is under this directory, and the clones
/// it is asking about were put there by the other two. (`flows::completion_cache`
/// carried a second copy of this call until the port finished and there was one
/// caller left.)
///
/// It is now the *only* input to the other two: the clone root is
/// `xdg::clone_root_in` of this, and nothing a user can write moves it (#467).
pub(crate) fn cache_dir() -> Result<PathBuf, NoHomeDirectory> {
    xdg::devlaunch_cache()
}

/// How to re-run *this* build as a detached child.
///
/// `current_exe()` is asked here and nowhere in core: a library that asked the OS
/// who it is would answer `wf` when wf links it and `python` when the harness
/// drives it, so the one process that knows which program it is hands the answer
/// down. No leading arguments — Python's re-invocation needs `-m devlaunch.dl` and
/// a compiled binary needs nothing.
pub(crate) fn self_invocation() -> SelfInvocation {
    SelfInvocation::new(refresh_program(std::env::current_exe().ok()))
}

/// The program the refresh child is spawned as, from what `current_exe()` said.
///
/// The answer has to still be spawnable, which the running binary's path is not
/// guaranteed to be: after `pixi global update` swaps the binary mid-run, Linux
/// reports the unlinked inode as `/path/dl (deleted)` — a path that exists for no
/// one — so spawning it fails with `ProgramNotFound` and completions silently
/// lose their freshness. Python's `sys.executable` survives the same swap, so
/// this is where the gap is closed: a path that no longer exists, or that carries
/// the kernel's ` (deleted)` mark (checked on its own too, against a file that
/// happens to sit at the marked name), falls back to the bare program name and
/// lets the spawn's PATH search find the replacement. That name is the only
/// other honest guess; a spawn that then finds nothing is
/// [`lifecycle::SpawnRefused::ProgramNotFound`](devlaunch_core::flows::lifecycle::SpawnRefused::ProgramNotFound),
/// and a refresh that could not be spawned costs completions their freshness and
/// nothing else.
fn refresh_program(current_exe: Option<PathBuf>) -> String {
    match current_exe {
        Some(path) if path.exists() && !path.to_string_lossy().ends_with(" (deleted)") => {
            path.to_string_lossy().into_owned()
        }
        _ => "dl".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_that_still_exists_is_respawned_by_its_own_path() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let binary = dir.path().join("dl");
        std::fs::write(&binary, "").expect("a file standing in for the binary");

        assert_eq!(
            refresh_program(Some(binary.clone())),
            binary.to_string_lossy()
        );
    }

    #[test]
    fn a_binary_swapped_out_from_under_the_run_falls_back_to_the_bare_name() {
        // What `current_exe()` answers after `pixi global update` replaces the
        // binary mid-run: the unlinked inode's path, which exists for no one. The
        // swap is simulated by the path-exists check — the path simply is not
        // there — which is exactly the fact the decision turns on.
        let dir = tempfile::tempdir().expect("a temporary directory");

        assert_eq!(refresh_program(Some(dir.path().join("dl (deleted)"))), "dl");
        assert_eq!(refresh_program(Some(dir.path().join("dl"))), "dl");
    }

    #[test]
    fn the_kernels_deleted_mark_is_refused_even_where_a_file_wears_it() {
        // ` (deleted)` is the kernel's annotation, not part of any name dl was
        // started by — so a file that happens to sit at the marked path must not
        // launder the mark into an answer.
        let dir = tempfile::tempdir().expect("a temporary directory");
        let marked = dir.path().join("dl (deleted)");
        std::fs::write(&marked, "").expect("a file at the marked name");

        assert_eq!(refresh_program(Some(marked)), "dl");
    }

    #[test]
    fn a_path_that_could_not_be_read_at_all_falls_back_to_the_bare_name() {
        assert_eq!(refresh_program(None), "dl");
    }
}
