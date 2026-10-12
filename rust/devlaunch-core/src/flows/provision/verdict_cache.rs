//! Remembering that a container was found provisioned, so a later pass over the
//! same container can skip the trip that found out.
//!
//! The setup pass is one `devpod ssh` round trip, measured at ~1.7s of which ~99%
//! is connection and process setup (#157), and on a container the host lends its
//! binaries to it is two trips and ~7s. A workspace that has had `gh` and a real
//! `claude` in it for a week pays that on every `dl <ws> up`, every time, to be told
//! the same thing it was told last time. This is the one file that lets the answer
//! be reused: a marker per workspace under devlaunch's own cache directory, holding
//! the verdict and — the whole of the mechanism — the identity of the container the
//! verdict was about.
//!
//! # What makes a remembered verdict still true
//!
//! Not the marker's own age alone, and not a TTL standing in for an identity. A
//! verdict about *this* container is true until there is a different container,
//! and "a different container" is a fact the host can read without asking
//! anything: devpod rewrites `workspace_result.json` on its way out of every
//! **completed** `up`, whoever ran that `up` — `dl`, VS Code, a hand-typed
//! `devpod up`, a `--recreate` — and that file carries `ContainerDetails.ID`, the
//! container's docker id. So the marker records the id, and the verdict is trusted
//! only while the result file still names the *same* id.
//!
//! The id and not the file's mtime, which is what this used to key on, because the
//! mtime moves on every `up` and the id does not. Measured on devpod 0.26.3: a
//! stop and an `up` leave `ContainerDetails.ID` and `.Created` exactly as they
//! were, and only a create, a `--recreate` or a `--reset` makes a new one. Keyed on
//! the mtime, every `up` threw away a verdict about a container that had merely
//! been stopped, and the pass after it always travelled.
//!
//! The mtime is still recorded beside the id, and it still answers one question:
//! *has this container been started since the verdict was written?* That matters
//! for one stage only, the hostname, which lives in the container's UTS namespace
//! and is rebuilt from the container's config on every start. A pass whose
//! `sudo hostname` worked has to run again after a start, or the prompt reads the
//! wrong name. A pass whose `sudo hostname` was refused, which is most containers
//! (it needs `CAP_SYS_ADMIN`), has nothing to redo, and every other stage writes
//! files the start leaves alone. So a verdict carries across a start exactly when
//! its pass recorded the hostname stage as refused. See [`Marker::survives_a_start`].
//!
//! Equality rather than "not newer", for both fields, because the comparison is an
//! identity check and not a freshness one. A file restored from a backup, a clock
//! that stepped backwards, a devpod home copied between machines: each is exactly
//! the case where the recorded verdict is about a container that is not the one
//! standing now.
//!
//! `workspace.json` beside it is deliberately not the anchor — devpod writes it on
//! the way *in* and never touches it again.
//!
//! # Absent is the only harmless misreading
//!
//! Every failure here reads as *no trusted verdict*: a marker that is not there,
//! will not parse, carries a word this build does not know, names a workspace whose
//! `workspace_result.json` cannot be found, is ambiguous between two devpod
//! contexts, carries no container id, or whose mtime cannot be read. That is the
//! same argument [`super::ProbeResult::parse`] makes for reading a garbled report as
//! *absent*: provisioning is idempotent, so reading a valid marker as invalid costs
//! one redundant round trip — today's behaviour, exactly — where reading an invalid
//! one as valid silently ships a container without the tools the pass exists to put
//! there.
//!
//! # What the id cannot see, and how long a verdict is believed across a start
//!
//! The id says the container is the same one. It does not say nothing changed
//! inside it. Two kinds of change are invisible to it:
//!
//! - **A dotfiles refresh** (`dl <ws> dotfiles`, or `DEVLAUNCH_DOTFILES_ON_ATTACH`)
//!   runs `chezmoi update` and `pixi global sync` in a standing container, which
//!   can add or remove tools. devlaunch runs it, so devlaunch drops the marker
//!   first: [`VerdictCache::forget`]. That is the one place a marker is ever
//!   deleted. Every other invalidation is the comparison, made afresh on every read.
//! - **A hand edit inside the container**, such as deleting the lent `claude`.
//!   Nothing on the host sees that. Before this module keyed on the id, the next
//!   `up` caught it, because its pass always travelled. Now a verdict that carries
//!   across a start is believed for at most [`MAX_CARRY_ACROSS_A_START`] after the
//!   pass that wrote it. After that, the pass after an `up` travels again and writes
//!   a fresh one. A container that never stops was already trusted for as long as it
//!   stood, which is the top-up's old bargain, and that is unchanged.
//!
//! # What is deliberately not remembered
//!
//! Only [`super::Provisioning::AlreadyProvisioned`] and
//! [`super::Provisioning::Lent`] are recorded. A lend is recorded because its
//! transfer ends with a gate that runs the lent binaries in the container, so a
//! lend that returned is a container that has them; and because a lent container
//! does not probe provisioned again when an image puts a `claude` shim ahead of
//! `~/.local/bin` on the login PATH, so waiting for "a later pass that probes
//! provisioned" waited for ever and every `up` lent the same binaries again.
//!
//! A pass that installed or kept a shim is **not** recorded, and `ShimKept` is the
//! one that says why: that arm is a documented residual (see [`super::provision`])
//! in which a container re-attempts one failing transfer on every `up`, and a
//! marker would turn "re-attempts on every up" into "never attempts again" — a
//! behaviour change wearing a cache's clothes. An install is followed by a later
//! pass that probes *provisioned*, and that pass writes the marker.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{
    ClaudeConfig, CodexSwitch, PassOccasion, StageResult, Switches, ToolsSwitch, ZellijSwitch,
};

use crate::clients::devpod_home::{DevpodHome, sole_workspace_result};

/// The directory the markers live in, under devlaunch's cache directory.
const MARKERS_DIR: &str = "tool-verdicts";

/// How long a verdict is believed across a container start.
///
/// The bound on the one stale verdict the container id cannot see: a change made
/// inside a container by hand. A week, so a workspace started every morning pays
/// one pass a week rather than one an `up`, and a container somebody broke by hand
/// is mended by the first `up` after the week, at the latest. `dl <ws> recreate`
/// and `dl <ws> dotfiles` mend it at once.
pub(crate) const MAX_CARRY_ACROSS_A_START: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Where the host remembers its verdicts, and what it checks them against.
///
/// Both fields are parameters rather than reads of the process environment, for the
/// reason [`super::provisioning_disabled`] gives: the decision is then a function of
/// its inputs, and a test can put a devpod home and a cache under a `tempdir`
/// without mutating an environment the whole binary shares. The binary resolves
/// both once, where it already resolves them for everything else.
///
/// `devpod_home` is `None` on a machine devpod's own records cannot be located on.
/// That is not a cache that fails: it is a cache that never trusts anything, because
/// the file every verdict is checked against is the one it cannot find.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerdictCache {
    markers_dir: PathBuf,
    devpod_home: Option<DevpodHome>,
}

impl VerdictCache {
    /// The cache devlaunch keeps under `cache_dir`, checked against `devpod_home`.
    pub fn under(cache_dir: &Path, devpod_home: Option<DevpodHome>) -> Self {
        Self {
            markers_dir: cache_dir.join(MARKERS_DIR),
            devpod_home,
        }
    }

    /// This workspace's marker file.
    ///
    /// The id goes into the name unescaped, as
    /// [`crate::flows::launch::Host::launch_lock_path`] already puts it into a lock
    /// file's name: devpod itself uses the id as a directory name under its own
    /// contexts, so an id that could not be a path component is one no workspace
    /// this could be asked about has.
    pub(super) fn marker(&self, workspace_id: &str) -> PathBuf {
        self.markers_dir.join(format!("{workspace_id}.json"))
    }

    /// The container standing for this workspace now, as devpod's records name it.
    fn anchor(&self, workspace_id: &str) -> Option<Anchor> {
        let result = sole_workspace_result(self.devpod_home.as_ref(), workspace_id)?;
        Anchor::of(&result)
    }

    /// Whether this workspace's remembered verdict still describes the container
    /// standing now, for a pass on `occasion`.
    ///
    /// Total, and false for every doubt — see the module note on why absent is the
    /// only harmless misreading.
    pub(crate) fn trusted(
        &self,
        workspace_id: &str,
        switches: Switches,
        occasion: PassOccasion,
    ) -> bool {
        self.trusted_at(workspace_id, switches, occasion, SystemTime::now())
    }

    /// [`Self::trusted`], with the clock as a parameter.
    fn trusted_at(
        &self,
        workspace_id: &str,
        switches: Switches,
        occasion: PassOccasion,
        now: SystemTime,
    ) -> bool {
        let Some(marker) = read_marker(&self.marker(workspace_id)) else {
            return false;
        };
        // Before the anchor, because it is the cheaper of the two and because it is
        // the one that is wrong about a container that really is standing: the
        // id can match perfectly while the marker describes a pass that skipped
        // the very stage this launch is asking for.
        if marker.switches != MarkerSwitches::of(switches) {
            return false;
        }
        let Some(standing) = self.anchor(workspace_id) else {
            return false;
        };
        if marker.container != standing.container {
            return false;
        }
        // The same container. Whether it has been started since the pass is the
        // mtime's question, and only a top-up can answer "no": the pass after an
        // `up` follows a start by definition, whatever the file says.
        let not_started_since = marker.result_mtime == standing.result_mtime;
        match occasion {
            PassOccasion::TopUp if not_started_since => true,
            PassOccasion::TopUp | PassOccasion::AfterUp => marker.survives_a_start(now),
        }
    }

    /// Drop this workspace's verdict, so the next pass travels.
    ///
    /// For the one change devlaunch makes inside a standing container that no file
    /// of devpod's records: a dotfiles refresh, which runs `chezmoi update` and
    /// `pixi global sync` and can add or take away the tools a verdict is about.
    /// Called *before* the refresh, so a refresh killed halfway still leaves no
    /// verdict behind. The Claude memo stays: it is about who mounted the config
    /// directory, which a refresh does not change.
    ///
    /// Silent, like every write here: a marker that could not be removed is one the
    /// next pass after an `up` may still trust, which is the bound the module note
    /// gives and no worse.
    pub(crate) fn forget(&self, workspace_id: &str) {
        let _ = std::fs::remove_file(self.marker(workspace_id));
    }

    /// Where the Claude memo for this workspace lives.
    fn memo(&self, workspace_id: &str) -> PathBuf {
        self.marker(workspace_id).with_extension("claude")
    }

    /// Remember who owns this container's Claude config directory, as the container
    /// [`Self::observe`] identified before the pass ran.
    ///
    /// Deliberately *not* a field on the verdict marker, and the difference is the
    /// question each answers. A marker answers "may this launch skip the trip", so
    /// it is written only for the verdicts worth remembering. This answers "who
    /// owns that directory", which a launch needs whether or not a trip was skipped
    /// and whether or not the container was ever provisioned. Folding the two
    /// together made a missing marker mean two different things, and the second one
    /// silently forwarded nothing.
    ///
    /// Anchored on the container id, as a marker is. A `devpod up` devlaunch did not
    /// run -- VS Code's, a hand-typed one, an older `dl` -- can rebuild the container
    /// without touching anything here, and a memo left standing would then describe
    /// a container that no longer exists. `ours` outliving its container is the
    /// direction that costs the host's credential. The id needs no mtime beside it
    /// here: who mounted the config directory is fixed when the container is
    /// created, and a start does not change it.
    ///
    /// Silent about every way of not working, exactly as [`Self::record`] is.
    pub(crate) fn remember_claude(
        &self,
        workspace_id: &str,
        claude: Option<ClaudeConfig>,
        observed: &Observed,
    ) {
        let memo = Memo {
            // Written, not skipped: "a pass ran and could not tell" is an answer,
            // and leaving the previous one in place would let a stale `ours` outlive
            // the container it was true of.
            claude: match claude {
                Some(ClaudeConfig::Ours) => MemoWord::Ours,
                Some(ClaudeConfig::Foreign) => MemoWord::Foreign,
                None => MemoWord::Unknown,
            },
            container: observed.0.container.clone(),
        };
        let Ok(text) = serde_json::to_string(&memo) else {
            return;
        };
        write_atomically(&self.memo(workspace_id), &text);
    }

    /// Whether any pass has recorded an answer for the container standing now.
    ///
    /// Distinct from [`Self::remembered_claude`] returning `None`, and the
    /// difference is what keeps two opposite mistakes apart. A memo saying "a pass
    /// ran and could not tell" must still let a launch skip its round trip; *no*
    /// memo must not, or a workspace whose verdict was recorded before this existed
    /// would skip the pass forever and never acquire one.
    pub fn has_claude_memo(&self, workspace_id: &str) -> bool {
        self.read_memo(workspace_id).is_some()
    }

    /// What the last pass saw of this workspace's Claude config directory.
    ///
    /// This is what keeps a launch that skips the round trip from skipping the
    /// decision the credential turns on, and it is also what serves the launch that
    /// never runs a pass at all: attaching to a workspace that is already up and
    /// finished creating goes straight to a session.
    ///
    /// `None` for every doubt, which is the reading that forwards no login: no memo
    /// yet, a truncated write, a word this build has never heard of, a memo whose
    /// container is not the one standing, and the recorded `unknown` itself.
    pub fn remembered_claude(&self, workspace_id: &str) -> Option<ClaudeConfig> {
        match self.read_memo(workspace_id)? {
            MemoWord::Ours => Some(ClaudeConfig::Ours),
            MemoWord::Foreign => Some(ClaudeConfig::Foreign),
            MemoWord::Unknown => None,
        }
    }

    /// This workspace's memo, if one parses and still describes the container that
    /// is standing.
    ///
    /// No `workspace_result.json` to compare against is itself a doubt, and doubt
    /// reads as no memo.
    fn read_memo(&self, workspace_id: &str) -> Option<MemoWord> {
        let text = std::fs::read_to_string(self.memo(workspace_id)).ok()?;
        let memo: Memo = serde_json::from_str(&text).ok()?;
        let standing = self.anchor(workspace_id)?;
        (memo.container == standing.container).then_some(memo.claude)
    }

    /// Which container a pass is about to be about, read **before** it runs.
    ///
    /// The whole of why this is a separate call. A pass is a `devpod ssh` that takes
    /// seconds, and nothing devlaunch holds keeps somebody else's `devpod up` — VS
    /// Code, a hand-typed one, a sibling `--recreate` — from completing inside that
    /// window. Read the anchor afterwards and the marker pairs the *new* container's
    /// id with the *old* container's verdict, which is a marker that matches until
    /// something else rebuilds: the silent misreading the module note says must
    /// never happen. Read it first and the same race writes an id that no longer
    /// matches, so the next launch travels — one redundant round trip, which is the
    /// direction that is allowed to be wrong.
    pub(crate) fn observe(&self, workspace_id: &str) -> Option<Observed> {
        self.anchor(workspace_id).map(Observed)
    }

    /// Remember that this workspace was found provisioned, or was just lent its
    /// tools, as the container [`Self::observe`] identified before the pass.
    ///
    /// `hostname` is how the pass's hostname stage went, which is what decides
    /// whether the verdict may carry across a start of this container: see
    /// [`Marker::survives_a_start`].
    ///
    /// Silent about every way of not working — a cache directory that will not be
    /// created, a write that fails. A verdict cache that could not write is a launch
    /// that pays what it pays today, and a launch is not worth failing over that.
    pub(crate) fn record(
        &self,
        workspace_id: &str,
        observed: &Observed,
        switches: Switches,
        hostname: StageResult,
    ) {
        self.record_at(
            workspace_id,
            observed,
            switches,
            hostname,
            SystemTime::now(),
        );
    }

    /// [`Self::record`], with the clock as a parameter.
    fn record_at(
        &self,
        workspace_id: &str,
        observed: &Observed,
        switches: Switches,
        hostname: StageResult,
        now: SystemTime,
    ) {
        let Some(recorded) = Stamp::at(now) else {
            return;
        };
        let marker = Marker {
            verdict: Verdict::Provisioned,
            container: observed.0.container.clone(),
            result_mtime: observed.0.result_mtime,
            switches: MarkerSwitches::of(switches),
            hostname: HostnameWord::of(hostname),
            recorded,
        };
        let Ok(text) = serde_json::to_string(&marker) else {
            return;
        };
        write_atomically(&self.marker(workspace_id), &text);
    }

    /// Where the in-flight record for this workspace lives.
    pub(super) fn in_flight(&self, workspace_id: &str) -> PathBuf {
        self.marker(workspace_id).with_extension("pass")
    }

    /// Note that a setup pass over the container [`Self::observe`] identified is
    /// about to run, and has not finished.
    ///
    /// Written before the pass's *first* trip and removed by [`Self::end_pass`]
    /// after its last, so the file surviving a launch is the one thing on this host
    /// that says the pass was cut short. A pass is up to three trips and the probe
    /// is the short one, so a record that closed when the probe answered would be
    /// open for ~1.7s of a pass that runs for minutes. `dl`'s signal handler `_exit`s without unwinding
    /// (`devlaunch_runner::interrupt`), which is exactly why the evidence has to be
    /// a file left standing rather than one written on the way out: nothing runs on
    /// the way out.
    ///
    /// Anchored on the container id like everything else here, and against the
    /// same failure: a record left by an interrupted pass over a container that has
    /// since been rebuilt describes a container that is not standing, and reading
    /// it would send every later launch on a round trip it does not owe. A start
    /// does not end it: the pass that was cut short is still owed to the container
    /// that was started.
    ///
    /// Silent about every way of not working, as [`Self::record`] is. A record that
    /// could not be written costs one interrupted launch its recovery and nothing
    /// else -- which is today's behaviour, exactly.
    pub(crate) fn begin_pass(&self, workspace_id: &str, observed: &Observed) {
        let in_flight = InFlight {
            container: observed.0.container.clone(),
        };
        let Ok(text) = serde_json::to_string(&in_flight) else {
            return;
        };
        write_atomically(&self.in_flight(workspace_id), &text);
    }

    /// The pass [`Self::begin_pass`] opened has finished.
    ///
    /// Called for a pass that got through to the container, however it turned out:
    /// an install that ran and failed is a pass that happened, and re-running it on
    /// every attach afterwards is the "re-attempts forever" behaviour the module
    /// note refuses for [`Provisioning::ShimKept`](super::Provisioning::ShimKept).
    /// A trip the OS would not make is the other thing and is not called for here --
    /// see [`Provisioning::reached_the_container`](super::Provisioning::reached_the_container),
    /// which is where the two are told apart.
    pub(crate) fn end_pass(&self, workspace_id: &str) {
        let _ = std::fs::remove_file(self.in_flight(workspace_id));
    }

    /// Whether a setup pass over the container standing now began and never
    /// finished.
    ///
    /// **Positive evidence, and it has to be.** The opposite reading -- "no record
    /// of a finished pass, so run one" -- is true of every workspace `dl` never
    /// provisioned: one created by VS Code, by a hand-typed `devpod up`, or by a
    /// build older than this file. Those must keep attaching in one round trip, so
    /// the question asked here is the narrow one, and every doubt answers *no*: no
    /// record, a record that will not parse, or one whose container is not the one
    /// standing.
    pub(crate) fn pass_never_finished(&self, workspace_id: &str) -> bool {
        let Some(text) = std::fs::read_to_string(self.in_flight(workspace_id)).ok() else {
            return false;
        };
        let Ok(in_flight) = serde_json::from_str::<InFlight>(&text) else {
            return false;
        };
        let Some(standing) = self.anchor(workspace_id) else {
            return false;
        };
        in_flight.container == standing.container
    }
}

/// Which container `workspace_result.json` names, and when devpod last wrote it.
///
/// Read from one open file, so the two halves describe one write of it: devpod
/// replaces the file by rename, and the handle keeps the file it opened.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Anchor {
    /// `ContainerDetails.ID`: which container. Moves on a create, a `--recreate`
    /// and a `--reset`, and on nothing else.
    container: String,
    /// The result file's mtime: moves on every completed `up`, which is to say on
    /// every start of the container as well as every rebuild.
    result_mtime: Stamp,
}

impl Anchor {
    /// The anchor `result` describes, or nothing when it describes none.
    ///
    /// Nothing for a file that will not open or parse, and for one with no
    /// container id or an empty one: an id that is not there cannot be told apart
    /// from the next container's, so it must not match anything.
    fn of(result: &Path) -> Option<Self> {
        let mut file = std::fs::File::open(result).ok()?;
        let result_mtime = Stamp::of_file(&file)?;
        let mut text = String::new();
        file.read_to_string(&mut text).ok()?;
        let document: ResultDocument = serde_json::from_str(&text).ok()?;
        let container = document.container?.id?;
        if container.is_empty() {
            return None;
        }
        Some(Self {
            container,
            result_mtime,
        })
    }
}

/// The one field of devpod's `workspace_result.json` this module reads.
///
/// Spelled as devpod writes it. Every other field is ignored, so a devpod that
/// adds one changes nothing here, and one that drops this one makes every marker
/// untrusted, which is the safe way to be wrong.
#[derive(Deserialize)]
struct ResultDocument {
    #[serde(rename = "ContainerDetails")]
    container: Option<ContainerDetails>,
}

/// `ContainerDetails`, narrowed to its id.
#[derive(Deserialize)]
struct ContainerDetails {
    #[serde(rename = "ID")]
    id: Option<String>,
}

/// What one in-flight record says.
///
/// One field, and a struct rather than the bare id it wraps, for [`Marker`]'s
/// reason: the check is the parse, so a file a later build writes with a second
/// field it needs is not silently read as this one. A record written before the
/// container id was the anchor has no `container` and is not read at all.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct InFlight {
    container: String,
}

/// The container a pass is about, as [`VerdictCache::observe`] read it.
///
/// A type rather than a bare [`Anchor`] so that [`VerdictCache::record`] cannot be
/// handed an anchor read at the wrong moment: the only way to make one is to ask
/// before the pass, which is the ordering the whole trust rule rests on.
pub(crate) struct Observed(Anchor);

/// What one Claude memo file says.
///
/// Its own file rather than a field on [`Marker`], for the reason
/// [`VerdictCache::remember_claude`] gives, and its own anchor because a memo that
/// outlives its container is the one misreading that costs something.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct Memo {
    claude: MemoWord,
    container: String,
}

/// The three things a pass can have concluded about the config directory.
///
/// An enum with serde's spelling rather than a `String` matched against three
/// literals, for the reason [`Verdict`] is one: the check is the parse, so a word a
/// later build writes, or a hand-edit, fails to deserialize and reads as no memo at
/// all. `Unknown` is a recorded answer and not the absence of one --
/// [`VerdictCache::has_claude_memo`] says why the two must stay apart -- which is
/// why it is an arm here and `None` at the call site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
enum MemoWord {
    #[serde(rename = "ours")]
    Ours,
    #[serde(rename = "foreign")]
    Foreign,
    #[serde(rename = "unknown")]
    Unknown,
}

/// What one marker file says.
///
/// A marker written before the container id was the anchor has no `container` key,
/// fails to deserialize, and is therefore untrusted: one redundant round trip on
/// the first launch after upgrading.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct Marker {
    verdict: Verdict,
    /// Which container the verdict is about. The identity check.
    container: String,
    /// The result file's mtime when the pass ran: whether the container has been
    /// started since.
    result_mtime: Stamp,
    /// The switches the pass ran under. Without this the marker answers "was this
    /// container provisioned?" when the question the flow asks it is "was it
    /// provisioned *the way this launch wants*?" — and those differ exactly when the
    /// two launches asked for different things. A pass that carried no zellij stage
    /// probes provisioned, because zellij is not what the probe is about, so it
    /// wrote a marker that a later launch asking for zellij trusted: zellij was
    /// never installed, and no top-up would ever notice, because the marker said
    /// there was nothing to do.
    ///
    /// Since #391 that is the *common* case rather than an opt-out's, and it is what
    /// makes `DEVLAUNCH_ZELLIJ=1` work on a workspace that is already up: the
    /// marker written by every launch before it disagrees with the one this launch
    /// wants, so the pass travels again and the stage lands.
    switches: MarkerSwitches,
    /// How the pass's hostname stage went.
    hostname: HostnameWord,
    /// When the pass ran, by the host's clock: the start of
    /// [`MAX_CARRY_ACROSS_A_START`].
    recorded: Stamp,
}

impl Marker {
    /// Whether this verdict still holds for its container after the container has
    /// been started again.
    ///
    /// Only when the pass's `sudo hostname` was refused. A container that refused
    /// it once refuses it on every start, because the capability it lacks is fixed
    /// when the container is created, so the pass after a start would ask again and
    /// be refused again. A hostname that was set has to be set again, and a stage
    /// that did not report is read as one that was set, since that is the reading
    /// that travels.
    ///
    /// And only for [`MAX_CARRY_ACROSS_A_START`] after the pass, the bound on what
    /// the id cannot see. A recorded time in the future is a clock that stepped
    /// back, and is read as expired.
    fn survives_a_start(&self, now: SystemTime) -> bool {
        let refused = matches!(self.hostname, HostnameWord::Refused);
        let young = now
            .duration_since(SystemTime::from(self.recorded))
            .is_ok_and(|age| age < MAX_CARRY_ACROSS_A_START);
        refused && young
    }
}

/// How a pass's hostname stage went, in the marker's own spelling.
///
/// Three words for [`StageResult`]'s three arms, and not the exit status: what the
/// marker needs is whether the name has to be set again after a start, and the
/// status adds nothing to that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
enum HostnameWord {
    #[serde(rename = "set")]
    Set,
    #[serde(rename = "refused")]
    Refused,
    #[serde(rename = "unreported")]
    Unreported,
}

impl HostnameWord {
    fn of(stage: StageResult) -> Self {
        match stage {
            StageResult::Ok => Self::Set,
            StageResult::Failed { .. } => Self::Refused,
            StageResult::NotReached => Self::Unreported,
        }
    }
}

/// The switches a marker was written under, in the marker's own spelling.
///
/// Deliberately not [`Switches`] itself. That type is `pub`, and giving it serde
/// derives would put the on-disk format of a cache file into the frozen surface,
/// where a rename becomes a breaking change to something no consumer ever asked
/// for. Two bools in a private struct say the same thing and belong to this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct MarkerSwitches {
    tools: bool,
    zellij: bool,
    codex: bool,
}

impl MarkerSwitches {
    fn of(switches: Switches) -> Self {
        Self {
            tools: matches!(switches.tools, ToolsSwitch::Install),
            zellij: matches!(switches.zellij, ZellijSwitch::Install),
            // Part of the key, which is what makes a workspace provisioned by
            // `dl <ws>` re-provision under `aid --codex <ws>`: the marker written
            // without codex disagrees with the switches this launch wants, so the
            // pass travels again and the codex stage lands. See
            // `provision::CodexSwitch`.
            codex: matches!(switches.codex, CodexSwitch::Install),
        }
    }
}

/// The one verdict worth remembering.
///
/// An enum with a single arm rather than a `String` compared against
/// `"provisioned"`, so the check is the parse: a marker carrying any other word —
/// one a later build writes, one a hand-edit put there — fails to deserialize and
/// is therefore untrusted, which is the reading every unknown has to get. A string
/// field would make that a comparison somebody has to remember to write, and
/// forgetting it trusts every future verdict this build has never heard of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
enum Verdict {
    #[serde(rename = "provisioned")]
    Provisioned,
}

/// One instant, as two integers.
///
/// Not a `SystemTime` serialized by serde's own representation, and not a float:
/// the value is compared for exact equality against a timestamp read back from a
/// filesystem, and a float round trip through JSON is where nanoseconds go to be
/// approximately preserved. Two integers compare exactly and read plainly in a file
/// somebody may well open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct Stamp {
    secs: u64,
    nanos: u32,
}

impl Stamp {
    /// This path's mtime, or nothing when there is not one to read.
    #[cfg(test)]
    fn of(path: &Path) -> Option<Self> {
        Self::of_file(&std::fs::File::open(path).ok()?)
    }

    /// This open file's mtime, or nothing when there is not one to read.
    fn of_file(file: &std::fs::File) -> Option<Self> {
        Self::at(file.metadata().and_then(|meta| meta.modified()).ok()?)
    }

    /// `instant`, or nothing for an instant *before* the epoch.
    ///
    /// It is not representable here, and the alternative — clamping it to zero —
    /// makes every such file share one timestamp, which is the one value that must
    /// not be shared: two different container generations would then compare equal.
    fn at(instant: SystemTime) -> Option<Self> {
        let since = instant.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            secs: since.as_secs(),
            nanos: since.subsec_nanos(),
        })
    }
}

impl From<Stamp> for SystemTime {
    fn from(stamp: Stamp) -> Self {
        UNIX_EPOCH + Duration::new(stamp.secs, stamp.nanos)
    }
}

/// One marker, or nothing when there is nothing readable there.
fn read_marker(path: &Path) -> Option<Marker> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// *text* into *path*, via a temp file in the same directory.
///
/// The same shape [`crate::flows::completion_cache`] writes its caches with —
/// staged under a name carrying the writer's pid, then renamed — and written out
/// again here rather than shared with it, because that function's docstring is an
/// argument about Python's `Path.with_suffix` and about two cache files that must
/// not stage through one name. Nothing about a marker is Python's. What the two
/// have in common is only the property, which is the part that matters: a reader
/// arriving mid-write sees the old marker or the new one, never half of either, and
/// half a marker is a marker that does not parse and so is untrusted anyway.
///
/// Silent: every caller here is best-effort.
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
    //! What these pin is one relation: a marker is trusted exactly when it names
    //! the container that is standing. Every test here is a way of that not being
    //! so — and each of them has to come out *untrusted*, because a false trust is
    //! the failure mode that costs a workspace its tools silently.

    use std::time::Duration;

    use super::*;
    use crate::clients::devpod_home::{ScratchHome, devpod_home_with};

    /// A cache directory nothing else writes to.
    fn cache() -> tempfile::TempDir {
        tempfile::tempdir().expect("a scratch cache directory")
    }

    /// The result file `devpod_home_with` wrote for this workspace, asked of the
    /// home itself rather than rebuilt: devpod's layout is
    /// `clients::devpod_home`'s to spell, and a fixture that spelled its own copy
    /// would go on passing after devpod moved it.
    fn result_in(home: &DevpodHome, workspace_id: &str) -> PathBuf {
        home.result("default", workspace_id)
    }

    /// The pair the flow always calls together: observe the container, then record
    /// the verdict about it. Nothing to observe is nothing to record, which is the
    /// same silence the flow's own `if let` makes.
    fn recorded(verdicts: &VerdictCache, workspace_id: &str) {
        recorded_under(verdicts, workspace_id, Switches::INSTALLING);
    }

    /// The same pair, for the tests that are about *which* pass wrote the marker.
    fn recorded_under(verdicts: &VerdictCache, workspace_id: &str, switches: Switches) {
        if let Some(observed) = verdicts.observe(workspace_id) {
            verdicts.record(workspace_id, &observed, switches, StageResult::Ok);
        }
    }

    /// A cache over a devpod home holding one workspace with a result file, which is
    /// the anchor a memo needs.
    fn anchored() -> (tempfile::TempDir, ScratchHome, VerdictCache) {
        let home = devpod_home_with(&[("default", "ws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));
        (cache, home, verdicts)
    }

    /// Open a pass over the container standing now, as [`provision`] does before
    /// its trip -- and, unlike [`provision`], never close it.
    fn began(verdicts: &VerdictCache, workspace_id: &str) {
        let observed = verdicts.observe(workspace_id).expect("an anchor");
        verdicts.begin_pass(workspace_id, &observed);
    }

    /// Remember `seen` against the container standing now, as [`provision`] does.
    fn remembered_under(verdicts: &VerdictCache, workspace_id: &str, seen: Option<ClaudeConfig>) {
        let observed = verdicts.observe(workspace_id).expect("an anchor");
        verdicts.remember_claude(workspace_id, seen, &observed);
    }

    #[test]
    fn what_the_pass_saw_of_the_claude_config_outlives_the_pass() {
        // What keeps a launch that skips the round trip, and a launch that runs no
        // pass at all, from skipping the decision the credential turns on.
        for (seen, expected) in [
            (Some(ClaudeConfig::Ours), Some(ClaudeConfig::Ours)),
            (Some(ClaudeConfig::Foreign), Some(ClaudeConfig::Foreign)),
            (None, None),
        ] {
            let (_cache, _home, verdicts) = anchored();
            remembered_under(&verdicts, "ws", seen);
            assert_eq!(verdicts.remembered_claude("ws"), expected, "{seen:?}");
            assert!(
                verdicts.has_claude_memo("ws"),
                "a pass that could not tell still recorded an answer: {seen:?}"
            );
        }
    }

    #[test]
    fn a_workspace_that_was_only_lent_its_tools_still_remembers() {
        // The case that made this a memo rather than a field on the marker. A
        // container the host lent its binaries to never probes provisioned, so it has
        // no marker at all -- and it is the very container the credential exists for,
        // since a repo with no devcontainer of its own is what gets lent to.
        let (_cache, _home, verdicts) = anchored();

        remembered_under(&verdicts, "ws", Some(ClaudeConfig::Ours));

        assert!(
            !verdicts.trusted("ws", Switches::INSTALLING, PassOccasion::TopUp),
            "no verdict was recorded, so the next pass must still travel"
        );
        assert_eq!(
            verdicts.remembered_claude("ws"),
            Some(ClaudeConfig::Ours),
            "and the session it opens still knows whose config directory that is"
        );
    }

    #[test]
    fn a_later_pass_that_cannot_tell_overwrites_an_earlier_answer() {
        // Written rather than skipped: leaving the previous answer in place would let
        // a stale `ours` outlive the container it was true of, and forward a login
        // into a directory that had since been mounted from somewhere else.
        let (_cache, _home, verdicts) = anchored();
        remembered_under(&verdicts, "ws", Some(ClaudeConfig::Ours));
        remembered_under(&verdicts, "ws", None);
        assert_eq!(verdicts.remembered_claude("ws"), None);
        assert!(
            verdicts.has_claude_memo("ws"),
            "`unknown` is a recorded answer, so the top-up shortcut still holds"
        );
    }

    #[test]
    fn a_memo_does_not_outlive_the_container_it_was_true_of() {
        // The anchor, and the reason it is here. A `devpod up` devlaunch did not run
        // -- VS Code's, a hand-typed one, an older `dl` -- rebuilds the container
        // without touching this cache. Unanchored, the `ours` written for the old
        // container stood, and every warm attach afterwards forwarded the host's
        // login into a directory the new container may well have mounted.
        let home = devpod_home_with(&[("default", "ws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));
        remembered_under(&verdicts, "ws", Some(ClaudeConfig::Ours));
        assert_eq!(verdicts.remembered_claude("ws"), Some(ClaudeConfig::Ours));

        // Somebody else's rebuild, which is a result file naming a new container.
        rebuild_result(&home, "ws");

        assert_eq!(
            verdicts.remembered_claude("ws"),
            None,
            "a memo whose container is gone is not an answer about the one standing"
        );
        assert!(
            !verdicts.has_claude_memo("ws"),
            "and the next top-up has to travel to write a new one"
        );
    }

    /// Somebody else's `devpod up --recreate`: the same workspace, a result file
    /// naming a container that was not there before.
    fn rebuild_result(home: &ScratchHome, workspace_id: &str) {
        let result = home.result("default", workspace_id);
        let document = serde_json::json!({
            "ContainerDetails": { "ID": format!("rebuilt-{workspace_id}") },
        });
        std::fs::write(&result, document.to_string()).expect("a result");
    }

    #[test]
    fn a_memo_this_build_cannot_read_forwards_nothing() {
        let (cache, _home, verdicts) = anchored();
        assert_eq!(verdicts.remembered_claude("never-asked"), None);
        remembered_under(&verdicts, "ws", Some(ClaudeConfig::Ours));
        let memo = memo_file(cache.path());
        for garbage in [
            "",
            "ours",
            "OURS",
            "a word from a later build",
            r#"{"claude":"maybe","result_mtime":{"secs":1,"nanos":0}}"#,
        ] {
            std::fs::write(&memo, garbage).expect("a write");
            assert_eq!(
                verdicts.remembered_claude("ws"),
                None,
                "{garbage:?} must not read as an answer"
            );
        }
    }

    /// The one memo file under a scratch cache, wherever this module puts it.
    fn memo_file(cache: &Path) -> PathBuf {
        fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else {
                    found.push(path);
                }
            }
        }
        let mut found = Vec::new();
        walk(cache, &mut found);
        assert_eq!(found.len(), 1, "expected one file, found {found:?}");
        found.pop().expect("the memo")
    }

    #[test]
    fn a_marker_written_by_a_pass_without_zellij_is_not_trusted_by_one_that_wants_it() {
        // The defect this field exists for, and since #391 the mechanism the opt-in
        // rests on. A launch that did not ask for zellij runs a pass carrying no
        // zellij stage, and that pass *probes provisioned* -- rightly, because the
        // probe is about the tools, not about zellij. So it wrote a marker, and the
        // marker said only "this container, provisioned".
        //
        // The next launch sets `DEVLAUNCH_ZELLIJ=1` and wants the stage. Without
        // this field it would find a trusted marker for a container that had never
        // stopped, skip the trip, and never get zellij -- and nothing later would
        // notice, because every subsequent top-up reads the same marker and skips
        // the same trip: the failure is permanent for the life of the container and
        // completely silent.
        let home = devpod_home_with(&[("default", "ws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));

        let unasked = Switches {
            codex: CodexSwitch::Skip,
            tools: ToolsSwitch::Install,
            zellij: ZellijSwitch::Skip,
        };
        recorded_under(&verdicts, "ws", unasked);

        assert!(
            verdicts.trusted("ws", unasked, PassOccasion::TopUp),
            "the pass that wrote it asks the same question and must still be answered"
        );
        assert!(
            !verdicts.trusted("ws", Switches::INSTALLING, PassOccasion::TopUp),
            "a launch that wants the zellij stage was told a pass that skipped it had already run"
        );
    }

    #[test]
    fn a_marker_from_a_build_that_did_not_record_switches_is_not_trusted() {
        // Forward compatibility in the only direction that is safe. A marker on
        // disk from 0.12.0 or earlier has no `switches` key, so it fails to parse
        // and reads as absent -- one redundant round trip on the first launch after
        // upgrading. The other direction, treating a keyless marker as a full
        // install, would silently re-open the defect above for every container that
        // already exists.
        let home = devpod_home_with(&[("default", "ws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));
        recorded(&verdicts, "ws");
        assert!(verdicts.trusted("ws", Switches::INSTALLING, PassOccasion::TopUp));

        let marker = cache.path().join(MARKERS_DIR).join("ws.json");
        let text = std::fs::read_to_string(&marker).expect("the marker just written");
        let old: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        let mut old = old.as_object().expect("an object").clone();
        old.remove("switches");
        std::fs::write(
            &marker,
            serde_json::to_string(&old).expect("re-serialisable"),
        )
        .expect("a writable cache");

        assert!(
            !verdicts.trusted("ws", Switches::INSTALLING, PassOccasion::TopUp),
            "a marker this build cannot read in full is a marker it must not act on"
        );
    }

    /// Move `path`'s mtime forward, the way a completed `devpod up` moves the
    /// result file's — by hand, because two writes inside one test can land in the
    /// same filesystem timestamp tick.
    fn rewritten(path: &Path, by: Duration) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("the file to restamp");
        let was = file
            .metadata()
            .expect("its metadata")
            .modified()
            .expect("an mtime");
        file.set_modified(was + by).expect("a moved mtime");
    }

    #[test]
    fn a_recorded_verdict_is_trusted_while_the_container_stands() {
        let home = devpod_home_with(&[("default", "myws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));

        assert!(
            !verdicts.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp),
            "nothing is recorded yet"
        );
        recorded(&verdicts, "myws");

        assert!(verdicts.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp));
        // And it survives being read by a second value over the same directories,
        // which is what "the next `dl` process" is.
        let later = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));
        assert!(later.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp));
    }

    #[test]
    fn the_marker_says_what_it_is_in_words_a_person_can_read() {
        // The file is under a user's cache directory and somebody will open it. It
        // is also the format a later build has to keep reading, so the two field
        // names and the verdict word are pinned rather than left to serde's whim.
        let home = devpod_home_with(&[("default", "myws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));

        recorded(&verdicts, "myws");

        let text = std::fs::read_to_string(cache.path().join(MARKERS_DIR).join("myws.json"))
            .expect("a marker");
        assert!(text.contains(r#""verdict":"provisioned""#), "{text}");
        assert!(
            text.contains(r#""container":"container-default-myws""#),
            "{text}"
        );
        assert!(text.contains(r#""hostname":"set""#), "{text}");
        assert!(text.contains(r#""recorded":{"secs":"#), "{text}");
        assert!(text.contains(r#""result_mtime":{"secs":"#), "{text}");
        assert!(text.contains(r#""nanos":"#), "{text}");
    }

    #[test]
    fn a_completed_up_moves_the_result_and_the_verdict_stops_being_trusted() {
        // The invalidation the whole design hangs off: devpod rewrites
        // `workspace_result.json` on its way out of every completed `up`, so a
        // rebuild — by `dl`, by VS Code, by hand — is a moved mtime and a marker
        // that no longer matches.
        let home = devpod_home_with(&[("default", "myws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));
        recorded(&verdicts, "myws");
        assert!(verdicts.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp));

        rewritten(&result_in(&home, "myws"), Duration::from_secs(30));

        assert!(!verdicts.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp));
    }

    #[test]
    fn a_result_stamped_backwards_is_not_trusted_either() {
        // Equality, not "no newer than": a devpod home restored from a backup or
        // copied off another machine moves the mtime somewhere that is not newer,
        // and it is still not the container the verdict was about.
        let home = devpod_home_with(&[("default", "myws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));
        recorded(&verdicts, "myws");

        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(result_in(&home, "myws"))
            .expect("the result file");
        let was = file
            .metadata()
            .expect("its metadata")
            .modified()
            .expect("an mtime");
        file.set_modified(was - Duration::from_secs(30))
            .expect("a backdated mtime");

        assert!(!verdicts.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp));
    }

    #[test]
    fn nothing_unreadable_is_ever_trusted() {
        // Six ways to have no answer, and every one of them has to read the same:
        // run the pass. A marker that is not there, one that is not JSON, one whose
        // fields are the wrong shape, one carrying a verdict word this build does
        // not know, one whose workspace has no result file, and one whose id is
        // held by two devpod contexts at once.
        let home = devpod_home_with(&[("default", "myws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));
        recorded(&verdicts, "myws");
        let marker = cache.path().join(MARKERS_DIR).join("myws.json");
        let good = std::fs::read_to_string(&marker).expect("a marker");

        for garbled in [
            "",
            "not json at all",
            r#"{"verdict":"provisioned"}"#,
            r#"{"verdict":"provisioned","result_mtime":"yesterday"}"#,
            r#"{"verdict":"lent","result_mtime":{"secs":1,"nanos":0}}"#,
        ] {
            std::fs::write(&marker, garbled).expect("a garbled marker");
            assert!(
                !verdicts.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp),
                "{garbled:?}"
            );
        }

        std::fs::write(&marker, &good).expect("the good marker back");
        assert!(
            verdicts.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp),
            "the fixture itself still holds"
        );
        std::fs::remove_file(&marker).expect("the marker removed");
        assert!(!verdicts.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp));
    }

    #[test]
    fn an_ambiguous_or_unfindable_workspace_is_not_trusted() {
        // The three ways `sole_workspace_result` declines to answer: two
        // contexts holding one id, a record with no result beside it (an `up` that
        // died in its hooks), and no devpod home to look in at all. Each leaves the
        // host with no file whose mtime it can key on.
        let ambiguous =
            devpod_home_with(&[("default", "myws", Some(())), ("other", "myws", Some(()))]);
        let unfinished = devpod_home_with(&[("default", "myws", None)]);
        let cache = cache();

        for home in [
            Some(DevpodHome::at(ambiguous.path())),
            Some(DevpodHome::at(unfinished.path())),
            None,
        ] {
            let verdicts = VerdictCache::under(cache.path(), home.clone());
            recorded(&verdicts, "myws");
            assert!(
                !verdicts.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp),
                "{home:?}"
            );
        }
    }

    #[test]
    fn a_workspace_with_no_result_file_records_nothing_to_be_trusted_later() {
        // Recording has the same anchor reading does, so a pass over a workspace
        // devpod never finished creating writes no marker at all -- rather than one
        // that a result file appearing later would silently validate.
        let home = devpod_home_with(&[("default", "myws", None)]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));

        recorded(&verdicts, "myws");

        assert!(!cache.path().join(MARKERS_DIR).join("myws.json").exists());
    }

    #[test]
    fn one_workspaces_verdict_says_nothing_about_another() {
        let home = devpod_home_with(&[
            ("default", "myws", Some(())),
            ("default", "other", Some(())),
        ]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));

        recorded(&verdicts, "myws");

        assert!(verdicts.trusted("myws", Switches::INSTALLING, PassOccasion::TopUp));
        assert!(!verdicts.trusted("other", Switches::INSTALLING, PassOccasion::TopUp));
    }

    #[test]
    fn a_pass_that_began_and_never_ended_is_still_unfinished() {
        // The whole of what this file adds. A `dl` killed between `devpod up` and
        // the end of its setup pass leaves a container devpod calls Running and
        // finished creating, so every later launch fast-attaches to it and the pass
        // never runs again. Nothing else on the host can tell that apart from a
        // container whose pass ran to completion, because the cache records only
        // success and an absent record is what a workspace dl never touched has too.
        let (_cache, _home, verdicts) = anchored();

        began(&verdicts, "ws");

        assert!(verdicts.pass_never_finished("ws"));
    }

    #[test]
    fn an_interrupted_pass_over_a_container_since_rebuilt_asks_for_nothing() {
        // The anchor earns its keep here as much as anywhere. A `--recreate` after
        // the interruption is a whole new container with a pass of its own, and a
        // record left by the old one would put a round trip on every attach from
        // then on -- for a pass that already ran, over a container that is gone.
        let (_cache, home, verdicts) = anchored();
        began(&verdicts, "ws");
        assert!(verdicts.pass_never_finished("ws"));

        rebuild_result(&home, "ws");

        assert!(!verdicts.pass_never_finished("ws"));
    }

    #[test]
    fn an_interrupted_pass_is_still_owed_after_a_start() {
        // A stop and an `up` keep the container, so the pass cut short is still
        // owed to it. The mtime-keyed record forgot it on the start.
        let (_cache, home, verdicts) = anchored();
        began(&verdicts, "ws");

        rewritten(&result_in(&home, "ws"), Duration::from_secs(30));

        assert!(verdicts.pass_never_finished("ws"));
    }

    #[test]
    fn nothing_unreadable_asks_for_a_pass_either() {
        // Every doubt reads as "no interrupted pass", which is the direction that
        // costs nothing: a workspace dl never provisioned has no record at all, and
        // it must go on attaching in one round trip.
        let (cache, _home, verdicts) = anchored();
        assert!(!verdicts.pass_never_finished("ws"), "no record");

        let path = cache.path().join(MARKERS_DIR).join("ws.pass");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
        for text in ["", "{", "{}", "null", r#"{"result_mtime": "yesterday"}"#] {
            std::fs::write(&path, text).expect("the record");
            assert!(!verdicts.pass_never_finished("ws"), "{text:?}");
        }
    }

    #[test]
    fn a_host_that_cannot_find_the_container_asks_for_nothing() {
        // No `workspace_result.json` to compare against is a doubt like any other.
        // It is also what a scratch `XDG_CACHE_HOME` and a machine with no devpod
        // home both look like, and neither is a launch that owes a round trip.
        let unfinished = devpod_home_with(&[("default", "ws", None)]);
        let cache = cache();
        for home in [Some(DevpodHome::at(unfinished.path())), None] {
            let verdicts = VerdictCache::under(cache.path(), home.clone());
            std::fs::create_dir_all(cache.path().join(MARKERS_DIR)).expect("the directory");
            std::fs::write(
                cache.path().join(MARKERS_DIR).join("ws.pass"),
                r#"{"result_mtime":{"secs":1,"nanos":0}}"#,
            )
            .expect("the record");
            assert!(!verdicts.pass_never_finished("ws"), "{home:?}");
        }
    }

    #[test]
    fn one_workspaces_interrupted_pass_says_nothing_about_another() {
        let home = devpod_home_with(&[("default", "ws", Some(())), ("default", "other", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));

        began(&verdicts, "ws");

        assert!(verdicts.pass_never_finished("ws"));
        assert!(!verdicts.pass_never_finished("other"));
    }

    #[test]
    fn a_pass_that_ended_left_nothing_to_finish() {
        let (_cache, _home, verdicts) = anchored();

        began(&verdicts, "ws");
        verdicts.end_pass("ws");

        assert!(!verdicts.pass_never_finished("ws"));
    }

    #[test]
    fn a_stamp_is_the_mtime_it_was_read_from() {
        // The round trip the equality check depends on: what `Stamp::of` reads back
        // is the instant the file carries, to the nanosecond the filesystem kept.
        let home = devpod_home_with(&[("default", "myws", Some(()))]);
        let result = result_in(&home, "myws");
        let stamp = Stamp::of(&result).expect("an mtime");

        let modified = std::fs::metadata(&result)
            .expect("its metadata")
            .modified()
            .expect("an mtime");
        assert_eq!(SystemTime::from(stamp), modified);
        assert_eq!(Stamp::of(Path::new("/no/such/file")), None);
    }

    /// Record a verdict about the container standing now, with the hostname stage
    /// having gone `hostname`, at `when`.
    fn recorded_with(verdicts: &VerdictCache, hostname: StageResult, when: SystemTime) {
        let observed = verdicts.observe("myws").expect("an anchor");
        verdicts.record_at("myws", &observed, Switches::INSTALLING, hostname, when);
    }

    /// One workspace, created and finished, and a cache over it.
    fn one_workspace() -> (ScratchHome, tempfile::TempDir, VerdictCache) {
        let home = devpod_home_with(&[("default", "myws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));
        (home, cache, verdicts)
    }

    /// Both occasions, asked at `now`.
    fn trusted_on_each(verdicts: &VerdictCache, now: SystemTime) -> [bool; 2] {
        [PassOccasion::TopUp, PassOccasion::AfterUp]
            .map(|occasion| verdicts.trusted_at("myws", Switches::INSTALLING, occasion, now))
    }

    #[test]
    fn a_stopped_and_started_container_keeps_a_verdict_whose_hostname_was_refused() {
        // The saving. A stop and an `up` keep the container id and move only the
        // result file's mtime. A pass whose `sudo hostname` was refused has nothing
        // to redo after a start, so the pass after the `up` may be skipped.
        let (home, _cache, verdicts) = one_workspace();
        let now = SystemTime::now();
        recorded_with(&verdicts, StageResult::Failed { status: 1 }, now);

        rewritten(&result_in(&home, "myws"), Duration::from_secs(30));

        assert_eq!(trusted_on_each(&verdicts, now), [true, true]);
    }

    #[test]
    fn a_hostname_that_was_set_has_to_be_set_again_after_a_start() {
        // The UTS name is rebuilt from the container's config on every start, so a
        // pass that set it owes the started container another pass, whatever the
        // verdict says about the tools.
        for hostname in [StageResult::Ok, StageResult::NotReached] {
            let (home, _cache, verdicts) = one_workspace();
            let now = SystemTime::now();
            recorded_with(&verdicts, hostname, now);
            assert_eq!(
                trusted_on_each(&verdicts, now),
                [true, false],
                "not started yet, and the pass after an `up` always follows a start: {hostname:?}"
            );

            rewritten(&result_in(&home, "myws"), Duration::from_secs(30));

            assert_eq!(
                trusted_on_each(&verdicts, now),
                [false, false],
                "{hostname:?}"
            );
        }
    }

    #[test]
    fn a_rebuilt_container_is_never_the_one_a_verdict_was_about() {
        // A create, a `--recreate` and a `--reset` all make a new container id,
        // and no verdict carries to it, however its hostname stage went.
        let (home, _cache, verdicts) = one_workspace();
        let now = SystemTime::now();
        recorded_with(&verdicts, StageResult::Failed { status: 1 }, now);

        rebuild_result(&home, "myws");

        assert_eq!(trusted_on_each(&verdicts, now), [false, false]);
    }

    #[test]
    fn a_verdict_carries_across_a_start_for_a_week_at_most() {
        // The bound on what the id cannot see: a tool somebody removed inside the
        // container by hand. After the bound, the pass after an `up` travels and
        // finds out. A container that never stopped keeps its old bargain.
        let (home, _cache, verdicts) = one_workspace();
        let then = SystemTime::now();
        recorded_with(&verdicts, StageResult::Failed { status: 1 }, then);
        let just_inside = then + MAX_CARRY_ACROSS_A_START - Duration::from_secs(1);
        let past = then + MAX_CARRY_ACROSS_A_START;

        assert_eq!(trusted_on_each(&verdicts, just_inside), [true, true]);
        assert_eq!(
            trusted_on_each(&verdicts, past),
            [true, false],
            "a top-up over a container never started since keeps trusting"
        );

        rewritten(&result_in(&home, "myws"), Duration::from_secs(30));
        assert_eq!(trusted_on_each(&verdicts, just_inside), [true, true]);
        assert_eq!(trusted_on_each(&verdicts, past), [false, false]);
    }

    #[test]
    fn a_verdict_recorded_in_the_future_does_not_carry_across_a_start() {
        // A clock that stepped back. Read as expired, the direction that travels.
        let (home, _cache, verdicts) = one_workspace();
        let now = SystemTime::now();
        recorded_with(
            &verdicts,
            StageResult::Failed { status: 1 },
            now + Duration::from_secs(3600),
        );
        rewritten(&result_in(&home, "myws"), Duration::from_secs(30));

        assert_eq!(trusted_on_each(&verdicts, now), [false, false]);
    }

    #[test]
    fn a_dotfiles_refresh_forgets_the_verdict_and_keeps_the_memo() {
        // `chezmoi update` and `pixi global sync` change a standing container in a
        // way no devpod record shows, so the verdict goes. Who mounted the Claude
        // config directory does not change, so the memo stays.
        let (_home, _cache, verdicts) = one_workspace();
        let now = SystemTime::now();
        recorded_with(&verdicts, StageResult::Failed { status: 1 }, now);
        remembered_under(&verdicts, "myws", Some(ClaudeConfig::Ours));
        assert_eq!(trusted_on_each(&verdicts, now), [true, true]);

        verdicts.forget("myws");

        assert_eq!(trusted_on_each(&verdicts, now), [false, false]);
        assert_eq!(verdicts.remembered_claude("myws"), Some(ClaudeConfig::Ours));
        // And forgetting what is not there is not a failure.
        verdicts.forget("myws");
        verdicts.forget("never-recorded");
    }

    #[test]
    fn a_memo_survives_a_start_of_its_container() {
        // The pass after an `up` may now be skipped, and the session it would have
        // fed still has to know whose config directory that is.
        let (home, _cache, verdicts) = one_workspace();
        remembered_under(&verdicts, "myws", Some(ClaudeConfig::Foreign));

        rewritten(&result_in(&home, "myws"), Duration::from_secs(30));

        assert_eq!(
            verdicts.remembered_claude("myws"),
            Some(ClaudeConfig::Foreign)
        );
    }

    #[test]
    fn a_result_that_names_no_container_anchors_nothing() {
        // An id that is not there cannot be told apart from the next container's.
        let (home, cache, verdicts) = one_workspace();
        for document in [
            "{}",
            r#"{"ContainerDetails":{}}"#,
            r#"{"ContainerDetails":{"ID":""}}"#,
            r#"{"ContainerDetails":{"ID":7}}"#,
            "not json",
        ] {
            std::fs::write(result_in(&home, "myws"), document).expect("a result");
            assert!(verdicts.observe("myws").is_none(), "{document}");
            recorded(&verdicts, "myws");
            assert!(
                !cache.path().join(MARKERS_DIR).join("myws.json").exists(),
                "{document}"
            );
        }
    }

    #[test]
    fn a_marker_from_before_the_container_id_is_not_trusted() {
        // What every marker on disk looks like on the first launch after upgrading:
        // keyed on the mtime alone. One redundant round trip, then a new marker.
        let (home, cache, verdicts) = one_workspace();
        let marker = cache.path().join(MARKERS_DIR).join("myws.json");
        std::fs::create_dir_all(marker.parent().expect("a parent")).expect("the directory");
        let result = result_in(&home, "myws");
        let stamp = Stamp::of(&result).expect("an mtime");
        let old = serde_json::json!({
            "verdict": "provisioned",
            "result_mtime": stamp,
            "switches": MarkerSwitches::of(Switches::INSTALLING),
        });
        std::fs::write(&marker, old.to_string()).expect("an old marker");

        assert_eq!(
            trusted_on_each(&verdicts, SystemTime::now()),
            [false, false]
        );
    }
}
