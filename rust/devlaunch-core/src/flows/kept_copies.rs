//! devlaunch's own copy of what devpod substituted into a workspace, kept so the
//! volumes of a workspace devpod has forgotten are still reclaimable by *reading*.
//!
//! Both volume names come from one file: devpod's `workspace_result.json`, whose
//! `SubstitutionContext` records what `${localWorkspaceFolderBasename}` and
//! `${devcontainerId}` expanded to. `flows::lifecycle` reads it at delete time,
//! immediately before `devpod delete` destroys it. That read closed the leak going
//! forward (devlaunch#325) and left one population behind: a workspace deleted by a
//! bare `devpod delete` outside `dl` takes the record away and leaves the volumes,
//! and after that there is nothing on the machine that names them.
//!
//! So this module takes a **second read of the same document**, at the tail of a
//! completed `up`, and keeps the answer. Nothing is synthesized: every name dl ever
//! hands `docker volume rm` still originates in a substitution devpod performed and
//! wrote down. A pattern over `docker volume ls` would name volumes *nobody ever
//! recorded*, which is somebody else's disk, and devlaunch#451 refuses it as a
//! design rather than deferring it.
//!
//! # A copy can be wrong in exactly two ways, and neither is caught by trust
//!
//! It can name a volume that is already gone, which `docker volume rm --force`
//! makes a silent success. It can name a volume something else now holds, which
//! docker refuses: measured on docker 29.7.2, a volume held by a container in state
//! `exited` exits 1 with `volume is in use`. Both are answered by something other
//! than believing the copy. What a copy cannot do is the thing a pattern does.
//!
//! The second guard is where the copies are *consulted*, not here: only for a
//! workspace no `devpod list` names. A devpod home copied between machines is the
//! dishonest case, and [`super::provision::verdict_cache`]'s mtime-equality anchor
//! is unavailable to it — the anchor dies with the workspace, which is the whole
//! occasion this exists for.
//!
//! # Its own marker, beside the tool verdicts rather than inside them
//!
//! Sibling to [`super::provision::verdict_cache`]'s markers and modelled on them,
//! down to the atomic write. Not a field on one, because that marker's *presence*
//! means `AlreadyProvisioned` and a copy has to be written on every completed `up`
//! whatever the verdict; one file with two write conditions makes one condition
//! govern the other. Not a `metadata.json` field either: that file is the pinned
//! shared format with a schema version, and its worktree records are what `--prune`
//! *drops* — the record would be destroyed by the command best placed to use it.
//!
//! # Where this diverges from the verdict cache: a copy is dropped
//!
//! "Nothing ever deletes a marker" is that module's rule, and the reason it holds
//! there is that a delete would be a second unproven mechanism saying what the
//! mtime comparison already says. Here there is no comparison to make: a copy is
//! about a workspace that may be gone, and the proof arrives from docker. So
//! [`KeptCopies::forget`] is called on exactly one occasion — a removal that came
//! back removed, for a workspace devpod does not list — and never on a refusal, so
//! the retry survives.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::clients::devpod_home::{DevpodHome, sole_workspace_result};
use crate::domain::workspace_state::NonEmpty;

/// The directory the copies live in, under devlaunch's cache directory.
///
/// Beside `tool-verdicts`, deliberately: two per-workspace stores with two write
/// conditions, and a reader of either can see at a glance which it is holding.
const COPIES_DIR: &str = "workspace-copies";

/// Where devlaunch keeps its copies of what devpod substituted.
///
/// A path parameter rather than a read of the process environment, for
/// [`super::provision::verdict_cache::VerdictCache`]'s reason: the binary resolves
/// the cache directory once and hands it down, and a store that resolved its own
/// could disagree with the one the launch already resolved. It is also what makes
/// the map's hard constraint hold by construction — a run pointed at a scratch
/// `XDG_CACHE_HOME` finds no copies at all, so it names no volume and removes
/// none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptCopies {
    dir: PathBuf,
}

impl KeptCopies {
    /// The copies devlaunch keeps under `cache_dir`.
    pub fn under(cache_dir: &Path) -> Self {
        Self {
            dir: cache_dir.join(COPIES_DIR),
        }
    }

    /// This workspace's copy.
    ///
    /// The id goes into the name unescaped, as
    /// [`super::provision::verdict_cache`]'s marker does and for its reason: devpod
    /// itself uses the id as a directory name under its own contexts, so an id that
    /// could not be a path component is one no workspace this could be asked about
    /// has.
    fn copy_path(&self, workspace_id: &str) -> PathBuf {
        self.dir.join(format!("{workspace_id}.json"))
    }

    /// Copy what devpod recorded about this workspace, having just brought it up.
    ///
    /// **The write verb, and it does the read itself** — one document, one read,
    /// one marker. Handing this the names instead would put a second parse of
    /// `workspace_result.json` in the launch flow, and two parses of one document
    /// are two answers waiting to disagree.
    ///
    /// An `up` that never completed leaves no result beside the record, so
    /// [`sole_workspace_result`] finds nothing and this writes nothing. That
    /// residual cannot be closed by anybody: devpod writes the result at the tail of
    /// `devPodUp` after every failing branch has returned, so a create that died
    /// leaves the dind volume standing with no record of it anywhere on the machine.
    /// It stays small rather than open-ended because the names are deterministic for
    /// a folder plus config: any later completed `up` of the same workspace records
    /// the same names, and the volume is nameable again.
    ///
    /// Silent about every way of not working, exactly as the verdict cache's write
    /// is. A copy that could not be written is a launch that behaves as it did
    /// before this existed, and a launch is not worth failing over that.
    pub(crate) fn keep(&self, workspace_id: &str, devpod_home: Option<&DevpodHome>) {
        let Some(result) = sole_workspace_result(devpod_home, workspace_id) else {
            return;
        };
        let Ok(bytes) = std::fs::read(&result) else {
            return;
        };
        let recorded = parse_result(&bytes);
        if recorded.is_empty() {
            // Nothing devpod substituted and no image it named: a document this
            // could not read is a document it copies nothing out of, which leaves
            // the workspace exactly where it was before the copy existed.
            return;
        }
        let Ok(text) = serde_json::to_string(&recorded) else {
            return;
        };
        write_atomically(&self.copy_path(workspace_id), &text);
    }

    /// The volume names this workspace's copy holds, or nothing where there is no
    /// copy to read.
    ///
    /// **The read verb.** `None` rather than an empty list, for the reason
    /// `flows::lifecycle::devcontainer_volumes` answers `None`: a caller must not be
    /// able to read "nothing was recorded" as "recorded nothing", and an empty list
    /// would have to mean both. Every doubt reads as no copy — a file that is not
    /// there, will not parse, or carries a shape this build does not know.
    pub(crate) fn volumes(&self, workspace_id: &str) -> Option<NonEmpty<String>> {
        NonEmpty::of(read_copy(&self.copy_path(workspace_id))?.volumes)
    }

    /// Every workspace this cache holds a copy for, sorted.
    ///
    /// The reclaim's domain, and it is deliberately not a walk of the clone
    /// directories. A copy whose clone the user deleted by hand names volumes no
    /// clone-shaped walk will ever reach, and reasoning over an enumeration that
    /// does not cover what it affects is the defect class devlaunch#445 exists to
    /// close.
    ///
    /// Sorted so two runs over an unchanged cache read alike; a directory that is
    /// not there is no copies rather than an error, which is what a fresh install
    /// and a scratch cache both are.
    pub(crate) fn copied(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension()? == "json")
                    .then(|| path.file_stem()?.to_str().map(str::to_owned))
                    .flatten()
            })
            .collect();
        ids.sort();
        ids
    }

    /// Drop this workspace's copy.
    ///
    /// **The drop verb, and its one occasion is proof** — see the module note. A
    /// copy that is not there is not a failure: the delete path and the reclaim can
    /// both arrive here about the same workspace, and the second one has nothing to
    /// do.
    pub(crate) fn forget(&self, workspace_id: &str) {
        let _ = std::fs::remove_file(self.copy_path(workspace_id));
    }
}

/// What one copy says.
///
/// Versioned by "unparseable means absent", which is the whole of the format story:
/// a copy a later build writes differently fails to deserialize and reads as no
/// copy, costing one workspace's volumes their record rather than costing anybody a
/// migration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
struct Copy {
    /// The derived volume names, in the order they were declared — **the names,
    /// not the substituted values they were built from.** Freezing them at read
    /// time is what keeps each one traceable to one real substitution: re-deriving
    /// later under a changed template would produce a string devpod never
    /// substituted, which is the pattern route arriving by the back door.
    volumes: Vec<String>,
    /// `ContainerDetails.Config.Image` — the image reference the container devpod
    /// built or pulled actually ran, whether or not anything named it.
    ///
    /// Read from the same document at the same moment as the volume names
    /// (devlaunch#450), because versioning a small per-workspace file twice within
    /// one map would be two migrations for one fact. **Nothing here removes an
    /// image**; devlaunch#458 is the reclaim that reads this.
    image: Option<String>,
    /// `MergedConfig.image` — the reference the devcontainer *named*, where it
    /// named one at all. Kept apart from `image` so a reference the config declared
    /// stays distinguishable from one devpod derived; a devcontainer built from a
    /// Dockerfile declares none.
    declared_image: Option<String>,
}

impl Copy {
    /// Whether this copy would say nothing at all, in which case none is written.
    fn is_empty(&self) -> bool {
        self.volumes.is_empty() && self.image.is_none() && self.declared_image.is_none()
    }
}

/// Read one copy, or nothing where there is nothing readable there.
fn read_copy(path: &Path) -> Option<Copy> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Everything devlaunch keeps out of one `workspace_result.json`, from one parse.
///
/// Total over anything the file could hold: bytes that are not JSON, or JSON of
/// another shape, answer the empty copy rather than an error. Nothing here is worth
/// a diagnostic — a result devlaunch cannot read is a result it copies nothing out
/// of, which is exactly what it did before this existed.
fn parse_result(bytes: &[u8]) -> Copy {
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Copy::default();
    };
    Copy {
        volumes: substitutions_in(&document).volume_names(),
        image: non_empty_string(&document["ContainerDetails"]["Config"]["Image"]),
        declared_image: non_empty_string(&document["MergedConfig"]["image"]),
    }
}

/// What devpod recorded substituting into one workspace's devcontainer.
///
/// Only the two fields the volume names are built from. Both optional because
/// devpod omits an empty one, and a record written by a devpod that never learned
/// about `${devcontainerId}` has neither.
///
/// There is deliberately **no provenance field**, here or on the names. A
/// `from_pattern: bool`, or a `Provenance` enum with a `Pattern` arm, would make an
/// inferred name representable and then rely on nobody building one, which is a
/// comment wearing a type's clothes. Instead there is no constructor for an
/// inferred name at all: names exist only as the return of
/// [`Substitutions::volume_names`], and a `Substitutions` is only ever built by
/// [`substitutions_in`] over bytes devpod wrote or devlaunch copied. Where the
/// distinction genuinely has to be visible is the report, and it belongs to the
/// occasion rather than the value — see `flows::lifecycle::SweepOccasion`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Substitutions {
    /// `SubstitutionContext.LocalWorkspaceFolder` — the host directory devpod
    /// opened, whose basename is what `${localWorkspaceFolderBasename}` expanded
    /// to.
    local_workspace_folder: Option<String>,
    /// `SubstitutionContext.DevContainerID` — what `${devcontainerId}` expanded
    /// to.
    devcontainer_id: Option<String>,
}

impl Substitutions {
    /// The volume names these substitutions imply, in the order they were
    /// declared. Empty where neither field was recorded.
    ///
    /// The two name **templates** are this repository's own devcontainer and the
    /// `docker-in-docker` feature's, which is devlaunch#325's scope; the two
    /// *values* in them are devpod's own. This is the only place either template is
    /// spelled, and `devlaunch-core/tests/volume_names.rs` holds the crate to that.
    pub(crate) fn volume_names(&self) -> Vec<String> {
        let basename = self
            .local_workspace_folder
            .as_deref()
            .map(Path::new)
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned());
        [
            basename.map(|basename| format!("{basename}-pixi")),
            self.devcontainer_id
                .as_deref()
                .map(|id| format!("dind-var-lib-docker-{id}")),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// The two substituted values devpod's create result records.
pub(crate) fn substitutions_in(document: &serde_json::Value) -> Substitutions {
    let context = &document["SubstitutionContext"];
    Substitutions {
        local_workspace_folder: non_empty_string(&context["LocalWorkspaceFolder"]),
        devcontainer_id: non_empty_string(&context["DevContainerID"]),
    }
}

/// Read the two substituted values off devpod's create result.
///
/// Total for [`parse_result`]'s reason, and the same function underneath: the live
/// read at delete time and the kept copy's read must agree about what a record says
/// by construction rather than by two implementations being written alike.
pub(crate) fn parse_substitutions(bytes: &[u8]) -> Substitutions {
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Substitutions::default();
    };
    substitutions_in(&document)
}

/// The string this value holds, where it holds a non-empty one. Empty is dropped
/// so a blank field cannot build the volume name `-pixi`.
fn non_empty_string(value: &serde_json::Value) -> Option<String> {
    match value.as_str() {
        Some(text) if !text.is_empty() => Some(text.to_owned()),
        _ => None,
    }
}

/// *text* into *path*, via a temp file in the same directory.
///
/// The shape [`super::provision::verdict_cache`] and
/// [`super::completion_cache`] both write with, and here for the same property: a
/// reader arriving mid-write sees the old copy or the new one, never half of
/// either, and half a copy is a copy that does not parse and so reads as absent
/// anyway.
///
/// Silent: the caller is best-effort.
fn write_atomically(path: &Path, text: &str) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let mut name = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
    name.push(format!(".{}.tmp", std::process::id()));
    let staged = path.with_file_name(name);
    if std::fs::write(&staged, text).is_err() {
        return;
    }
    let _ = std::fs::rename(&staged, path);
}

#[cfg(test)]
mod tests {
    //! The copy store on its own, over a scratch cache and a scratch devpod home.
    //!
    //! No docker and no devpod are involved anywhere here: what this module does is
    //! read one file devpod left behind and write one of its own, and the whole of
    //! it is decided by the state of two temporary directories.

    use super::*;
    use crate::clients::devpod_home::{ScratchHome, devpod_home_with};

    /// A cache directory, and the copies kept under it.
    fn a_cache() -> (tempfile::TempDir, KeptCopies) {
        let dir = tempfile::tempdir().expect("a scratch cache");
        let copies = KeptCopies::under(dir.path());
        (dir, copies)
    }

    /// A devpod home whose create result for `workspace_id` records what devpod
    /// substituted, in devpod's own shape: `SubstitutionContext` beside
    /// `ContainerDetails` and `MergedConfig`.
    fn a_completed_up(workspace_id: &str, folder: &str, devcontainer_id: &str) -> ScratchHome {
        a_result(
            workspace_id,
            serde_json::json!({
                "ContainerDetails": {
                    "Id": "container-id",
                    "Config": { "Image": "vsc-devlaunch-4f2a-uid" },
                },
                "MergedConfig": { "image": "mcr.microsoft.com/devcontainers/base:jammy" },
                "SubstitutionContext": {
                    "LocalWorkspaceFolder": folder,
                    "ContainerWorkspaceFolder": "/workspaces/whatever",
                    "DevContainerID": devcontainer_id,
                },
            }),
        )
    }

    /// A devpod home holding `document` as `workspace_id`'s create result.
    fn a_result(workspace_id: &str, document: serde_json::Value) -> ScratchHome {
        let home = devpod_home_with(&[("default", workspace_id, Some(()))]);
        std::fs::write(home.result("default", workspace_id), document.to_string())
            .expect("a create result");
        home
    }

    fn names(copies: &KeptCopies, workspace_id: &str) -> Vec<String> {
        copies
            .volumes(workspace_id)
            .map(|names| names.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Both names, from one read of one document, in the order the templates are
    /// declared.
    #[test]
    fn a_completed_up_is_copied_with_both_volume_names() {
        let (_dir, copies) = a_cache();
        let home = a_completed_up("myws", "/home/someone/repos/o/r/repo-main-ab12", "abcdef");

        copies.keep("myws", Some(&home));

        assert_eq!(
            names(&copies, "myws"),
            ["repo-main-ab12-pixi", "dind-var-lib-docker-abcdef"]
        );
    }

    /// The whole population this exists for: devpod's record dies with the
    /// workspace, and the copy is in devlaunch's cache, so it does not.
    #[test]
    fn a_copy_outlives_the_devpod_record_its_names_came_from() {
        let (_dir, copies) = a_cache();
        let home = a_completed_up("myws", "/repos/o/r/repo-main-ab12", "abcdef");
        copies.keep("myws", Some(&home));

        std::fs::remove_dir_all(home.path()).expect("a bare `devpod delete` outside dl");

        assert_eq!(
            names(&copies, "myws"),
            ["repo-main-ab12-pixi", "dind-var-lib-docker-abcdef"]
        );
    }

    /// An `up` that died in its lifecycle hooks leaves the workspace record with no
    /// result beside it. Nothing is named, so nothing is written, and the reclaim
    /// finds no copy to act on — unchanged from before this existed.
    #[test]
    fn an_up_that_never_completed_is_not_copied_at_all() {
        let (_dir, copies) = a_cache();
        let home = devpod_home_with(&[("default", "myws", None)]);

        copies.keep("myws", Some(&home));

        assert_eq!(copies.volumes("myws"), None);
        assert_eq!(copies.copied(), Vec::<String>::new());
    }

    /// The image reference rides on the same copy, read from the same document at
    /// the same moment (devlaunch#450), and the two references stay apart: what the
    /// container ran, and what the devcontainer named.
    #[test]
    fn the_copy_carries_the_image_reference_from_the_same_read() {
        let (dir, copies) = a_cache();
        let home = a_completed_up("myws", "/repos/o/r/repo-main-ab12", "abcdef");

        copies.keep("myws", Some(&home));

        let written = read_copy(&dir.path().join(COPIES_DIR).join("myws.json")).expect("a copy");
        assert_eq!(written.image.as_deref(), Some("vsc-devlaunch-4f2a-uid"));
        assert_eq!(
            written.declared_image.as_deref(),
            Some("mcr.microsoft.com/devcontainers/base:jammy")
        );
    }

    /// A devcontainer built from a Dockerfile names no image, and a devpod that
    /// recorded no substitutions names no volume. Neither is a reason to lose the
    /// other, and a document that says neither is not written down at all.
    #[test]
    fn each_recorded_fact_is_copied_without_the_others() {
        let (_dir, copies) = a_cache();

        let from_a_dockerfile = a_result(
            "built",
            serde_json::json!({
                "ContainerDetails": { "Config": { "Image": "vsc-built-9a" } },
                "MergedConfig": {},
                "SubstitutionContext": { "LocalWorkspaceFolder": "/repos/o/r/built" },
            }),
        );
        copies.keep("built", Some(&from_a_dockerfile));
        assert_eq!(names(&copies, "built"), ["built-pixi"]);

        let unreadable = a_result("garbled", serde_json::json!({ "MergedConfig": {} }));
        copies.keep("garbled", Some(&unreadable));
        assert_eq!(copies.copied(), ["built"]);
    }

    /// Every doubt reads as no copy: a file that is not there, one that will not
    /// parse, and one whose volume list is empty — which must not read as "recorded
    /// nothing" (the `NonEmpty` the ticket asks for, with no empty-list state).
    #[test]
    fn every_unreadable_copy_names_no_volume() {
        let (dir, copies) = a_cache();
        let store = dir.path().join(COPIES_DIR);
        std::fs::create_dir_all(&store).expect("a copies directory");

        assert_eq!(copies.volumes("never-written"), None);

        std::fs::write(store.join("garbled.json"), "{not json").expect("a truncated copy");
        assert_eq!(copies.volumes("garbled"), None);

        std::fs::write(store.join("empty.json"), r#"{"volumes":[]}"#).expect("an empty copy");
        assert_eq!(copies.volumes("empty"), None);
    }

    /// The domain of the reclaim: the set of copies, sorted, and nothing else in
    /// the directory joins it.
    #[test]
    fn the_copies_are_enumerated_and_nothing_else_in_the_directory_is() {
        let (dir, copies) = a_cache();
        for id in ["bee", "ant"] {
            let home = a_completed_up(id, &format!("/repos/o/r/{id}"), id);
            copies.keep(id, Some(&home));
        }
        std::fs::write(dir.path().join(COPIES_DIR).join("notes.txt"), "not a copy")
            .expect("a stray file");

        assert_eq!(copies.copied(), ["ant", "bee"]);
    }

    /// A scratch `XDG_CACHE_HOME` is the map's hard constraint, and this is what
    /// makes it hold by construction: a cache with no copies in it names no volume,
    /// so a scratch run removes none.
    #[test]
    fn a_cache_with_no_copies_names_no_volume() {
        let (_dir, copies) = a_cache();

        assert_eq!(copies.copied(), Vec::<String>::new());
        assert_eq!(copies.volumes("myws"), None);
    }

    /// Dropped once, and dropping one that is already gone is not a complaint: the
    /// delete path and the reclaim can both arrive about the same workspace.
    #[test]
    fn a_dropped_copy_is_gone_and_dropping_it_twice_is_no_complaint() {
        let (_dir, copies) = a_cache();
        let home = a_completed_up("myws", "/repos/o/r/repo-main-ab12", "abcdef");
        copies.keep("myws", Some(&home));

        copies.forget("myws");
        assert_eq!(copies.volumes("myws"), None);
        assert_eq!(copies.copied(), Vec::<String>::new());

        copies.forget("myws");
        copies.forget("never-there");
    }

    /// A later completed `up` rewrites the copy rather than leaving the first one
    /// standing. That is what keeps the residual small: a workspace whose first
    /// `up` died records nothing, and the next one that finishes records the names.
    #[test]
    fn a_later_completed_up_replaces_the_copy() {
        let (_dir, copies) = a_cache();
        copies.keep(
            "myws",
            Some(&a_completed_up("myws", "/repos/o/r/before", "aaaa")),
        );

        copies.keep(
            "myws",
            Some(&a_completed_up("myws", "/repos/o/r/after", "bbbb")),
        );

        assert_eq!(
            names(&copies, "myws"),
            ["after-pixi", "dind-var-lib-docker-bbbb"]
        );
    }

    /// A machine whose devpod records cannot be located has no document to copy
    /// from, which is nothing to write rather than something to fail over.
    #[test]
    fn a_host_with_no_devpod_home_copies_nothing() {
        let (_dir, copies) = a_cache();

        copies.keep("myws", None);

        assert_eq!(copies.copied(), Vec::<String>::new());
    }

    /// Ids are unique per context, not globally, and `sole_workspace_result`
    /// answers nothing to the ambiguity. Copying from whichever context the walk
    /// reached first would name a *different* workspace's volumes, and the copy
    /// outlives the record that could have corrected it.
    #[test]
    fn a_workspace_two_contexts_hold_is_copied_from_neither() {
        let (_dir, copies) = a_cache();
        let home = devpod_home_with(&[("default", "myws", Some(())), ("work", "myws", Some(()))]);

        copies.keep("myws", Some(&home));

        assert_eq!(copies.copied(), Vec::<String>::new());
    }

    /// Each recorded substitution names its own volume, and one recorded without
    /// the other still gets the volume it can name. Both spellings are devpod's.
    #[test]
    fn each_recorded_substitution_names_its_own_volume() {
        let both = Substitutions {
            local_workspace_folder: Some("/repos/o/r/repo-branch-abcd".to_owned()),
            devcontainer_id: Some("deadbeef".to_owned()),
        };
        assert_eq!(
            both.volume_names(),
            ["repo-branch-abcd-pixi", "dind-var-lib-docker-deadbeef"]
        );

        let no_id = Substitutions {
            local_workspace_folder: Some("/repos/o/r/repo-branch-abcd".to_owned()),
            devcontainer_id: None,
        };
        assert_eq!(no_id.volume_names(), ["repo-branch-abcd-pixi"]);

        assert!(Substitutions::default().volume_names().is_empty());

        // A blank field names nothing: `-pixi` and `dind-var-lib-docker-` are not
        // volumes anybody meant, and asking docker about them is asking about
        // somebody else's disk.
        let blank = parse_substitutions(
            serde_json::json!({
                "SubstitutionContext": { "LocalWorkspaceFolder": "", "DevContainerID": "" },
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(blank.volume_names(), Vec::<String>::new());
    }

    /// Bytes that are not JSON, and JSON of another shape, name nothing rather
    /// than failing: a result devlaunch cannot read is a result it removes no
    /// volumes from.
    #[test]
    fn a_result_that_is_not_a_devpod_record_names_nothing() {
        for bytes in [
            &b"not json at all"[..],
            b"[]",
            br#"{"SubstitutionContext": "a string"}"#,
            b"{}",
        ] {
            assert_eq!(
                parse_substitutions(bytes),
                Substitutions::default(),
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }
}
