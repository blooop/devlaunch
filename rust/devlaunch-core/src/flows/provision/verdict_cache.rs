//! Remembering that a container was found provisioned, so the next top-up of a
//! container that never stopped can skip the trip that found out.
//!
//! The setup pass is one `devpod ssh` round trip, measured at ~1.7s of which ~99%
//! is connection and process setup (#157). A workspace that has had `gh` and a real
//! `claude` in it for a week pays that on every `dl <ws> up`, every time, to be told
//! the same thing it was told last time. This is the one file that lets the answer
//! be reused: a marker per workspace under devlaunch's own cache directory, holding
//! the verdict and — the whole of the mechanism — the identity of the container the
//! verdict was about.
//!
//! # What makes a remembered verdict still true
//!
//! Not the marker's own age, and not a TTL. A verdict about *this* container is
//! true until there is a different container, and "a different container" is a fact
//! the host can read without asking anything: devpod rewrites
//! `workspace_result.json` on its way out of every **completed** `up`, whoever ran
//! that `up` — `dl`, VS Code, a hand-typed `devpod up`, a `--recreate`. So the
//! marker records that file's mtime, and the verdict is trusted only while the
//! file's mtime is still *equal* to the recorded one.
//!
//! Equality rather than "not newer", because the comparison is an identity check
//! and not a freshness one. A file restored from a backup, a clock that stepped
//! backwards, a devpod home copied between machines: each moves the mtime somewhere
//! that is not newer, and each is exactly the case where the recorded verdict is
//! about a container that is not the one standing now.
//!
//! `workspace.json` beside it is deliberately not the anchor — devpod writes it on
//! the way *in* and never touches it again, so a container rebuilt ten times still
//! carries the first create's timestamps, and a marker keyed on it would survive
//! every rebuild it exists to notice.
//!
//! # Absent is the only harmless misreading
//!
//! Every failure here reads as *no trusted verdict*: a marker that is not there,
//! will not parse, carries a word this build does not know, names a workspace whose
//! `workspace_result.json` cannot be found or is ambiguous between two devpod
//! contexts, or whose mtime cannot be read. That is the same argument
//! [`super::ProbeResult::parse`] makes for reading a garbled report as *absent*:
//! provisioning is idempotent, so reading a valid marker as invalid costs one
//! redundant round trip — today's behaviour, exactly — where reading an invalid one
//! as valid silently ships a container without the tools the pass exists to put
//! there.
//!
//! Which is also why nothing here ever *deletes* a marker. Invalidation is the
//! comparison, made afresh on every read; a delete would be a second mechanism
//! saying the same thing, and one that has to run to be correct.
//!
//! # What is deliberately not remembered
//!
//! Only [`super::Provisioning::AlreadyProvisioned`] is recorded. A pass that lent,
//! installed or kept a shim is **not**, and `ShimKept` is the one that says why:
//! that arm is a documented residual (see [`super::provision`]) in which a
//! container re-attempts one failing transfer on every `up`, and a marker would
//! turn "re-attempts on every up" into "never attempts again" — a behaviour change
//! wearing a cache's clothes. A lend or an install is also the launch on which the
//! container's state just changed, which is the worst moment to freeze an opinion
//! about it. Each of them is followed by a later pass that probes *provisioned*,
//! and that pass writes the marker.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{ClaudeConfig, Switches, ToolsSwitch, ZellijSwitch};

use crate::clients::devpod_home::{DevpodHome, sole_workspace_result};

/// The directory the markers live in, under devlaunch's cache directory.
const MARKERS_DIR: &str = "tool-verdicts";

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
    fn marker(&self, workspace_id: &str) -> PathBuf {
        self.markers_dir.join(format!("{workspace_id}.json"))
    }

    /// Whether this workspace's remembered verdict still describes the container
    /// standing now.
    ///
    /// Total, and false for every doubt — see the module note on why absent is the
    /// only harmless misreading.
    pub(crate) fn trusted(&self, workspace_id: &str, switches: Switches) -> bool {
        let Some(marker) = read_marker(&self.marker(workspace_id)) else {
            return false;
        };
        // Before the anchor, because it is the cheaper of the two and because it is
        // the one that is wrong about a container that really is standing: the
        // mtime can match perfectly while the marker describes a pass that skipped
        // the very stage this launch is asking for.
        if marker.switches != MarkerSwitches::of(switches) {
            return false;
        }
        let Some(result) = sole_workspace_result(self.devpod_home.as_ref(), workspace_id) else {
            return false;
        };
        Stamp::of(&result) == Some(marker.result_mtime)
    }

    /// Where the Claude memo for this workspace lives.
    fn memo(&self, workspace_id: &str) -> PathBuf {
        self.marker(workspace_id).with_extension("claude")
    }

    /// Remember who owns this container's Claude config directory.
    ///
    /// Deliberately *not* a field on the verdict marker, and the difference is the
    /// question each answers. A marker answers "may this launch skip the trip", so
    /// it is written only for the one verdict worth remembering. This answers "who
    /// owns that directory", which a launch needs whether or not a trip was skipped
    /// and whether or not the container was ever provisioned -- a workspace the host
    /// lent its binaries to has no marker at all, and that is the very case the
    /// credential exists for. Folding the two together made a missing marker mean
    /// two different things, and the second one silently forwarded nothing.
    ///
    /// Unanchored, unlike a marker: there is no container mtime paired with it. It
    /// does not need one. The answer can only change when the container does, and
    /// every route that changes a container -- `up`, `restart`, `recreate`, `reset`
    /// -- runs a pass that rewrites this.
    ///
    /// Silent about every way of not working, exactly as [`Self::record`] is.
    pub fn remember_claude(&self, workspace_id: &str, claude: Option<ClaudeConfig>) {
        let word = match claude {
            Some(ClaudeConfig::Ours) => ClaudeConfig::Ours.word(),
            Some(ClaudeConfig::Foreign) => ClaudeConfig::Foreign.word(),
            // Written, not skipped: "a pass ran and could not tell" is an answer,
            // and leaving the previous one in place would let a stale `ours` outlive
            // the container it was true of.
            None => "unknown",
        };
        write_atomically(&self.memo(workspace_id), word);
    }

    /// What the last pass saw of this workspace's Claude config directory.
    ///
    /// This is what keeps a launch that skips the round trip from skipping the
    /// decision the credential turns on, and it is also what serves the launch that
    /// never runs a pass at all: attaching to a workspace that is already up and
    /// finished creating goes straight to a session.
    /// `None` for anything unreadable, which is the reading that forwards no login:
    /// no memo yet, a truncated write, a word this build has never heard of.
    pub fn remembered_claude(&self, workspace_id: &str) -> Option<ClaudeConfig> {
        let word = std::fs::read_to_string(self.memo(workspace_id)).ok()?;
        match word.trim() {
            "ours" => Some(ClaudeConfig::Ours),
            "foreign" => Some(ClaudeConfig::Foreign),
            _ => None,
        }
    }

    /// Which container a pass is about to be about, read **before** it runs.
    ///
    /// The whole of why this is a separate call. A pass is a `devpod ssh` that takes
    /// seconds, and nothing devlaunch holds keeps somebody else's `devpod up` — VS
    /// Code, a hand-typed one, a sibling `--recreate` — from completing inside that
    /// window. Read the anchor afterwards and the marker pairs the *new* container's
    /// mtime with the *old* container's verdict, which is a marker that matches
    /// until something else rebuilds: the silent misreading the module note says
    /// must never happen. Read it first and the same race writes a stamp that no
    /// longer matches, so the next launch travels — one redundant round trip, which
    /// is the direction that is allowed to be wrong.
    pub(crate) fn observe(&self, workspace_id: &str) -> Option<Observed> {
        let result = sole_workspace_result(self.devpod_home.as_ref(), workspace_id)?;
        Stamp::of(&result).map(Observed)
    }

    /// Remember that this workspace probed provisioned, as the container
    /// [`Self::observe`] identified before the pass.
    ///
    /// Silent about every way of not working — a cache directory that will not be
    /// created, a write that fails. A verdict cache that could not write is a launch
    /// that pays what it pays today, and a launch is not worth failing over that.
    pub(crate) fn record(&self, workspace_id: &str, observed: Observed, switches: Switches) {
        let Observed(result_mtime) = observed;
        let marker = Marker {
            verdict: Verdict::Provisioned,
            result_mtime,
            switches: MarkerSwitches::of(switches),
        };
        let Ok(text) = serde_json::to_string(&marker) else {
            return;
        };
        write_atomically(&self.marker(workspace_id), &text);
    }
}

/// The container a pass is about, as [`VerdictCache::observe`] read it.
///
/// A type rather than a bare [`Stamp`] so that [`VerdictCache::record`] cannot be
/// handed a timestamp read at the wrong moment: the only way to make one is to ask
/// before the pass, which is the ordering the whole trust rule rests on.
pub(crate) struct Observed(Stamp);

/// What one marker file says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct Marker {
    verdict: Verdict,
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
    ///
    /// A marker written before this field existed has no `switches` key, fails to
    /// deserialize, and is therefore untrusted — one redundant round trip on the
    /// first launch after upgrading, which is the direction [`Verdict`] argues is
    /// the only harmless one.
    switches: MarkerSwitches,
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
}

impl MarkerSwitches {
    fn of(switches: Switches) -> Self {
        Self {
            tools: matches!(switches.tools, ToolsSwitch::Install),
            zellij: matches!(switches.zellij, ZellijSwitch::Install),
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

/// One file's modification time, as two integers.
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
    ///
    /// Nothing, too, for an mtime *before* the epoch. It is not representable here,
    /// and the alternative — clamping it to zero — makes every such file share one
    /// timestamp, which is the one value that must not be shared: two different
    /// container generations would then compare equal.
    fn of(path: &Path) -> Option<Self> {
        let modified = std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()?;
        let since = modified.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            secs: since.as_secs(),
            nanos: since.subsec_nanos(),
        })
    }
}

impl From<Stamp> for SystemTime {
    fn from(stamp: Stamp) -> Self {
        UNIX_EPOCH + std::time::Duration::new(stamp.secs, stamp.nanos)
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
    use crate::clients::devpod_home::devpod_home_with;

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
            verdicts.record(workspace_id, observed, switches);
        }
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
            let cache = cache();
            let verdicts = VerdictCache::under(cache.path(), None);
            verdicts.remember_claude("ws", seen);
            assert_eq!(verdicts.remembered_claude("ws"), expected, "{seen:?}");
        }
    }

    #[test]
    fn a_workspace_that_was_only_lent_its_tools_still_remembers() {
        // The case that made this a memo rather than a field on the marker. A
        // container the host lent its binaries to never probes provisioned, so it has
        // no marker at all -- and it is the very container the credential exists for,
        // since a repo with no devcontainer of its own is what gets lent to.
        let home = devpod_home_with(&[("default", "ws", Some(()))]);
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));

        verdicts.remember_claude("ws", Some(ClaudeConfig::Ours));

        assert!(
            !verdicts.trusted("ws", Switches::INSTALLING),
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
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), None);
        verdicts.remember_claude("ws", Some(ClaudeConfig::Ours));
        verdicts.remember_claude("ws", None);
        assert_eq!(verdicts.remembered_claude("ws"), None);
    }

    #[test]
    fn a_memo_this_build_cannot_read_forwards_nothing() {
        let cache = cache();
        let verdicts = VerdictCache::under(cache.path(), None);
        assert_eq!(verdicts.remembered_claude("never-asked"), None);
        verdicts.remember_claude("ws", Some(ClaudeConfig::Ours));
        let memo = read_dir_one(cache.path());
        for garbage in ["", "OURS", "a word from a later build", "ours\nand more"] {
            std::fs::write(&memo, garbage).expect("a write");
            assert_eq!(
                verdicts.remembered_claude("ws"),
                None,
                "{garbage:?} must not read as an answer"
            );
        }
    }

    /// The one memo file under a scratch cache, wherever this module puts it.
    fn read_dir_one(cache: &Path) -> PathBuf {
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
            tools: ToolsSwitch::Install,
            zellij: ZellijSwitch::Skip,
        };
        recorded_under(&verdicts, "ws", unasked);

        assert!(
            verdicts.trusted("ws", unasked),
            "the pass that wrote it asks the same question and must still be answered"
        );
        assert!(
            !verdicts.trusted("ws", Switches::INSTALLING),
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
        assert!(verdicts.trusted("ws", Switches::INSTALLING));

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
            !verdicts.trusted("ws", Switches::INSTALLING),
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
            !verdicts.trusted("myws", Switches::INSTALLING),
            "nothing is recorded yet"
        );
        recorded(&verdicts, "myws");

        assert!(verdicts.trusted("myws", Switches::INSTALLING));
        // And it survives being read by a second value over the same directories,
        // which is what "the next `dl` process" is.
        let later = VerdictCache::under(cache.path(), Some(DevpodHome::at(home.path())));
        assert!(later.trusted("myws", Switches::INSTALLING));
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
        assert!(verdicts.trusted("myws", Switches::INSTALLING));

        rewritten(&result_in(&home, "myws"), Duration::from_secs(30));

        assert!(!verdicts.trusted("myws", Switches::INSTALLING));
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

        assert!(!verdicts.trusted("myws", Switches::INSTALLING));
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
                !verdicts.trusted("myws", Switches::INSTALLING),
                "{garbled:?}"
            );
        }

        std::fs::write(&marker, &good).expect("the good marker back");
        assert!(
            verdicts.trusted("myws", Switches::INSTALLING),
            "the fixture itself still holds"
        );
        std::fs::remove_file(&marker).expect("the marker removed");
        assert!(!verdicts.trusted("myws", Switches::INSTALLING));
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
            assert!(!verdicts.trusted("myws", Switches::INSTALLING), "{home:?}");
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

        assert!(verdicts.trusted("myws", Switches::INSTALLING));
        assert!(!verdicts.trusted("other", Switches::INSTALLING));
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
}
