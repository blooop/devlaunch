//! Typed results in, bytes out. Every user-facing English word `dl` prints is
//! written in this file or in [`crate::commands`]; core holds none of it.
//!
//! Everything here is a pure function of a value core produced, which is what
//! lets the table's column arithmetic and the JSON document's spelling be tested
//! without a devpod, a cache or a process.

use std::fmt::Write as _;
use std::io;
use std::path::Path;

use devlaunch_core::clients::devpod::{self, ListingUnreadable, NotAListing, NotRun};
use devlaunch_core::domain::config;
use devlaunch_core::domain::metadata;
use devlaunch_core::domain::workspace_id::{NamePart, UnsafeName};
use devlaunch_core::domain::workspace_state::NonEmpty;
use devlaunch_core::flows::disk_usage::describe_usage;
use devlaunch_core::flows::lifecycle::{
    KeptBecause, LifecycleNotice, NotAdopted, Objection, Promotion, PrunePlan, PruneReport,
    PurgeOutcome, PurgePlan, PurgeStep, ReconcilePlan, RemovalRefused, RepointFailure, Unlocatable,
};
use devlaunch_core::flows::listing::{LastUsed, SizeCell, Sizes, TableRow, WorkspaceTable};
use devlaunch_core::flows::repo_manager::{CacheNotice, Refusal, RefusalReason};
use devlaunch_runner::Exit;
use serde_json::Value;
use serde_json::ser::{Formatter, PrettyFormatter};

// ---------------------------------------------------------------------------
// the `dl --ls` table
// ---------------------------------------------------------------------------

/// The sentence `dl --ls` prints on a machine with no workspaces.
pub(crate) const NO_WORKSPACES: &str = "No workspaces found.";

/// The lines `dl --ls` writes, header and separator included.
///
/// # The column arithmetic is Python's, quirk and all
///
/// `id_width` is a maximum over the *rows only*: the `WORKSPACE` heading does not
/// count, so a listing whose widest id is shorter than nine characters has its
/// heading overhang the column it heads. `size_width` does count its `SIZE`
/// heading, because a column of dashes under a wider heading would push every
/// `LAST USED` after it out of line — Python's comment says exactly that, one
/// line below the place it does not do the same for `WORKSPACE`.
///
/// Reproduced rather than tidied: this is what a person's `dl --ls` looks like
/// today, the harness compares it byte for byte, and a port is not the place to
/// change what a listing looks like. The separator's `+ 30` is Python's too — it
/// stands for the `LAST USED` column, which is never measured.
pub(crate) fn table_lines(table: &WorkspaceTable, sizes: Sizes) -> Vec<String> {
    let rows = match table {
        WorkspaceTable::Nothing => return vec![NO_WORKSPACES.to_owned()],
        WorkspaceTable::Rows(rows) => rows,
    };
    /// One row's four cells, measured and printed from the same strings.
    struct Cells {
        id: String,
        kind: String,
        detail: String,
        size: String,
        last_used: String,
    }

    let cells: Vec<Cells> = rows
        .iter()
        .map(|row| Cells {
            id: row.id.clone(),
            kind: row.source.kind.word().to_owned(),
            detail: row.source.detail.clone(),
            size: size_cell(row, sizes),
            last_used: last_used(&row.last_used),
        })
        .collect();

    let id_width = widest(cells.iter().map(|cell| cell.id.as_str()));
    let type_width = widest(cells.iter().map(|cell| cell.kind.as_str()));
    let source_width = widest(cells.iter().map(|cell| cell.detail.as_str()));
    // The heading counts as a cell here and not above; see the doc comment.
    let size_width =
        widest(std::iter::once("SIZE").chain(cells.iter().map(|cell| cell.size.as_str())));

    let sized = |text: &str| match sizes {
        Sizes::Skip => String::new(),
        Sizes::Measure => format!("{text:>size_width$}  "),
    };

    let mut lines = vec![
        format!(
            "{:<id_width$}  {:<type_width$}  {:<source_width$}  {}LAST USED",
            "WORKSPACE",
            "TYPE",
            "SOURCE",
            sized("SIZE")
        ),
        "-".repeat(id_width + type_width + source_width + width_of(&sized("SIZE")) + 30),
    ];
    for cell in &cells {
        lines.push(format!(
            "{:<id_width$}  {:<type_width$}  {:<source_width$}  {}{}",
            cell.id,
            cell.kind,
            cell.detail,
            sized(&cell.size),
            cell.last_used,
        ));
    }
    lines
}

/// One row's `SIZE` cell.
///
/// Total over both facts: whether the column exists at all (the table's), and
/// what was measured for this workspace (the row's).
fn size_cell(row: &TableRow, sizes: Sizes) -> String {
    match sizes {
        Sizes::Skip => String::new(),
        Sizes::Measure => match &row.size {
            // Unreachable from a table built under `Measure`, and answered anyway
            // so this stays a total function of the two values it is given.
            SizeCell::NoColumn => String::new(),
            // Not `0 B`: nothing was measured here, and a zero would say the
            // opposite of that.
            SizeCell::NotOurs => "-".to_owned(),
            SizeCell::Measured(usage) => describe_usage(usage),
        },
    }
}

/// The `LAST USED` cell.
fn last_used(stamp: &LastUsed) -> String {
    match stamp {
        LastUsed::Never => "never".to_owned(),
        LastUsed::At(when) => when.clone(),
    }
}

/// Python's `len()` and Rust's `{:<width$}` both count characters, so the widths
/// and the padding agree on what a column is wide in.
fn width_of(text: &str) -> usize {
    text.chars().count()
}

fn widest<'a>(texts: impl Iterator<Item = &'a str>) -> usize {
    texts.map(width_of).max().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// JSON as Python writes it
// ---------------------------------------------------------------------------

/// A JSON document spelled the way `json.dumps(value, indent=2)` spells it.
///
/// Grade A: `wf` parses `dl --ls --json`, so this is a wire format and not a
/// rendering choice. Two-space indentation, `": "` after a key, and — the part
/// `serde_json` does not do on its own — every non-ASCII character escaped as
/// `\uXXXX`, which is Python's `ensure_ascii=True`.
pub(crate) fn python_json_document(value: &Value) -> String {
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, PythonPretty::default());
    match serde::Serialize::serialize(value, &mut serializer) {
        // A document that cannot be serialized is not reachable from a
        // `serde_json::Value`, and an empty string is the one answer that cannot
        // be mistaken for a listing.
        Err(_) => String::new(),
        Ok(()) => String::from_utf8(out).unwrap_or_default(),
    }
}

/// `PrettyFormatter` with `ensure_ascii`.
///
/// Delegates the whole of the indentation to `serde_json`'s own pretty formatter,
/// which lays a document out exactly as Python's `indent=2` does, and overrides
/// only the one thing Python does differently: it escapes every character above
/// ASCII, as the surrogate pair for anything outside the basic plane.
#[derive(Default)]
struct PythonPretty {
    pretty: PrettyFormatter<'static>,
}

impl Formatter for PythonPretty {
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

    fn end_object_key<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_object_key(writer)
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

    /// The run of string bytes `serde_json` did not have to escape, which includes
    /// every non-ASCII one — it escapes only the control characters, `"` and `\`.
    /// Python escapes the rest too, and this is where that happens.
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        if fragment.is_ascii() {
            return writer.write_all(fragment.as_bytes());
        }
        let mut units = [0u16; 2];
        for character in fragment.chars() {
            if character.is_ascii() {
                writer.write_all(character.encode_utf8(&mut [0u8; 4]).as_bytes())?;
                continue;
            }
            for unit in character.encode_utf16(&mut units) {
                writer.write_all(format!("\\u{unit:04x}").as_bytes())?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Python's repr, for the diagnostics that quote what a tool said
// ---------------------------------------------------------------------------

/// `text` as Python's `repr()` writes it.
///
/// The failure messages quote what devpod said (`… is not JSON: 'not json\n'`),
/// and quoting it this way is what keeps a multi-line stderr from turning a
/// one-line diagnostic into five. Python's rules, as far as they are observable
/// here: single quotes unless the text holds one and no double quote, backslash
/// and the quote escaped, `\n`/`\r`/`\t` named, anything else unprintable as
/// `\xNN`, and printable non-ASCII left alone.
pub(crate) fn python_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ if character == quote => {
                out.push('\\');
                out.push(character);
            }
            // The C0 and C1 control ranges, which is as much of Python's
            // `unicodedata`-driven printability rule as these messages can hit.
            _ if character.is_control() => {
                let _ = write!(out, "\\x{:02x}", character as u32);
            }
            _ => out.push(character),
        }
    }
    out.push(quote);
    out
}

/// The name Python's `type(x).__name__` gives a JSON value of this kind.
///
/// `Number` answers `int`, because that is what every number in a document dl
/// reads is; a float would be reported as `int` here, which is a wording
/// difference in a message only a malformed devpod can provoke.
fn json_type_name(kind: devpod::JsonKind) -> &'static str {
    match kind {
        devpod::JsonKind::Null => "NoneType",
        devpod::JsonKind::Bool => "bool",
        devpod::JsonKind::Number => "int",
        devpod::JsonKind::String => "str",
        devpod::JsonKind::Array => "list",
        devpod::JsonKind::Object => "dict",
    }
}

fn metadata_type_name(kind: metadata::JsonKind) -> &'static str {
    match kind {
        metadata::JsonKind::Null => "NoneType",
        metadata::JsonKind::Bool => "bool",
        metadata::JsonKind::Number => "int",
        metadata::JsonKind::String => "str",
        metadata::JsonKind::Array => "list",
        metadata::JsonKind::Object => "dict",
    }
}

// ---------------------------------------------------------------------------
// the failures
// ---------------------------------------------------------------------------

/// The one-line message for a devpod that is not installed.
///
/// Names both install routes because devpod ships with the pixi/conda package and
/// does not ship with the pip one. One line, so a completion helper that trips
/// over it cannot spew into the user's shell.
pub(crate) const DEVPOD_MISSING: &str = concat!(
    "devpod not found on PATH: dl cannot manage workspaces without it. ",
    "Install devpod from https://devpod.sh/docs/getting-started/install ",
    "(pixi/conda installs of devlaunch include it; pip installs do not)."
);

/// Why the workspace list could not be read, in one line.
///
/// The `error: ` prefix and the truncations are Python's: 200 characters of a
/// refusal's stderr, 120 of output that was not JSON.
pub(crate) fn listing_refusal(refused: &ListingUnreadable) -> String {
    match refused {
        ListingUnreadable::NotRun(NotRun::NotInstalled) => DEVPOD_MISSING.to_owned(),
        ListingUnreadable::NotRun(NotRun::TimedOut) => {
            "error: `devpod list` did not answer in time".to_owned()
        }
        ListingUnreadable::NotRun(NotRun::Blocked(failure)) => {
            format!("error: `devpod list` could not be run ({:?})", failure.kind)
        }
        ListingUnreadable::Failed { exit, stderr } => format!(
            "error: `devpod list` exited {}: {}",
            exit_status(*exit),
            python_repr(&clipped(stderr.trim(), 200))
        ),
        ListingUnreadable::Unreadable(NotAListing::Silence) => "error: devpod said nothing when \
             asked to list workspaces; it prints `[]` when there are none"
            .to_owned(),
        ListingUnreadable::Unreadable(NotAListing::NotJson { output, .. }) => format!(
            "error: devpod's workspace listing is not JSON: {}",
            python_repr(&clipped(output, 120))
        ),
        ListingUnreadable::Unreadable(NotAListing::NotAnArray { kind }) => format!(
            "error: expected devpod to list workspaces, got {}",
            json_type_name(*kind)
        ),
        ListingUnreadable::Unreadable(NotAListing::EntryNotAnObject { kind }) => format!(
            "error: expected each listed workspace to be an object, got {}",
            json_type_name(*kind)
        ),
        ListingUnreadable::Unreadable(NotAListing::SourceNotAnObject { workspace_id, kind }) => {
            format!(
                "error: expected workspace {} to have an object for its source, got {}",
                python_repr(workspace_id),
                json_type_name(*kind)
            )
        }
    }
}

/// Why a devpod call never ran at all, in one line.
///
/// The listing has its own copy of this because Python's message for it names
/// `devpod list` and predates the shared one; this is every other call, named by the
/// subcommand that did not happen.
pub(crate) fn devpod_not_run(call: &str, refused: &NotRun) -> String {
    match refused {
        NotRun::NotInstalled => DEVPOD_MISSING.to_owned(),
        NotRun::TimedOut => format!("error: `devpod {call}` did not answer in time"),
        NotRun::Blocked(failure) => {
            format!(
                "error: `devpod {call}` could not be run ({:?})",
                failure.kind
            )
        }
    }
}

/// Whether this refusal is the one that means "devpod is not installed", which is
/// the only one that exits 127.
pub(crate) fn is_devpod_missing(refused: &ListingUnreadable) -> bool {
    matches!(refused, ListingUnreadable::NotRun(NotRun::NotInstalled))
}

/// A child's ending, as Python's `returncode` spells it: negative for a signal.
fn exit_status(exit: Exit) -> String {
    match exit {
        Exit::Code(code) => code.to_string(),
        Exit::Signal(signal) => format!("-{signal}"),
    }
}

/// The first `limit` characters of `text` — Python's `text[:limit]`, which counts
/// characters and not bytes.
fn clipped(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// The `dl: …` lines a metadata load has to say something about.
///
/// One per notice, in the order the load found them, on stderr — stdout is parsed
/// by the completion machinery. The wording is `storage.py`'s, because these are
/// the sentences a user has already seen when their `metadata.json` was damaged.
pub(crate) fn metadata_notices(notices: &[metadata::Notice]) -> Vec<String> {
    notices.iter().map(metadata_notice).collect()
}

fn metadata_notice(notice: &metadata::Notice) -> String {
    let line = match notice {
        metadata::Notice::FileUnusable {
            path,
            problem,
            quarantine,
        } => {
            let reason = match problem {
                metadata::FileProblem::Unreadable(failure) => format!(
                    "could not read metadata file {} ({})",
                    path.display(),
                    failure.message
                ),
                metadata::FileProblem::NotJson { reason } => {
                    format!("could not read metadata file {} ({reason})", path.display())
                }
                metadata::FileProblem::NotAnObject { found } => format!(
                    "metadata file {} is not a JSON object (found {})",
                    path.display(),
                    metadata_type_name(*found)
                ),
            };
            match quarantine {
                metadata::Quarantine::MovedAside { path } => format!(
                    "{reason}; moved it to {} and started with empty metadata",
                    path.display()
                ),
                metadata::Quarantine::CouldNotMove { path, failure } => format!(
                    "{reason}; could not move it aside to {} ({}); starting with empty metadata",
                    path.display(),
                    failure.message
                ),
            }
        }
        metadata::Notice::VersionHeaderUnusable { path, found } => format!(
            "metadata file {} has an invalid \"version\" header ({}); reading it as schema \
             version 1",
            path.display(),
            python_repr(&found.to_string())
        ),
        metadata::Notice::VersionFromNewerBuild {
            path,
            found,
            understood,
        } => format!(
            "{} was written by a newer devlaunch (schema version {found}, this build \
             understands {understood}); its entries are loaded as-is, and the next change \
             rewrites the whole file as schema version {understood}",
            path.display()
        ),
        metadata::Notice::SectionUnusable {
            path,
            section,
            found,
        } => format!(
            "ignoring the \"{}\" section of {}: expected an object, found {}",
            section.key(),
            path.display(),
            metadata_type_name(*found)
        ),
        metadata::Notice::EntryUnusable {
            path,
            section,
            key,
            problem,
        } => {
            let head = format!(
                "skipping malformed {} entry {} in {}",
                section.key(),
                python_repr(key),
                path.display()
            );
            match problem {
                metadata::EntryProblem::NotAnObject { found } => format!(
                    "{head}: expected an object, found {}",
                    metadata_type_name(*found)
                ),
                metadata::EntryProblem::NotRebuilt { reason } => {
                    format!("{head}: {}", python_repr(reason))
                }
            }
        }
        metadata::Notice::EntryHasUnknownFields {
            path,
            section,
            key,
            fields,
        } => format!(
            "{} entry {} in {} has field(s) this build does not understand ({}); they are \
             dropped when it is rewritten",
            section.key(),
            python_repr(key),
            path.display(),
            fields.join(", ")
        ),
        metadata::Notice::UnknownTopLevelKeys { path, keys } => format!(
            "{} has top-level key(s) this build does not understand ({}); they are dropped \
             when it is rewritten",
            path.display(),
            keys.join(", ")
        ),
        metadata::Notice::OriginalPreserved { path, backup } => {
            let reason = format!(
                "rewriting {} in this build's format will drop information it currently holds",
                path.display()
            );
            match backup {
                metadata::Backup::Copied { path } => {
                    format!("{reason}; preserved the original at {}", path.display())
                }
                metadata::Backup::CouldNotCopy { path, failure } => format!(
                    "{reason}; could not preserve the original at {} ({})",
                    path.display(),
                    failure.message
                ),
            }
        }
    };
    format!("dl: {line}")
}

/// Why the config could not be read, in one line.
///
/// Every arm named, because the three are fixed in three different places: an
/// environment with no home directory, a file that cannot be read, and a file
/// whose values are not of the types their keys must be (divergence rows 8 and 9 —
/// a mistyped `fetch_interval` is a refusal here where Python coerced it).
pub(crate) fn config_error(error: &config::ConfigError) -> String {
    match error {
        config::ConfigError::NoHomeDirectory => {
            "this machine names no home directory, so dl cannot find its config".to_owned()
        }
        config::ConfigError::Unreadable { path, source } => {
            format!("could not read {} ({source})", path.display())
        }
        config::ConfigError::Malformed { path, reason } => {
            format!("{} is not usable: {reason}", path.display())
        }
    }
}

/// Why a metadata write or open failed, in one line.
pub(crate) fn metadata_error(error: &metadata::MetadataError) -> String {
    // The arms carry different payloads and only the ones a read-side command can
    // reach are spelled out; the rest are named by their debug shape rather than
    // dropped, so nothing is silent.
    match error {
        metadata::MetadataError::CreateDir { path, failure } => format!(
            "could not create the directory for dl's records at {} ({})",
            path.display(),
            failure.message
        ),
        other => format!("could not read dl's records: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// the lifecycle notices
// ---------------------------------------------------------------------------

/// The lines the lifecycle flows' notices read as, in the order they happened.
///
/// Every one of them is a `logging.*` call Python made, so every one of them goes
/// to stderr — printed by the caller, which is the only thing that knows whether
/// a command has finished saying what it has to say.
pub(crate) fn lifecycle_notices(notices: &[LifecycleNotice]) -> Vec<String> {
    notices.iter().map(lifecycle_notice).collect()
}

fn lifecycle_notice(notice: &LifecycleNotice) -> String {
    match notice {
        LifecycleNotice::CloneRemoved { workspace_id } => {
            format!("Removed local clone for {workspace_id}")
        }
        LifecycleNotice::CloneNotRemoved { reason, .. } => {
            format!("Failed to remove local clone: {reason}")
        }
        LifecycleNotice::WorkspaceNotDeleted {
            workspace_id,
            stderr,
            ..
        } => format!("Failed to delete workspace {workspace_id}: {stderr}"),
        LifecycleNotice::RecordNotDropped { path, reason } => {
            format!("Could not drop the record for {}: {reason}", path.display())
        }
        LifecycleNotice::AddressingRecordedWorkspace {
            recorded,
            derived,
            owner,
            repo,
            branch,
        } => format!(
            "Addressing devpod workspace '{recorded}' from the record for {owner}/{repo}@{branch}; \
             this build derives '{derived}'"
        ),
        LifecycleNotice::Cache(cache) => cache_notice(cache),
    }
}

/// One storage-flow notice, in the words the module that logged it used.
///
/// `worktree/repo_manager.py` and `worktree/workspace_clone.py` write these
/// through module loggers, which `dl.py`'s `basicConfig(format="%(message)s")`
/// sends to stderr as the bare message — so there is no logger name, no level and
/// no prefix in any of them.
fn cache_notice(notice: &CacheNotice) -> String {
    match notice {
        CacheNotice::AdoptedBareClone { owner, repo, bare } => {
            format!(
                "Repository {owner}/{repo} already exists at {}",
                bare.display()
            )
        }
        CacheNotice::ClearedPartialClone { bare } => {
            format!("Removing partial clone at {}", bare.display())
        }
        CacheNotice::RecordWithoutClone { owner, repo, .. } => {
            format!("Repository {owner}/{repo} metadata exists but directory missing")
        }
        CacheNotice::RefNotFetched {
            owner,
            repo,
            branch,
            reason,
        } => format!("Could not fetch {branch} for {owner}/{repo}: {reason}"),
        CacheNotice::DefaultBranchUnknown { reason, .. } => {
            format!("Failed to resolve default branch: {reason}")
        }
        CacheNotice::PreparedFromStaleBase {
            owner,
            repo,
            branch,
            base,
            reason,
        } => format!(
            "Prepared '{owner}/{repo}@{branch}' from the cache's '{base}', which could not be \
             refreshed ({reason}); it may be behind the remote."
        ),
        CacheNotice::LfsCacheNotFilled { reason } => {
            format!("Could not fill the cache's git-lfs store: {reason}")
        }
        CacheNotice::LfsNotPulledFromCache { reason } => {
            format!("Could not materialize git-lfs objects from the cache: {reason}")
        }
        CacheNotice::TrackedFilesNotListed { reason } => {
            format!("Could not list tracked files: {reason}")
        }
        CacheNotice::LfsFilesNotListed { reason } => {
            format!("Could not list git-lfs files: {reason}")
        }
        CacheNotice::WorkspaceNotRecorded { reason } => {
            format!("Failed to save workspace metadata: {reason}")
        }
        CacheNotice::WorkspaceRecordNotRemoved { reason } => {
            format!("Failed to remove workspace metadata: {reason}")
        }
        CacheNotice::CloneNotNamed {
            owner,
            repo,
            branch,
            reason,
        } => format!("cannot name the clone directory for {owner}/{repo}@{branch}: {reason}"),
        CacheNotice::Metadata(notice) => metadata_notice(notice),
    }
}

/// Why an owner, repo or ref is not a name `dl` will build a path out of.
pub(crate) fn unsafe_name(refused: &UnsafeName) -> String {
    let part = match refused.part {
        NamePart::Owner => "owner",
        NamePart::Repo => "repo",
        NamePart::Ref => "ref",
    };
    format!("Invalid git {part} name: {}", python_repr(&refused.name))
}

// ---------------------------------------------------------------------------
// dl <ws> rm, and the delete under it
// ---------------------------------------------------------------------------

/// Why `dl <ws> rm` will not delete this workspace, and the way past it.
///
/// `spec` is the target *as the user typed it*, because the sentence ends in a
/// command they can run: a refusal that echoed the resolved id would print a line
/// that works, but not the line they typed.
pub(crate) fn removal_refusal(refused: &RemovalRefused, spec: &str) -> String {
    match refused {
        RemovalRefused::WouldLose {
            workspace_id,
            losses,
        } => format!(
            "{workspace_id} holds {}. Push or commit it, or run: dl {spec} rm --force",
            losses.describe()
        ),
        RemovalRefused::CouldNotTell {
            workspace_id,
            reason,
        } => format!(
            "{workspace_id}: {reason}. devlaunch will not delete a clone it cannot check. Look at \
             it, or run: dl {spec} rm --force"
        ),
    }
}

/// devpod would not let go of the workspace, and the clone was kept.
pub(crate) fn delete_refused(workspace: &str) -> String {
    format!(
        "devpod could not delete {workspace}; keeping the local clone so it stays retryable. If \
         its devcontainer.json moved, restore the path or run: devpod delete {workspace} --force"
    )
}

/// The one sentence a target no command can address gets.
pub(crate) fn unknown_workspace(target: &str) -> String {
    format!(
        "Unknown workspace '{target}'. Use 'dl --ls' to list workspaces, or specify owner/repo or \
         ./path"
    )
}

// ---------------------------------------------------------------------------
// the disk neither cleanup frees
// ---------------------------------------------------------------------------

/// Named by `--purge` on every ending, and by `--prune` on every ending that got
/// as far as looking at a directory.
///
/// A sentence and not a measurement: nothing here runs `docker`, so there is
/// nothing to fail on a machine where Docker is absent, and nothing that takes a
/// moment on a command whose work is already done.
pub(crate) const DOCKER_BOUNDARY: &str = concat!(
    "devlaunch does not manage Docker images or volumes: the containers these workspaces used ",
    "may still hold disk, and `docker system df` shows what Docker is holding."
);

// ---------------------------------------------------------------------------
// --purge
// ---------------------------------------------------------------------------

/// What a purge would take, printed before the question is asked.
///
/// The workspaces devlaunch did not create are *named* rather than merely left out
/// of the count: a user who asked for a clean slate and gets survivors should
/// learn it while saying no is still an option.
pub(crate) fn purge_plan_lines(plan: &PurgePlan) -> Vec<String> {
    let mut lines = vec![
        "This will remove all devlaunch data:".to_owned(),
        format!("  - {} DevPod workspace(s)", plan.ownership.mine.len()),
        format!(
            "  - {}/ (workspace clones, repo caches, the shared pixi cache, completions)",
            plan.cache_dir.display()
        ),
    ];
    if !plan.ownership.foreign.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "Leaving {} workspace(s) devlaunch did not create:",
            plan.ownership.foreign.len()
        ));
        lines.extend(
            plan.ownership
                .foreign
                .iter()
                .map(|workspace| format!("  - {}", workspace.id)),
        );
    }
    lines.push(String::new());
    lines
}

/// One rendered line, and which stream it belongs on.
///
/// Every other renderer here answers with lines for one stream, because its caller
/// knows which. A purge step does not: the step that says what is about to happen
/// is output, and the step that says it did not happen is a `logging.warning`, and
/// they arrive through the same callback.
pub(crate) enum Line {
    Out(String),
    Err(String),
}

/// The line a purge says before a round trip that may take a while.
///
/// Handed over as it happens rather than collected, which is why this renders one
/// step rather than a report: "Deleting workspace X" said afterwards is not said in
/// time. A failure is the same event as [`LifecycleNotice::WorkspaceNotDeleted`] and
/// is said here, where it happens, rather than twice.
pub(crate) fn purge_step(step: &PurgeStep) -> Line {
    match step {
        PurgeStep::Deleting { workspace_id } => {
            Line::Out(format!("Deleting DevPod workspace: {workspace_id}"))
        }
        PurgeStep::NotDeleted {
            workspace_id,
            stderr,
            ..
        } => Line::Err(format!(
            "Failed to delete workspace {workspace_id}: {stderr}"
        )),
    }
}

/// How a purge ended, in the words for that ending.
pub(crate) fn purge_outcome(outcome: &PurgeOutcome) -> Vec<String> {
    match outcome {
        PurgeOutcome::NothingToPurge => vec!["No data to purge.".to_owned()],
        // Deliberately nothing: a purge that deleted four workspaces and found no
        // cache directory has not done nothing, and Python's branch for it prints
        // neither sentence.
        PurgeOutcome::NoCacheDirectory => Vec::new(),
        PurgeOutcome::Removed { cache_dir } => {
            vec![format!("Removed: {}", cache_dir.display())]
        }
        PurgeOutcome::RemovedWhatItCould { cache_dir, refused } => report_refusals(
            refused.iter(),
            &format!(
                "Removed what was permitted under {}. These refused:",
                cache_dir.display()
            ),
            std::slice::from_ref(cache_dir),
        ),
        PurgeOutcome::RemovedNothing { cache_dir, refused } => report_refusals(
            refused.iter(),
            &format!(
                "Removed nothing under {}. These refused:",
                cache_dir.display()
            ),
            std::slice::from_ref(cache_dir),
        ),
    }
}

/// What would not come away, and the one thing that usually clears it.
///
/// Shared by `--purge` and `--prune`, because a second copy of this advice is a
/// second copy to keep true — and the advice is the part most likely to change,
/// being the only part that is a guess.
pub(crate) fn report_refusals<'a>(
    refused: impl Iterator<Item = &'a Refusal>,
    headline: &str,
    remove_by_hand: &[std::path::PathBuf],
) -> Vec<String> {
    let mut lines = vec![headline.to_owned()];
    for refusal in refused {
        lines.push(format!(
            "  - {}: {}",
            refusal.path.display(),
            refusal_reason(&refusal.reason)
        ));
    }
    lines.push(String::new());
    lines.push("Usually this means a container wrote them as a different user, and:".to_owned());
    // Quoted: these paths descend from $XDG_CACHE_HOME or $HOME, and a space in one
    // turns a pasted `sudo rm -rf` into two targets, the first of them wrong.
    lines.push(format!(
        "  sudo rm -rf {}",
        remove_by_hand
            .iter()
            .map(|path| shell_quoted(path))
            .collect::<Vec<_>>()
            .join(" ")
    ));
    lines.push(
        "clears them. Check the reasons above first -- it does not fix all of them.".to_owned(),
    );
    lines
}

fn refusal_reason(reason: &RefusalReason) -> String {
    match reason {
        RefusalReason::System(words) => words.clone(),
        RefusalReason::RootIsSymlink { points_at } => {
            let to = match points_at {
                Some(target) => format!(" to {}", target.display()),
                None => String::new(),
            };
            format!("is a symbolic link{to}, which a purge will not follow")
        }
    }
}

/// `path` as `shlex.quote` would write it.
///
/// The safe set is Python's: a path built only of those characters is printed
/// bare, and anything else is single-quoted with embedded quotes broken out.
fn shell_quoted(path: &Path) -> String {
    let text = path.display().to_string();
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if !text.is_empty() && text.chars().all(safe) {
        return text;
    }
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

// ---------------------------------------------------------------------------
// --prune
// ---------------------------------------------------------------------------

/// What is going, what is staying and why, before anything is asked.
///
/// Printed whether or not there is anything to do, because the reason a directory
/// is *staying* is the half a person cannot get anywhere else — `dl --ls` lists
/// workspaces, and a clone with no workspace has no row there to appear in.
pub(crate) fn prune_plan_lines(plan: &PrunePlan) -> Vec<String> {
    let mut lines = vec![
        format!("Clone directories under {}:", plan.root.display()),
        String::new(),
    ];
    if !plan.removing.is_empty() {
        lines.push(format!(
            "Removing {} that nothing references -- {}:",
            plan.removing.len(),
            describe_usage(&plan.freed())
        ));
        for reclaimable in &plan.removing {
            let mut line = format!(
                "  - {} ({})",
                reclaimable.path.display(),
                describe_usage(&reclaimable.usage)
            );
            // What `--force` is answering, on the line of the directory it answers
            // for. Without it the plan reads the same for a clone holding an
            // afternoon's uncommitted work as for an empty one.
            if let Promotion::Insisted { despite } = &reclaimable.promotion {
                line = format!("{line} -- holds {}, removing anyway", objection(despite));
            }
            lines.push(line);
        }
        lines.push(String::new());
    }
    if !plan.keeping.is_empty() {
        lines.push(format!("Leaving {}:", plan.keeping.len()));
        for kept in &plan.keeping {
            lines.push(format!(
                "  - {}: {}",
                kept.path.display(),
                kept_because(&kept.because)
            ));
        }
        lines.push(String::new());
    }
    if !plan.stale_records.is_empty() {
        lines.push(format!(
            "Dropping {} record(s) of directories already gone.",
            plan.stale_records.len()
        ));
        lines.push(String::new());
    }
    if plan.nothing_to_do() {
        lines.push("Nothing to prune.".to_owned());
    }
    lines
}

/// Why one clone directory is staying, as the report says it.
fn kept_because(because: &KeptBecause) -> String {
    match because {
        KeptBecause::StillOpened { workspace_id } => {
            format!("workspace {workspace_id} still opens it")
        }
        KeptBecause::Objected(objected) => {
            format!(
                "holds {} -- add --force to remove it anyway",
                objection(objected)
            )
        }
        KeptBecause::RecordsDisagree {
            workspace_id,
            sourced_at,
        } => format!(
            "devpod lists workspace {workspace_id} and sources it at {sourced_at}; see \
             devlaunch#88"
        ),
    }
}

/// What removing a clone would destroy or risk, as the clause after "holds".
fn objection(objected: &Objection) -> String {
    match objected {
        Objection::WouldLose(losses) => losses.describe(),
        Objection::CouldNotTell(reason) => {
            format!("work git could not be asked about ({reason})")
        }
    }
}

/// Which live workspaces could not be placed, and that nothing went.
///
/// Not a warning above a report: a workspace whose source cannot be followed could
/// be opening *any* of the candidates, so while one exists there is no directory
/// either command can honestly call unreferenced. The two callers differ in two
/// words, and the sentence under them is deliberately the same one.
pub(crate) fn report_unlocatable(
    unlocatable: &NonEmpty<Unlocatable>,
    command: &str,
    outcome: &str,
) -> Vec<String> {
    let mut lines = vec![format!(
        "dl {command} cannot follow these live workspaces' sources:"
    )];
    for source in unlocatable.iter() {
        lines.push(format!("  - {}: {}", source.workspace_id, source.detail));
    }
    lines.push(String::new());
    lines.push(format!(
        "{outcome}: no clone is unreferenced while a workspace is unaccounted for."
    ));
    lines
}

/// What the acting pass did.
///
/// The withheld lines say *that this was not so when the plan was printed*, which
/// is the whole of what a second classification has to tell somebody who has
/// already read the first one.
pub(crate) fn prune_report_lines(report: &PruneReport) -> Vec<String> {
    let mut lines = vec![format!(
        "Removed {} clone director(ies) -- {}.",
        report.removed.len(),
        describe_usage(&report.freed())
    )];
    for withheld in &report.withheld {
        lines.push(format!(
            "Left {}: {}. That was not so when the plan above was printed.",
            withheld.path.display(),
            kept_because(&withheld.because)
        ));
    }
    if !report.refused.is_empty() {
        let by_hand: Vec<std::path::PathBuf> = report
            .refused
            .iter()
            .map(|refusal| refusal.path.clone())
            .collect();
        lines.extend(report_refusals(
            report.refused.iter(),
            "Some directories would not come away. These refused:",
            &by_hand,
        ));
    }
    lines
}

// ---------------------------------------------------------------------------
// --reconcile
// ---------------------------------------------------------------------------

/// What would be re-pointed and what would only be named.
pub(crate) fn reconcile_plan_lines(plan: &ReconcilePlan) -> Vec<String> {
    let mut lines = vec![
        format!(
            "devpod workspaces sourced under {} at something that is not a clone:",
            plan.root.display()
        ),
        String::new(),
    ];
    if !plan.adopting.is_empty() {
        lines.push(format!("Re-pointing {}:", plan.adopting.len()));
        for adoptable in &plan.adopting {
            lines.push(format!(
                "  - {}: {} -> {}",
                adoptable.workspace_id,
                adoptable.sourced_at,
                adoptable.clone.display()
            ));
        }
        lines.push(String::new());
        // Stated before the confirmation rather than after the change, because it is
        // part of what is being consented to: the container was built with the dead
        // path bind-mounted into it, and no record change moves a running mount.
        lines.push(
            "Each of these needs `dl <workspace> recreate` afterwards: the container".to_owned(),
        );
        lines.push(
            "still has the old source bind-mounted, and no record change moves a mount.".to_owned(),
        );
        lines.push(String::new());
    }
    if !plan.reporting.is_empty() {
        lines.push(format!(
            "Leaving {}, which dl will not guess at:",
            plan.reporting.len()
        ));
        for unadoptable in &plan.reporting {
            lines.push(format!(
                "  - {} ({}): {}",
                unadoptable.workspace_id,
                unadoptable.sourced_at,
                not_adopted(&unadoptable.because)
            ));
        }
        lines.push(String::new());
        // Named as the user's decision, not offered as a follow-up dl will make.
        lines.push(
            "Nothing here is deleted. `dl <workspace> rm` is how one goes, if it should."
                .to_owned(),
        );
        lines.push(String::new());
    }
    if plan.nothing_to_do() {
        lines.push("Nothing to reconcile.".to_owned());
    }
    lines
}

fn not_adopted(because: &NotAdopted) -> String {
    match because {
        NotAdopted::NoCloneAnswers => "no clone of that repository answers to this name".to_owned(),
        NotAdopted::NameAnsweredByManyClones(answers) => format!(
            "{} clones answer to this name, so none of them can: {}",
            answers.iter().count(),
            answers
                .iter()
                .map(|answer| answer.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        NotAdopted::CloneWantedByManyWorkspaces { clone, workspaces } => {
            format!(
                "{workspaces} workspaces match {}, so none of them can",
                clone.display()
            )
        }
    }
}

/// Why one devpod record could not be re-pointed.
pub(crate) fn repoint_failure(failure: &RepointFailure) -> String {
    match failure {
        RepointFailure::Unreadable { path, reason } => {
            format!("could not read {}: {reason}", path.display())
        }
        RepointFailure::NotADevpodRecord { path } => format!(
            "{} is not a devpod workspace record dl can repair",
            path.display()
        ),
        RepointFailure::Unwritable { path, reason } => {
            format!("could not write {}: {reason}", path.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use devlaunch_core::flows::disk_usage::DiskUsage;
    use devlaunch_core::flows::listing::{SourceDescription, SourceKind};

    use super::*;

    fn row(id: &str, kind: SourceKind, detail: &str, size: SizeCell, when: LastUsed) -> TableRow {
        TableRow {
            id: id.to_owned(),
            source: SourceDescription {
                kind,
                detail: detail.to_owned(),
            },
            size,
            last_used: when,
        }
    }

    fn table(rows: Vec<TableRow>) -> WorkspaceTable {
        WorkspaceTable::Rows(NonEmpty::of(rows).expect("at least one row"))
    }

    #[test]
    fn a_machine_with_no_workspaces_gets_one_sentence() {
        assert_eq!(
            table_lines(&WorkspaceTable::Nothing, Sizes::Skip),
            ["No workspaces found."]
        );
    }

    #[test]
    fn the_workspace_heading_overhangs_a_narrow_column() {
        // Python's `id_width` is a maximum over the rows only, so a listing of
        // short ids leaves `WORKSPACE` wider than the column it heads and the
        // headings after it shifted right of their cells. Pinned because it is
        // what the harness compares against, not because it is good.
        let lines = table_lines(
            &table(vec![row(
                "ws",
                SourceKind::Local,
                "/tmp/x",
                SizeCell::NoColumn,
                LastUsed::At("2026-08-01 10:11:12".to_owned()),
            )]),
            Sizes::Skip,
        );
        assert_eq!(
            lines[0], "WORKSPACE  TYPE   SOURCE  LAST USED",
            "the heading is padded to its own length, not to the column's"
        );
        assert_eq!(lines[2], "ws  local  /tmp/x  2026-08-01 10:11:12");
    }

    #[test]
    fn the_size_heading_counts_as_a_cell() {
        // The other half of the quirk: `SIZE` *is* measured, so a column of
        // dashes cannot leave the heading wider than the column.
        let lines = table_lines(
            &table(vec![row(
                "workspace-with-a-long-id",
                SourceKind::Git,
                "https://github.com/o/r.git",
                SizeCell::NotOurs,
                LastUsed::Never,
            )]),
            Sizes::Measure,
        );
        assert!(
            lines[0].ends_with("  SIZE  LAST USED"),
            "expected a SIZE column, got {:?}",
            lines[0]
        );
        assert!(
            lines[2].ends_with("     -  never"),
            "expected the dash right-aligned under SIZE, got {:?}",
            lines[2]
        );
    }

    #[test]
    fn the_separator_is_the_measured_columns_plus_thirty() {
        let lines = table_lines(
            &table(vec![row(
                "ws",
                SourceKind::Local,
                "/tmp/x",
                SizeCell::NoColumn,
                LastUsed::Never,
            )]),
            Sizes::Skip,
        );
        // 2 (id) + 5 (type) + 6 (source) + 0 (no size column) + 30
        assert_eq!(lines[1], "-".repeat(43));
    }

    #[test]
    fn a_measured_size_reads_as_a_size_and_a_floor_reads_as_a_floor() {
        let lines = table_lines(
            &table(vec![
                row(
                    "a",
                    SourceKind::Local,
                    "/x",
                    SizeCell::Measured(DiskUsage::measured(2048)),
                    LastUsed::Never,
                ),
                row(
                    "b",
                    SourceKind::Local,
                    "/y",
                    SizeCell::NotOurs,
                    LastUsed::Never,
                ),
            ]),
            Sizes::Measure,
        );
        assert!(lines[2].contains("2.0 KiB"), "{:?}", lines[2]);
        assert!(lines[3].contains(" -  never"), "{:?}", lines[3]);
    }

    #[test]
    fn the_unknown_source_column_is_the_kind_word_and_the_payload() {
        let lines = table_lines(
            &table(vec![row(
                "ws",
                SourceKind::Unknown,
                r#"{"image": "ubuntu:22.04"}"#,
                SizeCell::NoColumn,
                LastUsed::Never,
            )]),
            Sizes::Skip,
        );
        assert!(lines[2].contains("unknown  {\"image\": \"ubuntu:22.04\"}"));
    }

    #[test]
    fn the_json_document_is_spelled_the_way_python_spells_it() {
        let document = serde_json::json!([{ "id": "ws", "devlaunch": true, "unsaved": null }]);
        assert_eq!(
            python_json_document(&document),
            "[\n  {\n    \"id\": \"ws\",\n    \"devlaunch\": true,\n    \"unsaved\": null\n  }\n]"
        );
    }

    #[test]
    fn an_empty_document_is_two_characters() {
        assert_eq!(python_json_document(&serde_json::json!([])), "[]");
    }

    #[test]
    fn non_ascii_is_escaped_as_python_escapes_it() {
        // `ensure_ascii=True`, with anything outside the basic plane written as
        // the surrogate pair Python writes.
        assert_eq!(
            python_json_document(&serde_json::json!("héllo 🚀")),
            r#""h\u00e9llo \ud83d\ude80""#
        );
    }

    #[test]
    fn repr_quotes_the_way_python_quotes() {
        assert_eq!(python_repr("plain"), "'plain'");
        assert_eq!(python_repr("not json\n"), "'not json\\n'");
        assert_eq!(python_repr("it's"), "\"it's\"");
        assert_eq!(python_repr("it's \"quoted\""), "'it\\'s \"quoted\"'");
        assert_eq!(python_repr("a\\b"), "'a\\\\b'");
        assert_eq!(python_repr("bell\u{7}"), "'bell\\x07'");
    }

    #[test]
    fn a_refusal_is_one_line_naming_what_devpod_said() {
        assert_eq!(
            listing_refusal(&ListingUnreadable::Failed {
                exit: Exit::Code(1),
                stderr: "context not found: default\n".to_owned(),
            }),
            "error: `devpod list` exited 1: 'context not found: default'"
        );
        assert_eq!(
            listing_refusal(&ListingUnreadable::Unreadable(NotAListing::NotJson {
                output: "not json at all\n".to_owned(),
                reason: "expected value".to_owned(),
            })),
            "error: devpod's workspace listing is not JSON: 'not json at all\\n'"
        );
        assert_eq!(
            listing_refusal(&ListingUnreadable::Unreadable(NotAListing::Silence)),
            "error: devpod said nothing when asked to list workspaces; it prints `[]` when \
             there are none"
        );
    }

    #[test]
    fn a_signal_reads_as_pythons_negative_returncode() {
        assert_eq!(
            listing_refusal(&ListingUnreadable::Failed {
                exit: Exit::Signal(9),
                stderr: String::new(),
            }),
            "error: `devpod list` exited -9: ''"
        );
    }

    #[test]
    fn a_missing_devpod_is_the_one_refusal_that_names_an_install() {
        let missing = ListingUnreadable::NotRun(NotRun::NotInstalled);
        assert!(is_devpod_missing(&missing));
        assert_eq!(listing_refusal(&missing), DEVPOD_MISSING);
        assert!(!is_devpod_missing(&ListingUnreadable::Unreadable(
            NotAListing::Silence
        )));
    }
}
