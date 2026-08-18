//! `metadata.json`: the record of what is in the cache, and how it is written.
//!
//! Ported from `devlaunch/worktree/storage.py`. The file is the one piece of
//! state the Python and Rust builds share while the port runs
//! (docs/rust-rewrite-plan.md, cutover check 3), so the shape, the version
//! header, the two-space indent and the escaping are all pinned against bytes
//! the real Python wrote.
//!
//! Four things this does that a plain "read JSON, write JSON" would not:
//!
//! - **Damaged input never raises.** A file that cannot be read at all is moved
//!   aside and the run continues with empty metadata; a single entry that cannot
//!   be rebuilt is skipped while its neighbours load. Losing the workspace list
//!   is bad, crashing on every invocation is worse.
//! - **Anything lossy is preserved first.** The next mutation rewrites the file
//!   from what was loaded, so whatever this build could not round-trip — a
//!   skipped entry, a field or a top-level key only a newer build knows, a
//!   version header it could not read — is gone by then. The copy is taken at
//!   load time, while the original bytes are still on disk.
//! - **Every write is atomic and every mutation re-reads first.** A save writes
//!   a fresh temp file in the same directory, fsyncs it and renames it over the
//!   target; a mutation takes the lock and reloads before touching anything,
//!   because the in-memory copy is as old as this process and other dl runs have
//!   been writing.
//! - **Nothing is printed.** Python warned on stderr from inside the loader;
//!   core renders no English (#251), so every one of those warnings comes back
//!   as a [`Notice`] carrying the path, the section, the key and the reason as
//!   data. The `dl` binary decides the words.
//!
//! The lock is a sidecar, `<name>.lock`, and not the file itself: a save
//! replaces `metadata.json` by rename, and a lock taken on a replaced inode
//! guards nothing. Lock ordering against the per-repo lock is documented in
//! [`super::locks`] — the metadata lock may be taken while a repo lock is held,
//! never the reverse.

// The flows that read and write metadata land in M4–M6; until then the entries,
// the notices and their typed reasons have no reader outside the tests.
#![allow(dead_code)]

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

use super::locks::{LockError, WaitStarted, hold_lock_watching};
use super::model::{BaseRepository, NotRebuilt, Rebuilt, WorktreeInfo};
use super::xdg::{self, NoHomeDirectory};

/// Version of the on-disk `metadata.json` format.
///
/// 1: the original shape. Clone-directory leaves are flattened branch names and
///    workspace ids are derived separately from them.
/// 2: leaves and workspace ids are both the workspace id (#64). Reached from 1
///    by the migration, which renames the directories on disk and then writes
///    the new paths and this header in one atomic save.
pub(crate) const SCHEMA_VERSION: i64 = 2;

/// What a file whose header cannot be read is assumed to be.
///
/// A file without a `version` key predates versioning, so it is the original
/// shape, not the current one — reading it as current would skip the migration
/// it needs. The same applies to a header that is present but nonsense: the
/// conservative reading is the oldest shape, because a migration that runs
/// against an already-migrated cache is a no-op while one that never runs leaves
/// directories nothing looks for.
pub(crate) const LEGACY_SCHEMA_VERSION: i64 = 1;

/// Top-level keys this build writes, and therefore the only ones a rewrite keeps.
const KNOWN_SECTIONS: [&str; 3] = ["version", "repositories", "worktrees"];

/// How far a chain of symlinks is followed before it is treated as the answer.
/// Linux gives up at 40; a bound is what keeps this total.
const MAX_LINK_DEPTH: usize = 40;

/// Which stored section something was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Repositories,
    Worktrees,
}

impl Section {
    /// The key this section is stored under.
    pub fn key(self) -> &'static str {
        match self {
            Section::Repositories => "repositories",
            Section::Worktrees => "worktrees",
        }
    }
}

/// The JSON type something turned out to be, where a report names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonKind {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

impl JsonKind {
    fn of(value: &Value) -> Self {
        match value {
            Value::Null => JsonKind::Null,
            Value::Bool(_) => JsonKind::Bool,
            Value::Number(_) => JsonKind::Number,
            Value::String(_) => JsonKind::String,
            Value::Array(_) => JsonKind::Array,
            Value::Object(_) => JsonKind::Object,
        }
    }
}

/// What the OS said when a step failed, as data.
///
/// The kind is what code branches on; the message is the OS's own words, quoted
/// the way Python's warnings quote the `OSError` they caught.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsFailure {
    pub kind: io::ErrorKind,
    pub message: String,
}

impl From<io::Error> for OsFailure {
    fn from(error: io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

/// Why a metadata file could not be used at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileProblem {
    /// The bytes could not be read.
    Unreadable(OsFailure),
    /// The bytes are not JSON — or not UTF-8, which arrives here for the same
    /// reason it does in Python: both are "the decoder refused".
    NotJson { reason: String },
    /// Valid JSON, but not an object, so there are no sections in it.
    NotAnObject { found: JsonKind },
}

/// Why one stored entry could not be rebuilt, and is therefore skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryProblem {
    NotAnObject { found: JsonKind },
    NotRebuilt { reason: String },
}

/// What became of a file that could not be used.
///
/// A single quarantine slot, overwritten on repeat corruption, kept apart from
/// the backup slot so the two recovery cases cannot clobber each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quarantine {
    MovedAside { path: PathBuf },
    CouldNotMove { path: PathBuf, failure: OsFailure },
}

/// What became of the original bytes before a lossy rewrite could overwrite them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backup {
    Copied { path: PathBuf },
    CouldNotCopy { path: PathBuf, failure: OsFailure },
}

/// Something a load found that the caller should be told about.
///
/// One arm per warning `storage.py` printed, carrying what that warning
/// interpolated. Collected in the order they were found and returned; nothing
/// here is a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The whole file was unusable, so it was moved aside and the run started
    /// with empty metadata.
    FileUnusable {
        path: PathBuf,
        problem: FileProblem,
        quarantine: Quarantine,
    },
    /// The `version` header could not be read, so the file was read as
    /// [`LEGACY_SCHEMA_VERSION`]. The entries never depend on the header, so
    /// they are kept.
    VersionHeaderUnusable { path: PathBuf, found: Value },
    /// The file was written by a newer build. Its entries are loaded as-is, and
    /// the next change rewrites the whole file as [`SCHEMA_VERSION`].
    VersionFromNewerBuild {
        path: PathBuf,
        found: i64,
        understood: i64,
    },
    /// A section was present but not an object, so all of it was ignored.
    SectionUnusable {
        path: PathBuf,
        section: Section,
        found: JsonKind,
    },
    /// One entry could not be rebuilt and was skipped; its neighbours loaded.
    EntryUnusable {
        path: PathBuf,
        section: Section,
        key: String,
        problem: EntryProblem,
    },
    /// An entry loaded, but carries field(s) this build has no place for; they
    /// are dropped when it is rewritten.
    EntryHasUnknownFields {
        path: PathBuf,
        section: Section,
        key: String,
        /// Sorted.
        fields: Vec<String>,
    },
    /// Top-level key(s) this build does not write, and so would drop.
    UnknownTopLevelKeys {
        path: PathBuf,
        /// Sorted.
        keys: Vec<String>,
    },
    /// Something above was lossy, so the original was copied aside first.
    OriginalPreserved { path: PathBuf, backup: Backup },
}

impl Notice {
    /// Whether this notice means a rewrite in this build's format would lose
    /// information the file currently holds — which is what triggers the backup.
    ///
    /// A quarantined file is not in this set: it was moved aside intact, so
    /// there is nothing left for a rewrite to lose. Neither is the backup notice
    /// itself.
    pub(crate) fn implies_lossy_rewrite(&self) -> bool {
        match self {
            Notice::VersionHeaderUnusable { .. }
            | Notice::VersionFromNewerBuild { .. }
            | Notice::SectionUnusable { .. }
            | Notice::EntryUnusable { .. }
            | Notice::EntryHasUnknownFields { .. }
            | Notice::UnknownTopLevelKeys { .. } => true,
            Notice::FileUnusable { .. } | Notice::OriginalPreserved { .. } => false,
        }
    }
}

/// A step of writing metadata that failed.
///
/// Write failures are deliberately not swallowed: silently losing the workspace
/// list is worse than an error.
#[derive(Debug)]
pub enum MetadataError {
    /// The directory the file lives in could not be created.
    CreateDir { path: PathBuf, failure: OsFailure },
    /// The metadata lock could not be taken.
    Lock(LockError),
    /// No temp file could be created next to the target.
    CreateTemp {
        directory: PathBuf,
        failure: OsFailure,
    },
    /// The document could not be turned into bytes.
    Encode { reason: String },
    /// The bytes could not be written, flushed or fsynced.
    Write { path: PathBuf, failure: OsFailure },
    /// The target's permissions could not be copied onto the temp file.
    SetMode {
        path: PathBuf,
        mode: u32,
        failure: OsFailure,
    },
    /// The finished temp file could not be renamed over the target.
    Replace {
        from: PathBuf,
        to: PathBuf,
        failure: OsFailure,
    },
}

/// Whether a migration finished, and therefore whether the header may move.
///
/// A sum rather than a bool because the two answers are not "yes/no" about one
/// fact: [`SchemaHeader::LeaveBehind`] is the deliberate outcome of a refusal —
/// the header stays where it was so the next run migrates the same directories
/// again (#180) — and a caller reading a bare `false` as "nothing to do" would
/// promote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaHeader {
    /// Every rename the migration could ever perform is done, so the header may
    /// claim the new shape.
    Promote,
    /// Something was refused and left behind, so the header stays where it is.
    LeaveBehind,
}

/// Which worktrees a listing asks for.
///
/// A sum type rather than Python's two optional arguments, where "a repo with no
/// owner" is representable and silently means "everything".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreeFilter<'a> {
    All,
    Owner(&'a str),
    OwnerAndRepo { owner: &'a str, repo: &'a str },
}

/// The persistent record of what the cache holds.
pub struct MetadataStorage {
    /// The path as the caller gave it, which may be a symlink.
    metadata_path: PathBuf,
    /// The real file every operation targets.
    file_path: PathBuf,
    /// The sidecar lock, never the file itself.
    lock_path: PathBuf,
    schema_version: i64,
    repositories: IndexMap<String, BaseRepository>,
    worktrees: IndexMap<String, WorktreeInfo>,
    wait_watcher: Option<Box<dyn Fn(WaitStarted)>>,
}

impl std::fmt::Debug for MetadataStorage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetadataStorage")
            .field("metadata_path", &self.metadata_path)
            .field("file_path", &self.file_path)
            .field("schema_version", &self.schema_version)
            .field("repositories", &self.repositories.len())
            .field("worktrees", &self.worktrees.len())
            .finish_non_exhaustive()
    }
}

impl MetadataStorage {
    /// Where the file lives when nothing says otherwise.
    pub fn default_path() -> Result<PathBuf, NoHomeDirectory> {
        xdg::devlaunch_cache().map(|cache| cache.join("metadata.json"))
    }

    /// Open the store at `metadata_path`, loading what is there.
    ///
    /// Returns the notices the load produced; a damaged file is recovered from
    /// rather than reported as an error.
    pub fn open(metadata_path: impl Into<PathBuf>) -> Result<(Self, Vec<Notice>), MetadataError> {
        let metadata_path = metadata_path.into();
        if let Some(parent) = metadata_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|error| MetadataError::CreateDir {
                path: parent.to_path_buf(),
                failure: error.into(),
            })?;
        }
        // Every file operation targets the real file, not a symlink pointing at
        // it: an atomic save renames a temp file over the target, which would
        // replace the link with a regular file and lose it for every later write.
        let file_path = resolve_link(&metadata_path);
        let lock_path = sibling(&file_path, ".lock");
        let mut storage = Self {
            metadata_path,
            file_path,
            lock_path,
            schema_version: SCHEMA_VERSION,
            repositories: IndexMap::new(),
            worktrees: IndexMap::new(),
            wait_watcher: None,
        };
        let notices = storage.load();
        Ok((storage, notices))
    }

    /// Be told when a mutation is about to queue behind another dl run.
    ///
    /// The one thing a returned notice cannot cover: the point of saying it is
    /// to explain a run that has gone quiet, so it has to be said before the
    /// wait rather than after it. Nothing has to subscribe.
    pub(crate) fn watch_waits(&mut self, watcher: impl Fn(WaitStarted) + 'static) {
        self.wait_watcher = Some(Box::new(watcher));
    }

    /// The path as the caller gave it, symlink and all.
    pub(crate) fn metadata_path(&self) -> &Path {
        &self.metadata_path
    }

    /// The schema version that was loaded — what a migration branches on.
    pub(crate) fn schema_version(&self) -> i64 {
        self.schema_version
    }

    pub(crate) fn repositories(&self) -> &IndexMap<String, BaseRepository> {
        &self.repositories
    }

    pub(crate) fn worktrees(&self) -> &IndexMap<String, WorktreeInfo> {
        &self.worktrees
    }

    // --- reads ------------------------------------------------------------

    pub(crate) fn get_repository(&self, owner: &str, repo: &str) -> Option<&BaseRepository> {
        self.repositories.get(&repository_key(owner, repo))
    }

    pub(crate) fn list_repositories(&self) -> Vec<&BaseRepository> {
        self.repositories.values().collect()
    }

    pub(crate) fn get_worktree(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Option<&WorktreeInfo> {
        self.worktrees.get(&worktree_key(owner, repo, branch))
    }

    pub(crate) fn list_worktrees(&self, filter: WorktreeFilter<'_>) -> Vec<&WorktreeInfo> {
        self.worktrees
            .values()
            .filter(|worktree| match filter {
                WorktreeFilter::All => true,
                WorktreeFilter::Owner(owner) => worktree.owner == owner,
                WorktreeFilter::OwnerAndRepo { owner, repo } => {
                    worktree.owner == owner && worktree.repo == repo
                }
            })
            .collect()
    }

    pub(crate) fn get_worktree_by_workspace_id(&self, workspace_id: &str) -> Option<&WorktreeInfo> {
        self.worktrees
            .values()
            .find(|worktree| worktree.workspace_id == workspace_id)
    }

    // --- mutations --------------------------------------------------------

    /// Add or update a repository.
    pub(crate) fn add_repository(
        &mut self,
        repository: BaseRepository,
    ) -> Result<Vec<Notice>, MetadataError> {
        let key = repository_key(&repository.owner, &repository.repo);
        self.exclusive(move |storage| {
            storage.repositories.insert(key, repository);
            storage.save()
        })
        .map(|((), notices)| notices)
    }

    /// Remove a repository, writing only if it was there.
    pub(crate) fn remove_repository(
        &mut self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<Notice>, MetadataError> {
        let key = repository_key(owner, repo);
        self.exclusive(move |storage| {
            if storage.repositories.shift_remove(&key).is_some() {
                storage.save()?;
            }
            Ok(())
        })
        .map(|((), notices)| notices)
    }

    /// Add or update a worktree, keeping its repository's branch list in step.
    ///
    /// Both changes are made in memory and written by one save: two writes would
    /// leave a window where the file says the worktree exists and its repository
    /// does not know about it.
    pub(crate) fn add_worktree(
        &mut self,
        worktree: WorktreeInfo,
    ) -> Result<Vec<Notice>, MetadataError> {
        let key = worktree_key(&worktree.owner, &worktree.repo, &worktree.branch);
        let repository = repository_key(&worktree.owner, &worktree.repo);
        let branch = worktree.branch.clone();
        self.exclusive(move |storage| {
            storage.worktrees.insert(key, worktree);
            if let Some(repository) = storage.repositories.get_mut(&repository)
                && !repository.worktrees.contains(&branch)
            {
                repository.worktrees.push(branch);
            }
            storage.save()
        })
        .map(|((), notices)| notices)
    }

    /// Remove a worktree and its entry in the repository's branch list.
    pub(crate) fn remove_worktree(
        &mut self,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Vec<Notice>, MetadataError> {
        let key = worktree_key(owner, repo, branch);
        let repository = repository_key(owner, repo);
        let branch = branch.to_owned();
        self.exclusive(move |storage| {
            if storage.worktrees.shift_remove(&key).is_none() {
                return Ok(());
            }
            if let Some(repository) = storage.repositories.get_mut(&repository)
                && let Some(at) = repository.worktrees.iter().position(|name| *name == branch)
            {
                repository.worktrees.remove(at);
            }
            storage.save()
        })
        .map(|((), notices)| notices)
    }

    /// Hold the metadata lock and reload before `work` runs.
    ///
    /// Every mutation goes through this. The in-memory copy was loaded whenever
    /// this process started, and other dl processes may have written since;
    /// rewriting the file from that stale copy silently drops their records, so
    /// reloading under the lock is what makes read-modify-write safe.
    ///
    /// Not reentrant (see [`super::locks`]): `work` must not call another
    /// mutator, because they take this same lock.
    fn exclusive<T>(
        &mut self,
        work: impl FnOnce(&mut Self) -> Result<T, MetadataError>,
    ) -> Result<(T, Vec<Notice>), MetadataError> {
        let lock_path = self.lock_path.clone();
        let watcher = self.wait_watcher.as_deref();
        let guard = hold_lock_watching(&lock_path, |wait| {
            if let Some(watcher) = watcher {
                watcher(wait);
            }
        })
        .map_err(MetadataError::Lock)?;
        let notices = self.load();
        let value = work(self)?;
        drop(guard);
        Ok((value, notices))
    }

    /// Write the metadata out atomically.
    ///
    /// A fresh temp file in the same directory, fsynced, then renamed over the
    /// real path, so an interrupted write can never leave a truncated
    /// `metadata.json` behind.
    pub(crate) fn save(&self) -> Result<(), MetadataError> {
        let document = Document {
            // The loaded version, not the constant: a cache a migration could
            // not finish keeps its old header, so the next run's version
            // comparison still lets it in (#180). Capped at SCHEMA_VERSION
            // because a file from a newer build has just been rewritten in
            // *this* build's shape, fields and all.
            version: self.schema_version.min(SCHEMA_VERSION),
            repositories: &self.repositories,
            worktrees: &self.worktrees,
        };
        let bytes = encode(&document)?;

        let directory = self
            .file_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = self
            .file_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // A name no other writer can be holding, in the same directory so the
        // rename stays atomic, created 0600 so the contents are never briefly
        // world-readable. Dropped without persisting, it takes the debris of a
        // failed write with it.
        let mut temp = tempfile::Builder::new()
            .prefix(&format!("{name}."))
            .suffix(".tmp")
            .tempfile_in(directory)
            .map_err(|error| MetadataError::CreateTemp {
                directory: directory.to_path_buf(),
                failure: error.into(),
            })?;

        let temp_path = temp.path().to_path_buf();
        let write = temp
            .write_all(&bytes)
            .and_then(|()| temp.as_file().sync_all());
        write.map_err(|error| MetadataError::Write {
            path: temp_path.clone(),
            failure: error.into(),
        })?;

        // Renaming a fresh file would otherwise reset the mode to the umask
        // default, silently widening a metadata.json the user locked down.
        if let Some(mode) = file_mode(&self.file_path) {
            fs::set_permissions(&temp_path, fs::Permissions::from_mode(mode)).map_err(|error| {
                MetadataError::SetMode {
                    path: temp_path.clone(),
                    mode,
                    failure: error.into(),
                }
            })?;
        }

        temp.persist(&self.file_path)
            .map_err(|error| MetadataError::Replace {
                from: temp_path,
                to: self.file_path.clone(),
                failure: error.error.into(),
            })?;
        Ok(())
    }

    /// The one write a migration makes: edit every record, then save once.
    ///
    /// `edit` runs against the in-memory worktree records — the v1→v2 migration
    /// renames clone directories and repoints the records that name them — and
    /// answers whether the header may move. Both halves reach disk in the single
    /// atomic save this makes, which is what keeps a header saying `2` from ever
    /// claiming more than the filesystem has done (#180). A refusal therefore
    /// costs the run the header and not the renames that did work: those are
    /// recorded immediately, and the next run finds each of them as "destination
    /// present, source gone" and catches metadata up to it.
    ///
    /// **No lock and no reload**, which is what [`MetadataStorage::save`] alone
    /// has always been and what `storage.py`'s migration used: the migration runs
    /// once, before the command the user typed, from a store loaded a moment
    /// earlier. Reloading under a lock would throw away the very edits `edit` is
    /// about to make, and holding the metadata lock across a whole-cache walk of
    /// renames would block every other dl run for the length of it.
    ///
    /// This is the only mutator that does not go through
    /// [`MetadataStorage::exclusive`], and the only one that may promote the
    /// header — every other write preserves the loaded version, because promoting
    /// it is the migration's job and no one else's.
    pub(crate) fn commit_migration(
        &mut self,
        edit: impl FnOnce(&mut IndexMap<String, WorktreeInfo>) -> SchemaHeader,
    ) -> Result<(), MetadataError> {
        if let SchemaHeader::Promote = edit(&mut self.worktrees) {
            self.schema_version = SCHEMA_VERSION;
        }
        self.save()
    }

    // --- loading ----------------------------------------------------------

    /// Load from disk, never failing on damaged input.
    fn load(&mut self) -> Vec<Notice> {
        self.schema_version = SCHEMA_VERSION;
        self.repositories = IndexMap::new();
        self.worktrees = IndexMap::new();

        let (data, mut notices) = self.read_file();
        let Some(mut object) = data else {
            return notices;
        };

        let (version, version_notice) = self.read_version(object.get("version"));
        self.schema_version = version;
        notices.extend(version_notice);

        let unknown_keys: Vec<String> = object
            .keys()
            .filter(|key| !KNOWN_SECTIONS.contains(&key.as_str()))
            .cloned()
            .collect();

        let (repositories, repository_notices) = self.read_section(
            object.remove(Section::Repositories.key()),
            Section::Repositories,
            BaseRepository::from_json,
        );
        self.repositories = repositories;
        notices.extend(repository_notices);

        let (worktrees, worktree_notices) = self.read_section(
            object.remove(Section::Worktrees.key()),
            Section::Worktrees,
            WorktreeInfo::from_json,
        );
        self.worktrees = worktrees;
        notices.extend(worktree_notices);

        if !unknown_keys.is_empty() {
            notices.push(Notice::UnknownTopLevelKeys {
                path: self.file_path.clone(),
                keys: unknown_keys,
            });
        }

        if notices.iter().any(Notice::implies_lossy_rewrite) {
            notices.push(self.backup());
        }
        notices
    }

    /// Read and sanity-check the file, quarantining it if it is unusable.
    fn read_file(&self) -> (Option<serde_json::Map<String, Value>>, Vec<Notice>) {
        if !self.file_path.exists() {
            return (None, Vec::new());
        }
        let problem = match fs::read(&self.file_path) {
            Err(error) => FileProblem::Unreadable(error.into()),
            // `from_slice` refuses bytes that are not UTF-8 as well as bytes
            // that are not JSON, which is the pair Python's one `except` caught.
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Err(error) => FileProblem::NotJson {
                    reason: error.to_string(),
                },
                Ok(Value::Object(object)) => return (Some(object), Vec::new()),
                Ok(other) => FileProblem::NotAnObject {
                    found: JsonKind::of(&other),
                },
            },
        };
        let notice = Notice::FileUnusable {
            path: self.file_path.clone(),
            problem,
            quarantine: self.quarantine(),
        };
        (None, vec![notice])
    }

    /// Move an unusable file aside so the data stays inspectable.
    fn quarantine(&self) -> Quarantine {
        let path = sibling(&self.file_path, ".corrupt");
        match fs::rename(&self.file_path, &path) {
            Ok(()) => Quarantine::MovedAside { path },
            Err(error) => Quarantine::CouldNotMove {
                path,
                failure: error.into(),
            },
        }
    }

    /// Copy the on-disk file aside before a lossy rewrite can overwrite it.
    ///
    /// This runs at load time, while the original bytes are still there: the
    /// next mutation rewrites the file from what was loaded, so anything the
    /// load could not round-trip is gone by then. A single slot, overwritten on
    /// repeat, separate from the quarantine slot.
    fn backup(&self) -> Notice {
        let path = sibling(&self.file_path, ".bak");
        let backup = match fs::copy(&self.file_path, &path) {
            Ok(_) => Backup::Copied { path },
            Err(error) => Backup::CouldNotCopy {
                path,
                failure: error.into(),
            },
        };
        Notice::OriginalPreserved {
            path: self.file_path.clone(),
            backup,
        }
    }

    /// Interpret the version header.
    fn read_version(&self, header: Option<&Value>) -> (i64, Option<Notice>) {
        // An absent version means a legacy pre-versioning file: the v1 shape.
        let Some(header) = header else {
            return (LEGACY_SCHEMA_VERSION, None);
        };
        // JSON has a single number type, so tools freely normalize 1 to 1.0; an
        // integral number is that version. A true/false header is nonsense
        // rather than version 1.
        let version = match header {
            Value::Number(number) => match number.as_i64() {
                Some(version) => Some(version),
                // Saturating, so a header nobody could have meant still reads as
                // "newer than this build" rather than wrapping into the past.
                None => number
                    .as_f64()
                    .filter(|value| value.is_finite() && value.fract() == 0.0)
                    .map(|value| value as i64),
            },
            _ => None,
        };
        let Some(version) = version else {
            // The entries do not depend on the header, so never discard them
            // over it: report it, read the file as legacy v1, and preserve the
            // original because the rewritten header will not match what is
            // there now.
            return (
                LEGACY_SCHEMA_VERSION,
                Some(Notice::VersionHeaderUnusable {
                    path: self.file_path.clone(),
                    found: header.clone(),
                }),
            );
        };

        if version > SCHEMA_VERSION {
            return (
                version,
                Some(Notice::VersionFromNewerBuild {
                    path: self.file_path.clone(),
                    found: version,
                    understood: SCHEMA_VERSION,
                }),
            );
        }

        // A version below SCHEMA_VERSION is an older shape. Plain saves preserve
        // it — only a fully successful migration promotes it (#180) — and the
        // value is exposed unchanged so a migration can branch on it.
        (version, None)
    }

    /// Rebuild one section, skipping (not discarding) individually broken entries.
    fn read_section<T>(
        &self,
        section_value: Option<Value>,
        section: Section,
        rebuild: impl Fn(Value) -> Result<Rebuilt<T>, NotRebuilt>,
    ) -> (IndexMap<String, T>, Vec<Notice>) {
        let mut loaded = IndexMap::new();
        let mut notices = Vec::new();
        let entries = match section_value {
            None => return (loaded, notices),
            Some(Value::Object(entries)) => entries,
            Some(other) => {
                notices.push(Notice::SectionUnusable {
                    path: self.file_path.clone(),
                    section,
                    found: JsonKind::of(&other),
                });
                return (loaded, notices);
            }
        };

        for (key, entry) in entries {
            if !entry.is_object() {
                notices.push(Notice::EntryUnusable {
                    path: self.file_path.clone(),
                    section,
                    key,
                    problem: EntryProblem::NotAnObject {
                        found: JsonKind::of(&entry),
                    },
                });
                continue;
            }
            match rebuild(entry) {
                Err(NotRebuilt { reason }) => notices.push(Notice::EntryUnusable {
                    path: self.file_path.clone(),
                    section,
                    key,
                    problem: EntryProblem::NotRebuilt { reason },
                }),
                Ok(rebuilt) => {
                    // The entry loaded, but a field only a newer build knows
                    // about is not carried into the rebuilt model and disappears
                    // on the next write.
                    if !rebuilt.unknown_fields.is_empty() {
                        notices.push(Notice::EntryHasUnknownFields {
                            path: self.file_path.clone(),
                            section,
                            key: key.clone(),
                            fields: rebuilt.unknown_fields,
                        });
                    }
                    loaded.insert(key, rebuilt.entry);
                }
            }
        }
        (loaded, notices)
    }
}

/// The document as it is written: these three keys, in this order.
#[derive(Serialize)]
struct Document<'a> {
    version: i64,
    repositories: &'a IndexMap<String, BaseRepository>,
    worktrees: &'a IndexMap<String, WorktreeInfo>,
}

fn repository_key(owner: &str, repo: &str) -> String {
    format!("{owner}/{repo}")
}

fn worktree_key(owner: &str, repo: &str, branch: &str) -> String {
    format!("{owner}/{repo}/{branch}")
}

/// `path` with `suffix` appended to its file name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// The permission bits of `path`, or `None` if it does not exist.
fn file_mode(path: &Path) -> Option<u32> {
    fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions().mode() & 0o7777)
}

/// The real file behind `path`, following it if it is a symlink.
///
/// Only the final component is resolved, and the chain behind it. Writing
/// atomically means renaming a temp file over the target, which would replace a
/// symlink with a regular file; anyone who points `metadata.json` at a synced
/// directory would silently lose the link and every later write. Resolving once
/// up front keeps every file operation — write, quarantine, backup — on the real
/// file.
///
/// A dangling link still resolves to what it names, and nothing here requires
/// the target to exist: a first run's `metadata.json` does not.
fn resolve_link(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_LINK_DEPTH {
        let is_link = fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false);
        if !is_link {
            return current;
        }
        let Ok(target) = fs::read_link(&current) else {
            return current;
        };
        current = if target.is_absolute() {
            target
        } else {
            match current.parent() {
                Some(parent) => parent.join(target),
                None => target,
            }
        };
    }
    current
}

/// Serialize `document` the way Python's `json.dump(..., indent=2)` does.
fn encode(document: &Document<'_>) -> Result<Vec<u8>, MetadataError> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, PythonJsonFormatter::default());
    document
        .serialize(&mut serializer)
        .map_err(|error| MetadataError::Encode {
            reason: error.to_string(),
        })?;
    Ok(bytes)
}

/// `json.dump(..., indent=2)`, escaping included.
///
/// Two-space indentation is what serde's pretty printer already does; what it
/// does not do is Python's `ensure_ascii`, which spells every non-ASCII
/// character as a `\uXXXX` escape. A branch name with an umlaut in it is enough
/// to make the two builds write different bytes for the same data, so the
/// escaping is matched rather than left to chance.
#[derive(Default)]
struct PythonJsonFormatter<'indent> {
    pretty: serde_json::ser::PrettyFormatter<'indent>,
}

impl serde_json::ser::Formatter for PythonJsonFormatter<'_> {
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        let mut ascii_from = 0;
        for (at, character) in fragment.char_indices() {
            if character.is_ascii() {
                continue;
            }
            writer.write_all(&fragment.as_bytes()[ascii_from..at])?;
            ascii_from = at + character.len_utf8();
            // Surrogate pairs for anything outside the basic plane, which is
            // what Python writes for an emoji.
            let mut units = [0u16; 2];
            for unit in character.encode_utf16(&mut units) {
                write!(writer, "\\u{unit:04x}")?;
            }
        }
        writer.write_all(&fragment.as_bytes()[ascii_from..])
    }

    // The rest is the pretty printer's layout, delegated unchanged.

    fn begin_array<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_array(writer)
    }

    fn end_array<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_array(writer)
    }

    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_array_value(writer, first)
    }

    fn end_array_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_array_value(writer)
    }

    fn begin_object<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_object(writer)
    }

    fn end_object<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_object(writer)
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_object_key(writer, first)
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_object_value(writer)
    }

    fn end_object_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_object_value(writer)
    }
}

#[cfg(test)]
mod tests {
    //! `test/test_worktree_storage.py`, re-pinned.
    //!
    //! Python patched two seams to observe behaviour it could not otherwise see
    //! — `Path.replace` to watch the atomic rename, `storage.save` to count
    //! writes. Both are reachable for real here: the rename is observable as
    //! "no debris and a complete file", and the single write as "one file that
    //! carries both halves of the change".
    //!
    //! The golden bytes below came from the Python this replaces, run from the
    //! worktree root against a `MetadataStorage` in a temp directory:
    //!
    //! ```text
    //! pixi run python -c "... s.add_repository(...); s.add_worktree(...);
    //!                     print(p.read_text(encoding='utf-8'))"
    //! ```
    //!
    //! They are constants so `cargo test` needs no Python, and they are what
    //! makes "byte-compatible with Python" an assertion rather than a claim.

    use super::*;
    use crate::domain::model::Timestamp;
    use jiff::civil;
    use serde_json::json;

    /// A file the real Python wrote: two-space indent, this key order, non-ASCII
    /// spelled as `\uXXXX`, and no trailing newline.
    const PYTHON_METADATA: &str = concat!(
        "{\n",
        "  \"version\": 2,\n",
        "  \"repositories\": {\n",
        "    \"blooop/devlaunch\": {\n",
        "      \"owner\": \"blooop\",\n",
        "      \"repo\": \"devlaunch\",\n",
        "      \"remote_url\": \"https://github.com/blooop/devlaunch.git\",\n",
        "      \"local_path\": \"/home/u/.cache/devlaunch/repos/blooop/devlaunch\",\n",
        "      \"default_branch\": \"main\",\n",
        "      \"last_fetched\": \"2026-08-18T14:03:22.123456\",\n",
        "      \"worktrees\": [\n",
        "        \"feature/br\\u00fcnch\"\n",
        "      ]\n",
        "    }\n",
        "  },\n",
        "  \"worktrees\": {\n",
        "    \"blooop/devlaunch/feature/br\\u00fcnch\": {\n",
        "      \"owner\": \"blooop\",\n",
        "      \"repo\": \"devlaunch\",\n",
        "      \"branch\": \"feature/br\\u00fcnch\",\n",
        "      \"local_path\": ",
        "\"/home/u/.cache/devlaunch/repos/blooop/devlaunch/clones/devlaunch-feature-br\\u00fcnch\",\n",
        "      \"workspace_id\": \"devlaunch-feature-brunch\",\n",
        "      \"created_at\": \"2026-08-18T14:03:22\",\n",
        "      \"last_used\": \"2026-08-18T14:03:22.123456\",\n",
        "      \"devpod_workspace_id\": \"devlaunch-feature-brunch\"\n",
        "    }\n",
        "  }\n",
        "}",
    );

    /// An empty store, as the real Python saves it.
    const PYTHON_EMPTY_METADATA: &str =
        "{\n  \"version\": 2,\n  \"repositories\": {},\n  \"worktrees\": {}\n}";

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temp dir")
    }

    /// A store on `metadata.json` in `directory`, asserting a clean load.
    fn quiet_storage(directory: &Path) -> MetadataStorage {
        let (storage, notices) =
            MetadataStorage::open(directory.join("metadata.json")).expect("a store");
        assert_eq!(notices, Vec::new(), "a clean load reports nothing");
        storage
    }

    fn open_at(path: &Path) -> (MetadataStorage, Vec<Notice>) {
        MetadataStorage::open(path).expect("a store")
    }

    fn write(path: &Path, text: &str) {
        fs::write(path, text).expect("writing the fixture");
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("reading the file")
    }

    fn names_in(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .expect("a listing")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn repository(owner: &str, repo: &str) -> BaseRepository {
        BaseRepository::new(
            owner,
            repo,
            &format!("https://github.com/{owner}/{repo}.git"),
            PathBuf::from(format!("/tmp/repos/{owner}/{repo}")),
        )
    }

    fn worktree(owner: &str, repo: &str, branch: &str) -> WorktreeInfo {
        WorktreeInfo {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            branch: branch.to_owned(),
            local_path: PathBuf::from(format!("/tmp/worktrees/{owner}/{repo}/{branch}")),
            workspace_id: branch.to_owned(),
            created_at: Timestamp::from_civil(civil::datetime(2024, 1, 1, 10, 0, 0, 0)),
            last_used: Timestamp::from_civil(civil::datetime(2024, 1, 1, 12, 0, 0, 0)),
            devpod_workspace_id: None,
        }
    }

    /// A valid stored repository entry.
    fn repository_entry(owner: &str, repo: &str) -> Value {
        json!({
            "owner": owner,
            "repo": repo,
            "remote_url": format!("https://github.com/{owner}/{repo}.git"),
            "local_path": format!("/tmp/repos/{owner}/{repo}"),
            "default_branch": "main",
            "last_fetched": null,
            "worktrees": [],
        })
    }

    /// A valid stored worktree entry.
    fn worktree_entry(branch: &str) -> Value {
        json!({
            "owner": "owner1",
            "repo": "repo1",
            "branch": branch,
            "local_path": format!("/tmp/worktrees/owner1/repo1/{branch}"),
            "workspace_id": branch,
            "created_at": "2024-01-01T10:00:00",
            "last_used": "2024-01-01T12:00:00",
            "devpod_workspace_id": null,
        })
    }

    /// A whole document, written to `metadata.json` in `directory`.
    fn given_metadata(directory: &Path, document: Value) -> PathBuf {
        let path = directory.join("metadata.json");
        write(&path, &serde_json::to_string(&document).expect("JSON"));
        path
    }

    // --- the store's own behaviour ----------------------------------------

    #[test]
    fn opening_it_creates_the_directory_it_lives_in() {
        let dir = temp_dir();
        let path = dir.path().join("subdir").join("metadata.json");

        let (storage, _) = open_at(&path);

        assert!(path.parent().expect("a parent").exists());
        assert_eq!(storage.metadata_path(), path);
    }

    #[test]
    fn a_store_with_no_file_behind_it_is_empty() {
        let dir = temp_dir();

        let storage = quiet_storage(dir.path());

        assert!(storage.repositories().is_empty());
        assert!(storage.worktrees().is_empty());
        assert_eq!(storage.schema_version(), SCHEMA_VERSION);
    }

    #[test]
    fn a_repository_can_be_added_read_listed_and_removed() {
        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());

        storage
            .add_repository(repository("test-owner", "test-repo"))
            .expect("saved");
        storage
            .add_repository(repository("owner2", "repo2"))
            .expect("saved");

        assert_eq!(
            storage.get_repository("test-owner", "test-repo"),
            Some(&repository("test-owner", "test-repo"))
        );
        assert_eq!(storage.get_repository("nonexistent", "repo"), None);
        assert_eq!(storage.list_repositories().len(), 2);

        storage
            .remove_repository("test-owner", "test-repo")
            .expect("saved");

        assert_eq!(storage.get_repository("test-owner", "test-repo"), None);
        assert_eq!(storage.list_repositories().len(), 1);
    }

    #[test]
    fn removing_a_repository_that_is_not_there_is_not_an_error() {
        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());

        storage
            .remove_repository("nonexistent", "repo")
            .expect("no error");
    }

    #[test]
    fn adding_a_worktree_records_it_on_its_repository_too() {
        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());
        storage
            .add_repository(repository("test-owner", "test-repo"))
            .expect("saved");

        storage
            .add_worktree(worktree("test-owner", "test-repo", "feature-branch"))
            .expect("saved");

        assert!(
            storage
                .worktrees()
                .contains_key("test-owner/test-repo/feature-branch")
        );
        assert_eq!(
            storage
                .get_repository("test-owner", "test-repo")
                .expect("the repository")
                .worktrees,
            vec!["feature-branch".to_owned()]
        );
    }

    #[test]
    fn a_worktree_is_recorded_even_with_no_repository_entry_to_update() {
        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());

        storage
            .add_worktree(worktree("test-owner", "test-repo", "feature-branch"))
            .expect("saved");

        assert!(
            storage
                .get_worktree("test-owner", "test-repo", "feature-branch")
                .is_some()
        );
        assert_eq!(storage.get_worktree("nonexistent", "repo", "branch"), None);
    }

    #[test]
    fn worktrees_list_all_by_owner_or_by_owner_and_repo() {
        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());
        for (owner, repo, branch) in [
            ("owner1", "repo1", "branch1"),
            ("owner1", "repo1", "branch2"),
            ("owner1", "repo2", "branch3"),
            ("owner2", "repo3", "branch4"),
        ] {
            storage
                .add_worktree(worktree(owner, repo, branch))
                .expect("saved");
        }

        assert_eq!(storage.list_worktrees(WorktreeFilter::All).len(), 4);
        assert_eq!(
            storage
                .list_worktrees(WorktreeFilter::Owner("owner1"))
                .len(),
            3
        );
        let branches: Vec<&str> = storage
            .list_worktrees(WorktreeFilter::OwnerAndRepo {
                owner: "owner1",
                repo: "repo1",
            })
            .iter()
            .map(|worktree| worktree.branch.as_str())
            .collect();
        assert_eq!(branches, vec!["branch1", "branch2"]);
    }

    #[test]
    fn a_worktree_can_be_found_by_its_workspace_id() {
        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());
        let mut entry = worktree("owner1", "repo1", "main");
        entry.workspace_id = "repo1-main".to_owned();
        storage.add_worktree(entry).expect("saved");

        let found = storage
            .get_worktree_by_workspace_id("repo1-main")
            .expect("the worktree");

        assert_eq!(found.owner, "owner1");
        assert_eq!(found.branch, "main");
        assert_eq!(storage.get_worktree_by_workspace_id("nonexistent"), None);
    }

    #[test]
    fn removing_a_worktree_takes_it_off_its_repository_too() {
        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());
        storage
            .add_repository(repository("test-owner", "test-repo"))
            .expect("saved");
        storage
            .add_worktree(worktree("test-owner", "test-repo", "feature-branch"))
            .expect("saved");

        storage
            .remove_worktree("test-owner", "test-repo", "feature-branch")
            .expect("saved");

        assert_eq!(
            storage.get_worktree("test-owner", "test-repo", "feature-branch"),
            None
        );
        assert!(
            storage
                .get_repository("test-owner", "test-repo")
                .expect("the repository")
                .worktrees
                .is_empty()
        );
    }

    #[test]
    fn removing_a_worktree_that_is_not_there_is_not_an_error() {
        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());

        storage
            .remove_worktree("nonexistent", "repo", "branch")
            .expect("no error");
    }

    #[test]
    fn what_one_store_wrote_the_next_one_reads() {
        let dir = temp_dir();
        let mut first = quiet_storage(dir.path());
        first
            .add_repository(repository("test-owner", "test-repo"))
            .expect("saved");
        first
            .add_worktree(worktree("test-owner", "test-repo", "feature-branch"))
            .expect("saved");

        let second = quiet_storage(dir.path());

        assert_eq!(
            second.get_repository("test-owner", "test-repo"),
            first.get_repository("test-owner", "test-repo")
        );
        assert_eq!(
            second.get_worktree("test-owner", "test-repo", "feature-branch"),
            first.get_worktree("test-owner", "test-repo", "feature-branch")
        );
    }

    #[test]
    fn one_write_carries_both_halves_of_a_worktree_change() {
        // Python counted `save` calls through a patched attribute; the reason it
        // counted is that the worktree and its repository's branch list must
        // never be written apart. That is visible in the file itself.
        let dir = temp_dir();
        let path = dir.path().join("metadata.json");
        let mut storage = quiet_storage(dir.path());
        storage
            .add_repository(repository("owner1", "repo1"))
            .expect("saved");

        storage
            .add_worktree(worktree("owner1", "repo1", "branch1"))
            .expect("saved");

        let on_disk: Value = serde_json::from_str(&read(&path)).expect("JSON");
        assert_eq!(
            on_disk["worktrees"]["owner1/repo1/branch1"]["branch"],
            "branch1"
        );
        assert_eq!(
            on_disk["repositories"]["owner1/repo1"]["worktrees"],
            json!(["branch1"])
        );

        storage
            .remove_worktree("owner1", "repo1", "branch1")
            .expect("saved");

        let on_disk: Value = serde_json::from_str(&read(&path)).expect("JSON");
        assert_eq!(on_disk["worktrees"], json!({}));
        assert_eq!(
            on_disk["repositories"]["owner1/repo1"]["worktrees"],
            json!([])
        );
    }

    // --- byte compatibility with Python -----------------------------------

    #[test]
    fn a_save_is_byte_for_byte_what_python_writes() {
        let dir = temp_dir();
        let path = dir.path().join("metadata.json");
        let mut storage = quiet_storage(dir.path());
        let mut repository = BaseRepository::new(
            "blooop",
            "devlaunch",
            "https://github.com/blooop/devlaunch.git",
            PathBuf::from("/home/u/.cache/devlaunch/repos/blooop/devlaunch"),
        );
        repository.last_fetched = Some(Timestamp::from_civil(civil::datetime(
            2026,
            8,
            18,
            14,
            3,
            22,
            123_456_000,
        )));
        storage.add_repository(repository).expect("saved");
        storage
            .add_worktree(WorktreeInfo {
                owner: "blooop".to_owned(),
                repo: "devlaunch".to_owned(),
                branch: "feature/brünch".to_owned(),
                local_path: PathBuf::from(
                    "/home/u/.cache/devlaunch/repos/blooop/devlaunch/clones/devlaunch-feature-brünch",
                ),
                workspace_id: "devlaunch-feature-brunch".to_owned(),
                created_at: Timestamp::from_civil(civil::datetime(2026, 8, 18, 14, 3, 22, 0)),
                last_used: Timestamp::from_civil(civil::datetime(
                    2026, 8, 18, 14, 3, 22, 123_456_000,
                )),
                devpod_workspace_id: Some("devlaunch-feature-brunch".to_owned()),
            })
            .expect("saved");

        assert_eq!(read(&path), PYTHON_METADATA);
    }

    #[test]
    fn an_empty_save_is_byte_for_byte_what_python_writes() {
        let dir = temp_dir();
        let path = dir.path().join("metadata.json");

        quiet_storage(dir.path()).save().expect("saved");

        assert_eq!(read(&path), PYTHON_EMPTY_METADATA);
    }

    #[test]
    fn a_file_python_wrote_loads_and_re_saves_to_the_same_bytes() {
        // The coexistence case in one test: the two builds alternate over one
        // file, so what Python wrote must load here and come back unchanged.
        let dir = temp_dir();
        let path = dir.path().join("metadata.json");
        write(&path, PYTHON_METADATA);

        let storage = quiet_storage(dir.path());

        assert_eq!(storage.schema_version(), 2);
        assert_eq!(
            storage
                .get_worktree("blooop", "devlaunch", "feature/brünch")
                .map(|w| w.workspace_id.as_str()),
            Some("devlaunch-feature-brunch")
        );
        storage.save().expect("saved");
        assert_eq!(read(&path), PYTHON_METADATA);
    }

    // --- a file that cannot be read at all --------------------------------

    #[test]
    fn an_unusable_file_is_quarantined_and_the_run_starts_empty() {
        for content in [
            "{not json",
            r#"{"repositories": {"a": {"owner": "a""#,
            "",
            "[]",
            "\"x\"",
        ] {
            let dir = temp_dir();
            let path = dir.path().join("metadata.json");
            write(&path, content);

            let (storage, notices) = open_at(&path);

            assert!(storage.repositories().is_empty(), "{content:?}");
            assert!(storage.worktrees().is_empty(), "{content:?}");
            let corrupt = dir.path().join("metadata.json.corrupt");
            assert_eq!(read(&corrupt), content, "the bytes stay inspectable");
            assert!(!path.exists(), "the unusable file was moved, not copied");
            match notices.as_slice() {
                [
                    Notice::FileUnusable {
                        path: named,
                        quarantine,
                        ..
                    },
                ] => {
                    assert_eq!(named, &path);
                    assert_eq!(quarantine, &Quarantine::MovedAside { path: corrupt });
                }
                other => panic!("one notice naming the quarantine, got {other:?}"),
            }
        }
    }

    #[test]
    fn bytes_that_are_not_utf8_are_corruption_rather_than_a_crash() {
        for content in [
            vec![b'{', b'"', b'a', b'"', b':', 0x82, 0xff, b'}'],
            "{\"repositories\": {}}"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ] {
            let dir = temp_dir();
            let path = dir.path().join("metadata.json");
            fs::write(&path, &content).expect("writing the fixture");

            let (storage, notices) = open_at(&path);

            assert!(storage.repositories().is_empty());
            let corrupt = dir.path().join("metadata.json.corrupt");
            assert_eq!(fs::read(&corrupt).expect("the quarantined bytes"), content);
            assert!(matches!(
                notices.as_slice(),
                [Notice::FileUnusable {
                    problem: FileProblem::NotJson { .. },
                    ..
                }]
            ));
        }
    }

    #[test]
    fn a_json_value_that_is_not_an_object_says_what_it_found() {
        let dir = temp_dir();
        let path = dir.path().join("metadata.json");
        write(&path, "[]");

        let (_, notices) = open_at(&path);

        assert!(matches!(
            notices.as_slice(),
            [Notice::FileUnusable {
                problem: FileProblem::NotAnObject {
                    found: JsonKind::Array
                },
                ..
            }]
        ));
    }

    #[test]
    fn repeated_corruption_overwrites_the_one_quarantine_slot() {
        let dir = temp_dir();
        let path = dir.path().join("metadata.json");
        for content in ["first-corruption", "second-corruption"] {
            write(&path, content);
            open_at(&path);
        }

        assert_eq!(
            names_in(dir.path()),
            vec!["metadata.json.corrupt".to_owned()]
        );
        assert_eq!(
            read(&dir.path().join("metadata.json.corrupt")),
            "second-corruption"
        );
    }

    #[test]
    fn a_store_is_usable_again_after_a_quarantine() {
        let dir = temp_dir();
        let path = dir.path().join("metadata.json");
        write(&path, "{broken");
        let (mut storage, _) = open_at(&path);

        storage
            .add_repository(repository("owner1", "repo1"))
            .expect("saved");

        assert!(
            quiet_storage(dir.path())
                .get_repository("owner1", "repo1")
                .is_some()
        );
    }

    // --- one bad entry must not cost the file -----------------------------

    #[test]
    fn a_worktree_entry_that_cannot_be_rebuilt_is_skipped_not_the_file() {
        let mut without_local_path = worktree_entry("bad");
        without_local_path
            .as_object_mut()
            .expect("an object")
            .remove("local_path");
        for bad in [
            without_local_path,
            json!({ "created_at": "not-a-timestamp", "owner": "owner1", "repo": "repo1",
                    "branch": "bad", "local_path": "/p", "workspace_id": "bad",
                    "last_used": "2024-01-01T12:00:00" }),
            json!({ "owner": "owner1", "repo": "repo1", "branch": "bad", "local_path": null,
                    "workspace_id": "bad", "created_at": "2024-01-01T10:00:00",
                    "last_used": "2024-01-01T12:00:00" }),
            json!("not-an-object"),
        ] {
            let dir = temp_dir();
            let path = given_metadata(
                dir.path(),
                json!({
                    "version": SCHEMA_VERSION,
                    "repositories": { "owner1/repo1": repository_entry("owner1", "repo1") },
                    "worktrees": {
                        "owner1/repo1/good": worktree_entry("good"),
                        "owner1/repo1/bad": bad,
                    },
                }),
            );

            let (storage, notices) = open_at(&path);

            assert_eq!(
                storage.worktrees().keys().collect::<Vec<_>>(),
                vec!["owner1/repo1/good"]
            );
            assert!(storage.repositories().contains_key("owner1/repo1"));
            assert!(!dir.path().join("metadata.json.corrupt").exists());
            assert!(path.exists(), "a skipped entry is not a corrupt file");
            // One notice naming the skipped entry, one naming the backup.
            match notices.as_slice() {
                [
                    Notice::EntryUnusable {
                        section: Section::Worktrees,
                        key,
                        ..
                    },
                    Notice::OriginalPreserved { .. },
                ] => assert_eq!(key, "owner1/repo1/bad"),
                other => panic!("a skip and a backup, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_skipped_entry_survives_in_the_backup_the_next_write_makes_necessary() {
        let dir = temp_dir();
        let path = given_metadata(
            dir.path(),
            json!({
                "version": SCHEMA_VERSION,
                "repositories": { "owner1/repo1": repository_entry("owner1", "repo1") },
                "worktrees": {
                    "owner1/repo1/good": worktree_entry("good"),
                    "owner1/repo1/bad": json!({ "owner": "owner1", "repo": "repo1",
                        "branch": "bad", "local_path": "/p", "workspace_id": "bad",
                        "created_at": "not-a-timestamp", "last_used": "2024-01-01T12:00:00" }),
                },
            }),
        );
        let original = read(&path);

        let (mut storage, notices) = open_at(&path);

        // The backup exists before anything can overwrite the file.
        let backup = dir.path().join("metadata.json.bak");
        assert_eq!(read(&backup), original);
        assert!(notices.iter().any(|notice| matches!(
            notice,
            Notice::OriginalPreserved {
                backup: Backup::Copied { .. },
                ..
            }
        )));

        storage
            .add_worktree(worktree("owner1", "repo1", "new"))
            .expect("saved");

        let on_disk: Value = serde_json::from_str(&read(&path)).expect("JSON");
        let mut keys: Vec<&String> = on_disk["worktrees"]
            .as_object()
            .expect("an object")
            .keys()
            .collect();
        keys.sort();
        assert_eq!(keys, vec!["owner1/repo1/good", "owner1/repo1/new"]);
        assert_eq!(
            read(&backup),
            original,
            "the backup is not touched by the write"
        );
        assert!(read(&backup).contains("not-a-timestamp"));
    }

    #[test]
    fn a_field_only_a_newer_build_knows_costs_the_field_not_the_entry() {
        let dir = temp_dir();
        let mut repository = repository_entry("owner1", "repo1");
        repository["future_repo_field"] = json!(1);
        let mut worktree = worktree_entry("branch1");
        worktree["pinned_by_newer_build"] = json!(true);
        let path = given_metadata(
            dir.path(),
            json!({
                "version": SCHEMA_VERSION,
                "repositories": { "owner1/repo1": repository },
                "worktrees": { "owner1/repo1/branch1": worktree },
            }),
        );
        let original = read(&path);

        let (storage, notices) = open_at(&path);

        assert_eq!(
            storage.worktrees().keys().collect::<Vec<_>>(),
            vec!["owner1/repo1/branch1"]
        );
        assert_eq!(
            storage.repositories().keys().collect::<Vec<_>>(),
            vec!["owner1/repo1"]
        );
        // One notice per entry naming the dropped field, one for the backup.
        match notices.as_slice() {
            [
                Notice::EntryHasUnknownFields {
                    section: Section::Repositories,
                    fields: repository_fields,
                    ..
                },
                Notice::EntryHasUnknownFields {
                    section: Section::Worktrees,
                    fields: worktree_fields,
                    ..
                },
                Notice::OriginalPreserved { .. },
            ] => {
                assert_eq!(repository_fields, &vec!["future_repo_field".to_owned()]);
                assert_eq!(worktree_fields, &vec!["pinned_by_newer_build".to_owned()]);
            }
            other => panic!("two dropped fields and a backup, got {other:?}"),
        }
        assert!(!dir.path().join("metadata.json.corrupt").exists());
        assert_eq!(read(&dir.path().join("metadata.json.bak")), original);

        storage.save().expect("saved");

        let rewritten = read(&path);
        assert!(!rewritten.contains("pinned_by_newer_build"));
        assert!(!rewritten.contains("future_repo_field"));
        assert!(
            rewritten.contains("owner1/repo1/branch1"),
            "the entry itself survived"
        );
        assert_eq!(read(&dir.path().join("metadata.json.bak")), original);
    }

    #[test]
    fn a_top_level_key_this_build_does_not_write_is_preserved_then_dropped() {
        let dir = temp_dir();
        let path = given_metadata(
            dir.path(),
            json!({
                "version": SCHEMA_VERSION,
                "repositories": { "owner1/repo1": repository_entry("owner1", "repo1") },
                "worktrees": {},
                "pinned_workspaces": { "owner1/repo1": true },
            }),
        );
        let original = read(&path);

        let (storage, notices) = open_at(&path);

        assert_eq!(
            storage.repositories().keys().collect::<Vec<_>>(),
            vec!["owner1/repo1"]
        );
        match notices.as_slice() {
            [
                Notice::UnknownTopLevelKeys { keys, .. },
                Notice::OriginalPreserved { .. },
            ] => {
                assert_eq!(keys, &vec!["pinned_workspaces".to_owned()]);
            }
            other => panic!("an unknown key and a backup, got {other:?}"),
        }
        let backup = dir.path().join("metadata.json.bak");
        assert_eq!(read(&backup), original);

        storage.save().expect("saved");

        assert!(!read(&path).contains("pinned_workspaces"));
        assert_eq!(read(&backup), original);
    }

    #[test]
    fn the_backup_slot_is_one_slot_and_is_not_the_quarantine_slot() {
        let dir = temp_dir();
        let path = dir.path().join("metadata.json");
        for marker in ["first", "second"] {
            given_metadata(
                dir.path(),
                json!({
                    "version": SCHEMA_VERSION,
                    "repositories": {},
                    "worktrees": { format!("owner1/repo1/{marker}"): "not-an-object" },
                }),
            );
            open_at(&path);
        }

        assert_eq!(
            names_in(dir.path()),
            vec!["metadata.json".to_owned(), "metadata.json.bak".to_owned()]
        );
        assert!(read(&dir.path().join("metadata.json.bak")).contains("second"));
    }

    #[test]
    fn a_repository_entry_that_cannot_be_rebuilt_is_skipped() {
        let dir = temp_dir();
        let mut bad = repository_entry("owner1", "bad");
        bad["local_path"] = Value::Null;
        let path = given_metadata(
            dir.path(),
            json!({
                "version": SCHEMA_VERSION,
                "repositories": { "owner1/good": repository_entry("owner1", "good"), "owner1/bad": bad },
                "worktrees": {},
            }),
        );

        let (storage, notices) = open_at(&path);

        assert_eq!(
            storage.repositories().keys().collect::<Vec<_>>(),
            vec!["owner1/good"]
        );
        assert!(!dir.path().join("metadata.json.corrupt").exists());
        assert!(dir.path().join("metadata.json.bak").exists());
        assert!(notices.iter().any(|notice| matches!(
            notice,
            Notice::EntryUnusable { section: Section::Repositories, key, .. } if key == "owner1/bad"
        )));
    }

    #[test]
    fn a_section_that_is_not_an_object_costs_that_section_only() {
        let dir = temp_dir();
        let path = given_metadata(
            dir.path(),
            json!({
                "version": SCHEMA_VERSION,
                "repositories": ["not", "an", "object"],
                "worktrees": { "owner1/repo1/branch1": worktree_entry("branch1") },
            }),
        );

        let (storage, notices) = open_at(&path);

        assert!(storage.repositories().is_empty());
        assert_eq!(
            storage.worktrees().keys().collect::<Vec<_>>(),
            vec!["owner1/repo1/branch1"]
        );
        assert!(!dir.path().join("metadata.json.corrupt").exists());
        assert!(dir.path().join("metadata.json.bak").exists());
        match notices.as_slice() {
            [
                Notice::SectionUnusable {
                    section: Section::Repositories,
                    found: JsonKind::Array,
                    ..
                },
                Notice::OriginalPreserved { .. },
            ] => {}
            other => panic!("an unusable section and a backup, got {other:?}"),
        }
    }

    // --- atomic save ------------------------------------------------------

    #[test]
    fn a_save_leaves_no_debris_beside_the_file_it_wrote() {
        // The temp file is renamed into place, not left behind, and the lock
        // sidecar is deliberate and permanent (unlinking an flock'd file breaks
        // the lock) so it is not debris.
        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());

        storage
            .add_repository(repository("owner1", "repo1"))
            .expect("saved");

        assert_eq!(
            names_in(dir.path()),
            vec!["metadata.json".to_owned(), "metadata.json.lock".to_owned()]
        );
    }

    #[test]
    fn a_write_that_cannot_start_leaves_the_previous_file_readable() {
        // Write failures are not swallowed: silently losing the workspace list is
        // worse than an error. A directory nothing may create a file in is the
        // failure available without patching a seam — the temp file is where the
        // write starts, so the original is never touched.
        let dir = temp_dir();
        let path = dir.path().join("nested").join("metadata.json");
        let storage = {
            let (mut storage, _) = open_at(&path);
            storage
                .add_repository(repository("owner1", "repo1"))
                .expect("saved");
            storage
        };
        let original = read(&path);
        let nested = path.parent().expect("a parent").to_path_buf();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o500)).expect("chmod");

        let failed = storage.save().expect_err("the write cannot start");

        fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).expect("chmod");
        assert!(
            matches!(failed, MetadataError::CreateTemp { .. }),
            "{failed:?}"
        );
        assert_eq!(read(&path), original);
        assert_eq!(
            names_in(&nested),
            vec!["metadata.json".to_owned(), "metadata.json.lock".to_owned()],
            "a failed write leaves no half-written file behind"
        );
    }

    // --- file permissions -------------------------------------------------

    #[test]
    fn an_existing_mode_survives_the_atomic_replace() {
        let dir = temp_dir();
        let path = given_metadata(
            dir.path(),
            json!({ "version": SCHEMA_VERSION, "repositories": {}, "worktrees": {} }),
        );
        for mode in [0o600, 0o640] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");

            quiet_storage(dir.path()).save().expect("saved");

            assert_eq!(file_mode(&path), Some(mode));
        }
    }

    #[test]
    fn a_file_created_from_scratch_is_private() {
        let dir = temp_dir();
        let path = dir.path().join("metadata.json");

        quiet_storage(dir.path()).save().expect("saved");

        assert_eq!(
            file_mode(&path).expect("a mode") & 0o077,
            0,
            "metadata.json holds repo owners and local paths: keep it private"
        );
    }

    // --- a symlinked metadata.json ----------------------------------------

    /// A `metadata.json` that is a symlink to a file in another directory.
    fn linked(dir: &Path) -> (PathBuf, PathBuf) {
        let real = dir.join("synced").join("metadata.json");
        fs::create_dir(real.parent().expect("a parent")).expect("mkdir");
        let link = dir.join("metadata.json");
        std::os::unix::fs::symlink(&real, &link).expect("a symlink");
        (real, link)
    }

    #[test]
    fn a_save_writes_through_a_symlink_rather_than_replacing_it() {
        let dir = temp_dir();
        let (real, link) = linked(dir.path());
        write(
            &real,
            &serde_json::to_string(
                &json!({ "version": SCHEMA_VERSION, "repositories": {}, "worktrees": {} }),
            )
            .expect("JSON"),
        );

        let (mut storage, _) = open_at(&link);
        storage
            .add_repository(repository("owner1", "repo1"))
            .expect("saved");

        assert!(
            fs::symlink_metadata(&link)
                .expect("a stat")
                .file_type()
                .is_symlink(),
            "the link itself is still a link"
        );
        assert_eq!(fs::read_link(&link).expect("the target"), real);
        assert!(read(&real).contains("owner1/repo1"));
        let (reopened, _) = open_at(&link);
        assert!(reopened.get_repository("owner1", "repo1").is_some());
        assert_eq!(
            names_in(real.parent().expect("a parent")),
            vec!["metadata.json".to_owned(), "metadata.json.lock".to_owned()],
            "the lock and the temp file follow the real file, not the link"
        );
    }

    #[test]
    fn corruption_behind_a_symlink_quarantines_the_real_file() {
        let dir = temp_dir();
        let (real, link) = linked(dir.path());
        write(&real, "{broken");

        let (_, notices) = open_at(&link);

        assert!(
            fs::symlink_metadata(&link)
                .expect("a stat")
                .file_type()
                .is_symlink()
        );
        assert!(!real.exists());
        let corrupt = real.with_file_name("metadata.json.corrupt");
        assert_eq!(read(&corrupt), "{broken");
        assert!(matches!(
            notices.as_slice(),
            [Notice::FileUnusable { quarantine: Quarantine::MovedAside { path }, .. }] if *path == corrupt
        ));
    }

    // --- the version header -----------------------------------------------

    #[test]
    fn a_round_trip_writes_and_reads_back_the_current_version() {
        let dir = temp_dir();
        let path = dir.path().join("metadata.json");

        quiet_storage(dir.path()).save().expect("saved");

        assert_eq!(SCHEMA_VERSION, 2);
        let on_disk: Value = serde_json::from_str(&read(&path)).expect("JSON");
        assert_eq!(on_disk["version"], json!(2));
        assert_eq!(quiet_storage(dir.path()).schema_version(), 2);
    }

    #[test]
    fn a_file_with_no_version_header_is_the_oldest_shape_read_in_silence() {
        // Not the *current* version: an absent header must still put the file
        // below SCHEMA_VERSION, or the id-scheme migration keyed on that
        // comparison would skip exactly the caches that predate versioning.
        let dir = temp_dir();
        let path = given_metadata(
            dir.path(),
            json!({
                "repositories": { "owner1/repo1": repository_entry("owner1", "repo1") },
                "worktrees": { "owner1/repo1/branch1": worktree_entry("branch1") },
            }),
        );

        let (storage, notices) = open_at(&path);

        assert_eq!(storage.schema_version(), LEGACY_SCHEMA_VERSION);
        const { assert!(LEGACY_SCHEMA_VERSION < SCHEMA_VERSION) };
        assert_eq!(storage.repositories().len(), 1);
        assert_eq!(storage.worktrees().len(), 1);
        assert_eq!(notices, Vec::new());
        assert!(!dir.path().join("metadata.json.corrupt").exists());
    }

    #[test]
    fn a_file_from_a_newer_build_loads_as_is_and_is_preserved_before_a_rewrite() {
        let dir = temp_dir();
        let path = given_metadata(
            dir.path(),
            json!({
                "version": 99,
                "repositories": { "owner1/repo1": repository_entry("owner1", "repo1") },
                "worktrees": { "owner1/repo1/branch1": worktree_entry("branch1") },
            }),
        );
        let original = read(&path);

        let (storage, notices) = open_at(&path);

        assert_eq!(storage.schema_version(), 99);
        assert_eq!(storage.repositories().len(), 1);
        assert_eq!(storage.worktrees().len(), 1);
        assert!(path.exists());
        assert!(!dir.path().join("metadata.json.corrupt").exists());
        match notices.as_slice() {
            [
                Notice::VersionFromNewerBuild {
                    found: 99,
                    understood: 2,
                    ..
                },
                Notice::OriginalPreserved { .. },
            ] => {}
            other => panic!("a newer header and a backup, got {other:?}"),
        }
        let backup = dir.path().join("metadata.json.bak");
        assert_eq!(read(&backup), original);

        storage.save().expect("saved");

        // The promise is not immutability: the file is rewritten in this format.
        let on_disk: Value = serde_json::from_str(&read(&path)).expect("JSON");
        assert_eq!(on_disk["version"], json!(SCHEMA_VERSION));
        assert_eq!(read(&backup), original);
    }

    #[test]
    fn an_older_header_survives_a_write_because_promoting_it_is_the_migrations_job() {
        // The header is what tells the migration this cache still needs it, so a
        // save from an unrelated operation must not answer that question on its
        // behalf (#180).
        let dir = temp_dir();
        let path = given_metadata(
            dir.path(),
            json!({
                "version": 0,
                "repositories": { "owner1/repo1": repository_entry("owner1", "repo1") },
                "worktrees": { "owner1/repo1/branch1": worktree_entry("branch1") },
            }),
        );

        let (storage, notices) = open_at(&path);

        assert_eq!(storage.schema_version(), 0);
        assert_eq!(notices, Vec::new());
        assert!(!dir.path().join("metadata.json.bak").exists());

        storage.save().expect("saved");

        let on_disk: Value = serde_json::from_str(&read(&path)).expect("JSON");
        assert_eq!(on_disk["version"], json!(0));
        assert_eq!(quiet_storage(dir.path()).schema_version(), 0);
    }

    #[test]
    fn an_integral_number_is_that_version_however_it_is_spelled() {
        // JSON has one number type, so tools freely normalize 1 to 1.0.
        for (header, expected) in [("1.0", 1), ("2.0", 2), ("3.0", 3)] {
            let dir = temp_dir();
            let path = dir.path().join("metadata.json");
            write(&path, &document_with_header(header));

            let (storage, notices) = open_at(&path);

            assert_eq!(storage.schema_version(), expected, "{header}");
            assert_eq!(storage.repositories().len(), 1);
            assert_eq!(storage.worktrees().len(), 1);
            if expected <= SCHEMA_VERSION {
                assert_eq!(notices, Vec::new(), "{header}");
                assert!(!dir.path().join("metadata.json.bak").exists());
            }
        }
    }

    #[test]
    fn a_header_that_is_not_a_version_never_costs_the_entries() {
        // An unreadable header is read as the oldest shape, not the current one,
        // so a cache that needs migrating never claims to be current.
        for header in ["true", "null", "\"1\"", "1.5", "[1]"] {
            let dir = temp_dir();
            let path = dir.path().join("metadata.json");
            write(&path, &document_with_header(header));
            let original = read(&path);

            let (storage, notices) = open_at(&path);

            assert_eq!(storage.schema_version(), LEGACY_SCHEMA_VERSION, "{header}");
            assert_eq!(storage.repositories().len(), 1, "{header}");
            assert_eq!(storage.worktrees().len(), 1, "{header}");
            assert!(path.exists());
            assert!(!dir.path().join("metadata.json.corrupt").exists());
            let backup = dir.path().join("metadata.json.bak");
            match notices.as_slice() {
                [
                    Notice::VersionHeaderUnusable { found, .. },
                    Notice::OriginalPreserved { .. },
                ] => assert_eq!(found, &serde_json::from_str::<Value>(header).expect("JSON")),
                other => panic!("an unusable header and a backup for {header}, got {other:?}"),
            }
            assert_eq!(read(&backup), original);
        }
    }

    /// A document whose `version` is the raw text `header`.
    fn document_with_header(header: &str) -> String {
        format!(
            "{{\"version\": {header}, \"repositories\": {{\"owner1/repo1\": {}}}, \
             \"worktrees\": {{\"owner1/repo1/branch1\": {}}}}}",
            serde_json::to_string(&repository_entry("owner1", "repo1")).expect("JSON"),
            serde_json::to_string(&worktree_entry("branch1")).expect("JSON"),
        )
    }

    // --- the lock -----------------------------------------------------------

    #[test]
    fn the_lock_is_a_sidecar_and_not_the_file_itself() {
        // A save replaces metadata.json by rename, so a lock held on that inode
        // would guard nothing once the first write landed.
        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());

        storage
            .add_repository(repository("owner1", "repo1"))
            .expect("saved");

        assert!(dir.path().join("metadata.json.lock").exists());
    }

    #[test]
    fn a_mutation_reloads_under_the_lock_rather_than_rewriting_a_stale_copy() {
        // Two stores over one file is what two dl runs look like: the second
        // loaded before the first wrote, so a rewrite from its own copy would
        // drop the first one's record.
        let dir = temp_dir();
        let mut first = quiet_storage(dir.path());
        let mut second = quiet_storage(dir.path());

        first
            .add_repository(repository("owner1", "first"))
            .expect("saved");
        second
            .add_repository(repository("owner2", "second"))
            .expect("saved");

        let reloaded = quiet_storage(dir.path());
        let mut keys: Vec<&String> = reloaded.repositories().keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["owner1/first", "owner2/second"]);
    }

    #[test]
    fn a_run_that_has_to_queue_can_say_so_before_it_blocks() {
        // The one thing a returned notice cannot cover, because the point of
        // saying it is to explain a run that has gone quiet.
        use std::sync::mpsc;

        let dir = temp_dir();
        let mut storage = quiet_storage(dir.path());
        let (announced, waits) = mpsc::channel();
        storage.watch_waits(move |wait| announced.send(wait).expect("the test listens"));
        let held = crate::domain::locks::hold_lock(&dir.path().join("metadata.json.lock"))
            .expect("the sidecar lock");
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(held);
        });

        storage
            .add_repository(repository("owner1", "repo1"))
            .expect("saved");

        release.join().expect("the holder let go");
        let wait = waits.try_recv().expect("the wait was announced");
        assert_eq!(wait.lock_path, dir.path().join("metadata.json.lock"));
    }
}
