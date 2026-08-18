//! One workspace clone per branch, cut from the bare cache next to it.
//!
//! Ported from `devlaunch/worktree/workspace_clone.py`. The cold path in order:
//! bare-clone cache → targeted fetch and branch → workspace clone → remote
//! repointed at the forge → checkout → git-lfs materialized → recorded.
//!
//! ```text
//! repos_dir/blooop/devlaunch/
//! ├── .bare/                          the bare git repository
//! ├── devlaunch-main-zovomobo/            a workspace clone
//! └── devlaunch-feature-auth-poliseno/    another
//! ```
//!
//! The leaf names are [`WorkspaceId::value`] — the same string that names the
//! devpod workspace. A bare branch name would be unique only *within* its parent,
//! which is what let a downstream consumer reading one path component collapse
//! every branch of a repository onto a single identity
//! (kinisi-robotics/kinisi_ros#9766).
//!
//! # What this module is careful about, in one list
//!
//! - **One lock scope for the whole cold path** (devlaunch#200):
//!   [`WorkspaceCloneManager::prepare_cold`] takes the per-repo lock once and
//!   every step under it takes the [`RepoLock`] that scope minted.
//! - **The clone hardlinks the cache's objects**, which is what makes a workspace
//!   per branch affordable, and no flag guards it — see
//!   [`WorkspaceCloneManager::prepare_workspace`].
//! - **git-lfs is decided by pointer *content***, three phases, cache before
//!   network — see [`WorkspaceCloneManager::materialize_lfs`].
//! - **A record's clone directory is named in exactly one place**
//!   ([`WorkspaceCloneManager::resolve_clone_path`]), because the delete guard and
//!   the delete used to name it separately and could disagree (devlaunch#174).
//! - **Staleness is reported as a value, not a log line** ([`BranchBase`]), because
//!   `wf` reads dl's output and a launch from an unrefreshed base has to be
//!   distinguishable (devlaunch#245).
//!
//! Nothing here prints: every warning Python wrote is a [`CacheNotice`] carrying
//! that line's data, and every failure is a typed error.

// The launch path (M7) and the lifecycle flows (M6) are the remaining consumers.
#![allow(dead_code)] // consumed from M6/M7

use std::path::{Path, PathBuf};

use super::branch_manager::{BranchError, BranchManager, EnsureBranch};
use super::repo_manager::{
    CacheNotice, Cleanup, CloneError, CloneIfMissingError, FetchOutcome, RemoveTreeError, RepoLock,
    RepositoryManager, TreeRemoval, WrongRepoLock, clone_dir, remove_tree,
};
use crate::clients::git::{self, Git};
use crate::domain::config::WorktreeConfig;
use crate::domain::locks::LockError;
use crate::domain::metadata::MetadataStorage;
use crate::domain::model::WorktreeInfo;
use crate::domain::workspace_id::{NamePart, UnsafeName, WorkspaceId, validate_ref_name};
use crate::timing;

/// The remote every workspace clone talks to.
const ORIGIN: &str = "origin";

/// What `default_branch` is taken to be when no record says.
const FALLBACK_DEFAULT_BRANCH: &str = "main";

// -------------------------------------------------------- the base report

/// Which ref this launch is standing on, and whether anything refreshed it.
///
/// Two arms and no third: every path through
/// [`WorkspaceCloneManager::ensure_branch`] answers one of these, so "the fetch
/// quietly did not back the base" cannot exist as an unnamed state.
///
/// Deliberately not an error (devlaunch#144: launch-from-cache is the offline
/// contract) and deliberately not only a notice (devlaunch#245: the caller — and
/// `wf`, reading dl's output — must be able to tell a fresh base from a stale
/// one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchBase {
    /// The ref this launch runs on was fetched from the remote this call.
    ///
    /// Either the requested branch itself was refreshed, or the branch is new and
    /// the base it was cut from was. Carries no payload: "your tip is current" is
    /// the whole message.
    Fresh,
    /// The launch proceeds on a ref nothing refreshed this call.
    ///
    /// `base` names that ref — the default branch a new branch was cut from, the
    /// cache's own `HEAD` when no default branch could even be named, or the
    /// requested branch itself when its fetch failed. `reason` is why nothing
    /// refreshed it, carried rather than reconstructed at the print site.
    Stale { base: String, reason: String },
}

/// The name-or-why of a repository's default branch.
///
/// A pair of locals held this before, and nothing stopped them disagreeing: an
/// error string beside a name, or neither set. The two facts travel as one value,
/// and "no name" carries its reason by construction rather than by a second
/// variable being consulted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DefaultBranch {
    /// The repository's default branch is known, and this is it.
    Named(String),
    /// No default branch could be named, and this is why.
    ///
    /// One arm for both ways that happens — the resolver could not answer, or it
    /// answered with an empty name — because what follows is the same for each:
    /// nothing to fetch, and a new branch cut from the bare cache's own `HEAD`.
    /// The reason is what tells the two apart afterwards, and it is carried rather
    /// than rebuilt at the print site.
    Unknown { reason: String },
}

impl DefaultBranch {
    /// What a new branch is cut from, whichever arm this is.
    ///
    /// Exists so the two call sites do not each hand-roll the fallback.
    fn start_point(&self) -> &str {
        match self {
            DefaultBranch::Named(name) => name,
            DefaultBranch::Unknown { .. } => "HEAD",
        }
    }
}

/// Whether `path` is there: yes, no, or "could not look".
///
/// Three-valued because the third answer is the dangerous one to fold into
/// either neighbour. A plain existence check swallows ENOENT, ENOTDIR, EBADF and
/// ELOOP and re-raises everything else, so a clone whose parent is mode `000`
/// raised on some Python versions and answered `false` on others — and `false` is
/// the answer that sends [`WorkspaceCloneManager::resolve_clone_path`] off to name
/// a *different* directory.
///
/// One `lstat`, and a symlink counts as present without being followed: the link
/// itself is a thing that is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    There,
    NotThere,
    CouldNotLook,
}

fn presence_of(path: &Path) -> Presence {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Presence::There,
        Err(error) => match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => Presence::NotThere,
            _ => Presence::CouldNotLook,
        },
    }
}

/// Whether git-lfs is installed, as this manager sees it.
///
/// A value rather than a call at each use, so a test can drive both arms without
/// touching this process's PATH — and so the answer is one fact for the length of
/// a run rather than a PATH lookup per probe. Python asked `shutil.which` inside
/// the probe; nothing installs or removes git-lfs mid-launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitLfs {
    Installed,
    NotInstalled,
}

impl GitLfs {
    /// What this machine's PATH says.
    pub(crate) fn detected() -> Self {
        if git::lfs_is_installed() {
            GitLfs::Installed
        } else {
            GitLfs::NotInstalled
        }
    }
}

// --------------------------------------------------------------- errors

/// Why a branch could not be ensured in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnsureBranchError {
    WrongRepoLock(WrongRepoLock),
    /// The branch could not be created, which is where an empty cache is
    /// discovered: it is the first step that actually consults it.
    Branch(BranchError),
}

/// Why a workspace clone could not be prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrepareWorkspaceError {
    WrongRepoLock(WrongRepoLock),
    /// The repository directory the clone goes in could not be created.
    ParentNotCreated {
        path: PathBuf,
        reason: String,
    },
    /// `git clone` refused. `cleanup` is what became of the half-made clone.
    CloneRefused {
        reason: String,
        cleanup: Cleanup,
    },
    /// The clone exists but still points at the local bare cache, which is not a
    /// workspace anybody can push from.
    RemoteNotRepointed {
        reason: String,
    },
    /// Neither the requested branch nor the default branch is on the remote, so
    /// there is nothing to cut the branch from.
    NoStartPoint {
        branch: String,
        default_branch: String,
    },
    /// A name this step interpolates into a ref could not be trusted, so nothing
    /// was asked of git. In practice this is the *recorded* default branch — the
    /// one ref on this path that does not arrive inside a [`WorkspaceId`] — and the
    /// refusal says which part it judged.
    UnsafeRefName(UnsafeName),
    CheckoutRefused {
        branch: String,
        reason: String,
    },
    /// Neither the cache nor the forge could supply the git-lfs objects, so the
    /// workspace still holds pointer files. The next launch retries, because the
    /// gate is pointer content and not "did we just clone this".
    LfsNotMaterialized {
        reason: String,
    },
}

/// Why a cold launch's host-side preparation could not finish.
#[derive(Debug)]
pub(crate) enum PrepareColdError {
    /// The triple is not one a workspace can be named from. Refused before
    /// anything was locked or written on its behalf.
    UnsafeTriple(UnsafeName),
    Lock(LockError),
    WrongRepoLock(WrongRepoLock),
    Clone(CloneError),
    Branch(EnsureBranchError),
    Workspace(PrepareWorkspaceError),
}

impl From<CloneIfMissingError> for PrepareColdError {
    fn from(error: CloneIfMissingError) -> Self {
        match error {
            CloneIfMissingError::WrongRepoLock(wrong) => PrepareColdError::WrongRepoLock(wrong),
            CloneIfMissingError::Clone(failed) => PrepareColdError::Clone(failed),
        }
    }
}

/// What a cold launch's host-side preparation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedWorkspace {
    /// The workspace clone's path, which is what dl hands devpod.
    pub(crate) path: PathBuf,
    /// Which ref the workspace is standing on, and whether it is current.
    pub(crate) base: BranchBase,
}

/// What removing a workspace clone did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Removed {
    /// The clone was there and is gone.
    Clone,
    /// There was no clone to remove — or dl could not name one, which answers the
    /// same way: it removed nothing, and says so.
    Nothing,
}

/// Why a workspace clone could not be removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoveWorkspaceError {
    /// The triple is not one a clone directory can be derived from.
    UnsafeTriple(UnsafeName),
    /// The directory is still there, and this is why.
    DirectoryLeft(RemoveTreeError),
}

// ---------------------------------------------------------- the manager

/// The workspace clones, over one `repos_dir`.
pub(crate) struct WorkspaceCloneManager<'r> {
    repos_dir: PathBuf,
    git: Git<'r>,
    repo_manager: RepositoryManager<'r>,
    branch_manager: BranchManager<'r>,
    lfs: GitLfs,
}

impl std::fmt::Debug for WorkspaceCloneManager<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceCloneManager")
            .field("repos_dir", &self.repos_dir)
            .field("lfs", &self.lfs)
            .finish_non_exhaustive()
    }
}

impl<'r> WorkspaceCloneManager<'r> {
    /// A manager over the cache `config` describes.
    pub(crate) fn from_config(config: &WorktreeConfig, git: Git<'r>) -> Self {
        Self::new(
            &config.repos_dir,
            std::time::Duration::from_secs(config.fetch_interval),
            git,
            GitLfs::detected(),
        )
    }

    pub(crate) fn new(
        repos_dir: impl Into<PathBuf>,
        fetch_interval: std::time::Duration,
        git: Git<'r>,
        lfs: GitLfs,
    ) -> Self {
        let repos_dir = repos_dir.into();
        Self {
            repo_manager: RepositoryManager::with_fetch_interval(
                repos_dir.clone(),
                git,
                fetch_interval,
            ),
            repos_dir,
            git,
            branch_manager: BranchManager::new(git),
            lfs,
        }
    }

    pub(crate) fn repo_manager(&self) -> &RepositoryManager<'r> {
        &self.repo_manager
    }

    pub(crate) fn repo_manager_mut(&mut self) -> &mut RepositoryManager<'r> {
        &mut self.repo_manager
    }

    pub(crate) fn repos_dir(&self) -> &Path {
        &self.repos_dir
    }

    // -------------------------------------------------------------- paths

    /// The directory a workspace clone lives in.
    ///
    /// Goes through [`WorkspaceId`], so this path cannot be built from an
    /// unvalidated ref: there is no other way to name the leaf. That closes the
    /// gap where this was the one of three ref-consuming paths with no guard,
    /// because the old validator returned a naked string that carried no evidence
    /// of having been checked.
    pub(crate) fn workspace_path(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<PathBuf, UnsafeName> {
        let workspace = WorkspaceId::new(owner, repo, branch)?;
        Ok(self.path_of(&workspace))
    }

    /// The directory a *validated* triple's clone lives in.
    fn path_of(&self, workspace: &WorkspaceId) -> PathBuf {
        clone_dir(
            &self.repos_dir,
            workspace.owner(),
            workspace.repo(),
            &workspace.value(),
        )
    }

    /// The one directory a record's clone lives in, or `None` if dl cannot say.
    ///
    /// Every caller that has to name a record's clone comes through here: the
    /// `dl <ws> rm` guard, the `--ls --json` row, and the delete itself. They used
    /// to name it separately and could disagree (devlaunch#174). The guard read
    /// `local_path` unconditionally while the delete fell back to the derivation
    /// whenever that path was not on disk, so a record left pointing somewhere
    /// stale had the guard clearing an *absent* directory — "nothing there holds
    /// anything" — while the delete removed the derived one, which was the one
    /// holding the work. Exit 0, no `--force`, nothing said. A guard is only a
    /// guard if it inspects what the delete will actually remove.
    ///
    /// The recorded path wins when it is usable, which is what keeps clones made
    /// before the current id scheme removable with no migration.
    ///
    /// **Usable has to mean absolute, not merely non-empty.** An empty recorded
    /// path is the working directory, whose existence check passes — so it passed
    /// both of the old tests and sent the recursive delete at dl's own working
    /// directory. A clone path is always absolute, so anything relative is a
    /// record dl cannot honour.
    ///
    /// **`None` means dl cannot name a directory at all**, which happens when the
    /// recorded path is unusable *and* the derivation refuses the record's own
    /// triple. Every caller has to treat that as a refusal rather than as an empty
    /// answer, which is why it is an absent answer rather than an error: deriving
    /// raises on an unsafe ref, and one hand-edited record must not be able to
    /// take down the whole of `dl --ls --json`.
    pub(crate) fn resolve_clone_path(
        &self,
        recorded: &WorktreeInfo,
        notices: &mut Vec<CacheNotice>,
    ) -> Option<PathBuf> {
        let path = &recorded.local_path;
        if path.is_absolute() && presence_of(path) != Presence::NotThere {
            // There, or "dl was not allowed to look". Both keep the record: a path
            // dl cannot stat is not a path it has established the absence of, and
            // deriving a *different* directory off the back of that would put the
            // guard and the delete back on two answers — which is the whole
            // defect. The delete guard then answers "could not tell" for it and
            // stops, which is the right end for both.
            return Some(path.clone());
        }
        match self.workspace_path(&recorded.owner, &recorded.repo, &recorded.branch) {
            Ok(derived) => Some(derived),
            Err(unsafe_name) => {
                // Named by the triple the derivation refused rather than by the
                // workspace id: that triple *is* what failed, and it is the field
                // a hand-edited metadata.json would have to be fixed in.
                notices.push(CacheNotice::CloneNotNamed {
                    owner: recorded.owner.clone(),
                    repo: recorded.repo.clone(),
                    branch: recorded.branch.clone(),
                    reason: format!(
                        "{:?} is not a safe {:?} name",
                        unsafe_name.name, unsafe_name.part
                    ),
                });
                None
            }
        }
    }

    /// Whether a workspace clone is on disk.
    pub(crate) fn workspace_exists(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<bool, UnsafeName> {
        Ok(clone_is_there(&self.workspace_path(owner, repo, branch)?))
    }

    // --------------------------------------------------------- the cold path

    /// Everything a cold launch needs on the host, under one repo lock.
    ///
    /// The cold path's single entrypoint, and the only place the repo lock is
    /// taken for it. Clone-if-missing, the targeted fetch and branch creation, and
    /// the workspace clone all run inside one scope that provably owns the lock, so
    /// the sequence cannot be interrupted partway through.
    ///
    /// It used to be four separate acquisitions, and the cost was never the flocks
    /// — four uncontended ones are microseconds. It was that between any two of
    /// them another process could act on a repository this launch was halfway
    /// through preparing: `dl --prune` weighing or removing a clone still being
    /// filled, or two launches of different branches of one repository interleaving
    /// their steps. Atomicity and legibility, not speed.
    pub(crate) fn prepare_cold(
        &self,
        storage: &mut MetadataStorage,
        owner: &str,
        repo: &str,
        branch: &str,
        remote_url: &str,
        notices: &mut Vec<CacheNotice>,
    ) -> Result<PreparedWorkspace, PrepareColdError> {
        // Derived here rather than passed in, and derived before the lock: it is
        // the parse boundary for the triple, and an unsafe ref should be refused
        // without anything having been locked or written on its behalf.
        let workspace =
            WorkspaceId::new(owner, repo, branch).map_err(PrepareColdError::UnsafeTriple)?;

        let mut stage = timing::stage(timing::Stage::HostPrep);
        let prepared = self.prepare_cold_under_lock(storage, &workspace, remote_url, notices);
        if prepared.is_err() {
            stage.fail();
        }
        prepared
    }

    /// The locked half of [`WorkspaceCloneManager::prepare_cold`].
    ///
    /// Takes the [`WorkspaceId`] rather than the triple, so the three steps under
    /// the lock cannot be told about a different repository than the one whose
    /// clone directory they are building.
    fn prepare_cold_under_lock(
        &self,
        storage: &mut MetadataStorage,
        workspace: &WorkspaceId,
        remote_url: &str,
        notices: &mut Vec<CacheNotice>,
    ) -> Result<PreparedWorkspace, PrepareColdError> {
        let (owner, repo, branch) = (workspace.owner(), workspace.repo(), workspace.git_ref());
        let lock = self
            .repo_manager
            .hold_repo_lock(owner, repo)
            .map_err(PrepareColdError::Lock)?;

        self.repo_manager
            .clone_if_missing(&lock, storage, owner, repo, remote_url, notices)?;

        let base = self
            .ensure_branch(&lock, storage, owner, repo, branch, notices)
            .map_err(PrepareColdError::Branch)?;
        if let BranchBase::Stale { base: from, reason } = &base {
            // The one consequence-stating notice for the whole degraded family:
            // the fetch notices above say what failed, this says what it means
            // for the tree the agent is about to work on.
            notices.push(CacheNotice::PreparedFromStaleBase {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
                branch: branch.to_owned(),
                base: from.clone(),
                reason: reason.clone(),
            });
        }

        let path = self
            .prepare_workspace(&lock, storage, workspace, remote_url, notices)
            .map_err(PrepareColdError::Workspace)?;
        Ok(PreparedWorkspace { path, base })
    }

    /// Ensure a branch exists in the bare repository, at the remote's current tip.
    ///
    /// Answers which of those two things it actually delivered: [`BranchBase::Fresh`]
    /// when the tip the launch runs on was fetched this call, [`BranchBase::Stale`]
    /// naming the unrefreshed ref and the reason when it was not (devlaunch#245).
    /// The degraded arms all proceed — that is the devlaunch#144 contract — but
    /// they no longer proceed silently.
    ///
    /// This is the whole of the launch path's network use, and the staleness
    /// contract devlaunch#144 settled is stated here because this is the code that
    /// keeps it:
    ///
    /// - **Push upstream, then immediately dl the branch → you get the pushed
    ///   tip.** One targeted fetch of the requested ref, every time, no interval
    ///   gate.
    /// - **A branch that exists nowhere yet** is created from the default branch's
    ///   freshly fetched tip — one more targeted fetch, and no more than one.
    /// - **Offline, or any other fetch failure:** report it and carry on with
    ///   whatever the cache holds. The launch of an already-cached branch still
    ///   works. When there is nothing to launch from at all, the branch creation at
    ///   the end of this method fails — it is the first thing that actually
    ///   consults the cache, so that is where its emptiness is discovered.
    /// - **Every other ref** (other branches, tags, prunes) converges within
    ///   `fetch_interval` via the detached updater sweep, blocking nobody.
    ///
    /// Does not push to the remote.
    ///
    /// Takes a [`RepoLock`] rather than acquiring one: the fetch and the branch
    /// creation both write refs in the shared bare repository, and two processes
    /// doing so at once trip over git's own ref locks.
    pub(crate) fn ensure_branch(
        &self,
        lock: &RepoLock,
        storage: &MetadataStorage,
        owner: &str,
        repo: &str,
        branch: &str,
        notices: &mut Vec<CacheNotice>,
    ) -> Result<BranchBase, EnsureBranchError> {
        lock.require(owner, repo)
            .map_err(EnsureBranchError::WrongRepoLock)?;
        let bare = self.repo_manager.bare_dir(owner, repo);

        // The requested branch reaches here proven by a WorkspaceId on the cold
        // path, so a refusal is only reachable for the recorded default branch
        // below; an unsafe requested ref answers as a failed fetch rather than
        // changing what this method returns.
        let outcome = self
            .repo_manager
            .fetch_ref(owner, repo, branch)
            .unwrap_or_else(|unsafe_name| FetchOutcome::Failed {
                reason: unsafe_reason(&unsafe_name),
            });
        let default = self.resolve_default_branch(storage, owner, repo, notices);

        let base = match outcome {
            FetchOutcome::Updated => BranchBase::Fresh,
            FetchOutcome::RefMissingOnRemote => match &default {
                // The remote answered, so the branch really is new: base it on the
                // default branch, and fetch *that* so it is based on something
                // current. Whatever this second fetch answers, the branch creation
                // below proceeds — there is no third ref to fall back to, and the
                // cache's own default branch is the best remaining start point.
                DefaultBranch::Named(name) => self.fetch_base_branch(owner, repo, name, notices),
                // No default branch could even be named, so nothing was fetched
                // and the creation below starts from the bare cache's own HEAD — a
                // ref of unbounded age, and the report says so.
                DefaultBranch::Unknown { reason } => BranchBase::Stale {
                    base: "HEAD".to_owned(),
                    reason: reason.clone(),
                },
            },
            // Not an error here: a cached branch still launches. Deliberately no
            // default-branch fetch — nothing was learned about the remote, so
            // nothing licenses treating this branch as new.
            FetchOutcome::Failed { reason } => {
                notices.push(CacheNotice::RefNotFetched {
                    owner: owner.to_owned(),
                    repo: repo.to_owned(),
                    branch: branch.to_owned(),
                    reason: reason.clone(),
                });
                BranchBase::Stale {
                    base: branch.to_owned(),
                    reason,
                }
            }
        };

        self.branch_manager
            .ensure_branch_exists(EnsureBranch::in_cache(&bare, branch, default.start_point()))
            .map_err(EnsureBranchError::Branch)?;
        Ok(base)
    }

    /// The repository's default branch, or why it could not be named.
    ///
    /// An empty answer is folded into [`DefaultBranch::Unknown`] here rather than
    /// being left to read as absent downstream: a branch named `""` is not a
    /// thing, so the one place that can tell is the one place that asked.
    fn resolve_default_branch(
        &self,
        storage: &MetadataStorage,
        owner: &str,
        repo: &str,
        notices: &mut Vec<CacheNotice>,
    ) -> DefaultBranch {
        let name = self
            .repo_manager
            .get_default_branch(storage, owner, repo, notices);
        if name.is_empty() {
            // Unreachable while the resolver falls back to `main`, and kept
            // because the resolver's fallback is not this method's to assume: an
            // empty name has to mean "no default branch" here rather than
            // downstream, where it would read as a branch.
            let reason = "no default branch is recorded".to_owned();
            notices.push(CacheNotice::DefaultBranchUnknown {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
                reason: reason.clone(),
            });
            return DefaultBranch::Unknown { reason };
        }
        DefaultBranch::Named(name)
    }

    /// Refresh `default_branch` to cut a brand-new branch from, and report it.
    ///
    /// Split out of [`WorkspaceCloneManager::ensure_branch`] so the "which ref is
    /// the base" question is answered in one place per outcome rather than nested
    /// three deep inside the requested ref's own dispatch.
    fn fetch_base_branch(
        &self,
        owner: &str,
        repo: &str,
        default_branch: &str,
        notices: &mut Vec<CacheNotice>,
    ) -> BranchBase {
        // Unlike the requested branch, this name is read back from metadata.json
        // and carries no proof. Its refusal is reported as a stale base rather
        // than propagated, so a hand-edited record cannot change what this method
        // answers with.
        let outcome = match self.repo_manager.fetch_ref(owner, repo, default_branch) {
            Ok(outcome) => outcome,
            Err(unsafe_name) => {
                let reason = unsafe_reason(&unsafe_name);
                notices.push(CacheNotice::RefNotFetched {
                    owner: owner.to_owned(),
                    repo: repo.to_owned(),
                    branch: default_branch.to_owned(),
                    reason: reason.clone(),
                });
                return BranchBase::Stale {
                    base: default_branch.to_owned(),
                    reason,
                };
            }
        };
        match outcome {
            FetchOutcome::Updated => BranchBase::Fresh,
            // The remote has no default branch under that name either. Nothing
            // more to try — there is no third ref — and the branch creation still
            // has the cache's own default branch to start from. Unverifiable is
            // stale here: nothing fetched backs it.
            FetchOutcome::RefMissingOnRemote => BranchBase::Stale {
                base: default_branch.to_owned(),
                reason: format!("the remote has no branch '{default_branch}' to refresh from"),
            },
            // Noticed here as well as reported, because the reason is carried
            // precisely so it can be printed: the new branch is about to be cut
            // from a possibly stale cache.
            FetchOutcome::Failed { reason } => {
                notices.push(CacheNotice::RefNotFetched {
                    owner: owner.to_owned(),
                    repo: repo.to_owned(),
                    branch: default_branch.to_owned(),
                    reason: reason.clone(),
                });
                BranchBase::Stale {
                    base: default_branch.to_owned(),
                    reason,
                }
            }
        }
    }

    /// Make the workspace clone and record it; the caller holds `lock`.
    ///
    /// 1. Clone from the bare repository to the workspace path (if not already
    ///    there).
    /// 2. Point the remote at the forge, not at the bare repository.
    /// 3. Check the requested branch out.
    /// 4. Materialize git-lfs content.
    /// 5. Record the workspace, so it can be found and deleted later.
    ///
    /// All of it mutates the workspace clone, which is why it needs the lock: fire
    /// the same workspace twice at once and, unserialized, each process saw no
    /// clone, both cloned into the same path, and the loser's cleanup deleted the
    /// winner's.
    ///
    /// The workspace id written to metadata comes from the `workspace` argument
    /// rather than being re-derived here, and that argument is the proof its
    /// triple was validated. It has to equal the clone directory's leaf name for
    /// later lookups to find the clone.
    ///
    /// **Step 1 shares the cache's object files, and that is the design rather
    /// than a happy accident.** `git clone <path> <path>` hardlinks the pack files
    /// instead of copying them, which is the whole reason a workspace per branch is
    /// affordable. Measured on blooop/devlaunch itself (`du -sc` over the cache and
    /// each clone's `.git`, ext4, git 2.55.0): cache plus one workspace is 2400 KB
    /// shared against 4472 KB with `--no-hardlinks`, and each further workspace's
    /// `.git` costs 196 KB instead of 2268 KB — about 91% of it is the cache's
    /// copy, not its own. Nothing in the call *says* so, so an integration test
    /// asserts it (the same `(st_dev, st_ino)` pair, `st_nlink >= 2`) and goes red
    /// on the silent ways to lose it: a `file://` URL, an intermediate copy, an
    /// explicit `--no-hardlinks`.
    ///
    /// No clone flag guards it, deliberately. `--local` is already the default and
    /// does not even error on a `file://` source, so it would pin nothing;
    /// `--shared` and `--reference` were measured to leave an fsck-broken workspace
    /// once the cache's force-refspec fetch and gc have run, for a 2 KB saving, and
    /// are rejected (devlaunch#154).
    ///
    /// Two measured ways the sharing erodes, neither of which this call fights:
    ///
    /// 1. **A repack of the cache drops an existing clone's pack to
    ///    `st_nlink == 1`** — its own private, complete copy. Measured: the clone
    ///    still passes `git fsck` afterwards. That is the safety property that
    ///    makes alternates unnecessary, not a bug. The workspace stops being cheap
    ///    and never stops being valid, which is exactly what an alternates-based
    ///    workspace fails to do here.
    /// 2. **A destination on another filesystem makes git fall back to a full copy,
    ///    silently.** Measured across ext4 and tmpfs: exit 0, no warning, and a
    ///    pack with a different inode on a different device. Devlaunch's own layout
    ///    makes it unreachable — `.bare` and every workspace clone are siblings
    ///    inside one repository directory, so source and destination are on the
    ///    same filesystem by construction.
    ///
    /// **No fetch in the workspace clone.** A new one was just cloned from a bare
    /// cache that `ensure_branch` had already fetched the requested ref into, and
    /// for an existing one the fetch's output was never read: the checkout below is
    /// a plain `git checkout <branch>` against the local branch and consults no
    /// remote-tracking ref. It was a network round trip per launch that bought
    /// nothing (devlaunch#144).
    ///
    /// The bare cache and the clone directory are *derived* from `workspace`
    /// rather than passed in, which Python did because its caller had both to
    /// hand. Deriving them is the stronger statement: the directory this builds
    /// and the id it records cannot name different workspaces.
    pub(crate) fn prepare_workspace(
        &self,
        lock: &RepoLock,
        storage: &mut MetadataStorage,
        workspace: &WorkspaceId,
        remote_url: &str,
        notices: &mut Vec<CacheNotice>,
    ) -> Result<PathBuf, PrepareWorkspaceError> {
        let (owner, repo, branch) = (workspace.owner(), workspace.repo(), workspace.git_ref());
        lock.require(owner, repo)
            .map_err(PrepareWorkspaceError::WrongRepoLock)?;
        let bare = self.repo_manager.bare_dir(owner, repo);
        let bare = bare.as_path();
        let ws_path = self.path_of(workspace);
        let ws_path = ws_path.as_path();

        // Asked of the path this call was given rather than re-derived from the
        // triple: it is the directory every step below acts on, and the answer
        // decides which of the two checkout shapes runs.
        let is_new_workspace = !clone_is_there(ws_path);
        if is_new_workspace {
            if let Some(parent) = ws_path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    PrepareWorkspaceError::ParentNotCreated {
                        path: parent.to_path_buf(),
                        reason: error.to_string(),
                    }
                })?;
            }

            // Step 1: clone from the bare cache. `GIT_LFS_SKIP_SMUDGE` is set by
            // the client, because the bare cache is the repository's LFS store
            // only for refs some earlier launch already materialized: it arrives
            // empty and is filled one ref at a time by `materialize_lfs`, which
            // runs after this clone. So a smudge here has nothing to fetch on a
            // repository's first workspace and no guarantee of this ref's objects
            // on any later one, and fails with "remote missing object" whenever it
            // comes up short. Deferring it keeps the fill and the pull together,
            // where the cache is topped up for this ref and then hardlinked out of.
            let cloned = {
                let _span = timing::span("git clone");
                self.git.clone_from_cache(bare, ws_path)
            };
            if let Some(refused) = cloned.refusal() {
                return Err(PrepareWorkspaceError::CloneRefused {
                    reason: refused.reason().to_owned(),
                    cleanup: Cleanup::of(ws_path),
                });
            }

            // Step 2: point the remote at the forge.
            if let Some(refused) = self
                .git
                .set_remote_url(ws_path, ORIGIN, remote_url)
                .refusal()
            {
                return Err(PrepareWorkspaceError::RemoteNotRepointed {
                    reason: refused.reason().to_owned(),
                });
            }
        }

        // Step 3: check the branch out.
        let checked_out = if is_new_workspace {
            // For a new workspace the branch is reset to the remote ref, so the
            // launch starts from the latest commit rather than from a stale clone.
            // No validation call for the requested branch: `workspace` is the
            // proof.
            if self.remote_ref_exists(ws_path, branch, ORIGIN)? {
                self.git
                    .checkout_reset(ws_path, branch, &format!("{ORIGIN}/{branch}"))
            } else {
                let default_branch = self
                    .repo_manager
                    .get_repo(storage, owner, repo, notices)
                    .map(|recorded| recorded.default_branch)
                    .unwrap_or_else(|| FALLBACK_DEFAULT_BRANCH.to_owned());
                if !self.remote_ref_exists(ws_path, &default_branch, ORIGIN)? {
                    return Err(PrepareWorkspaceError::NoStartPoint {
                        branch: branch.to_owned(),
                        default_branch,
                    });
                }
                self.git
                    .checkout_reset(ws_path, branch, &format!("{ORIGIN}/{default_branch}"))
            }
        } else {
            // An existing workspace: a plain checkout preserves local work.
            self.git.checkout(ws_path, branch)
        };
        if let Some(refused) = checked_out.refusal() {
            return Err(PrepareWorkspaceError::CheckoutRefused {
                branch: branch.to_owned(),
                reason: refused.reason().to_owned(),
            });
        }

        // Step 4: materialize LFS content. Not gated on `is_new_workspace`: a
        // failed pull leaves a workspace that already "exists", and gating here
        // would take the existing-workspace path forever after, silently building
        // against pointers. The bare is passed because it is the repository's LFS
        // store, and the ref because it is what bounds what gets fetched into it.
        self.materialize_lfs(ws_path, bare, branch, notices)?;

        // Step 5: record the workspace. A record that cannot be written is a
        // notice and not a failure: the clone is on disk and usable, and refusing
        // the launch over its bookkeeping would be the worse outcome.
        let mut recorded = WorktreeInfo::new(
            owner,
            repo,
            branch,
            ws_path.to_path_buf(),
            &workspace.value(),
        );
        // The devpod workspace id, written down rather than left to be re-derived
        // later (devlaunch#88). It is the same string as `workspace_id` at the
        // moment it is written — dl hands this clone to `devpod up --id
        // <workspace.value>` immediately after this call returns — and the point of
        // storing it is that the two stop being the same string the day the
        // derivation moves. Every workspace created before #81 changed the scheme
        // had no second copy of its old id anywhere, so the whole population became
        // unaddressable at once. Re-registration writes it too, which is how a
        // record from an older dl acquires one without a migration.
        recorded.devpod_workspace_id = Some(workspace.value());
        match storage.add_worktree(recorded) {
            Ok(store_notices) => {
                notices.extend(store_notices.into_iter().map(CacheNotice::Metadata));
            }
            Err(error) => notices.push(CacheNotice::WorkspaceNotRecorded {
                reason: format!("{error:?}"),
            }),
        }

        Ok(ws_path.to_path_buf())
    }

    /// Whether a remote-tracking ref exists in a workspace.
    ///
    /// Validates the branch with the same predicate the id constructor uses: the
    /// default branch reaches here from stored metadata rather than from a
    /// [`WorkspaceId`], so it is the one ref that still arrives unproven. The
    /// remote is this module's own constant and is checked with it, because the
    /// check costs nothing and the constant is what a future caller would change.
    fn remote_ref_exists(
        &self,
        ws_path: &Path,
        branch: &str,
        remote: &str,
    ) -> Result<bool, PrepareWorkspaceError> {
        validate_ref_name(remote, NamePart::Repo)
            .and_then(|()| validate_ref_name(branch, NamePart::Ref))
            .map_err(PrepareWorkspaceError::UnsafeRefName)?;
        Ok(self
            .git
            .verify_ref(ws_path, &git::refs_remotes(remote, branch))
            .is_said())
    }

    // ---------------------------------------------------------------- LFS

    /// Whether anything git-lfs could name still holds an unmaterialized pointer.
    ///
    /// Checked by content rather than by "did we just clone this", so an
    /// interrupted or failed materialization is retried on the next run instead of
    /// leaving the workspace on pointer files for good.
    ///
    /// The working-tree scan runs first and can only rule the answer *out*, never
    /// in, so the git-lfs probe still decides which pointer-shaped files are really
    /// LFS — the answer is what it always was, minus a fork nobody needed.
    fn has_lfs_pointers(&self, ws_path: &Path, notices: &mut Vec<CacheNotice>) -> bool {
        if let GitLfs::NotInstalled = self.lfs {
            return false;
        }
        if !self.may_hold_lfs_pointers(ws_path, notices) {
            return false;
        }
        self.lfs_tracked_files(ws_path, notices)
            .iter()
            .any(|name| git::is_lfs_pointer(&ws_path.join(name)))
    }

    /// Whether *anything* tracked here is pointer-shaped.
    ///
    /// A necessary condition for [`WorkspaceCloneManager::has_lfs_pointers`],
    /// standing in front of the git-lfs fork. `git lfs ls-files` reports the union
    /// of HEAD's tree and the index, and `--with-tree=HEAD` is what makes
    /// `git ls-files` enumerate that same union — so if none of those paths holds a
    /// pointer the probe would answer false anyway, and forking git-lfs to hear it
    /// is pure cost, which the overwhelmingly common non-LFS repository pays on
    /// every single launch.
    ///
    /// Cheaper than the probe, not free: one fork plus the first few bytes of each
    /// listed path. It is the same O(tracked files) shape as the probe it stands in
    /// front of, at a much smaller constant.
    ///
    /// Deliberately a question about pointer *content*, not about declarations: a
    /// repository can hold committed pointers with no `filter=lfs` attribute of its
    /// own, and can be LFS-tracked through attributes git reads from outside the
    /// clone. Reading either as "no LFS here" would leave such a workspace on stub
    /// files permanently. Content is the thing the caller actually needs to know,
    /// and it is also the thing that stops being true once materialization
    /// succeeds.
    ///
    /// Fails open: paths that cannot be enumerated mean "cannot tell", not "no
    /// LFS", so the probe runs. An unborn HEAD lands there too, and pays one probe
    /// to be told that a repository with no commits holds nothing.
    fn may_hold_lfs_pointers(&self, ws_path: &Path, notices: &mut Vec<CacheNotice>) -> bool {
        let listed = {
            let _span = timing::span("git ls-files");
            self.git.tracked_files(ws_path)
        };
        match listed {
            git::GitAnswer::Refused(refused) => {
                notices.push(CacheNotice::TrackedFilesNotListed {
                    reason: refused.reason().to_owned(),
                });
                true
            }
            git::GitAnswer::Said(tracked) => tracked
                .iter()
                .any(|name| git::is_lfs_pointer(&ws_path.join(name))),
        }
    }

    /// The paths in the tree that git-lfs tracks.
    ///
    /// An empty list for a refusal, *reported*: degrading silently to "no LFS here"
    /// would ship a tree of pointer files as though it were complete.
    fn lfs_tracked_files(&self, ws_path: &Path, notices: &mut Vec<CacheNotice>) -> Vec<String> {
        let listed = {
            let _span = timing::span("git lfs ls-files");
            self.git.lfs_tracked_files(ws_path)
        };
        match listed {
            git::GitAnswer::Said(files) => files,
            git::GitAnswer::Refused(refused) => {
                notices.push(CacheNotice::LfsFilesNotListed {
                    reason: refused.reason().to_owned(),
                });
                Vec::new()
            }
        }
    }

    /// Fetch a ref's git-lfs objects into the bare cache's own store.
    ///
    /// A bare clone arrives with an **empty** `lfs/` directory — git-lfs has no
    /// bare-clone hook — so this call is what actually makes the cache the
    /// repository's store. The client runs it with the bare as cwd, which is the
    /// only thing that decides where the objects land, and bounds it with the two
    /// `fetchrecent` knobs.
    ///
    /// Best-effort, and the caller must treat it as such: offline, or against a
    /// remote that has gone away, this fails and the workspace falls through to the
    /// network phase, which is exactly the behaviour that was there before the
    /// cache existed.
    ///
    /// Writes into the shared bare repository, so it must not run unserialized;
    /// its one caller runs under the repo lock `prepare_workspace` holds.
    fn fill_cache_lfs_store(&self, bare: &Path, reference: &str, notices: &mut Vec<CacheNotice>) {
        let _span = timing::span("git lfs fetch (cache)");
        if let Some(refused) = self.git.lfs_fetch_into_cache(bare, reference).refusal() {
            notices.push(CacheNotice::LfsCacheNotFilled {
                reason: refused.reason().to_owned(),
            });
        }
    }

    /// Materialize the workspace's git-lfs content out of the bare cache.
    ///
    /// `file://<bare>` as the remote, given on the command line; the client carries
    /// why each half of that is load-bearing. Best-effort for the same reason as
    /// the cache fill: an object the cache does not hold makes this fail, and the
    /// caller's next question is whether any pointer survived.
    fn pull_lfs_from_cache(&self, ws_path: &Path, bare: &Path, notices: &mut Vec<CacheNotice>) {
        let _span = timing::span("git lfs pull (cache)");
        if let Some(refused) = self.git.lfs_pull_from_cache(ws_path, bare).refusal() {
            notices.push(CacheNotice::LfsNotPulledFromCache {
                reason: refused.reason().to_owned(),
            });
        }
    }

    /// Replace pointer files with real content: cache first, origin second.
    ///
    /// The workspace is cloned from the local bare cache with
    /// `GIT_LFS_SKIP_SMUDGE=1`, so after the origin URL is fixed to point at the
    /// real remote the pointers must be materialized explicitly — a same-commit
    /// checkout will not rewrite them.
    ///
    /// **Two phases, and the second one is the network.** The cache phase fills
    /// `<bare>/lfs` and hardlinks out of it, which makes the payload a per-*repo*
    /// cost instead of a per-*workspace* one: before it, every workspace of an LFS
    /// repository downloaded the whole payload from the forge and kept a private
    /// copy of it in `.git/lfs/objects` (devlaunch#154). The origin phase is what
    /// ran before this existed, unchanged, and it is entered only if a pointer
    /// survived the cache phase — which is asked by the same content predicate that
    /// opened the method, not by whether the cache commands succeeded. An object the
    /// cache could not supply is the case that matters, and only the pointers say
    /// so.
    ///
    /// Neither cache-phase step can fail the launch. Both degrade to the origin
    /// pull, so the offline-and-uncached path fails exactly where it always did,
    /// and the retry contract is unchanged: the workspace is left holding pointers,
    /// and the next run tries again because the gate is content and not "did we
    /// just clone this".
    ///
    /// Output is not captured: a multi-gigabyte fetch has to be able to show
    /// progress rather than look like a hang.
    fn materialize_lfs(
        &self,
        ws_path: &Path,
        bare: &Path,
        reference: &str,
        notices: &mut Vec<CacheNotice>,
    ) -> Result<(), PrepareWorkspaceError> {
        if !self.has_lfs_pointers(ws_path, notices) {
            return Ok(());
        }
        self.fill_cache_lfs_store(bare, reference, notices);
        self.pull_lfs_from_cache(ws_path, bare, notices);
        if !self.has_lfs_pointers(ws_path, notices) {
            return Ok(());
        }
        let pulled = {
            let _span = timing::span("git lfs pull");
            self.git.lfs_pull_origin(ws_path)
        };
        match pulled.refusal() {
            None => Ok(()),
            Some(refused) => Err(PrepareWorkspaceError::LfsNotMaterialized {
                reason: refused.reason().to_owned(),
            }),
        }
    }

    // ------------------------------------------------------------- removal

    /// Remove a workspace clone, locating it by deriving its path.
    pub(crate) fn remove_workspace(
        &self,
        storage: &mut MetadataStorage,
        owner: &str,
        repo: &str,
        branch: &str,
        notices: &mut Vec<CacheNotice>,
    ) -> Result<Removed, RemoveWorkspaceError> {
        let ws_path = self
            .workspace_path(owner, repo, branch)
            .map_err(RemoveWorkspaceError::UnsafeTriple)?;
        self.remove_clone(storage, &ws_path, owner, repo, branch, notices)
    }

    /// Remove a workspace clone by its workspace id.
    ///
    /// Looks the workspace up in metadata and removes the directory
    /// [`WorkspaceCloneManager::resolve_clone_path`] names — the same one the
    /// `dl <ws> rm` guard inspected before deciding this was safe to call.
    /// Resolving it in one place is what makes those two the same directory
    /// (devlaunch#174).
    ///
    /// Following the record matters because the derivation has changed: every
    /// workspace created before the current id scheme has a bare branch name as
    /// its clone-directory leaf. Re-deriving the leaf here looked for a directory
    /// that never existed, so removal deleted the devpod workspace and then
    /// reported failure — orphaning the clone and its metadata entry, silently,
    /// because the caller only reports success. The stored path makes old and new
    /// workspaces both removable with no migration.
    pub(crate) fn remove_workspace_by_id(
        &self,
        storage: &mut MetadataStorage,
        workspace_id: &str,
        notices: &mut Vec<CacheNotice>,
    ) -> Result<Removed, RemoveWorkspaceError> {
        let Some(recorded) = storage.get_worktree_by_workspace_id(workspace_id).cloned() else {
            return Ok(Removed::Nothing);
        };
        let Some(ws_path) = self.resolve_clone_path(&recorded, notices) else {
            // dl cannot name the directory, so it must not delete one. Same answer
            // as "no record": removes nothing, says so.
            return Ok(Removed::Nothing);
        };
        self.remove_clone(
            storage,
            &ws_path,
            &recorded.owner,
            &recorded.repo,
            &recorded.branch,
            notices,
        )
    }

    /// Delete `ws_path` and its metadata entry.
    fn remove_clone(
        &self,
        storage: &mut MetadataStorage,
        ws_path: &Path,
        owner: &str,
        repo: &str,
        branch: &str,
        notices: &mut Vec<CacheNotice>,
    ) -> Result<Removed, RemoveWorkspaceError> {
        match remove_tree(ws_path) {
            Ok(TreeRemoval::WasNotThere) => return Ok(Removed::Nothing),
            Ok(TreeRemoval::Removed) => {}
            Err(error) => return Err(RemoveWorkspaceError::DirectoryLeft(error)),
        }
        // The clone is gone either way, so a record that cannot be removed is a
        // notice rather than a failure: reporting the removal as failed would send
        // the caller looking for a directory that is not there.
        match storage.remove_worktree(owner, repo, branch) {
            Ok(store_notices) => {
                notices.extend(store_notices.into_iter().map(CacheNotice::Metadata));
            }
            Err(error) => notices.push(CacheNotice::WorkspaceRecordNotRemoved {
                reason: format!("{error:?}"),
            }),
        }
        Ok(Removed::Clone)
    }
}

/// Whether a directory holds a git clone: it is there, and it has a `.git`.
fn clone_is_there(ws_path: &Path) -> bool {
    ws_path.exists() && ws_path.join(".git").exists()
}

/// An unsafe name as a reason a caller can carry.
///
/// The refusal's own data — which part, and what the name was — rendered as one
/// string because it lands in a [`BranchBase::Stale`] beside git's own words,
/// where the *shape* has to be uniform even though the sources are not.
fn unsafe_reason(refused: &UnsafeName) -> String {
    format!("{:?} is not a safe {:?} name", refused.name, refused.part)
}

#[cfg(test)]
mod tests {
    //! `test/test_workspace_clone.py`, `test/test_cold_launch_fetches.py`, the
    //! token half of `test/test_repo_lock_cycles.py`, and the four integration
    //! files about what a clone shares with the cache.
    //!
    //! **The argv seam does more here than Python's mocks did.** Python replaced
    //! `RepositoryManager` and `BranchManager` with `MagicMock`s, so a test of
    //! `prepare_cold` saw four subprocess calls; here those two are real objects
    //! over the same fake runner, so the assertion is the whole cold sequence —
    //! the fetch and the branch step included. That is strictly more contract, and
    //! it is the sequence the shim log would show.
    //!
    //! Real git is used where the claim is about the filesystem: shared pack
    //! inodes, the staleness contract end to end, and what git-lfs really does.
    //! Real git-lfs is used where nothing else can answer, and those tests step
    //! aside when the machine has no git-lfs (see [`lfs_is_usable`]).

    use std::cell::RefCell;
    use std::process::Command;
    use std::rc::Rc;
    use std::time::Duration;

    use super::*;
    use crate::domain::locks;
    use crate::domain::model::Timestamp;
    use crate::flows::repo_manager::{
        Cleanup, RemoveTreeError, bare_dir, clone_dir, repo_dir, repo_lock_path,
        tests::{
            Cache, FakeGit, a_cache, a_fixture_remote, as_strs, commit_on, head_sha, real_git,
            refusing_reads, run_git,
        },
    };
    use crate::runner::{
        CapturedText, DetachOutcome, EnvBase, Exit, Invocation, Outcome, ProcessRunner, Runner,
        SpawnSpec,
    };
    use devlaunch_test_support::Response;

    const REMOTE_URL: &str = "git@github.com:owner/repo.git";

    /// The clone-directory leaf for a branch of `owner/repo`.
    ///
    /// Derived rather than hardcoded: the leaf and the devpod workspace id are the
    /// same string by construction, and what that string *is* for a given triple is
    /// pinned in `domain::workspace_id`. Restating it here would pin the same fact
    /// twice and make these tests fail for the wrong reason.
    fn leaf(branch: &str) -> String {
        WorkspaceId::new("owner", "repo", branch)
            .expect("a safe triple")
            .value()
    }

    fn a_workspace(branch: &str) -> WorkspaceId {
        WorkspaceId::new("owner", "repo", branch).expect("a safe triple")
    }

    fn a_clone_manager<'r>(cache: &Cache, git: Git<'r>, lfs: GitLfs) -> WorkspaceCloneManager<'r> {
        WorkspaceCloneManager::new(&cache.repos_dir, Duration::from_secs(3600), git, lfs)
    }

    /// A `Vec` for notices a test does not read.
    fn ignoring() -> Vec<CacheNotice> {
        Vec::new()
    }

    /// The pointer bytes a clone made with `GIT_LFS_SKIP_SMUDGE=1` leaves behind.
    const POINTER: &[u8] = b"version https://git-lfs.github.com/spec/v1\noid sha256:x\n";

    /// A workspace clone already on disk, holding `files`.
    fn given_clone(cache: &Cache, branch: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let ws = clone_dir(&cache.repos_dir, "owner", "repo", &leaf(branch));
        std::fs::create_dir_all(ws.join(".git")).expect("the clone");
        for (name, content) in files {
            let path = ws.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("the parent");
            }
            std::fs::write(path, content).expect("the file");
        }
        ws
    }

    /// A cache with a bare clone and a record, which is what a warm repository is.
    fn given_cached_repo(cache: &mut Cache) -> PathBuf {
        let bare = cache.given_bare_clone("owner", "repo");
        cache.given_record("owner", "repo");
        bare
    }

    // ============================================================== paths

    #[test]
    fn a_workspace_lives_beside_the_cache_under_its_own_id() {
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let path = manager
            .workspace_path("owner", "repo", "feature/my-branch")
            .expect("a safe triple");

        assert_eq!(
            path,
            repo_dir(&cache.repos_dir, "owner", "repo").join(leaf("feature/my-branch"))
        );
        assert_eq!(
            path.file_name().expect("a leaf").to_string_lossy(),
            leaf("feature/my-branch"),
            "the clone directory and the devpod workspace share one name"
        );
    }

    #[test]
    fn the_same_branch_of_two_repositories_has_two_leaf_names() {
        // A bare branch name is unique only *within* its parent directory, so any
        // consumer keying on a single path component saw every branch of a
        // repository as one workspace (kinisi-robotics/kinisi_ros#9766).
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let one = manager
            .workspace_path("owner", "repo-one", "main")
            .expect("safe");
        let two = manager
            .workspace_path("owner", "repo-two", "main")
            .expect("safe");

        assert_ne!(one.file_name(), two.file_name());
    }

    #[test]
    fn an_unvalidated_ref_cannot_name_a_workspace_by_any_route() {
        // There is no other way to produce the leaf, so no code path can reach a
        // clone directory with an unvalidated ref.
        let mut cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        assert!(manager.workspace_path("owner", "repo", "--evil").is_err());
        assert!(
            manager
                .workspace_exists("owner", "repo", "branch name")
                .is_err()
        );
        assert!(
            manager
                .remove_workspace(
                    &mut cache.storage,
                    "owner",
                    "repo",
                    "--evil",
                    &mut ignoring()
                )
                .is_err()
        );

        let refused = manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "--evil",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect_err("an unsafe ref");
        assert!(
            matches!(refused, PrepareColdError::UnsafeTriple(_)),
            "{refused:?}"
        );
        assert_eq!(
            fake.call_count(),
            0,
            "refused before anything was locked or written on its behalf"
        );
        assert!(
            !repo_lock_path(&cache.repos_dir, "owner", "repo").exists(),
            "and before the lock file was even created"
        );
    }

    #[test]
    fn a_workspace_is_there_when_the_directory_has_a_git() {
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        assert!(
            !manager
                .workspace_exists("owner", "repo", "main")
                .expect("safe")
        );

        let ws = clone_dir(&cache.repos_dir, "owner", "repo", &leaf("main"));
        std::fs::create_dir_all(&ws).expect("the directory");
        assert!(
            !manager
                .workspace_exists("owner", "repo", "main")
                .expect("safe"),
            "a directory with no .git is not a clone"
        );

        std::fs::create_dir_all(ws.join(".git")).expect("a .git");
        assert!(
            manager
                .workspace_exists("owner", "repo", "main")
                .expect("safe")
        );
    }

    // ==================================================== the cold sequence

    #[test]
    fn a_cold_launch_issues_exactly_this_sequence() {
        // The whole host-side cold path, in order, with git-lfs absent. Every line
        // of it is a contract: the bare clone, the branch read off it, the one
        // targeted fetch in the cache, the local-refs branch probe, the workspace
        // clone that hardlinks, the remote repointed at the forge, and the reset
        // checkout.
        let mut cache = a_cache();
        let fake = FakeGit::new().headed_at_main();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);
        let bare = bare_dir(&cache.repos_dir, "owner", "repo");
        let ws = clone_dir(&cache.repos_dir, "owner", "repo", &leaf("nb4"));

        let prepared = manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        assert_eq!(prepared.path, ws);
        assert_eq!(prepared.base, BranchBase::Fresh);
        let argvs = fake.argvs();
        assert_eq!(
            as_strs(&argvs),
            [
                vec![
                    "git",
                    "clone",
                    "--bare",
                    REMOTE_URL,
                    &bare.display().to_string()
                ],
                vec!["git", "symbolic-ref", "HEAD"],
                vec!["git", "fetch", "origin", "+refs/heads/nb4:refs/heads/nb4"],
                vec!["git", "show-ref", "--verify", "refs/heads/nb4"],
                vec![
                    "git",
                    "clone",
                    &bare.display().to_string(),
                    &ws.display().to_string()
                ],
                vec!["git", "remote", "set-url", "origin", REMOTE_URL],
                vec!["git", "show-ref", "--verify", "refs/remotes/origin/nb4"],
                vec!["git", "checkout", "-B", "nb4", "origin/nb4"],
            ]
        );
    }

    #[test]
    fn the_workspace_clone_skips_the_lfs_smudge_and_runs_from_no_directory() {
        // The bare cache is the repository's LFS store only for refs some earlier
        // launch already materialized: it arrives empty and is filled one ref at a
        // time *after* this clone, so a smudge here has nothing to fetch on a
        // repository's first workspace and fails with "remote missing object"
        // whenever it comes up short.
        let mut cache = a_cache();
        let bare = given_cached_repo(&mut cache);
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        let clone = fake
            .calls()
            .into_iter()
            .find(|call| call.args().first().map(String::as_str) == Some("clone"))
            .expect("a workspace clone");
        assert_eq!(
            clone
                .invocation()
                .env
                .entries
                .get("GIT_LFS_SKIP_SMUDGE")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(clone.invocation().env.base, EnvBase::Parent);
        assert_eq!(
            clone.invocation().cwd,
            None,
            "both ends are absolute, and a cwd would be the thing that made the \
             hardlinking depend on where dl was run from"
        );
        assert!(bare.is_dir());
    }

    #[test]
    fn a_workspace_already_on_disk_is_checked_out_and_nothing_else() {
        // Re-registration touches the network not at all. This path used to run an
        // unconditional `git fetch origin` in the clone whose output nothing then
        // read — the checkout below is a plain `git checkout <branch>` against the
        // local branch and never consults a remote-tracking ref (devlaunch#144).
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        given_clone(&cache, "nb4", &[]);
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        let argvs = fake.argvs();
        let issued = as_strs(&argvs);
        assert_eq!(
            issued
                .iter()
                .filter(|argv| argv.contains(&"checkout"))
                .collect::<Vec<_>>(),
            [&vec!["git", "checkout", "nb4"]],
            "a plain checkout preserves local work"
        );
        assert!(
            !issued
                .iter()
                .any(|argv| argv.contains(&"set-url") || argv.contains(&"-B")),
            "nothing is re-cloned or re-pointed: {issued:?}"
        );
    }

    #[test]
    fn no_launch_fetches_inside_the_workspace_clone() {
        let mut cache = a_cache();
        let bare = given_cached_repo(&mut cache);
        let ws = clone_dir(&cache.repos_dir, "owner", "repo", &leaf("nb4"));
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        let fetches: Vec<_> = fake
            .calls()
            .into_iter()
            .filter(|call| call.args().first().map(String::as_str) == Some("fetch"))
            .collect();
        assert_eq!(fetches.len(), 1, "one network call for the whole launch");
        assert_eq!(fetches[0].invocation().cwd.as_deref(), Some(bare.as_path()));
        assert!(
            fetches[0].invocation().cwd.as_deref() != Some(ws.as_path()),
            "fetching into the workspace would leave the cache stale for the next \
             branch and put the round trip after the clone"
        );
    }

    #[test]
    fn a_new_branch_is_cut_from_the_default_branchs_remote_ref() {
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let fake = FakeGit::new()
            // The requested branch is not on the remote…
            .with_script(
                [
                    "git",
                    "show-ref",
                    "--verify",
                    "refs/remotes/origin/new-feature",
                ],
                Response::exited(1),
            )
            // …but the default branch is.
            .with_script(["git", "show-ref"], Response::ok());
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "new-feature",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        let argvs = fake.argvs();
        let issued = as_strs(&argvs);
        assert_eq!(
            *issued.last().expect("a checkout"),
            ["git", "checkout", "-B", "new-feature", "origin/main"]
        );
    }

    #[test]
    fn a_new_branch_with_nothing_to_cut_from_is_refused_rather_than_invented() {
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let fake = FakeGit::new().with_script(["git", "show-ref"], Response::exited(1));
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let failed = manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "new-feature",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect_err("nothing to start from");

        match failed {
            PrepareColdError::Workspace(PrepareWorkspaceError::NoStartPoint {
                branch,
                default_branch,
            }) => {
                assert_eq!(branch, "new-feature");
                assert_eq!(default_branch, "main");
            }
            other => panic!("no start point, got {other:?}"),
        }
    }

    #[test]
    fn every_step_of_the_workspace_reports_what_git_said_or_its_exit_status() {
        // Four failures, each of which reported "…: None" when an uncaptured stderr
        // was quoted raw. The clone and the repoint are the local steps where git is
        // usually silent (a full disk); the checkout is the one git command a warm
        // launch runs.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);

        for (script, expect_silent) in [
            (vec!["git", "clone"], false),
            (vec!["git", "clone"], true),
            (vec!["git", "remote", "set-url"], true),
            (vec!["git", "checkout"], true),
        ] {
            let answer = if expect_silent {
                Response::exited(128)
            } else {
                Response::failed(1, "fatal: error\n")
            };
            let fake = FakeGit::new().with_script(script.clone(), answer);
            let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

            let failed = manager
                .prepare_cold(
                    &mut cache.storage,
                    "owner",
                    "repo",
                    "nb4",
                    REMOTE_URL,
                    &mut ignoring(),
                )
                .expect_err("refused");

            let reason = match &failed {
                PrepareColdError::Workspace(PrepareWorkspaceError::CloneRefused {
                    reason, ..
                })
                | PrepareColdError::Workspace(PrepareWorkspaceError::RemoteNotRepointed {
                    reason,
                })
                | PrepareColdError::Workspace(PrepareWorkspaceError::CheckoutRefused {
                    reason,
                    ..
                }) => reason.clone(),
                other => panic!("a step's own failure for {script:?}, got {other:?}"),
            };
            assert!(!reason.contains("None"), "{script:?}: {reason}");
            if expect_silent {
                assert!(reason.contains("128"), "{script:?}: {reason}");
            } else {
                assert_eq!(reason, "fatal: error");
            }
        }
    }

    #[test]
    fn a_clone_that_failed_takes_its_debris_with_it() {
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let ws = clone_dir(&cache.repos_dir, "owner", "repo", &leaf("nb4"));
        // What a killed `git clone` leaves: a directory with no `.git` in it.
        std::fs::create_dir_all(ws.join("half-written")).expect("the debris");
        let fake = FakeGit::new().with_script(["git", "clone"], Response::exited(128));
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let failed = manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect_err("refused");

        match failed {
            PrepareColdError::Workspace(PrepareWorkspaceError::CloneRefused {
                cleanup, ..
            }) => assert_eq!(cleanup, Cleanup::Cleared),
            other => panic!("a refused clone, got {other:?}"),
        }
        assert!(!ws.exists());
    }

    #[test]
    fn the_record_carries_the_id_dl_hands_devpod_on_both_routes() {
        // devlaunch#88. The field existed on the record since the worktree backend
        // was written and nothing ever assigned it, so when the id derivation moved
        // there was no second copy of the old id anywhere and every workspace
        // created before the change became unaddressable. Re-registration writes it
        // too, which is how a record from an older dl acquires one with no
        // migration.
        for already_there in [false, true] {
            let mut cache = a_cache();
            given_cached_repo(&mut cache);
            if already_there {
                given_clone(&cache, "nb4", &[]);
            }
            let fake = FakeGit::new();
            let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

            manager
                .prepare_cold(
                    &mut cache.storage,
                    "owner",
                    "repo",
                    "nb4",
                    REMOTE_URL,
                    &mut ignoring(),
                )
                .expect("prepared");

            let recorded = cache
                .storage
                .get_worktree("owner", "repo", "nb4")
                .expect("a record");
            assert_eq!(recorded.workspace_id, leaf("nb4"));
            assert_eq!(
                recorded.devpod_workspace_id.as_deref(),
                Some(leaf("nb4").as_str())
            );
            assert_eq!(
                recorded.local_path,
                clone_dir(&cache.repos_dir, "owner", "repo", &leaf("nb4"))
            );
        }
    }

    // ================================================== the branch and base

    /// `ensure_branch` under a lock, with the notices it produced.
    fn ensure_branch_with(
        manager: &WorkspaceCloneManager<'_>,
        cache: &mut Cache,
        branch: &str,
    ) -> (Result<BranchBase, EnsureBranchError>, Vec<CacheNotice>) {
        let lock = manager
            .repo_manager()
            .hold_repo_lock("owner", "repo")
            .expect("the lock");
        let mut notices = ignoring();
        let base =
            manager.ensure_branch(&lock, &cache.storage, "owner", "repo", branch, &mut notices);
        (base, notices)
    }

    #[test]
    fn the_branch_step_fetches_the_requested_ref_and_then_ensures_it() {
        // By name and unconditionally: this one call is what makes "push upstream,
        // immediately dl the branch" land on the pushed tip, and it replaced an
        // interval-gated fetch of every ref in the repository.
        let mut cache = a_cache();
        let bare = given_cached_repo(&mut cache);
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let (base, notices) = ensure_branch_with(&manager, &mut cache, "newbranch");

        assert_eq!(base.expect("ensured"), BranchBase::Fresh);
        assert_eq!(notices, Vec::new());
        let argvs = fake.argvs();
        assert_eq!(
            as_strs(&argvs),
            [
                vec![
                    "git",
                    "fetch",
                    "origin",
                    "+refs/heads/newbranch:refs/heads/newbranch"
                ],
                vec!["git", "show-ref", "--verify", "refs/heads/newbranch"],
            ]
        );
        assert_eq!(
            fake.calls()[0].invocation().cwd.as_deref(),
            Some(bare.as_path())
        );
    }

    #[test]
    fn a_ref_the_remote_has_not_got_costs_exactly_one_extra_fetch() {
        // Without the second targeted fetch the new branch would be cut from
        // whatever the cache happened to hold, so a branch created today could start
        // from last week's main. And no more than one: pinned so a later "just fetch
        // the fallback's fallback" cannot turn one launch into a chain of round
        // trips.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let fake = FakeGit::new().with_script(
            ["git", "fetch"],
            Response::failed(
                128,
                "fatal: couldn't find remote ref refs/heads/newbranch\n",
            ),
        );
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let (base, _) = ensure_branch_with(&manager, &mut cache, "newbranch");

        let argvs = fake.argvs();
        let fetches: Vec<_> = as_strs(&argvs)
            .into_iter()
            .filter(|argv| argv.contains(&"fetch"))
            .collect();
        assert_eq!(
            fetches,
            [
                vec![
                    "git",
                    "fetch",
                    "origin",
                    "+refs/heads/newbranch:refs/heads/newbranch"
                ],
                vec!["git", "fetch", "origin", "+refs/heads/main:refs/heads/main"],
            ]
        );
        // Both fetches answered "no such ref", so nothing this call fetched backs
        // the ref the branch is cut from: unverifiable is stale here.
        assert_eq!(
            base.expect("ensured"),
            BranchBase::Stale {
                base: "main".to_owned(),
                reason: "the remote has no branch 'main' to refresh from".to_owned(),
            }
        );
    }

    #[test]
    fn a_new_branch_cut_from_a_freshly_fetched_default_is_a_fresh_base() {
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let fake = FakeGit::new().with_script(
            [
                "git",
                "fetch",
                "origin",
                "+refs/heads/newbranch:refs/heads/newbranch",
            ],
            Response::failed(
                128,
                "fatal: couldn't find remote ref refs/heads/newbranch\n",
            ),
        );
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let (base, _) = ensure_branch_with(&manager, &mut cache, "newbranch");

        assert_eq!(base.expect("ensured"), BranchBase::Fresh);
    }

    #[test]
    fn a_base_fetch_that_failed_is_reported_and_still_cut_from() {
        // Arm 1 of devlaunch#245: the base-branch fetch fails after the remote called
        // the branch new. The branch is still cut from the cache's default branch —
        // and the answer says which ref that was and why nothing refreshed it,
        // instead of leaving both facts in a warning.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let fake = FakeGit::new()
            .with_script(
                [
                    "git",
                    "fetch",
                    "origin",
                    "+refs/heads/newbranch:refs/heads/newbranch",
                ],
                Response::failed(
                    128,
                    "fatal: couldn't find remote ref refs/heads/newbranch\n",
                ),
            )
            .with_script(
                ["git", "fetch"],
                Response::failed(128, "fatal: no such host\n"),
            );
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let (base, notices) = ensure_branch_with(&manager, &mut cache, "newbranch");

        assert_eq!(
            base.expect("ensured"),
            BranchBase::Stale {
                base: "main".to_owned(),
                reason: "fatal: no such host".to_owned(),
            }
        );
        assert_eq!(
            notices,
            vec![CacheNotice::RefNotFetched {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                branch: "main".to_owned(),
                reason: "fatal: no such host".to_owned(),
            }],
            "the reason is carried precisely so it can be printed"
        );
        let argvs = fake.argvs();
        assert!(
            as_strs(&argvs).contains(&vec!["git", "show-ref", "--verify", "refs/heads/newbranch"]),
            "the branch is still ensured, from the cache's own default branch"
        );
    }

    #[test]
    fn an_unreachable_remote_leaves_the_requested_ref_as_the_stale_base() {
        // Offline still launches, from whatever the cache holds — and deliberately
        // with no default-branch fetch: nothing was learned about the remote, so
        // nothing licenses treating this branch as new.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let fake = FakeGit::new().with_script(
            ["git", "fetch"],
            Response::failed(128, "fatal: Could not read from remote repository\n"),
        );
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let (base, notices) = ensure_branch_with(&manager, &mut cache, "newbranch");

        match base.expect("ensured") {
            BranchBase::Stale { base, reason } => {
                assert_eq!(base, "newbranch");
                assert!(reason.contains("Could not read from remote repository"));
            }
            other => panic!("a stale base, got {other:?}"),
        }
        assert!(matches!(
            notices.as_slice(),
            [CacheNotice::RefNotFetched { .. }]
        ));
        let argvs = fake.argvs();
        assert_eq!(
            as_strs(&argvs)
                .iter()
                .filter(|argv| argv.contains(&"fetch"))
                .count(),
            1
        );
    }

    #[test]
    fn a_recorded_default_branch_the_fetch_refuses_is_reported_and_still_cut_from() {
        // Arm 3 of devlaunch#245, and the shape observed in the wild: a recorded
        // default-branch name the fetch validator refuses means the base is never
        // refreshed — yet git happily resolves that same name as a start point, so
        // the launch succeeds on a base of the cache's age. The name and the
        // rejection are carried, where before both died as a warning.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let mut recorded = cache
            .storage
            .get_repository("owner", "repo")
            .expect("a record")
            .clone();
        recorded.default_branch = "--upload-pack=evil".to_owned();
        cache.storage.add_repository(recorded).expect("recorded");
        let fake = FakeGit::new().with_script(
            ["git", "fetch"],
            Response::failed(
                128,
                "fatal: couldn't find remote ref refs/heads/newbranch\n",
            ),
        );
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let (base, notices) = ensure_branch_with(&manager, &mut cache, "newbranch");

        match base.expect("ensured, not raised") {
            BranchBase::Stale { base, reason } => {
                assert_eq!(base, "--upload-pack=evil");
                assert!(reason.contains("not a safe"), "{reason}");
            }
            other => panic!("a stale base, got {other:?}"),
        }
        assert!(matches!(
            notices.as_slice(),
            [CacheNotice::RefNotFetched { .. }]
        ));
        let argvs = fake.argvs();
        assert_eq!(
            as_strs(&argvs)
                .iter()
                .filter(|argv| argv.contains(&"fetch"))
                .count(),
            1,
            "the unsafe name never reached git as argv"
        );
        assert!(
            as_strs(&argvs).contains(&vec!["git", "branch", "newbranch", "--upload-pack=evil"])
                || as_strs(&argvs).contains(&vec![
                    "git",
                    "show-ref",
                    "--verify",
                    "refs/heads/newbranch"
                ]),
            "and the branch step still ran, leaving the failure to git as before"
        );
    }

    #[test]
    fn the_branch_step_refuses_a_token_for_another_repository() {
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);
        let lock = manager
            .repo_manager()
            .hold_repo_lock("owner", "repo")
            .expect("the lock");

        let refused = manager
            .ensure_branch(
                &lock,
                &cache.storage,
                "owner",
                "other",
                "main",
                &mut ignoring(),
            )
            .expect_err("a token for one repository cannot vouch for another");

        assert!(matches!(refused, EnsureBranchError::WrongRepoLock(_)));
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn the_default_branch_fold_treats_an_empty_name_as_no_name_at_all() {
        // A branch named "" is not a thing, so the one place that can tell is the
        // one place that asked. The arm is defensive — the resolver's last resort is
        // the literal `main`, exactly as Python's was — so it is pinned on the fold
        // itself rather than through a resolver that cannot produce it.
        assert_eq!(
            DefaultBranch::Named("main".to_owned()).start_point(),
            "main"
        );
        assert_eq!(
            DefaultBranch::Unknown {
                reason: "no default branch is recorded".to_owned()
            }
            .start_point(),
            "HEAD",
            "a ref of unbounded age, and the report says so"
        );
    }

    // ================================================ prepare_cold's report

    #[test]
    fn a_launch_from_an_unrefreshed_base_says_what_it_means_for_the_tree() {
        // The report crosses the branch step's boundary as a value; this is where it
        // becomes something the binary can print. `wf` reads that line.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let fake = FakeGit::new()
            .with_script(
                ["git", "fetch", "origin", "+refs/heads/nb4:refs/heads/nb4"],
                Response::failed(128, "fatal: couldn't find remote ref refs/heads/nb4\n"),
            )
            .with_script(
                ["git", "fetch"],
                Response::failed(128, "fatal: no such host\n"),
            );
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);
        let mut notices = ignoring();

        let prepared = manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut notices,
            )
            .expect("prepared");

        assert_eq!(
            prepared.path,
            clone_dir(&cache.repos_dir, "owner", "repo", &leaf("nb4")),
            "the path is unchanged: the workspace is still handed to devpod as before"
        );
        let stale: Vec<&CacheNotice> = notices
            .iter()
            .filter(|notice| matches!(notice, CacheNotice::PreparedFromStaleBase { .. }))
            .collect();
        assert_eq!(
            stale,
            [&CacheNotice::PreparedFromStaleBase {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                branch: "nb4".to_owned(),
                base: "main".to_owned(),
                reason: "fatal: no such host".to_owned(),
            }],
            "one line for the whole degraded family, not one per failed fetch"
        );
    }

    #[test]
    fn a_launch_from_a_fresh_base_claims_nothing_about_staleness() {
        // The notice exists only when it is true, or it trains readers to skip it
        // and wf to distrust it.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);
        let mut notices = ignoring();

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut notices,
            )
            .expect("prepared");

        assert!(
            !notices
                .iter()
                .any(|notice| matches!(notice, CacheNotice::PreparedFromStaleBase { .. })),
            "{notices:?}"
        );
    }

    #[test]
    fn the_cold_entrypoint_holds_the_repo_lock_for_every_step_it_runs() {
        // devlaunch#200. Four separate acquisitions cost nothing in flocks — four
        // uncontended ones are microseconds. What they cost was that between any two
        // of them another process could act on a repository this launch was halfway
        // through preparing. Measured rather than asserted about the source: each git
        // call asks the real lock file whether it is held right now, through a second
        // open file description, which is the truth about the real lock the real code
        // took.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let lock_path = repo_lock_path(&cache.repos_dir, "owner", "repo");
        // Recorded through a shared cell rather than returned, because the hook runs
        // inside the call it is observing.
        /// One git call, and whether the repo lock was held while it ran.
        type Observed = Rc<RefCell<Vec<(Vec<String>, bool)>>>;
        let observed: Observed = Rc::new(RefCell::new(Vec::new()));
        let fake = FakeGit::new().and_then({
            let lock_path = lock_path.clone();
            let observed = Rc::clone(&observed);
            move |argv: &[String]| {
                // A second open file description on the same path: flock is
                // per-open-file-description, so this conflicts with the production
                // code's lock even though both live in one process, and it releases
                // immediately so it can never be the thing a later call sees.
                let free = locks::run_if_lock_free(&lock_path, || ())
                    .expect("no error")
                    .is_some();
                observed.borrow_mut().push((argv.to_vec(), !free));
            }
        });
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "feature/x",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        let observed = observed.borrow();
        assert!(!observed.is_empty(), "no git call was observed at all");
        for (argv, was_held) in observed.iter() {
            assert!(
                was_held,
                "{argv:?} ran outside the lock the entrypoint took"
            );
        }
        assert!(
            locks::run_if_lock_free(&lock_path, || ())
                .expect("no error")
                .is_some(),
            "and the lock is handed back when the entrypoint returns"
        );
    }

    #[test]
    fn nothing_broad_is_ever_fetched_in_the_foreground() {
        // Not one wildcard refspec, however overdue the cache's sweep is: the record
        // here has never been fetched, which is the exact state that used to fetch
        // every head and tag before the launch could proceed.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "feature/x",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        let argvs = fake.argvs();
        let fetches: Vec<Vec<&str>> = as_strs(&argvs)
            .into_iter()
            .filter(|argv| argv.contains(&"fetch"))
            .collect();
        assert_eq!(
            fetches,
            [vec![
                "git",
                "fetch",
                "origin",
                "+refs/heads/feature/x:refs/heads/feature/x"
            ]]
        );
        assert!(
            !fetches
                .iter()
                .any(|argv| argv.iter().any(|arg| arg.contains('*'))
                    || argv.contains(&"--tags")
                    || argv.contains(&"--prune")),
            "{fetches:?}"
        );
    }

    // ================================================================= LFS

    /// A workspace holding one pointer file, and the two listings git would give
    /// for it.
    fn a_workspace_holding_a_pointer(cache: &Cache, name: &str) -> (PathBuf, FakeGit) {
        let ws = given_clone(cache, "nb4", &[(name, POINTER)]);
        let fake = FakeGit::new()
            .with_script(["git", "ls-files"], Response::stdout(format!("{name}\0")))
            .with_script(
                ["git", "lfs", "ls-files"],
                Response::stdout(format!("{name}\n")),
            );
        (ws, fake)
    }

    /// Every argv that named git-lfs, in order.
    fn lfs_calls(fake: &FakeGit) -> Vec<Vec<String>> {
        fake.argvs()
            .into_iter()
            .filter(|argv| argv.iter().any(|arg| arg == "lfs"))
            .collect()
    }

    #[test]
    fn a_workspace_holding_no_pointer_content_never_forks_git_lfs() {
        // The overwhelmingly common repository has no LFS content at all, and probing
        // it costs a fork on every single launch for an answer already sitting in the
        // working tree. The word is looked for anywhere in the argv rather than at a
        // fixed position, because the cache fetch reaches git as `git -c … -c … lfs
        // fetch` and a prefix check would have missed it entirely.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        given_clone(&cache, "nb4", &[("main.py", b"print('hi')\n")]);
        let fake = FakeGit::new().with_script(["git", "ls-files"], Response::stdout("main.py\0"));
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::Installed);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        assert_eq!(lfs_calls(&fake), Vec::<Vec<String>>::new());
        let argvs = fake.argvs();
        assert!(
            as_strs(&argvs).contains(&vec!["git", "ls-files", "-z", "--with-tree=HEAD"]),
            "the cheap check is the union of HEAD and the index: {argvs:?}"
        );
    }

    #[test]
    fn with_git_lfs_absent_not_even_the_cheap_listing_runs() {
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        a_workspace_holding_a_pointer(&cache, "big.bin");
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        let argvs = fake.argvs();
        assert!(
            !as_strs(&argvs)
                .iter()
                .any(|argv| argv.contains(&"ls-files")),
            "nothing git-lfs could name matters when git-lfs is not installed: {argvs:?}"
        );
    }

    #[test]
    fn a_pointer_on_disk_drives_the_cache_phase_and_then_the_network() {
        // Order is the property, not merely presence: a cache pull issued *after* the
        // origin pull would leave every launch paying the download it was added to
        // avoid, and every assertion about disk would still hold.
        let mut cache = a_cache();
        let bare = given_cached_repo(&mut cache);
        let (ws, fake) = a_workspace_holding_a_pointer(&cache, "big.bin");
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::Installed);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        let calls = fake.calls();
        let lfs: Vec<&devlaunch_test_support::Call> = calls
            .iter()
            .filter(|call| call.args().iter().any(|arg| arg == "lfs"))
            .collect();
        assert_eq!(
            lfs.iter().map(|call| call.argv()).collect::<Vec<_>>(),
            [
                vec!["git", "lfs", "ls-files", "--name-only"],
                // cwd is the bare, and that alone is what puts the objects in
                // `<bare>/lfs` and makes the cache the repository's store. The ref is
                // named, or a bare fetch takes the default ref set. Both
                // `fetchrecent` knobs are zero, or git-lfs also walks recent refs and
                // recent commits and launching one branch of a busy repository
                // downloads several branches' payloads.
                vec![
                    "git",
                    "-c",
                    "lfs.fetchrecentrefsdays=0",
                    "-c",
                    "lfs.fetchrecentcommitsdays=0",
                    "lfs",
                    "fetch",
                    "origin",
                    "nb4"
                ],
                vec!["git", "lfs", "pull", &format!("file://{}", bare.display())],
                vec!["git", "lfs", "ls-files", "--name-only"],
                vec!["git", "lfs", "pull", "origin"],
            ]
            .iter()
            .map(|argv| argv
                .iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
        );
        assert_eq!(lfs[1].invocation().cwd.as_deref(), Some(bare.as_path()));
        assert_eq!(
            lfs[2].invocation().cwd.as_deref(),
            Some(ws.as_path()),
            "the pull runs in the workspace, out of the cache"
        );
        assert_eq!(lfs[4].invocation().cwd.as_deref(), Some(ws.as_path()));
    }

    #[test]
    fn origin_is_not_pulled_when_the_cache_materialized_everything() {
        // The saving, stated as a command that does not run. Deciding it from the
        // cache pull's exit status instead would be wrong in both directions —
        // git-lfs exits zero having fetched only some objects, and a partial failure
        // must still fall through — so the same content predicate that opened
        // materialization is what closes it.
        let mut cache = a_cache();
        let bare = given_cached_repo(&mut cache);
        let (ws, fake) = a_workspace_holding_a_pointer(&cache, "big.bin");
        let pointer = ws.join("big.bin");
        let cache_url = format!("file://{}", bare.display());
        let fake = fake.and_then(move |argv| {
            // What the real command does when the cache holds the object.
            if argv.len() >= 4 && argv[1] == "lfs" && argv[2] == "pull" && argv[3] == cache_url {
                std::fs::write(&pointer, b"real content, no longer a pointer").expect("written");
            }
        });
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::Installed);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        let issued = lfs_calls(&fake);
        assert!(
            issued
                .iter()
                .any(|argv| argv.last().is_some_and(|last| last.starts_with("file://"))),
            "{issued:?}"
        );
        assert!(
            !issued
                .iter()
                .any(|argv| argv.last().is_some_and(|last| last == "origin")),
            "the forge is never contacted: {issued:?}"
        );
    }

    #[test]
    fn a_cache_phase_that_failed_degrades_to_the_network_rather_than_the_launch() {
        // Both cache steps exit non-zero in ordinary conditions — offline, or an
        // object this repository's cache has never held — and both are speculative:
        // the network pull behind them is the thing that was always there. Letting
        // either failure out would turn a working offline-ish launch of an LFS
        // repository into a crash, which is *worse* than the state before the cache
        // existed.
        let mut cache = a_cache();
        let bare = given_cached_repo(&mut cache);
        let (_, fake) = a_workspace_holding_a_pointer(&cache, "big.bin");
        let fake = fake
            .with_script(
                ["git", "-c", "lfs.fetchrecentrefsdays=0"],
                Response::exited(2),
            )
            .with_script(
                ["git", "lfs", "pull", &format!("file://{}", bare.display())],
                Response::exited(2),
            );
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::Installed);
        let mut notices = ignoring();

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut notices,
            )
            .expect("prepared anyway");

        assert!(
            lfs_calls(&fake)
                .iter()
                .any(|argv| argv.last().is_some_and(|last| last == "origin")),
            "the network phase is what both cache steps degrade to"
        );
        assert!(
            notices
                .iter()
                .any(|notice| matches!(notice, CacheNotice::LfsCacheNotFilled { .. })),
            "and each failure is reported rather than swallowed: {notices:?}"
        );
        assert!(
            notices
                .iter()
                .any(|notice| matches!(notice, CacheNotice::LfsNotPulledFromCache { .. })),
            "{notices:?}"
        );
    }

    #[test]
    fn a_workspace_nothing_could_materialize_is_refused_and_retried_next_launch() {
        // The contract the cache phase was not allowed to weaken: a workspace whose
        // pointers nothing could resolve must not be handed over as though it were
        // complete, because a build against stub files fails much further from the
        // cause. The retry is real — the gate is pointer content — so the next launch
        // tries again.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        let (_, fake) = a_workspace_holding_a_pointer(&cache, "big.bin");
        let fake = fake
            .with_script(
                ["git", "-c", "lfs.fetchrecentrefsdays=0"],
                Response::exited(2),
            )
            .with_script(["git", "lfs", "pull"], Response::exited(2));
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::Installed);

        let failed = manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect_err("nothing could supply the objects");

        match failed {
            PrepareColdError::Workspace(PrepareWorkspaceError::LfsNotMaterialized { reason }) => {
                assert!(
                    reason.contains('2'),
                    "the exit status is what is left to say: {reason}"
                );
            }
            other => panic!("an unmaterialized workspace, got {other:?}"),
        }
    }

    #[test]
    fn an_lfs_path_that_is_not_on_disk_is_not_an_unmaterialized_pointer() {
        // A sparse checkout leaves LFS-tracked paths out of the working tree
        // altogether, so opening them fails. Reading that as "still a pointer" would
        // run an unbounded, uncaptured `git lfs pull origin` on every launch of such
        // a workspace, forever, because the pull does not put the excluded path on
        // disk and so never changes the answer.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        given_clone(&cache, "nb4", &[]);
        let fake = FakeGit::new()
            .with_script(["git", "ls-files"], Response::stdout("big.bin\0"))
            .with_script(["git", "lfs", "ls-files"], Response::stdout("big.bin\n"));
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::Installed);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        assert_eq!(
            lfs_calls(&fake),
            Vec::<Vec<String>>::new(),
            "the cheap check ruled it out, so not even the probe was paid"
        );
    }

    #[test]
    fn a_tracked_set_that_cannot_be_listed_fails_open_to_the_probe() {
        // The cheap check exists to save a fork, not to decide LFS is absent. Reading
        // "cannot tell" as "no LFS here" would silently strand a workspace on pointer
        // files — the same degradation the probe itself refuses. An unborn HEAD lands
        // here too, and pays one probe to be told a repository with no commits holds
        // nothing.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        given_clone(&cache, "nb4", &[("big.bin", POINTER)]);
        // Scripted before anything else, because the fake answers with the first
        // entry that matches.
        let fake = FakeGit::new()
            .with_script(
                ["git", "ls-files"],
                Response::failed(128, "fatal: broken index\n"),
            )
            .with_script(["git", "lfs", "ls-files"], Response::stdout("big.bin\n"));
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::Installed);
        let mut notices = ignoring();

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut notices,
            )
            .expect("prepared");

        assert!(
            lfs_calls(&fake)
                .iter()
                .any(|argv| argv.last().is_some_and(|last| last == "origin")),
            "the probe ran and found the pointer"
        );
        assert!(
            notices
                .iter()
                .any(|notice| matches!(notice, CacheNotice::TrackedFilesNotListed { .. })),
            "{notices:?}"
        );
    }

    #[test]
    fn a_probe_that_refused_names_no_pointer_and_says_why() {
        // Degrading silently to "no LFS here" would ship a tree of pointer files as
        // though it were complete.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        given_clone(&cache, "nb4", &[("big.bin", POINTER)]);
        let fake = FakeGit::new()
            .with_script(
                ["git", "lfs", "ls-files"],
                Response::failed(1, "fatal: not a git-lfs repository\n"),
            )
            .with_script(["git", "ls-files"], Response::stdout("big.bin\0"));
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::Installed);
        let mut notices = ignoring();

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut notices,
            )
            .expect("prepared");

        assert!(
            !lfs_calls(&fake)
                .iter()
                .any(|argv| argv.contains(&"pull".to_owned())),
            "no path was named, so nothing was pulled"
        );
        assert!(
            notices
                .iter()
                .any(|notice| matches!(notice, CacheNotice::LfsFilesNotListed { .. })),
            "{notices:?}"
        );
    }

    #[test]
    fn a_workspace_whose_content_is_already_real_needs_no_pull() {
        // Retrying must stop when it has worked, or every launch would re-pull.
        let mut cache = a_cache();
        given_cached_repo(&mut cache);
        given_clone(
            &cache,
            "nb4",
            &[("big.bin", b"\x00\x01real binary content")],
        );
        let fake = FakeGit::new()
            .with_script(["git", "ls-files"], Response::stdout("big.bin\0"))
            .with_script(["git", "lfs", "ls-files"], Response::stdout("big.bin\n"));
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::Installed);

        manager
            .prepare_cold(
                &mut cache.storage,
                "owner",
                "repo",
                "nb4",
                REMOTE_URL,
                &mut ignoring(),
            )
            .expect("prepared");

        assert_eq!(lfs_calls(&fake), Vec::<Vec<String>>::new());
    }

    // ======================================= naming a record's clone directory

    /// A record pointing at `local_path`, as `metadata.json` holds one.
    fn a_record(branch: &str, local_path: PathBuf) -> WorktreeInfo {
        let mut recorded = WorktreeInfo::new("owner", "repo", branch, local_path, &leaf(branch));
        recorded.created_at = Timestamp::from_civil(jiff::civil::datetime(2024, 1, 1, 10, 0, 0, 0));
        recorded.last_used = recorded.created_at.clone();
        recorded
    }

    #[test]
    fn a_record_pointing_somewhere_stale_resolves_to_the_directory_that_holds_the_work() {
        // Reproduced before the fix: the guard answered "nothing to lose" about the
        // absent recorded directory — correctly, nothing absent holds anything — and
        // the delete then removed the derived one, which held an uncommitted file.
        // Exit 0 and no `--force`.
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);
        let derived = given_clone(&cache, "nb4", &[]);
        let stale = repo_dir(&cache.repos_dir, "owner", "repo").join("moved-away");
        assert!(!stale.exists(), "the fixture's point is that this is gone");

        let named = manager.resolve_clone_path(&a_record("nb4", stale), &mut ignoring());

        assert_eq!(named, Some(derived));
    }

    #[test]
    fn a_recorded_path_that_is_not_absolute_is_a_record_dl_cannot_honour() {
        // An empty recorded path is the working directory, which exists — so it
        // passed both of the old tests and sent the recursive delete at dl's own
        // working directory, emptying it, `.git` included. Absolute is the property
        // that rules it out; being non-empty does not.
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);
        let derived = clone_dir(&cache.repos_dir, "owner", "repo", &leaf("nb4"));

        for recorded in [PathBuf::from(""), PathBuf::from("relative/clone")] {
            let named = manager
                .resolve_clone_path(&a_record("nb4", recorded.clone()), &mut ignoring())
                .expect("a directory is still named");

            assert_eq!(named, derived, "{recorded:?}");
            assert!(named.is_absolute());
        }
    }

    #[test]
    fn a_recorded_path_dl_was_not_allowed_to_look_at_is_kept_not_derived_away() {
        // "Could not look" is not "not there", and only one of them may derive: a
        // path dl cannot stat is not a path it has established the absence of, and
        // deriving a *different* directory off the back of that would put the guard
        // and the delete back on two answers — which is the whole defect. The delete
        // guard then answers "could not tell" for it and stops, which is the right
        // end for both.
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);
        let shut = repo_dir(&cache.repos_dir, "owner", "repo").join("behind-a-closed-door");
        let recorded = shut.join("clone");
        std::fs::create_dir_all(&recorded).expect("the fixture");

        let Some(_closed) = refusing_reads(&shut) else {
            return; // root, or a filesystem that does not enforce directory modes
        };
        let named = manager.resolve_clone_path(&a_record("nb4", recorded.clone()), &mut ignoring());

        assert_eq!(named, Some(recorded));
    }

    #[test]
    fn a_usable_recorded_path_wins_however_it_is_spelled() {
        // The pre-#64 clones this fallback exists for must stay removable, which is
        // what makes old and new workspaces both deletable with no migration.
        let cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);
        let old_scheme = repo_dir(&cache.repos_dir, "owner", "repo").join("nb4");
        std::fs::create_dir_all(&old_scheme).expect("the old-scheme clone");
        assert_ne!(
            old_scheme.file_name().expect("a leaf"),
            leaf("nb4").as_str()
        );

        assert_eq!(
            manager.resolve_clone_path(&a_record("nb4", old_scheme.clone()), &mut ignoring()),
            Some(old_scheme)
        );
    }

    #[test]
    fn a_record_neither_route_can_name_is_a_refusal_rather_than_an_empty_answer() {
        // `metadata.json` can hold an unsafe ref if somebody hand-edited or truncated
        // it, and letting the derivation's refusal propagate would take down the whole
        // of `dl --ls --json` for one bad record.
        let mut cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);
        let mut recorded = a_record(
            "nb4",
            repo_dir(&cache.repos_dir, "owner", "repo").join("gone"),
        );
        recorded.branch = "--evil".to_owned();
        recorded.workspace_id = "repo-evil".to_owned();
        cache
            .storage
            .add_worktree(recorded.clone())
            .expect("recorded");
        let mut notices = ignoring();

        assert_eq!(manager.resolve_clone_path(&recorded, &mut notices), None);
        assert_eq!(
            notices,
            vec![CacheNotice::CloneNotNamed {
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                branch: "--evil".to_owned(),
                reason: "\"--evil\" is not a safe Ref name".to_owned(),
            }],
            "named by the triple the derivation refused, which is the field to fix"
        );
        assert_eq!(
            manager
                .remove_workspace_by_id(&mut cache.storage, "repo-evil", &mut ignoring())
                .expect("no error"),
            Removed::Nothing,
            "dl cannot name the directory, so it must not delete one"
        );
    }

    // ============================================================== removal

    #[test]
    fn removing_a_workspace_takes_the_directory_and_its_record() {
        let mut cache = a_cache();
        let ws = given_clone(&cache, "nb4", &[("file.txt", b"content")]);
        cache
            .storage
            .add_worktree(a_record("nb4", ws.clone()))
            .expect("recorded");
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let removed = manager
            .remove_workspace(&mut cache.storage, "owner", "repo", "nb4", &mut ignoring())
            .expect("removed");

        assert_eq!(removed, Removed::Clone);
        assert!(!ws.exists());
        assert_eq!(cache.storage.get_worktree("owner", "repo", "nb4"), None);
    }

    #[test]
    fn removing_a_workspace_that_is_not_there_removes_nothing_and_says_so() {
        let mut cache = a_cache();
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        assert_eq!(
            manager
                .remove_workspace(
                    &mut cache.storage,
                    "owner",
                    "repo",
                    "nonexistent",
                    &mut ignoring()
                )
                .expect("no error"),
            Removed::Nothing
        );
        assert_eq!(
            manager
                .remove_workspace_by_id(&mut cache.storage, "nonexistent", &mut ignoring())
                .expect("no error"),
            Removed::Nothing,
            "a workspace id nothing recorded answers the same way"
        );
    }

    #[test]
    fn removing_by_id_follows_the_record_rather_than_re_deriving_the_leaf() {
        // Every workspace created before the current id scheme has a bare branch name
        // as its clone-directory leaf. Re-deriving the leaf here looked for a
        // directory that never existed, so removal deleted the devpod workspace and
        // then reported failure — orphaning the clone and its record silently, since
        // the caller only reports success.
        let mut cache = a_cache();
        let old_scheme = repo_dir(&cache.repos_dir, "owner", "repo").join("nb4");
        std::fs::create_dir_all(old_scheme.join(".git")).expect("the old-scheme clone");
        let mut recorded = a_record("nb4", old_scheme.clone());
        recorded.workspace_id = "repo-nb4".to_owned();
        cache.storage.add_worktree(recorded).expect("recorded");
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let removed = manager
            .remove_workspace_by_id(&mut cache.storage, "repo-nb4", &mut ignoring())
            .expect("removed");

        assert_eq!(removed, Removed::Clone);
        assert!(!old_scheme.exists());
        assert_eq!(cache.storage.get_worktree("owner", "repo", "nb4"), None);
    }

    #[test]
    fn the_delete_removes_exactly_what_the_resolution_named() {
        // The binding itself: one resolution, and the delete uses that one. Without
        // it, the two could be made to disagree again by editing the delete alone and
        // every test above would stay green.
        let mut cache = a_cache();
        let derived = given_clone(&cache, "nb4", &[]);
        let mut recorded = a_record(
            "nb4",
            repo_dir(&cache.repos_dir, "owner", "repo").join("moved-away"),
        );
        recorded.workspace_id = "repo-nb4".to_owned();
        cache
            .storage
            .add_worktree(recorded.clone())
            .expect("recorded");
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let named = manager
            .resolve_clone_path(&recorded, &mut ignoring())
            .expect("a directory");
        assert_eq!(
            manager
                .remove_workspace_by_id(&mut cache.storage, "repo-nb4", &mut ignoring())
                .expect("removed"),
            Removed::Clone
        );

        assert_eq!(named, derived);
        assert!(!named.exists(), "the delete removed something else");
    }

    #[test]
    fn a_symlinked_clone_directory_is_refused_rather_than_followed() {
        let cache = a_cache();
        let elsewhere = cache.dir.path().join("real-clone");
        std::fs::create_dir_all(elsewhere.join("work")).expect("the real clone");
        let link = repo_dir(&cache.repos_dir, "owner", "repo").join(leaf("nb4"));
        std::fs::create_dir_all(link.parent().expect("a parent")).expect("the repo directory");
        std::os::unix::fs::symlink(&elsewhere, &link).expect("a symlink");
        let mut cache = cache;
        let fake = FakeGit::new();
        let manager = a_clone_manager(&cache, Git::new(&fake), GitLfs::NotInstalled);

        let refused = manager
            .remove_workspace(&mut cache.storage, "owner", "repo", "nb4", &mut ignoring())
            .expect_err("a symlinked clone is refused");

        assert!(matches!(
            refused,
            RemoveWorkspaceError::DirectoryLeft(RemoveTreeError::RootIsSymlink { .. })
        ));
        assert!(elsewhere.join("work").is_dir());
    }

    // ============================================================= real git

    /// The pack files under an objects directory, keyed by name.
    fn packs(objects: &Path) -> Vec<(String, std::fs::Metadata)> {
        let pack_dir = objects.join("pack");
        let mut found: Vec<(String, std::fs::Metadata)> = std::fs::read_dir(&pack_dir)
            .map(|listing| {
                listing
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .path()
                            .extension()
                            .is_some_and(|extension| extension == "pack")
                    })
                    .map(|entry| {
                        (
                            entry.file_name().to_string_lossy().into_owned(),
                            entry.metadata().expect("a stat"),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        found.sort_by(|left, right| left.0.cmp(&right.0));
        found
    }

    /// The pair that says two directory entries are one file.
    ///
    /// An inode number is unique only within its filesystem, so on its own it would
    /// be satisfied by a copy that landed on another device and reused the number.
    fn identity(metadata: &std::fs::Metadata) -> (u64, u64) {
        use std::os::unix::fs::MetadataExt as _;
        (metadata.dev(), metadata.ino())
    }

    /// A cache whose bare clone's objects are in a pack file.
    ///
    /// The repack is not decoration, it is what makes the assertion capable of
    /// failing: the fixture repository is three objects, far under git's
    /// `transfer.unpackLimit`, so a push explodes into loose objects and the bare
    /// cache ends up with no `objects/pack` directory at all — against which "every
    /// pack file is shared" is a statement about the empty set. A cache cloned from a
    /// real forge arrives packed, so packing it here is also the honest starting
    /// state.
    fn a_packed_cache(
        manager: &WorkspaceCloneManager<'_>,
        storage: &mut MetadataStorage,
        url: &str,
    ) -> PathBuf {
        manager
            .repo_manager()
            .ensure_repo(storage, "test", "repo", url, &mut ignoring())
            .expect("cloned");
        let bare = manager.repo_manager().bare_dir("test", "repo");
        run_git(&bare, &["repack", "-a", "-d"]);
        bare
    }

    #[test]
    fn real_git_a_workspace_shares_the_caches_pack_files_rather_than_copying_them() {
        // Every workspace is a full clone of the bare cache next to it, and the
        // reason that is affordable is that git's default local transport
        // *hardlinks* the pack files: the second workspace of a repository costs its
        // working tree and its refs, not another copy of the history. Nothing in the
        // code says so — the sharing is what `git clone <path> <path>` does when
        // nobody asks it for anything else — so this is where it is said, and where a
        // change that forfeits it goes red: a `file://` URL, an intermediate copy, an
        // explicit `--no-hardlinks`.
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_clone_manager(&cache, Git::new(&runner), GitLfs::NotInstalled);
        let mut storage = cache.storage;
        let bare = a_packed_cache(&manager, &mut storage, &remote.url);

        let prepared = manager
            .prepare_cold(
                &mut storage,
                "test",
                "repo",
                "main",
                &remote.url,
                &mut ignoring(),
            )
            .expect("prepared");

        let cache_packs = packs(&bare.join("objects"));
        assert!(
            !cache_packs.is_empty(),
            "the cache holds no pack file, so nothing below is being checked"
        );
        let workspace_packs = packs(&prepared.path.join(".git").join("objects"));
        assert_eq!(
            workspace_packs
                .iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            cache_packs.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        for ((name, cached), (_, in_workspace)) in cache_packs.iter().zip(&workspace_packs) {
            assert_eq!(
                identity(in_workspace),
                identity(cached),
                "{name} is a copy, not a shared object file"
            );
            use std::os::unix::fs::MetadataExt as _;
            assert!(
                in_workspace.nlink() >= 2,
                "{name} has {} link(s); a shared pack has at least 2",
                in_workspace.nlink()
            );
        }
    }

    #[test]
    fn real_git_a_repack_of_the_cache_leaves_a_live_workspace_its_own_copy() {
        // The safety property that makes `--shared`/`--reference` unnecessary rather
        // than merely more fragile. Sharing is a hardlink and not a pointer, so a
        // repack of the cache — which unlinks the old pack and writes a new one —
        // drops the workspace's pack to a link count of one and leaves it a private,
        // complete copy. The workspace stops being cheap and never stops being valid,
        // which is the opposite of what an alternates-based workspace does here.
        use std::os::unix::fs::MetadataExt as _;
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_clone_manager(&cache, Git::new(&runner), GitLfs::NotInstalled);
        let mut storage = cache.storage;
        let bare = a_packed_cache(&manager, &mut storage, &remote.url);
        let prepared = manager
            .prepare_cold(
                &mut storage,
                "test",
                "repo",
                "main",
                &remote.url,
                &mut ignoring(),
            )
            .expect("prepared");
        let objects = prepared.path.join(".git").join("objects");
        let shared_before: Vec<u64> = packs(&objects)
            .iter()
            .map(|(_, metadata)| metadata.nlink())
            .collect();
        assert!(!shared_before.is_empty() && shared_before.iter().all(|links| *links >= 2));

        run_git(&bare, &["repack", "-a", "-d"]);

        assert_eq!(
            packs(&objects)
                .iter()
                .map(|(_, metadata)| metadata.nlink())
                .collect::<Vec<_>>(),
            vec![1; shared_before.len()]
        );
        run_git(&prepared.path, &["fsck"]);
    }

    #[test]
    fn real_git_a_commit_pushed_after_the_cache_was_built_is_what_you_launch() {
        // The headline promise of devlaunch#144. The cache is deliberately built
        // *first* and its interval left unelapsed, which is exactly the state in
        // which the old lazy-fetch path would have skipped the network and handed
        // back a workspace one commit behind the remote.
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_clone_manager(&cache, Git::new(&runner), GitLfs::NotInstalled);
        let mut storage = cache.storage;
        manager
            .repo_manager()
            .ensure_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("the cache");

        let pushed = commit_on(
            &remote.work,
            "main",
            "after_cache.txt",
            "Pushed after the cache",
        );

        let prepared = manager
            .prepare_cold(
                &mut storage,
                "test",
                "repo",
                "main",
                &remote.url,
                &mut ignoring(),
            )
            .expect("prepared");

        assert_eq!(head_sha(&prepared.path), pushed);
        assert_eq!(prepared.base, BranchBase::Fresh);
    }

    #[test]
    fn real_git_a_branch_that_reached_the_remote_after_the_cache_still_launches() {
        // Distinct from the case above: there the ref existed in the cache and moved,
        // here it is absent from the cache entirely, which is the path that used to
        // depend on the broad sweep having happened to run.
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_clone_manager(&cache, Git::new(&runner), GitLfs::NotInstalled);
        let mut storage = cache.storage;
        manager
            .repo_manager()
            .ensure_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("the cache");

        let pushed = commit_on(&remote.work, "feature/late", "late.txt", "Pushed later");

        let prepared = manager
            .prepare_cold(
                &mut storage,
                "test",
                "repo",
                "feature/late",
                &remote.url,
                &mut ignoring(),
            )
            .expect("prepared");

        assert_eq!(head_sha(&prepared.path), pushed);
    }

    #[test]
    fn real_git_a_brand_new_branch_starts_from_the_current_default_branch() {
        // The remote-has-not-got-it arm end to end. `main` moves after the cache is
        // built, and the new branch must still start from the moved tip — otherwise
        // every branch created on a cold cache silently starts from history.
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_clone_manager(&cache, Git::new(&runner), GitLfs::NotInstalled);
        let mut storage = cache.storage;
        manager
            .repo_manager()
            .ensure_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("the cache");

        let main_tip = commit_on(&remote.work, "main", "moved.txt", "main moved on");

        let prepared = manager
            .prepare_cold(
                &mut storage,
                "test",
                "repo",
                "brand/new",
                &remote.url,
                &mut ignoring(),
            )
            .expect("prepared");

        assert_eq!(head_sha(&prepared.path), main_tip);
        assert_eq!(
            run_git(&prepared.path, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
            "brand/new"
        );
        assert_eq!(
            prepared.base,
            BranchBase::Fresh,
            "its base was fetched this call"
        );
    }

    #[test]
    fn real_git_a_ref_that_exists_nowhere_at_all_still_fails() {
        // With nothing to launch from, the launch fails rather than inventing one.
        // The three-way outcome deliberately keeps "the remote says no" separate from
        // "the remote did not answer", and neither of them is licence to hand back a
        // workspace built on nothing.
        let cache = a_cache();
        let empty = cache.dir.path().join("empty_remote.git");
        run_git(
            cache.dir.path(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                &empty.display().to_string(),
            ],
        );
        let url = empty.display().to_string();
        let runner = real_git();
        let manager = a_clone_manager(&cache, Git::new(&runner), GitLfs::NotInstalled);
        let mut storage = cache.storage;
        manager
            .repo_manager()
            .ensure_repo(&mut storage, "test", "empty", &url, &mut ignoring())
            .expect("an empty cache is still a cache");

        let failed = manager
            .prepare_cold(
                &mut storage,
                "test",
                "empty",
                "nosuch",
                &url,
                &mut ignoring(),
            )
            .expect_err("nothing to launch from");

        // The branch step runs before the workspace step inside the one locked
        // scope, so its branch creation is where the empty cache is discovered — and
        // the whole preparation fails there rather than handing back a workspace
        // built on nothing.
        assert!(matches!(failed, PrepareColdError::Branch(_)), "{failed:?}");
    }

    #[test]
    fn real_git_an_unreachable_remote_launches_from_the_cache_and_says_it_is_stale() {
        // The offline arm's whole point: losing the network costs you freshness and
        // not the workspace. The remote is made unreachable by moving it, which is
        // indistinguishable to git from a host that is not answering.
        let cache = a_cache();
        let remote = a_fixture_remote(cache.dir.path());
        let cached_tip = head_sha(&remote.work);
        let runner = real_git();
        let manager = a_clone_manager(&cache, Git::new(&runner), GitLfs::NotInstalled);
        let mut storage = cache.storage;
        manager
            .repo_manager()
            .ensure_repo(&mut storage, "test", "repo", &remote.url, &mut ignoring())
            .expect("the cache");
        std::fs::rename(&remote.path, remote.path.with_file_name("moved_away.git"))
            .expect("the remote goes away");
        let mut notices = ignoring();

        let prepared = manager
            .prepare_cold(
                &mut storage,
                "test",
                "repo",
                "main",
                &remote.url,
                &mut notices,
            )
            .expect("prepared from the cache");

        assert_eq!(head_sha(&prepared.path), cached_tip);
        match &prepared.base {
            BranchBase::Stale { base, reason } => {
                assert_eq!(base, "main");
                assert!(!reason.is_empty(), "git said something about why");
            }
            other => panic!("a stale base, got {other:?}"),
        }
        assert!(
            notices
                .iter()
                .any(|notice| matches!(notice, CacheNotice::PreparedFromStaleBase { .. })),
            "{notices:?}"
        );
    }

    // ======================================================== real git-lfs

    /// Whether this machine can answer an LFS question at all.
    ///
    /// Two conditions, and the second is not pedantry: `git lfs pull` fetches the
    /// objects but silently skips the *checkout* unless the `filter.lfs`
    /// clean/smudge configuration is installed, so on a machine where nobody has run
    /// `git lfs install` the materialization assertions below would fail for a reason
    /// that has nothing to do with the cache. Python scoped a `GIT_CONFIG_GLOBAL` of
    /// its own to arrange it; a Rust test binary runs its tests in threads of one
    /// process, where mutating the environment races every other test that spawns
    /// git, so this reads the ambient configuration and steps aside instead.
    fn lfs_is_usable() -> bool {
        let installed = Command::new("git")
            .args(["lfs", "version"])
            .output()
            .is_ok_and(|output| output.status.success());
        if !installed {
            eprintln!("skipped: this machine has no git-lfs");
            return false;
        }
        let filters = Command::new("git")
            .args(["config", "--get", "filter.lfs.smudge"])
            .output()
            .is_ok_and(|output| output.status.success() && !output.stdout.is_empty());
        if !filters {
            eprintln!("skipped: git-lfs is installed but `git lfs install` has not been run");
        }
        filters
    }

    /// Big enough that a copy and a hardlink are visibly different things, small
    /// enough that the suite does not notice. Deterministic, so a half-materialized
    /// worktree cannot pass by accident.
    fn payload() -> Vec<u8> {
        (0..=255u8).cycle().take(131_072).collect()
    }

    /// A local "remote" whose payload really lives in git-lfs, on two branches.
    ///
    /// Two branches both naming the same object: `main`, and a `feature/lfs` that
    /// adds one ordinary file. The second workspace is launched from that branch — a
    /// workspace is per branch, so two of them cannot share one — and the extra
    /// commit deliberately leaves `big.bin` untouched, so the checkout that switches
    /// branches has no reason to smudge and the only thing that can put real bytes on
    /// disk is the materialization under test.
    fn an_lfs_remote(dir: &Path) -> (String, PathBuf) {
        let remote = dir.join("lfs_remote.git");
        run_git(
            dir,
            &[
                "init",
                "--bare",
                "-q",
                "--initial-branch=main",
                &remote.display().to_string(),
            ],
        );
        let work = dir.join("lfs_work");
        std::fs::create_dir_all(&work).expect("the working copy");
        run_git(&work, &["init", "-q", "--initial-branch=main"]);
        run_git(&work, &["lfs", "install", "--local"]);
        run_git(&work, &["lfs", "track", "*.bin"]);
        std::fs::write(work.join("big.bin"), payload()).expect("the payload");
        std::fs::write(work.join("README.md"), "# lfs fixture\n").expect("a README");
        run_git(&work, &["add", "-A"]);
        run_git(&work, &["commit", "-qm", "add an lfs payload"]);
        run_git(
            &work,
            &["remote", "add", "origin", &remote.display().to_string()],
        );
        run_git(&work, &["push", "-q", "-u", "origin", "main"]);
        run_git(&work, &["checkout", "-q", "-b", "feature/lfs"]);
        std::fs::write(work.join("notes.txt"), "an ordinary file\n").expect("a note");
        run_git(&work, &["add", "-A"]);
        run_git(&work, &["commit", "-qm", "add a plain file"]);
        run_git(&work, &["push", "-q", "-u", "origin", "feature/lfs"]);
        (remote.display().to_string(), remote)
    }

    /// The git-lfs object files under a store, keyed by oid.
    fn lfs_objects(store: &Path) -> Vec<(String, std::fs::Metadata)> {
        fn walk(directory: &Path, found: &mut Vec<(String, std::fs::Metadata)>) {
            let Ok(listing) = std::fs::read_dir(directory) else {
                return;
            };
            for entry in listing.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if let Ok(metadata) = entry.metadata() {
                    found.push((entry.file_name().to_string_lossy().into_owned(), metadata));
                }
            }
        }
        let mut found = Vec::new();
        walk(&store.join("objects"), &mut found);
        found.sort_by(|left, right| left.0.cmp(&right.0));
        found
    }

    #[test]
    fn real_lfs_the_bare_cache_is_the_store_and_workspaces_hardlink_out_of_it() {
        // LFS objects are not git objects and the clone does not carry them at all,
        // so without this every workspace of an LFS repository paid a **full download
        // from the forge** and kept a **private copy** of the payload in
        // `.git/lfs/objects` (devlaunch#154). The remote is **removed from disk**
        // between the two launches, which is harder than an unreachable URL: nothing
        // can be silently re-fetched, so anything the second workspace holds provably
        // came out of the cache.
        if !lfs_is_usable() {
            return;
        }
        let cache = a_cache();
        let (url, remote_path) = an_lfs_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_clone_manager(&cache, Git::new(&runner), GitLfs::detected());
        let mut storage = cache.storage;

        let first = manager
            .prepare_cold(&mut storage, "test", "repo", "main", &url, &mut ignoring())
            .expect("the first workspace");

        assert_eq!(
            std::fs::read(first.path.join("big.bin")).expect("the payload"),
            payload()
        );
        let bare = manager.repo_manager().bare_dir("test", "repo");
        let cached = lfs_objects(&bare.join("lfs"));
        assert!(
            !cached.is_empty(),
            "the bare cache holds no LFS object, so it is not the store"
        );
        // The direction matters: materializing the first workspace from origin and
        // letting the cache stay empty would still give that workspace real content,
        // and would leave every later one to download the payload again.
        let in_first = lfs_objects(&first.path.join(".git").join("lfs"));
        assert_eq!(
            in_first.iter().map(|(oid, _)| oid).collect::<Vec<_>>(),
            cached.iter().map(|(oid, _)| oid).collect::<Vec<_>>()
        );
        for ((_, cached), (_, mine)) in cached.iter().zip(&in_first) {
            assert_eq!(identity(mine), identity(cached));
        }

        std::fs::remove_dir_all(&remote_path).expect("the remote goes away");

        let second = manager
            .prepare_cold(
                &mut storage,
                "test",
                "repo",
                "feature/lfs",
                &url,
                &mut ignoring(),
            )
            .expect("the second workspace pays no network");

        assert_eq!(
            std::fs::read(second.path.join("big.bin")).expect("the payload"),
            payload()
        );
        let in_second = lfs_objects(&second.path.join(".git").join("lfs"));
        assert_eq!(
            in_second.iter().map(|(oid, _)| oid).collect::<Vec<_>>(),
            cached.iter().map(|(oid, _)| oid).collect::<Vec<_>>()
        );
        use std::os::unix::fs::MetadataExt as _;
        for ((oid, cached), (_, mine)) in cached.iter().zip(&in_second) {
            assert_eq!(
                identity(mine),
                identity(cached),
                "{oid} is a copy in the workspace, not the cache's own file"
            );
            assert!(mine.nlink() >= 2);
        }
    }

    #[test]
    fn real_lfs_nothing_host_specific_is_persisted_into_the_clone() {
        // `dl` hands the clone directory to `devpod up`, which bind-mounts *it* into
        // the container. `.bare` is a sibling and is not mounted, so a host path
        // written into the clone's config names a directory that does not exist
        // inside — and git-lfs consults it on every checkout. The failure would not
        // show up on the host at all, only in the container, on the repositories this
        // feature exists to make cheap.
        if !lfs_is_usable() {
            return;
        }
        let cache = a_cache();
        let (url, _) = an_lfs_remote(cache.dir.path());
        let runner = real_git();
        let manager = a_clone_manager(&cache, Git::new(&runner), GitLfs::detected());
        let mut storage = cache.storage;

        let prepared = manager
            .prepare_cold(&mut storage, "test", "repo", "main", &url, &mut ignoring())
            .expect("prepared");

        // `lfs.storage` stays unset, however the objects got there: pointing it at
        // the bare would share the objects with no copying at all, and it was
        // rejected twice over — persisted it breaks the container, and passed as `-c`
        // it was measured to break against local-path remotes outright, because
        // `GIT_CONFIG_PARAMETERS` is inherited by the remote-side git-lfs child.
        let storage_override = Command::new("git")
            .args(["config", "--local", "--get", "lfs.storage"])
            .current_dir(&prepared.path)
            .output()
            .expect("git runs");
        assert_eq!(String::from_utf8_lossy(&storage_override.stdout).trim(), "");
        // And no `file://` remote is left behind pointing at the bare. The URL is
        // checked as well as the name: an `origin` repointed at the bare would satisfy
        // a count and would still leave the container talking to a directory it
        // cannot see — and would break `git push` on the host besides.
        assert_eq!(
            run_git(&prepared.path, &["remote"])
                .split_whitespace()
                .collect::<Vec<_>>(),
            ["origin"]
        );
        assert_eq!(
            run_git(&prepared.path, &["remote", "get-url", "origin"]).trim(),
            url
        );
    }

    // ============================ the pointer probe, against real repositories

    /// A runner that is real git for everything except the git-lfs fork.
    ///
    /// What `git lfs ls-files` reports for each repository below was verified against
    /// git-lfs 3.7.1 and is recorded here, so these tests need no git-lfs installed.
    /// Notably it reports a committed pointer file even when the repository declares
    /// no `filter=lfs` attribute anywhere — `git check-attr` says `filter:
    /// unspecified` for that same file — so "this repository declares LFS" is not a
    /// usable stand-in for "this repository holds pointers".
    struct StubbedLfs {
        real: ProcessRunner,
        reports: Vec<String>,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl StubbedLfs {
        fn reporting(names: &[&str]) -> Self {
            Self {
                real: ProcessRunner::new(),
                reports: names.iter().map(|name| (*name).to_string()).collect(),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn forked_git_lfs(&self) -> bool {
            self.calls
                .borrow()
                .iter()
                .any(|argv| argv.get(1).is_some_and(|arg| arg == "lfs"))
        }
    }

    impl Runner for StubbedLfs {
        fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
            let argv = spec.invocation.argv();
            self.calls.borrow_mut().push(argv.clone());
            if argv.get(1).is_some_and(|arg| arg == "lfs") {
                return Outcome::Ran {
                    exit: Exit::Code(0),
                    io: CapturedText {
                        stdout: self
                            .reports
                            .iter()
                            .map(|name| format!("{name}\n"))
                            .collect(),
                        stderr: String::new(),
                    },
                };
            }
            self.real.capture(spec)
        }

        fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
            self.calls.borrow_mut().push(spec.invocation.argv());
            Outcome::Ran {
                exit: Exit::Code(0),
                io: (),
            }
        }

        fn session(&self, spec: &SpawnSpec, _on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
            self.passthrough(spec)
        }

        fn detach(&self, what: &Invocation) -> DetachOutcome {
            self.calls.borrow_mut().push(what.argv());
            DetachOutcome::Started { pid: 900_001 }
        }
    }

    /// A real repository these tests can commit to.
    fn a_real_repo(path: &Path) -> PathBuf {
        std::fs::create_dir_all(path).expect("the directory");
        run_git(path, &["init", "-q", "--initial-branch=main"]);
        path.to_path_buf()
    }

    /// A syntactically real pointer file, as `git lfs track` plus a commit would
    /// leave it in a clone made with `GIT_LFS_SKIP_SMUDGE=1`.
    fn real_pointer() -> Vec<u8> {
        let mut pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:".to_vec();
        pointer.extend(std::iter::repeat_n(b'0', 64));
        pointer.extend_from_slice(b"\nsize 12\n");
        pointer
    }

    const LFS_ATTRIBUTE: &str = "*.bin filter=lfs diff=lfs merge=lfs -text\n";

    /// Commit `name` as an unmaterialized pointer.
    fn commit_pointer(repo: &Path, name: &str) {
        let path = repo.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the parent");
        }
        std::fs::write(path, real_pointer()).expect("the pointer");
        run_git(repo, &["add", "-A"]);
        run_git(repo, &["commit", "-qm", "add pointer"]);
    }

    /// Answer the probe for a real repository, stubbing only the git-lfs fork.
    fn probe(ws: &Path, reports: &[&str]) -> (bool, bool) {
        let cache = a_cache();
        let stubbed = StubbedLfs::reporting(reports);
        let manager = a_clone_manager(&cache, Git::new(&stubbed), GitLfs::Installed);
        let answer = manager.has_lfs_pointers(ws, &mut ignoring());
        (answer, stubbed.forked_git_lfs())
    }

    #[test]
    fn real_git_a_committed_pointer_is_found_however_it_was_declared() {
        // Nothing stops a pointer file being committed into a repository that
        // declares no LFS filter — a deleted `.gitattributes`, or a file added by a
        // tool — and git honours attributes from outside the working tree besides. A
        // workspace holding one must still be materialized, or it is shipped to the
        // user as a stub.
        let dir = tempfile::tempdir().expect("a temp dir");

        // No gitattributes anywhere.
        let bare_declaration = a_real_repo(&dir.path().join("undeclared"));
        commit_pointer(&bare_declaration, "big.bin");
        assert_eq!(probe(&bare_declaration, &["big.bin"]), (true, true));

        // Declared only in `.git/info/attributes`, which is local and untracked, so
        // it never appears in the index or the working tree.
        let local = a_real_repo(&dir.path().join("local-info"));
        commit_pointer(&local, "big.bin");
        let info = local.join(".git").join("info");
        std::fs::create_dir_all(&info).expect("the info directory");
        std::fs::write(info.join("attributes"), LFS_ATTRIBUTE).expect("the declaration");
        assert_eq!(probe(&local, &["big.bin"]), (true, true));

        // At any depth, not only at the top level.
        let nested = a_real_repo(&dir.path().join("nested"));
        std::fs::write(nested.join(".gitattributes"), LFS_ATTRIBUTE).expect("the declaration");
        commit_pointer(&nested, "assets/big.bin");
        assert_eq!(probe(&nested, &["assets/big.bin"]), (true, true));
    }

    #[test]
    fn real_git_a_pointer_survives_a_clone_with_no_index_and_an_unstaged_path() {
        // An interrupted clone or checkout can leave the index missing entirely, and
        // git answers `ls-files` for such a clone with *success and no output* — not
        // with an error. A gate that asked only the index would read that as "nothing
        // is tracked, so nothing can be a pointer" and skip, while git-lfs, which
        // reads HEAD as well, still names the pointer. Nothing about that heals on
        // its own.
        let dir = tempfile::tempdir().expect("a temp dir");

        let no_index = a_real_repo(&dir.path().join("no-index"));
        std::fs::write(no_index.join(".gitattributes"), LFS_ATTRIBUTE).expect("the declaration");
        commit_pointer(&no_index, "big.bin");
        std::fs::remove_file(no_index.join(".git").join("index")).expect("the index goes");
        assert_eq!(probe(&no_index, &["big.bin"]), (true, true));

        // `git lfs ls-files` reports the union of HEAD's tree and the index, so
        // un-staging a tracked path leaves it named by git-lfs and absent from the
        // index. The gate has to ask the same union.
        let head_only = a_real_repo(&dir.path().join("head-only"));
        std::fs::write(head_only.join(".gitattributes"), LFS_ATTRIBUTE).expect("the declaration");
        commit_pointer(&head_only, "big.bin");
        run_git(&head_only, &["rm", "-q", "--cached", "big.bin"]);
        assert_eq!(probe(&head_only, &["big.bin"]), (true, true));
    }

    #[test]
    fn real_git_tracked_paths_that_will_not_open_neither_stop_the_scan_nor_count() {
        // Every ordinary workspace has tracked paths that will not open — a file the
        // user deleted, a dangling symlink, a submodule's directory — and none of
        // them says anything about LFS. Treating any as an error would break the
        // launch of a perfectly normal workspace; giving up at the first would strand
        // a real pointer sitting behind it (so the pointer here is named to sort
        // last); and reading "cannot open it" as "assume pointer" would reinstate the
        // fork on every launch of every such workspace, with the answer unchanged.
        let dir = tempfile::tempdir().expect("a temp dir");
        let ws = a_real_repo(&dir.path().join("ws"));
        std::fs::write(ws.join("gone.txt"), "deleted later\n").expect("a file");
        std::os::unix::fs::symlink("no-such-target", ws.join("dangling")).expect("a symlink");
        let submodule = a_real_repo(&ws.join("nested"));
        std::fs::write(submodule.join("f"), "x\n").expect("a file");
        run_git(&submodule, &["add", "-A"]);
        run_git(&submodule, &["commit", "-qm", "nested"]);
        commit_pointer(&ws, "zz_big.bin");
        std::fs::remove_file(ws.join("gone.txt")).expect("the file goes");

        assert_eq!(
            probe(&ws, &["zz_big.bin"]),
            (true, true),
            "the scan carried on past all three and found the pointer behind them"
        );

        let ordinary = a_real_repo(&dir.path().join("ordinary"));
        std::fs::write(ordinary.join("gone.txt"), "deleted later\n").expect("a file");
        std::os::unix::fs::symlink("no-such-target", ordinary.join("dangling")).expect("a symlink");
        let nested = a_real_repo(&ordinary.join("nested"));
        std::fs::write(nested.join("f"), "x\n").expect("a file");
        run_git(&nested, &["add", "-A"]);
        run_git(&nested, &["commit", "-qm", "nested"]);
        run_git(&ordinary, &["add", "-A"]);
        run_git(&ordinary, &["commit", "-qm", "init"]);
        std::fs::remove_file(ordinary.join("gone.txt")).expect("the file goes");

        assert_eq!(
            probe(&ordinary, &[]),
            (false, false),
            "and none of them was read as a pointer, so no fork was paid"
        );
    }

    #[test]
    fn real_git_a_materialized_or_ordinary_repository_never_forks_git_lfs() {
        // Declaring LFS is not the question; holding an unmaterialized pointer is. A
        // warm workspace whose LFS content is already on disk gets the same free
        // answer as a repository that never used LFS.
        let dir = tempfile::tempdir().expect("a temp dir");

        let ordinary = a_real_repo(&dir.path().join("ordinary"));
        std::fs::write(ordinary.join("main.py"), "print('hi')\n").expect("a file");
        run_git(&ordinary, &["add", "-A"]);
        run_git(&ordinary, &["commit", "-qm", "init"]);
        assert_eq!(probe(&ordinary, &[]), (false, false));

        let materialized = a_real_repo(&dir.path().join("materialized"));
        std::fs::write(materialized.join(".gitattributes"), LFS_ATTRIBUTE).expect("declared");
        commit_pointer(&materialized, "big.bin");
        std::fs::write(materialized.join("big.bin"), b"real content").expect("the real content");
        assert_eq!(probe(&materialized, &["big.bin"]), (false, false));
    }

    #[test]
    fn real_git_a_directory_that_is_not_a_repository_still_pays_the_probe() {
        // When the tracked files cannot be listed the probe runs anyway: the cheap
        // check exists to save a fork, not to decide LFS is absent.
        let dir = tempfile::tempdir().expect("a temp dir");
        let not_a_repo = dir.path().join("not-a-repo");
        std::fs::create_dir_all(&not_a_repo).expect("the directory");
        std::fs::write(not_a_repo.join("big.bin"), real_pointer()).expect("the pointer");

        assert_eq!(probe(&not_a_repo, &["big.bin"]), (true, true));
    }
}
