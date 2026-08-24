//! The bare-clone cache: one repository per `(owner, repo)`, cloned once.
//!
//! Ported from `devlaunch/worktree/repo_manager.py`. This is the layer that
//! decides *sequence* over [`crate::clients::git`]'s verbs — clone-if-missing,
//! the recovery of a cache some other run left half-made, the fetch a background
//! sweep may make and the one a launch may not — and the layer that owns the
//! per-repo lock every one of those steps runs under.
//!
//! # The layout, and why the clones are siblings of the cache
//!
//! ```text
//! repos_dir/<owner>/<repo>/.bare/          the bare git repository
//! repos_dir/<owner>/<repo>/.lock           the per-repo lock (a file, never a directory)
//! repos_dir/<owner>/<repo>/<workspace-id>/ one workspace clone per branch
//! ```
//!
//! The cache and the clones being *siblings of one directory* is load-bearing
//! rather than tidy: it is what puts every workspace clone on the same filesystem
//! as the objects it clones from, so git's local transport can hardlink the pack
//! files instead of copying them. See
//! [`crate::flows::workspace_clone::WorkspaceCloneManager::prepare_workspace`]
//! for what that is worth and for the cross-filesystem fallback this layout makes
//! unreachable.
//!
//! The lock is a *file* inside the repo directory, not a directory: every walker
//! of the cache filters on "is a directory", so it is invisible to discovery,
//! migration and completion scans.
//!
//! # Nothing here prints
//!
//! Python logged fifteen lines from this module. Core renders no English (#251),
//! so each one that carried a decision — a clone adopted rather than remade, a
//! record naming a directory that is gone — comes back as a [`CacheNotice`] with
//! the data the line interpolated, and each failure is a typed error. The words
//! are the `dl` binary's.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::clients::git::{self, Failure, Git, GitRefused};
use crate::domain::locks::{self, Contention, LockError, LockGuard};
use crate::domain::metadata::{self, MetadataError, MetadataStorage};
use crate::domain::model::{BaseRepository, RecordedDefaultBranch, Timestamp};
use crate::domain::workspace_id::{NamePart, UnsafeName, validate_ref_name};
use crate::domain::workspace_state::NonEmpty;
use crate::notices::Notices;
use crate::timing;

/// The bare repository's directory name, inside the repo directory.
///
/// It shares the parent of the clone directories and is never one of them, which
/// is why the migration skips it by name: it is the layout's one fixed leaf.
pub(crate) const BARE_DIR_NAME: &str = ".bare";

/// The per-repo lock's file name.
pub(crate) const LOCK_FILE_NAME: &str = ".lock";

/// What `default_branch` is when nothing better can be established.
const FALLBACK_DEFAULT_BRANCH: &str = "main";

/// Seconds between background fetches when no configuration says otherwise —
/// `WorktreeConfig`'s default, repeated here for a manager built without one.
///
/// Only [`RepositoryManager::new`] reads it, which only tests call today.
#[cfg_attr(not(test), allow(dead_code))]
const DEFAULT_FETCH_INTERVAL: Duration = Duration::from_secs(3600);

/// How long the background sweep may spend fetching one repository before it
/// gives up and leaves that repository to the next pass.
///
/// This is not a performance budget — an incremental fetch of an already-cloned
/// repository is seconds — it is the ceiling on how long a *foreground* launch of
/// the same repository can be made to wait behind the sweep's repo lock.
/// Generous enough that a slow but working remote finishes, short enough that a
/// hung one costs one pass rather than the hour until the next. The interval
/// itself is 3600s, so a repository that times out every time is no worse off
/// than one that is simply unreachable.
///
/// The sweep that passes it is the detached updater (M6); the bound belongs here,
/// with the fetch it bounds.
pub(crate) const BACKGROUND_FETCH_TIMEOUT: Duration = Duration::from_secs(300);

// ------------------------------------------------------------- the layout

/// The root directory for one repository: `.bare`, `.lock` and the clones.
pub(crate) fn repo_dir(repos_dir: &Path, owner: &str, repo: &str) -> PathBuf {
    repos_dir.join(owner).join(repo)
}

/// The bare git directory for one repository.
pub(crate) fn bare_dir(repos_dir: &Path, owner: &str, repo: &str) -> PathBuf {
    repo_dir(repos_dir, owner, repo).join(BARE_DIR_NAME)
}

/// The lock every process takes before mutating `repos_dir/<owner>/<repo>`.
pub(crate) fn repo_lock_path(repos_dir: &Path, owner: &str, repo: &str) -> PathBuf {
    repo_dir(repos_dir, owner, repo).join(LOCK_FILE_NAME)
}

/// The directory one workspace clone lives in: a sibling of `.bare`, named by the
/// workspace id.
///
/// The leaf is the workspace id — the same string that names the devpod workspace
/// — and not the branch. A bare branch name is unique only *within* its parent,
/// which is what let a downstream consumer reading one path component collapse
/// every branch of a repository onto a single identity
/// (kinisi-robotics/kinisi_ros#9766).
///
/// Takes the id as a string rather than a `WorkspaceId` so a caller holding a
/// *recorded* id — the listing flows, reading `metadata.json` — can name the same
/// directory without re-deriving it. A caller starting from a triple should go
/// through
/// [`crate::flows::workspace_clone::WorkspaceCloneManager::workspace_path`],
/// which derives the id and therefore validates the triple.
pub(crate) fn clone_dir(repos_dir: &Path, owner: &str, repo: &str, workspace_id: &str) -> PathBuf {
    repo_dir(repos_dir, owner, repo).join(workspace_id)
}

// ---------------------------------------------------------------- reports

/// Something the storage flows did that the `dl` binary may want to report.
///
/// One vocabulary for the whole subsystem — this module,
/// [`crate::flows::workspace_clone`] and the branch step between them — because
/// they are one operation to a user and a single launch produces notices from all
/// three. Every arm is one `logging` call Python made, carrying what that line
/// interpolated; nothing here is a sentence.
///
/// Two kinds of arm, and the difference is Python's level rather than anything a
/// reader of this type has to act on. The first group is the `logger.info`
/// **progress** lines — what the flow is about to do, or has just done — and the
/// rest are the `warning`/`error` lines, where something was adopted, degraded or
/// refused. Both reach stderr as the bare message (`dl.py` configures
/// `level=INFO, format="%(message)s"`), so there is no level to render and the
/// groups are not two types; a notice pushed in flow order stays in flow order
/// either way, which is the whole reason the progress lines can travel this way at
/// all.
#[derive(Debug, Clone, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum CacheNotice {
    // --- progress: what the flow is doing (Python's `logger.info` lines) ---
    /// A bare clone is about to be made, because there is none.
    ///
    /// The remote and the destination, which is what the line names: the first
    /// launch of a large repository sits here for minutes, and the two paths are
    /// what say which repository is being fetched and where the disk is going.
    CloningRepository { remote_url: String, bare: PathBuf },
    /// The bare clone is on disk and recorded.
    ClonedRepository { owner: String, repo: String },
    /// Every head and tag is about to be swept into the bare clone — the broad
    /// fetch, which is the sweep's and the forced refresh's, never a launch's.
    FetchingUpdates { owner: String, repo: String },
    /// The sweep finished and the record's `last_fetched` moved.
    FetchedUpdates { owner: String, repo: String },
    /// One ref is about to be fetched — the launch path's entire network budget.
    FetchingRef {
        owner: String,
        repo: String,
        branch: String,
    },
    /// A workspace clone is about to be cut from the bare cache.
    CreatingWorkspaceClone { path: PathBuf },

    // The branch decision, one arm per state
    // [`crate::flows::branch_manager::BranchEnsured`] can be in. Reported by the
    // caller that holds this channel, because `branch_manager` answers rather than
    // logs — see [`crate::flows::workspace_clone::WorkspaceCloneManager::ensure_branch`].
    /// The branch was already there locally and on the remote, so nothing was cut,
    /// pushed or pointed.
    BranchAlreadyBothSides { branch: String },
    /// The remote had the branch and the local side did not, so a local branch was
    /// cut from `<remote>/<branch>`.
    BranchCutFromRemote { branch: String, remote: String },
    /// A local branch was created — from the start point, since the remote has not
    /// got this branch.
    BranchCreated { branch: String },
    /// The branch was pushed to the remote. Reachable only for a caller that asked
    /// for it: the launch path never does.
    BranchPushed { branch: String, remote: String },
    /// The cache's own git-lfs store is about to be filled for one ref. Carries
    /// nothing: Python's line names neither the ref nor the cache.
    FillingLfsCache,
    /// A pointer survived the cache phase, so the objects are about to come from
    /// the forge. Carries nothing, as Python's line does.
    PullingLfsFromOrigin,
    /// A workspace clone directory is gone.
    WorkspaceCloneRemoved { path: PathBuf },
    /// There was no workspace clone at the directory dl named, so nothing was
    /// removed. Reported rather than silent: the directory is the answer to "then
    /// where is my work?", and dl naming one that is not there is how a stale
    /// record shows itself.
    NoWorkspaceCloneToRemove { path: PathBuf },

    // --- the rest: adopted, degraded or refused (`warning`/`error` lines) ---
    /// A working bare clone was already on disk for a repository this process has
    /// no record of, so the record was rebuilt from the clone rather than the
    /// clone from the remote. Either another process cloned it just now, or a run
    /// died between the clone and its save.
    AdoptedBareClone {
        owner: String,
        repo: String,
        bare: PathBuf,
    },
    /// A directory in the bare clone's place held no `HEAD`, so it was a dead
    /// run's partial clone; it was cleared and the clone made fresh.
    ClearedPartialClone { bare: PathBuf },
    /// `metadata.json` names a repository whose clone directory is not there, so
    /// the record is not a repository and is reported as absent.
    RecordWithoutClone {
        owner: String,
        repo: String,
        bare: PathBuf,
    },
    /// A ref could not be fetched. The launch proceeds from whatever the cache
    /// holds — that is the offline contract (devlaunch#144) — so this is a
    /// notice rather than an error.
    RefNotFetched {
        owner: String,
        repo: String,
        branch: String,
        reason: NotRefreshed,
    },
    /// The *recorded* default branch is not a safe git name, so git was never
    /// asked to fetch it. Its own arm rather than a [`CacheNotice::RefNotFetched`]
    /// because Python warns a different sentence here: the name was read back
    /// from `metadata.json` with no proof, and the line says so rather than
    /// echoing a name that cannot safely be interpolated anywhere.
    RecordedDefaultBranchUnsafe { refused: UnsafeName },
    /// The repository's default branch could not be named, so nothing was
    /// fetched to cut a new branch from.
    DefaultBranchUnknown {
        owner: String,
        repo: String,
        reason: NotRefreshed,
    },
    /// The workspace was prepared from a base nothing refreshed this call. The
    /// one consequence-stating notice of the whole degraded family: the fetch
    /// notices above say what failed, this says what it means for the tree the
    /// agent is about to work in, and `wf` reads it (devlaunch#245).
    PreparedFromStaleBase {
        owner: String,
        repo: String,
        branch: String,
        base: String,
        reason: NotRefreshed,
    },
    /// The cache's git-lfs store could not be filled. Best-effort: the workspace
    /// falls through to the network phase.
    LfsCacheNotFilled { reason: String },
    /// The workspace's git-lfs content could not be materialized out of the
    /// cache. Best-effort, for the same reason.
    LfsNotPulledFromCache { reason: String },
    /// The tracked files could not be listed, so the git-lfs probe ran anyway:
    /// "cannot tell" is not "no LFS here".
    TrackedFilesNotListed { reason: String },
    /// `git lfs ls-files` refused, so no pointer could be named. Reported rather
    /// than read as "no LFS here", which would ship a tree of pointer files as
    /// though it were complete.
    LfsFilesNotListed { reason: String },
    /// The workspace clone is on disk but its record could not be written. Carries
    /// the write's own refusal, for the reason [`CloneError::NotRecorded`] does.
    WorkspaceNotRecorded { refusal: metadata::MetadataError },
    /// The workspace clone is gone but its record could not be removed. Carries the
    /// write's own refusal, for the reason [`CloneError::NotRecorded`] does.
    WorkspaceRecordNotRemoved { refusal: metadata::MetadataError },
    /// No clone directory can be named for a record: the recorded path is
    /// unusable *and* the record's own triple is not a safe one. Named by the
    /// triple, because that triple is what failed and is the field a hand-edited
    /// `metadata.json` would have to be fixed in; `refused` says which part the
    /// derivation judged.
    CloneNotNamed {
        owner: String,
        repo: String,
        branch: String,
        refused: UnsafeName,
    },
    /// Something the metadata store reported while loading under a mutation.
    Metadata(metadata::Notice),
}

/// The moment a run finds the per-repo lock held and is about to block.
///
/// Handed over *before* the blocking acquisition, which is the only moment at
/// which "this run is now waiting" can be reported at all: after it returns, the
/// wait is over. A first launch of a large repository can sit for a minute behind
/// a sibling's clone, and the two look identical from outside — a dl that has
/// printed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoWait {
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) lock_path: PathBuf,
}

// ------------------------------------------------------- the fetch outcome

/// What asking the remote for one ref turned out to be.
///
/// Three arms and no fourth. "The cache is not on disk" is a
/// [`FetchOutcome::Failed`] rather than a fourth arm: nothing was learned about
/// the remote, which is exactly the property that decides what a caller may do
/// next.
///
/// Python needed an `unhandled_fetch_outcome` helper to keep an `else` at a call
/// site from silently reading a new arm as an old one. Here that is `match`
/// exhaustiveness, so the helper has no analogue and adding an arm breaks every
/// reader instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FetchOutcome {
    /// The requested ref in the cache now matches the remote's.
    ///
    /// Carries no payload: "it is current" is the whole message.
    Updated,
    /// The remote has no such ref, and answered to say so.
    ///
    /// Not a failure. This is what an ordinary "start a new branch" launch gets,
    /// and the caller's response is to base the branch on the default branch
    /// instead. A reachable remote is *evidence* here — the ref's absence is
    /// established rather than assumed, which is what makes basing a new branch
    /// on the default branch the right move rather than a guess.
    RefMissingOnRemote,
    /// The remote could not be asked, and `reason` is what git said.
    ///
    /// Distinct from [`FetchOutcome::RefMissingOnRemote`] because nothing was
    /// learned about the remote: the ref may well exist. So the caller may only
    /// fall back to whatever the cache already holds, and must not invent a
    /// branch off the default branch on the strength of an answer it never got.
    ///
    /// The reason is carried rather than reconstructed at the print site: no such
    /// host, a refused connection, an expired credential and a bare cache that is
    /// not there all arrive here and read differently to whoever has to fix it.
    Failed { reason: String },
}

/// Why a ref went unrefreshed — the reason inside
/// [`crate::flows::workspace_clone::BranchBase::Stale`] and the fetch notices.
///
/// A `reason: String` before, and two of its producers *composed* it in core — one
/// interpolating a `{:?}` debug rendering of [`crate::domain::workspace_id::NamePart`]
/// into text a person reads. Typed arms carry the data instead, and the `dl` binary
/// owns the words (#251 §5); only [`NotRefreshed::FetchFailed`] carries text, and
/// that text is git's or the OS's, never this crate's.
#[derive(Debug, Clone, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum NotRefreshed {
    /// The fetch ran and failed, or could not be run; `reason` is what git or the
    /// OS said, carried verbatim from [`FetchOutcome::Failed`].
    FetchFailed { reason: String },
    /// The remote answered: it has no branch by this name to refresh from.
    NoBranchOnRemote { branch: String },
    /// The name is not one git can safely be asked about, so nothing was fetched.
    /// In practice this is the *recorded* default branch — the one ref on this
    /// path that does not arrive inside a validated
    /// [`crate::domain::workspace_id::WorkspaceId`].
    UnsafeName(UnsafeName),
    /// No default branch is recorded, so there was nothing to name a fetch of.
    NoDefaultBranchRecorded,
}

// -------------------------------------------------------------- the token

/// Evidence that the per-repo lock for `(owner, repo)` is held right now.
///
/// A method that takes one cannot be called without the lock, so the rule
/// [`locks::hold_lock`] needs from its callers is stated by the signature instead
/// of begged for in a comment at each site. Three such comments used to stand in
/// front of the acquisitions on the cold path, and a comment is not read by the
/// caller who is about to deadlock against it.
///
/// **It carries the pair, and that is not decoration.** A bare marker type would
/// let a lock taken on `owner/repo` vouch for work on `owner/other`: the lock
/// genuinely held, the wrong repository genuinely unserialized, and nothing in
/// the signature able to tell. Every method that takes a token checks it against
/// the repository it is about to touch — see [`RepoLock::require`].
///
/// Minted only by [`RepositoryManager::hold_repo_lock`], which needs no runtime
/// check to enforce: the fields are private to this module, so no other module
/// can build one. Python spent a sentinel object and a `__post_init__` raise on
/// the same guarantee.
///
/// It owns the lock, so the lock is released when the token is dropped — by a
/// return, by a `?`, or by a panic.
#[derive(Debug)]
pub(crate) struct RepoLock {
    owner: String,
    repo: String,
    /// The flock itself. Never read; dropping it is the release.
    #[allow(dead_code)]
    guard: LockGuard,
}

/// A token offered as evidence about a repository it says nothing about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrongRepoLock {
    /// The repository the lock is actually held for.
    pub held: (String, String),
    /// The repository the caller wanted it to vouch for.
    pub wanted: (String, String),
}

impl RepoLock {
    /// Only this module's tests read the token's repository back; the flows above
    /// state which repository they want and let [`RepoLock::require`] refuse.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn owner(&self) -> &str {
        &self.owner
    }

    /// See [`RepoLock::owner`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn repo(&self) -> &str {
        &self.repo
    }

    /// Whether this token is evidence about `owner`/`repo` specifically.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn covers(&self, owner: &str, repo: &str) -> bool {
        self.owner == owner && self.repo == repo
    }

    /// Refuse unless this token is evidence about `owner`/`repo`.
    pub(crate) fn require(&self, owner: &str, repo: &str) -> Result<(), WrongRepoLock> {
        if self.covers(owner, repo) {
            Ok(())
        } else {
            Err(WrongRepoLock {
                held: (self.owner.clone(), self.repo.clone()),
                wanted: (owner.to_owned(), repo.to_owned()),
            })
        }
    }

    /// Whether this acquisition had to queue behind another holder.
    ///
    /// Only this module's tests ask a token about its wait; the flows above take
    /// the measurement where the blocking call is.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn contention(&self) -> Contention {
        self.guard.contention()
    }
}

// -------------------------------------------------------------- removal

/// What became of a tree this asked to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreeRemoval {
    Removed,
    /// There was nothing there, which is not a failure: a cleanup run twice is
    /// not a failure the second time.
    WasNotThere,
}

/// Why a tree could not be removed.
#[derive(Debug, Clone, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7). Public because
// `RemoveWorkspaceError` carries it and reaches the binary through
// `LifecycleNotice::CloneNotRemoved`, unrendered.
pub enum RemoveTreeError {
    /// The root is a symbolic link, and neither of the two things that could be
    /// done with it is honest.
    ///
    /// Following it empties a directory the caller never named. Unlinking just
    /// the link is no better and is worse to diagnose: the clones are still on
    /// the other disk and the removal reports success. A cache directory is a
    /// symlink because somebody moved their cache, so both answers cost them
    /// their workspaces — one by deleting them, one by telling them they are
    /// gone. `points_at` is carried because `sudo rm -rf <path>` would remove the
    /// link and nothing else, so the reader needs the real location to act on.
    RootIsSymlink {
        path: PathBuf,
        points_at: Option<PathBuf>,
    },
    /// The root could not be looked at, so nothing was attempted. Something is
    /// there that this process is not allowed to see, and calling it gone is the
    /// failure this guards.
    CouldNotLook { path: PathBuf, reason: String },
    /// The walk refused partway.
    Refused { path: PathBuf, reason: String },
}

/// Remove `tree` and everything under it, refusing a symlinked root.
///
/// Presence and symlink-ness come from **one** `lstat`, and that is deliberate:
/// asking twice is two answers about a directory that can change between them,
/// and the "does it exist" question cannot be asked with a plain existence check
/// at all — that answers `false` for a path this process was not allowed to look
/// at on some runtimes and raises on others, and neither of those is "there is
/// nothing to remove".
///
/// What this does *not* do is keep going past a refusal. `dl --purge` needs that
/// — a clone directory written by the container's user refuses every one of its
/// children separately, and abandoning the rest of the cache over it is a worse
/// outcome than the permission error (devlaunch#131) — and it needs the
/// three-armed answer that goes with it. This is the cleanup the cache's own
/// steps make, where the tree being removed is one this process just created, and
/// where a refusal is the caller's error to report. The partial-removal walk is
/// the lifecycle flows' (M6).
pub(crate) fn remove_tree(tree: &Path) -> Result<TreeRemoval, RemoveTreeError> {
    let stat = match std::fs::symlink_metadata(tree) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TreeRemoval::WasNotThere);
        }
        Err(error) => {
            return Err(RemoveTreeError::CouldNotLook {
                path: tree.to_path_buf(),
                reason: error.to_string(),
            });
        }
    };
    if stat.file_type().is_symlink() {
        return Err(RemoveTreeError::RootIsSymlink {
            path: tree.to_path_buf(),
            points_at: std::fs::read_link(tree).ok(),
        });
    }
    let removed = if stat.is_dir() {
        std::fs::remove_dir_all(tree)
    } else {
        std::fs::remove_file(tree)
    };
    match removed {
        Ok(()) => Ok(TreeRemoval::Removed),
        Err(error) => Err(RemoveTreeError::Refused {
            path: tree.to_path_buf(),
            reason: error.to_string(),
        }),
    }
}

// ------------------------------------------- removal that keeps going (#131)

/// Whether something is at *path*, where "cannot tell" counts as there.
///
/// Only [`std::io::ErrorKind::NotFound`] means there is nothing to do. Any other
/// refusal — an unreadable parent directory, say — means something is there that
/// this process cannot look at, and treating that as absent is how a purge reports
/// a clean sweep over an intact cache.
///
/// [`Path::exists`] cannot make that distinction: it collapses every error into
/// `false`, which is exactly the sentinel Python's `Path.exists()` produced on
/// 3.14 (and it raised on 3.13, so the same expression was two behaviours across
/// the versions dl supported). A symlink counts as present whether or not it
/// resolves, because the link itself is a thing to remove.
pub(crate) fn present(path: &Path) -> bool {
    !matches!(
        std::fs::symlink_metadata(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

/// The system's own words for a failure, as Python's `OSError.strerror` gives
/// them.
///
/// [`std::io::Error`]'s `Display` is `"{strerror} (os error {errno})"` for an OS
/// error, and the errno is already carried by the arm that holds this string —
/// where it is carried at all. So the suffix is dropped: the reason is printed to
/// a person beside the path it is about, and `Permission denied` is the whole of
/// what they need from it.
pub(crate) fn system_words(error: &std::io::Error) -> String {
    let text = error.to_string();
    match error.raw_os_error() {
        Some(errno) => text
            .strip_suffix(&format!(" (os error {errno})"))
            .unwrap_or(&text)
            .to_owned(),
        None => text,
    }
}

/// One path a removal could not remove, and what the system said about it.
///
/// The reason is carried rather than reconstructed at the print site because the
/// cause is not guessable from the path. A container writing as another user is
/// the common one and the one devlaunch#131 is about, but a read-only mount, an
/// immutable file and a busy mountpoint all reach here too — and for the last two
/// the advice that fixes the common case does not work.
#[derive(Debug, Clone, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub struct Refusal {
    pub path: PathBuf,
    pub reason: RefusalReason,
}

/// Why one path would not come away.
///
/// A sum rather than the sentence Python interpolated, because the two arms carry
/// different data and only one of them has an errno to quote. The words are the
/// `dl` binary's (#251).
#[derive(Debug, Clone, PartialEq, Eq)]
// binary surface — not part of the frozen wf API (#251 §7)
pub enum RefusalReason {
    /// What the OS said, in the words it used.
    System(String),
    /// The root of the removal is a symbolic link, and neither of the two things
    /// that could be done with it is honest — see [`RemoveTreeError::RootIsSymlink`].
    ///
    /// `points_at` is carried because the advice a report gives is
    /// `sudo rm -rf <path>`, which would remove the link and nothing else, so the
    /// reader needs the real location to act on.
    RootIsSymlink { points_at: Option<PathBuf> },
}

/// What became of a tree a removal was allowed to get part-way through.
///
/// Three arms, because a removal permitted to remove *part* of a tree has three
/// answers and a flat list of refusals records only two of them: what refused,
/// never whether anything went. devlaunch#182 is what that cost — a purge whose
/// cache root was itself the obstruction removed not one path and printed the
/// sentence for a partial success, because the only question its caller could ask
/// was "were there refusals".
///
/// A `(removed_something, refused)` pair would answer it and would also make
/// "removed everything, and here is what it refused" expressible, leaving every
/// reader to be trusted not to build it. These arms cannot say it: the arm that
/// means a clean sweep has nowhere to put a refusal, and each refusal arm carries
/// a [`NonEmpty`], so "refused, and here is the empty list" has no representation
/// either (Python's `Tuple[Refusal, ...]` had both).
///
/// The exit status a caller derives from this stays two-valued on purpose: zero
/// means the tree is gone and nothing else does. Which of the two failures
/// happened is in the report, where somebody can act on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Removal {
    /// The tree is gone, and so is everything that was under it — including when
    /// it was never there: a purge run twice is not a failure the second time.
    Everything,
    /// Some of it went. `refused` is what the filesystem would not let go.
    WhatItCould(NonEmpty<Refusal>),
    /// None of it went. `refused` is what the filesystem would not let go.
    Nothing(NonEmpty<Refusal>),
}

/// Remove `tree` and everything under it, **keeping going past a refusal**, and
/// say which of [`Removal`]'s three things happened.
///
/// [`remove_tree`] is the other half of this pair and is the one the cache's own
/// steps use: it stops at the first failure, which is right when the tree is one
/// this process just created and a refusal is the caller's error to report.
/// `dl --purge` needs the opposite. A container writes into its bind-mounted
/// clone as its own user — uid 1000 in the standard devcontainer base image — and
/// where the host user is not also uid 1000 the directories it made cannot be
/// emptied by us. That is one clone out of a cache full of them, and abandoning
/// the other clones, the completion caches and `metadata.json` on account of it is
/// a worse outcome than the permission error (devlaunch#131).
///
/// **Only the obstruction is named**, which is not the same as the path that
/// failed. Unlinking needs write permission on the *directory*, not on the file,
/// so a clone directory owned by the container's user refuses every one of its
/// children separately — on a real e2e workspace that is forty-odd
/// `.git/objects` entries, hooks and a README, none of them an ancestor of
/// another and every one of them the same single fact. So a failure is attributed
/// upward to the outermost directory that cannot be written into, which is the
/// directory the original errno named and the one a person would go and look at.
///
/// A path is then suppressed when something already reported accounts for it: a
/// directory that cannot be removed because a child refused adds nothing. A
/// *separately* sealed ancestor is not suppressed and should not be, because
/// fixing the one below it would not free it — so a chain of two sealed
/// directories is two lines, and each is work somebody has to do.
///
/// **What refused is decided from the disk, not from what failed.** A failure
/// during the walk is only a candidate; the report keeps the ones still on disk
/// when it is over, and both suppression rules are applied to that surviving list.
/// That is load-bearing rather than belt-and-braces, and randomised trees found
/// the case: a directory that cannot be listed is reported as unscannable and then
/// `rmdir`s fine if it is *empty*, so noting it where it failed named a path that
/// is not there and — through the ancestor rule — could have silenced a genuine
/// refusal above it.
///
/// **Whether anything came away is answered, not inferred.** A path that went is
/// counted as it goes, so the two refusal arms are told apart by what the removal
/// did rather than by what its report happens to look like.
///
/// A symlinked root is refused outright, and that is the only one of the three
/// available answers that is not a lie: following it empties a directory the
/// caller never named, and unlinking just the link reports a clean sweep over
/// clones that are still on disk somewhere else. Symlinks *inside* the tree are
/// unlinked and never descended.
pub(crate) fn remove_tree_as_far_as_it_goes(tree: &Path) -> Removal {
    // One lstat, three outcomes, none of them inferred — see `present` for why
    // this question cannot be asked with an existence check.
    let stat = match std::fs::symlink_metadata(tree) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Removal::Everything;
        }
        Err(error) => {
            // Something is there that we are not allowed to look at. Nothing was
            // attempted, so nothing came away.
            return refused_nothing(Refusal {
                path: tree.to_path_buf(),
                reason: RefusalReason::System(system_words(&error)),
            });
        }
    };
    if stat.file_type().is_symlink() {
        // Refused before anything was attempted: the link is still there and so
        // is everything it points at.
        return refused_nothing(Refusal {
            path: tree.to_path_buf(),
            reason: RefusalReason::RootIsSymlink {
                points_at: std::fs::read_link(tree).ok(),
            },
        });
    }

    let mut walk = Walk::default();
    if stat.is_dir() {
        walk.sweep(tree);
    }
    // The root is in nobody's directory listing, so it is removed by name.
    walk.remove_one(tree);

    // Bottom-up order is what the ancestor rule needs, and the candidate list is
    // already in it: `sweep` recurses into a directory before it attempts the
    // directory itself.
    let mut refused: Vec<Refusal> = Vec::new();
    let mut blocked: HashSet<PathBuf> = HashSet::new();
    for candidate in walk.candidates {
        // `present`, not an existence check: a path this process cannot look at
        // must be reported, not dropped. Dropping it is how the filter that exists
        // to prevent phantom refusals would have started causing silent ones.
        if !present(&candidate.path) {
            continue; // it went in the end, so there is nothing to report
        }
        let path = obstruction(&candidate.path, tree);
        if blocked.insert(path.clone()) {
            refused.push(Refusal {
                path: path.clone(),
                reason: candidate.reason,
            });
        }
        if let Some(parent) = path.parent() {
            blocked.insert(parent.to_path_buf());
        }
    }
    match NonEmpty::of(refused) {
        // Nothing survived that anybody needs to know about, so the tree is gone —
        // including the case where the walk hit failures the disk then
        // contradicted.
        None => Removal::Everything,
        Some(refused) if walk.removed_any => Removal::WhatItCould(refused),
        Some(refused) => Removal::Nothing(refused),
    }
}

fn refused_nothing(refusal: Refusal) -> Removal {
    Removal::Nothing(NonEmpty::one(refusal))
}

/// The bottom-up walk's running state: what failed, and whether anything went.
#[derive(Default)]
struct Walk {
    candidates: Vec<Refusal>,
    /// Counted where it happens. Deriving this from the refusal list afterwards is
    /// exactly the inference that cannot be made: a cache root that refuses its
    /// own entries reports one refusal whether there were two clones beside it or
    /// none.
    removed_any: bool,
}

impl Walk {
    /// Empty `dir`, deepest first. Does not remove `dir` itself.
    fn sweep(&mut self, dir: &Path) {
        let listed = match std::fs::read_dir(dir) {
            Ok(listed) => listed,
            Err(error) => {
                // A directory that cannot be listed must not pass for empty: it
                // may hold files, and walking it as though it held none would
                // leave them neither removed nor mentioned.
                self.candidates.push(Refusal {
                    path: dir.to_path_buf(),
                    reason: RefusalReason::System(system_words(&error)),
                });
                return;
            }
        };
        let mut children: Vec<PathBuf> = Vec::new();
        for entry in listed {
            match entry {
                Ok(entry) => children.push(entry.path()),
                Err(error) => self.candidates.push(Refusal {
                    path: dir.to_path_buf(),
                    reason: RefusalReason::System(system_words(&error)),
                }),
            }
        }
        for child in children {
            // A symlink is unlinked, never followed — descending one would put a
            // purge outside the tree it was asked to remove.
            if matches!(std::fs::symlink_metadata(&child), Ok(stat) if stat.is_dir()) {
                self.sweep(&child);
            }
            self.remove_one(&child);
        }
    }

    fn remove_one(&mut self, path: &Path) {
        let removed = match std::fs::symlink_metadata(path) {
            Ok(stat) if stat.is_dir() => std::fs::remove_dir(path),
            Ok(_) => std::fs::remove_file(path),
            // Already gone, or unlookable. Try the unlink anyway: it either
            // succeeds (which is the answer) or reports the same refusal the stat
            // did, on the path a person would go and look at.
            Err(_) => std::fs::remove_file(path),
        };
        match removed {
            Ok(()) => self.removed_any = true,
            Err(error) => self.candidates.push(Refusal {
                path: path.to_path_buf(),
                reason: RefusalReason::System(system_words(&error)),
            }),
        }
    }
}

/// The outermost path that actually explains a failure to remove `path`.
///
/// `access(2)` is advisory — it answers for the real uid and knows nothing about
/// ACLs — and that is acceptable precisely here, because it only decides *which*
/// path is named. A wrong answer makes the report less pointed; it can never turn
/// a refusal into a success.
///
/// The walk is bounded at `tree` and at the filesystem root. Nothing reaches here
/// from outside `tree` today; the second bound is so that a future caller that
/// does gets a wrong answer rather than a hung purge.
fn obstruction(path: &Path, tree: &Path) -> PathBuf {
    let mut path = path.to_path_buf();
    while path != tree {
        let Some(parent) = path.parent() else {
            break;
        };
        if parent == path {
            break; // the filesystem root: there is nothing above to blame
        }
        if writable_directory(parent) {
            break; // this one is reachable, so `path` is where it stops
        }
        path = parent.to_path_buf();
    }
    path
}

/// Whether this process could unlink an entry from `directory`, as far as
/// `access(2)` will say.
fn writable_directory(directory: &Path) -> bool {
    rustix::fs::access(
        directory,
        rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK,
    )
    .is_ok()
}

/// What became of the debris a failed step left, reported *alongside* the failure
/// that caused it.
///
/// Two facts rather than one: Python's cleanup raised its own `OSError` over the
/// top of the clone's reason, so the run that mattered — why the clone failed —
/// was the one lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cleanup {
    /// The debris was removed, or there was none.
    Cleared,
    /// The debris is still there, and this is why.
    Left(RemoveTreeError),
}

impl Cleanup {
    /// Remove `tree`, keeping the outcome rather than raising it.
    pub(crate) fn of(tree: &Path) -> Self {
        match remove_tree(tree) {
            Ok(_) => Cleanup::Cleared,
            Err(error) => Cleanup::Left(error),
        }
    }
}

// --------------------------------------------------------------- errors

/// Why the bare cache for a repository could not be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneError {
    /// The directory the clone goes in could not be created.
    ParentNotCreated { path: PathBuf, reason: String },
    /// A dead run's partial clone could not be cleared out of the way.
    PartialCloneNotCleared(RemoveTreeError),
    /// `git clone --bare` refused. `cleanup` is what became of whatever it left
    /// behind — for every failure reachable in practice git removes the
    /// destination itself, so this is normally [`Cleanup::Cleared`] with nothing
    /// having been there.
    GitRefused {
        refused: GitRefused,
        cleanup: Cleanup,
    },
    /// The clone is on disk and its record could not be written. Not swallowed:
    /// every caller reads a returned record as "the cache is ready".
    ///
    /// Carries the write's own refusal rather than a rendering of it: the words are
    /// the `dl` binary's, and the step that failed is what a reader has to act on.
    NotRecorded(metadata::MetadataError),
}

/// Why a repository's whole ref set could not be swept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FetchRepoError {
    /// There is no clone to fetch into.
    NoLocalClone {
        owner: String,
        repo: String,
        bare: PathBuf,
    },
    /// The bound elapsed and the child was killed.
    ///
    /// git writes new objects to a temp pack and moves refs only at the end, so a
    /// fetch cut off partway leaves the clone usable and the next pass redoes the
    /// work.
    TimedOut {
        owner: String,
        repo: String,
        limit: Option<Duration>,
    },
    /// git refused.
    Refused { reason: String },
    /// The fetch worked and `last_fetched` could not be written. Typed, for the
    /// reason [`CloneError::NotRecorded`] is.
    NotRecorded(metadata::MetadataError),
}

/// Why a conditional sweep could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LazyFetchError {
    /// The repository is not in `metadata.json`, so there is no clock to compare
    /// against.
    NotInMetadata {
        owner: String,
        repo: String,
    },
    Fetch(FetchRepoError),
}

/// Whether a conditional sweep actually fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fetched {
    Fetched,
    /// The interval had not elapsed.
    Skipped,
}

/// Why clone-if-missing could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloneIfMissingError {
    WrongRepoLock(WrongRepoLock),
    Clone(CloneError),
}

/// Why a lock scope of its own could not deliver the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureRepoError {
    Lock(LockError),
    /// Unreachable in practice — the scope mints the token it then passes — and
    /// kept as an arm rather than an `unwrap` so nothing in this module has a
    /// "this cannot happen" in it.
    WrongRepoLock(WrongRepoLock),
    Clone(CloneError),
}

impl From<CloneIfMissingError> for EnsureRepoError {
    fn from(error: CloneIfMissingError) -> Self {
        match error {
            CloneIfMissingError::WrongRepoLock(wrong) => EnsureRepoError::WrongRepoLock(wrong),
            CloneIfMissingError::Clone(failed) => EnsureRepoError::Clone(failed),
        }
    }
}

/// Whether a removal takes the directory with the record.
///
/// Named arms rather than Python's `remove_directory: bool`, because at the call
/// site `false` says nothing about *what* is not being removed.
/// Held for the #251 §7 public-API freeze — the `remove` verb's two shapes. Only
/// this module's tests name one today.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoveScope {
    /// Forget the repository and delete everything under its directory — the
    /// clones, the cache and the lock file with them.
    RecordAndDirectory,
    /// Forget the repository, leaving the directory where it is.
    RecordOnly,
}

/// Why a repository could not be removed from management.
///
/// Held for the #251 §7 public-API freeze — the `remove` verb's failures. Only
/// this module's tests name them today, which is also why the payloads are
/// unread: the binary that renders them is not wired yet.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
pub(crate) enum RemoveRepositoryError {
    NotUnrecorded(#[allow(dead_code)] MetadataError),
    DirectoryLeft(#[allow(dead_code)] RemoveTreeError),
}

// ---------------------------------------------------------- the manager

/// The bare-clone cache, over one `repos_dir`.
///
/// Holds no storage: `metadata.json` is handed to the methods that read or write
/// it, so a caller that owns the store can hand the same one to this and to
/// [`crate::flows::workspace_clone`] without either of them borrowing it for the
/// length of the run. Python injected it in the constructor and had two managers
/// sharing one object; the parameter is that, said out loud.
// binary surface — not part of the frozen wf API (#251 §7)
pub struct RepositoryManager<'r> {
    repos_dir: PathBuf,
    fetch_interval: Duration,
    git: Git<'r>,
    wait_watcher: Option<Box<dyn Fn(RepoWait)>>,
    /// How many times this manager has acquired a per-repo lock.
    ///
    /// Per-instance rather than a process-global counter, so parallel tests do
    /// not corrupt each other's count — and per *acquisition* rather than per
    /// lock *file*, because the files are never unlinked, so counting them can
    /// only tell 0 from more-than-0 and misses the 1-vs-2 the launch-shape
    /// cycle-count ledger (devlaunch#200) is about.
    #[cfg(test)]
    acquisitions: std::cell::Cell<usize>,
}

impl std::fmt::Debug for RepositoryManager<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepositoryManager")
            .field("repos_dir", &self.repos_dir)
            .field("fetch_interval", &self.fetch_interval)
            .finish_non_exhaustive()
    }
}

impl<'r> RepositoryManager<'r> {
    /// A manager over `repos_dir`, creating it if it is not there.
    ///
    /// The `mkdir` is best-effort, exactly as Python's constructor's was: a first
    /// run's convenience, not a step anything depends on. A `repos_dir` that
    /// cannot be created fails at the step that actually needs it — the clone —
    /// which is where the reason belongs.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(repos_dir: impl Into<PathBuf>, git: Git<'r>) -> Self {
        Self::with_fetch_interval(repos_dir, git, DEFAULT_FETCH_INTERVAL)
    }

    pub(crate) fn with_fetch_interval(
        repos_dir: impl Into<PathBuf>,
        git: Git<'r>,
        fetch_interval: Duration,
    ) -> Self {
        let repos_dir = repos_dir.into();
        let _ = std::fs::create_dir_all(&repos_dir);
        Self {
            repos_dir,
            fetch_interval,
            git,
            wait_watcher: None,
            #[cfg(test)]
            acquisitions: std::cell::Cell::new(0),
        }
    }

    /// How many per-repo lock acquisitions this manager has made, for the
    /// cycle-count tests (devlaunch#200).
    #[cfg(test)]
    pub(crate) fn repo_lock_acquisitions(&self) -> usize {
        self.acquisitions.get()
    }

    /// Be told when an acquisition of a per-repo lock is about to queue.
    ///
    /// The one thing a returned notice cannot cover: the point of saying it is to
    /// explain a run that has gone quiet, so it has to be said before the wait
    /// rather than after it. Nothing has to subscribe.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn watch_waits(&mut self, watcher: impl Fn(RepoWait) + 'static) {
        self.wait_watcher = Some(Box::new(watcher));
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn repos_dir(&self) -> &Path {
        &self.repos_dir
    }

    pub(crate) fn git(&self) -> Git<'r> {
        self.git
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn repo_dir(&self, owner: &str, repo: &str) -> PathBuf {
        repo_dir(&self.repos_dir, owner, repo)
    }

    pub(crate) fn bare_dir(&self, owner: &str, repo: &str) -> PathBuf {
        bare_dir(&self.repos_dir, owner, repo)
    }

    pub(crate) fn lock_path(&self, owner: &str, repo: &str) -> PathBuf {
        repo_lock_path(&self.repos_dir, owner, repo)
    }

    // ------------------------------------------------------------ the lock

    /// Hold the per-repo lock for the length of the returned token.
    ///
    /// The only place a [`RepoLock`] is minted, which is what makes the token mean
    /// something: every method that takes one is reachable only from inside a
    /// scope like this one.
    ///
    /// One scope per launch is the shape devlaunch#200 settled on. The
    /// alternatives were an outer scope in the command layer, which leaks the
    /// lock-ordering doctrine into code that has no business knowing it, and a
    /// reentrant lock, which makes ownership invisible — with nothing in a
    /// signature to say who holds what, "is this call already under the lock?"
    /// becomes a question answered by reading upwards through call sites.
    ///
    /// The contention the guard reports is deliberately not surfaced to the
    /// caller. A launch that waited may well find the world changed, but nothing
    /// under this lock acts on that: every step below is idempotent and re-checks
    /// the disk itself. What the wait *is* used for is the one measurement only
    /// the blocking call can make, recorded here under `locks.py`'s span name.
    pub(crate) fn hold_repo_lock(&self, owner: &str, repo: &str) -> Result<RepoLock, LockError> {
        let lock_path = self.lock_path(owner, repo);
        let watcher = self.wait_watcher.as_deref();
        let guard = locks::hold_lock_watching(&lock_path, |wait| {
            if let Some(watcher) = watcher {
                watcher(RepoWait {
                    owner: owner.to_owned(),
                    repo: repo.to_owned(),
                    lock_path: wait.lock_path,
                });
            }
        })?;
        // One acquisition, counted the way Python's `hold_lock` patch counts them:
        // each successful hold of a `.lock`, so a launch shape's cycle count is
        // exactly this (devlaunch#200).
        #[cfg(test)]
        self.acquisitions.set(self.acquisitions.get() + 1);
        // Only the queueing is measured, not the holding: an uncontended lock
        // costs nothing and records nothing, and what a summary should show is
        // the time this process spent behind a sibling rather than the time its
        // own work then took.
        if let Contention::Queued { waited } = guard.contention() {
            timing::record("lock wait", waited);
        }
        Ok(RepoLock {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            guard,
        })
    }

    // ----------------------------------------------------------- the clone

    /// Clone a new base repository as bare (no working directory).
    ///
    /// `--bare` ensures no branch is checked out, so every branch can have a
    /// workspace clone made from it without conflict.
    ///
    /// Four states the destination can be in, and each needs a different answer:
    ///
    /// 1. **A clone and a record.** The record is returned; nothing is cloned.
    /// 2. **A clone with a `HEAD` and no record.** Another process just made it
    ///    (this process's metadata was loaded before that one saved), or an
    ///    earlier run died between clone and save. Either way the clone on disk
    ///    is the authority and the record is derived state: the record is
    ///    rebuilt. Cloning over it is not an option — git refuses a non-empty
    ///    destination, and the failure cleanup below would then delete a cache
    ///    another launch is using.
    /// 3. **A directory with no `HEAD`.** A dead run's partial clone; nothing can
    ///    be recovered from it. No live process owns it, and what says so is the
    ///    repo lock: in production this is reached only through
    ///    [`RepositoryManager::clone_if_missing`], which cannot be called without
    ///    the token [`RepositoryManager::hold_repo_lock`] alone mints. So it is
    ///    cleared and cloned fresh.
    /// 4. **Nothing.** Clone.
    ///
    /// This method itself is reachable without a token, so case 3 is a property
    /// of the callers rather than one the signature enforces: the tests call it
    /// directly and unlocked, which is safe only because each has the cache to
    /// itself. A new production caller would have to come through the lock scope
    /// to keep the removal above true, and nothing here would stop it doing
    /// otherwise.
    pub(crate) fn clone_repo(
        &self,
        storage: &mut MetadataStorage,
        owner: &str,
        repo: &str,
        remote_url: &str,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> Result<BaseRepository, CloneError> {
        let bare = self.bare_dir(owner, repo);

        if bare.exists() {
            if let Some(existing) = self.get_repo(storage, owner, repo, notices) {
                return Ok(existing);
            }
            if bare.join("HEAD").exists() {
                notices.say(CacheNotice::AdoptedBareClone {
                    owner: owner.to_owned(),
                    repo: repo.to_owned(),
                    bare: bare.clone(),
                });
                return self
                    .register_existing_bare(storage, owner, repo, remote_url, &bare, notices);
            }
            notices.say(CacheNotice::ClearedPartialClone { bare: bare.clone() });
            remove_tree(&bare).map_err(CloneError::PartialCloneNotCleared)?;
        }

        if let Some(parent) = bare.parent() {
            std::fs::create_dir_all(parent).map_err(|error| CloneError::ParentNotCreated {
                path: parent.to_path_buf(),
                reason: error.to_string(),
            })?;
        }

        notices.say(CacheNotice::CloningRepository {
            remote_url: remote_url.to_owned(),
            bare: bare.clone(),
        });
        let cloned = {
            let _span = timing::span("git clone --bare");
            self.git.clone_bare(remote_url, &bare)
        };
        if let Some(refused) = cloned.refusal() {
            // Safe to delete: the exists-cases were all handled above, so this
            // directory is one this call created.
            return Err(CloneError::GitRefused {
                refused: refused.clone(),
                cleanup: Cleanup::of(&bare),
            });
        }

        let repository = BaseRepository {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            remote_url: remote_url.to_owned(),
            local_path: bare.clone(),
            default_branch: RecordedDefaultBranch::from_stored(self.default_branch_of(&bare)),
            last_fetched: Some(Timestamp::now()),
            worktrees: Vec::new(),
        };
        let recorded = self.record(storage, repository, notices)?;
        // After the record, as Python logs it: what the line reports is a clone
        // that is both on disk and known about.
        notices.say(CacheNotice::ClonedRepository {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        });
        Ok(recorded)
    }

    /// Rebuild the metadata record for a bare clone already on disk.
    fn register_existing_bare(
        &self,
        storage: &mut MetadataStorage,
        owner: &str,
        repo: &str,
        remote_url: &str,
        bare: &Path,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> Result<BaseRepository, CloneError> {
        let repository = BaseRepository {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            remote_url: remote_url.to_owned(),
            local_path: bare.to_path_buf(),
            // Read off the adopted clone, not defaulted: a repository whose
            // default branch is `master` and one this could read nothing at all
            // from would otherwise get the same answer.
            default_branch: RecordedDefaultBranch::from_stored(self.default_branch_of(bare)),
            last_fetched: Some(Timestamp::now()),
            worktrees: Vec::new(),
        };
        // The record *is* the point of this call, so a write that fails is the
        // call failing: there is nothing else it accomplished.
        self.record(storage, repository, notices)
    }

    /// Write a repository record, keeping the store's notices.
    fn record(
        &self,
        storage: &mut MetadataStorage,
        repository: BaseRepository,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> Result<BaseRepository, CloneError> {
        let written = storage.add_repository(repository.clone());
        match written {
            Ok(store_notices) => {
                notices.say_all(store_notices.into_iter().map(CacheNotice::Metadata));
                Ok(repository)
            }
            Err(error) => Err(CloneError::NotRecorded(error)),
        }
    }

    /// Clone the bare cache for `owner`/`repo` if it is not already there.
    ///
    /// Clone-if-missing and nothing else. It deliberately does **not** refresh a
    /// cache that is already there, however stale: the broad sweep that used to
    /// run from here is the detached updater's job now (devlaunch#149), and the
    /// launch path's entire network budget is the one targeted ref fetch in
    /// [`crate::flows::workspace_clone::WorkspaceCloneManager::ensure_branch`]. A
    /// fetch here would be unbounded network *under the repo lock*, so the launch
    /// that drew the short straw paid for everyone's freshness and every
    /// concurrent launch of the same repository queued behind it — the defect
    /// devlaunch#144 resolved.
    ///
    /// Freshness is not lost, it moved: see the staleness contract on
    /// `ensure_branch`.
    ///
    /// Takes the `lock` rather than acquiring one, because the whole
    /// exists-check-then-clone sequence has to be serialized and the cold path
    /// wants it serialized together with what follows it: without the lock, two
    /// processes launching the same repository at once both saw no clone and both
    /// ran `git clone --bare` into the same path — and the loser's cleanup in
    /// [`RepositoryManager::clone_repo`] deleted the winner's half-written cache.
    /// Serialized, the loser just waits and then reuses the winner's clone.
    pub(crate) fn clone_if_missing(
        &self,
        lock: &RepoLock,
        storage: &mut MetadataStorage,
        owner: &str,
        repo: &str,
        remote_url: &str,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> Result<BaseRepository, CloneIfMissingError> {
        lock.require(owner, repo)
            .map_err(CloneIfMissingError::WrongRepoLock)?;
        if self.repo_exists(owner, repo)
            && let Some(existing) = self.get_repo(storage, owner, repo, notices)
        {
            return Ok(existing);
        }
        // Either no clone, or a clone with no record: both fall through to
        // clone_repo, which adopts the second case and clones the first.
        self.clone_repo(storage, owner, repo, remote_url, notices)
            .map_err(CloneIfMissingError::Clone)
    }

    /// Clone-if-missing in a lock scope of its own.
    ///
    /// What a bare `owner/repo` spec needs before it can name the default branch,
    /// and the one repo-lock cycle the launch path takes outside
    /// [`crate::flows::workspace_clone::WorkspaceCloneManager::prepare_cold`].
    /// Folding it into that scope would mean holding this lock across the
    /// fast-attach `devpod status` that comes between them, so every sibling
    /// launch of the repository would queue behind a subprocess — a far worse
    /// trade than the uncontended flock it saves (devlaunch#200). Only the branch
    /// *name* crosses the gap, and the collapsed scope re-verifies
    /// clone-if-missing under its own lock.
    pub(crate) fn ensure_repo(
        &self,
        storage: &mut MetadataStorage,
        owner: &str,
        repo: &str,
        remote_url: &str,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> Result<BaseRepository, EnsureRepoError> {
        timing::stage_result(timing::Stage::HostPrep, || {
            let lock = self
                .hold_repo_lock(owner, repo)
                .map_err(EnsureRepoError::Lock)?;
            self.clone_if_missing(&lock, storage, owner, repo, remote_url, notices)
                .map_err(EnsureRepoError::from)
        })
    }

    // ----------------------------------------------------------- the fetch

    /// Sweep every head and tag into the bare cache.
    ///
    /// `limit` bounds the fetch, and matters because this runs under the repo
    /// lock: whoever holds that lock for the length of a fetch is somebody every
    /// other dl run wanting the same repository has to wait for. A launch is
    /// watched and interruptible, so it passes `None`. The background sweep is
    /// neither — it is a detached child in its own session, so a fetch of it that
    /// never returns is a repository wedged until reboot — and it passes
    /// [`BACKGROUND_FETCH_TIMEOUT`]. Reaching the bound fails like any other
    /// failure, in its own arm.
    pub(crate) fn fetch_repo(
        &self,
        storage: &mut MetadataStorage,
        owner: &str,
        repo: &str,
        limit: Option<Duration>,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> Result<(), FetchRepoError> {
        let bare = self.bare_dir(owner, repo);
        if !bare.exists() {
            return Err(FetchRepoError::NoLocalClone {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
                bare,
            });
        }

        notices.say(CacheNotice::FetchingUpdates {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        });
        let fetched = {
            let _span = timing::span("git fetch");
            self.git.fetch_all(&bare, limit)
        };
        if let Some(refused) = fetched.refusal() {
            return Err(match refused.how() {
                Failure::TimedOut => FetchRepoError::TimedOut {
                    owner: owner.to_owned(),
                    repo: repo.to_owned(),
                    limit,
                },
                _ => FetchRepoError::Refused {
                    reason: refused.reason().to_owned(),
                },
            });
        }

        if let Some(mut recorded) = storage.get_repository(owner, repo).cloned() {
            recorded.last_fetched = Some(Timestamp::now());
            match storage.add_repository(recorded) {
                Ok(store_notices) => {
                    notices.say_all(store_notices.into_iter().map(CacheNotice::Metadata));
                }
                Err(error) => return Err(FetchRepoError::NotRecorded(error)),
            }
        }
        // Last, where Python logs it: past the fetch and past the bookkeeping, so
        // the line means both are done.
        notices.say(CacheNotice::FetchedUpdates {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        });
        Ok(())
    }

    /// Fetch exactly one branch into the bare cache, and say what happened.
    ///
    /// The launch path's entire network budget. Where [`RepositoryManager::fetch_repo`]
    /// sweeps every head and tag, this moves one ref, so the time it can hold the
    /// repo lock is bounded by one branch's worth of objects rather than by the
    /// size of the repository's whole history of branches.
    ///
    /// Unconditional by design — no interval gate. The conditional version is
    /// more code and yields a mushy contract (fresh for branches you have not
    /// seen, stale for the ones you have); one single-ref fetch is noise next to
    /// the clone and `devpod up` this path already pays for.
    ///
    /// Deliberately does **not** write `last_fetched`: that is the broad sweep's
    /// bookkeeping, and claiming it here would suppress the sweep for a whole
    /// interval on the strength of having fetched one branch. Not writing it also
    /// keeps the repo-lock→metadata-lock nesting off this path.
    ///
    /// Writes a ref in the shared bare repository, so it must not run
    /// unserialized. Its one caller,
    /// [`crate::flows::workspace_clone::WorkspaceCloneManager::ensure_branch`],
    /// holds a [`RepoLock`] for this repository, which is what says so.
    pub(crate) fn fetch_ref(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> Result<FetchOutcome, UnsafeName> {
        // The branch is interpolated into a refspec that reaches git as argv, so
        // it is checked here rather than trusted. The caller usually holds a
        // WorkspaceId proving it, but the default-branch retry arrives from
        // stored metadata unproven.
        validate_ref_name(branch, NamePart::Ref)?;
        let bare = self.bare_dir(owner, repo);
        if !bare.exists() {
            // Not RefMissingOnRemote: nothing was asked of the remote, so nothing
            // is known about it. Sending the caller off to base a branch on the
            // default branch would be basing it in a cache that is equally absent.
            return Ok(FetchOutcome::Failed {
                reason: format!("no local clone of {owner}/{repo} at {}", bare.display()),
            });
        }

        // Past both guards, where Python logs it: a ref nothing could be fetched
        // for was never announced as being fetched.
        notices.say(CacheNotice::FetchingRef {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            branch: branch.to_owned(),
        });
        let fetched = {
            let _span = timing::span("git fetch");
            self.git.fetch_ref(&bare, branch)
        };
        Ok(match fetched.refusal() {
            None => FetchOutcome::Updated,
            // git reached the remote and was told the ref is not there: the one
            // case where a non-zero exit is an *answer*. Which case that is, the
            // client says — it is the module that reads git's words.
            Some(refused) => match refused.how() {
                Failure::RefMissingOnRemote => FetchOutcome::RefMissingOnRemote,
                _ => FetchOutcome::Failed {
                    reason: refused.reason().to_owned(),
                },
            },
        })
    }

    /// Whether the fetch interval has elapsed since this repository's last sweep.
    ///
    /// True when it has never been fetched, or when more than the interval has
    /// passed. A clock that cannot be subtracted — a stored timestamp this
    /// runtime cannot place in the local zone — answers true as well: the
    /// conservative reading is that the cache is due, since a needless fetch
    /// costs one pass and a skipped one costs the interval.
    pub(crate) fn should_fetch(&self, repository: &BaseRepository) -> bool {
        let Some(last) = &repository.last_fetched else {
            return true;
        };
        match seconds_since(last) {
            None => true,
            Some(elapsed) => elapsed > self.fetch_interval.as_secs() as i64,
        }
    }

    /// Fetch only if the fetch interval has elapsed since the last fetch.
    ///
    /// `limit` is passed straight to [`RepositoryManager::fetch_repo`]; see there
    /// for why it exists.
    pub(crate) fn lazy_fetch(
        &self,
        storage: &mut MetadataStorage,
        owner: &str,
        repo: &str,
        limit: Option<Duration>,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> Result<Fetched, LazyFetchError> {
        let Some(recorded) = storage.get_repository(owner, repo).cloned() else {
            return Err(LazyFetchError::NotInMetadata {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
            });
        };
        if !self.should_fetch(&recorded) {
            return Ok(Fetched::Skipped);
        }
        self.fetch_repo(storage, owner, repo, limit, notices)
            .map_err(LazyFetchError::Fetch)?;
        Ok(Fetched::Fetched)
    }

    // ------------------------------------------------------------- reading

    /// Whether a usable bare clone is on disk.
    ///
    /// `HEAD` and not merely the directory: a directory `git clone` was killed
    /// halfway through is not a clone, and nothing can be recovered from it.
    pub(crate) fn repo_exists(&self, owner: &str, repo: &str) -> bool {
        let bare = self.bare_dir(owner, repo);
        bare.exists() && bare.join("HEAD").exists()
    }

    /// The record for a repository that is really on disk.
    ///
    /// `None` for a record whose directory is gone — a restored backup, a
    /// hand-deleted cache, a half-finished `dl --purge`. The filesystem wins: a
    /// record alone is not a repository, and every caller reads a returned record
    /// as "the cache is ready".
    pub(crate) fn get_repo(
        &self,
        storage: &MetadataStorage,
        owner: &str,
        repo: &str,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> Option<BaseRepository> {
        let recorded = storage.get_repository(owner, repo)?.clone();
        if !self.repo_exists(owner, repo) {
            notices.say(CacheNotice::RecordWithoutClone {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
                bare: self.bare_dir(owner, repo),
            });
            return None;
        }
        Some(recorded)
    }

    /// Every managed repository, as recorded.
    pub(crate) fn list_repositories(&self, storage: &MetadataStorage) -> Vec<BaseRepository> {
        storage.list_repositories().into_iter().cloned().collect()
    }

    /// The default branch of the repository at `repo_path`, whichever shape it is.
    ///
    /// Three sources, in this order, and the order is the point: what the
    /// repository itself says, then what it remembers the remote saying, then a
    /// guess from the remote-tracking branches it happens to have.
    ///
    /// 1. `symbolic-ref HEAD` — for a bare clone, `HEAD` points straight at
    ///    `refs/heads/<branch>`.
    /// 2. `symbolic-ref refs/remotes/origin/HEAD` — the same question of a
    ///    non-bare clone.
    /// 3. `branch -r`, searched as text for `origin/main` and then
    ///    `origin/master`.
    /// 4. `main`.
    ///
    /// The namespace prefix is *stripped* rather than the last path segment
    /// taken: a branch name may contain slashes, so `split("/")[-1]` turned a
    /// default branch of `release/1.0` into `1.0` — a ref the repository does not
    /// have, recorded as the one every later operation targets.
    pub(crate) fn default_branch_of(&self, repo_path: &Path) -> String {
        for reference in ["HEAD", "refs/remotes/origin/HEAD"] {
            if let Some(named) = self.git.symbolic_ref(repo_path, reference).said() {
                return git::branch_in_symbolic_ref(&named).to_owned();
            }
        }
        if let Some(listed) = self.git.remote_branch_listing(repo_path).said() {
            for (marker, branch) in [("origin/main", "main"), ("origin/master", "master")] {
                if listed.contains(marker) {
                    return branch.to_owned();
                }
            }
        }
        FALLBACK_DEFAULT_BRANCH.to_owned()
    }

    /// The default branch for a repository, from the record or from the remote.
    ///
    /// Checks the local record first, then asks the remote without a clone, then
    /// falls back to `main`. The remote question is bounded inside the client (ten
    /// seconds): it stands in front of a guess, so a remote that hangs must cost a
    /// pause rather than the launch.
    pub fn get_default_branch(
        &self,
        storage: &MetadataStorage,
        owner: &str,
        repo: &str,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> String {
        let _stage = timing::stage(timing::Stage::HostPrep);
        if let Some(recorded) = self.get_repo(storage, owner, repo, notices)
            && let Some(named) = recorded.default_branch.named()
        {
            return named.to_owned();
        }
        let remote_url = format!("git@github.com:{owner}/{repo}.git");
        if let Some(answered) = self.git.ls_remote_symref_head(&remote_url).said()
            && let Some(branch) = git::head_branch_in_symref(&answered)
        {
            return branch;
        }
        FALLBACK_DEFAULT_BRANCH.to_owned()
    }

    // ------------------------------------------------------------ removal

    /// Remove a repository from management.
    ///
    /// [`RemoveScope::RecordAndDirectory`] takes the whole repository directory,
    /// the clones and the lock file with it — which is what makes it different
    /// from the cleanup a failed clone does, where the lock file is the one thing
    /// that must survive.
    ///
    /// Held for the #251 §7 public-API freeze — the `remove` verb. Only this
    /// module's tests call it today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn remove_repository(
        &self,
        storage: &mut MetadataStorage,
        owner: &str,
        repo: &str,
        scope: RemoveScope,
        notices: &mut dyn Notices<CacheNotice>,
    ) -> Result<(), RemoveRepositoryError> {
        match storage.remove_repository(owner, repo) {
            Ok(store_notices) => {
                notices.say_all(store_notices.into_iter().map(CacheNotice::Metadata));
            }
            Err(error) => return Err(RemoveRepositoryError::NotUnrecorded(error)),
        }
        if let RemoveScope::RecordAndDirectory = scope {
            remove_tree(&self.repo_dir(owner, repo))
                .map_err(RemoveRepositoryError::DirectoryLeft)?;
        }
        Ok(())
    }
}

/// Seconds from a recorded timestamp until now, or `None` if the two cannot be
/// compared.
///
/// The stored spelling is naive *local* time — that is what Python's
/// `datetime.now().isoformat()` writes — so both ends are placed in the system
/// zone and the difference taken there. A local time that does not exist or is
/// ambiguous (the hour a DST change removes or repeats) has no single instant, and
/// answers `None` rather than a guess.
fn seconds_since(recorded: &Timestamp) -> Option<i64> {
    let zone = jiff::tz::TimeZone::system();
    let then = recorded.at().to_zoned(zone).ok()?;
    Some(jiff::Zoned::now().timestamp().as_second() - then.timestamp().as_second())
}

// `pub(crate)` for the fixtures it defines: the fake git with real side effects,
// the temp cache and the real-git repository builders are wanted by
// `flows::workspace_clone`'s and `flows::migration`'s tests too, and three private
// copies of them would be three places for the wiring to drift.
#[cfg(test)]
pub(crate) mod tests {
    //! What the bare-clone cache does, at two seams.
    //!
    //! **The argv seam.** Every verb's whole argv, its working directory, its
    //! environment and its bound, asserted through the fake runner. That is where
    //! Python's `mock.patch("subprocess.run")` tests land, and it is what a
    //! rewrite of a body cannot preserve by accident.
    //!
    //! **Real git.** The recovery arms cannot be reached any other way: what
    //! distinguishes them is the state of a real directory (does `.bare` have a
    //! `HEAD`?) and the exit status of a real `git clone`, so a faked spawn decides
    //! the answer before the code does. Those tests build a "remote" that is a bare
    //! repository in the same temp directory — a real clone, no network — and are
    //! marked in their names.
    //!
    //! Ported from `test/test_worktree_repo_manager.py`,
    //! `test/integration/test_repo_manager_real.py`,
    //! `test/integration/test_repo_manager_recovery.py`,
    //! `test/integration/test_clone_race.py` and the fetch-count half of
    //! `test/test_cold_launch_fetches.py`.

    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Instant;

    use super::*;
    use crate::domain::model::Timestamp;
    use crate::runner::{
        CapturedText, DetachOutcome, Invocation, Outcome, ProcessRunner, Runner, SpawnSpec,
    };
    use devlaunch_test_support::{FakeRunner, Response};
    use jiff::civil;

    // ------------------------------------------------------- the fake git

    /// A [`FakeRunner`] with the side effects real git would have left behind.
    ///
    /// Two of them are load-bearing rather than decorative, and they are the two
    /// Python's `FakeGit` had: a bare clone must leave a `HEAD` or
    /// [`RepositoryManager::repo_exists`] reports the clone that was just made as
    /// absent, and a workspace clone must leave a `.git` or the workspace step does
    /// the same. Everything else about the fake — the recorder, the argv→response
    /// table, the quiet success by default — is the shared fake's.
    ///
    /// Deliberately *not* a holder of [`timing::exclusive`], unlike [`Cache`]: this
    /// is constructed inside the worker threads of the contention tests, and the
    /// exclusion is reentrant per thread rather than per test — a worker asking for
    /// a guard its own test already holds would wait for itself. The cache is the
    /// hook instead, because it is what the span-recording flows are driven over.
    pub(crate) struct FakeGit {
        fake: FakeRunner,
        extra: Vec<Effect>,
    }

    /// Something a test wants to happen when a given argv is spawned.
    type Effect = Box<dyn Fn(&[String])>;

    impl FakeGit {
        pub(crate) fn new() -> Self {
            Self {
                fake: FakeRunner::new(),
                extra: Vec::new(),
            }
        }

        /// Answer `response` to any call whose whole argv starts this way.
        #[must_use]
        pub(crate) fn with_script<I, S>(self, argv: I, response: Response) -> Self
        where
            I: IntoIterator<Item = S>,
            S: Into<String>,
        {
            self.fake.script(argv, response);
            self
        }

        /// A `git symbolic-ref` that answers `main`, which is what a clone of the
        /// fixture repositories would.
        #[must_use]
        pub(crate) fn headed_at_main(self) -> Self {
            self.with_script(
                ["git", "symbolic-ref"],
                Response::stdout("refs/heads/main\n"),
            )
        }

        /// Do this as well, whenever a call is made. For the effect a test needs
        /// that git would have had — a pull that materializes a pointer file.
        #[must_use]
        pub(crate) fn and_then(mut self, effect: impl Fn(&[String]) + 'static) -> Self {
            self.extra.push(Box::new(effect));
            self
        }

        pub(crate) fn argvs(&self) -> Vec<Vec<String>> {
            self.fake.argvs()
        }

        pub(crate) fn calls(&self) -> Vec<devlaunch_test_support::Call> {
            self.fake.calls()
        }

        pub(crate) fn call_count(&self) -> usize {
            self.fake.call_count()
        }

        pub(crate) fn forget_calls(&self) {
            self.fake.forget_calls();
        }

        /// The one call this test made, or a panic naming what it found instead.
        pub(crate) fn only_call(&self) -> devlaunch_test_support::Call {
            let calls = self.calls();
            assert_eq!(calls.len(), 1, "expected exactly one spawn: {calls:?}");
            calls.into_iter().next().expect("one call")
        }

        fn effects(&self, argv: &[String]) {
            if argv.first().map(String::as_str) == Some("git") {
                let rest: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
                match rest.as_slice() {
                    ["clone", "--bare", _url, dest] => {
                        let dest = Path::new(dest);
                        let _ = std::fs::create_dir_all(dest);
                        let _ = std::fs::write(dest.join("HEAD"), "ref: refs/heads/main\n");
                    }
                    ["clone", _src, dest] => {
                        let _ = std::fs::create_dir_all(Path::new(dest).join(".git"));
                    }
                    _ => {}
                }
            }
            for effect in &self.extra {
                effect(argv);
            }
        }
    }

    impl Runner for FakeGit {
        fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
            self.effects(&spec.invocation.argv());
            self.fake.capture(spec)
        }

        fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
            self.effects(&spec.invocation.argv());
            self.fake.passthrough(spec)
        }

        fn session(&self, spec: &SpawnSpec, on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
            self.effects(&spec.invocation.argv());
            self.fake.session(spec, on_stderr_line)
        }

        fn detach(&self, what: &Invocation) -> DetachOutcome {
            self.effects(&what.argv());
            self.fake.detach(what)
        }
    }

    /// `argv`s as slices of `&str`, so a test can compare against literals.
    pub(crate) fn as_strs(argvs: &[Vec<String>]) -> Vec<Vec<&str>> {
        argvs
            .iter()
            .map(|argv| argv.iter().map(String::as_str).collect())
            .collect()
    }

    // --------------------------------------------------------- the fixtures

    /// A cache in a temp directory: a `repos` tree and a `metadata.json`.
    pub(crate) struct Cache {
        pub(crate) dir: tempfile::TempDir,
        pub(crate) repos_dir: PathBuf,
        pub(crate) storage: MetadataStorage,
        /// See [`timing::exclusive`]. Last field, so it is dropped last.
        _serialized: timing::Exclusive,
    }

    /// A cache, and the timing exclusion for as long as it lives.
    ///
    /// Every flow reached from here opens a `host-prep` stage or records a span
    /// against the **process-global** registry, so a test holding one of these
    /// would otherwise write into whatever document a concurrent measured test had
    /// installed. Holding the exclusion in the fixture rather than per test is what
    /// makes it impossible for a new test to forget.
    pub(crate) fn a_cache() -> Cache {
        let serialized = timing::exclusive();
        let dir = tempfile::tempdir().expect("a temp dir");
        let repos_dir = dir.path().join("repos");
        let (storage, notices) =
            MetadataStorage::open(dir.path().join("metadata.json")).expect("a store");
        assert_eq!(notices, Vec::new(), "a fresh cache reports nothing");
        Cache {
            dir,
            repos_dir,
            storage,
            _serialized: serialized,
        }
    }

    impl Cache {
        /// A bare clone on disk, with a `HEAD`, as a real clone would leave it.
        pub(crate) fn given_bare_clone(&self, owner: &str, repo: &str) -> PathBuf {
            let bare = bare_dir(&self.repos_dir, owner, repo);
            std::fs::create_dir_all(&bare).expect("the bare directory");
            std::fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").expect("a HEAD");
            bare
        }

        /// A recorded repository, never fetched — the state the old interval gate
        /// treated as "fetch now, unconditionally".
        pub(crate) fn given_record(&mut self, owner: &str, repo: &str) -> BaseRepository {
            let recorded = BaseRepository::new(
                owner,
                repo,
                &format!("git@github.com:{owner}/{repo}.git"),
                bare_dir(&self.repos_dir, owner, repo),
            );
            self.storage
                .add_repository(recorded.clone())
                .expect("recorded");
            recorded
        }
    }

    /// The manager these tests drive, over `cache`'s repos directory.
    pub(crate) fn a_manager<'r>(cache: &Cache, git: Git<'r>) -> RepositoryManager<'r> {
        RepositoryManager::new(&cache.repos_dir, git)
    }

    /// A `Vec` to collect notices into, for a test that does not read them.
    pub(crate) fn ignoring() -> Vec<CacheNotice> {
        Vec::new()
    }

    // ------------------------------------------------------ real git helpers

    /// Run a real git command, failing loudly.
    ///
    /// Identity and signing are pinned per command rather than in a global config:
    /// a contributor with `commit.gpgsign = true` would otherwise see every commit
    /// here fail for want of a key, and a dozen red tests about the clone cache.
    pub(crate) fn run_git(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args([
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=T",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "protocol.file.allow=always",
            ])
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git is installed");
        assert!(
            output.status.success(),
            "git {args:?} in {cwd:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A "remote" that is a bare repository in `dir`, with one commit on `branch`.
    ///
    /// `branch` is the remote's `HEAD`, which is what the default-branch reading is
    /// asserted against: `main` is also what that reading answers when it could
    /// read nothing at all, so a `main`-headed remote pins nothing.
    pub(crate) fn a_remote_headed_at(dir: &Path, branch: &str, name: &str) -> RemoteRepo {
        let remote = dir.join(format!("{name}.git"));
        let work = dir.join(name);
        run_git(
            dir,
            &[
                "init",
                "--bare",
                &format!("--initial-branch={branch}"),
                &remote.display().to_string(),
            ],
        );
        run_git(
            dir,
            &[
                "clone",
                &remote.display().to_string(),
                &work.display().to_string(),
            ],
        );
        std::fs::write(work.join("README.md"), "hello\n").expect("a README");
        run_git(&work, &["add", "-A"]);
        run_git(&work, &["commit", "-m", "first"]);
        run_git(&work, &["push", "-u", "origin", branch]);
        RemoteRepo {
            url: remote.display().to_string(),
            path: remote,
            work,
        }
    }

    /// The fixture repository every real-git test clones: `main` plus
    /// `feature/test`.
    pub(crate) fn a_fixture_remote(dir: &Path) -> RemoteRepo {
        let remote = a_remote_headed_at(dir, "main", "remote");
        commit_on(&remote.work, "feature/test", "feature.txt", "Add feature");
        run_git(&remote.work, &["checkout", "main"]);
        remote
    }

    pub(crate) struct RemoteRepo {
        /// The remote as git is told about it.
        pub(crate) url: String,
        pub(crate) path: PathBuf,
        /// A working copy pushed from, for a test that moves the remote on.
        pub(crate) work: PathBuf,
    }

    /// Add one commit to `branch` in a working copy and push it; answer its sha.
    pub(crate) fn commit_on(work: &Path, branch: &str, file: &str, message: &str) -> String {
        run_git(work, &["checkout", "-B", branch]);
        std::fs::write(work.join(file), format!("{message}\n")).expect("the file");
        run_git(work, &["add", file]);
        run_git(work, &["commit", "-m", message]);
        run_git(work, &["push", "origin", branch]);
        head_sha(work)
    }

    pub(crate) fn head_sha(repo: &Path) -> String {
        run_git(repo, &["rev-parse", "HEAD"]).trim().to_owned()
    }

    /// Every ref name in a repository, sorted.
    pub(crate) fn refs_of(repo: &Path) -> Vec<String> {
        let mut refs: Vec<String> = run_git(repo, &["for-each-ref", "--format=%(refname)"])
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        refs.sort();
        refs
    }

    /// A real git client over this process's `git`.
    pub(crate) fn real_git() -> ProcessRunner {
        ProcessRunner::new()
    }

    // ---------------------------------------------------- refusing a write

    /// A directory whose mode this test tightened, restored when this drops.
    pub(crate) struct Denied {
        path: PathBuf,
        mode: u32,
    }

    impl Drop for Denied {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt as _;
            let _ =
                std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.mode));
        }
    }

    /// Make `directory` genuinely refuse writes, or answer `None`.
    ///
    /// **A `chmod` that returns successfully is not a `chmod` that denies
    /// anything.** The mode is stored and then ignored on Docker Desktop's and
    /// Colima's bind mounts, on some overlay and network mounts, and for any
    /// process holding `CAP_DAC_OVERRIDE` — none of which a `geteuid() == 0` guard
    /// notices, though root is checked too because it is the common case. Where
    /// that happens the operation under test quietly succeeds, the assertion that
    /// it was refused fails, and a contributor sees a red suite with no defect
    /// anywhere near it — and the usual response to that is to delete the test,
    /// which costs the coverage permanently.
    ///
    /// So the refusal is verified, not assumed: the mode is applied and then a
    /// write is actually attempted. `None` means this filesystem does not do this,
    /// and the caller steps aside.
    pub(crate) fn refusing_writes(directory: &Path) -> Option<Denied> {
        let denied = deny(directory, 0o500)?;
        let probe = directory.join(".probe-can-i-write");
        match std::fs::write(&probe, "x") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                None
            }
            Err(_) => Some(denied),
        }
    }

    /// [`refusing_writes`]'s sibling: a directory that cannot even be listed, and
    /// inside which a `stat` therefore answers "could not look" rather than "not
    /// there". Verified the same way, and `None` the same way.
    pub(crate) fn refusing_reads(directory: &Path) -> Option<Denied> {
        let denied = deny(directory, 0o000)?;
        match std::fs::read_dir(directory) {
            Ok(_) => None,
            Err(_) => Some(denied),
        }
    }

    /// Apply `mode`, remembering what to put back. `None` for root, which is
    /// refused by nothing.
    fn deny(directory: &Path, mode: u32) -> Option<Denied> {
        use std::os::unix::fs::PermissionsExt as _;
        // SAFETY: a bare `geteuid` syscall, which cannot fail and touches nothing.
        if unsafe { libc::geteuid() } == 0 {
            return None;
        }
        let was = std::fs::metadata(directory)
            .expect("the directory is there")
            .permissions()
            .mode()
            & 0o7777;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode))
            .expect("chmod is accepted");
        Some(Denied {
            path: directory.to_path_buf(),
            mode: was,
        })
    }

    // ========================================================== the layout

    #[test]
    fn the_layout_is_a_bare_a_lock_and_the_clones_side_by_side() {
        let repos = Path::new("/cache/repos");

        assert_eq!(repo_dir(repos, "owner", "repo"), repos.join("owner/repo"));
        assert_eq!(
            bare_dir(repos, "owner", "repo"),
            repos.join("owner/repo/.bare")
        );
        assert_eq!(
            repo_lock_path(repos, "owner", "repo"),
            repos.join("owner/repo/.lock")
        );
        assert_eq!(
            clone_dir(repos, "owner", "repo", "repo-main-zovomobo"),
            repos.join("owner/repo/repo-main-zovomobo"),
            "a clone is a sibling of the cache it was cut from, on one filesystem"
        );
    }

    #[test]
    fn opening_a_manager_creates_the_repos_directory() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let repos_dir = dir.path().join("not").join("yet").join("there");
        let fake = FakeGit::new();

        let manager = RepositoryManager::new(&repos_dir, Git::new(&fake));

        assert!(repos_dir.is_dir());
        assert_eq!(manager.repos_dir(), repos_dir);
    }

    #[test]
    fn a_repository_is_there_only_when_its_bare_clone_has_a_head() {
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        assert!(!manager.repo_exists("owner", "repo"), "nothing on disk");

        std::fs::create_dir_all(manager.repo_dir("owner", "repo")).expect("the repo directory");
        assert!(!manager.repo_exists("owner", "repo"), "no .bare");

        let bare = manager.bare_dir("owner", "repo");
        std::fs::create_dir_all(&bare).expect("the bare directory");
        assert!(
            !manager.repo_exists("owner", "repo"),
            "a directory git clone was killed halfway through is not a clone"
        );

        std::fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").expect("a HEAD");
        assert!(manager.repo_exists("owner", "repo"));
    }

    // ============================================================ the clone

    #[test]
    fn a_clone_asks_git_for_a_bare_one_and_records_what_it_got() {
        let mut cache = a_cache();
        let fake = FakeGit::new().headed_at_main();
        let manager = a_manager(&cache, Git::new(&fake));
        let mut notices = ignoring();

        let cloned = manager
            .clone_repo(
                &mut cache.storage,
                "owner",
                "repo",
                "https://github.com/owner/repo.git",
                &mut notices,
            )
            .expect("cloned");

        let bare = manager.bare_dir("owner", "repo");
        assert_eq!(
            as_strs(&fake.argvs())[0],
            [
                "git",
                "clone",
                "--bare",
                "https://github.com/owner/repo.git",
                &bare.display().to_string()
            ]
        );
        assert_eq!(cloned.owner, "owner");
        assert_eq!(cloned.repo, "repo");
        assert_eq!(cloned.remote_url, "https://github.com/owner/repo.git");
        assert_eq!(cloned.local_path, bare);
        assert_eq!(cloned.default_branch.named(), Some("main"));
        assert!(
            cloned.last_fetched.is_some(),
            "the sweep's clock starts here"
        );
        assert!(cloned.worktrees.is_empty());
        assert_eq!(
            cache.storage.get_repository("owner", "repo"),
            Some(&cloned),
            "and the record is on disk"
        );
        assert_eq!(
            notices,
            vec![
                CacheNotice::CloningRepository {
                    remote_url: "https://github.com/owner/repo.git".to_owned(),
                    bare: bare.clone(),
                },
                CacheNotice::ClonedRepository {
                    owner: "owner".to_owned(),
                    repo: "repo".to_owned(),
                },
            ],
            "the progress lines bracket the clone, in the order Python logs them"
        );
    }

    #[test]
    fn a_clone_of_a_repository_already_recorded_asks_git_nothing() {
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        cache.given_record("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        let existing = manager
            .clone_repo(
                &mut cache.storage,
                "owner",
                "repo",
                "https://github.com/owner/repo.git",
                &mut ignoring(),
            )
            .expect("the existing record");

        assert_eq!(existing.owner, "owner");
        assert_eq!(fake.call_count(), 0, "nothing was cloned");
    }

    #[test]
    fn a_clone_that_git_refused_names_what_git_said() {
        let mut cache = a_cache();
        let fake = FakeGit::new().with_script(
            ["git", "clone"],
            Response::failed(128, "fatal: repository not found\n"),
        );
        let manager = a_manager(&cache, Git::new(&fake));

        let failed = manager
            .clone_repo(
                &mut cache.storage,
                "owner",
                "repo",
                "https://github.com/owner/repo.git",
                &mut ignoring(),
            )
            .expect_err("not cloned");

        match failed {
            CloneError::GitRefused { refused, cleanup } => {
                assert_eq!(refused.reason(), "fatal: repository not found");
                assert_eq!(cleanup, Cleanup::Cleared);
            }
            other => panic!("git's own words, got {other:?}"),
        }
        assert_eq!(
            cache.storage.get_repository("owner", "repo"),
            None,
            "no record for a clone that does not exist"
        );
    }

    #[test]
    fn a_clone_git_said_nothing_about_carries_its_exit_status() {
        // This is the first thing a launch does for a repository nobody has cached
        // yet, so it is the failure a new user is most likely to meet first, and
        // quoting an uncaptured stderr raw met them with "Failed to clone
        // repository: None". The exit status is what is left to say.
        let mut cache = a_cache();
        let fake = FakeGit::new().with_script(["git", "clone"], Response::exited(128));
        let manager = a_manager(&cache, Git::new(&fake));

        let failed = manager
            .clone_repo(&mut cache.storage, "owner", "repo", "u", &mut ignoring())
            .expect_err("not cloned");

        match failed {
            CloneError::GitRefused { refused, .. } => {
                assert_eq!(refused.reason(), "git clone exited 128");
                assert!(!refused.reason().contains("None"));
            }
            other => panic!("an exit status, got {other:?}"),
        }
    }

    #[test]
    fn a_clone_on_disk_with_no_record_is_adopted_rather_than_cloned_over() {
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let fake = FakeGit::new().with_script(
            ["git", "symbolic-ref"],
            Response::stdout("refs/heads/release/1.0\n"),
        );
        let manager = a_manager(&cache, Git::new(&fake));
        let mut notices = ignoring();

        let adopted = manager
            .clone_repo(
                &mut cache.storage,
                "owner",
                "repo",
                "https://github.com/owner/repo.git",
                &mut notices,
            )
            .expect("adopted");

        assert_eq!(
            as_strs(&fake.argvs())
                .iter()
                .filter(|argv| argv.contains(&"clone"))
                .count(),
            0,
            "cloning over it is refused by git and would delete a live cache"
        );
        assert_eq!(
            adopted.default_branch.named(),
            Some("release/1.0"),
            "read off the adopted clone, prefix stripped rather than split"
        );
        assert_eq!(
            notices,
            vec![CacheNotice::AdoptedBareClone {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                bare: manager.bare_dir("owner", "repo"),
            }]
        );
        assert!(cache.storage.get_repository("owner", "repo").is_some());
    }

    #[test]
    fn a_partial_clone_with_no_head_is_cleared_and_cloned_fresh() {
        let mut cache = a_cache();
        let bare = bare_dir(&cache.repos_dir, "owner", "repo");
        std::fs::create_dir_all(bare.join("objects")).expect("the wreckage");
        std::fs::write(bare.join("half-written"), "nothing usable\n").expect("the wreckage");
        let fake = FakeGit::new().headed_at_main();
        let manager = a_manager(&cache, Git::new(&fake));
        let mut notices = ignoring();

        manager
            .clone_repo(&mut cache.storage, "owner", "repo", "u", &mut notices)
            .expect("cloned");

        assert!(
            !bare.join("half-written").exists(),
            "the wreckage was left in place"
        );
        assert!(bare.join("HEAD").exists(), "and the fake clone ran");
        assert_eq!(
            notices,
            vec![
                CacheNotice::ClearedPartialClone { bare: bare.clone() },
                CacheNotice::CloningRepository {
                    remote_url: "u".to_owned(),
                    bare,
                },
                CacheNotice::ClonedRepository {
                    owner: "owner".to_owned(),
                    repo: "repo".to_owned(),
                },
            ],
            "the removal is reported, because nothing else would say it happened"
        );
    }

    #[test]
    fn a_symlinked_cache_directory_is_refused_rather_than_followed_or_unlinked() {
        // Both of the other answers cost somebody their workspaces: following the
        // link empties a directory nobody named, and unlinking it reports the
        // clones gone while they sit on the other disk.
        let dir = tempfile::tempdir().expect("a temp dir");
        let elsewhere = dir.path().join("moved-cache");
        std::fs::create_dir_all(elsewhere.join("clone")).expect("the real cache");
        let link = dir.path().join("repos");
        std::os::unix::fs::symlink(&elsewhere, &link).expect("a symlink");

        let refused = remove_tree(&link).expect_err("a symlinked root is refused");

        match refused {
            RemoveTreeError::RootIsSymlink { path, points_at } => {
                assert_eq!(path, link);
                assert_eq!(
                    points_at.as_deref(),
                    Some(elsewhere.as_path()),
                    "the reader needs the real location, since rm -rf would take the link"
                );
            }
            other => panic!("a symlinked root, got {other:?}"),
        }
        assert!(link.is_symlink(), "the link is still there");
        assert!(
            elsewhere.join("clone").is_dir(),
            "and so is everything it names"
        );
    }

    #[test]
    fn removing_a_tree_that_was_never_there_is_not_a_failure() {
        let dir = tempfile::tempdir().expect("a temp dir");

        assert_eq!(
            remove_tree(&dir.path().join("absent")),
            Ok(TreeRemoval::WasNotThere),
            "a cleanup run twice is not a failure the second time"
        );
    }

    // ============================================================ the fetch

    #[test]
    fn the_sweep_fetches_every_head_and_tag_in_the_bare_cache() {
        let mut cache = a_cache();
        let bare = cache.given_bare_clone("owner", "repo");
        cache.given_record("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        manager
            .fetch_repo(&mut cache.storage, "owner", "repo", None, &mut ignoring())
            .expect("fetched");

        let call = fake.only_call();
        assert_eq!(
            as_strs(&[call.argv()])[0],
            [
                "git",
                "fetch",
                "origin",
                "+refs/heads/*:refs/heads/*",
                "--tags",
                "--prune"
            ]
        );
        assert_eq!(call.invocation().cwd.as_deref(), Some(bare.as_path()));
        assert!(
            cache
                .storage
                .get_repository("owner", "repo")
                .expect("the record")
                .last_fetched
                .is_some(),
            "the sweep's clock is what the interval is measured from"
        );
    }

    #[test]
    fn a_sweep_says_which_repository_it_is_fetching_and_that_it_finished() {
        // Python's two `logger.info` lines around the fetch, and both are worth
        // carrying: the first is what a user watches through a slow one, and the
        // second is what says the record's clock moved with it.
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        cache.given_record("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));
        let mut notices = ignoring();

        manager
            .fetch_repo(&mut cache.storage, "owner", "repo", None, &mut notices)
            .expect("fetched");

        assert_eq!(
            notices,
            vec![
                CacheNotice::FetchingUpdates {
                    owner: "owner".to_owned(),
                    repo: "repo".to_owned(),
                },
                CacheNotice::FetchedUpdates {
                    owner: "owner".to_owned(),
                    repo: "repo".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn a_sweep_with_no_clone_to_fetch_into_announces_nothing() {
        // The announcement sits past the guard, where Python's does: nothing was
        // fetched, so nothing claimed to be fetching.
        let mut cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));
        let mut notices = ignoring();

        manager
            .fetch_repo(&mut cache.storage, "owner", "repo", None, &mut notices)
            .expect_err("there is no clone to fetch into");

        assert_eq!(notices, Vec::new());
    }

    #[test]
    fn a_fetch_that_failed_says_it_started_and_never_says_it_finished() {
        // Which is the whole use of having two lines rather than one.
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let fake = FakeGit::new().with_script(
            ["git", "fetch"],
            Response::failed(128, "fatal: no such host\n"),
        );
        let manager = a_manager(&cache, Git::new(&fake));
        let mut notices = ignoring();

        manager
            .fetch_repo(&mut cache.storage, "owner", "repo", None, &mut notices)
            .expect_err("the remote is unreachable");

        assert_eq!(
            notices,
            vec![CacheNotice::FetchingUpdates {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
            }]
        );
    }

    #[test]
    fn the_background_sweeps_bound_reaches_the_spawn() {
        // The ceiling on how long a foreground launch of the same repository can be
        // made to wait behind the sweep's repo lock. Asserted at the spawn spec,
        // which is the only place the guarantee can be made.
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        manager
            .fetch_repo(
                &mut cache.storage,
                "owner",
                "repo",
                Some(BACKGROUND_FETCH_TIMEOUT),
                &mut ignoring(),
            )
            .expect("fetched");

        assert_eq!(BACKGROUND_FETCH_TIMEOUT, Duration::from_secs(300));
        match fake.only_call() {
            devlaunch_test_support::Call::Capture(spec) => {
                assert_eq!(spec.timeout, Some(BACKGROUND_FETCH_TIMEOUT));
            }
            other => panic!("a captured fetch, got {other:?}"),
        }
    }

    #[test]
    fn a_sweep_with_no_clone_to_fetch_into_says_so_before_spawning_anything() {
        let mut cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        let failed = manager
            .fetch_repo(
                &mut cache.storage,
                "nonexistent",
                "repo",
                None,
                &mut ignoring(),
            )
            .expect_err("no clone");

        assert!(matches!(failed, FetchRepoError::NoLocalClone { .. }));
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn a_sweep_that_ran_out_of_time_is_told_apart_from_one_that_failed() {
        // git writes new objects to a temp pack and moves refs only at the end, so
        // a fetch cut off partway leaves the clone usable — which is why the two
        // are different arms rather than one message.
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let timed_out = FakeGit::new().with_script(["git", "fetch"], Response::TimedOut);
        let manager = a_manager(&cache, Git::new(&timed_out));

        let failed = manager
            .fetch_repo(
                &mut cache.storage,
                "owner",
                "repo",
                Some(Duration::from_secs(300)),
                &mut ignoring(),
            )
            .expect_err("timed out");

        assert_eq!(
            failed,
            FetchRepoError::TimedOut {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                limit: Some(Duration::from_secs(300)),
            }
        );
    }

    #[test]
    fn a_sweep_git_said_nothing_about_carries_its_exit_status() {
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let fake = FakeGit::new().with_script(["git", "fetch"], Response::exited(128));
        let manager = a_manager(&cache, Git::new(&fake));

        let failed = manager
            .fetch_repo(&mut cache.storage, "owner", "repo", None, &mut ignoring())
            .expect_err("refused");

        assert_eq!(
            failed,
            FetchRepoError::Refused {
                reason: "git fetch exited 128".to_owned()
            },
            "same git, same silence, the same answer as the one-ref fetch beside it"
        );
    }

    // --- fetch_ref: the launch path's one network call ---------------------

    #[test]
    fn the_targeted_fetch_names_one_branch_and_pins_gits_locale() {
        let cache = a_cache();
        let bare = cache.given_bare_clone("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        let outcome = manager
            .fetch_ref("owner", "repo", "feature/x", &mut ignoring())
            .expect("a safe ref");

        assert_eq!(outcome, FetchOutcome::Updated);
        let call = fake.only_call();
        assert_eq!(
            as_strs(&[call.argv()])[0],
            [
                "git",
                "fetch",
                "origin",
                "+refs/heads/feature/x:refs/heads/feature/x"
            ],
            "a wildcard here is an unbounded fetch of every head and tag"
        );
        assert_eq!(call.invocation().cwd.as_deref(), Some(bare.as_path()));
        let env = &call.invocation().env;
        assert_eq!(env.entries.get("LC_ALL").map(String::as_str), Some("C"));
        assert_eq!(env.entries.get("LANGUAGE").map(String::as_str), Some("C"));
        assert_eq!(
            env.base,
            crate::runner::EnvBase::Parent,
            "the rest of the environment must survive — losing it breaks ssh auth"
        );
    }

    #[test]
    fn a_ref_the_remote_has_not_got_is_its_own_answer() {
        // Distinct from a failure because the caller does something different with
        // it — bases a new branch on the default branch — and reporting it as a
        // failure would send an ordinary "start a new branch" launch down the
        // offline path.
        let cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let fake = FakeGit::new().with_script(
            ["git", "fetch"],
            Response::failed(128, "fatal: couldn't find remote ref refs/heads/nosuch\n"),
        );
        let manager = a_manager(&cache, Git::new(&fake));

        assert_eq!(
            manager
                .fetch_ref("owner", "repo", "nosuch", &mut ignoring())
                .expect("safe"),
            FetchOutcome::RefMissingOnRemote
        );
    }

    #[test]
    fn a_ref_the_remote_has_not_got_is_its_own_answer_whatever_case_git_uses() {
        // Up to v2.20.0 git said `Couldn't find remote ref` — capital C, and
        // `die()` rather than `die(_())`, so pinning `LC_ALL=C` never covered it
        // (`remote.c:1785` at v2.20.0; lowercase and translated from v2.21.0). On
        // a host still running that git, an ordinary "start a new branch" launch
        // becomes a failure, because the answer reads as one.
        let cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let fake = FakeGit::new().with_script(
            ["git", "fetch"],
            Response::failed(128, "fatal: Couldn't find remote ref refs/heads/nosuch\n"),
        );
        let manager = a_manager(&cache, Git::new(&fake));

        assert_eq!(
            manager
                .fetch_ref("owner", "repo", "nosuch", &mut ignoring())
                .expect("safe"),
            FetchOutcome::RefMissingOnRemote
        );
    }

    #[test]
    fn an_unreachable_remote_carries_the_reason_it_gave() {
        let cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let fake = FakeGit::new().with_script(
            ["git", "fetch"],
            Response::failed(128, "fatal: Could not read from remote repository\n"),
        );
        let manager = a_manager(&cache, Git::new(&fake));

        match manager
            .fetch_ref("owner", "repo", "main", &mut ignoring())
            .expect("safe")
        {
            FetchOutcome::Failed { reason } => {
                assert!(
                    reason.contains("Could not read from remote repository"),
                    "{reason}"
                );
            }
            other => panic!("a failure carrying its reason, got {other:?}"),
        }
    }

    #[test]
    fn a_silent_fetch_failure_is_still_one_of_the_three_answers() {
        let cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let fake = FakeGit::new().with_script(["git", "fetch"], Response::exited(128));
        let manager = a_manager(&cache, Git::new(&fake));

        match manager
            .fetch_ref("owner", "repo", "main", &mut ignoring())
            .expect("safe")
        {
            FetchOutcome::Failed { reason } => assert!(reason.contains("128"), "{reason}"),
            other => panic!("a failure, got {other:?}"),
        }
    }

    #[test]
    fn the_targeted_fetch_never_advances_the_sweeps_clock() {
        // Advancing `last_fetched` here would suppress the background sweep for a
        // whole interval on the strength of having fetched a single branch — every
        // other ref starved by the thing meant to keep the launch path cheap. It
        // also keeps the repo→metadata lock nesting off this path entirely.
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        cache.given_record("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        manager
            .fetch_ref("owner", "repo", "main", &mut ignoring())
            .expect("safe");

        assert_eq!(
            cache
                .storage
                .get_repository("owner", "repo")
                .expect("the record")
                .last_fetched,
            None
        );
    }

    #[test]
    fn a_ref_that_would_reach_git_as_an_option_is_refused_before_the_spawn() {
        let cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        let refused = manager
            .fetch_ref("owner", "repo", "--upload-pack=evil", &mut ignoring())
            .expect_err("an unsafe ref");

        assert_eq!(refused.name, "--upload-pack=evil");
        assert_eq!(refused.part, NamePart::Ref);
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn a_missing_cache_is_a_failure_and_not_a_claim_about_the_remote() {
        // Reading it as "the remote has not got it" would send the caller off to
        // create the branch from a default branch that is equally not there.
        let mut cache = a_cache();
        cache.given_record("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        match manager
            .fetch_ref("owner", "repo", "main", &mut ignoring())
            .expect("safe")
        {
            FetchOutcome::Failed { reason } => assert!(reason.contains("owner/repo"), "{reason}"),
            other => panic!("a failure, got {other:?}"),
        }
        assert_eq!(fake.call_count(), 0);
    }

    // --- the interval gate ------------------------------------------------

    #[test]
    fn a_repository_never_fetched_is_always_due() {
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));
        let recorded = BaseRepository::new("o", "r", "u", PathBuf::from("/p"));

        assert!(manager.should_fetch(&recorded));
    }

    #[test]
    fn a_repository_fetched_within_the_interval_is_not_due() {
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = RepositoryManager::with_fetch_interval(
            &cache.repos_dir,
            Git::new(&fake),
            Duration::from_secs(3600),
        );
        let mut recorded = BaseRepository::new("o", "r", "u", PathBuf::from("/p"));
        recorded.last_fetched = Some(Timestamp::now());

        assert!(!manager.should_fetch(&recorded));

        recorded.last_fetched = Some(Timestamp::from_civil(civil::datetime(
            2001, 1, 1, 0, 0, 0, 0,
        )));
        assert!(manager.should_fetch(&recorded), "and long ago is due");
    }

    #[test]
    fn a_conditional_sweep_fetches_when_it_is_due_and_skips_when_it_is_not() {
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let mut recorded = cache.given_record("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        assert_eq!(
            manager
                .lazy_fetch(&mut cache.storage, "owner", "repo", None, &mut ignoring())
                .expect("fetched"),
            Fetched::Fetched
        );
        assert_eq!(fake.call_count(), 1);

        // The fetch above wrote `last_fetched`, so the next pass is inside the
        // interval.
        fake.forget_calls();
        recorded.last_fetched = Some(Timestamp::now());
        cache.storage.add_repository(recorded).expect("recorded");
        assert_eq!(
            manager
                .lazy_fetch(&mut cache.storage, "owner", "repo", None, &mut ignoring())
                .expect("skipped"),
            Fetched::Skipped
        );
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn a_conditional_sweep_of_a_repository_nobody_recorded_has_no_clock_to_read() {
        let mut cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        let failed = manager
            .lazy_fetch(
                &mut cache.storage,
                "nonexistent",
                "repo",
                None,
                &mut ignoring(),
            )
            .expect_err("no record");

        assert_eq!(
            failed,
            LazyFetchError::NotInMetadata {
                owner: "nonexistent".to_owned(),
                repo: "repo".to_owned()
            }
        );
    }

    #[test]
    fn a_conditional_sweep_carries_the_fetchs_failure_out() {
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        cache.given_record("owner", "repo");
        let fake = FakeGit::new().with_script(["git", "fetch"], Response::failed(1, "fail\n"));
        let manager = a_manager(&cache, Git::new(&fake));

        let failed = manager
            .lazy_fetch(&mut cache.storage, "owner", "repo", None, &mut ignoring())
            .expect_err("refused");

        assert!(matches!(
            failed,
            LazyFetchError::Fetch(FetchRepoError::Refused { .. })
        ));
    }

    // ==================================================== clone-if-missing

    #[test]
    fn clone_if_missing_returns_an_existing_cache_without_fetching_it() {
        // ensure_repo is the clone-if-missing primitive and nothing else:
        // freshness is the background sweep's job, and the launch path's one
        // network call is the targeted ref fetch. A fetch here would put an
        // unbounded round trip back under the repo lock (devlaunch#144).
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        // last_fetched = None is the strongest form of "the interval has elapsed":
        // the old lazy-fetch gate fetched unconditionally in this state.
        cache.given_record("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        let existing = manager
            .ensure_repo(
                &mut cache.storage,
                "owner",
                "repo",
                "https://github.com/owner/repo.git",
                &mut ignoring(),
            )
            .expect("the existing cache");

        assert_eq!(existing.owner, "owner");
        assert_eq!(fake.call_count(), 0, "not one spawn");
    }

    #[test]
    fn clone_if_missing_clones_when_there_is_nothing_there() {
        let mut cache = a_cache();
        let fake = FakeGit::new().headed_at_main();
        let manager = a_manager(&cache, Git::new(&fake));

        let cloned = manager
            .ensure_repo(&mut cache.storage, "owner", "repo", "u", &mut ignoring())
            .expect("cloned");

        assert_eq!(cloned.owner, "owner");
        assert!(manager.repo_exists("owner", "repo"));
    }

    #[test]
    fn only_the_lock_scope_mints_a_token_and_it_vouches_for_one_repository() {
        // A bare marker type would let a lock taken on owner/repo vouch for work on
        // owner/other: the lock genuinely held, the wrong repository genuinely
        // unserialized, and nothing in the signature able to tell.
        let mut cache = a_cache();
        let fake = FakeGit::new().headed_at_main();
        let manager = a_manager(&cache, Git::new(&fake));

        let lock = manager.hold_repo_lock("owner", "repo").expect("the lock");

        assert_eq!((lock.owner(), lock.repo()), ("owner", "repo"));
        assert!(lock.covers("owner", "repo"));
        assert_eq!(lock.contention(), Contention::WalkedIn);
        let wrong = manager
            .clone_if_missing(
                &lock,
                &mut cache.storage,
                "owner",
                "other",
                "u",
                &mut ignoring(),
            )
            .expect_err("a token for one repository cannot vouch for another");
        assert_eq!(
            wrong,
            CloneIfMissingError::WrongRepoLock(WrongRepoLock {
                held: ("owner".to_owned(), "repo".to_owned()),
                wanted: ("owner".to_owned(), "other".to_owned()),
            })
        );
        assert_eq!(fake.call_count(), 0, "and nothing was cloned for it");
    }

    #[test]
    fn the_lock_is_free_again_once_the_token_is_dropped() {
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        let first = manager.hold_repo_lock("owner", "repo").expect("the lock");
        let lock_path = manager.lock_path("owner", "repo");
        assert!(
            locks::run_if_lock_free(&lock_path, || ())
                .expect("no error")
                .is_none(),
            "the token holds the lock for as long as it lives"
        );
        drop(first);

        assert!(
            locks::run_if_lock_free(&lock_path, || ())
                .expect("no error")
                .is_some(),
            "and dropping it is the release"
        );
    }

    #[test]
    fn a_run_that_has_to_queue_is_told_which_repository_it_is_waiting_for() {
        // A first launch of a large repository can sit for a minute behind a
        // sibling's clone, and the two look identical from outside: a dl that has
        // printed nothing. The wait is announced *before* the blocking call, which
        // is the only moment at which it can be announced at all.
        // The contender runs on a thread of its own with a manager of its own, so
        // what it queues behind is the kernel's lock rather than this test's idea
        // of one: `flock` is per open file description, so two descriptions on one
        // path conflict inside one process exactly as they would across two.
        let cache = a_cache();
        let lock_path = repo_lock_path(&cache.repos_dir, "owner", "repo");
        let held = locks::hold_lock(&lock_path).expect("the lock");

        let (announced, waits) = mpsc::channel();
        let (queued, got_it) = mpsc::channel();
        let repos_dir = cache.repos_dir.clone();
        let contender = std::thread::spawn(move || {
            let fake = FakeGit::new();
            let mut manager = RepositoryManager::new(&repos_dir, Git::new(&fake));
            manager.watch_waits(move |wait| announced.send(wait).expect("the test listens"));
            let lock = manager.hold_repo_lock("owner", "repo").expect("eventually");
            queued.send(lock.contention()).expect("the test listens");
        });

        // Receiving the announcement is what makes this deterministic rather than a
        // sleep: the watcher fires exactly when the lock was found held and before
        // the blocking call, so it proves the contender is queued.
        let wait = waits
            .recv_timeout(Duration::from_secs(10))
            .expect("the contender must say what it is waiting for");
        assert_eq!(
            wait,
            RepoWait {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                lock_path: lock_path.clone(),
            }
        );
        assert!(
            got_it.try_recv().is_err(),
            "the contender got in while the lock was held"
        );

        drop(held);
        match got_it
            .recv_timeout(Duration::from_secs(10))
            .expect("the contender never got the lock after it was released")
        {
            Contention::Queued { .. } => {}
            Contention::WalkedIn => panic!("a run that queued must say so"),
        }
        contender.join().expect("the contender finished");
    }

    // ================================================= the default branch

    #[test]
    fn the_default_branch_is_read_from_head_then_the_remote_head_then_the_listing() {
        let cache = a_cache();
        let repo = cache.dir.path().join("some-clone");

        // 1. `symbolic-ref HEAD`, which is where a bare clone answers.
        let head = FakeGit::new().with_script(
            ["git", "symbolic-ref", "HEAD"],
            Response::stdout("refs/heads/release/1.0\n"),
        );
        assert_eq!(
            a_manager(&cache, Git::new(&head)).default_branch_of(&repo),
            "release/1.0",
            "the prefix is stripped, not the last segment taken"
        );
        assert_eq!(
            as_strs(&head.argvs()),
            [["git", "symbolic-ref", "HEAD"]],
            "and nothing further is asked once it answers"
        );

        // 2. the remote-tracking HEAD, which is where a non-bare clone answers.
        let remote_head = FakeGit::new()
            .with_script(["git", "symbolic-ref", "HEAD"], Response::exited(128))
            .with_script(
                ["git", "symbolic-ref", "refs/remotes/origin/HEAD"],
                Response::stdout("refs/remotes/origin/main\n"),
            );
        assert_eq!(
            a_manager(&cache, Git::new(&remote_head)).default_branch_of(&repo),
            "main"
        );

        // 3. the remote-tracking branches it happens to have, searched as text.
        for (listed, expected) in [
            ("  origin/HEAD -> origin/main\n  origin/main\n", "main"),
            ("  origin/master\n", "master"),
        ] {
            let listing = FakeGit::new()
                .with_script(["git", "symbolic-ref"], Response::exited(128))
                .with_script(["git", "branch", "-r"], Response::stdout(listed));
            assert_eq!(
                a_manager(&cache, Git::new(&listing)).default_branch_of(&repo),
                expected
            );
        }

        // 4. and a guess, when git would say nothing at all.
        let silent = FakeGit::new().with_script(["git"], Response::exited(128));
        assert_eq!(
            a_manager(&cache, Git::new(&silent)).default_branch_of(&repo),
            "main"
        );
    }

    #[test]
    fn a_recorded_default_branch_is_the_answer_without_asking_any_remote() {
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        let mut recorded = cache.given_record("owner", "repo");
        recorded.default_branch = RecordedDefaultBranch::Named("develop".to_owned());
        cache.storage.add_repository(recorded).expect("recorded");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        let named = manager.get_default_branch(&cache.storage, "owner", "repo", &mut ignoring());

        assert_eq!(named, "develop");
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn an_unrecorded_repository_asks_the_remote_for_its_head_and_then_guesses() {
        let cache = a_cache();
        let answered = FakeGit::new().with_script(
            ["git", "ls-remote"],
            Response::stdout("ref: refs/heads/trunk\tHEAD\n0123\tHEAD\n"),
        );
        let manager = a_manager(&cache, Git::new(&answered));

        let named = manager.get_default_branch(&cache.storage, "owner", "repo", &mut ignoring());

        assert_eq!(named, "trunk");
        let call = answered.only_call();
        assert_eq!(
            as_strs(&[call.argv()])[0],
            [
                "git",
                "ls-remote",
                "--symref",
                "git@github.com:owner/repo.git",
                "HEAD"
            ]
        );
        match call {
            devlaunch_test_support::Call::Capture(spec) => assert_eq!(
                spec.timeout,
                Some(Duration::from_secs(10)),
                "it stands in front of a guess, so a hung remote costs a pause"
            ),
            other => panic!("a captured ls-remote, got {other:?}"),
        }

        let silent = FakeGit::new().with_script(["git", "ls-remote"], Response::exited(128));
        assert_eq!(
            a_manager(&cache, Git::new(&silent)).get_default_branch(
                &cache.storage,
                "owner",
                "repo",
                &mut ignoring()
            ),
            "main"
        );
    }

    // ==================================================== reading and removal

    #[test]
    fn a_record_whose_clone_is_gone_is_not_a_repository() {
        let mut cache = a_cache();
        cache.given_record("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));
        let mut notices = ignoring();

        let found = manager.get_repo(&cache.storage, "owner", "repo", &mut notices);

        assert_eq!(found, None, "a record alone is not a repository");
        assert_eq!(
            notices,
            vec![CacheNotice::RecordWithoutClone {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                bare: manager.bare_dir("owner", "repo"),
            }]
        );
        assert!(
            cache.storage.get_repository("owner", "repo").is_some(),
            "and the record is left exactly as it was"
        );
    }

    #[test]
    fn repositories_are_listed_as_recorded() {
        let mut cache = a_cache();
        cache.given_record("owner1", "repo1");
        cache.given_record("owner2", "repo2");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));

        assert_eq!(manager.list_repositories(&cache.storage).len(), 2);
    }

    #[test]
    fn removing_a_repository_takes_the_record_and_optionally_the_directory() {
        let mut cache = a_cache();
        cache.given_bare_clone("owner", "repo");
        cache.given_record("owner", "repo");
        let fake = FakeGit::new();
        let manager = a_manager(&cache, Git::new(&fake));
        let repo_directory = manager.repo_dir("owner", "repo");

        manager
            .remove_repository(
                &mut cache.storage,
                "owner",
                "repo",
                RemoveScope::RecordOnly,
                &mut ignoring(),
            )
            .expect("removed");

        assert_eq!(cache.storage.get_repository("owner", "repo"), None);
        assert!(repo_directory.is_dir(), "the directory is left where it is");

        cache.given_record("owner", "repo");
        manager
            .remove_repository(
                &mut cache.storage,
                "owner",
                "repo",
                RemoveScope::RecordAndDirectory,
                &mut ignoring(),
            )
            .expect("removed");

        assert!(!repo_directory.exists(), "and the clones with it");
    }

    // ============================================================= real git

    #[test]
    fn real_git_clones_a_bare_cache_with_every_branch_and_no_working_tree() {
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_manager(&cache, Git::new(&runner));
        let mut storage = cache.storage;

        let cloned = manager
            .clone_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("cloned");

        let bare = manager.bare_dir("test", "repo");
        assert_eq!(cloned.remote_url, remote.url);
        assert!(
            bare.join("HEAD").exists(),
            "a bare clone's HEAD is at its root"
        );
        assert!(!bare.join(".git").exists(), "and it has no working tree");
        let refs = refs_of(&bare);
        assert!(refs.contains(&"refs/heads/main".to_owned()), "{refs:?}");
        assert!(
            refs.contains(&"refs/heads/feature/test".to_owned()),
            "{refs:?}"
        );
        assert_eq!(cloned.default_branch.named(), Some("main"));
    }

    #[test]
    fn real_git_cloning_the_same_repository_twice_answers_with_the_first_clone() {
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_manager(&cache, Git::new(&runner));
        let mut storage = cache.storage;

        let first = manager
            .clone_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("cloned");
        let second = manager
            .clone_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("the same clone");

        assert_eq!(first, second);
    }

    #[test]
    fn real_git_refuses_a_remote_that_is_not_there_and_leaves_no_record() {
        // For every failure reachable from here git removes the destination it
        // created before exiting, so the cleanup is not what makes the directory go
        // away. What is pinned is the state the cache is left in: no directory, no
        // record, and a lock file still where the next process will look for it.
        let cache = a_cache();
        let runner = real_git();
        let manager = a_manager(&cache, Git::new(&runner));
        let mut storage = cache.storage;
        let nowhere = cache.dir.path().join("no-such-repo.git");

        let failed = manager
            .ensure_repo(
                &mut storage,
                "test",
                "repo",
                &nowhere.display().to_string(),
                &mut ignoring(),
            )
            .expect_err("not cloned");

        assert!(matches!(
            failed,
            EnsureRepoError::Clone(CloneError::GitRefused { .. })
        ));
        assert!(!manager.bare_dir("test", "repo").exists());
        // The residue that would actually hurt: a record is every caller's answer
        // to "is the cache ready", and one naming a directory that is not there
        // sends the next launch to a path with nothing in it.
        assert_eq!(storage.get_repository("test", "repo"), None);
        // The cleanup deletes `.bare` and must not widen to the directory above it,
        // because that is where the lock this call was *holding* lives. Unlinking
        // an flock'd file is the classic self-defeating move: the holder still holds
        // an inode nobody else can see, and the next arrival locks a fresh file and
        // walks straight past it.
        assert!(
            manager.lock_path("test", "repo").exists(),
            "a failed clone took the repo lock file with it"
        );

        // And what the cleanup is for, as an outcome rather than a directory
        // listing: the run that failed left the cache in a state the next one can
        // clone into.
        let remote = a_fixture_remote(cache.dir.path());
        let recovered = manager
            .ensure_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("cloned");
        assert_eq!(recovered.default_branch.named(), Some("main"));
        assert!(manager.repo_exists("test", "repo"));
    }

    #[test]
    fn real_git_adopts_a_clone_on_disk_that_metadata_has_never_heard_of() {
        // Two ways in, indistinguishable from the inside: another process cloned it
        // just now and this process loaded its metadata before that one saved, or a
        // run died between the clone and the save. Both leave the same two facts —
        // a working `.bare`, no record — and in both the clone is the authority.
        let cache = a_cache();
        // Headed at `master`, not `main`: `main` is what the reading answers when
        // it could read nothing at all, so against a main-headed remote the
        // assertion below would be satisfied by the fallback.
        let remote = a_remote_headed_at(cache.dir.path(), "master", "adopted");
        let runner = real_git();
        let manager = a_manager(&cache, Git::new(&runner));
        let mut storage = cache.storage;

        manager
            .clone_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("cloned");
        let bare = manager.bare_dir("test", "repo");
        // A ref the remote does not have, so "the same clone" is a fact about this
        // directory's contents rather than about its mtime: a re-clone would
        // rebuild the refs from the remote and this one would be gone.
        run_git(
            &bare,
            &["update-ref", "refs/heads/only-here", &head_sha(&bare)],
        );
        let before = refs_of(&bare);
        assert!(before.contains(&"refs/heads/only-here".to_owned()));
        storage
            .remove_repository("test", "repo")
            .expect("forgotten");

        let adopted = manager
            .ensure_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("adopted");

        assert_eq!(
            refs_of(&bare),
            before,
            "the clone was rebuilt instead of adopted"
        );
        assert_eq!(adopted.local_path, bare);
        assert_eq!(adopted.remote_url, remote.url);
        assert_eq!(
            adopted.default_branch.named(),
            Some("master"),
            "the rebuilt record fell back to `main` instead of reading the clone"
        );
        assert!(storage.get_repository("test", "repo").is_some());
    }

    #[test]
    fn real_git_clears_a_partial_clone_and_replaces_it() {
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_manager(&cache, Git::new(&runner));
        let mut storage = cache.storage;
        let bare = manager.bare_dir("test", "repo");
        std::fs::create_dir_all(bare.join("objects")).expect("the wreckage");
        std::fs::write(bare.join("half-written"), "nothing usable\n").expect("the wreckage");

        let cloned = manager
            .ensure_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("cloned");

        assert!(!bare.join("half-written").exists());
        assert!(bare.join("HEAD").exists());
        assert_eq!(cloned.default_branch.named(), Some("main"));
        assert!(refs_of(&bare).contains(&"refs/heads/main".to_owned()));
    }

    #[test]
    fn real_git_reads_the_default_branch_off_the_clone_whatever_it_is_called() {
        // `master` because a repository whose default really is master and one this
        // could not read otherwise give the same answer. `release/1.0` because a
        // branch name may contain slashes and the reading used to take the segment
        // after the last one, silently recording a ref the repository does not have.
        for branch in ["master", "release/1.0", "feature/auth"] {
            let cache = a_cache();
            let remote = a_remote_headed_at(cache.dir.path(), branch, &branch.replace('/', "-"));
            let runner = real_git();
            let manager = a_manager(&cache, Git::new(&runner));
            let mut storage = cache.storage;

            let cloned = manager
                .ensure_repo(&mut storage, "test", "headed", &remote.url, &mut ignoring())
                .expect("cloned");

            assert_eq!(cloned.default_branch.named(), Some(branch));
            // And it names a ref that is really there, which is the property the
            // equality above is a proxy for.
            assert!(
                refs_of(&manager.bare_dir("test", "headed"))
                    .contains(&format!("refs/heads/{branch}")),
                "{branch}"
            );
        }
    }

    #[test]
    fn real_git_fetches_a_commit_pushed_after_the_clone_was_made() {
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_manager(&cache, Git::new(&runner));
        let mut storage = cache.storage;
        manager
            .clone_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("cloned");
        let bare = manager.bare_dir("test", "repo");
        let pushed = commit_on(&remote.work, "main", "new_file.txt", "Add new file");

        manager
            .fetch_repo(&mut storage, "test", "repo", None, &mut ignoring())
            .expect("fetched");

        assert_eq!(
            run_git(&bare, &["rev-parse", "refs/heads/main"]).trim(),
            pushed,
            "the sweep's refspec moves the local head, not just a tracking ref"
        );
    }

    #[test]
    fn real_git_leaves_a_record_alone_when_only_the_directory_is_gone() {
        // A restored backup, a hand-deleted cache, a half-finished `dl --purge`.
        // The record is the stale one here, which is the opposite of the adoption
        // case, and the resolution is the same principle: the filesystem wins.
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_manager(&cache, Git::new(&runner));
        let mut storage = cache.storage;
        manager
            .clone_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("cloned");
        assert!(
            manager
                .get_repo(&storage, "test", "repo", &mut ignoring())
                .is_some()
        );

        std::fs::remove_dir_all(manager.bare_dir("test", "repo")).expect("only the directory goes");

        assert!(storage.get_repository("test", "repo").is_some());
        assert_eq!(
            manager.get_repo(&storage, "test", "repo", &mut ignoring()),
            None
        );
        assert!(!manager.repo_exists("test", "repo"));

        let recovered = manager
            .ensure_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("cloned back");
        assert!(manager.repo_exists("test", "repo"));
        assert_eq!(recovered.default_branch.named(), Some("main"));
    }

    #[test]
    fn real_git_writes_no_record_for_a_clone_the_filesystem_refused() {
        // The first version of this test made the whole repos_dir unwritable, and
        // that never reached the code it was about: `ensure_repo` takes the repo
        // lock first, and taking it creates the directory the lock file lives in,
        // so the call died there with the clone and every storage write
        // unexecuted. So the directories the lock needs are made first and only the
        // *clone* is blocked, which puts the failure where the claim lives.
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_manager(&cache, Git::new(&runner));
        let mut storage = cache.storage;
        let repo_directory = manager.repo_dir("test", "repo");
        std::fs::create_dir_all(&repo_directory).expect("the repo directory");
        std::fs::write(manager.lock_path("test", "repo"), "").expect("the lock file");

        let Some(_denied) = refusing_writes(&repo_directory) else {
            return; // this filesystem does not enforce directory modes
        };

        let failed = manager
            .ensure_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect_err("the clone cannot be written");

        assert!(matches!(failed, EnsureRepoError::Clone(_)), "{failed:?}");
        assert_eq!(
            storage.get_repository("test", "repo"),
            None,
            "a record was written for a clone that does not exist"
        );
        assert!(!manager.bare_dir("test", "repo").exists());
        assert!(manager.lock_path("test", "repo").exists());
    }

    // ---------------------------------------------------- racing the clone

    #[test]
    fn real_git_a_waiting_run_adopts_the_clone_it_waited_for() {
        // The race `locks.rs` was written for, with the schedule forced rather than
        // hoped for: the obvious version starts two runs at once and asserts they
        // converge, and that version passed a quarter of the time with the lock
        // removed entirely — when the two happen to serialize by luck, the second
        // one's stale metadata sends it down the adoption path and every assertion
        // holds.
        //
        // Two `MetadataStorage` handles over one file, the loser's loaded while the
        // file is still empty: that is what two dl processes are just after
        // startup, and it is why the loser finds a `.bare` its own records have
        // never heard of.
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_manager(&cache, Git::new(&runner));
        let metadata = cache.dir.path().join("metadata.json");
        let (mut winner_storage, _) = MetadataStorage::open(&metadata).expect("the winner's store");

        let held = manager.hold_repo_lock("test", "repo").expect("the lock");
        let bare = manager.bare_dir("test", "repo");

        let (announced, waits) = mpsc::channel();
        let (finished, adopted) = mpsc::channel();
        let repos_dir = cache.repos_dir.clone();
        let loser_metadata = metadata.clone();
        let url = remote.url.clone();
        let loser = std::thread::spawn(move || {
            let runner = ProcessRunner::new();
            let mut manager = RepositoryManager::new(&repos_dir, Git::new(&runner));
            manager.watch_waits(move |wait| announced.send(wait).expect("the test listens"));
            // Loaded while the file is still empty — the winner has not cloned yet,
            // because it waits for the announcement below. That ordering *is* the
            // substance of the race.
            let (mut storage, _) = MetadataStorage::open(&loser_metadata).expect("the store");
            let got = manager
                .ensure_repo(&mut storage, "test", "repo", &url, &mut ignoring())
                .expect("the loser gets the clone it waited for");
            finished.send(got.local_path).expect("the test listens");
        });

        // Receiving the announcement proves the loser is queued: no sleep, no
        // /proc peeking.
        waits
            .recv_timeout(Duration::from_secs(10))
            .expect("the loser must queue behind the held lock");

        manager
            .clone_repo(
                &mut winner_storage,
                "test",
                "repo",
                &remote.url,
                &mut ignoring(),
            )
            .expect("the winner clones");
        // Written inside the clone, still under the lock. Nothing the loser is
        // allowed to do removes it: adopting the clone leaves it, re-cloning over it
        // is refused by git, and clearing it first would take this with it.
        std::fs::write(bare.join("winner-was-here"), "x").expect("the marker");
        drop(held);

        assert_eq!(
            adopted
                .recv_timeout(Duration::from_secs(120))
                .expect("the loser never finished"),
            bare
        );
        loser.join().expect("the loser finished");

        assert!(
            bare.join("winner-was-here").exists(),
            "the loser destroyed the winner's clone"
        );
        assert!(!head_sha(&bare).is_empty());
        let (fresh, _) = MetadataStorage::open(&metadata).expect("a fresh store");
        assert_eq!(
            fresh.repositories().keys().collect::<Vec<_>>(),
            ["test/repo"],
            "one record, whichever writer got there last"
        );
        let mut leaves: Vec<String> = std::fs::read_dir(manager.repo_dir("test", "repo"))
            .expect("a listing")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        leaves.sort();
        assert_eq!(leaves, [".bare", ".lock"]);
    }

    #[test]
    fn real_git_a_run_queues_behind_a_lock_another_process_holds() {
        // The one true cross-process claim: threads share a GIL-less process but
        // `flock` is per open file description, so the in-process tests above pin
        // the same kernel lock. What they cannot pin is that the lock is visible to
        // a *different program* — here `flock(1)` from util-linux, taking the same
        // advisory lock on the same file.
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let lock_path = repo_lock_path(&cache.repos_dir, "test", "repo");
        std::fs::create_dir_all(lock_path.parent().expect("a parent")).expect("the repo directory");

        // The shell takes the lock on a descriptor of its own and then `exec`s, so
        // the process holding it is the one this test can kill.
        let mut holder = Command::new("sh")
            .arg("-c")
            .arg(r#"exec 9>"$1"; flock --exclusive 9; exec sleep 300"#)
            .arg("sh")
            .arg(&lock_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("a shell and flock(1) from util-linux");

        let deadline = Instant::now() + Duration::from_secs(10);
        while locks::run_if_lock_free(&lock_path, || ())
            .expect("no error")
            .is_some()
        {
            assert!(Instant::now() < deadline, "the holder never took the lock");
            std::thread::sleep(Duration::from_millis(10));
        }

        let (done, finished) = mpsc::channel();
        let repos_dir = cache.repos_dir.clone();
        let metadata = cache.dir.path().join("metadata.json");
        let url = remote.url.clone();
        let queueing = std::thread::spawn(move || {
            let runner = ProcessRunner::new();
            let manager = RepositoryManager::new(&repos_dir, Git::new(&runner));
            let (mut storage, _) = MetadataStorage::open(&metadata).expect("the store");
            let cloned = manager
                .ensure_repo(&mut storage, "test", "repo", &url, &mut ignoring())
                .expect("cloned once the holder let go");
            done.send(cloned.local_path).expect("the test listens");
        });

        assert!(
            finished.recv_timeout(Duration::from_millis(300)).is_err(),
            "the run walked straight past a lock another process holds"
        );
        holder.kill().expect("the holder is killable");
        holder.wait().expect("reaping the holder");

        let bare = bare_dir(&cache.repos_dir, "test", "repo");
        assert_eq!(
            finished
                .recv_timeout(Duration::from_secs(120))
                .expect("a run that queued must proceed once the lock is free"),
            bare
        );
        queueing.join().expect("the queueing run finished");
        assert!(!head_sha(&bare).is_empty());
    }
}
