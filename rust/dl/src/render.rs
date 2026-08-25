//! Typed results in, bytes out. Every user-facing English word `dl` prints is
//! written in this file or in [`crate::commands`]; core holds none of it.
//!
//! Everything here is a pure function of a value core produced, which is what
//! lets the table's column arithmetic and the JSON document's spelling be tested
//! without a devpod, a cache or a process.

use std::fmt::Write as _;
use std::io;
use std::path::Path;

use devlaunch_core::clients::devpod::{ListingUnreadable, NotAListing, NotRun};
use devlaunch_core::clients::gh::{GhEvent, GhUnavailable};
use devlaunch_core::clients::ssh::{NotRun as SshNotRun, UnsafeRequest};
use devlaunch_core::domain::config;
use devlaunch_core::domain::locks::LockError;
use devlaunch_core::domain::metadata;
use devlaunch_core::domain::workspace_id::{NamePart, UnsafeName};
use devlaunch_core::domain::workspace_state::NonEmpty;
use devlaunch_core::domain::xdg;
use devlaunch_core::flows::agent_worktrees::{
    SeenAs, WorktreeKept, WorktreeObjection, WorktreePromotion, WorktreeReport, WorktreeSweep,
};
use devlaunch_core::flows::branch_manager::BranchError;
use devlaunch_core::flows::completion_cache::CompletionData;
use devlaunch_core::flows::disk_usage::describe_usage;
use devlaunch_core::flows::launch::{
    BranchNotNamed, LaunchAborted, LaunchNotice, LaunchRefusal, NotPrepared, SessionRefused,
};
use devlaunch_core::flows::lifecycle::{
    KeptBecause, LifecycleNotice, NotAdopted, Objection, Promotion, PrunePlan, PruneReport,
    PurgeOutcome, PurgePlan, PurgeStep, ReconcilePlan, RemovalRefused, RepointFailure, Unlocatable,
    VolumeRefusal,
};
use devlaunch_core::flows::listing::{
    CloneDisk, LastUsed, SizeCell, Sizes, TableRow, WorkspaceTable,
};
use devlaunch_core::flows::migration::{Listing, MigrationReport};
use devlaunch_core::flows::provision::{BundleFailed, FailureLevel, ProvisionEvent};
use devlaunch_core::flows::repo_manager::{
    CacheNotice, Cleanup, CloneError, EnsureRepoError, NotRefreshed, Refusal, RefusalReason,
    RemoveTreeError, WrongRepoLock,
};
use devlaunch_core::flows::workspace_clone::{
    EnsureBranchError, PrepareColdError, PrepareWorkspaceError, RemoveWorkspaceError,
};
use devlaunch_core::json::JsonKind;
use devlaunch_core::notices::Notices;
use devlaunch_core::shell;
use devlaunch_runner::{Exit, OsFailure};
use serde_json::Value;
use serde_json::ser::{Formatter, PrettyFormatter};

use crate::session::StartupError;

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
            SizeCell::Measured(disk) => size_of(disk),
        },
    }
}

/// One clone's size, with the part of it that is agent git worktrees named.
///
/// The parenthetical appears only where there is something to say, so the column
/// reads as it always did on a machine that has never run an agent in a
/// workspace. Where there is, it is the number that would otherwise be invisible:
/// on the host devlaunch#426 was found on, the worktrees were 82% of the cache and
/// no `--ls --size` row said so.
///
/// A part of the figure beside it and never an addition — the worktrees are inside
/// the clone.
fn size_of(disk: &CloneDisk) -> String {
    let total = describe_usage(disk.freed());
    match disk.worktrees_worth_naming() {
        Some(worktrees) => format!("{total} ({} in worktrees)", describe_usage(worktrees)),
        None => total,
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
pub fn python_repr(text: &str) -> String {
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
fn json_type_name(kind: JsonKind) -> &'static str {
    match kind {
        JsonKind::Null => "NoneType",
        JsonKind::Bool => "bool",
        JsonKind::Number => "int",
        JsonKind::String => "str",
        JsonKind::Array => "list",
        JsonKind::Object => "dict",
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
            format!(
                "error: `devpod list` could not be run ({})",
                os_error_phrase(failure)
            )
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
                "error: `devpod {call}` could not be run ({})",
                os_error_phrase(failure)
            )
        }
    }
}

/// Whether this refusal is the one that means "devpod is not installed", which is
/// the only one that exits 127.
pub(crate) fn is_devpod_missing(refused: &ListingUnreadable) -> bool {
    matches!(refused, ListingUnreadable::NotRun(NotRun::NotInstalled))
}

/// The OS's own phrasing for a refusal to start a program, as Python's
/// `str(OSError)` carries it.
///
/// `from_raw_os_error` turns the errno into the sentence the C library gives it
/// (`Permission denied (os error 13)`), which is what `dl --install` already
/// renders and what docs/rust-rewrite-plan.md row 4 promised for every OS refusal.
/// Debug-printing the [`std::io::ErrorKind`] instead (`(Uncategorized)`,
/// `(NotFound)`) leaked Rust's own vocabulary into a diagnostic Python spelled
/// differently. Only when the runner had no errno to carry does this fall back to
/// naming the kind.
fn os_error_phrase(failure: &OsFailure) -> String {
    match failure.errno {
        Some(errno) => std::io::Error::from_raw_os_error(errno).to_string(),
        None => format!("{:?}", failure.kind),
    }
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
                    json_type_name(*found)
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
            json_type_name(*found)
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
                    json_type_name(*found)
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

/// The `dl: …` lines the cache migration has to say, Python's `_announce`.
///
/// One line per kind of outcome, on stderr — stdout is parsed by the completion
/// machinery, which is why `migration.py::_notice` chose stderr. Core renders no
/// English (#251): the [`MigrationReport`] carries every fact these sentences
/// interpolate, and the words are Python's, byte for byte, because a user reading
/// them mid-migration is reading the same instructions the shipping `dl` printed —
/// most load-bearingly the orphaned-container line, the only pointer they get to
/// `dl --reconcile` and `dl <workspace> recreate`.
///
/// The order is `migrate_cache`'s: the record-less scan's refusals first (Python's
/// `_clone_dirs` said them during the walk), then `_announce`'s block.
pub(crate) fn migration_notices(report: &MigrationReport) -> Vec<String> {
    let mut lines = Vec::new();

    for not_scanned in &report.not_scanned {
        lines.push(format!(
            "dl: could not scan {} for old workspace clones ({})",
            not_scanned.path.display(),
            not_scanned.reason
        ));
    }

    if let Some(first) = report.renamed.first() {
        let count = report.renamed.len();
        lines.push(format!(
            "dl: migrated {count} workspace clone director{} to the new id scheme (e.g. {} -> {})",
            plural_directory(count),
            leaf(&first.from),
            leaf(&first.to),
        ));
    }

    for failed in &report.failed {
        lines.push(format!(
            "dl: could not rename {} to {} ({}); it was left where it is",
            failed.from.display(),
            failed.to.display(),
            failed.reason
        ));
    }

    if !report.missing.is_empty() {
        let count = report.missing.len();
        lines.push(format!(
            "dl: {count} metadata record(s) pointed at a clone directory that is no longer \
             there; they now point at their new-scheme path"
        ));
    }

    for unusable in &report.unusable {
        lines.push(format!(
            "dl: left {} as it is: its recorded branch {} is not a usable git ref, so no id \
             can be derived for it",
            unusable.path.display(),
            python_repr(&unusable.branch)
        ));
    }

    for blocked in &report.blocked {
        lines.push(format!(
            "dl: left {} as it is: its new name {} is already another workspace's clone \
             directory; move or delete one of them by hand",
            blocked.from.display(),
            leaf(&blocked.to)
        ));
    }

    if !report.unmigrated.is_empty() {
        let count = report.unmigrated.len();
        // Python's `_write_lines` announces its own failure in place, before the
        // summary line, and returns None so the summary drops its "; listed in".
        let suffix = match listing_path(&report.unmigrated_listing, &mut lines) {
            Some(path) => format!("; listed in {}", path.display()),
            None => String::new(),
        };
        lines.push(format!(
            "dl: {count} clone director{} could not be renamed (no metadata record, so the \
             branch they were cloned for is unknown) and were left as they are{suffix}",
            plural_directory(count),
        ));
    }

    if !report.orphaned_ids.is_empty() {
        let count = report.orphaned_ids.len();
        let cleanup = match listing_path(&report.orphan_listing, &mut lines) {
            Some(path) => format!("xargs -r -n1 devpod delete < {}", path.display()),
            None => "devpod delete <old-id>, one per workspace".to_owned(),
        };
        lines.push(format!(
            "dl: {count} devpod container(s) still carry the old workspace ids and are now \
             orphaned; dl --reconcile re-points them at the renamed clones, and dl <workspace> \
             recreate finishes each repair -- that restores the clone association and the \
             workspace, not state that lived only inside the old container, and only until the \
             branch is launched again (a fresh launch claims the clone, and reconcile never \
             re-points a clone a live container holds). dl deletes nothing for you; for the ones \
             you are finished with: {cleanup}"
        ));
    }

    lines
}

/// Python's `'y' if n == 1 else 'ies'`, appended to `director`.
fn plural_directory(count: usize) -> &'static str {
    if count == 1 { "y" } else { "ies" }
}

/// A path's final component, Python's `Path.name`.
fn leaf(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Python's `_write_lines` return, folded into rendering: the path when the listing
/// was written, and — when it could not be — the `dl: could not write …` line said
/// in place (as `_write_lines` printed it before returning None) plus `None`, so the
/// caller degrades to its no-listing wording.
fn listing_path<'a>(listing: &'a Listing, lines: &mut Vec<String>) -> Option<&'a Path> {
    match listing {
        Listing::Written { path, .. } => Some(path),
        Listing::CouldNotWrite { path, reason } => {
            lines.push(format!("dl: could not write {} ({reason})", path.display()));
            None
        }
        Listing::NothingToList => None,
    }
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
        // One sentence for both parse arms: the reason already says whether the
        // parser or the typed read refused, and the arms exist for callers.
        config::ConfigError::NotToml { path, reason }
        | config::ConfigError::WrongType { path, reason } => {
            format!("{} is not usable: {reason}", path.display())
        }
    }
}

/// Why a metadata write or open failed, in one line.
///
/// A reason phrase and not a sentence: every caller has its own opening — `Could not
/// migrate the workspace cache: …`, `Could not drop the record for X: …` — and this
/// is what goes after the colon. Python interpolated the `OSError` it caught there,
/// which is divergence row 4: the OS's own words are Rust's spelling of them, and
/// the step that failed is named where Python's traceback would have shown it.
pub(crate) fn metadata_error(error: &metadata::MetadataError) -> String {
    match error {
        metadata::MetadataError::CreateDir { path, failure } => format!(
            "could not create the directory for dl's records at {} ({})",
            path.display(),
            failure.message
        ),
        metadata::MetadataError::Lock(error) => lock_refusal(error, "the lock on dl's records"),
        metadata::MetadataError::CreateTemp { directory, failure } => format!(
            "could not create a temporary file in {} ({})",
            directory.display(),
            failure.message
        ),
        // No path: the document did not get as far as having one.
        metadata::MetadataError::Encode { reason } => {
            format!("dl's records could not be encoded ({reason})")
        }
        metadata::MetadataError::Write { path, failure } => {
            format!("could not write {} ({})", path.display(), failure.message)
        }
        metadata::MetadataError::SetMode {
            path,
            mode,
            failure,
        } => format!(
            "could not set the permissions {mode:o} on {} ({})",
            path.display(),
            failure.message
        ),
        metadata::MetadataError::Replace { from, to, failure } => format!(
            "could not move {} into place at {} ({})",
            from.display(),
            to.display(),
            failure.message
        ),
    }
}

/// Why a lock could not be taken, naming the lock it is about.
///
/// The three steps stay apart because they are fixed in three different places: a
/// parent directory that cannot be made, a lock file that cannot be opened, and an
/// `flock` that failed for a reason other than somebody else holding it.
pub(crate) fn lock_refusal(error: &LockError, lock: &str) -> String {
    match error {
        LockError::CreateParent { path, failure } => format!(
            "could not create the directory for {lock} at {} ({})",
            path.display(),
            failure.message
        ),
        LockError::Open { path, failure } => format!(
            "could not open {lock} {} ({})",
            path.display(),
            failure.message
        ),
        LockError::Acquire { path, failure } => format!(
            "could not take {lock} {} ({})",
            path.display(),
            failure.message
        ),
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
    notices.iter().filter_map(lifecycle_notice).collect()
}

/// One notice's line, or `None` for a notice Python printed nothing for.
fn lifecycle_notice(notice: &LifecycleNotice) -> Option<String> {
    Some(match notice {
        LifecycleNotice::CloneRemoved { workspace_id } => {
            format!("Removed local clone for {workspace_id}")
        }
        LifecycleNotice::CloneNotRemoved { refusal, .. } => {
            format!("Failed to remove local clone: {}", not_removed(refusal))
        }
        LifecycleNotice::VolumesNotRemoved {
            workspace_id,
            refusal,
        } => volumes_not_removed(workspace_id, refusal),
        LifecycleNotice::RecordNotDropped { path, refusal } => {
            format!(
                "Could not drop the record for {}: {}",
                path.display(),
                metadata_error(refusal)
            )
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
        LifecycleNotice::Cache(cache) => return cache_notice(cache),
    })
}

/// One storage-flow notice, in the words the module that logged it used.
///
/// `worktree/repo_manager.py` and `worktree/workspace_clone.py` write these
/// through module loggers, which `dl.py`'s `basicConfig(format="%(message)s")`
/// sends to stderr as the bare message — so there is no logger name, no level and
/// no prefix in any of them.
///
/// The progress half of the vocabulary — the `logger.info` lines that say what the
/// flow is *about to* do — is rendered here with the rest of it, and is said as it
/// happens (core's channel is a sink, not a list). That is not decoration: the first
/// launch of a large repository sits for minutes inside `Cloning repository …`, and a
/// line printed after the wait it explains is a line that explained nothing.
///
/// `None` is left for the notices Python logged at `debug`, of which there are none
/// here: every arm below is a line a default-configured `dl.py` printed.
fn cache_notice(notice: &CacheNotice) -> Option<String> {
    Some(match notice {
        // --- progress (info; repo_manager.py 247/277/321/342/390,
        //     workspace_clone.py 436/527/792/955/958)
        CacheNotice::CloningRepository { remote_url, bare } => {
            format!("Cloning repository {remote_url} to {}", bare.display())
        }
        CacheNotice::ClonedRepository { owner, repo } => {
            format!("Successfully cloned {owner}/{repo}")
        }
        CacheNotice::FetchingUpdates { owner, repo } => {
            format!("Fetching updates for {owner}/{repo}")
        }
        CacheNotice::FetchedUpdates { owner, repo } => {
            format!("Successfully fetched updates for {owner}/{repo}")
        }
        CacheNotice::FetchingRef {
            owner,
            repo,
            branch,
        } => format!("Fetching {branch} for {owner}/{repo}"),
        CacheNotice::CreatingWorkspaceClone { path } => {
            format!("Creating workspace clone at {}", path.display())
        }

        // --- the branch decision (info; branch_manager.py 49/56/62/67)
        CacheNotice::BranchAlreadyBothSides { branch } => {
            format!("Branch {branch} already exists locally and remotely")
        }
        CacheNotice::BranchCutFromRemote { branch, remote } => {
            format!("Created local branch {branch} tracking {remote}/{branch}")
        }
        CacheNotice::BranchCreated { branch } => format!("Created local branch {branch}"),
        CacheNotice::BranchPushed { branch, remote } => {
            format!("Pushed branch {branch} to {remote}")
        }
        // Python's two lines name neither the ref nor the cache, and neither do the
        // arms: there is nothing to interpolate.
        CacheNotice::FillingLfsCache => "Fetching git-lfs objects into the cache".to_owned(),
        CacheNotice::PullingLfsFromOrigin => "Fetching git-lfs objects from origin".to_owned(),
        CacheNotice::WorkspaceCloneRemoved { path } => {
            format!("Removed workspace clone: {}", path.display())
        }
        CacheNotice::NoWorkspaceCloneToRemove { path } => {
            format!("No workspace clone to remove at {}", path.display())
        }

        // --- adopted, degraded or refused (warning/error)
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
        } => format!(
            "Could not fetch {branch} for {owner}/{repo}: {}",
            not_refreshed(reason)
        ),
        // Python's own sentence for this site (`workspace_clone.py`'s "Cannot
        // fetch recorded default branch: {e}"): the name came out of
        // `metadata.json` with no proof, and the line says so.
        CacheNotice::RecordedDefaultBranchUnsafe { refused } => {
            format!(
                "Cannot fetch recorded default branch: {}",
                unsafe_name(refused)
            )
        }
        CacheNotice::DefaultBranchUnknown { reason, .. } => {
            format!(
                "Failed to resolve default branch: {}",
                not_refreshed(reason)
            )
        }
        CacheNotice::PreparedFromStaleBase {
            owner,
            repo,
            branch,
            base,
            reason,
        } => format!(
            "Prepared '{owner}/{repo}@{branch}' from the cache's '{base}', which could not be \
             refreshed ({}); it may be behind the remote.",
            not_refreshed(reason)
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
        CacheNotice::WorkspaceNotRecorded { refusal } => {
            format!(
                "Failed to save workspace metadata: {}",
                metadata_error(refusal)
            )
        }
        CacheNotice::WorkspaceRecordNotRemoved { refusal } => {
            format!(
                "Failed to remove workspace metadata: {}",
                metadata_error(refusal)
            )
        }
        CacheNotice::CloneNotNamed {
            owner,
            repo,
            branch,
            refused,
        } => format!(
            "cannot name the clone directory for {owner}/{repo}@{branch}: {}",
            unsafe_name(refused)
        ),
        CacheNotice::Metadata(notice) => metadata_notice(notice),
    })
}

/// Why nothing refreshed a ref, as the clause inside the fetch warnings and the
/// stale-base report.
///
/// Every arm renders the words Python put there: git's own for a failed fetch,
/// and `str(ValueError)` — [`unsafe_name`]'s sentence — for a name git was never
/// asked about (`workspace_clone.py`'s `_fetch_base_branch` interpolates exactly
/// that into its `StaleBase.reason`).
fn not_refreshed(reason: &NotRefreshed) -> String {
    match reason {
        NotRefreshed::FetchFailed { reason } => reason.clone(),
        NotRefreshed::NoBranchOnRemote { branch } => {
            format!("the remote has no branch '{branch}' to refresh from")
        }
        NotRefreshed::UnsafeName(refused) => unsafe_name(refused),
        NotRefreshed::NoDefaultBranchRecorded => "no default branch is recorded".to_owned(),
    }
}

/// Why a workspace clone directory is still on disk.
///
/// The reason phrase Python interpolated its `Exception` into (`dl.py`:4122). The
/// symlinked-root arm names what the link points at, because `rm -rf` on the link
/// removes the link and nothing else — the reader needs the real location to act on,
/// which is the same reasoning `refusal_reason` gives for the purge's copy of it.
fn not_removed(refusal: &RemoveWorkspaceError) -> String {
    match refusal {
        RemoveWorkspaceError::UnsafeTriple(name) => unsafe_name(name),
        RemoveWorkspaceError::DirectoryLeft(error) => tree_not_removed(error),
    }
}

/// A refused volume removal, in the one sentence it gets.
///
/// One function rather than the same `format!` in both printers: `rm` reports it as
/// a notice and `--purge` as a step, the vocabularies differ but the sentence does
/// not, and two copies each pinned by its own test is how they come to differ by a
/// word.
fn volumes_not_removed(workspace_id: &str, refusal: &VolumeRefusal) -> String {
    format!(
        "Failed to remove the Docker volumes for {workspace_id}: {}",
        volumes_refused(refusal)
    )
}

/// Why a deleted workspace's Docker volumes are still on this machine.
///
/// docker's own stderr where docker spoke, trimmed of the newline it ends on
/// because this is interpolated mid-sentence. A machine with no docker never
/// reaches here: it is a silent arm of the sweep, not a refusal.
fn volumes_refused(refusal: &VolumeRefusal) -> String {
    match refusal {
        VolumeRefusal::Docker { exit, stderr } => match stderr.trim() {
            "" => format!("docker exited {}", exit_status(*exit)),
            said => said.to_owned(),
        },
        VolumeRefusal::NotRun { failure } => {
            format!("could not run docker ({})", os_error_phrase(failure))
        }
    }
}

/// Why a directory tree is still there.
fn tree_not_removed(error: &RemoveTreeError) -> String {
    match error {
        RemoveTreeError::RootIsSymlink { path, points_at } => {
            let to = match points_at {
                Some(target) => format!(" to {}", target.display()),
                None => String::new(),
            };
            format!(
                "{} is a symbolic link{to}, which dl will not follow",
                path.display()
            )
        }
        RemoveTreeError::CouldNotLook { path, reason } => {
            format!("could not look at {} ({reason})", path.display())
        }
        RemoveTreeError::Refused { path, reason } => {
            format!("could not remove {} ({reason})", path.display())
        }
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
            cause,
        } => format!(
            "{workspace_id}: {}. devlaunch will not delete a clone it cannot check. Look at it, \
             or run: dl {spec} rm --force",
            cause.describe()
        ),
    }
}

/// `--rm`, at the moment the session has ended and the removal begins.
///
/// Named rather than silent, and it names the *target as typed* rather than the
/// resolved workspace id for two reasons: the id has not been resolved yet when this
/// is said, and the word the user recognises is the one they wrote. Whatever the
/// removal then decides — the clone lines, or [`removal_refusal`]'s sentence and a
/// workspace still standing — reads as an answer to this.
pub(crate) fn rm_on_exit_removing(spec: &str) -> String {
    format!("--rm: the session has ended, removing {spec}.")
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
/// **Images, and no longer volumes** (devlaunch#325). The named volumes a
/// workspace's devcontainer created now go with the workspace, so a sentence that
/// still disclaimed them would be describing a leak that was fixed. Images stay
/// out deliberately rather than for want of a fix: they are shared between
/// workspaces, expensive to rebuild, and which workspace owns one is genuinely
/// ambiguous — which is exactly why it is still worth saying.
///
/// Still a sentence and not a measurement: printing it runs no `docker`, so it
/// costs nothing on a machine where Docker is absent and nothing on a command
/// whose work is already done.
pub(crate) const DOCKER_BOUNDARY: &str = concat!(
    "devlaunch does not manage Docker images: the images these workspaces built may still hold ",
    "disk, and `docker system df` shows what Docker is holding."
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
    let ownership = plan.ownership();
    let mut lines = vec![
        "This will remove all devlaunch data:".to_owned(),
        format!("  - {} DevPod workspace(s)", ownership.mine.len()),
        format!(
            "  - {}/ (workspace clones, repo caches, the shared pixi cache, completions)",
            plan.cache_dir().display()
        ),
    ];
    if !ownership.foreign.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "Leaving {} workspace(s) devlaunch did not create:",
            ownership.foreign.len()
        ));
        lines.extend(
            ownership
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
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Line {
    Out(String),
    Err(String),
}

/// The line a purge says before a round trip that may take a while.
///
/// Handed over as it happens rather than collected, which is why this renders one
/// step rather than a report: "Deleting workspace X" said afterwards is not said in
/// time. A failed delete is said here too — the step is its one report, so nothing
/// upstream can print it a second time.
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
        PurgeStep::VolumesNotRemoved {
            workspace_id,
            refusal,
        } => Line::Err(volumes_not_removed(workspace_id, refusal)),
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
    // turns a pasted `sudo rm -rf` into two targets, the first of them wrong. The
    // crate's one quoter does it, the same one `aid` builds its `dl` command line
    // with — a private copy here would be a third spelling of `shlex.quote` to keep
    // true.
    let by_hand: Vec<String> = remove_by_hand
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    lines.push(format!(
        "  sudo rm -rf {}",
        shell::join(by_hand.iter().map(String::as_str))
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
        format!("Clone directories under {}:", plan.root().display()),
        String::new(),
    ];
    if !plan.removing().is_empty() {
        lines.push(format!(
            "Removing {} that nothing references -- {}:",
            plan.removing().len(),
            describe_usage(&plan.clones_freed())
        ));
        for reclaimable in plan.removing() {
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
    if !plan.keeping().is_empty() {
        lines.push(format!("Leaving {}:", plan.keeping().len()));
        for kept in plan.keeping() {
            lines.push(format!(
                "  - {}: {}",
                kept.path.display(),
                kept_because(&kept.because)
            ));
        }
        lines.push(String::new());
    }
    if !plan.stale_records().is_empty() {
        lines.push(format!(
            "Dropping {} record(s) of directories already gone.",
            plan.stale_records().len()
        ));
        lines.push(String::new());
    }
    lines.extend(worktree_plan_lines(plan.worktrees()));
    if plan.nothing_to_do() {
        lines.push("Nothing to prune.".to_owned());
    }
    lines
}

/// The agent git worktrees inside the clones this run is keeping, and what each
/// of them is (devlaunch#426).
///
/// Its own section under the clone plan rather than rows mixed into it, because
/// these are a different kind of thing: every one of them is inside a clone the
/// run has just said it is *not* touching, and the rules that reach them are
/// their own. Nothing at all is printed when there is nothing to say, which is
/// every host that has never run an agent in a workspace.
fn worktree_plan_lines(sweep: &WorktreeSweep) -> Vec<String> {
    if sweep.nothing_to_say() {
        return Vec::new();
    }
    let mut lines = vec![
        format!(
            "Agent git worktrees inside the clones above -- {}:",
            describe_usage(&sweep.freed())
        ),
        String::new(),
    ];
    for found in sweep.clones() {
        lines.push(format!("  {}:", found.clone_path().display()));
        for worktree in found.removing() {
            let mut line = format!(
                "    - removing {} ({}): {}",
                worktree.path.display(),
                describe_usage(&worktree.usage),
                seen_as(worktree.seen_as)
            );
            // What `--force-worktrees` is answering, on the line of the directory
            // it answers for. Without it the plan reads the same for a worktree
            // holding an afternoon's work as for a finished one.
            if let WorktreePromotion::Insisted { despite } = &worktree.promotion {
                line = format!("{line}, and {}; removing anyway", objected(despite));
            }
            lines.push(line);
        }
        for kept in found.keeping() {
            lines.push(format!(
                "    - leaving {}: {}",
                kept.path.display(),
                worktree_kept_because(&kept.because)
            ));
        }
        // Two sentences rather than one number, because the two are different
        // facts: a container path is what every worktree an agent made inside a
        // devcontainer carries, and a path in this clone with nothing at it is
        // somebody's own removal or a run interrupted halfway. Neither frees
        // anything.
        let nothing_here = found.registrations_with_nothing_here();
        if nothing_here.container_paths() > 0 {
            lines.push(format!(
                "    - {} registration(s) here name a path inside a container, which never \
                 resolved on this host, so nothing is freed by forgetting them",
                nothing_here.container_paths()
            ));
        }
        if nothing_here.deleted() > 0 {
            lines.push(format!(
                "    - {} registration(s) here name a directory in this clone that is not there \
                 any more, so nothing is freed by forgetting them",
                nothing_here.deleted()
            ));
        }
        if !found.metadata_gate().open() {
            lines.push(
                "    - git worktree prune is held back here: it is all-or-nothing across a \
                 clone, and it would drop the registration that is keeping a worktree above"
                    .to_owned(),
            );
        }
    }
    lines.push(String::new());
    // Said once, rather than implied by every line above it. `--prune` is a local
    // command and deliberately does not fetch, so "nothing else reaches these
    // commits" is a statement about the last fetch and not about the forge now.
    lines.push(
        "Whether a worktree's commits are anywhere else is as of the last fetch into the \
         repository cache; --prune does not fetch."
            .to_owned(),
    );
    lines.push(String::new());
    lines
}

/// How git saw a directory that is going.
fn seen_as(seen: SeenAs) -> &'static str {
    match seen {
        SeenAs::Forgotten => "git has already forgotten it",
        SeenAs::Prunable => "git says the registration for it can go",
        SeenAs::Locked => "git is holding it locked",
    }
}

/// Why one worktree directory is staying, as the report says it.
///
/// Every arm names the fact it rests on and none of them claims the worktree is
/// idle, because nothing on a host can establish that: a lock is the agent
/// harness's courtesy, and a killed session leaves one behind.
fn worktree_kept_because(because: &WorktreeKept) -> String {
    match because {
        WorktreeKept::StillHeld { head } => format!(
            "git still holds it and does not offer it up, on {}",
            head.named()
        ),
        WorktreeKept::Objected(objections) => format!(
            "{} -- add --force-worktrees to remove it anyway",
            objected(objections)
        ),
    }
}

/// Everything arguing against removing one worktree, joined as one clause.
fn objected(objections: &NonEmpty<WorktreeObjection>) -> String {
    objections
        .iter()
        .map(worktree_objection)
        .collect::<Vec<_>>()
        .join(" and ")
}

fn worktree_objection(objected: &WorktreeObjection) -> String {
    match objected {
        WorktreeObjection::Locked { lock } => match &lock.reason {
            None => "git is holding it locked".to_owned(),
            Some(reason) => format!("git is holding it locked ({reason})"),
        },
        WorktreeObjection::Holds(holds) => format!("holds {}", objection(holds)),
    }
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
        Objection::CouldNotTell(cause) => {
            format!("work git could not be asked about ({})", cause.describe())
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
        describe_usage(&report.clones_freed())
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
    lines.extend(worktree_report_lines(&report.worktrees));
    lines
}

/// What the run did about the agent worktrees.
///
/// The withheld lines say *that this was not so when the plan was printed*, which
/// is the whole of what a second classification has to tell somebody who has
/// already read the first one — and here it is not a rare race: a container is
/// not a participant in devlaunch's repository lock, so it can register a
/// worktree while the plan is on screen.
fn worktree_report_lines(report: &WorktreeReport) -> Vec<String> {
    if report.nothing_to_say() {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "Removed {} agent worktree(s) -- {}.",
        report.removed.len(),
        describe_usage(&report.freed())
    )];
    for withheld in &report.withheld {
        lines.push(format!(
            "Left {}: {}. That was not so when the plan above was printed.",
            withheld.path.display(),
            worktree_kept_because(&withheld.because)
        ));
    }
    for clone in &report.metadata_held_back {
        lines.push(format!(
            "Did not run git worktree prune in {}: a worktree there is being kept, and the \
             registration is what goes on protecting it.",
            clone.display()
        ));
    }
    for clone in &report.metadata_refused {
        lines.push(format!(
            "git worktree prune would not run in {}, so git still lists worktrees that are \
             gone. The next --prune will offer them again.",
            clone.display()
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
            "Some agent worktrees would not come away. These refused:",
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
            plan.root().display()
        ),
        String::new(),
    ];
    if !plan.adopting().is_empty() {
        lines.push(format!("Re-pointing {}:", plan.adopting().len()));
        for adoptable in plan.adopting() {
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
    if !plan.reporting().is_empty() {
        lines.push(format!(
            "Leaving {}, which dl will not guess at:",
            plan.reporting().len()
        ));
        for unadoptable in plan.reporting() {
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
        // One sentence for both, as Python's one `except (OSError,
        // json.JSONDecodeError)` wrote it; the arms differ so a caller can.
        RepointFailure::Unreadable { path, reason } | RepointFailure::NotJson { path, reason } => {
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

// ---------------------------------------------------------------------------
// the launch
// ---------------------------------------------------------------------------

/// One launch notice's line, or `None` for one Python prints nothing for.
///
/// `dl.py` configures `logging.basicConfig(level=logging.INFO,
/// format="%(message)s")`, so an `info` and a `warning` are the same bytes on
/// stderr — no level, no logger name, no prefix — and a `debug` is *nothing at
/// all*. Which of the two a notice is happens to be the whole of its rendering,
/// and it is the binary's to decide (#251 §5): core carries the typed event and no
/// level. The level each arm stands for is named in its comment.
pub(crate) fn launch_notice(notice: &LaunchNotice) -> Option<String> {
    Some(match notice {
        // --- the shared pixi cache (warning; dl.py `_pixi_cache_up_args`, 3533/3554)
        LaunchNotice::PixiCacheNotCreated { source, reason } => format!(
            "Could not create the shared pixi cache at {} ({reason}), so each container downloads \
             its own packages.",
            source.display()
        ),
        LaunchNotice::PixiCacheNotADirectory { source } => format!(
            "The shared pixi cache at {} is not there after all, so this container downloads its \
             own packages.",
            source.display()
        ),

        // --- the launch lock (locks.py:89's bare `print`, and dl.py 3727/3744)
        LaunchNotice::WaitingForSiblingLaunch { workspace_id } => {
            format!("dl: waiting for another launch of {workspace_id}")
        }
        // debug: a lock that could not be taken costs this `up` its serialization
        // and nothing a user acts on.
        LaunchNotice::LaunchLockUnavailable { .. } => return None,
        // info
        LaunchNotice::BroughtUpBySibling { workspace_id } => {
            format!("Workspace {workspace_id} was brought up by another dl run.")
        }

        // --- the token (warning; gh_auth.py 84/94/105/139)
        LaunchNotice::NoGitHubToken(event) => no_github_token(event),
        LaunchNotice::TokenNotStaged { reason } => format!(
            "Could not create a file to pass the GitHub token to devpod ({reason}), so this \
             workspace opens without a GitHub login."
        ),

        // --- the session (warning at 3845, info at 3864/3891, debug at 3875)
        LaunchNotice::NoTerminalAlias { workspace_id } => format!(
            "No devpod ssh host entry for {workspace_id}, so this command gets no terminal; \
             interactive programs may exit immediately. `dl {workspace_id} restart` republishes it."
        ),
        LaunchNotice::SshCommand { argv } => format!("SSH command: {}", argv.join(" ")),
        // debug: devpod's own diagnostics are already on the user's stderr, and the
        // status is the exit code this command ends with.
        LaunchNotice::DevpodSessionFailed { .. } => return None,

        // --- the launch's own arms (info, bar the last; dl.py 4869/4762/4844/4871)
        LaunchNotice::AlreadyRunningAttaching { workspace_id } => {
            format!("Workspace {workspace_id} is already running, attaching...")
        }
        LaunchNotice::CreateNeverFinished { workspace_id } => format!(
            "Workspace {workspace_id} was never finished setting up — devpod recorded no result \
             for its create, so attaching to it would land as root in a container whose setup \
             did not run. Bringing it up instead."
        ),
        LaunchNotice::AlreadyRunning { workspace_id } => {
            format!("Workspace {workspace_id} is already running.")
        }
        LaunchNotice::StartingForDotfiles { workspace_id } => {
            format!("Starting workspace {workspace_id}...")
        }
        LaunchNotice::DevcontainerIgnoredRunning { workspace_id, spec } => format!(
            "Ignoring --devcontainer: {workspace_id} is already running. Use 'dl {spec} recreate \
             --devcontainer ...' to switch config."
        ),

        // --- the terminal title (no level at all: not a sentence)
        //
        // The one notice with no line. What it carries is an escape sequence, so
        // rendering it as one would put its bytes into `--prune`'s collected
        // report and into every test that asserts on a launch's sentences.
        // `Saying` writes it instead, which is the only sink that should.
        LaunchNotice::TerminalTitle(_) => return None,

        // --- passed through from the layers below, in those modules' own words
        LaunchNotice::Cache(cache) => return cache_notice(cache),
        LaunchNotice::Lifecycle(notice) => return lifecycle_notice(notice),
    })
}

/// The lines a launch's notices read as, in the order they happened, with the
/// debug ones dropped as a default-configured Python drops them.
pub(crate) fn launch_notices(notices: &[LaunchNotice]) -> Vec<String> {
    notices.iter().filter_map(launch_notice).collect()
}

/// The launch's notice sink: one line on stderr, at the moment the notice happens.
///
/// This is the other half of core's streaming channel, and the reason it is a sink
/// rather than a list the binary drains at the end: `Cloning repository …` exists to
/// explain a wait, and `Workspace X is already running, attaching...` comes before
/// the shell it announces. Said with `eprintln!` for the reason every diagnostic is:
/// stdout belongs to the completion machinery and to `wf`.
pub(crate) struct Saying;

impl Notices<LaunchNotice> for Saying {
    fn say(&mut self, notice: LaunchNotice) {
        // The terminal title before the line check, because it is the one notice
        // `launch_notice` has no line for and this is the sink that owes it its
        // bytes. Written raw: no newline, since an OSC sequence is not a line,
        // and flushed, because the next thing to touch this terminal is a session
        // that may hold it for hours -- an unflushed title is a title that
        // arrives when the work is over.
        if let LaunchNotice::TerminalTitle(title) = &notice {
            if let Some(osc) = title.osc() {
                use std::io::Write as _;
                let mut stderr = io::stderr();
                let _ = stderr.write_all(osc.as_bytes());
                let _ = stderr.flush();
            }
            return;
        }
        if let Some(line) = launch_notice(&notice) {
            eprintln!("{line}");
        }
    }
}

impl Notices<ProvisionEvent> for Saying {
    fn say(&mut self, event: ProvisionEvent) {
        if let Some(line) = provision_event(&event) {
            eprintln!("{line}");
        }
    }
}

/// Why this workspace opens without a GitHub login.
///
/// The `Refused` arm names the directory gh read its config from, because that is
/// the one arm whose commonest cause is not a missing login at all: a run that
/// scoped `XDG_CONFIG_HOME` to a scratch directory hides the host's gh login, gh
/// refuses, and `gh auth login` is exactly the wrong remedy.
fn no_github_token(event: &GhEvent) -> String {
    match event {
        GhEvent::CouldNotRun(unavailable) => format!(
            "Could not read a GitHub token from gh ({}), so this workspace opens without a GitHub \
             login.",
            gh_unavailable(unavailable)
        ),
        GhEvent::Refused { exit } => format!(
            "gh auth token exited {}, so this workspace opens without a GitHub login. gh read its \
             config from {} -- if you are logged in on this host, that directory is the thing to \
             check before `gh auth login`.",
            exit_status(*exit),
            gh_config_home().display()
        ),
        // Never the junk itself: what gh printed may be a malformed credential,
        // and a warning is not a place to put one.
        GhEvent::NotAToken => "gh auth token printed something that is not a token, so this \
             workspace opens without a GitHub login."
            .to_owned(),
    }
}

fn gh_unavailable(unavailable: &GhUnavailable) -> String {
    match unavailable {
        GhUnavailable::TimedOut => "it did not answer in time".to_owned(),
        GhUnavailable::Blocked(failure) => format!("{:?}", failure.kind),
    }
}

/// The directory gh reads its config from, for the sentence that names it.
///
/// A machine with no home directory cannot get this far — every command that
/// forwards a token has already resolved dl's cache directory out of the same
/// home — so the fallback is the spec's own spelling rather than an answer.
fn gh_config_home() -> std::path::PathBuf {
    xdg::config_home().unwrap_or_else(|_| std::path::PathBuf::from("~/.config"))
}

/// The one-line message for an ssh that is not installed.
///
/// Names the way out that needs no ssh at all: the devpod transport still runs
/// commands, it just cannot give them a terminal.
pub(crate) const SSH_MISSING: &str = concat!(
    "ssh not found on PATH: dl needs OpenSSH to give a workspace command a terminal. ",
    "Install it, or set DEVLAUNCH_NO_TTY=1 to run commands through devpod instead ",
    "(interactive programs will not work)."
);

/// Why a launch could not even be attempted.
///
/// The class Python's `main()` handles rather than `_run_cli`: a missing binary
/// travels as a type nothing in between catches and is printed bare (no `error: `
/// prefix, because `main` prints the exception itself), and an unreadable listing
/// gets the `error: ` line the read side already writes.
pub(crate) fn launch_abort(aborted: &LaunchAborted) -> String {
    match aborted {
        LaunchAborted::DevpodNotRun(NotRun::NotInstalled) => DEVPOD_MISSING.to_owned(),
        LaunchAborted::DevpodNotRun(NotRun::TimedOut) => {
            "error: devpod did not answer in time".to_owned()
        }
        LaunchAborted::DevpodNotRun(NotRun::Blocked(failure)) => {
            format!(
                "error: devpod could not be run ({})",
                os_error_phrase(failure)
            )
        }
        LaunchAborted::SshNotRun(SshNotRun::NotInstalled) => SSH_MISSING.to_owned(),
        LaunchAborted::SshNotRun(SshNotRun::TimedOut) => {
            "error: ssh did not answer in time".to_owned()
        }
        LaunchAborted::SshNotRun(SshNotRun::Blocked(failure)) => {
            format!("error: ssh could not be run ({})", os_error_phrase(failure))
        }
        LaunchAborted::ListingUnreadable(refused) => listing_refusal(refused),
    }
}

/// Whether this abort is one of the two missing binaries, which is what exits 127.
pub(crate) fn is_binary_missing(aborted: &LaunchAborted) -> bool {
    match aborted {
        LaunchAborted::DevpodNotRun(refused) => matches!(refused, NotRun::NotInstalled),
        LaunchAborted::SshNotRun(refused) => matches!(refused, SshNotRun::NotInstalled),
        LaunchAborted::ListingUnreadable(refused) => is_devpod_missing(refused),
    }
}

/// Why a launch will not go ahead, in the words `_run_cli` refused it with.
///
/// `None` is a refusal with nothing to say: `devpod up` and `devpod stop` write
/// their own diagnostics to this process's stderr — the calls inherit the streams —
/// so dl has nothing to add but the exit code.
pub(crate) fn launch_refusal(refused: &LaunchRefusal) -> Option<String> {
    match refused {
        LaunchRefusal::UnsafeSpec(name) => Some(unsafe_name(name)),
        LaunchRefusal::UnknownWorkspace { name } => Some(unknown_workspace(name)),
        LaunchRefusal::BranchNotNamed { owner, repo, error } => Some(format!(
            "Repository '{owner}/{repo}': {}",
            branch_not_named(error)
        )),
        LaunchRefusal::NotPrepared {
            owner,
            repo,
            branch,
            error,
        } => Some(format!(
            "Failed to prepare workspace '{owner}/{repo}@{branch}': {}",
            not_prepared(error)
        )),
        LaunchRefusal::UpRefused { .. } | LaunchRefusal::StopRefused { .. } => None,
        LaunchRefusal::NoSession(refused) => Some(session_refusal(refused)),
    }
}

/// The second line a wrong-owner spec deserves, or nothing to add.
///
/// The case it answers is a mistyped or half-remembered *owner* —
/// `kinisi/kinisi_ros` where the repository is `kinisi-robotics/kinisi_ros`. git's
/// own words are accurate and useless for it: "Repository not found" plus six
/// lines of ssh advice describe a machine that cannot see a repository, when what
/// happened is that the reader named the wrong one. The right name is already on
/// disk, in the same list the shell completes from, so this looks it up there and
/// says it.
///
/// Only a not-found refusal gets the line, and only from a clone step. A clone
/// that failed on the network, on credentials or on the disk names a repository
/// that may well exist under the owner given, and a "did you mean" would send the
/// reader after a problem they do not have.
///
/// **Divergence row 29**: Python printed git's stderr and stopped. Additive —
/// nothing above this line changes, and a machine whose cache holds no candidate
/// sees exactly what it saw before.
///
/// Pure like everything else here: the cache is read by the caller and passed in.
pub(crate) fn wrong_owner_hint(refused: &LaunchRefusal, known: &CompletionData) -> Option<String> {
    let (owner, repo, branch, clone) = match refused {
        LaunchRefusal::BranchNotNamed {
            owner,
            repo,
            error: BranchNotNamed::Repository(EnsureRepoError::Clone(clone)),
        } => (owner, repo, None, clone),
        LaunchRefusal::NotPrepared {
            owner,
            repo,
            branch,
            error: NotPrepared::Preparation(PrepareColdError::Clone(clone)),
        } => (owner, repo, Some(branch.as_str()), clone),
        _ => return None,
    };
    let CloneError::GitRefused { refused: git, .. } = clone else {
        return None;
    };
    if !reads_as_repository_not_found(git.reason()) {
        return None;
    }
    let typed = format!("{owner}/{repo}");
    // The cache holding the spec that was typed is the strongest evidence available
    // that the owner is *not* misremembered: this machine has launched it. Whatever
    // the host is refusing today — access revoked, the repository made private, a
    // clone pruned out from under a record that survived it — the name is one the
    // reader has used before, and pointing them at a different owner would be wrong.
    if known
        .repos
        .iter()
        .any(|known| known.eq_ignore_ascii_case(&typed))
    {
        return None;
    }
    let candidates = same_repo_other_owners(known, owner, repo);
    if candidates.is_empty() {
        return None;
    }
    // Each suggestion is a whole spec, branch included: a spec typed with a
    // `@branch` is offered back with the same one, so it can be retyped as it reads.
    // A spec rather than a command line, because `aid` reaches this same rendering
    // through `dl::run` and its own invocation carries an agent prompt after the
    // spec — naming the spec is right for both binaries, where naming `dl …` would
    // be right for one and misleading for the other.
    let suffix = branch.map_or_else(String::new, |branch| format!("@{branch}"));
    let specs: Vec<String> = candidates
        .iter()
        .take(MOST_CANDIDATES_LISTED)
        .map(|spec| python_repr(&format!("{spec}{suffix}")))
        .collect();
    // What is left out is counted, not dropped: a repository name common across a
    // dozen cached owners would otherwise make one unreadable line out of the one
    // line whose whole job is to be read.
    let owners = match candidates.len() - specs.len() {
        0 if specs.len() == 1 => "another owner.".to_owned(),
        0 => "other owners.".to_owned(),
        _ => format!(
            "{} other owners — 'dl --repos' lists them all.",
            candidates.len()
        ),
    };
    Some(format!(
        "Did you mean {}? git could not find {}, and dl knows that repository name under {owners}",
        or_list(&specs),
        python_repr(&typed)
    ))
}

/// How many candidates the hint spells out before it starts counting them instead.
const MOST_CANDIDATES_LISTED: usize = 3;

/// Every repository the cache knows by the name *repo* under an owner that is not
/// *owner*, as full `owner/repo` specs.
///
/// Reads the cache's `repos` list rather than asking the network: the answer is
/// wanted on a path that has already failed, and a machine that has launched the
/// repository once has it. An owner never launched from here cannot be suggested,
/// which is the honest limit of an offline guess.
///
/// Matched case-insensitively, both halves, because GitHub and GitLab are: a clone
/// of `kinisi/Kinisi_ROS` is refused by the same host that would have served
/// `kinisi-robotics/kinisi_ros`, so a reader who shifted the capitals as well as
/// the owner should still be told. Each candidate keeps the cache's spelling rather
/// than the typed one — that is the name the host actually has.
///
/// Here rather than in `devlaunch-core` beside the cache it reads, because
/// `CompletionData`'s fields are already the crate's public surface and that
/// surface is frozen by CI: a `pub fn` on it is an API change, and this is a
/// sentence-building detail of one diagnostic rather than a flow anything else
/// needs.
fn same_repo_other_owners(known: &CompletionData, owner: &str, repo: &str) -> Vec<String> {
    known
        .repos
        .iter()
        .filter(|known| {
            known
                .split_once('/')
                .is_some_and(|(known_owner, known_repo)| {
                    known_repo.eq_ignore_ascii_case(repo)
                        && !known_owner.eq_ignore_ascii_case(owner)
                })
        })
        .cloned()
        .collect()
}

/// Whether git's stderr is the host saying the repository is not there.
///
/// Matched on the text because that is the only place the distinction exists: git
/// exits 128 for this, for a refused key and for a DNS failure alike, so the exit
/// status cannot tell them apart. The three phrases are the three hosts' own
/// wordings — GitHub's `Repository not found` (ssh and https both), GitLab's `The
/// project you were looking for could not be found`, and Bitbucket's `conq:
/// repository does not exist`.
///
/// Each is matched whole, and each shorter form was tried and rejected. `not
/// exist` alone also catches git's *local* complaint, `repository '/some/path'
/// does not exist`, which is a missing directory rather than a host's answer.
/// `could not be found` alone is generic English rather than anything GitLab
/// specifically said. And `and the repository exists` rides along with every ssh
/// failure git reports, refused keys included, one word from the wording above.
///
/// # Why no `LC_ALL=C`, when the other substring-classified verbs pin it
///
/// `Git::fetch_ref` and `ensure_branch` force `LC_ALL=C`/`LANGUAGE=C` precisely
/// because their callers match on `couldn't find remote ref` and `already exists`,
/// which git *translates*. `clone_bare` inherits the environment instead, and may:
/// all three phrases here are the **remote's** bytes, relayed over the wire by the
/// host's own git-upload-pack and never passed through git's gettext catalogue, so
/// a French locale does not move them. What a non-English locale can lose is the
/// hint from git's own translatable `repository '%s' not found` wording — a
/// candidate not offered, never a wrong one offered.
///
/// A host that words it some fourth way loses the hint and keeps git's own
/// message, which is the safe direction for this to be wrong in.
fn reads_as_repository_not_found(reason: &str) -> bool {
    let reason = reason.to_lowercase();
    [
        "repository not found",
        "project you were looking for could not be found",
        "repository does not exist",
    ]
    .iter()
    .any(|phrase| reason.contains(phrase))
}

/// `a`, `a or b`, `a, b or c` — the list joined the way a sentence wants it.
fn or_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

fn branch_not_named(error: &BranchNotNamed) -> String {
    match error {
        BranchNotNamed::Cold(refused) => refused.reason.clone(),
        BranchNotNamed::Repository(refused) => ensure_repo_failure(refused),
    }
}

fn not_prepared(error: &NotPrepared) -> String {
    match error {
        NotPrepared::Cold(refused) => refused.reason.clone(),
        NotPrepared::Preparation(refused) => prepare_cold_failure(refused),
    }
}

/// Why the bare-clone cache could not be brought up.
///
/// The words are `worktree/repo_manager.py`'s own exceptions, which is what Python
/// interpolated into `Repository '{owner}/{repo}': {e}`. The step is named because
/// the steps are fixed in different places: a lock, a directory, a `git clone`.
fn ensure_repo_failure(refused: &EnsureRepoError) -> String {
    match refused {
        EnsureRepoError::Lock(error) => lock_refusal(error, "the repository lock"),
        EnsureRepoError::WrongRepoLock(wrong) => wrong_repo_lock(wrong),
        EnsureRepoError::Clone(error) => clone_failure(error),
    }
}

/// Why the bare clone itself could not be made — `Failed to clone repository: {git}`
/// and its neighbours.
fn clone_failure(refused: &CloneError) -> String {
    match refused {
        CloneError::ParentNotCreated { path, reason } => {
            format!("could not create {} ({reason})", path.display())
        }
        CloneError::PartialCloneNotCleared(error) => format!(
            "a partial clone from an earlier run is in the way: {}",
            tree_not_removed(error)
        ),
        // Python's `RuntimeError(f"Failed to clone repository: {reason}")`, where the
        // reason is git's own stderr. The debris is named only when it is still
        // there: for every failure reachable in practice git removes the destination
        // itself, and a line about a directory that is gone is noise.
        CloneError::GitRefused { refused, cleanup } => {
            let left = match cleanup {
                Cleanup::Cleared => String::new(),
                Cleanup::Left(error) => {
                    format!(" (and {} was left behind)", tree_not_removed(error))
                }
            };
            format!("Failed to clone repository: {}{left}", refused.reason())
        }
        CloneError::NotRecorded(error) => format!(
            "the clone is on disk and its record could not be written: {}",
            metadata_error(error)
        ),
    }
}

/// Why the host-side preparation of a cold launch stopped, step by step.
fn prepare_cold_failure(refused: &PrepareColdError) -> String {
    match refused {
        PrepareColdError::UnsafeTriple(name) => unsafe_name(name),
        PrepareColdError::Lock(error) => lock_refusal(error, "the repository lock"),
        PrepareColdError::WrongRepoLock(wrong) => wrong_repo_lock(wrong),
        PrepareColdError::Clone(error) => clone_failure(error),
        PrepareColdError::Branch(EnsureBranchError::WrongRepoLock(wrong)) => wrong_repo_lock(wrong),
        PrepareColdError::Branch(EnsureBranchError::Branch(error)) => branch_failure(error),
        PrepareColdError::Workspace(error) => workspace_failure(error),
    }
}

/// Why a branch could not be ensured — `branch_manager.py`'s two exceptions.
fn branch_failure(refused: &BranchError) -> String {
    match refused {
        BranchError::NotCreated { reason, .. } => format!("Failed to create branch: {reason}"),
        BranchError::NotPushed { reason, .. } => {
            format!("Failed to push branch to remote: {reason}")
        }
    }
}

/// Why the workspace clone could not be cut — `workspace_clone.py`'s exceptions, in
/// its own words.
fn workspace_failure(refused: &PrepareWorkspaceError) -> String {
    match refused {
        PrepareWorkspaceError::WrongRepoLock(wrong) => wrong_repo_lock(wrong),
        PrepareWorkspaceError::ParentNotCreated { path, reason } => {
            format!("could not create {} ({reason})", path.display())
        }
        PrepareWorkspaceError::CloneRefused { reason, cleanup } => {
            let left = match cleanup {
                Cleanup::Cleared => String::new(),
                Cleanup::Left(error) => {
                    format!(" (and {} was left behind)", tree_not_removed(error))
                }
            };
            format!("Failed to clone workspace: {reason}{left}")
        }
        PrepareWorkspaceError::RemoteNotRepointed { reason } => {
            format!("Failed to set remote URL: {reason}")
        }
        PrepareWorkspaceError::NoStartPoint {
            branch,
            default_branch,
        } => format!(
            "Cannot create branch '{branch}': neither 'origin/{branch}' nor \
             'origin/{default_branch}' exist on the remote"
        ),
        PrepareWorkspaceError::UnsafeRefName(name) => unsafe_name(name),
        PrepareWorkspaceError::CheckoutRefused { branch, reason } => {
            format!("Failed to checkout branch '{branch}': {reason}")
        }
        PrepareWorkspaceError::LfsNotMaterialized { reason } => format!(
            "Failed to pull git-lfs objects ({reason}). The workspace still holds pointer \
             files; re-run to retry."
        ),
    }
}

/// A repo lock offered as evidence about a repository it says nothing about.
///
/// Unreachable in practice — every scope mints the token it then passes — and worded
/// rather than debug-printed because "unreachable" is not "unrenderable": Python
/// raised a `ValueError` here with this sentence, and its `except` on the launch path
/// printed it.
fn wrong_repo_lock(wrong: &WrongRepoLock) -> String {
    let (held_owner, held_repo) = &wrong.held;
    let (wanted_owner, wanted_repo) = &wrong.wanted;
    format!(
        "repo lock held for {held_owner}/{held_repo} cannot vouch for \
         {wanted_owner}/{wanted_repo}"
    )
}

/// Why no session could be composed.
///
/// The two never-ran arms are lifted to [`LaunchAborted`] before they get here, so
/// what is left is the pair Python has no refusal for: **divergence row 19**, a
/// command holding a NUL, and the ssh invocation `clients::ssh` will not compose.
fn session_refusal(refused: &SessionRefused) -> String {
    match refused {
        SessionRefused::Unquotable(command) => format!(
            "error: cannot run {} in a workspace: a command holding a NUL byte cannot be a shell \
             word.",
            python_repr(&command.command)
        ),
        SessionRefused::UnsafeRequest(UnsafeRequest::OptionLikeWorkspaceId { workspace_id }) => {
            format!(
                "error: refusing to ssh to {}: a workspace name starting with '-' would reach ssh \
                 as an option.",
                python_repr(workspace_id)
            )
        }
        SessionRefused::UnsafeRequest(UnsafeRequest::UnquotableWorkdir { workdir }) => format!(
            "error: cannot enter {}: a directory holding a NUL byte cannot be a shell word.",
            python_repr(workdir)
        ),
        // Lifted to `LaunchAborted` by the launch itself; answered anyway so this
        // stays a total function of the value it is given.
        SessionRefused::Devpod(refused) => devpod_not_run("ssh", refused),
        SessionRefused::Ssh(SshNotRun::NotInstalled) => SSH_MISSING.to_owned(),
        SessionRefused::Ssh(SshNotRun::TimedOut) => "error: ssh did not answer in time".to_owned(),
        SessionRefused::Ssh(SshNotRun::Blocked(failure)) => {
            format!("error: ssh could not be run ({})", os_error_phrase(failure))
        }
    }
}

/// Why a command could not get as far as running, without the `error: ` prefix.
///
/// Quoted inside core's own refusals as well as printed on its own, which is why
/// the prefix is the caller's: `Repository 'owner/repo': <this>` must not carry one.
pub(crate) fn startup_reason(refused: &StartupError) -> String {
    match refused {
        StartupError::NoHomeDirectory => {
            "this machine names no home directory, so dl cannot find its cache".to_owned()
        }
        StartupError::Config(error) => config_error(error),
        StartupError::Metadata(error) => metadata_error(error),
    }
}

// ---------------------------------------------------------------------------
// provisioning
// ---------------------------------------------------------------------------

/// One provisioning event's line, or `None` for one Python prints nothing for.
///
/// Four of the six are `logging.debug` — provisioning is a convenience whose
/// failures cost the workspace its tools and not its session, and Python says
/// nothing about most of them at the default level. The two that speak are the
/// setup stages, each at the level the stage itself declares
/// ([`FailureLevel`]) — and both of those levels print, because
/// `basicConfig(level=INFO)` prints an info as well as a warning.
pub(crate) fn provision_event(event: &ProvisionEvent) -> Option<String> {
    Some(match event {
        // tools.py:926/934. `loudness` is info for the hostname stage and warning
        // for the rest — `sudo hostname` cannot succeed without CAP_SYS_ADMIN,
        // which Docker drops by default, so failure is the majority case there and
        // a warning on most cold launches would erode what a warning means. Both
        // reach stderr as the bare message, so the field is read and not rendered.
        ProvisionEvent::StageFailed {
            workspace,
            stage,
            status,
            loudness,
        } => {
            let _: &FailureLevel = loudness;
            format!("{workspace}: the {stage} setup stage exited {status}.")
        }
        ProvisionEvent::StageNotReported {
            workspace,
            stage,
            loudness,
        } => {
            let _: &FailureLevel = loudness;
            format!("{workspace}: the {stage} setup stage did not report; it may not have run.")
        }
        // debug (tools.py:1063), and the `%s` is the variable's own name.
        ProvisionEvent::ProvisioningDisabled { .. } => return None,
        // debug (tools.py:998): the network install follows, which is the path
        // Python took for any bundle failure.
        ProvisionEvent::PayloadNotBundled { failure } => {
            let _: &BundleFailed = failure;
            return None;
        }
        // debug (tools.py:1100): Python's `except OSError` answer.
        ProvisionEvent::TripRefused { .. } => return None,
        // warning (tools.py:1106) — the one arm a user is meant to see. The exit
        // status is not in the sentence: Python's line does not carry it.
        ProvisionEvent::NotInstalled {
            workspace,
            tools,
            exit,
        } => {
            let _: &Exit = exit;
            format!(
                "Could not install {} into {workspace}; the session will start without them.",
                tools.join(" and ")
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use devlaunch_core::flows::launch::TerminalTitle;
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

    // ---------------------------------------------- the refusal advice line

    /// The one line a person is meant to paste, with paths a shell would
    /// mis-split if they went in bare.
    ///
    /// The expectations are CPython's `shlex.quote` output for the same paths —
    /// including the embedded single quote, which is exactly where the `shlex`
    /// crate answers different bytes and where a hand-rolled quoter drifts.
    #[test]
    fn the_advice_line_quotes_a_path_a_shell_would_mis_split() {
        let lines = report_refusals(
            std::iter::empty::<&Refusal>(),
            "Removed nothing. These refused:",
            &[
                PathBuf::from("/home/u/my cache/devlaunch"),
                PathBuf::from("/home/u/it's/devlaunch"),
                PathBuf::from("/home/u/.cache/devlaunch"),
            ],
        );

        assert!(
            lines.contains(
                &r#"  sudo rm -rf '/home/u/my cache/devlaunch' '/home/u/it'"'"'s/devlaunch' /home/u/.cache/devlaunch"#
                    .to_owned()
            ),
            "the pasteable line, got {lines:#?}"
        );
    }

    /// The rest of the classes a pasted line has to survive: the shell
    /// metacharacters, a newline, and a path that stringifies to nothing.
    ///
    /// Single quoting is what makes `$` and a backtick inert rather than a
    /// substitution the paste would run as root, and an empty word has to survive
    /// as a word — `sudo rm -rf` with one silently dropped is a command that
    /// removes the wrong thing. Expectations are CPython's `shlex.join` for the
    /// same words.
    #[test]
    fn the_advice_line_makes_the_shell_metacharacters_inert() {
        let lines = report_refusals(
            std::iter::empty::<&Refusal>(),
            "Removed nothing. These refused:",
            &[
                PathBuf::from("/home/u/a\nb/devlaunch"),
                PathBuf::from("/home/u/$HOME/devlaunch"),
                PathBuf::from("/home/u/`id`/devlaunch"),
                PathBuf::from(""),
                PathBuf::from("/home/u/na\u{ef}ve/devlaunch"),
            ],
        );

        assert!(
            lines.contains(
                &concat!(
                    "  sudo rm -rf '/home/u/a\nb/devlaunch' '/home/u/$HOME/devlaunch' ",
                    "'/home/u/`id`/devlaunch' '' '/home/u/na\u{ef}ve/devlaunch'",
                )
                .to_owned()
            ),
            "the pasteable line, got {lines:#?}"
        );
    }

    // ------------------------------------------------- the wrong-owner hint
    //
    // The whole refusal cannot be built here — `GitRefused` has no public
    // constructor, by design — so the two halves are judged separately: the
    // candidate lookup and the not-found classification here, and the sentence
    // they produce end to end in `tests/launch.rs` against a real `git clone`.

    fn known(repos: &[&str]) -> CompletionData {
        CompletionData {
            repos: repos.iter().map(|repo| (*repo).to_owned()).collect(),
            ..CompletionData::default()
        }
    }

    #[test]
    fn the_same_repo_name_under_a_different_owner_is_what_a_wrong_owner_is_found_by() {
        // The wrong-owner case: `kinisi/kinisi_ros` is nobody's repository and
        // `kinisi-robotics/kinisi_ros` is in the list the shell completes from.
        let known = known(&[
            "blooop/bencher",
            "kinisi-robotics/kinisi_ros",
            "other/kinisi_ros",
        ]);

        assert_eq!(
            same_repo_other_owners(&known, "kinisi", "kinisi_ros"),
            ["kinisi-robotics/kinisi_ros", "other/kinisi_ros"]
        );
    }

    #[test]
    fn the_owner_asked_about_is_not_one_of_its_own_candidates() {
        // A repository that *is* in the cache under the owner given failed to clone
        // for some other reason, and suggesting the spec just typed would be noise.
        let known = known(&["a/b", "c/d"]);

        assert!(same_repo_other_owners(&known, "a", "b").is_empty());
        assert!(same_repo_other_owners(&known, "a", "unknown").is_empty());
    }

    #[test]
    fn the_capitals_a_host_ignores_are_ignored_here_too() {
        // GitHub and GitLab match both halves case-insensitively, so the host that
        // refused `kinisi/Kinisi_ROS` is the one that would have served
        // `kinisi-robotics/kinisi_ros`. The candidate keeps the cache's spelling,
        // which is the name the host actually has.
        let known = known(&["kinisi-robotics/kinisi_ros"]);

        assert_eq!(
            same_repo_other_owners(&known, "kinisi", "Kinisi_ROS"),
            ["kinisi-robotics/kinisi_ros"]
        );
        // And the same-owner exclusion is case-insensitive in the other direction:
        // `KINISI-ROBOTICS` is not another owner.
        assert!(
            same_repo_other_owners(&known, "KINISI-ROBOTICS", "kinisi_ros").is_empty(),
            "the owner given was offered back to itself under different capitals"
        );
    }

    #[test]
    fn the_hosts_not_found_wordings_are_told_from_its_other_refusals() {
        // The three hosts' own wordings.
        assert!(reads_as_repository_not_found(
            "ERROR: Repository not found."
        ));
        assert!(reads_as_repository_not_found(
            "remote: Repository not found.\nfatal: repository 'https://x/y.git' not found"
        ));
        assert!(reads_as_repository_not_found(
            "GitLab: The project you were looking for could not be found or you don't have \
             permission to view it."
        ));
        // The whole phrase, not the tail of it: `could not be found` on its own is
        // generic English rather than anything a host said.
        assert!(!reads_as_repository_not_found(
            "error: object file .git/objects/ab/cdef could not be found"
        ));
        assert!(reads_as_repository_not_found(
            "conq: repository does not exist."
        ));

        // And the near misses. The last line of git's stock ssh advice — "and the
        // repository exists" — rides along with *every* ssh failure, refused keys
        // included, and is one word from the wording above; git's own complaint
        // about a missing local directory is not a host's answer at all.
        assert!(!reads_as_repository_not_found(
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote \
             repository.\n\nPlease make sure you have the correct access rights\nand the \
             repository exists."
        ));
        assert!(!reads_as_repository_not_found(
            "ssh: Could not resolve hostname github.com: Temporary failure in name resolution"
        ));
        assert!(!reads_as_repository_not_found(
            "fatal: repository '/home/someone/not-there' does not exist"
        ));
    }

    #[test]
    fn a_list_of_candidates_reads_as_a_sentence() {
        let one = ["'a/r'".to_owned()];
        let two = ["'a/r'".to_owned(), "'b/r'".to_owned()];
        let three = ["'a/r'".to_owned(), "'b/r'".to_owned(), "'c/r'".to_owned()];

        assert_eq!(or_list(&one), "'a/r'");
        assert_eq!(or_list(&two), "'a/r' or 'b/r'");
        assert_eq!(or_list(&three), "'a/r', 'b/r' or 'c/r'");
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
                    SizeCell::Measured(CloneDisk::measured(2048, None)),
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
    fn a_size_cell_names_the_part_of_it_that_is_agent_worktrees() {
        // On the host devlaunch#426 was found on, the worktrees were 82% of the
        // whole cache and no row said so, which is how it reached 100%.
        let lines = table_lines(
            &table(vec![
                row(
                    "a",
                    SourceKind::Local,
                    "/x",
                    SizeCell::Measured(CloneDisk::measured(4096, Some(3072))),
                    LastUsed::Never,
                ),
                row(
                    "b",
                    SourceKind::Local,
                    "/y",
                    SizeCell::Measured(CloneDisk::measured(2048, Some(0))),
                    LastUsed::Never,
                ),
            ]),
            Sizes::Measure,
        );

        assert!(
            lines[2].contains("4.0 KiB (3.0 KiB in worktrees)"),
            "{:?}",
            lines[2]
        );
        // A clone with an empty `.claude/worktrees/` has a measurement and nothing
        // to say, and a "0 B in worktrees" in every row would be noise.
        assert!(lines[3].contains("2.0 KiB  never"), "{:?}", lines[3]);
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

    // ================================================================ the launch

    #[test]
    fn the_two_missing_binaries_are_the_two_lines_that_exit_127() {
        // The ssh half cannot be reached from a headless test: the OpenSSH
        // transport is chosen only when dl is *on* a terminal, so the line is pinned
        // here and the route it travels is M9's pty work (see the note in
        // `tests/launch.rs`). Both are printed bare, as Python's `print(e)` prints
        // the exception, and both are 127.
        let devpod = LaunchAborted::DevpodNotRun(NotRun::NotInstalled);
        let ssh = LaunchAborted::SshNotRun(SshNotRun::NotInstalled);
        assert_eq!(launch_abort(&devpod), DEVPOD_MISSING);
        assert_eq!(
            launch_abort(&ssh),
            "ssh not found on PATH: dl needs OpenSSH to give a workspace command a terminal. \
             Install it, or set DEVLAUNCH_NO_TTY=1 to run commands through devpod instead \
             (interactive programs will not work)."
        );
        assert!(is_binary_missing(&devpod) && is_binary_missing(&ssh));
        // Everything else about a launch that could not start is exit 1, which is
        // what `UNREADABLE_WORKSPACE_LIST_EXIT_CODE` says and what a devpod that is
        // there but unreachable deserves.
        let unreadable =
            LaunchAborted::ListingUnreadable(ListingUnreadable::Unreadable(NotAListing::Silence));
        assert!(!is_binary_missing(&unreadable));
        assert!(!is_binary_missing(&LaunchAborted::DevpodNotRun(
            NotRun::TimedOut
        )));
    }

    #[test]
    fn a_devpod_that_wrote_its_own_diagnostics_gets_no_sentence_from_dl() {
        // `devpod up` and `devpod stop` inherit this process's streams, so their
        // refusal is already on the user's stderr and dl has only the exit code to
        // add. `None` is what makes "say nothing" a value rather than a forgotten
        // branch at the call site.
        assert_eq!(
            launch_refusal(&LaunchRefusal::UpRefused {
                exit: Exit::Code(7)
            }),
            None
        );
        assert_eq!(
            launch_refusal(&LaunchRefusal::StopRefused {
                exit: Exit::Code(9)
            }),
            None
        );
    }

    #[test]
    fn the_terminal_title_is_the_one_notice_with_no_line() {
        // Not a debug line -- a notice that is not a sentence at all. Rendering it
        // as one would put an escape sequence into `launch_notices`, which is what
        // fills a collected report and what the tests that assert on a launch's
        // words read. `Saying` is the only sink that may write these bytes.
        assert_eq!(
            launch_notice(&LaunchNotice::TerminalTitle(TerminalTitle::Write(
                "\x1b]2;myws\x07".to_owned()
            ))),
            None
        );
        assert_eq!(
            launch_notice(&LaunchNotice::TerminalTitle(TerminalTitle::Off)),
            None
        );
        assert!(
            launch_notices(&[
                LaunchNotice::TerminalTitle(TerminalTitle::Write("\x1b]2;myws\x07".to_owned())),
                LaunchNotice::WaitingForSiblingLaunch {
                    workspace_id: "ws".to_owned(),
                },
            ])
            .iter()
            .all(|line| !line.contains('\x1b')),
            "no escape reaches a collected report"
        );
    }

    #[test]
    fn the_launch_lock_and_the_failed_session_are_debug_lines_nobody_sees() {
        // `logging.debug` under `basicConfig(level=INFO)`: not a rendering choice,
        // the absence of one.
        assert_eq!(
            launch_notice(&LaunchNotice::LaunchLockUnavailable {
                workspace_id: "ws".to_owned(),
                reason: "Permission denied (os error 13)".to_owned(),
            }),
            None
        );
        assert_eq!(
            launch_notice(&LaunchNotice::DevpodSessionFailed {
                exit: Exit::Code(1)
            }),
            None
        );
        // And the one a sibling launch prints, which is a bare `print` to stderr
        // rather than a log line at all (locks.py:89).
        assert_eq!(
            launch_notice(&LaunchNotice::WaitingForSiblingLaunch {
                workspace_id: "ws".to_owned(),
            }),
            Some("dl: waiting for another launch of ws".to_owned())
        );
    }

    #[test]
    fn the_gh_refusal_names_the_config_directory_gh_read() {
        // The arm whose commonest cause is not a missing login at all: a run that
        // scoped `XDG_CONFIG_HOME` to a scratch directory hides the host's gh login,
        // so gh refuses even though the user is logged in — and `gh auth login` is
        // exactly the wrong remedy for that.
        let line = launch_notice(&LaunchNotice::NoGitHubToken(GhEvent::Refused {
            exit: Exit::Code(1),
        }))
        .expect("a warning");
        assert!(
            line.starts_with(
                "gh auth token exited 1, so this workspace opens without a GitHub login. gh read \
                 its config from "
            ) && line.ends_with(
                " -- if you are logged in on this host, that directory is the thing to check \
                 before `gh auth login`."
            ),
            "{line}"
        );
        // Never the junk gh printed: it may be a malformed credential, and a warning
        // is not a place to put one.
        assert_eq!(
            launch_notice(&LaunchNotice::NoGitHubToken(GhEvent::NotAToken)),
            Some(
                "gh auth token printed something that is not a token, so this workspace opens \
                 without a GitHub login."
                    .to_owned()
            )
        );
    }

    #[test]
    fn the_other_two_reasons_a_workspace_opens_without_a_login() {
        // gh_auth.py:84 and :139. Both are warnings, because the degradation is
        // invisible from inside the container: a `gh` with no token there looks like
        // a `gh` nobody logged in with.
        assert_eq!(
            launch_notice(&LaunchNotice::NoGitHubToken(GhEvent::CouldNotRun(
                GhUnavailable::TimedOut
            ))),
            Some(
                "Could not read a GitHub token from gh (it did not answer in time), so this \
                 workspace opens without a GitHub login."
                    .to_owned()
            )
        );
        assert_eq!(
            launch_notice(&LaunchNotice::TokenNotStaged {
                reason: "No space left on device (os error 28)".to_owned(),
            }),
            Some(
                "Could not create a file to pass the GitHub token to devpod (No space left on \
                 device (os error 28)), so this workspace opens without a GitHub login."
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_shared_pixi_cache_that_could_not_be_made_is_a_warning_not_a_failure() {
        // The launch survives either way, which is exactly why these are warnings:
        // the degradation is invisible and permanent until the cache home is
        // writable again, and it costs every container a 1.2GB download the last one
        // already paid for (devlaunch#232).
        assert_eq!(
            launch_notice(&LaunchNotice::PixiCacheNotCreated {
                source: std::path::PathBuf::from("/c/pixi"),
                reason: "Permission denied (os error 13)".to_owned(),
            }),
            Some(
                "Could not create the shared pixi cache at /c/pixi (Permission denied (os error \
                 13)), so each container downloads its own packages."
                    .to_owned()
            )
        );
        assert_eq!(
            launch_notice(&LaunchNotice::PixiCacheNotADirectory {
                source: std::path::PathBuf::from("/c/pixi"),
            }),
            Some(
                "The shared pixi cache at /c/pixi is not there after all, so this container \
                 downloads its own packages."
                    .to_owned()
            )
        );
    }

    #[test]
    fn the_provisioning_events_a_user_sees_are_the_stages_and_the_failed_install() {
        // Everything else `tools.py` says about provisioning is a `logging.debug`.
        assert_eq!(
            provision_event(&ProvisionEvent::StageFailed {
                workspace: "ws".to_owned(),
                stage: "hostname",
                status: 1,
                loudness: FailureLevel::default(),
            }),
            Some("ws: the hostname setup stage exited 1.".to_owned())
        );
        assert_eq!(
            provision_event(&ProvisionEvent::NotInstalled {
                workspace: "ws".to_owned(),
                tools: vec!["gh", "claude"],
                exit: Exit::Code(1),
            }),
            Some(
                "Could not install gh and claude into ws; the session will start without them."
                    .to_owned()
            )
        );
        assert_eq!(
            provision_event(&ProvisionEvent::ProvisioningDisabled {
                workspace: "ws".to_owned(),
            }),
            None
        );
    }

    // -----------------------------------------------------------------------
    // the storage flows' own lines
    // -----------------------------------------------------------------------

    #[test]
    fn the_progress_lines_are_the_ones_the_storage_flows_logged() {
        // `worktree/repo_manager.py` 247/277/321/342/390 and
        // `worktree/workspace_clone.py` 436/527/792/955/958, byte for byte: these are
        // the lines that explain a wait, so a launch that sits for minutes cloning a
        // large repository says which repository and where the disk is going.
        let said = |notice: CacheNotice| cache_notice(&notice).expect("a line");

        assert_eq!(
            said(CacheNotice::CloningRepository {
                remote_url: "git@github.com:blooop/devlaunch.git".to_owned(),
                bare: PathBuf::from("/c/repos/blooop/devlaunch/.bare"),
            }),
            "Cloning repository git@github.com:blooop/devlaunch.git to \
             /c/repos/blooop/devlaunch/.bare"
        );
        assert_eq!(
            said(CacheNotice::ClonedRepository {
                owner: "blooop".to_owned(),
                repo: "devlaunch".to_owned(),
            }),
            "Successfully cloned blooop/devlaunch"
        );
        assert_eq!(
            said(CacheNotice::FetchingUpdates {
                owner: "blooop".to_owned(),
                repo: "devlaunch".to_owned(),
            }),
            "Fetching updates for blooop/devlaunch"
        );
        assert_eq!(
            said(CacheNotice::FetchedUpdates {
                owner: "blooop".to_owned(),
                repo: "devlaunch".to_owned(),
            }),
            "Successfully fetched updates for blooop/devlaunch"
        );
        assert_eq!(
            said(CacheNotice::FetchingRef {
                owner: "blooop".to_owned(),
                repo: "devlaunch".to_owned(),
                branch: "fix/42".to_owned(),
            }),
            "Fetching fix/42 for blooop/devlaunch"
        );
        assert_eq!(
            said(CacheNotice::CreatingWorkspaceClone {
                path: PathBuf::from("/c/repos/blooop/devlaunch/devlaunch-main-abc"),
            }),
            "Creating workspace clone at /c/repos/blooop/devlaunch/devlaunch-main-abc"
        );
        // The two git-lfs lines name neither the ref nor the cache, as Python's do
        // not: there is nothing in the arms to interpolate.
        assert_eq!(
            said(CacheNotice::FillingLfsCache),
            "Fetching git-lfs objects into the cache"
        );
        assert_eq!(
            said(CacheNotice::PullingLfsFromOrigin),
            "Fetching git-lfs objects from origin"
        );
        assert_eq!(
            said(CacheNotice::WorkspaceCloneRemoved {
                path: PathBuf::from("/c/repos/o/r/ws"),
            }),
            "Removed workspace clone: /c/repos/o/r/ws"
        );
        assert_eq!(
            said(CacheNotice::NoWorkspaceCloneToRemove {
                path: PathBuf::from("/c/repos/o/r/ws"),
            }),
            "No workspace clone to remove at /c/repos/o/r/ws"
        );
    }

    #[test]
    fn the_branch_decision_reads_as_the_four_lines_python_logged() {
        // `worktree/branch_manager.py` 49/56/62/67. Which of the four happened is an
        // answer (`BranchEnsured`) rather than a log line in the decision itself, and
        // these are the words the answer is said in.
        let said = |notice: CacheNotice| cache_notice(&notice).expect("a line");

        assert_eq!(
            said(CacheNotice::BranchAlreadyBothSides {
                branch: "main".to_owned(),
            }),
            "Branch main already exists locally and remotely"
        );
        assert_eq!(
            said(CacheNotice::BranchCutFromRemote {
                branch: "fix/42".to_owned(),
                remote: "origin".to_owned(),
            }),
            "Created local branch fix/42 tracking origin/fix/42"
        );
        assert_eq!(
            said(CacheNotice::BranchCreated {
                branch: "fix/42".to_owned(),
            }),
            "Created local branch fix/42"
        );
        assert_eq!(
            said(CacheNotice::BranchPushed {
                branch: "fix/42".to_owned(),
                remote: "origin".to_owned(),
            }),
            "Pushed branch fix/42 to origin"
        );
    }

    #[test]
    fn a_clone_that_would_not_go_names_what_stopped_it() {
        // `dl.py`:4122's `Failed to remove local clone: {e}`, with the exception
        // replaced by the typed refusal's own words. The symlink arm names what the
        // link points at, because `rm -rf` on the link removes the link and nothing
        // else.
        assert_eq!(
            lifecycle_notice(&LifecycleNotice::CloneNotRemoved {
                workspace_id: "ws".to_owned(),
                refusal: RemoveWorkspaceError::DirectoryLeft(RemoveTreeError::RootIsSymlink {
                    path: PathBuf::from("/c/repos/o/r/ws"),
                    points_at: Some(PathBuf::from("/mnt/disk/ws")),
                }),
            }),
            Some(
                "Failed to remove local clone: /c/repos/o/r/ws is a symbolic link to /mnt/disk/ws, \
                 which dl will not follow"
                    .to_owned()
            )
        );
        assert_eq!(
            lifecycle_notice(&LifecycleNotice::CloneNotRemoved {
                workspace_id: "ws".to_owned(),
                refusal: RemoveWorkspaceError::DirectoryLeft(RemoveTreeError::Refused {
                    path: PathBuf::from("/c/repos/o/r/ws/.git"),
                    reason: "Permission denied (os error 13)".to_owned(),
                }),
            }),
            Some(
                "Failed to remove local clone: could not remove /c/repos/o/r/ws/.git (Permission \
                 denied (os error 13))"
                    .to_owned()
            )
        );
    }

    /// devlaunch#325's notice. docker's own words where docker spoke, because a
    /// volume some other container still holds is the case that matters and only
    /// docker knows which container that is. A machine with no docker never reaches
    /// here — that is a silent arm of the sweep, not a refusal — so there is no
    /// sentence for it to have.
    #[test]
    fn volumes_that_would_not_go_carry_dockers_own_words() {
        assert_eq!(
            lifecycle_notice(&LifecycleNotice::VolumesNotRemoved {
                workspace_id: "ws".to_owned(),
                refusal: VolumeRefusal::Docker {
                    exit: Exit::Code(1),
                    stderr: "Error response from daemon: remove ws-pixi: volume is in use\n"
                        .to_owned(),
                },
            }),
            Some(
                "Failed to remove the Docker volumes for ws: Error response from daemon: remove \
                 ws-pixi: volume is in use"
                    .to_owned()
            )
        );
        // A docker that failed silently still gets a line: the status is all there
        // is to report, and reporting nothing would be reporting a removal.
        assert_eq!(
            lifecycle_notice(&LifecycleNotice::VolumesNotRemoved {
                workspace_id: "ws".to_owned(),
                refusal: VolumeRefusal::Docker {
                    exit: Exit::Signal(9),
                    stderr: String::new(),
                },
            }),
            Some("Failed to remove the Docker volumes for ws: docker exited -9".to_owned())
        );
        assert_eq!(
            lifecycle_notice(&LifecycleNotice::VolumesNotRemoved {
                workspace_id: "ws".to_owned(),
                refusal: VolumeRefusal::NotRun {
                    failure: OsFailure {
                        kind: std::io::ErrorKind::PermissionDenied,
                        errno: Some(13),
                    },
                },
            }),
            Some(
                "Failed to remove the Docker volumes for ws: could not run docker (Permission \
                 denied (os error 13))"
                    .to_owned()
            )
        );
    }

    /// The purge says the same thing in its own channel: it reports as it goes, so
    /// a refusal is a step rather than a notice — and it goes to stderr, as every
    /// other thing that did not work does.
    #[test]
    fn a_purge_says_which_workspaces_volumes_stayed() {
        assert_eq!(
            purge_step(&PurgeStep::VolumesNotRemoved {
                workspace_id: "ws".to_owned(),
                refusal: VolumeRefusal::Docker {
                    exit: Exit::Code(1),
                    stderr: "volume is in use\n".to_owned(),
                },
            }),
            Line::Err("Failed to remove the Docker volumes for ws: volume is in use".to_owned())
        );
    }

    #[test]
    fn a_record_that_would_not_go_names_the_step_that_refused() {
        // `dl.py`:2545's `Could not drop the record for {path}: {e}`. The notice
        // carries the write's own `MetadataError`, so the line names which step
        // failed — divergence row 4's phrasing, Python's sentence.
        assert_eq!(
            lifecycle_notice(&LifecycleNotice::RecordNotDropped {
                path: PathBuf::from("/c/repos/o/r/ws"),
                refusal: metadata::MetadataError::Lock(LockError::Acquire {
                    path: PathBuf::from("/c/metadata.json.lock"),
                    failure: metadata::OsFailure {
                        kind: std::io::ErrorKind::PermissionDenied,
                        message: "Permission denied (os error 13)".to_owned(),
                    },
                }),
            }),
            Some(
                "Could not drop the record for /c/repos/o/r/ws: could not take the lock on dl's \
                 records /c/metadata.json.lock (Permission denied (os error 13))"
                    .to_owned()
            )
        );
        assert_eq!(
            lifecycle_notice(&LifecycleNotice::RecordNotDropped {
                path: PathBuf::from("/c/repos/o/r/ws"),
                refusal: metadata::MetadataError::Encode {
                    reason: "a NaN cannot be JSON".to_owned(),
                },
            }),
            Some(
                "Could not drop the record for /c/repos/o/r/ws: dl's records could not be encoded \
                 (a NaN cannot be JSON)"
                    .to_owned()
            )
        );
    }
}
