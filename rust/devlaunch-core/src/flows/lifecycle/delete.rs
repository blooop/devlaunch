//! `dl <ws> rm`: a workspace, its clone, and the volumes its devcontainer made.

use std::path::Path;
use std::time::Duration;

use super::delete_guard::{
    Finding, Guarded, Insistence, Probe, Removal, RemovalRefused, guard_removal, unsaved_work_in,
};
use super::notices::{LifecycleNotice, as_cache};
use super::refresh::{Refresh, RefreshReason};
use crate::clients::devpod::{self, Call, NotRun};
use crate::clients::devpod_home::{DevpodHome, sole_workspace_result};
use crate::clients::docker;
use crate::domain::metadata::MetadataStorage;
use crate::domain::workspace_state::NonEmpty;
use crate::flows::kept_copies::{self, KeptCopies};
use crate::flows::listing::CommandContext;
use crate::flows::workspace_clone::{RemoveWorkspaceError, Removed, WorkspaceCloneManager};
use crate::notices::Notices;
use crate::runner::{Exit, OsFailure, Runner};

/// What became of the docker volumes a deleted workspace's devcontainer created.
///
/// Every arm is an outcome of a delete that **succeeded** — the workspace is gone
/// in all four — which is why this rides inside [`RemoveOutcome::Deleted`] rather
/// than being able to fail it. Reporting a failure here would send the caller
/// looking for a workspace that is not there, which is the same reasoning the
/// clone arm beside it uses.
///
/// Three of the four are silent, and the line is whether there is anything a user
/// could act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeSweep {
    /// docker removed the volumes named, or found them already gone — which
    /// `docker volume rm --force` counts as removed, so a repository whose
    /// devcontainer never declared one of these names lands here rather than in a
    /// refusal on every delete.
    Removed,
    /// Nothing was named, so docker was not run at all: devpod's record of what it
    /// substituted into this workspace's devcontainer is not there to read, which
    /// is what an `up` that never finished leaves behind.
    NothingNamed,
    /// No docker on this machine. Silent on purpose, and the reason it is its own
    /// arm: a host with no docker never made these volumes, so there is nothing
    /// here to have failed.
    NoDocker,
    /// The volumes are still on this machine, and this is why.
    Refused(VolumeRefusal),
}

/// Which read named the volumes one sweep was about.
///
/// **Two arms, and the third one is the point.** The distinction the design turns
/// on is between a name that was *read* and a name that was *inferred*, and it is
/// deliberately not on the name: a `Provenance` field with a `Pattern` arm would
/// make an inferred name representable and then rely on nobody building one, which
/// is a comment wearing a type's clothes. There is no constructor for an inferred
/// name at all (see [`crate::flows::kept_copies`]), so what is left to say belongs
/// to the *occasion* — which document this particular sweep read. Two arms and no
/// third, so adding a pattern arm later is a compile error at every match rather
/// than a branch nobody notices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepOccasion {
    /// devpod's own create result, read while the workspace still had one — at the
    /// tail of a delete, immediately before `devpod delete` takes it away.
    DevpodResult,
    /// devlaunch's kept copy, for a workspace devpod no longer lists.
    KeptCopy,
}

/// Why a deleted workspace's docker volumes are still on this machine.
///
/// Apart from [`VolumeSweep`]'s three silent arms rather than among them, so that
/// [`LifecycleNotice::VolumesNotRemoved`] cannot be built from an outcome that
/// went fine. Neither arm is a sentence: the words are the `dl` binary's, as they
/// are for every other refusal core hands over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeRefusal {
    /// docker ran and would not remove them — a volume some other container still
    /// holds is the case this exists for. `stderr` is docker's own words.
    Docker { exit: Exit, stderr: String },
    /// docker never answered: the OS would not start it, or it was killed. The
    /// errno and nothing else, for the same reason [`NotRun::Blocked`] carries
    /// one.
    NotRun { failure: OsFailure },
}

/// How a removal ended.
///
/// Three arms and one sum, rather than a guard's answer beside a delete's: the
/// refusal is an *end* of the removal, and separating the two is what let a caller
/// hold the first and go on to the second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveOutcome {
    /// The clone holds work that exists nowhere else, so nothing was deleted:
    /// devpod was never asked, the clone is where it was, and the workspace is
    /// still there.
    ///
    /// Only [`Removal::Guarded`] ends here. The refusal carries what would have
    /// been lost, so the caller writes the sentence and the way past it without
    /// asking again — the words are the caller's for the reason every other refusal
    /// in this crate leaves them there (#251 §5).
    Refused(RemovalRefused),
    /// devpod let go of the workspace. `clone` says what became of the local
    /// clone: `Ok` with which no-op or removal happened, or `Err` when the
    /// removal was attempted and refused (the workspace is gone regardless, which
    /// is why this is still `Deleted` and the refusal rides in the field rather
    /// than failing the delete). The `Err` was a fourth meaning crammed into the
    /// old `Removed::Nothing`; it is its own channel now.
    ///
    /// `volumes` is the same bargain for the named docker volumes the workspace's
    /// devcontainer created — see [`VolumeSweep`].
    Deleted {
        clone: Result<Removed, RemoveWorkspaceError>,
        volumes: VolumeSweep,
    },
    /// devpod refused, and the local clone was **kept** so the delete stays
    /// retryable. devpod re-parses the workspace's `devcontainer.json` to tear the
    /// container down, so a config that has since moved makes deletion fail — and
    /// removing the clone regardless strands the workspace for good, because devpod
    /// can then never find the config to retry with.
    DevpodRefused { exit: Exit },
}

/// Remove a workspace: the guard, the delete, and the clone with it.
///
/// **The whole of `dl <ws> rm`, `rm --force` and `kill` behind one call**, and the
/// reason it is one call is what the three used to be. The probe, the guard and the
/// delete were three separate exported functions the caller had to run in the right
/// order with the right arguments, and only the last of them was on the promised
/// surface — so the promise was an unguarded delete, and the sequence that makes it
/// safe lived in the `dl` binary where nothing else could reuse it or be held to
/// it. A second consumer following the promise exactly would delete somebody's only
/// copy of an afternoon's work. Folding them removes the way to get that wrong:
/// there is no argument to this function that skips the guard and reaches the
/// delete.
///
/// The order is the point, and it is fixed here rather than documented for a caller
/// to reproduce:
///
/// 1. **Probe**, but only for a [`Removal`] that will act on the answer — see
///    [`Removal::probe`]. It is a `git status` and a `git log` per clone.
/// 2. **Guard**, always asked with [`Insistence::NotInsisted`] whatever this
///    removal insists, because what is wanted from it is the *finding* rather than
///    the verdict: [`Removal::Wedged`] acts on the same finding differently, and
///    passing its own insistence would collapse the finding to
///    [`Guarded::MayRemove`] before it could.
/// 3. **Name the volumes, then delete, then remove the clone**, which is
///    [`workspace_delete`] and where the rest of the ordering lives.
///
/// `git` is not a parameter: inside core it is [`CommandContext::git`], so the
/// probe cannot be pointed at a different git from the one the delete's own clone
/// work uses.
///
/// `copies` is: it is the store devlaunch keeps its own copy of the substituted
/// volume names in ([`kept_copies`]), and a removal that came back removed drops
/// this workspace's copy on that proof. It passes straight through to
/// [`workspace_delete`], which is where the proof arrives. It is a parameter for
/// the store's own reason — the binary resolves the cache directory once and hands
/// it down, so a store that resolved its own could name a different directory from
/// the launch that wrote the copy.
#[allow(clippy::too_many_arguments)]
pub fn workspace_remove(
    context: &mut CommandContext<'_>,
    refresh: &mut Refresh<'_>,
    clones: &WorkspaceCloneManager<'_>,
    storage: &mut MetadataStorage,
    cache_dir: &Path,
    devpod_home: Option<&DevpodHome>,
    copies: &KeptCopies,
    workspace_id: &str,
    removal: Removal,
    stalled: &mut dyn FnMut(DeleteStalled),
    notices: &mut dyn Notices<LifecycleNotice>,
) -> Result<RemoveOutcome, NotRun> {
    if let Probe::Look(finding) = removal.probe() {
        let unsaved = unsaved_work_in(
            clones,
            storage,
            &context.git(),
            cache_dir,
            workspace_id,
            notices,
        );
        if let Guarded::Refused(refusal) =
            guard_removal(workspace_id, unsaved, Insistence::NotInsisted)
        {
            match finding {
                // The one thing dl refuses on its own account. Nothing below this
                // line has run, so the workspace and its clone are exactly as they
                // were.
                Finding::Refuses => return Ok(RemoveOutcome::Refused(refusal)),
                // Said and stepped past. A workspace reached with `kill` is one
                // somebody has already given up on, and stopping here is the failure
                // the verb was rebuilt to stop having: a wedged workspace's clone is
                // dirty almost by construction, since what wedged it interrupted
                // whatever was being done in it.
                Finding::Says => notices.say(LifecycleNotice::RemovingOverWork { refusal }),
            }
        }
    }
    // Which workspace this is, named after the guard has had its say and before
    // devpod is asked.
    notices.say(LifecycleNotice::Removing {
        workspace_id: workspace_id.to_owned(),
    });
    workspace_delete(
        context,
        refresh,
        clones,
        storage,
        devpod_home,
        copies,
        workspace_id,
        removal.insistence(),
        removal.persistence(),
        stalled,
        notices,
    )
}

/// Delete a workspace and its local clone (if any).
///
/// **Not the removal**: this is [`workspace_remove`]'s second half, with no
/// unsaved-work guard in front of it, and it is `pub(crate)` for exactly that
/// reason. It used to be the promised surface's only removal.
///
/// The clone is removed only once devpod has actually let go of the workspace —
/// see [`RemoveOutcome::DevpodRefused`] for why.
///
/// [`Insistence::Insisted`] passes devpod's own `--ignore-not-found`, which makes a
/// workspace devpod does not have count as deleted, so a forced remove is "ensure
/// absent" the way `rm -f` is. The clone cleanup still runs on that path: a stale
/// clone with no workspace is exactly what a half-finished delete leaves, and what
/// a cold-bench reset (devlaunch#140) must clear.
#[allow(clippy::too_many_arguments)]
pub(crate) fn workspace_delete(
    context: &mut CommandContext<'_>,
    refresh: &mut Refresh<'_>,
    clones: &WorkspaceCloneManager<'_>,
    storage: &mut MetadataStorage,
    devpod_home: Option<&DevpodHome>,
    copies: &KeptCopies,
    workspace_id: &str,
    insistence: Insistence,
    persistence: Persistence,
    stalled: &mut dyn FnMut(DeleteStalled),
    notices: &mut dyn Notices<LifecycleNotice>,
) -> Result<RemoveOutcome, NotRun> {
    // Named *before* the delete, and that ordering is the whole of why this is two
    // steps: `devpod delete` takes devpod's own record of the workspace away with
    // the workspace, and that record is the only place the substituted volume
    // names live. Named afterwards, this would find nothing every time and look
    // like a working cleanup.
    let named = devcontainer_volumes(devpod_home, workspace_id);
    let mut said = false;
    let exit = match devpod::run_watching_stderr(
        context.runner(),
        &delete_call(workspace_id, insistence, persistence),
        &mut |line| {
            if !said && devpod::says_it_is_blocked(line) {
                said = true;
                stalled(DeleteStalled::OnTheLock);
            }
        },
    ) {
        Ok(exit) => exit,
        // A devpod killed at [`WEDGED_DELETE`] is a devpod that *ran*, for a whole
        // minute, and may have unlinked the workspace record before the signal
        // reached it. So it is on the same side of this line as a non-zero exit,
        // and only the two ways of never starting are on the other. The
        // distinction matters because it did not exist before the deadline did:
        // `NotRun` here used to mean devpod was missing or would not exec.
        Err(NotRun::TimedOut) => {
            context.forget_workspaces();
            refresh.ask(context.runner(), RefreshReason::Forced);
            return Err(NotRun::TimedOut);
        }
        Err(never_ran) => return Err(never_ran),
    };
    // Unconditionally: a delete that reports failure may still have got far enough
    // to change what devpod lists.
    context.forget_workspaces();
    if !exit.is_success() {
        refresh.ask(context.runner(), RefreshReason::Forced);
        return Ok(RemoveOutcome::DevpodRefused { exit });
    }

    // Streamed rather than collected and appended, because the storage flow's own
    // line comes *first* in Python: `Removed workspace clone: <path>` is logged
    // inside the removal, and `Removed local clone for <id>` after it returns.
    let removal = clones.remove_workspace_by_id(storage, workspace_id, &mut as_cache(notices));
    let clone = match removal {
        Ok(removed) => {
            // Exhaustive rather than `if let`: only the clone actually removed gets
            // the `Removed local clone` line, and a new no-op arm must be a compile
            // error here rather than silently join the silent ones.
            match removed {
                Removed::Clone => notices.say(LifecycleNotice::CloneRemoved {
                    workspace_id: workspace_id.to_owned(),
                }),
                Removed::NothingRecorded
                | Removed::DirectoryNotNamed
                | Removed::DirectoryAbsent => {}
            }
            Ok(removed)
        }
        Err(error) => {
            // The workspace is gone whatever happened to the clone, so this is a
            // notice rather than the delete failing: reporting failure would send
            // the caller looking for a workspace that is not there. The refusal is
            // carried in the outcome too, so a caller reads it without re-deriving
            // it from the notice stream.
            notices.say(LifecycleNotice::CloneNotRemoved {
                workspace_id: workspace_id.to_owned(),
                refusal: error.clone(),
            });
            Err(error)
        }
    };
    let volumes = sweep_volumes(context.runner(), named);
    match &volumes {
        VolumeSweep::Refused(refusal) => notices.say(LifecycleNotice::VolumesNotRemoved {
            workspace_id: workspace_id.to_owned(),
            occasion: SweepOccasion::DevpodResult,
            refusal: refusal.clone(),
        }),
        // Dropped on proof, and the proof is the same one `--prune`'s reclaim
        // drops a copy on: a removal that came back removed, for a workspace
        // devpod no longer has. The copy named volumes that are gone, so it is
        // pointless — and left standing it would have the next `--prune` report
        // reclaiming volumes that went with the workspace. `Refused` keeps it, so
        // the retry survives; the two silent arms name nothing and prove nothing.
        VolumeSweep::Removed => copies.forget(workspace_id),
        VolumeSweep::NothingNamed | VolumeSweep::NoDocker => {}
    }
    refresh.ask(context.runner(), RefreshReason::Forced);
    Ok(RemoveOutcome::Deleted { clone, volumes })
}

/// Remove the volumes `named`, and say what became of them.
///
/// The one place a [`docker::Answer`] becomes a [`VolumeSweep`], so every removal
/// path draws the same line in the same place: nothing to name and no docker to
/// name it with are silent, and a docker that was asked and did not deliver is
/// not. It reports nothing itself — `rm` says its piece as a
/// [`LifecycleNotice`] and `--purge` as a [`PurgeStep`], and neither vocabulary
/// belongs to the removal.
pub(super) fn sweep_volumes(runner: &dyn Runner, named: Option<NonEmpty<String>>) -> VolumeSweep {
    let Some(names) = named else {
        return VolumeSweep::NothingNamed;
    };
    match docker::remove_volumes(runner, &names) {
        docker::Answer::Ran { exit, .. } if exit.is_success() => VolumeSweep::Removed,
        docker::Answer::Ran { exit, stderr } => {
            VolumeSweep::Refused(VolumeRefusal::Docker { exit, stderr })
        }
        docker::Answer::NotInstalled => VolumeSweep::NoDocker,
        docker::Answer::NotStarted(failure) => {
            VolumeSweep::Refused(VolumeRefusal::NotRun { failure })
        }
    }
}

/// The named docker volumes one workspace's devcontainer created, or nothing where
/// devpod's own record cannot say.
///
/// **One source for both names, and it is devpod's create result** — the file
/// devpod writes on its way out of a successful `up`, recording what it
/// substituted into the devcontainer. Both volumes are named from variables devpod
/// expanded, so the record is by definition the answer:
///
/// - `${localWorkspaceFolderBasename}-pixi`, from this repository's own
///   `.devcontainer/devcontainer.json` mount for the `.pixi` cache;
/// - `dind-var-lib-docker-${devcontainerId}`, from the `docker-in-docker` feature.
///
/// Deriving the basename from the clone directory devlaunch chose instead would be
/// a second answer to a question devpod has already answered, and the two can
/// disagree — a workspace opened before a rename, say. Neither *value* is guessed:
/// a substitution that is not in the record names no volume. The two name
/// **templates** are still this repository's own devcontainer and the
/// `docker-in-docker` feature's, so a `mounts` entry naming some third volume is
/// not swept — devlaunch#325's scope, and the follow-up that would end it is
/// reading the mount sources out of the recorded merged config instead.
///
/// [`sole_workspace_result`] is what finds the file, rather than a contexts walk of
/// its own: an id under two contexts must answer nothing, and `devpod delete`
/// resolves that id against the *current* context, so a walk that picked one would
/// remove a living workspace's volumes. Sharing the walk makes the rule identical
/// by construction instead of by comment.
///
/// `None` rather than an empty list, so a caller cannot read "nothing to remove"
/// as "removed nothing" — an `up` that died in its lifecycle hooks leaves the
/// workspace record with no result beside it, which is exactly this case.
pub(super) fn devcontainer_volumes(
    devpod_home: Option<&DevpodHome>,
    workspace_id: &str,
) -> Option<NonEmpty<String>> {
    let result = sole_workspace_result(devpod_home, workspace_id)?;
    let recorded = kept_copies::parse_substitutions(&std::fs::read(result).ok()?);
    NonEmpty::of(recorded.volume_names())
}

/// `devpod delete <id> [--ignore-not-found] [--force]` — argv-exact.
///
/// The two flags answer two different questions and are not two spellings of one.
/// `--ignore-not-found` is [`Insistence`]'s, and it is about *dl's* verdict: a
/// workspace devpod never heard of counts as deleted, so `rm --force` is "ensure
/// absent" the way `rm -f` is. `--force` is [`Persistence`]'s, and it is about
/// devpod's: delete the record even for a workspace whose container or machine
/// devpod can no longer reach. A wedged workspace is exactly that case, which is
/// why dl's own refusal already told people to type this flag by hand.
fn delete_call(workspace_id: &str, insistence: Insistence, persistence: Persistence) -> Call {
    let mut args = vec!["delete".to_owned(), workspace_id.to_owned()];
    if let Insistence::Insisted = insistence {
        args.push("--ignore-not-found".to_owned());
    }
    if let Persistence::Wedged = persistence {
        args.push("--force".to_owned());
    }
    let call = Call::new(args);
    match persistence {
        Persistence::Ordinary => call,
        Persistence::Wedged => call.with_timeout(WEDGED_DELETE),
    }
}

/// How long `kill`'s delete may take before dl abandons it.
///
/// **A deadline exists on this call and on no other delete**, and the asymmetry is
/// the point rather than an oversight. `rm`'s delete is allowed to take as long as
/// it takes, because a container that is slow to come down is a container that is
/// coming down. `kill`'s is reached for by somebody who has just sat through
/// devpod's five-second `Trying to lock workspace` line, which is a *blocking*
/// acquire with nothing behind it — so the one ending this call must not have is
/// the one they came here to escape.
///
/// A minute rather than a few seconds, and it has to cover the case where the
/// sweep in front of it killed no containers at all: it leaves a live build's
/// alone, it kills nothing on a host with no docker, and it kills nothing when
/// docker refuses. So the budget is a real `devpod delete` from a standing start,
/// stopping containers and removing volumes, and not merely the record unlinking
/// that is left when the sweep did clear them. The measured delete in
/// devlaunch#484's own report took one second.
pub(super) const WEDGED_DELETE: Duration = Duration::from_secs(60);

/// What devpod said mid-delete that means the delete is not going to finish.
///
/// One arm, and a channel of its own rather than an outcome, because the whole
/// point is that there is no outcome: devpod's lock acquire has nothing behind it,
/// so a delete that hits this returns when the holder dies and not before. By the
/// time a `Result` could carry the fact, the fact is hours stale.
///
/// Reported once per call however many times devpod says it. The line repeats
/// every five seconds for as long as the holder lives, and advice repeated on that
/// timer buries itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteStalled {
    /// Something else holds this workspace's lock, and devpod is waiting on it
    /// with no deadline.
    OnTheLock,
}

/// How hard devpod is pushed to let go of the workspace.
///
/// A named pair rather than a `bool` for [`Insistence`]'s reason, and kept
/// separate from it for a sharper one: the two are read together at exactly one
/// call site and mean opposite-facing things there. [`Insistence`] is what dl will
/// *accept* as a delete, and this is what devpod is *asked* for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Persistence {
    /// `rm`'s delete: devpod's defaults, and devpod's own patience.
    Ordinary,
    /// `kill`'s: `--force`, so a workspace devpod can no longer reach goes anyway,
    /// and a deadline, so the delete cannot join the lock loop the sweep that runs
    /// in front of it was reached for.
    Wedged,
}
