//! The stored shape of a cached repository and a workspace clone.
//!
//! Ported from `devlaunch/worktree/models.py`, field for field, because these
//! two structs are the on-disk contract: Python and Rust alternate over one
//! `metadata.json` during the port (docs/rust-rewrite-plan.md, cutover check 3),
//! so a field renamed, reordered or spelled differently here is a file the other
//! build reads wrong. Everything about the JSON — key names, key order, the
//! spelling of a timestamp, `local_path` being a string — is pinned by tests
//! against output from the real Python.
//!
//! Two decisions carry over from `models.py`:
//!
//! - **An unrecognized field costs the field, not the entry.** Every stored
//!   entry has the same shape, so treating one unknown key as corruption would
//!   let a single field added by a newer devlaunch wipe the whole worktree list
//!   at once. The keys this build has no field for come back in
//!   [`Rebuilt::unknown_fields`] so storage can report them and preserve the
//!   original before a rewrite drops them.
//! - **A timestamp keeps the spelling it was read with.** Python writes
//!   `datetime.now().isoformat()` — naive local time, microseconds omitted when
//!   zero — and re-derives the text on every write. [`Timestamp`] instead
//!   remembers the exact source string, so a value this build only passes
//!   through is written back byte for byte.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jiff::civil;
use serde::{Deserialize, Serialize, Serializer};

/// What `default_branch` is when a stored entry does not say.
const DEFAULT_BRANCH: &str = "main";

/// The stored `default_branch`: a name, or the record not saying.
///
/// `models.py` holds a plain string read with a falsy test, so `""` was the
/// sentinel for "no default branch recorded" — a second meaning riding in the
/// value domain, representable anywhere the string travelled. The two meanings
/// are two arms here; the wire keeps the sentinel spelling (an unrecorded branch
/// serializes back as `""`, byte for byte what Python writes), and
/// [`RecordedDefaultBranch::from_stored`] is the one place that reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordedDefaultBranch {
    /// The recorded name. Non-empty: [`RecordedDefaultBranch::from_stored`] is
    /// the constructor every producer goes through.
    Named(String),
    /// The record does not say. Python's falsy read of `""`.
    Unrecorded,
}

impl RecordedDefaultBranch {
    /// Parse the stored spelling: empty is the sentinel and means unrecorded.
    pub(crate) fn from_stored(name: String) -> Self {
        if name.is_empty() {
            Self::Unrecorded
        } else {
            Self::Named(name)
        }
    }

    /// The recorded name, or nothing when the record does not say.
    pub(crate) fn named(&self) -> Option<&str> {
        match self {
            Self::Named(name) => Some(name),
            Self::Unrecorded => None,
        }
    }

    /// The stored spelling — the name, or the `""` the wire keeps for
    /// unrecorded. What a save writes, and what the one consumer whose Python
    /// interpolates the field verbatim reads
    /// (`workspace_clone.py`'s start-point fallback).
    pub(crate) fn stored_spelling(&self) -> &str {
        match self {
            Self::Named(name) => name,
            Self::Unrecorded => "",
        }
    }
}

impl Serialize for RecordedDefaultBranch {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.stored_spelling())
    }
}

/// A naive local timestamp, in Python's `datetime.isoformat()` spelling.
///
/// Two values in one because the two jobs differ: `at` is what comparisons and
/// age arithmetic use, `spelling` is what goes back to disk. Keeping the source
/// text means a file this build merely loads and re-saves is unchanged in those
/// bytes, where re-deriving it would rewrite `10:00:00.000000` as `10:00:00` —
/// the same instant, different bytes.
///
/// Ordering is by instant first, and only then by spelling, so it agrees with
/// equality without ever ordering two different instants by their text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Timestamp {
    at: civil::DateTime,
    spelling: String,
}

/// Text that is not a timestamp this build can read.
///
/// `reason` quotes the parser, which is what Python's warning does with the
/// `ValueError` it caught; it is data, not a sentence this crate composed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotATimestamp {
    pub(crate) text: String,
    pub(crate) reason: String,
}

impl Timestamp {
    /// Now, on the local clock, at microsecond resolution.
    ///
    /// Microseconds because that is all `datetime` has: a nanosecond Rust wrote
    /// would be a value Python cannot round-trip, and truncating (not rounding)
    /// is what reading a microsecond clock does.
    pub(crate) fn now() -> Self {
        Self::from_civil(truncate_to_microseconds(jiff::Zoned::now().datetime()))
    }

    /// A timestamp at `at`, spelled the way Python would spell it.
    pub(crate) fn from_civil(at: civil::DateTime) -> Self {
        let at = truncate_to_microseconds(at);
        Self {
            spelling: spell(at),
            at,
        }
    }

    /// Read a stored timestamp, keeping its exact spelling.
    pub(crate) fn parse(text: &str) -> Result<Self, NotATimestamp> {
        match text.parse::<civil::DateTime>() {
            Ok(at) => Ok(Self {
                at,
                spelling: text.to_owned(),
            }),
            Err(err) => Err(NotATimestamp {
                text: text.to_owned(),
                reason: err.to_string(),
            }),
        }
    }

    /// The instant, for comparing and for age arithmetic.
    pub(crate) fn at(&self) -> civil::DateTime {
        self.at
    }

    /// The stored spelling, which is what a save writes.
    ///
    /// Only this module's tests read it; a save goes through [`Serialize`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn as_str(&self) -> &str {
        &self.spelling
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.spelling)
    }
}

/// `datetime.isoformat()`: seconds always, microseconds only when they are not
/// zero, and then exactly six digits.
fn spell(at: civil::DateTime) -> String {
    let microsecond = at.subsec_nanosecond() / 1_000;
    let seconds = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        at.year(),
        at.month(),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
    );
    if microsecond == 0 {
        seconds
    } else {
        format!("{seconds}.{microsecond:06}")
    }
}

/// Drop sub-microsecond digits, which `datetime` has no room for.
fn truncate_to_microseconds(at: civil::DateTime) -> civil::DateTime {
    let microsecond = at.subsec_nanosecond() / 1_000;
    at.with()
        .subsec_nanosecond(microsecond * 1_000)
        .build()
        // A nanosecond count derived from a valid one is valid; keeping the
        // original rather than unwrapping is what makes this total.
        .unwrap_or(at)
}

/// A base git repository in the bare-clone cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BaseRepository {
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) remote_url: String,
    #[serde(serialize_with = "as_string")]
    pub(crate) local_path: PathBuf,
    pub(crate) default_branch: RecordedDefaultBranch,
    pub(crate) last_fetched: Option<Timestamp>,
    /// The branch names this repository has workspace clones for.
    ///
    /// Narrower than its neighbours on purpose: [`super::metadata`] keeps it in
    /// step with the worktree map on every add and remove, and it is the one
    /// field of this record a dl run that holds no repo lock can move (#400
    /// §4). Out of reach of `flows`, so a caller with one field to change has
    /// to say which field, and cannot carry a stale copy of this one along.
    pub(super) worktrees: Vec<String>,
    /// What the last background sweep of this repository had to say — see
    /// [`SweepNote`].
    ///
    /// Last in the record rather than beside `last_fetched`, which is where it
    /// belongs by meaning: the two are written by the same pass, but a key
    /// appended is a key every older reader already ignores, where a key
    /// inserted moves `worktrees` and rewrites the file for every repository
    /// that never had a note.
    ///
    /// Omitted from the file entirely when there is none, so a clean cache's
    /// `metadata.json` is byte for byte the one this build wrote before the
    /// field existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_sweep: Option<SweepNote>,
}

/// What one background sweep had to say about one repository.
///
/// The sweep is a detached child with all three descriptors on `/dev/null`
/// (`ProcessRunner::detach`), so every notice it raises has always gone
/// nowhere. This is the one that survives to be read: `dl --ls` prints it and
/// `dl --ls --json` carries it, which is the whole of devlaunch#480.
///
/// **Overwritten, never accumulated.** Each pass that actually acts on a
/// repository replaces this outright — a pass that fetched cleanly clears it —
/// so one repository holds at most one note and the record needs no rotation
/// and no second file. A pass that attempted nothing (the interval had not
/// elapsed, another run held the lock, the lock would not open) leaves the last
/// pass's note standing, because clearing it there would report a clean sweep
/// that never ran.
///
/// **Absent rather than empty when there is nothing to say.** The key is left
/// out of the file, so "the last sweep was clean" is one state, "a note is
/// here" is another, and a stored value this build cannot read is a third —
/// a [`NotRebuilt`], exactly as an unparsable `last_fetched` is. A bare string
/// would have spelled the first two the same way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepNote {
    /// Which of the sweep's troubles this was.
    pub trouble: SweepTrouble,
    /// The words of whatever refused, verbatim — git's own stderr where git is
    /// what refused.
    ///
    /// `None` where nothing said anything: a fetch killed at the bound and a
    /// clone that is not on disk are conditions dl observed rather than
    /// sentences anything wrote, and an empty string for them would be a
    /// refusal that said nothing dressed up as one that did.
    pub said: Option<String>,
}

/// What went wrong for a sweep, in the arms the sweep itself can tell apart.
///
/// Not a rendering: the words are the `dl` binary's (#251 §5), and what travels
/// here is which condition it was plus whatever git said about it. Snake case
/// on the wire in both places it appears, because it is a token rather than a
/// sentence — the same way devpod's `Running` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SweepTrouble {
    /// The fetch worked and `git pack-refs --all` would not collapse the loose
    /// refs it wrote. The cache is fresh either way; see docs/cleanup.md.
    RefsNotPacked,
    /// git refused the fetch.
    FetchRefused,
    /// The fetch ran past the sweep's own time bound and the child was killed.
    FetchTimedOut,
    /// The record names a bare clone that is not on disk, so there was nothing
    /// to fetch into.
    CloneMissing,
    /// The fetch worked and the freshness stamp could not be written.
    NotRecorded,
}

/// A workspace clone of one branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeInfo {
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) branch: String,
    #[serde(serialize_with = "as_string")]
    pub(crate) local_path: PathBuf,
    pub(crate) workspace_id: String,
    /// Written by [`WorktreeInfo::new`] and by nothing else. Narrower than
    /// their neighbours because no production caller outside this module reads
    /// either one (#400 §4) — `dl ls` takes the last-used column off the devpod
    /// listing, not off the record — so the day one wants them, it asks here
    /// rather than reaching in.
    pub(super) created_at: Timestamp,
    pub(super) last_used: Timestamp,
    pub(crate) devpod_workspace_id: Option<String>,
}

/// One rebuilt entry, and the stored keys this build has no field for.
///
/// Reported rather than dropped silently: the entry loads, and the next write
/// loses those keys, so storage preserves the original first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rebuilt<T> {
    pub(crate) entry: T,
    /// Sorted, as Python's `unknown_fields` returns them.
    pub(crate) unknown_fields: Vec<String>,
}

/// A stored entry that could not be rebuilt at all, and is therefore skipped.
///
/// `reason` quotes the parser — Python warns with the `repr` of the `KeyError`,
/// `TypeError` or `ValueError` it caught, which is the same data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotRebuilt {
    pub(crate) reason: String,
}

impl BaseRepository {
    /// A repository with the defaults `models.py` declares.
    ///
    /// The record `up` writes on a first clone, and now the only way to build
    /// one outside this module: `worktrees` is out of `flows`' reach (#412), so
    /// a caller registering a repository no longer picks what its branch list
    /// says. Both flows-layer sites used to spell the record out as a literal
    /// and both wrote `worktrees: Vec::new()`. That is still the value; it is
    /// this module's answer now rather than one invented at the call site.
    pub(crate) fn new(owner: &str, repo: &str, remote_url: &str, local_path: PathBuf) -> Self {
        Self {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            remote_url: remote_url.to_owned(),
            local_path,
            default_branch: RecordedDefaultBranch::Named(DEFAULT_BRANCH.to_owned()),
            last_fetched: None,
            worktrees: Vec::new(),
            last_sweep: None,
        }
    }

    /// Rebuild a stored entry, reporting the keys a rewrite would drop.
    pub(crate) fn from_json(value: serde_json::Value) -> Result<Rebuilt<Self>, NotRebuilt> {
        let stored: StoredRepository = rebuild(value)?;
        Ok(Rebuilt {
            entry: Self {
                owner: stored.owner,
                repo: stored.repo,
                remote_url: stored.remote_url,
                local_path: PathBuf::from(stored.local_path),
                default_branch: RecordedDefaultBranch::from_stored(stored.default_branch),
                last_fetched: read_optional_timestamp(stored.last_fetched)?,
                worktrees: stored.worktrees,
                last_sweep: read_optional_sweep_note(stored.last_sweep)?,
            },
            unknown_fields: stored.unknown.into_keys().collect(),
        })
    }
}

impl WorktreeInfo {
    /// A worktree recorded as created and used now.
    pub(crate) fn new(
        owner: &str,
        repo: &str,
        branch: &str,
        local_path: PathBuf,
        workspace_id: &str,
    ) -> Self {
        let now = Timestamp::now();
        Self {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            branch: branch.to_owned(),
            local_path,
            workspace_id: workspace_id.to_owned(),
            created_at: now.clone(),
            last_used: now,
            devpod_workspace_id: None,
        }
    }

    /// Rebuild a stored entry, reporting the keys a rewrite would drop.
    pub(crate) fn from_json(value: serde_json::Value) -> Result<Rebuilt<Self>, NotRebuilt> {
        let stored: StoredWorktree = rebuild(value)?;
        Ok(Rebuilt {
            entry: Self {
                owner: stored.owner,
                repo: stored.repo,
                branch: stored.branch,
                local_path: PathBuf::from(stored.local_path),
                workspace_id: stored.workspace_id,
                created_at: read_timestamp(&stored.created_at)?,
                last_used: read_timestamp(&stored.last_used)?,
                devpod_workspace_id: stored.devpod_workspace_id,
            },
            unknown_fields: stored.unknown.into_keys().collect(),
        })
    }
}

/// `local_path` is stored as a string, as `str(Path)` writes it.
fn as_string<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    match path.to_str() {
        Some(text) => serializer.serialize_str(text),
        // JSON has no room for bytes that are not UTF-8, and neither has any
        // path devlaunch builds; failing the write beats writing a lie.
        None => Err(serde::ser::Error::custom("path is not valid UTF-8")),
    }
}

/// The wire shape of a repository entry: every key optional that `models.py`
/// gives a default, everything else required, unknown keys collected.
#[derive(Deserialize)]
struct StoredRepository {
    owner: String,
    repo: String,
    remote_url: String,
    local_path: String,
    #[serde(default = "default_branch")]
    default_branch: String,
    #[serde(default)]
    last_fetched: Option<serde_json::Value>,
    #[serde(default)]
    worktrees: Vec<String>,
    #[serde(default)]
    last_sweep: Option<serde_json::Value>,
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

/// The wire shape of a worktree entry.
#[derive(Deserialize)]
struct StoredWorktree {
    owner: String,
    repo: String,
    branch: String,
    local_path: String,
    workspace_id: String,
    created_at: String,
    last_used: String,
    #[serde(default)]
    devpod_workspace_id: Option<String>,
    #[serde(flatten)]
    unknown: BTreeMap<String, serde_json::Value>,
}

fn default_branch() -> String {
    DEFAULT_BRANCH.to_owned()
}

fn rebuild<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, NotRebuilt> {
    serde_json::from_value(value).map_err(|err| NotRebuilt {
        reason: err.to_string(),
    })
}

fn read_timestamp(text: &str) -> Result<Timestamp, NotRebuilt> {
    Timestamp::parse(text).map_err(|err| NotRebuilt { reason: err.reason })
}

/// `last_fetched` as `models.py` reads it: absent, null and empty all mean
/// "never fetched", a string is a timestamp, anything else is unreadable.
fn read_optional_timestamp(
    stored: Option<serde_json::Value>,
) -> Result<Option<Timestamp>, NotRebuilt> {
    match stored {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) if text.is_empty() => Ok(None),
        Some(serde_json::Value::String(text)) => read_timestamp(&text).map(Some),
        Some(other) => Err(NotRebuilt {
            reason: format!("last_fetched is {}", json_kind(&other)),
        }),
    }
}

/// `last_sweep` as the sweep writes it: absent and null both mean the last sweep
/// left nothing to say, an object is a note, anything else is unreadable.
///
/// No empty-string arm, where [`read_optional_timestamp`] has one: that arm is
/// bug-compatibility with a Python `last_fetched` this field has no history with,
/// and an empty note that reads as no note is the exact conflation the field was
/// shaped to avoid.
///
/// A note whose *object* this build cannot read costs the entry, the way an
/// unparsable `last_fetched` does — the load reports it and storage preserves the
/// original before any rewrite. A key inside the note that this build has no
/// field for costs only that key, which is the module's other rule and the reason
/// a newer devlaunch's extra detail cannot take a repository out of the listing.
fn read_optional_sweep_note(
    stored: Option<serde_json::Value>,
) -> Result<Option<SweepNote>, NotRebuilt> {
    match stored {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(object @ serde_json::Value::Object(_)) => serde_json::from_value(object)
            .map(Some)
            .map_err(|error| NotRebuilt {
                reason: format!("last_sweep is unreadable: {error}"),
            }),
        Some(other) => Err(NotRebuilt {
            reason: format!("last_sweep is {}", json_kind(&other)),
        }),
    }
}

/// The JSON type of a value, as data for a report.
fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    //! Pinned against the real Python where the bytes matter.
    //!
    //! The golden strings below were produced by the Python this replaces,
    //! from the worktree root:
    //!
    //! ```text
    //! pixi run python -c "import json; from datetime import datetime;
    //!   from pathlib import Path;
    //!   from devlaunch.worktree.models import BaseRepository, WorktreeInfo;
    //!   print(json.dumps(<entry>.to_dict(), separators=(',', ':')))"
    //! ```
    //!
    //! They are constants rather than a call-out to pixi so `cargo test` needs
    //! no Python installed; regenerate them if `models.py` ever changes shape,
    //! which is exactly the event they exist to catch.

    use super::*;

    /// `BaseRepository(...).to_dict()`, from the real Python.
    const PYTHON_REPOSITORY: &str = concat!(
        r#"{"owner":"test-owner","repo":"test-repo","#,
        r#""remote_url":"https://github.com/test-owner/test-repo.git","#,
        r#""local_path":"/tmp/repos/test-owner/test-repo","default_branch":"main","#,
        r#""last_fetched":"2024-01-01T12:00:00","worktrees":["feature-1","feature-2"]}"#,
    );

    /// A repository built with `models.py`'s defaults.
    const PYTHON_DEFAULTED_REPOSITORY: &str = concat!(
        r#"{"owner":"o","repo":"r","remote_url":"u","local_path":"/p","#,
        r#""default_branch":"main","last_fetched":null,"worktrees":[]}"#,
    );

    /// `WorktreeInfo(...).to_dict()`, from the real Python.
    const PYTHON_WORKTREE: &str = concat!(
        r#"{"owner":"test-owner","repo":"test-repo","branch":"feature-branch","#,
        r#""local_path":"/tmp/worktrees/test-owner/test-repo/feature-branch","#,
        r#""workspace_id":"test-repo-feature-branch","created_at":"2024-01-01T10:00:00","#,
        r#""last_used":"2024-01-01T12:00:00.123456","#,
        r#""devpod_workspace_id":"test-repo-feature-branch"}"#,
    );

    fn python_repository() -> BaseRepository {
        BaseRepository {
            owner: "test-owner".to_owned(),
            repo: "test-repo".to_owned(),
            remote_url: "https://github.com/test-owner/test-repo.git".to_owned(),
            local_path: PathBuf::from("/tmp/repos/test-owner/test-repo"),
            default_branch: RecordedDefaultBranch::Named("main".to_owned()),
            last_fetched: Some(Timestamp::from_civil(civil::datetime(
                2024, 1, 1, 12, 0, 0, 0,
            ))),
            worktrees: vec!["feature-1".to_owned(), "feature-2".to_owned()],
            // The golden below is Python's, from before this field existed, and
            // it still is: a record with no note writes no key.
            last_sweep: None,
        }
    }

    fn python_worktree() -> WorktreeInfo {
        WorktreeInfo {
            owner: "test-owner".to_owned(),
            repo: "test-repo".to_owned(),
            branch: "feature-branch".to_owned(),
            local_path: PathBuf::from("/tmp/worktrees/test-owner/test-repo/feature-branch"),
            workspace_id: "test-repo-feature-branch".to_owned(),
            created_at: Timestamp::from_civil(civil::datetime(2024, 1, 1, 10, 0, 0, 0)),
            last_used: Timestamp::from_civil(civil::datetime(2024, 1, 1, 12, 0, 0, 123_456_000)),
            devpod_workspace_id: Some("test-repo-feature-branch".to_owned()),
        }
    }

    fn json_of(text: &str) -> serde_json::Value {
        serde_json::from_str(text).expect("the golden is JSON")
    }

    // --- timestamps -------------------------------------------------------

    #[test]
    fn a_whole_second_is_spelled_without_microseconds() {
        // Python's isoformat omits the fraction when the microsecond is zero,
        // and a Rust that always wrote `.000000` would differ on every file.
        let at = Timestamp::from_civil(civil::datetime(2024, 1, 1, 12, 0, 0, 0));

        assert_eq!(at.as_str(), "2024-01-01T12:00:00");
    }

    #[test]
    fn microseconds_are_spelled_with_exactly_six_digits() {
        let at = Timestamp::from_civil(civil::datetime(2024, 1, 1, 12, 0, 0, 5_000));

        assert_eq!(at.as_str(), "2024-01-01T12:00:00.000005");
    }

    #[test]
    fn sub_microsecond_precision_is_truncated_not_rounded() {
        // `datetime` has no room for it, so a Rust-written nanosecond would be a
        // value Python could not round-trip. Truncation is what reading a
        // microsecond clock does; rounding would invent an instant.
        let at = Timestamp::from_civil(civil::datetime(2024, 1, 1, 12, 0, 0, 1_999));

        assert_eq!(at.as_str(), "2024-01-01T12:00:00.000001");
    }

    #[test]
    fn a_parsed_timestamp_keeps_the_spelling_it_was_read_with() {
        // The byte-for-byte round trip: whatever the file said is what a save
        // writes back, even where this build would have spelled it differently.
        for text in [
            "2024-01-01T10:00:00",
            "2024-01-01T10:00:00.000000",
            "2024-01-01T10:00:00.123456",
        ] {
            let parsed = Timestamp::parse(text).expect("a timestamp");
            assert_eq!(parsed.as_str(), text);
            assert_eq!(
                serde_json::to_string(&parsed).expect("serializable"),
                format!("\"{text}\"")
            );
        }
    }

    #[test]
    fn now_is_spelled_the_way_python_spells_it() {
        let now = Timestamp::now();

        let reparsed = Timestamp::parse(now.as_str()).expect("its own spelling parses");
        assert_eq!(reparsed, now);
        let spelling = now.as_str();
        assert!(
            spelling.len() == 19 || spelling.len() == 26,
            "seconds, or seconds and six digits: {spelling:?}"
        );
        assert_eq!(&spelling[4..5], "-");
        assert_eq!(&spelling[10..11], "T");
    }

    #[test]
    fn text_that_is_not_a_timestamp_is_reported_with_the_parser_s_reason() {
        let failed = Timestamp::parse("not-a-timestamp").expect_err("not a timestamp");

        assert_eq!(failed.text, "not-a-timestamp");
        assert!(!failed.reason.is_empty(), "the parser said something");
    }

    #[test]
    fn timestamps_order_by_instant_whatever_their_spelling() {
        let plain = Timestamp::parse("2024-01-01T10:00:00").expect("a timestamp");
        let zero_padded = Timestamp::parse("2024-01-01T10:00:00.000000").expect("a timestamp");
        let later = Timestamp::parse("2024-01-01T10:00:01").expect("a timestamp");

        assert!(plain < later);
        assert_eq!(plain.at(), zero_padded.at());
        assert_ne!(plain, zero_padded, "the spelling is part of the value");
    }

    // --- repositories -----------------------------------------------------

    #[test]
    fn a_repository_serializes_to_the_bytes_python_writes() {
        let written = serde_json::to_string(&python_repository()).expect("serializable");

        assert_eq!(written, PYTHON_REPOSITORY);
    }

    #[test]
    fn a_repository_python_wrote_rebuilds_into_the_same_entry() {
        let rebuilt = BaseRepository::from_json(json_of(PYTHON_REPOSITORY)).expect("rebuilt");

        assert_eq!(rebuilt.entry, python_repository());
        assert!(rebuilt.unknown_fields.is_empty());
    }

    #[test]
    fn the_defaults_match_the_dataclass_defaults() {
        let repo = BaseRepository::new("o", "r", "u", PathBuf::from("/p"));

        assert_eq!(repo.default_branch.named(), Some("main"));
        assert_eq!(repo.last_fetched, None);
        assert!(repo.worktrees.is_empty());
        assert_eq!(
            serde_json::to_string(&repo).expect("serializable"),
            PYTHON_DEFAULTED_REPOSITORY
        );
    }

    #[test]
    fn a_repository_entry_missing_the_defaulted_keys_still_rebuilds() {
        let stored = json_of(r#"{"owner":"o","repo":"r","remote_url":"u","local_path":"/p"}"#);

        let rebuilt = BaseRepository::from_json(stored).expect("rebuilt");

        assert_eq!(
            rebuilt.entry,
            BaseRepository::new("o", "r", "u", PathBuf::from("/p"))
        );
    }

    #[test]
    fn a_null_last_fetched_means_never_fetched() {
        let stored = json_of(PYTHON_DEFAULTED_REPOSITORY);

        let rebuilt = BaseRepository::from_json(stored).expect("rebuilt");

        assert_eq!(rebuilt.entry.last_fetched, None);
    }

    #[test]
    fn a_repository_entry_without_a_local_path_cannot_be_rebuilt() {
        // `models.py` reads `data["local_path"]` directly, so a missing path is
        // a `KeyError` and the entry is skipped.
        let failed =
            BaseRepository::from_json(json_of(r#"{"owner":"o","repo":"r","remote_url":"u"}"#))
                .expect_err("not rebuilt");

        assert!(
            failed.reason.contains("local_path"),
            "the reason names the missing field: {:?}",
            failed.reason
        );
    }

    #[test]
    fn a_repository_entry_whose_local_path_is_not_a_string_cannot_be_rebuilt() {
        // `Path(None)` is Python's `TypeError` here, and like Python's message
        // the reason describes the type rather than naming the field.
        for stored in [
            r#"{"owner":"o","repo":"r","remote_url":"u","local_path":null}"#,
            r#"{"owner":"o","repo":"r","remote_url":"u","local_path":5}"#,
        ] {
            let failed = BaseRepository::from_json(json_of(stored)).expect_err("not rebuilt");
            assert!(!failed.reason.is_empty(), "the parser said something");
        }
    }

    #[test]
    fn an_unparsable_last_fetched_cannot_be_rebuilt() {
        let stored = json_of(
            r#"{"owner":"o","repo":"r","remote_url":"u","local_path":"/p","last_fetched":"nope"}"#,
        );

        BaseRepository::from_json(stored).expect_err("not rebuilt");
    }

    // --- the last sweep's note (devlaunch#480) ----------------------------

    #[test]
    fn a_record_written_before_the_field_existed_still_loads_and_says_nothing() {
        // Every `metadata.json` on every machine dl has ever run on. The key is not
        // there, and "the last sweep left nothing to say" is the right reading of
        // that: a record cannot be made unloadable by a field it predates.
        let stored = json_of(PYTHON_REPOSITORY);

        let rebuilt = BaseRepository::from_json(stored).expect("rebuilt");

        assert_eq!(rebuilt.entry.last_sweep, None);
        assert!(
            rebuilt.unknown_fields.is_empty(),
            "an absent key is not an unknown one"
        );
    }

    #[test]
    fn a_record_with_no_note_writes_no_key_at_all() {
        // What keeps the upgrade silent: a cache whose sweeps all went cleanly
        // rewrites byte for byte the file this build wrote before the field
        // existed, so nothing about `metadata.json` moves for anybody who has no
        // trouble to report.
        let written = serde_json::to_string(&python_repository()).expect("serializable");

        assert!(
            !written.contains("last_sweep"),
            "absent, not null: {written}"
        );
    }

    #[test]
    fn a_note_round_trips_through_the_record() {
        let mut repository = python_repository();
        repository.last_sweep = Some(SweepNote {
            trouble: SweepTrouble::RefsNotPacked,
            said: Some("fatal: unable to create 'packed-refs.lock'".to_owned()),
        });

        let written = serde_json::to_string(&repository).expect("serializable");
        let rebuilt = BaseRepository::from_json(json_of(&written)).expect("rebuilt");

        assert!(
            written.contains(r#""last_sweep":{"trouble":"refs_not_packed","said":"#),
            "the tokens are the record's own spelling: {written}"
        );
        assert_eq!(rebuilt.entry, repository);
    }

    #[test]
    fn a_null_note_reads_as_no_note_rather_than_as_damage() {
        let stored = json_of(
            r#"{"owner":"o","repo":"r","remote_url":"u","local_path":"/p","last_sweep":null}"#,
        );

        let rebuilt = BaseRepository::from_json(stored).expect("rebuilt");

        assert_eq!(rebuilt.entry.last_sweep, None);
    }

    #[test]
    fn a_note_this_build_cannot_read_costs_the_entry_the_way_a_bad_stamp_does() {
        // Absence, a note, and a stored value that is not one are three states and
        // not two: the third goes the way `last_fetched` already goes — the entry is
        // reported unusable and storage preserves the original before any rewrite —
        // rather than being quietly read as "nothing to say".
        for stored in [
            r#"{"owner":"o","repo":"r","remote_url":"u","local_path":"/p","last_sweep":""}"#,
            r#"{"owner":"o","repo":"r","remote_url":"u","local_path":"/p","last_sweep":3}"#,
            r#"{"owner":"o","repo":"r","remote_url":"u","local_path":"/p",
                "last_sweep":{"trouble":"a_trouble_this_build_has_no_arm_for"}}"#,
        ] {
            let failed = BaseRepository::from_json(json_of(stored)).expect_err("not rebuilt");
            assert!(
                failed.reason.contains("last_sweep"),
                "the reason names the field: {:?}",
                failed.reason
            );
        }
    }

    #[test]
    fn a_key_inside_a_note_that_this_build_has_no_field_for_costs_only_that_key() {
        // The module's other rule, and the reason a newer devlaunch's extra detail
        // cannot take a repository out of somebody's listing. The note itself is
        // overwritten wholesale by the next sweep anyway.
        let stored = json_of(
            r#"{"owner":"o","repo":"r","remote_url":"u","local_path":"/p",
                "last_sweep":{"trouble":"fetch_timed_out","said":null,"after":"30s"}}"#,
        );

        let rebuilt = BaseRepository::from_json(stored).expect("rebuilt");

        assert_eq!(
            rebuilt.entry.last_sweep,
            Some(SweepNote {
                trouble: SweepTrouble::FetchTimedOut,
                said: None,
            })
        );
    }

    #[test]
    fn a_repository_key_this_build_does_not_declare_is_reported_and_dropped() {
        let stored = json_of(
            r#"{"owner":"o","repo":"r","remote_url":"u","local_path":"/p",
                "zzz_future":1,"future_repo_field":2}"#,
        );

        let rebuilt = BaseRepository::from_json(stored).expect("rebuilt");

        assert_eq!(
            rebuilt.unknown_fields,
            vec!["future_repo_field".to_owned(), "zzz_future".to_owned()],
            "sorted, as Python's unknown_fields returns them"
        );
        assert_eq!(
            serde_json::to_string(&rebuilt.entry).expect("serializable"),
            PYTHON_DEFAULTED_REPOSITORY,
            "a rewrite drops them, which is why they are reported"
        );
    }

    // --- worktrees --------------------------------------------------------

    #[test]
    fn a_worktree_serializes_to_the_bytes_python_writes() {
        let written = serde_json::to_string(&python_worktree()).expect("serializable");

        assert_eq!(written, PYTHON_WORKTREE);
    }

    #[test]
    fn a_worktree_python_wrote_rebuilds_into_the_same_entry() {
        let rebuilt = WorktreeInfo::from_json(json_of(PYTHON_WORKTREE)).expect("rebuilt");

        assert_eq!(rebuilt.entry, python_worktree());
        assert!(rebuilt.unknown_fields.is_empty());
    }

    #[test]
    fn a_worktree_entry_without_its_timestamps_cannot_be_rebuilt() {
        // Both are read directly, so an absent, null or unparsable value skips
        // the entry rather than defaulting it to now.
        for stored in [
            r#"{"owner":"o","repo":"r","branch":"b","local_path":"/p","workspace_id":"w",
                "last_used":"2024-01-01T10:00:00"}"#,
            r#"{"owner":"o","repo":"r","branch":"b","local_path":"/p","workspace_id":"w",
                "created_at":null,"last_used":"2024-01-01T10:00:00"}"#,
            r#"{"owner":"o","repo":"r","branch":"b","local_path":"/p","workspace_id":"w",
                "created_at":"not-a-timestamp","last_used":"2024-01-01T10:00:00"}"#,
        ] {
            WorktreeInfo::from_json(json_of(stored)).expect_err("not rebuilt");
        }
    }

    #[test]
    fn an_absent_devpod_workspace_id_is_none() {
        let stored = json_of(
            r#"{"owner":"o","repo":"r","branch":"b","local_path":"/p","workspace_id":"w",
                "created_at":"2024-01-01T10:00:00","last_used":"2024-01-01T10:00:00"}"#,
        );

        let rebuilt = WorktreeInfo::from_json(stored).expect("rebuilt");

        assert_eq!(rebuilt.entry.devpod_workspace_id, None);
    }

    #[test]
    fn a_worktree_key_this_build_does_not_declare_is_reported_and_dropped() {
        let stored = json_of(
            r#"{"owner":"o","repo":"r","branch":"b","local_path":"/p","workspace_id":"w",
                "created_at":"2024-01-01T10:00:00","last_used":"2024-01-01T10:00:00",
                "pinned_by_newer_build":true}"#,
        );

        let rebuilt = WorktreeInfo::from_json(stored).expect("rebuilt");

        assert_eq!(
            rebuilt.unknown_fields,
            vec!["pinned_by_newer_build".to_owned()]
        );
        let written = serde_json::to_string(&rebuilt.entry).expect("serializable");
        assert!(!written.contains("pinned_by_newer_build"));
    }

    #[test]
    fn a_new_worktree_is_created_and_used_at_the_same_moment() {
        let worktree = WorktreeInfo::new("o", "r", "b", PathBuf::from("/p"), "w");

        assert_eq!(worktree.created_at, worktree.last_used);
        assert_eq!(worktree.devpod_workspace_id, None);
    }
}
