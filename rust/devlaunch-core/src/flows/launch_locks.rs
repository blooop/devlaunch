//! The per-workspace launch locks, as a directory something can sweep.
//!
//! Two `dl`s launching one workspace serialize on a file under
//! `<cache>/launch-locks/<workspace-id>.lock` -- an empty file whose only content
//! is the `flock` the kernel holds on it. [`crate::flows::launch::serialize_launch`]
//! takes it; this module is the store that knows where they all are.
//!
//! # Why it is a store and not two lines in `launch`
//!
//! Because a lock file outlives the launch that made it, and until devlaunch#575
//! nothing reclaimed one. Every workspace ever launched left an entry and no
//! removal path took it away: measured on the reference host, 18 of 26 named no
//! workspace `devpod list` still returns, the oldest three weeks old, against
//! 8 live workspaces. `dl --purge` reached them only because it removes the whole
//! cache directory, which is to say the answer to a straggler was to delete
//! everything else too.
//!
//! They are zero-byte files, so this is not disk. It is principle 2 of
//! devlaunch#444 -- nothing devlaunch causes to exist should accumulate
//! unreclaimed -- and a directory that only ever grows is also a directory nobody
//! can read as a description of anything.
//!
//! # What made the reclaim possible, and it is not here
//!
//! Unlinking an flock'd file is the self-defeating move [`crate::domain::locks`]
//! argues at length: a process holding the old inode excludes nobody while new
//! arrivals lock a fresh file. What closes it is that module's revalidation --
//! a guard is handed back only for the inode the path still names -- and
//! [`crate::domain::locks::reclaim`], which holds the lock across the unlink so a
//! launch inside its critical section keeps its file. The judgement of *which*
//! locks to offer up is this module's; the safety of removing one is not.
//!
//! # The other file called `workspace.lock` is not in scope
//!
//! devpod keeps its own per-workspace flock under `contexts/<ctx>/locks`, and the
//! same measurement found 70 of 78 of those naming no live workspace.
//! **devlaunch does not touch it**, for the reason
//! [`crate::clients::devpod_home::untouchable_flock`] is named after: it is
//! another program's lock, taken by processes devlaunch does not run, and a
//! revalidation devpod does not perform cannot be given to it from out here.
//! Reclaiming it would be devlaunch deciding that devpod's mutual exclusion is
//! devlaunch's to break.

use std::path::{Path, PathBuf};

use crate::domain::locks::{self, Reclaimed};

/// The leaf under devlaunch's cache directory that the launch locks live in.
///
/// Its own directory rather than the repo cache: this lock is keyed by workspace,
/// exists for workspaces that have no clone under the cache at all (paths, URLs),
/// and must not look like a repo to the cache's walkers.
pub(crate) const LAUNCH_LOCK_DIR: &str = "launch-locks";

/// The extension every launch lock file carries, and the whole of what marks one.
///
/// Named rather than written twice, because [`LaunchLocks::keyed`] recovers a
/// workspace id by taking it off again: a walk that filtered on one spelling and
/// stripped another would report ids that are not ids.
const LOCK_EXTENSION: &str = "lock";

/// Where this host's launch locks are, and the only thing that says so.
///
/// A path parameter rather than a read of the process environment, for
/// [`crate::flows::kept_copies::KeptCopies`]'s reason: the binary resolves the
/// cache directory once and hands it down, and a store that resolved its own could
/// disagree with the one the launch already resolved. It is what makes a run
/// pointed at a scratch `XDG_CACHE_HOME` find no locks and so reclaim none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchLocks {
    dir: PathBuf,
}

impl LaunchLocks {
    /// The launch locks under `cache_dir`.
    pub fn under(cache_dir: &Path) -> Self {
        Self {
            dir: cache_dir.join(LAUNCH_LOCK_DIR),
        }
    }

    /// The lock two `up`s of this workspace serialize on.
    ///
    /// The id goes into the name unescaped, as
    /// [`crate::flows::kept_copies::KeptCopies`]'s copy does and for its reason:
    /// devpod itself uses the id as a directory name under its own contexts, so an
    /// id that could not be a path component is one no workspace this could be
    /// asked about has.
    pub(crate) fn path_for(&self, workspace_id: &str) -> PathBuf {
        self.dir.join(format!("{workspace_id}.{LOCK_EXTENSION}"))
    }

    /// Every workspace this host holds a launch lock for, sorted.
    ///
    /// The reclaim's domain, and like [`crate::flows::kept_copies::KeptCopies::copied`]
    /// it is deliberately not a walk of anything else: a lock is keyed by workspace
    /// and exists for workspaces with no clone under the cache at all, so a
    /// clone-shaped walk would never reach one of those and reasoning over an
    /// enumeration that does not cover what it affects is the defect class
    /// devlaunch#445 exists to close.
    ///
    /// A directory that is not there is no locks rather than an error -- what a
    /// fresh install and a scratch cache both are -- and anything that is not a
    /// `.lock` file is not one of these, which is what keeps the sweep from
    /// reporting a workspace id it invented from somebody else's file.
    pub(crate) fn keyed(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension()? == LOCK_EXTENSION)
                    .then(|| path.file_stem()?.to_str().map(str::to_owned))
                    .flatten()
            })
            .collect();
        ids.sort();
        ids
    }

    /// Reclaim this workspace's launch lock, if nothing holds it.
    ///
    /// The judgement of whether this workspace's lock *should* go is the caller's
    /// -- it is the one that knows what `devpod list` returned. What this adds is
    /// the second half of it, and it is the half a caller cannot check for itself:
    /// a lock a launch is holding right now stays, whatever the listing said a
    /// moment ago.
    pub(crate) fn reclaim(&self, workspace_id: &str) -> Reclaimed {
        locks::reclaim(&self.path_for(workspace_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temp dir")
    }

    /// Create a launch lock file the way a launch does, and let go of it.
    fn launched(locks: &LaunchLocks, workspace_id: &str) {
        let path = locks.path_for(workspace_id);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
        std::fs::write(&path, "").expect("the lock file");
    }

    #[test]
    fn a_cache_with_no_launch_lock_directory_holds_no_locks() {
        // A fresh install and a scratch `XDG_CACHE_HOME` are the same case, and the
        // second is what makes a scratch run reclaim nothing by construction.
        let dir = temp_dir();

        assert_eq!(LaunchLocks::under(dir.path()).keyed(), Vec::<String>::new());
    }

    #[test]
    fn every_workspace_with_a_lock_is_named_once_and_in_order() {
        let dir = temp_dir();
        let locks = LaunchLocks::under(dir.path());
        launched(&locks, "repo-main-9zzz");
        launched(&locks, "repo-feature-1aaa");

        assert_eq!(locks.keyed(), ["repo-feature-1aaa", "repo-main-9zzz"]);
    }

    #[test]
    fn a_file_that_is_not_a_lock_names_no_workspace() {
        // The walk recovers an id by taking the extension off a filename, so
        // anything else in the directory would arrive as a workspace id that no
        // launch ever used -- and the reclaim's whole precondition is a comparison
        // against ids devpod knows.
        let dir = temp_dir();
        let locks = LaunchLocks::under(dir.path());
        launched(&locks, "repo-main-9zzz");
        std::fs::write(dir.path().join(LAUNCH_LOCK_DIR).join("README"), "").expect("a stray file");
        std::fs::write(dir.path().join(LAUNCH_LOCK_DIR).join("notes.txt"), "")
            .expect("another one");

        assert_eq!(locks.keyed(), ["repo-main-9zzz"]);
    }

    #[test]
    fn the_lock_a_launch_takes_is_the_lock_the_sweep_reclaims() {
        // One spelling of the path, which is the whole reason this store exists
        // rather than a `format!` at each end: a sweep addressing a file no launch
        // writes would report reclaiming locks and leave every real one standing.
        let dir = temp_dir();
        let locks = LaunchLocks::under(dir.path());
        launched(&locks, "repo-main-9zzz");

        assert_eq!(locks.keyed(), ["repo-main-9zzz"]);
        assert_eq!(locks.reclaim("repo-main-9zzz"), Reclaimed::Removed);
        assert_eq!(locks.keyed(), Vec::<String>::new());
    }

    #[test]
    fn reclaiming_a_workspace_that_never_launched_here_creates_nothing() {
        let dir = temp_dir();
        let locks = LaunchLocks::under(dir.path());

        assert_eq!(locks.reclaim("never-launched"), Reclaimed::AlreadyGone);
        assert!(!locks.path_for("never-launched").exists());
    }
}
