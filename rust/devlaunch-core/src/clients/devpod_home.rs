//! devpod's own records on disk, as one layout rather than a path passed around.
//!
//! [`super::devpod`] is the seam for devpod-the-*command*. This is the seam for
//! devpod-the-*filesystem*: the directory devpod keeps its state in, and the
//! convention it keeps it under —
//!
//! ```text
//! <devpod home>/contexts/<context>/workspaces/<id>/workspace.json
//! <devpod home>/contexts/<context>/workspaces/<id>/workspace_result.json
//! ```
//!
//! That convention is devpod's, not devlaunch's, and it is knowledge the way a
//! response format is knowledge: it can move under a devpod upgrade, and every
//! place that spelled it would then be wrong on its own. So it is spelled here,
//! and `devlaunch-core/tests/devpod_layout.rs` holds the rest of the crate to
//! that.
//!
//! # Reading it is the point; two writes are the exception
//!
//! Everything here but `DevpodHome::repoint` and [`remove_busy_marker`] reads.
//! Both writes are argued for where they are declared, and between them they are
//! what makes this an adapter rather than a path helper: the module owns devpod's
//! files on the way out as well as the way in, so the flows above it never open
//! one — and, for the removal, never have to hold the right one of the two files
//! devpod calls `workspace.lock`.
//!
//! # The home is taken, not resolved
//!
//! [`DevpodHome::at`] takes the directory; [`DevpodHome::locate`] is the one place
//! that reads the environment for it, and the binary calls it once and hands the
//! answer down — the same convention every other environment answer in dl follows
//! (`dl/src/commands.rs`, at the `workspace_delete` call). A flow that resolved its
//! own could disagree with the one the launch already resolved.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::osext::system_words;

/// The directory devpod keeps its own records in.
///
/// A type rather than a `PathBuf` because the layout underneath it is the whole of
/// what this module knows: a bare path can be joined by anybody, and eight places
/// in this crate did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevpodHome {
    root: PathBuf,
}

impl DevpodHome {
    /// devpod's home at this directory, whether or not anything is there yet.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Where this machine's devpod keeps its records, or nothing on a machine with
    /// no home directory to put them under.
    ///
    /// Honours `DEVPOD_HOME` for the same reason the rest of dl does: it is what
    /// scopes devpod, and the test suite sets it. The thin half of the pair — it
    /// reads the process environment and nothing else, so `Self::located` is
    /// where the cases are pinned.
    pub fn locate() -> Option<Self> {
        Self::located(std::env::var_os("DEVPOD_HOME"), crate::osext::home_dir)
    }

    /// The pure core of [`Self::locate`]: `configured` is what `DEVPOD_HOME` holds
    /// (or its absence), `home_dir` is consulted only when it holds nothing usable.
    ///
    /// An *empty* `DEVPOD_HOME` falls through to the home directory rather than
    /// naming the current directory, which is what an empty path would mean — a
    /// variable someone cleared must not silently point devpod's records at
    /// wherever dl was run from.
    fn located(
        configured: Option<OsString>,
        home_dir: impl FnOnce() -> Option<PathBuf>,
    ) -> Option<Self> {
        match configured {
            Some(home) if !home.is_empty() => Some(Self::at(home)),
            _ => home_dir().map(|home| Self::at(home.join(".devpod"))),
        }
    }

    /// The directory itself, for the callers that only have to name it.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// devpod's config file, which holds every context and its options.
    pub(crate) fn config(&self) -> PathBuf {
        self.root.join("config.yaml")
    }

    /// The directory holding every context's records.
    fn contexts(&self) -> PathBuf {
        self.root.join("contexts")
    }

    /// One workspace's directory under one context — the address both record files
    /// hang off, and the only place the layout is spelled.
    fn workspace_dir(&self, context: &str, workspace_id: &str) -> PathBuf {
        self.contexts()
            .join(context)
            .join("workspaces")
            .join(workspace_id)
    }

    /// devpod's own record for one workspace.
    pub(crate) fn record(&self, context: &str, workspace_id: &str) -> PathBuf {
        self.workspace_dir(context, workspace_id)
            .join("workspace.json")
    }

    /// devpod's agent-side *busy marker* for one workspace.
    ///
    /// **The second file called `workspace.lock`, and the one that goes stale.**
    /// It is a plain marker, not a lock: devpod's agent creates it on the way into
    /// an `up` and removes it from a `defer` on the way out, and its daemon reads
    /// it to know a build is running. A `defer` does not run under SIGKILL, so a
    /// hard-killed `up` leaves this behind, which is what makes it worth sweeping.
    ///
    /// The *other* `workspace.lock` — the `flock` under `contexts/<ctx>/locks` —
    /// is deliberately absent from this module and must stay absent: the kernel
    /// releases it when its holder dies, and unlinking it is the hazard
    /// [`crate::domain::locks`] argues against at length. Naming only the safe one
    /// here is what keeps a caller from reaching for whichever it remembers.
    fn busy_marker(&self, context: &str, workspace_id: &str) -> PathBuf {
        self.root
            .join("agent")
            .join("contexts")
            .join(context)
            .join("workspaces")
            .join(workspace_id)
            .join("workspace.lock")
    }

    /// devpod's record of a *completed* create for one workspace.
    ///
    /// devpod writes this beside [`Self::record`] on its way out of a successful
    /// `up`, and it is where the container's remote user lives
    /// (`.MergedConfig.remoteUser`). A create that died in its lifecycle hooks
    /// leaves the record and no result — which is also why `devpod ssh` into one
    /// lands as root: there is no recorded user for it to become.
    pub(crate) fn result(&self, context: &str, workspace_id: &str) -> PathBuf {
        self.workspace_dir(context, workspace_id)
            .join("workspace_result.json")
    }

    /// The one context whose records name this workspace, or nothing when the
    /// question has no single answer.
    ///
    /// Nothing for all four ways of not having one: a `contexts` directory that
    /// would not read, no context holding the id, two contexts holding it, and a
    /// context whose name is not UTF-8 (skipped, because a path this cannot spell
    /// is one devpod's own id lookup cannot have written).
    ///
    /// Split out because [`create_record`] and [`sole_workspace_result`] both have
    /// to walk the contexts *the same way* and read the same ambiguity out of them:
    /// the two answer different questions about one workspace, and a second copy of
    /// this loop is how they would come to disagree about which context that
    /// workspace is in.
    fn sole_context_holding(&self, workspace_id: &str) -> Option<String> {
        let contexts = std::fs::read_dir(self.contexts()).ok()?;
        let mut sole: Option<String> = None;
        for context in contexts.flatten() {
            let Some(name) = context.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !self.record(&name, workspace_id).is_file() {
                continue;
            }
            if sole.is_some() {
                return None;
            }
            sole = Some(name);
        }
        sole
    }

    /// Rewrite one devpod workspace record's source folder.
    ///
    /// **devpod's own file, written directly, and that is a decision rather than a
    /// convenience.** devpod v0.26.1 has no subcommand that changes an existing
    /// workspace's source: the surface is build, delete, list, logs, ssh, status,
    /// stop and up, and the only one that sets a source is a create, which needs a
    /// container daemon and would destroy the very record being repaired. So the
    /// choice is between one field of one JSON file and not repairing anything.
    /// Writing it from *inside* the module that already owns devpod's layout is
    /// what keeps that a seam rather than a leak.
    ///
    /// Only `source.localFolder` is touched, and the file is rewritten from what
    /// devpod itself last wrote, so every key devpod knows about and dl does not —
    /// `uid`, provider options, timestamps — survives untouched. Written through a
    /// temporary file in the same directory and renamed over the original, so a
    /// failure partway leaves devpod's record whole rather than truncated.
    pub(crate) fn repoint(
        &self,
        context: &str,
        workspace_id: &str,
        source: &Path,
    ) -> Result<(), RepointFailure> {
        let path = self.record(context, workspace_id);
        let text = std::fs::read_to_string(&path).map_err(|error| RepointFailure::Unreadable {
            path: path.clone(),
            reason: system_words(&error),
        })?;
        let mut record: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| RepointFailure::NotJson {
                path: path.clone(),
                reason: error.to_string(),
            })?;
        let Some(recorded) = record
            .as_object_mut()
            .and_then(|record| record.get_mut("source"))
            .and_then(serde_json::Value::as_object_mut)
        else {
            return Err(RepointFailure::NotADevpodRecord { path });
        };
        recorded.insert(
            "localFolder".to_owned(),
            serde_json::Value::String(source.display().to_string()),
        );
        // Two-space indentation, as devpod and Python's `json.dumps(…, indent=2)`
        // both write it: a file two tools take turns rewriting should not churn its
        // shape.
        let rewritten =
            serde_json::to_string_pretty(&record).map_err(|error| RepointFailure::Unwritable {
                path: path.clone(),
                reason: error.to_string(),
            })?;
        let temp = path.with_extension("dl-tmp");
        if let Err(error) = std::fs::write(&temp, rewritten) {
            let _ = std::fs::remove_file(&temp);
            return Err(RepointFailure::Unwritable {
                path,
                reason: system_words(&error),
            });
        }
        if let Err(error) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(RepointFailure::Unwritable {
                path,
                reason: system_words(&error),
            });
        }
        Ok(())
    }
}

/// Whether devpod finished creating a workspace, as far as its own records say.
///
/// Three arms rather than a bool, for the reason [`crate::domain::workspace_state`]
/// needed three: "no evidence it finished" and "no evidence either way" are
/// different facts, and only the first is a reason to act. A caller that acted on
/// the second would rebuild every workspace on a host whose devpod home it cannot
/// read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CreateRecord {
    /// devpod wrote its create result, so the workspace was set up.
    Completed,
    /// devpod holds a record for this workspace and no result beside it: an `up`
    /// started and did not finish.
    NeverCompleted,
    /// Nothing here can answer — no devpod home, no record under any context, a
    /// directory that would not read, or one id in two contexts.
    Unknown,
}

/// What devpod's own records say about whether this workspace's create finished.
///
/// Takes an `Option` rather than being a method on [`DevpodHome`] because "this
/// machine has no devpod home" is one of the answers. Absorbing it here is what
/// keeps four call sites from each deciding again what a missing home means, and
/// they would not all decide it the same way.
///
/// Every context is searched rather than `default` being assumed: ids are unique
/// per context, [`crate::flows::launch::Placement`] carries no context, and asking
/// devpod for one would put a second round trip on the warm attach path this
/// exists to guard. Two contexts holding the same id answer [`CreateRecord::Unknown`]
/// rather than picking one — the ambiguity is the answer.
pub(crate) fn create_record(devpod_home: Option<&DevpodHome>, workspace_id: &str) -> CreateRecord {
    let Some(home) = devpod_home else {
        return CreateRecord::Unknown;
    };
    let Some(context) = home.sole_context_holding(workspace_id) else {
        return CreateRecord::Unknown;
    };
    if home.result(&context, workspace_id).is_file() {
        CreateRecord::Completed
    } else {
        CreateRecord::NeverCompleted
    }
}

/// The one `workspace_result.json` this workspace has, if it has exactly one.
///
/// devpod rewrites that file on its way out of every *completed* `up`, whoever ran
/// the `up` — `dl`, VS Code, a hand-typed `devpod up` — which is what makes its
/// mtime a usable "has this container been rebuilt since?" anchor for
/// [`crate::flows::provision::verdict_cache`]. The record beside it is not: devpod
/// writes `workspace.json` on the way *in* and leaves it alone afterwards, so a
/// container recreated ten times still carries the first create's timestamps.
///
/// Nothing rather than a guess in every ambiguous case, because the caller's only
/// use for the answer is to *skip* work: no devpod home, no record, two contexts
/// holding the id, or a record with no result beside it (an `up` that died in its
/// hooks) all mean the same thing to it — there is no file whose mtime can be
/// trusted to move when this container is rebuilt, so trust nothing.
pub(crate) fn sole_workspace_result(
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
) -> Option<PathBuf> {
    let home = devpod_home?;
    let context = home.sole_context_holding(workspace_id)?;
    let result = home.result(&context, workspace_id);
    result.is_file().then_some(result)
}

/// Where this workspace's busy marker would be, if the context is unambiguous.
///
/// Nothing for every ambiguity [`sole_workspace_result`] answers nothing to, and
/// the reason is stronger here: this is the path [`remove_busy_marker`] unlinks,
/// and the wrong context's marker belongs to a workspace nobody asked about.
pub(crate) fn sole_busy_marker(
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
) -> Option<PathBuf> {
    let home = devpod_home?;
    let context = home.sole_context_holding(workspace_id)?;
    Some(home.busy_marker(&context, workspace_id))
}

/// What removing devpod's busy marker for one workspace came to.
///
/// Four ways and not a `Result`, because two of them are neither a success nor a
/// failure: a marker that was never left behind is the *good* ending, and a host
/// whose records cannot address one was never an attempt. Each is a different
/// thing for the flow above to say, and only one of them names a file that moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MarkerRemoval {
    /// It was there, and it is gone.
    Removed(PathBuf),
    /// Nothing was there to remove.
    AlreadyGone,
    /// There, and it would not go. `reason` is the OS's own words.
    Refused { path: PathBuf, reason: String },
    /// Nowhere to look: no devpod home on this host, or no single context whose
    /// records name this workspace.
    Unlocatable,
}

/// Unlink devpod's busy marker for one workspace, wherever its records put it.
///
/// **The one place in devlaunch that removes a file of devpod's**, and it is here
/// for [`DevpodHome::repoint`]'s reason: this module owns devpod's layout on the
/// way out as well as the way in. The removal in particular has to be here,
/// because devpod has *two* files called `workspace.lock` and the other one is
/// the flock — which must never be unlinked ([`crate::domain::locks`]). A
/// `remove_file` written in a flow is one edit away from being pointed at
/// whichever of the two its author remembered; one written against the address
/// this module hands out cannot be.
///
/// Whether the marker is *stale* is not a question this can answer: that is a
/// fact about the host's process table, and the caller has already established
/// it before asking.
pub(crate) fn remove_busy_marker(
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
) -> MarkerRemoval {
    let Some(path) = sole_busy_marker(devpod_home, workspace_id) else {
        return MarkerRemoval::Unlocatable;
    };
    match std::fs::remove_file(&path) {
        Ok(()) => MarkerRemoval::Removed(path),
        // Already gone is the good ending, not a failure: a `devpod up` that took
        // SIGTERM ran the `defer` that removes it.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => MarkerRemoval::AlreadyGone,
        Err(error) => MarkerRemoval::Refused {
            path,
            reason: system_words(&error),
        },
    }
}

/// Why one devpod record could not be re-pointed.
///
/// Unreadable and not-JSON are two arms where Python's one `except` clause caught
/// both: an `OSError` and a decode error read the same to its f-string but not to
/// a caller, and one `reason: String` could not say which had happened. Both
/// render through the same sentence, so the split changes no output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepointFailure {
    /// devpod's record's bytes could not be read; `reason` is the OS's words.
    Unreadable { path: PathBuf, reason: String },
    /// The bytes are not JSON; `reason` is the parser's words.
    NotJson { path: PathBuf, reason: String },
    /// Not the shape this repair understands. Refusing is the whole of the
    /// response: a source key dl cannot read is one it cannot safely replace.
    NotADevpodRecord { path: PathBuf },
    /// The rewritten record could not be put in place. devpod's own file is left
    /// whole: the write goes to a sibling temp file and is renamed over it.
    Unwritable { path: PathBuf, reason: String },
}

/// A devpod home under a temporary directory, holding whatever these workspaces
/// were left holding. `Some(())` writes the create result beside the record;
/// `None` leaves the record alone, which is what an aborted `up` leaves behind.
///
/// Lives beside the layout it builds rather than in the tests that use it — three
/// modules' tests want one, and the layout is spelled here or it is spelled three
/// more times. A plain `#[cfg(test)]` item at module scope, not an export of a test
/// module: `flows::lifecycle::tests` was `pub(crate)` for exactly this fixture,
/// which put a test module into the crate's internal surface because this module
/// did not exist.
#[cfg(test)]
pub(crate) fn devpod_home_with(entries: &[(&str, &str, Option<()>)]) -> ScratchHome {
    let dir = tempfile::tempdir().expect("a scratch devpod home");
    let home = DevpodHome::at(dir.path());
    for (context, workspace_id, result) in entries {
        let record = home.record(context, workspace_id);
        std::fs::create_dir_all(record.parent().expect("a record directory"))
            .expect("a record directory");
        std::fs::write(record, "{}").expect("a record");
        if result.is_some() {
            std::fs::write(home.result(context, workspace_id), "{}").expect("a result");
        }
    }
    ScratchHome { _dir: dir, home }
}

/// The `flock` devpod blocks on, created empty, for the one test that has to
/// assert it is still standing afterwards.
///
/// Not part of this adapter's real surface, and it must not become one: nothing
/// in dl opens this file and nothing may unlink it — the kernel releases it when
/// its holder dies, which is the entire reason `dl <ws> kill` kills the holder
/// instead of tidying the file. It is spelled here for the layout guard's reason,
/// so that the test asserting it survived does not become the ninth copy of
/// devpod's directory convention.
#[cfg(test)]
pub(crate) fn untouchable_flock(home: &DevpodHome, context: &str, workspace_id: &str) -> PathBuf {
    let path = home
        .contexts()
        .join(context)
        .join("locks")
        .join(format!("{workspace_id}.workspace.lock"));
    std::fs::create_dir_all(path.parent().expect("a locks directory")).expect("a locks directory");
    std::fs::write(&path, "").expect("a lock file");
    path
}

/// A [`DevpodHome`] and the temporary directory keeping it alive.
///
/// Derefs to the home, so a test that holds one reads as if it held the home
/// itself; the directory is a field only because dropping it would take the
/// records away.
#[cfg(test)]
pub(crate) struct ScratchHome {
    _dir: tempfile::TempDir,
    home: DevpodHome,
}

#[cfg(test)]
impl std::ops::Deref for ScratchHome {
    type Target = DevpodHome;

    fn deref(&self) -> &DevpodHome {
        &self.home
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The second file called `workspace.lock`, and the whole reason both are
    /// named here rather than at the one call site that removes one of them: the
    /// flock under `contexts/<ctx>/locks` must never be unlinked, this one is
    /// exactly what wants sweeping, and two paths a caller has to keep straight
    /// are two paths it will one day mix up.
    #[test]
    fn the_busy_marker_is_the_agents_copy_and_not_the_flock() {
        let home = devpod_home_with(&[("default", "myws", Some(()))]);

        assert_eq!(
            sole_busy_marker(Some(&home), "myws"),
            Some(
                home.path()
                    .join("agent/contexts/default/workspaces/myws/workspace.lock")
            )
        );
    }

    /// The three endings a removal has, and the flow above says something
    /// different about each: one file moved, one was already gone because the
    /// `defer` that removes it did run, and one is still there and has to carry
    /// the OS's own words, because that is the only one anybody can act on.
    ///
    /// The refusal is a marker that is a *directory*, which `unlink` refuses with
    /// EISDIR whoever is running the test — a permission bit would prove nothing
    /// on a CI runner that is root.
    #[test]
    fn removing_the_busy_marker_says_which_of_the_three_endings_it_was() {
        let home = devpod_home_with(&[("default", "myws", Some(()))]);
        let marker = home.busy_marker("default", "myws");
        std::fs::create_dir_all(marker.parent().expect("a marker directory"))
            .expect("a marker directory");

        assert_eq!(
            remove_busy_marker(Some(&home), "myws"),
            MarkerRemoval::AlreadyGone
        );

        std::fs::write(&marker, "").expect("a marker");
        assert_eq!(
            remove_busy_marker(Some(&home), "myws"),
            MarkerRemoval::Removed(marker.clone())
        );
        assert!(!marker.exists());

        std::fs::create_dir(&marker).expect("a marker that will not unlink");
        match remove_busy_marker(Some(&home), "myws") {
            MarkerRemoval::Refused { path, reason } => {
                assert_eq!(path, marker);
                assert!(
                    !reason.is_empty(),
                    "the OS's own words, not an empty string"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// A host whose records cannot address a marker is not a removal that failed:
    /// nobody looked, and there is nothing to report.
    #[test]
    fn a_workspace_with_no_addressable_marker_is_not_a_failed_removal() {
        assert_eq!(remove_busy_marker(None, "myws"), MarkerRemoval::Unlocatable);
    }

    /// The same ambiguities `sole_workspace_result` answers nothing to, for a
    /// stronger reason: this address is one [`remove_busy_marker`] unlinks, and
    /// the wrong context's marker belongs to a workspace nobody asked about.
    #[test]
    fn a_workspace_no_single_context_holds_has_no_marker_to_name() {
        let home = devpod_home_with(&[("default", "myws", Some(())), ("work", "myws", None)]);

        assert_eq!(sole_busy_marker(Some(&home), "myws"), None);
        assert_eq!(sole_busy_marker(Some(&home), "other"), None);
        assert_eq!(sole_busy_marker(None, "myws"), None);
    }

    #[test]
    fn a_create_result_beside_the_record_reads_as_completed() {
        let home = devpod_home_with(&[("default", "myws", Some(()))]);
        assert_eq!(create_record(Some(&home), "myws"), CreateRecord::Completed);
    }

    /// The shape a `postCreateCommand` that exited non-zero leaves behind: devpod
    /// wrote the workspace record on the way in and never wrote the result on the
    /// way out. Measured against devpod 0.26.1.
    #[test]
    fn a_record_with_no_result_reads_as_never_completed() {
        let home = devpod_home_with(&[("default", "myws", None)]);
        assert_eq!(
            create_record(Some(&home), "myws"),
            CreateRecord::NeverCompleted
        );
    }

    /// Three ways to have no answer, and none of them may read as
    /// `NeverCompleted` -- each would rebuild a workspace that is perfectly fine.
    #[test]
    fn nothing_to_read_reads_as_unknown() {
        let home = devpod_home_with(&[("default", "other", None)]);
        assert_eq!(create_record(None, "myws"), CreateRecord::Unknown);
        assert_eq!(create_record(Some(&home), "myws"), CreateRecord::Unknown);
        let empty = tempfile::tempdir().expect("an empty home");
        assert_eq!(
            create_record(Some(&DevpodHome::at(empty.path())), "myws"),
            CreateRecord::Unknown
        );
    }

    /// A context directory whose name is not text devlaunch can spell holds no
    /// record it could have read anyway, so the walk steps over it and the context
    /// that *does* hold one still answers. Losing the answer to it would rebuild a
    /// healthy workspace — and, since `flows::lifecycle`'s volume sweep shares this
    /// walk, leave that workspace's volumes on disk for a name nothing was going to
    /// read.
    #[test]
    fn a_context_directory_nobody_can_spell_does_not_cost_the_others_their_answer() {
        use std::os::unix::ffi::OsStrExt as _;

        let home = devpod_home_with(&[("default", "myws", Some(()))]);
        let unspellable = std::ffi::OsStr::from_bytes(b"\xff\xfe-not-utf8").to_os_string();
        std::fs::create_dir_all(home.contexts().join(unspellable)).expect("a context directory");

        assert_eq!(create_record(Some(&home), "myws"), CreateRecord::Completed);
    }

    /// Ids are unique per context, not globally, so one id in two contexts is an
    /// ambiguity rather than a finding. Answering from whichever context the
    /// directory iteration reached first would rebuild a healthy workspace on the
    /// strength of a different context's abandoned one.
    #[test]
    fn one_id_in_two_contexts_reads_as_unknown() {
        let home = devpod_home_with(&[("default", "myws", Some(())), ("work", "myws", None)]);
        assert_eq!(create_record(Some(&home), "myws"), CreateRecord::Unknown);
    }

    /// A record dl cannot read as one is refused rather than replaced: the file is
    /// left exactly as devpod wrote it.
    #[test]
    fn a_devpod_record_dl_cannot_read_as_one_is_refused_rather_than_replaced() {
        let dir = tempfile::tempdir().expect("a scratch home");
        let home = DevpodHome::at(dir.path().join("devpod"));
        let path = home.record("default", "ws");
        std::fs::create_dir_all(path.parent().expect("a record directory"))
            .expect("a record directory");
        std::fs::write(&path, r#"{"id": "ws", "source": "a string"}"#).expect("a record");

        assert_eq!(
            home.repoint("default", "ws", &dir.path().join("clone")),
            Err(RepointFailure::NotADevpodRecord { path: path.clone() })
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("still there"),
            r#"{"id": "ws", "source": "a string"}"#,
            "a source key dl cannot read is one it cannot safely replace"
        );
    }

    /// `DEVPOD_HOME` wins when it names somewhere, and an empty one is not
    /// somewhere. Pinned through the pure half so every case can be asserted
    /// without mutating an environment the whole test binary shares.
    #[test]
    fn a_devpod_home_comes_from_the_environment_or_the_home_directory() {
        let passwd = || Some(PathBuf::from("/home/someone"));
        assert_eq!(
            DevpodHome::located(Some(OsString::from("/elsewhere/devpod")), passwd),
            Some(DevpodHome::at("/elsewhere/devpod"))
        );
        assert_eq!(
            DevpodHome::located(Some(OsString::new()), passwd),
            Some(DevpodHome::at("/home/someone/.devpod"))
        );
        assert_eq!(
            DevpodHome::located(None, passwd),
            Some(DevpodHome::at("/home/someone/.devpod"))
        );
        assert_eq!(DevpodHome::located(None, || None), None);
    }
}
