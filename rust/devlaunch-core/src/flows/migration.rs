//! Bring a cache written by an older devlaunch onto the current id scheme.
//!
//! Ported from `devlaunch/worktree/migration.py`, and ported rather than dropped
//! deliberately: the Rust binary ships as the *same* `dl` against the *same*
//! cache, so a machine that has never run the migration must still get it, and
//! the cutover release must migrate nothing new (docs/rust-rewrite-plan.md,
//! cutover check 3).
//!
//! Before blooop/devlaunch#64 a clone directory's leaf was the flattened branch
//! name (`<cache>/repos/blooop/devlaunch/main`) and the devpod workspace id was a
//! second, separately derived string. Now [`WorkspaceId`] derives one id that
//! names both (`devlaunch-main-zovomobo`). Every clone directory written by an
//! older build therefore sits under a name nothing looks for any more.
//!
//! **Renaming is the right answer, not orphaning.** A workspace is a `git clone`
//! whose `origin` points at the `.bare` path, and `.bare` does not move, so a
//! plain rename is lossless: the clone keeps working and **uncommitted work
//! survives**. That work is the one thing in the cache that is not cheaply
//! recreatable, which is what decides the strategy (see #55).
//!
//! **The trigger is the version header, not the directory name.** `metadata.json`
//! carries a `version` (#56), so this runs exactly when
//! `schema_version < SCHEMA_VERSION` and then writes the new version. Sniffing the
//! leaf for "a dash plus consonant-vowel pairs" was considered and rejected: a
//! branch literally named `foo-bexoza` false-positives, and the header makes the
//! trigger deterministic and idempotent by construction.
//!
//! **Write ordering.** All renames happen first; then a single save writes the new
//! paths, and the new version header *only if every rename succeeded*, in one
//! atomic replace ([`MetadataStorage::commit_migration`]). Nothing writes the
//! header early, so "header says 2" always means "every rename this migration
//! could ever perform is done". The two outcomes it can never perform — a
//! collision with a directory another record owns, and a branch no legal id
//! derives from — are reported and deliberately left behind, because retrying
//! those would never end differently. A crash anywhere in the renames leaves the
//! header at 1, so the next run migrates again and finds each already-renamed
//! directory as "destination present, source gone" — which it treats as a resumed
//! rename and simply catches metadata up to. The reverse ordering has no safe
//! resume: saving first would bump the header to 2 while directories were still
//! under their old names, and the next run would skip them for good.
//!
//! **A refusal is held to the crash standard** (#180). A rename the filesystem
//! declines — read-only mount, tightened permissions, full disk — is not a crash,
//! but stranding its records would be just as permanent, so the header stays at 1
//! and the next run retries exactly the refused directories. The save still
//! happens: the renames that did work are recorded immediately, and the resume path
//! above is what stops them being redone. That is why every other save writes the
//! store's own version rather than the constant — the migration is not the only
//! writer, and a save from any other operation would otherwise re-strand the
//! records this run deliberately left behind. A permanently refusing cache
//! therefore re-reports on every invocation, which is correct: the walk is bounded
//! and the report names directories that really do still need a hand.
//!
//! # No process, and nothing printed
//!
//! This module takes no runner, which is what makes "the migration spawns nothing"
//! a fact about its type rather than a test: it renames directories and rewrites
//! records, and the orphaned container ids it reports come out of `metadata.json`.
//! Python's `_announce` wrote nine sentences to stderr; core renders no English
//! (#251), so the [`MigrationReport`] carries every fact those sentences
//! interpolated and the `dl` binary writes the words. The two *listing files* are
//! written here, because their contents are data — one path or one id per line —
//! and the notice that names them needs to know whether they exist.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::repo_manager::{BARE_DIR_NAME, clone_dir};
use crate::domain::metadata::{MetadataError, MetadataStorage, SCHEMA_VERSION, SchemaHeader};
use crate::domain::model::WorktreeInfo;
use crate::domain::workspace_id::WorkspaceId;

/// Old devpod workspace ids, one per line, for the cleanup command in the notice.
pub(crate) const ORPHAN_LIST_NAME: &str = "orphaned-workspaces.txt";

/// Clone directories the migration deliberately did not rename, one path per line.
pub(crate) const UNMIGRATED_LIST_NAME: &str = "unmigrated-clones.txt";

/// One directory that moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Renamed {
    pub(crate) from: PathBuf,
    pub(crate) to: PathBuf,
}

/// One rename the filesystem refused, and what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenameRefused {
    pub(crate) from: PathBuf,
    pub(crate) to: PathBuf,
    pub(crate) reason: String,
}

/// A record holding a ref no id can be derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnusableRecord {
    pub(crate) path: PathBuf,
    pub(crate) branch: String,
}

/// A rename whose destination is another record's clone directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Blocked {
    pub(crate) from: PathBuf,
    pub(crate) to: PathBuf,
}

/// A directory the scan could not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotScanned {
    pub(crate) path: PathBuf,
    pub(crate) reason: String,
}

/// What became of one of the two listing files.
///
/// The notice that names a listing has to degrade to an instruction rather than to
/// a path that is not there: naming a file that does not exist is worse than
/// naming none, because the user runs the pasted command against nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Listing {
    /// There was nothing to list, so no file was written.
    #[default]
    NothingToList,
    Written {
        path: PathBuf,
        lines: usize,
    },
    CouldNotWrite {
        path: PathBuf,
        reason: String,
    },
}

/// What one migration run did, for the caller and for the notices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    /// Each directory actually renamed.
    pub(crate) renamed: Vec<Renamed>,
    /// Each rename the filesystem refused. Non-empty is what keeps the version
    /// header behind (#180).
    pub(crate) failed: Vec<RenameRefused>,
    /// Recorded paths that no longer exist, so there was nothing to rename.
    pub(crate) missing: Vec<PathBuf>,
    /// Directories left under their old name because no record names their ref.
    pub(crate) unmigrated: Vec<PathBuf>,
    /// Records holding a ref no id can be derived from.
    pub(crate) unusable: Vec<UnusableRecord>,
    /// Renames whose derived name is another record's clone.
    pub(crate) blocked: Vec<Blocked>,
    /// Old devpod workspace ids, now orphaned because the id derivation changed.
    pub(crate) orphaned_ids: Vec<String>,
    /// Corners of the cache the record-less scan could not read.
    pub(crate) not_scanned: Vec<NotScanned>,
    pub(crate) orphan_listing: Listing,
    pub(crate) unmigrated_listing: Listing,
}

/// Migrate `storage` and the clone directories under `repos_dir`, once.
///
/// Answers `None` when the cache is already current, which is the common case and
/// costs a single integer comparison — no filesystem scan and no devpod call.
pub fn migrate_cache(
    storage: &mut MetadataStorage,
    repos_dir: &Path,
) -> Result<Option<MigrationReport>, MetadataError> {
    if storage.schema_version() >= SCHEMA_VERSION {
        return Ok(None);
    }

    let mut report = MigrationReport::default();
    let cache_dir = storage
        .metadata_path()
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();

    // One edit, one save: the new paths and the new header travel together, so the
    // header can never claim more than the filesystem has done.
    storage.commit_migration(|worktrees| {
        // Snapshotted before any record is touched: it has to describe the layout
        // the run started from, not one the run is halfway through rewriting.
        let claimed: HashSet<PathBuf> = worktrees
            .values()
            .map(|record| record.local_path.clone())
            .collect();
        for record in worktrees.values_mut() {
            migrate_record(record, repos_dir, &claimed, &mut report);
        }

        // Anything still under an old-scheme name that no record claims. Computed
        // after the renames so it picks up both never-recorded directories and the
        // leftover side of a collision, and excludes everything just moved into
        // place.
        let recorded: HashSet<PathBuf> = worktrees
            .values()
            .map(|record| record.local_path.clone())
            .collect();
        report.unmigrated = clone_dirs(repos_dir, &mut report.not_scanned)
            .into_iter()
            .filter(|path| !recorded.contains(path))
            .collect();

        if report.failed.is_empty() {
            SchemaHeader::Promote
        } else {
            SchemaHeader::LeaveBehind
        }
    })?;

    // Written after the save, because they describe what the save recorded. Each
    // one is data — one path or one id per line — and the words that name them are
    // the binary's.
    if !report.orphaned_ids.is_empty() {
        let mut ids: Vec<String> = report.orphaned_ids.clone();
        ids.sort();
        report.orphan_listing = write_lines(&cache_dir.join(ORPHAN_LIST_NAME), &ids);
    }
    if !report.unmigrated.is_empty() {
        let paths: Vec<String> = report
            .unmigrated
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        report.unmigrated_listing = write_lines(&cache_dir.join(UNMIGRATED_LIST_NAME), &paths);
    }
    Ok(Some(report))
}

/// Put one record's directory under its derived name and update the record.
///
/// `local_path` as stored is the source, never a recomputed old path: the record is
/// the truth about where the clone is now, which is the same principle that made
/// removal work for old-scheme workspaces (#64).
///
/// `claimed` is every path some record pointed at before this run started.
fn migrate_record(
    record: &mut WorktreeInfo,
    repos_dir: &Path,
    claimed: &HashSet<PathBuf>,
    report: &mut MigrationReport,
) {
    let Ok(workspace) = WorkspaceId::new(&record.owner, &record.repo, &record.branch) else {
        // The old derivation coerced unsafe refs instead of rejecting them, so a
        // stored branch is not necessarily a legal ref. No id can be derived, so
        // there is no name to rename to; leave the record and the directory as they
        // are and say so.
        report.unusable.push(UnusableRecord {
            path: record.local_path.clone(),
            branch: record.branch.clone(),
        });
        return;
    };

    let src = record.local_path.clone();
    let dest = clone_dir(repos_dir, &record.owner, &record.repo, &workspace.value());

    if dest != src && claimed.contains(&dest) {
        // The derived name is a directory some *other* record owns. Only possible
        // when a branch was literally named after another branch's derived id —
        // #55's `foo-bexoza` case, now needing an exact hash match. Rename nothing
        // and, unlike every other outcome, do not repoint the record either:
        // adopting a clone another record owns is how one workspace's `rm` deletes
        // another's work, which is the class of bug #9766 was.
        report.blocked.push(Blocked {
            from: src,
            to: dest,
        });
        return;
    }

    if dest.exists() {
        // Either an interrupted earlier run already renamed this clone, or a
        // newer-scheme clone was created alongside the old one. Rename nothing. The
        // record follows the canonically named directory, so that a later
        // `dl … rm` deletes the clone devpod is actually using; a leftover source
        // is reported below, because it becomes a directory no record points at.
    } else if src.exists() {
        if !rename(&src, &dest, report) {
            return;
        }
    } else {
        // Already stale before this run: the record outlived its directory. Not a
        // failure — repointing it at the derived path is what a fresh clone would
        // use, and the workspace's existence is read off the filesystem, so nothing
        // is misled.
        report.missing.push(src);
    }

    let derived = workspace.value();
    if record.workspace_id != derived {
        report.orphaned_ids.push(record.workspace_id.clone());
    }
    record.local_path = dest;
    // The record carries the derived id, because removal by id looks records up by
    // exactly the id dl derives from the spec. `devpod_workspace_id` is left alone:
    // #55 flagged holding two ids in one record as a modelling defect, and giving
    // that field a second meaning ("the orphaned old container") would make the
    // defect worse. The orphaned ids go in the report instead.
    record.workspace_id = derived;
}

/// Move `src` to `dest`, recording the outcome. `false` if it did not happen.
///
/// A rename and not a copying move: a rename either happens or does not, while a
/// copying fallback could leave a half-written duplicate of a clone that holds
/// uncommitted work. A cross-filesystem cache is rare enough to report and leave to
/// the user.
fn rename(src: &Path, dest: &Path, report: &mut MigrationReport) -> bool {
    let moved = match dest.parent() {
        Some(parent) => std::fs::create_dir_all(parent).and_then(|()| std::fs::rename(src, dest)),
        None => std::fs::rename(src, dest),
    };
    match moved {
        Ok(()) => {
            report.renamed.push(Renamed {
                from: src.to_path_buf(),
                to: dest.to_path_buf(),
            });
            true
        }
        Err(error) => {
            report.failed.push(RenameRefused {
                from: src.to_path_buf(),
                to: dest.to_path_buf(),
                reason: error.to_string(),
            });
            false
        }
    }
}

/// Every workspace clone directory under `repos_dir/<owner>/<repo>/`.
///
/// The layout is exactly three levels deep, so this is a bounded walk rather than a
/// recursive glob: descending into the clones themselves would traverse every
/// checked-out working tree in the cache.
///
/// **One unreadable directory costs that directory and no more.** The refusal is
/// caught at each level rather than around the whole walk, because a single guard
/// around all three meant the first unreadable owner ended the scan for every owner
/// after it — and since the owners are walked in sorted order, an unreadable `acme`
/// silently abandoned every unmigrated clone under `blooop` with nothing naming
/// them. What the caller does with this list is decide which directories to
/// *report* as left behind, so a short list is not a smaller job, it is a quieter
/// one.
fn clone_dirs(repos_dir: &Path, not_scanned: &mut Vec<NotScanned>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !repos_dir.is_dir() {
        return found;
    }
    for owner_dir in children(repos_dir, not_scanned) {
        for repo_dir in children(&owner_dir, not_scanned) {
            found.extend(
                children(&repo_dir, not_scanned)
                    .into_iter()
                    // The bare reference repository shares the parent of the clone
                    // directories and is never one of them. It is skipped by name
                    // because it is the layout's one fixed leaf.
                    .filter(|path| path.file_name().is_some_and(|name| name != BARE_DIR_NAME)),
            );
        }
    }
    found
}

/// The directories directly under `directory`, sorted; a refusal costs this one.
fn children(directory: &Path, not_scanned: &mut Vec<NotScanned>) -> Vec<PathBuf> {
    let listed = match std::fs::read_dir(directory) {
        Ok(listed) => listed,
        Err(error) => {
            not_scanned.push(NotScanned {
                path: directory.to_path_buf(),
                reason: error.to_string(),
            });
            return Vec::new();
        }
    };
    let mut found = Vec::new();
    for entry in listed {
        match entry {
            Ok(entry) => {
                if entry.path().is_dir() {
                    found.push(entry.path());
                }
            }
            Err(error) => not_scanned.push(NotScanned {
                path: directory.to_path_buf(),
                reason: error.to_string(),
            }),
        }
    }
    found.sort();
    found
}

/// Write one line per entry.
fn write_lines(path: &Path, lines: &[String]) -> Listing {
    let document: String = lines.iter().map(|line| format!("{line}\n")).collect();
    match std::fs::write(path, document) {
        Ok(()) => Listing::Written {
            path: path.to_path_buf(),
            lines: lines.len(),
        },
        Err(error) => Listing::CouldNotWrite {
            path: path.to_path_buf(),
            reason: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    //! `test/test_worktree_migration.py`, re-pinned.
    //!
    //! Every test builds its own cache under a temp directory. Nothing here may
    //! read or write the real cache: the migration renames directories that can
    //! hold uncommitted work.
    //!
    //! Two of Python's classes have no analogue here and are not gaps:
    //!
    //! - `TestWiring` is about `dl.py`'s clone-manager factory — where the
    //!   migration runs from, and that it runs once per process and never on
    //!   `--help`. That is the binary's wiring, and it lands with the binary (M5).
    //! - `TestOrphanedContainers::test_the_notice_costs_no_devpod_call` and
    //!   `test_no_container_is_deleted` patch `subprocess.run` to prove the
    //!   migration spawns nothing. Here that is a fact about the type:
    //!   [`migrate_cache`] takes a store and a directory, and there is no runner in
    //!   its signature for a process to be started through.
    //!
    //! The nine sentences `_announce` printed are assertions about
    //! [`MigrationReport`] and the two listing files instead, because the words are
    //! the binary's (#251) and the data is what the words interpolate.

    use std::collections::BTreeMap;

    use super::*;
    use crate::domain::metadata::{LEGACY_SCHEMA_VERSION, Notice};
    use crate::flows::repo_manager::tests::{refusing_reads, refusing_writes, run_git};
    use serde_json::{Value, json};

    /// The pre-#64 devpod id: repository and branch flattened, no identity suffix.
    fn old_workspace_id(repo: &str, branch: &str) -> String {
        format!("{repo}-{branch}")
            .replace('/', "-")
            .replace('_', "-")
    }

    /// The pre-#64 clone-directory leaf: the flattened branch name alone.
    fn old_leaf(branch: &str) -> String {
        branch.replace('/', "-")
    }

    /// The leaf the current scheme derives.
    fn new_leaf(owner: &str, repo: &str, branch: &str) -> String {
        WorkspaceId::new(owner, repo, branch)
            .expect("a safe triple")
            .value()
    }

    /// A cache written by an older devlaunch.
    struct LegacyCache {
        dir: tempfile::TempDir,
        metadata: PathBuf,
        repos_dir: PathBuf,
    }

    impl LegacyCache {
        fn document(&self) -> Value {
            serde_json::from_slice(&std::fs::read(&self.metadata).expect("the file")).expect("JSON")
        }

        fn version(&self) -> i64 {
            self.document()["version"].as_i64().expect("a version")
        }

        fn worktree(&self, key: &str) -> Value {
            self.document()["worktrees"][key].clone()
        }

        fn repo_root(&self, owner: &str, repo: &str) -> PathBuf {
            self.repos_dir.join(owner).join(repo)
        }

        /// The leaf names directly under a repository directory, sorted.
        fn leaves(&self, owner: &str, repo: &str) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(self.repo_root(owner, repo))
                .expect("a listing")
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }

        fn store(&self) -> MetadataStorage {
            MetadataStorage::open(&self.metadata).expect("a store").0
        }

        fn migrate(&self) -> Option<MigrationReport> {
            // A fresh store each time, as a fresh dl invocation would have.
            let mut storage = self.store();
            migrate_cache(&mut storage, &self.repos_dir).expect("the save")
        }
    }

    /// How a legacy cache is described: which branches each repository has.
    type Layout<'a> = &'a [(&'a str, &'a str, &'a [&'a str])];

    fn build_legacy_cache(layout: Layout<'_>) -> LegacyCache {
        build_legacy_cache_with(layout, Some(1), &[], true)
    }

    /// `version` of `None` writes no header at all — a pre-versioning file.
    /// `unrecorded` names `(owner, repo, leaf)` directories no record points at.
    /// `make_dirs` of false records the clones without creating them.
    fn build_legacy_cache_with(
        layout: Layout<'_>,
        version: Option<i64>,
        unrecorded: &[(&str, &str, &str)],
        make_dirs: bool,
    ) -> LegacyCache {
        let dir = tempfile::tempdir().expect("a temp dir");
        let devlaunch = dir.path().join("devlaunch");
        let repos_dir = devlaunch.join("repos");
        let mut repositories = BTreeMap::new();
        let mut worktrees = BTreeMap::new();

        for (owner, repo, branches) in layout {
            let repo_root = repos_dir.join(owner).join(repo);
            std::fs::create_dir_all(repo_root.join(BARE_DIR_NAME)).expect("the bare directory");
            repositories.insert(
                format!("{owner}/{repo}"),
                json!({
                    "owner": owner,
                    "repo": repo,
                    "remote_url": format!("git@github.com:{owner}/{repo}.git"),
                    "local_path": repo_root.join(BARE_DIR_NAME).display().to_string(),
                    "default_branch": "main",
                    "last_fetched": null,
                    "worktrees": branches,
                }),
            );
            for branch in *branches {
                let clone = repo_root.join(old_leaf(branch));
                if make_dirs {
                    std::fs::create_dir_all(clone.join(".git")).expect("the clone");
                }
                worktrees.insert(
                    format!("{owner}/{repo}/{branch}"),
                    worktree_entry(owner, repo, branch, &clone),
                );
            }
        }
        for (owner, repo, leaf) in unrecorded {
            std::fs::create_dir_all(repos_dir.join(owner).join(repo).join(leaf).join(".git"))
                .expect("an unrecorded clone");
        }

        let mut document = json!({ "repositories": repositories, "worktrees": worktrees });
        if let Some(version) = version {
            document["version"] = json!(version);
        }
        std::fs::create_dir_all(&devlaunch).expect("the cache directory");
        let metadata = devlaunch.join("metadata.json");
        std::fs::write(
            &metadata,
            serde_json::to_string_pretty(&document).expect("JSON"),
        )
        .expect("the fixture");
        LegacyCache {
            dir,
            metadata,
            repos_dir,
        }
    }

    fn worktree_entry(owner: &str, repo: &str, branch: &str, local_path: &Path) -> Value {
        json!({
            "owner": owner,
            "repo": repo,
            "branch": branch,
            "local_path": local_path.display().to_string(),
            "workspace_id": old_workspace_id(repo, branch),
            "created_at": "2024-01-01T10:00:00",
            "last_used": "2024-01-01T12:00:00",
            "devpod_workspace_id": null,
        })
    }

    /// One repository, three branches, no surprises.
    fn a_simple_cache() -> LegacyCache {
        build_legacy_cache(&[(
            "blooop",
            "devlaunch",
            &["main", "feature/auth", "aid_auto_2"],
        )])
    }

    /// The document as another dl operation would write it, knowing nothing about
    /// migrations.
    fn an_unrelated_save(cache: &LegacyCache) {
        cache.store().save().expect("saved");
    }

    // ============================================================== renaming

    #[test]
    fn every_recorded_clone_is_renamed_onto_the_derived_id() {
        let cache = a_simple_cache();

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(report.renamed.len(), 3);
        let mut expected = vec![
            BARE_DIR_NAME.to_owned(),
            new_leaf("blooop", "devlaunch", "main"),
            new_leaf("blooop", "devlaunch", "feature/auth"),
            new_leaf("blooop", "devlaunch", "aid_auto_2"),
        ];
        expected.sort();
        assert_eq!(cache.leaves("blooop", "devlaunch"), expected);
        // The bare reference repository shares the parent of the clone directories
        // and is never one of them.
        assert!(
            report
                .renamed
                .iter()
                .all(
                    |renamed| renamed.from.file_name().expect("a leaf") != BARE_DIR_NAME
                        && renamed.to.file_name().expect("a leaf") != BARE_DIR_NAME
                )
        );
        assert!(
            !report
                .unmigrated
                .contains(&cache.repo_root("blooop", "devlaunch").join(BARE_DIR_NAME))
        );
    }

    #[test]
    fn the_records_agree_with_the_filesystem_afterwards() {
        let cache = a_simple_cache();

        cache.migrate().expect("a migration ran");

        assert_eq!(cache.version(), SCHEMA_VERSION);
        for branch in ["main", "feature/auth", "aid_auto_2"] {
            let entry = cache.worktree(&format!("blooop/devlaunch/{branch}"));
            let derived = new_leaf("blooop", "devlaunch", branch);
            let expected = cache.repo_root("blooop", "devlaunch").join(&derived);
            assert_eq!(
                entry["local_path"].as_str(),
                Some(expected.display().to_string().as_str())
            );
            assert!(expected.is_dir(), "{branch}");
            // The record carries the derived id, because removal by id looks records
            // up by exactly the id dl derives from the spec.
            assert_eq!(entry["workspace_id"].as_str(), Some(derived.as_str()));
            // `devpod_workspace_id` is left alone: giving that field a second meaning
            // ("the orphaned old container") would make #55's modelling defect worse.
            assert_eq!(entry["devpod_workspace_id"], Value::Null);
        }
    }

    #[test]
    fn a_second_run_does_nothing_and_reports_nothing() {
        let cache = a_simple_cache();
        cache.migrate().expect("a migration ran");
        let before = cache.document();
        let leaves = cache.leaves("blooop", "devlaunch");

        assert!(
            cache.migrate().is_none(),
            "already current: a single integer comparison, no filesystem scan"
        );

        assert_eq!(cache.document(), before);
        assert_eq!(cache.leaves("blooop", "devlaunch"), leaves);
    }

    #[test]
    fn a_cache_already_on_the_new_scheme_still_gains_the_version() {
        let cache = build_legacy_cache(&[]);

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(report.renamed, Vec::new());
        assert_eq!(cache.version(), SCHEMA_VERSION);
    }

    #[test]
    fn a_file_with_no_version_header_is_migrated_and_a_newer_one_never_is() {
        // A pre-versioning file is version 1, not the current version: reading it as
        // current would skip the migration it needs.
        let legacy =
            build_legacy_cache_with(&[("blooop", "devlaunch", &["main"])], None, &[], true);
        assert_eq!(legacy.store().schema_version(), LEGACY_SCHEMA_VERSION);

        assert_eq!(legacy.migrate().expect("a migration ran").renamed.len(), 1);

        let newer = build_legacy_cache_with(
            &[("blooop", "devlaunch", &["main"])],
            Some(SCHEMA_VERSION + 1),
            &[],
            true,
        );

        assert!(newer.migrate().is_none());
        assert!(
            newer.repo_root("blooop", "devlaunch").join("main").is_dir(),
            "a cache a newer build wrote is left exactly as it is"
        );
    }

    #[test]
    fn the_recorded_path_is_the_source_and_not_a_recomputed_one() {
        // The record is the truth about where the clone is now, which is the same
        // principle that made removal work for old-scheme workspaces.
        let cache = build_legacy_cache(&[("blooop", "devlaunch", &["main"])]);
        let repo_root = cache.repo_root("blooop", "devlaunch");
        let odd = repo_root.join("not-the-branch-name");
        std::fs::rename(repo_root.join("main"), &odd).expect("moved somewhere unexpected");
        let mut document = cache.document();
        document["worktrees"]["blooop/devlaunch/main"]["local_path"] =
            json!(odd.display().to_string());
        std::fs::write(&cache.metadata, document.to_string()).expect("the fixture");

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(
            report.renamed,
            [Renamed {
                from: odd.clone(),
                to: repo_root.join(new_leaf("blooop", "devlaunch", "main")),
            }]
        );
        assert!(!odd.exists());
    }

    #[test]
    fn real_git_uncommitted_work_survives_the_rename() {
        // The reason the strategy is rename-not-orphan: a workspace is a `git clone`
        // whose `origin` points at the `.bare` path, and `.bare` does not move, so a
        // plain rename is lossless and the work that exists nowhere else survives.
        let cache =
            build_legacy_cache_with(&[("blooop", "devlaunch", &["main"])], Some(1), &[], false);
        let repo_root = cache.repo_root("blooop", "devlaunch");
        let bare = repo_root.join(BARE_DIR_NAME);
        run_git(
            cache.dir.path(),
            &["init", "--bare", "-b", "main", &bare.display().to_string()],
        );
        let clone = repo_root.join("main");
        run_git(
            cache.dir.path(),
            &[
                "clone",
                &bare.display().to_string(),
                &clone.display().to_string(),
            ],
        );
        std::fs::write(clone.join("committed.txt"), "committed\n").expect("a file");
        run_git(&clone, &["add", "committed.txt"]);
        run_git(&clone, &["commit", "-m", "first"]);
        run_git(&clone, &["push", "origin", "main"]);
        std::fs::write(clone.join("work-in-progress.txt"), "do not lose me\n").expect("a file");
        std::fs::write(clone.join("committed.txt"), "edited but not committed\n").expect("a file");

        cache.migrate().expect("a migration ran");

        let moved = repo_root.join(new_leaf("blooop", "devlaunch", "main"));
        assert!(!clone.exists());
        assert_eq!(
            std::fs::read_to_string(moved.join("work-in-progress.txt")).expect("the file"),
            "do not lose me\n"
        );
        assert_eq!(
            std::fs::read_to_string(moved.join("committed.txt")).expect("the file"),
            "edited but not committed\n"
        );
        let status = run_git(&moved, &["status", "--porcelain"]);
        assert!(status.contains(" M committed.txt"), "{status}");
        assert!(status.contains("?? work-in-progress.txt"), "{status}");
        // The remote still points at `.bare`, which did not move, so the clone is not
        // just present but still functional.
        assert!(
            run_git(&moved, &["log", "--oneline"])
                .trim()
                .ends_with("first")
        );
        assert_eq!(run_git(&moved, &["fetch", "origin"]), "");
    }

    // ============================ directories the migration will not touch

    #[test]
    fn a_destination_that_is_already_there_is_never_renamed_over() {
        // Either an interrupted earlier run already renamed this clone, or a
        // newer-scheme clone was created alongside the old one. The record follows the
        // canonically named directory, so a later `dl … rm` deletes the clone devpod
        // is actually using; the leftover source becomes a directory no record points
        // at and is reported as one.
        let cache = build_legacy_cache(&[("blooop", "devlaunch", &["main"])]);
        let repo_root = cache.repo_root("blooop", "devlaunch");
        let old = repo_root.join("main");
        let already = repo_root.join(new_leaf("blooop", "devlaunch", "main"));
        std::fs::create_dir_all(already.join(".git")).expect("the new-scheme clone");
        std::fs::write(already.join("marker.txt"), "new scheme clone\n").expect("a marker");
        std::fs::write(old.join("marker.txt"), "old scheme clone\n").expect("a marker");

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(report.renamed, Vec::new());
        assert!(old.is_dir());
        assert_eq!(
            std::fs::read_to_string(old.join("marker.txt")).expect("the file"),
            "old scheme clone\n"
        );
        assert_eq!(
            std::fs::read_to_string(already.join("marker.txt")).expect("the file"),
            "new scheme clone\n"
        );
        assert!(report.unmigrated.contains(&old));
        assert_eq!(
            cache.worktree("blooop/devlaunch/main")["local_path"].as_str(),
            Some(already.display().to_string().as_str())
        );
        let listing = cache
            .dir
            .path()
            .join("devlaunch")
            .join(UNMIGRATED_LIST_NAME);
        assert_eq!(
            report.unmigrated_listing,
            Listing::Written {
                path: listing.clone(),
                lines: 1
            }
        );
        assert!(
            std::fs::read_to_string(&listing)
                .expect("the listing")
                .contains(&old.display().to_string())
        );
    }

    #[test]
    fn directories_no_record_claims_are_left_where_they_are_and_listed() {
        let cache = build_legacy_cache_with(
            &[("blooop", "devlaunch", &["main"])],
            Some(1),
            &[
                ("blooop", "devlaunch", "orphan-dir"),
                ("blooop", "bencher", "w1"),
            ],
            true,
        );

        let report = cache.migrate().expect("a migration ran");

        let orphan = cache.repo_root("blooop", "devlaunch").join("orphan-dir");
        let stray = cache.repo_root("blooop", "bencher").join("w1");
        assert!(orphan.is_dir() && stray.is_dir());
        let mut unmigrated = report.unmigrated.clone();
        unmigrated.sort();
        assert_eq!(unmigrated, {
            let mut expected = vec![orphan.clone(), stray.clone()];
            expected.sort();
            expected
        });
        let listing = std::fs::read_to_string(
            cache
                .dir
                .path()
                .join("devlaunch")
                .join(UNMIGRATED_LIST_NAME),
        )
        .expect("the listing");
        assert!(listing.contains(&orphan.display().to_string()));
        assert!(listing.contains(&stray.display().to_string()));
    }

    #[test]
    fn no_listing_is_written_when_every_directory_migrated() {
        let cache = a_simple_cache();

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(report.unmigrated_listing, Listing::NothingToList);
        assert!(
            !cache
                .dir
                .path()
                .join("devlaunch")
                .join(UNMIGRATED_LIST_NAME)
                .exists()
        );
    }

    #[test]
    fn a_record_whose_directory_is_gone_is_repointed_rather_than_failed() {
        // Already stale before this run: the record outlived its directory. Not a
        // failure — repointing it at the derived path is what a fresh clone would use,
        // and the workspace's existence is read off the filesystem.
        let cache = build_legacy_cache(&[("blooop", "devlaunch", &["main", "w1"])]);
        let gone = cache.repo_root("blooop", "devlaunch").join("w1");
        std::fs::remove_dir_all(&gone).expect("the directory goes");

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(report.missing, [gone]);
        assert_eq!(report.renamed.len(), 1);
        assert_eq!(
            cache.worktree("blooop/devlaunch/w1")["local_path"].as_str(),
            Some(
                cache
                    .repo_root("blooop", "devlaunch")
                    .join(new_leaf("blooop", "devlaunch", "w1"))
                    .display()
                    .to_string()
                    .as_str()
            )
        );
    }

    #[test]
    fn a_record_whose_branch_is_not_a_usable_ref_is_left_exactly_as_it_is() {
        // The old derivation coerced unsafe refs instead of rejecting them, so a
        // stored branch is not necessarily a legal ref. No id can be derived, so there
        // is no name to rename to.
        let cache = build_legacy_cache(&[("blooop", "devlaunch", &["main"])]);
        let bad = cache.repo_root("blooop", "devlaunch").join("feature-auth");
        std::fs::create_dir_all(bad.join(".git")).expect("the clone");
        let mut document = cache.document();
        document["worktrees"]["blooop/devlaunch/feature auth"] =
            worktree_entry("blooop", "devlaunch", "feature auth", &bad);
        std::fs::write(&cache.metadata, document.to_string()).expect("the fixture");

        let report = cache.migrate().expect("a migration ran");

        assert!(bad.is_dir());
        assert_eq!(
            report.unusable,
            [UnusableRecord {
                path: bad.clone(),
                branch: "feature auth".to_owned(),
            }]
        );
        let entry = cache.worktree("blooop/devlaunch/feature auth");
        assert_eq!(
            entry["local_path"].as_str(),
            Some(bad.display().to_string().as_str())
        );
        assert_eq!(
            entry["workspace_id"].as_str(),
            Some(old_workspace_id("devlaunch", "feature auth").as_str()),
            "the record keeps its old id: there is no derived one to give it"
        );
    }

    #[test]
    fn a_branch_named_after_another_branchs_derived_id_is_refused() {
        // #55's `foo-bexoza` case, now needing an exact hash match. Adopting a clone
        // another record owns is how one workspace's `rm` deletes another's work,
        // which is the class of bug #9766 was — so this renames nothing and, unlike
        // every other outcome, does not repoint the record either.
        let squatter = new_leaf("blooop", "devlaunch", "main");
        let cache = build_legacy_cache(&[("blooop", "devlaunch", &["main", &squatter])]);
        let repo_root = cache.repo_root("blooop", "devlaunch");

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(
            report.blocked,
            [Blocked {
                from: repo_root.join("main"),
                to: repo_root.join(&squatter),
            }]
        );
        assert!(repo_root.join("main").is_dir());
        let main = cache.worktree("blooop/devlaunch/main");
        assert_eq!(
            main["local_path"].as_str(),
            Some(repo_root.join("main").display().to_string().as_str())
        );
        assert_eq!(
            main["workspace_id"].as_str(),
            Some(old_workspace_id("devlaunch", "main").as_str())
        );
        // The other record migrated normally, out of the way.
        assert_eq!(
            cache.worktree(&format!("blooop/devlaunch/{squatter}"))["local_path"].as_str(),
            Some(
                repo_root
                    .join(new_leaf("blooop", "devlaunch", &squatter))
                    .display()
                    .to_string()
                    .as_str()
            )
        );
    }

    // =========================================================== interruption

    #[test]
    fn a_crash_between_the_renames_and_the_save_is_resumable() {
        // What a crash after the first rename and before the single save leaves: one
        // directory on the new scheme, the file untouched at version 1. The next run
        // finds it as "destination present, source gone" and catches metadata up to
        // it.
        let cache = build_legacy_cache(&[("blooop", "devlaunch", &["main", "w1"])]);
        let repo_root = cache.repo_root("blooop", "devlaunch");
        std::fs::rename(
            repo_root.join("main"),
            repo_root.join(new_leaf("blooop", "devlaunch", "main")),
        )
        .expect("the interrupted rename");
        assert_eq!(cache.version(), 1);

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(
            report.renamed,
            [Renamed {
                from: repo_root.join("w1"),
                to: repo_root.join(new_leaf("blooop", "devlaunch", "w1")),
            }]
        );
        assert_eq!(report.unmigrated, Vec::<PathBuf>::new());
        assert_eq!(report.missing, Vec::<PathBuf>::new());
        assert_eq!(cache.version(), SCHEMA_VERSION);
        for branch in ["main", "w1"] {
            let expected = repo_root.join(new_leaf("blooop", "devlaunch", branch));
            assert_eq!(
                cache.worktree(&format!("blooop/devlaunch/{branch}"))["local_path"].as_str(),
                Some(expected.display().to_string().as_str())
            );
            assert!(expected.is_dir());
        }
    }

    #[test]
    fn the_header_and_the_paths_reach_disk_in_one_save() {
        // No intermediate write may claim the migration finished. Observed from inside
        // the edit, which is the only place the question can be asked: the file on
        // disk still says 1 while the records are being rewritten, and says 2 the
        // moment the single save lands.
        let cache = a_simple_cache();
        let mut storage = cache.store();
        let path = cache.metadata.clone();

        storage
            .commit_migration(|worktrees| {
                assert_eq!(
                    serde_json::from_slice::<Value>(&std::fs::read(&path).expect("the file"))
                        .expect("JSON")["version"],
                    json!(1),
                    "a write before the edit would have bumped the header already"
                );
                for record in worktrees.values_mut() {
                    record.workspace_id = "rewritten".to_owned();
                }
                SchemaHeader::Promote
            })
            .expect("saved");

        assert_eq!(cache.version(), SCHEMA_VERSION);
        assert_eq!(
            cache.worktree("blooop/devlaunch/main")["workspace_id"],
            json!("rewritten"),
            "both halves of the change, in one atomic replace"
        );
    }

    // ==================================================== orphaned containers

    #[test]
    fn the_ids_the_old_containers_carry_are_reported_and_listed_sorted() {
        let cache = a_simple_cache();

        let report = cache.migrate().expect("a migration ran");

        let mut expected: Vec<String> = ["main", "feature/auth", "aid_auto_2"]
            .iter()
            .map(|branch| old_workspace_id("devlaunch", branch))
            .collect();
        expected.sort();
        let mut reported = report.orphaned_ids.clone();
        reported.sort();
        assert_eq!(reported, expected);
        let listing = cache.dir.path().join("devlaunch").join(ORPHAN_LIST_NAME);
        assert_eq!(
            report.orphan_listing,
            Listing::Written {
                path: listing.clone(),
                lines: 3
            }
        );
        assert_eq!(
            std::fs::read_to_string(&listing)
                .expect("the listing")
                .lines()
                .collect::<Vec<_>>(),
            expected,
            "one id per line, sorted, so the cleanup command reads deterministically"
        );
    }

    #[test]
    fn a_record_already_carrying_the_derived_id_orphans_nothing() {
        let cache = build_legacy_cache(&[]);
        let repo_root = cache.repos_dir.join("blooop").join("devlaunch");
        let leaf = new_leaf("blooop", "devlaunch", "main");
        std::fs::create_dir_all(repo_root.join(BARE_DIR_NAME)).expect("the bare directory");
        std::fs::create_dir_all(repo_root.join(&leaf).join(".git")).expect("the clone");
        let mut document = cache.document();
        let mut entry = worktree_entry("blooop", "devlaunch", "main", &repo_root.join(&leaf));
        entry["workspace_id"] = json!(leaf);
        document["worktrees"]["blooop/devlaunch/main"] = entry;
        std::fs::write(&cache.metadata, document.to_string()).expect("the fixture");

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(report.orphaned_ids, Vec::<String>::new());
        assert_eq!(report.orphan_listing, Listing::NothingToList);
        assert!(
            !cache
                .dir
                .path()
                .join("devlaunch")
                .join(ORPHAN_LIST_NAME)
                .exists()
        );
    }

    #[test]
    fn a_listing_that_cannot_be_written_is_reported_rather_than_named() {
        // The listing turns "12 containers are orphaned" into a command the user can
        // paste. When it cannot be written the notice has to degrade to an instruction
        // rather than to a path that is not there — naming a file that does not exist
        // is worse than naming none, because the user runs the pasted command against
        // nothing. The refusal is a directory sitting where the file goes, so this
        // needs no permissions at all and holds on every filesystem.
        let cache = a_simple_cache();
        let in_the_way = cache.dir.path().join("devlaunch").join(ORPHAN_LIST_NAME);
        std::fs::create_dir(&in_the_way).expect("a directory where the file goes");

        let report = cache.migrate().expect("a migration ran");

        assert!(
            !report.orphaned_ids.is_empty(),
            "there was something to list"
        );
        match report.orphan_listing {
            Listing::CouldNotWrite { path, reason } => {
                assert_eq!(path, in_the_way);
                assert!(!reason.is_empty(), "the OS said something");
            }
            other => panic!("a listing that could not be written, got {other:?}"),
        }
    }

    // =========================================== what the filesystem refuses

    #[test]
    fn a_rename_the_filesystem_refuses_costs_that_directory_and_no_more() {
        // Nothing exotic: a cache on a read-only mount, a directory whose permissions
        // someone tightened, a disk with nothing left on it. The migration is a
        // whole-cache operation running before the command the user typed, so the
        // standard it is held to is that one refusal costs the run one directory,
        // never the run.
        let cache = a_simple_cache();
        let repo_root = cache.repo_root("blooop", "devlaunch");
        let before = cache.leaves("blooop", "devlaunch");
        let Some(_denied) = refusing_writes(&repo_root) else {
            return; // this filesystem does not enforce directory modes
        };

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(
            report.failed.len(),
            3,
            "every rename in the locked directory was refused"
        );
        assert!(report.renamed.is_empty());
        for refused in &report.failed {
            assert_eq!(refused.from.parent(), Some(repo_root.as_path()));
            assert_eq!(refused.to.parent(), Some(repo_root.as_path()));
            assert!(!refused.reason.is_empty());
        }
        // The directories are still where they were, which is the whole point: a
        // clone that could not be renamed holds work, and the migration would rather
        // leave it under a name nothing looks for than lose track of it.
        assert_eq!(cache.leaves("blooop", "devlaunch"), before);
        // And every record still points at a directory that is really there. A record
        // repointed at a name the rename did not produce would send the next
        // `dl … rm` at a path with nothing in it.
        for branch in ["main", "feature/auth", "aid_auto_2"] {
            let recorded = cache.worktree(&format!("blooop/devlaunch/{branch}"))["local_path"]
                .as_str()
                .expect("a path")
                .to_owned();
            assert!(Path::new(&recorded).is_dir(), "{branch}: {recorded}");
        }
    }

    #[test]
    fn a_refused_rename_leaves_the_header_behind_until_the_retry_succeeds() {
        // The consequence the test above stops one line short of. A refused rename is
        // not a crash, but it has to be survivable the same way: the header is what
        // the next run's version comparison reads, so a run that left a directory
        // under its old name must leave the header at 1 too. Advancing it would strand
        // those records on their pre-#64 id forever — removal by id matches on exactly
        // the id dl derives today, so `dl acme/widgets@main rm` could never find them
        // again (#180).
        //
        // Two repositories, one of them locked: the refusal has to be *partial* to
        // show that the save still happens. The successful renames are recorded
        // immediately — declining the save entirely would lose them.
        let cache = build_legacy_cache(&[
            ("blooop", "devlaunch", &["main"]),
            ("acme", "widgets", &["main", "release"]),
        ]);
        let locked = cache.repo_root("acme", "widgets");
        let denied = refusing_writes(&locked);
        if denied.is_none() {
            return; // this filesystem does not enforce directory modes
        }

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(report.renamed.len(), 1);
        assert_eq!(report.failed.len(), 2);
        let mut refused: Vec<PathBuf> = report.failed.iter().map(|f| f.from.clone()).collect();
        refused.sort();
        assert_eq!(
            cache.version(),
            1,
            "the header may not claim more than the filesystem has done"
        );
        // The half that worked is on disk, not just in memory.
        let done = cache.worktree("blooop/devlaunch/main");
        let moved =
            cache
                .repo_root("blooop", "devlaunch")
                .join(new_leaf("blooop", "devlaunch", "main"));
        assert_eq!(
            done["local_path"].as_str(),
            Some(moved.display().to_string().as_str())
        );
        assert!(moved.is_dir());
        assert_eq!(
            done["workspace_id"].as_str(),
            Some(new_leaf("blooop", "devlaunch", "main").as_str())
        );
        // The half that was refused still points at the directory that is really
        // there, under its old name and its old id.
        for branch in ["main", "release"] {
            let left = cache.worktree(&format!("acme/widgets/{branch}"));
            assert!(Path::new(left["local_path"].as_str().expect("a path")).is_dir());
            assert_eq!(
                left["workspace_id"].as_str(),
                Some(old_workspace_id("widgets", branch).as_str())
            );
        }

        // A second run picks the cache back up and retries *exactly* the refused set:
        // the already-renamed clone is the documented "destination present, source
        // gone" resume, which is caught up to without a second rename.
        let second = cache
            .migrate()
            .expect("the header at 1 is what lets the next run in");
        assert_eq!(second.renamed, Vec::new());
        let mut refused_again: Vec<PathBuf> =
            second.failed.iter().map(|f| f.from.clone()).collect();
        refused_again.sort();
        assert_eq!(
            refused_again, refused,
            "the report re-reports until it is fixed by hand"
        );

        // And when the refusal lifts, the retry completes and only then does the
        // header advance — the records were recoverable the whole time.
        drop(denied);
        let third = cache.migrate().expect("a migration ran");
        assert_eq!(third.renamed.len(), 2);
        assert_eq!(third.failed, Vec::new());
        assert_eq!(cache.version(), SCHEMA_VERSION);
        for (owner, repo, branch) in [
            ("blooop", "devlaunch", "main"),
            ("acme", "widgets", "main"),
            ("acme", "widgets", "release"),
        ] {
            let entry = cache.worktree(&format!("{owner}/{repo}/{branch}"));
            assert_eq!(
                entry["workspace_id"].as_str(),
                Some(new_leaf(owner, repo, branch).as_str())
            );
            assert!(Path::new(entry["local_path"].as_str().expect("a path")).is_dir());
        }
    }

    #[test]
    fn an_unrelated_save_between_runs_does_not_advance_the_header() {
        // The migration is not the only thing that writes `metadata.json`: opening a
        // workspace or reconciling saves too, through a store loaded fresh long after
        // the migration ran. If a save stamped the current version, the very next such
        // write would re-strand the records a refused rename had deliberately left
        // behind — and gating only the migration's own save would miss it entirely.
        let cache = a_simple_cache();
        let Some(_denied) = refusing_writes(&cache.repo_root("blooop", "devlaunch")) else {
            return;
        };
        assert!(!cache.migrate().expect("a migration ran").failed.is_empty());

        an_unrelated_save(&cache);

        assert_eq!(cache.version(), 1);
        assert!(cache.migrate().is_some(), "still reachable");
    }

    #[test]
    fn a_corner_of_the_cache_that_cannot_be_scanned_costs_only_that_corner() {
        // The scan for record-less directories runs after the renames and over the
        // *whole* cache, so it reaches owners this run has no business with. One
        // unreadable directory there must not cost the migration the work it has
        // already done — the renames are on disk by this point and the header has not
        // been written yet.
        //
        // The unreadable owner is named to sort **before** the real one, and that is
        // the entire point of the name: owners are walked in sorted order, and while
        // the whole three-level walk sat under one guard the first refusal ended the
        // scan for every owner after it.
        let cache = build_legacy_cache_with(
            &[("blooop", "devlaunch", &["main"])],
            Some(1),
            &[("blooop", "devlaunch", "stray-clone")],
            true,
        );
        let unreadable = cache.repos_dir.join("aaa-corp");
        std::fs::create_dir_all(&unreadable).expect("the corner");
        let Some(_denied) = refusing_reads(&unreadable) else {
            return;
        };

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(report.renamed.len(), 1, "the readable half migrated");
        assert!(
            cache
                .repo_root("blooop", "devlaunch")
                .join(new_leaf("blooop", "devlaunch", "main"))
                .is_dir()
        );
        assert_eq!(cache.version(), SCHEMA_VERSION);
        // The scan carried on past the refusal and still found what it was for.
        assert_eq!(
            report
                .unmigrated
                .iter()
                .map(|path| path
                    .file_name()
                    .expect("a leaf")
                    .to_string_lossy()
                    .into_owned())
                .collect::<Vec<_>>(),
            ["stray-clone"]
        );
        assert_eq!(
            report
                .not_scanned
                .iter()
                .map(|refused| refused.path.clone())
                .collect::<Vec<_>>(),
            [unreadable],
            "and named the corner it could not read, so nothing is silently dropped"
        );
    }

    // ======================================================= a realistic cache

    #[test]
    fn a_cache_shaped_like_the_one_this_was_written_for_migrates_end_to_end() {
        let layout: Layout<'_> = &[
            (
                "blooop",
                "bencher",
                &[
                    "main",
                    "w1",
                    "w2",
                    "w3",
                    "asdf1",
                    "asdf2",
                    "test-dotfix",
                    "rerun30",
                    "update",
                    "tmp",
                ],
            ),
            (
                "blooop",
                "devlaunch",
                &["main", "aid", "aid_auto_2", "bugfix1", "slow_install"],
            ),
            (
                "blooop",
                "python_template",
                &["main", "ws1", "ws3", "ws99", "prek", "wsnew"],
            ),
            ("blooop", "wayfinder", &["main", "format", "devlaunch"]),
            ("blooop", "rockerc", &["main", "nb3"]),
        ];
        let unrecorded = [
            ("blooop", "bencher", "ws9"),
            ("blooop", "wayfinder", "leftover"),
        ];
        let cache = build_legacy_cache_with(layout, Some(1), &unrecorded, true);
        let expected: usize = layout.iter().map(|(_, _, branches)| branches.len()).sum();

        let report = cache.migrate().expect("a migration ran");

        assert_eq!(report.renamed.len(), expected);
        assert_eq!(report.orphaned_ids.len(), expected);
        assert_eq!(report.unmigrated.len(), unrecorded.len());
        assert_eq!(report.missing, Vec::<PathBuf>::new());
        assert_eq!(report.unusable, Vec::new());
        assert_eq!(report.blocked, Vec::new());
        assert_eq!(report.failed, Vec::new());
        assert_eq!(report.not_scanned, Vec::new());

        // Every recorded directory is where metadata says it is, and every leaf is
        // globally unique now rather than unique only within its parent.
        assert_eq!(cache.version(), SCHEMA_VERSION);
        let document = cache.document();
        let mut leaves = Vec::new();
        for (key, entry) in document["worktrees"].as_object().expect("worktrees") {
            let (owner, rest) = key.split_once('/').expect("owner/repo/branch");
            let (repo, branch) = rest.split_once('/').expect("repo/branch");
            let path = PathBuf::from(entry["local_path"].as_str().expect("a path"));
            assert!(path.is_dir(), "{key}");
            assert_eq!(
                path,
                cache
                    .repo_root(owner, repo)
                    .join(new_leaf(owner, repo, branch))
            );
            assert_eq!(
                entry["workspace_id"].as_str(),
                path.file_name().expect("a leaf").to_str()
            );
            leaves.push(path.file_name().expect("a leaf").to_owned());
        }
        let unique: std::collections::HashSet<_> = leaves.iter().collect();
        assert_eq!(unique.len(), leaves.len());
        assert_eq!(leaves.len(), expected);

        for (owner, repo, _) in layout {
            assert!(cache.repo_root(owner, repo).join(BARE_DIR_NAME).is_dir());
        }
        for (owner, repo, leaf) in unrecorded {
            assert!(cache.repo_root(owner, repo).join(leaf).is_dir());
        }

        // Second run: nothing to do.
        assert!(cache.migrate().is_none());
    }

    #[test]
    fn a_damaged_metadata_file_is_still_migrated_around_its_damage() {
        // The store's own recovery and this migration meet here: an entry that cannot
        // be rebuilt is skipped by the loader, so the migration never sees it — and
        // the records beside it are migrated normally rather than the whole run being
        // abandoned over one bad entry.
        let cache = build_legacy_cache(&[("blooop", "devlaunch", &["main"])]);
        let mut document = cache.document();
        document["worktrees"]["blooop/devlaunch/broken"] = json!({ "owner": "blooop" });
        std::fs::write(&cache.metadata, document.to_string()).expect("the fixture");
        let (mut storage, notices) = MetadataStorage::open(&cache.metadata).expect("a store");
        assert!(
            notices
                .iter()
                .any(|notice| matches!(notice, Notice::EntryUnusable { .. })),
            "{notices:?}"
        );

        let report = migrate_cache(&mut storage, &cache.repos_dir)
            .expect("the save")
            .expect("a migration ran");

        assert_eq!(report.renamed.len(), 1);
        assert_eq!(cache.version(), SCHEMA_VERSION);
        assert!(cache.document()["worktrees"]["blooop/devlaunch/broken"].is_null());
    }
}
