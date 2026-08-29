//! What a lifecycle flow did, in one vocabulary for the whole family.

use std::path::PathBuf;

use super::delete::{SweepOccasion, VolumeRefusal};
use super::delete_guard::RemovalRefused;
use crate::domain::metadata::{self};
use crate::flows::repo_manager::CacheNotice;
use crate::flows::workspace_clone::RemoveWorkspaceError;
use crate::notices::{Notices, Wrapped};

/// Something a lifecycle flow did that the `dl` binary may want to report.
///
/// One vocabulary for the whole family, because a single command produces notices
/// from several of these flows. Every arm is one `logging.*` call Python made,
/// carrying what that line interpolated; nothing here is a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleNotice {
    /// The workspace's local clone was removed with it.
    CloneRemoved { workspace_id: String },
    /// devpod let go of the workspace and the clone could not be removed. The
    /// workspace is gone either way, so this is a notice rather than a failure.
    ///
    /// Carries the refusal itself, not a rendering of it: Python interpolates the
    /// exception (`Failed to remove local clone: {e}`) and the words for each arm
    /// are the `dl` binary's to choose.
    CloneNotRemoved {
        workspace_id: String,
        refusal: RemoveWorkspaceError,
    },
    /// devpod let go of the workspace and the named docker volumes its
    /// devcontainer created are still on this machine.
    ///
    /// A notice for the reason [`LifecycleNotice::CloneNotRemoved`] is one: the
    /// workspace is gone either way, and the disk left behind is a thing to say
    /// rather than a delete to fail. Carries the refusal itself and not a
    /// rendering of it — and can only be built from a refusal, because
    /// [`VolumeRefusal`] holds no arm for a sweep that went fine.
    ///
    /// `occasion` says which read named them, because that is what tells somebody
    /// where to look: devpod's own record, or devlaunch's copy of one.
    VolumesNotRemoved {
        workspace_id: String,
        occasion: SweepOccasion,
        refusal: VolumeRefusal,
    },
    /// A clone directory went and its `metadata.json` record could not be
    /// dropped. Named by the path, which is what the record described.
    ///
    /// Carries the refusal itself, for the reason
    /// [`LifecycleNotice::CloneNotRemoved`] does: Python's line is `Could not drop
    /// the record for {path}: {e}`, and the `{e}` is the binary's to write.
    RecordNotDropped {
        path: PathBuf,
        refusal: metadata::MetadataError,
    },
    /// This command is addressing a devpod workspace named by the record rather
    /// than the one this build derives (devlaunch#88).
    AddressingRecordedWorkspace {
        recorded: String,
        derived: String,
        owner: String,
        repo: String,
        branch: String,
    },
    /// Which workspace is being removed, said once the guard has had its say and
    /// before devpod is asked.
    ///
    /// The resolved id, which is the point of saying it at all: a target that was a
    /// branch, a path or a row in a picker is not this word, and the line is the
    /// only place a reader learns what it actually resolved to. It is a notice
    /// rather than something the caller prints around the call because the *timing*
    /// is what makes it a warning instead of a receipt — it has to land between the
    /// guard and `devpod delete`, and only [`workspace_remove`] knows where that
    /// is.
    Removing { workspace_id: String },
    /// The removal found work that exists nowhere else and is going ahead anyway.
    ///
    /// Only [`Removal::Wedged`] produces this: `dl <ws> rm` refuses on the same
    /// finding and `rm --force` never looks. Carries the refusal it stepped past,
    /// so the list of what is about to be destroyed is the same value the refusal
    /// would have carried — one guard, one finding, two things to do with it.
    RemovingOverWork { refusal: RemovalRefused },
    /// Something one of the storage flows reported on the way through.
    Cache(CacheNotice),
}

/// A lifecycle channel, as a storage flow's — for the callers that hand it down
/// rather than collecting a vector, which is what keeps a storage flow's own line in
/// the place Python logged it.
pub(super) fn as_cache<'a>(
    notices: &'a mut dyn Notices<LifecycleNotice>,
) -> Wrapped<'a, CacheNotice, LifecycleNotice> {
    Wrapped::new(notices, LifecycleNotice::Cache)
}

/// Collect the notices one of the storage flows produced.
pub(super) fn extend_with_cache(
    notices: &mut dyn Notices<LifecycleNotice>,
    cache: Vec<CacheNotice>,
) {
    notices.say_all(cache.into_iter().map(LifecycleNotice::Cache));
}

/// Collect the notices a `metadata.json` write produced.
pub(super) fn extend_with_store(
    notices: &mut dyn Notices<LifecycleNotice>,
    store: Vec<metadata::Notice>,
) {
    extend_with_cache(
        notices,
        store.into_iter().map(CacheNotice::Metadata).collect(),
    );
}
