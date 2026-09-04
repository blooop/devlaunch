//! Typed results in, bytes out. Every user-facing English word `dl` prints is
//! written in this file or in [`crate::commands`]; core holds none of it.
//!
//! Everything here is a pure function of a value core produced, which is what
//! lets the table's column arithmetic and the JSON document's spelling be tested
//! without a devpod, a cache or a process.

use std::fmt::Write as _;
use std::io;
use std::path::Path;
use std::process;

use devlaunch_core::clients::devpod::{ListingUnreadable, NotAListing, NotRun, Workspace};
use devlaunch_core::clients::devpod_home::RepointFailure;
use devlaunch_core::clients::gh::{GhEvent, GhUnavailable};
use devlaunch_core::clients::git::Failure as GitFailure;
use devlaunch_core::clients::ssh::{NotRun as SshNotRun, UnsafeRequest};
use devlaunch_core::domain::config;
use devlaunch_core::domain::locks::LockError;
use devlaunch_core::domain::metadata;
use devlaunch_core::domain::model::SweepTrouble;
use devlaunch_core::domain::workspace_id::{NamePart, UnsafeName};
use devlaunch_core::domain::workspace_state::NonEmpty;
use devlaunch_core::domain::xdg;
use devlaunch_core::flows::agent_worktrees::{
    Collectable, Standing as WorktreeStanding, WorktreePromotion, WorktreeReport, WorktreeSweep,
};
use devlaunch_core::flows::branch_manager::BranchError;
use devlaunch_core::flows::completion_cache::CompletionData;
use devlaunch_core::flows::disk_usage::describe_usage;
use devlaunch_core::flows::kill::{
    ContainerRefusal, Containers, Ending, Holding, HostCannot, Marker, NoSignal, Standing, Sweep,
    TableUnreadable,
};
use devlaunch_core::flows::launch::{
    BranchNotNamed, ClaudeProfileProblem, ColdRefused, LaunchAborted, LaunchNotice, LaunchRefusal,
    NotPrepared, SessionRefused,
};
use devlaunch_core::flows::lifecycle::{
    Insistence, KeptBecause, LifecycleNotice, NotAdopted, Promotion, PrunePlan, PruneReport,
    PurgeOutcome, PurgePlan, PurgeStep, ReconcilePlan, RemovalGrounds, RemovalRefused,
    SweepOccasion, Unlocatable, VolumeRefusal, VolumesKeptBecause,
};
use devlaunch_core::flows::listing::{
    self, CloneDisk, LastUsed, SizeCell, Sizes, SourceKind, SweptRepoNote, TableRow, WorkspaceTable,
};
use devlaunch_core::flows::migration::{Listing, MigrationReport};
use devlaunch_core::flows::provision::{BundleFailed, FailureLevel, ProvisionEvent};
use devlaunch_core::flows::records::{RecordsNotice, StartupError};
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

use crate::select::Chosen;

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

/// The lines `dl --ls` writes under its table for the repositories whose last
/// background sweep left something to say.
///
/// The sweep runs detached with all three descriptors on `/dev/null`, so until
/// now every complaint it had went nowhere (devlaunch#480). It writes them into
/// the record instead, and this is the reading: one line per repository, said
/// once, under the table a person was already looking at.
///
/// Per repository and not per row, because a note is about a bare clone rather
/// than about a workspace — several rows can share one, and a repository with no
/// workspace at all still has one to report.
pub(crate) fn sweep_notes(notes: &[SweptRepoNote]) -> Vec<String> {
    notes
        .iter()
        .map(|outstanding| {
            let mut line = format!(
                "Last cache sweep of {}: {}",
                outstanding.slug(),
                sweep_trouble(outstanding.note.trouble)
            );
            // Only where something spoke. A trailing `: ` over nothing would read
            // as a refusal whose words were lost, which is a different fact.
            if let Some(said) = &outstanding.note.said {
                line.push_str(": ");
                line.push_str(said);
            }
            line
        })
        .collect()
}

/// What one sweep trouble reads as. The words are the binary's (#251 §5); what
/// travels in the record is the arm.
fn sweep_trouble(trouble: SweepTrouble) -> &'static str {
    match trouble {
        SweepTrouble::RefsNotPacked => "could not pack the refs it fetched",
        SweepTrouble::FetchRefused => "could not fetch",
        SweepTrouble::FetchTimedOut => "ran out of time fetching",
        SweepTrouble::CloneMissing => "found no bare clone to fetch into",
        SweepTrouble::NotRecorded => "fetched, and could not write the record",
    }
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
/// workspace. Where there is, it is the number that would otherwise be
/// invisible: on the host devlaunch#426 was found on, the worktrees were 82% of
/// the cache and no `--ls --size` row said so.
///
/// A part of the figure beside it and never an addition -- the worktrees are
/// inside the clone.
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
/// rendering choice, and a byte of it is not this file's to pick.
///
/// One line, because the spelling is core's. It was a formatter here until
/// devlaunch#346 — a hundred lines forwarding every layout method to
/// `serde_json`'s pretty printer, plus an `ensure_ascii` loop, standing beside a
/// formatter in core doing the same for `metadata.json`. Two copies of one fact,
/// and what they cost was visible before they were merged: the escaping gate was
/// wrong about DEL in both, and closing it meant closing it twice.
///
/// The name stays here rather than the call moving to core's, because the
/// document's own pins hang off it: the tests below assert the same literals
/// against the same call they asserted against the deleted formatter, so what
/// they now measure is that the collapse changed no byte.
pub(crate) fn python_json_document(value: &Value) -> String {
    devlaunch_core::json::as_python_writes_it_indented(value)
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

/// devpod, mid-delete, waiting on a lock it is never told to give up on.
///
/// Printed while the command is still running, which is the only time it is worth
/// anything: this is the one failure with no downstream to report it, because
/// devpod's acquire returns when the holder dies and not before. Somebody watching
/// the five-second line repeat has two choices, wait or intervene, and until now
/// dl said nothing about either.
///
/// **It names another terminal**, because this one is busy holding the command the
/// advice is about, and Ctrl-C is the alternative it saves people from finding on
/// their own. `word` is the verb that was typed, so a `--rm` firing at the end of a
/// session offers the same `kill` the `rm` verb does rather than a word that is not
/// on the line.
pub(crate) fn delete_blocked(workspace_id: &str, word: &str) -> String {
    format!(
        "dl: devpod is waiting for another process to let go of {workspace_id}, and it will wait \
         for as long as that takes. In another terminal, 'dl {workspace_id} kill' clears whatever \
         is holding it and deletes it. (This {word} is still waiting.)"
    )
}

/// The delete that was still running when its deadline ran out.
///
/// Only `kill`'s delete carries one, so this only ever follows that verb, and what
/// it has to say is the state it left rather than the fact of the timeout, which
/// the line above it already gives. Both halves are load-bearing: the clone is
/// still on disk, so nothing has been lost; and the workspace may be *partly*
/// deleted, because a devpod killed a minute into the job is not a devpod that did
/// nothing. Running it again is the only way to find out which, and it is safe,
/// which is what the last sentence is for.
pub(crate) fn delete_timed_out(workspace_id: &str) -> String {
    format!(
        "{workspace_id} and its clone are still here, and devpod may have got part of the way \
         through. Run 'dl {workspace_id} kill' again to pick up where it stopped."
    )
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
            format!("could not read {} ({})", path.display(), source.message)
        }
        // One sentence for both parse arms: the reason already says whether the
        // parser or the typed read refused, and the arms exist for callers.
        config::ConfigError::NotToml { path, reason }
        | config::ConfigError::WrongType { path, reason } => {
            format!("{} is not usable: {reason}", path.display())
        }
    }
}

/// What a `config.toml` naming a key this build no longer reads is told, one line
/// each.
///
/// The whole of `repos_dir`'s migration (#467). Nothing on disk is touched, so
/// the notice is the only thing standing between a user who set that key and a
/// clone tree nothing will ever mention again: it names the directory, says dl
/// has stopped reading the key, and points at `XDG_CACHE_HOME`, which is the
/// supported way to move the cache and always was.
///
/// Not an error, because a stale config is not punished here, and not a
/// suggestion to delete anything either: what is at that path is the user's, and
/// dl has no business having an opinion about it.
pub(crate) fn retired_keys(keys: &[config::RetiredKey]) -> Vec<String> {
    keys.iter()
        .map(|key| match key {
            config::RetiredKey::ReposDir { named } => format!(
                "config.toml still sets worktree.repos_dir = '{named}'. dl no longer reads it: \
                 clones live under dl's cache directory, and XDG_CACHE_HOME is what moves that. \
                 Nothing in that tree was moved or removed, so it is yours to keep or delete."
            ),
        })
        .collect()
}

/// One thing the records' open had to say, as the lines it reads as.
///
/// A list because one arm is many lines: [`RecordsNotice::Migrated`] carries a whole
/// report, and Python's `_announce` printed up to nine separate warnings out of it.
/// Everything else is the one line its own renderer already produced — this is the
/// dispatch, not a new vocabulary.
pub(crate) fn records_notice(notice: &RecordsNotice) -> Vec<String> {
    match notice {
        RecordsNotice::RetiredKey(key) => retired_keys(std::slice::from_ref(key)),
        RecordsNotice::Metadata(notice) => metadata_notices(std::slice::from_ref(notice)),
        RecordsNotice::Migrated(report) => migration_notices(report),
        RecordsNotice::MigrationRefused(refused) => vec![format!(
            "Could not migrate the workspace cache: {}",
            metadata_error(refused)
        )],
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
            occasion,
            refusal,
        } => volumes_not_removed(workspace_id, *occasion, refusal),
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
        LifecycleNotice::Removing { workspace_id } => removing(workspace_id),
        LifecycleNotice::RemovingOverWork { refusal } => removing_over_work(refusal),
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
        CacheNotice::RefsNotPacked {
            owner,
            repo,
            reason,
        } => format!("Could not pack the refs of {owner}/{repo}: {reason}"),
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
fn volumes_not_removed(
    workspace_id: &str,
    occasion: SweepOccasion,
    refusal: &VolumeRefusal,
) -> String {
    format!(
        "Failed to remove the {} for {workspace_id}: {}",
        named_volumes(occasion),
        volumes_refused(refusal)
    )
}

/// Which read named the volumes a sweep was about, as the noun phrase in the
/// sentence about them.
///
/// The one place the two-arm occasion becomes English, and a match rather than a
/// field on the name: a third read would be a compile error here, which is the
/// refusal enforced by exhaustiveness rather than by prose. The wording differs
/// because where to look differs. devpod's record is gone with the workspace; a
/// kept copy is a file under devlaunch's cache, and a volume this could not release
/// is one something on the machine is still holding.
fn named_volumes(occasion: SweepOccasion) -> &'static str {
    match occasion {
        SweepOccasion::DevpodResult => "Docker volumes",
        SweepOccasion::KeptCopy => "Docker volumes devlaunch recorded",
    }
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
///
/// `word` is the verb, for the same reason and one more. `rm` and `rme` refuse
/// identically, and the way past is `--force` on whichever was typed — so a line
/// that always said `rm --force` would answer an `rme` by sending the reader back
/// to the two-step `rme` exists to collapse: delete, wait, then close the tab by
/// hand. Both words are offered the way past *they* asked for.
pub(crate) fn removal_refusal(refused: &RemovalRefused, spec: &str, word: &str) -> String {
    let workspace_id = &refused.workspace_id;
    match &refused.because {
        RemovalGrounds::WouldLose(holds) => format!(
            "{workspace_id} holds {holds}. Push or commit it, or run: dl {spec} {word} --force"
        ),
        RemovalGrounds::CouldNotTell(blank) => format!(
            "{workspace_id}: {blank}. devlaunch will not delete a clone it cannot check. Look \
             at it, or run: dl {spec} {word} --force"
        ),
        // Both at once -- a dirty tree beside a refused probe, or a nested
        // worktree's loss beside a lock. Saying one would be telling half the
        // truth, so both are said (devlaunch#446).
        RemovalGrounds::BothAtOnce {
            would_lose: holds,
            could_not_tell: blank,
        } => format!(
            "{workspace_id} holds {holds}, and {blank}. devlaunch will not delete a clone it \
             cannot check. Push or commit it, look at it, or run: dl {spec} {word} --force"
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

/// What `kill` is about to destroy, said and then done anyway.
///
/// The same finding `removal_refusal` renders and the opposite sentence, built
/// from the same [`RemovalRefused`] so the two cannot come to describe the work
/// differently. What changes is only the verb's answer to it: `rm` stops and hands
/// over `--force`, and `kill` is `--force` already, so the only thing left worth
/// doing with the finding is putting it on screen where somebody can read it
/// before their terminal scrolls.
///
/// No way past is offered, because there is nothing to offer: this is the last
/// line before the delete, and the workspace is gone by the next one.
pub(crate) fn removing_over_work(refused: &RemovalRefused) -> String {
    let workspace_id = &refused.workspace_id;
    match &refused.because {
        RemovalGrounds::WouldLose(holds) => {
            format!("{workspace_id} holds {holds}, and kill is deleting it anyway.")
        }
        RemovalGrounds::CouldNotTell(blank) => format!(
            "{workspace_id}: {blank}, so there may be work here that is nowhere else. kill is \
             deleting it anyway."
        ),
        RemovalGrounds::BothAtOnce {
            would_lose: holds,
            could_not_tell: blank,
        } => format!(
            "{workspace_id} holds {holds}, and {blank}, so there may be more here that is \
             nowhere else. kill is deleting it anyway."
        ),
    }
}

/// The workspace a removal is about to ask devpod for, said before the round trip.
///
/// The delete is the one place where "which workspace" was a question dl's own
/// output did not answer until it was over. `dl <ws> rm` can be handed a branch, a
/// path or a bare `owner/repo`, and `dl rm` is handed nothing at all: the picker
/// takes a row reading `<owner> | <repo> | <branch>`, hands an id back and then draws
/// its own screen away, so the name devpod is addressed by was never on screen at
/// the moment it was chosen. Everything the delete says afterwards names that id,
/// and this is the line they are answers to.
///
/// Before the round trip rather than after, for [`rm_on_exit_removing`]'s reason: a
/// `devpod delete` is seconds of container teardown, and a name that arrives once
/// the container is gone is a receipt rather than a warning. Said only once the
/// unsaved-work guard has passed, so it never announces a removal that is about to
/// be refused.
pub(crate) fn removing(workspace_id: &str) -> String {
    format!("Removing workspace {workspace_id}...")
}

/// devpod let go of the workspace: the receipt a delete that worked used to leave
/// entirely to devpod.
///
/// The clone lines beside this one name the *clone*, and a workspace with no clone
/// recorded prints none of them, so the only thing naming what had gone was `devpod
/// delete`'s own stdout: devpod's wording to change, on the other stream from every
/// line dl says about the same delete, and absent from exactly the case that has
/// nothing else. Last of the delete's lines because it is the one that closes it,
/// which is what makes a batch of picked rows readable by its ends.
///
/// **`insistence` decides the words, because it decides what the exit code proved.**
/// Without `--force`, devpod fails on a workspace it does not have, so a delete that
/// succeeded is a workspace devpod had and let go: `Removed` is a fact. `--force`
/// passes devpod's own `--ignore-not-found`, which is the flag asking for absence
/// rather than a removal — and it makes "there was nothing there" exit 0, with
/// nothing in the answer to tell it from a real delete. A path spec is resolved
/// without asking devpod anything at all, so `dl ./wrong-directory rm --force` gets
/// that far and comes back successful. Saying `Removed` there would be dl affirming,
/// on its own account, a delete that never happened.
///
/// One phrasing for `--force` whatever it found, rather than a guess between the
/// two: absence is what it asked for and absence is what it established, on a
/// workspace that was really there as much as on one that never was.
pub(crate) fn removed(workspace_id: &str, insistence: Insistence) -> String {
    match insistence {
        Insistence::NotInsisted => format!("Removed workspace {workspace_id}."),
        Insistence::Insisted => format!("Workspace {workspace_id} is gone."),
    }
}

/// What the picker took, said before the first workspace is touched: each row as it
/// was drawn, and the workspace id it resolved to.
///
/// **Both halves, because neither one is enough.** The picker draws
/// `<owner> | <repo> | <branch>` and every line after this names a workspace id, and an
/// id is `<repo-slug>-<ref-slug>-<suffix>`
/// ([`devlaunch_core::domain::workspace_id`]) with **no owner in it at all**: a fork
/// and its upstream are one id apart only in four characters of hash, which
/// [`select`](crate::select) deliberately never puts on screen because reading it is
/// no part of choosing a workspace. So an id alone cannot be checked against the row
/// that was chosen, and the row alone is not what devpod is addressed by. This line
/// is the one place both are known.
///
/// A batch takes a heading and one indented line each, because every workspace in a
/// batch is attempted whatever happened to the ones before it: a refusal in the
/// middle leaves a gap in the blocks below rather than ending the run, and the rows
/// written down here first are the only thing that gap can be read against. A single
/// pick is the same pair on one line: it is the common case, and a heading over one
/// row is a heading with nothing under it.
pub(crate) fn picked(verb: &str, picks: &NonEmpty<Chosen>) -> Vec<String> {
    let listed: Vec<String> = picks
        .iter()
        .map(|pick| format!("{} -> {}", pick.row, pick.workspace_id))
        .collect();
    match listed.as_slice() {
        [only] => vec![format!("Picked {only}")],
        several => {
            let mut lines = vec![format!("Picked {} workspaces for {verb}:", several.len())];
            lines.extend(several.iter().map(|pick| format!("  {pick}")));
            lines
        }
    }
}

/// `rme`: the shell is about to be hung up, said before the signal that does it.
///
/// **The pid is on the line because there is no way to check it beforehand, and no
/// way to predict it either.** `getppid()` names an interactive shell for the run
/// this verb is for and something else for others — a script, a surviving subshell —
/// and nothing distinguishes them. Worse, which one it is depends on the shell
/// rather than on the line: a subshell running a single command is usually replaced
/// by it, so `$(dl <ws> rme)` signals the shell that typed it and the terminal does
/// close, while the same line with a redirection leaves a subshell to die instead
/// (both measured, bash 5 and dash). So this line reports what was signalled rather
/// than claiming what it was.
///
/// Before the signal rather than after, for [`removing`]'s reason taken to its
/// limit: there is no "after" on the run that works. The shell reads this line or
/// nothing.
pub(crate) fn hanging_up(parent: i32) -> String {
    format!("Hanging up the shell dl was called from (pid {parent}).")
}

/// `rme` on a run that inherited an ignored SIGHUP: the removal happened, and the
/// signal is the one thing this run was told not to deal in.
///
/// Names the removal for [`nothing_to_hang_up`]'s reason. It does not name `nohup`
/// as the cause, because it is not the only one: `setsid` sets no `SIG_IGN` and does
/// not reach this, but a wrapper script or a supervisor that disarmed the signal
/// does, and a sentence naming the wrong tool is worse than one naming none.
pub(crate) fn hangup_disarmed() -> String {
    "rme: SIGHUP was already ignored when dl started, so the shell stays. The removal is done."
        .to_owned()
}

/// `rme` on a run whose parent has already gone: the removal happened, the hangup
/// has nobody to reach.
///
/// Says the removal is done, because this is the one `rme` line printed *instead*
/// of the signal rather than beside it, and a sentence that only said what did not
/// happen would read like a failed delete.
pub(crate) fn nothing_to_hang_up() -> String {
    "rme: dl's parent process has already gone, so there is no shell to hang up. The removal is \
     done."
        .to_owned()
}

/// `rme` whose signal was refused: a parent that exited in the meantime, or one
/// this user may not signal.
///
/// The removal is named for [`nothing_to_hang_up`]'s reason, and the OS error is
/// quoted because the two causes are told apart by nothing else — a terminal left
/// standing looks the same either way.
pub(crate) fn could_not_hang_up(parent: i32, why: &str) -> String {
    format!("rme: could not hang up pid {parent} ({why}). The removal is done.")
}

/// Whether the delete that was refused had `kill`'s sweep standing in front of it.
///
/// Carried into [`delete_refused`] rather than decided there, because the sentence
/// it settles is a piece of advice and the advice is only worth giving once: `dl
/// <ws> rm` that devpod refuses has a hammer left to reach for, and `dl <ws> kill`
/// that devpod refuses has already swung it. A message telling somebody to run the
/// command they are reading the output of is worse than no message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Swept {
    /// This delete is `kill`'s, so the sweep's report is already on screen above.
    Already,
    /// This delete is `rm`'s or `--rm`'s. Nothing has looked at the host yet.
    NotYet,
}

/// devpod would not let go of the workspace, and the clone was kept.
///
/// **Two causes and now two ways out.** The devcontainer.json that moved is the
/// one the sentence has always named, and it is the tidy failure: devpod parses
/// the file to tear the container down, and a delete that cannot find it refuses
/// promptly and says so. The other is the wedge, where devpod is refusing because
/// something on this host still holds the workspace, and the person reading this
/// has no way to tell the two apart from the outside. `kill` is the answer to the
/// second one and does the delete itself once it has cleared the first half, which
/// is why it is offered as a whole command rather than as a step to take first.
pub(crate) fn delete_refused(workspace: &str, swept: Swept) -> String {
    let hammer = match swept {
        Swept::NotYet => {
            format!(". If something on this host is holding it instead, run: dl {workspace} kill")
        }
        Swept::Already => String::new(),
    };
    format!(
        "devpod could not delete {workspace}; keeping the local clone so it stays retryable. If \
         its devcontainer.json moved, restore the path or run: devpod delete {workspace} \
         --force{hammer}"
    )
}

/// The one sentence a target no command can address gets.
/// Why two branches cannot both be launched, and which two they are.
///
/// **Names both specs and the id, because renaming a branch is the only way past
/// this.** A message that named one side would leave the reader hunting for the
/// other through `dl --ls`, where the two rows are the same 47 characters.
///
/// The second sentence is the stake rather than the mechanism. Nobody needs the
/// birthday bound at the moment they are stopped; they need to know that going
/// ahead would have handed them somebody else's checkout, because that is the
/// thing they would otherwise have gone looking for a devpod bug about.
pub(crate) fn id_collision(workspace_id: &str, spec: &str, held_by: &str) -> String {
    format!(
        "Workspace id '{workspace_id}' is already held by '{held_by}'. Launching '{spec}' under \
         it would put both in one clone directory and one container, so each would open the \
         other's checkout and 'dl <ws> rm' on either would delete work the other still owns. \
         Rename one of the two branches and launch it again."
    )
}

pub(crate) fn unknown_workspace(target: &str) -> String {
    format!(
        "Unknown workspace '{target}'. Use 'dl --ls' to list workspaces, or specify owner/repo or \
         ./path"
    )
}

// ---------------------------------------------------------------------------
// dl <ws> kill
// ---------------------------------------------------------------------------

/// The line the kill opens with, before anything is signalled.
///
/// Before rather than after, for [`removing`]'s reason and one more: the person
/// who typed this has just come from a `Trying to lock workspace` line repeating
/// every five seconds, and the first thing they need to know is that something
/// else is now happening.
pub(crate) fn killing(workspace_id: &str) -> String {
    format!("Killing whatever holds workspace {workspace_id}...")
}

/// Everything one sweep did, and whether the workspace is free at the end of it.
///
/// **The pid and the whole command line, per process.** The issue's author found
/// the orphan with `ps`, read its argv, and killed it by hand; a report that said
/// "killed 1 process" would ask them to do that again to check. The command line
/// is also the only thing that distinguishes an `up` from an `ssh`, which is the
/// difference between a build that was interrupted and a session that was.
///
/// **The last line is the verdict and it is always there**, because it is what
/// the exit code means: `dl <ws> kill` exits 0 when nothing is left holding the
/// workspace, and a script that reads `$?` is reading this sentence.
pub(crate) fn killed(workspace_id: &str, sweep: &Sweep) -> Vec<String> {
    let mut lines: Vec<String> = sweep
        .signalled
        .iter()
        .map(|signalled| {
            let what = match signalled.ending {
                Ending::Terminated => "gone (SIGTERM)",
                Ending::Killed => "gone (SIGKILL)",
                Ending::Survived => "still running after SIGKILL",
            };
            format!(
                "  {} {what}: {}",
                signalled.process.pid, signalled.process.command
            )
        })
        .collect();
    let already_named: Vec<u32> = lines_named(sweep);
    lines.extend(
        sweep
            .holding
            .holders()
            .iter()
            .filter(|standing| !already_named.contains(&standing.process().pid))
            .map(|standing| {
                let why = match standing {
                    // Named as a build rather than as "something is watching it",
                    // because the two ask different things of the reader: a session
                    // is somebody to ignore, and a build is somebody to wait for.
                    Standing::ABuild(_) => "left alone, this is a live build",
                    Standing::ASession(_) => "left alone, something is still watching it",
                    // The holder that took no signal, because it lost its parent
                    // while the sweep was running. Without this line it is the one
                    // thing on the report that stops the delete and is nowhere on
                    // the report.
                    Standing::AnOrphan(_) => "still holding it, and nothing is waiting on it",
                };
                let process = standing.process();
                format!("  {} {why}: {}", process.pid, process.command)
            }),
    );
    lines.extend(busy_marker(&sweep.marker));
    lines.extend(containers_killed(&sweep.containers));
    lines.push(verdict(workspace_id, sweep));
    lines
}

/// The pids the signal lines above have already accounted for.
///
/// A survivor is in both lists by construction — it is a holder in the final
/// reading and it is the thing `still running after SIGKILL` is said about — and
/// one report saying it twice, in two different phrasings, reads as two processes.
/// The signal line wins because it says more: how far the escalation got.
fn lines_named(sweep: &Sweep) -> Vec<u32> {
    sweep
        .signalled
        .iter()
        .map(|signalled| signalled.process.pid)
        .collect()
}

/// What became of devpod's busy marker, where there is anything to say.
///
/// Silent for the two arms that are neither an action nor a problem: a marker
/// that was never left behind, and a host whose devpod records cannot address
/// one. Both are ordinary, and a line about each would bury the two that matter.
fn busy_marker(marker: &Marker) -> Vec<String> {
    match marker {
        Marker::Removed(path) => vec![format!(
            "Removed devpod's stale busy marker: {}",
            path.display()
        )],
        Marker::LeftForALiveHolder => {
            vec![
                "Left devpod's busy marker alone: something still holds this workspace.".to_owned(),
            ]
        }
        Marker::Unremovable { path, reason } => vec![format!(
            "Could not remove devpod's busy marker at {} ({reason})",
            path.display()
        )],
        Marker::Absent | Marker::Unlocatable => Vec::new(),
    }
}

/// What became of the containers, where there is anything to say.
///
/// Silent for a project with nothing running and for a host with no docker, for
/// the reason the volume sweep is silent about the same two: neither is a fact
/// about this workspace. The container is rarely what was stuck.
fn containers_killed(containers: &Containers) -> Vec<String> {
    match containers {
        Containers::Killed(ids) => vec![format!(
            "Killed {} container{}: {}",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" },
            ids.join(", ")
        )],
        Containers::Standing(refusal) => vec![format!(
            "Could not kill this workspace's containers: {}",
            container_refusal(refusal)
        )],
        // Said, unlike the two silent arms, because it is the one place the sweep
        // decided *not* to do something the verb advertises: the line above it
        // already names the build that was left alone, and this says its
        // containers went with it.
        Containers::LeftForALiveBuild => {
            vec!["Left this workspace's containers alone: they belong to that build.".to_owned()]
        }
        Containers::NoneRunning | Containers::NoDocker => Vec::new(),
    }
}

fn container_refusal(refusal: &ContainerRefusal) -> String {
    match refusal {
        ContainerRefusal::Refused { exit, stderr } => match stderr.trim() {
            "" => format!("docker exited {}", exit_status(*exit)),
            said => said.to_owned(),
        },
        ContainerRefusal::NotRun { failure } => {
            format!("could not run docker ({})", os_error_phrase(failure))
        }
    }
}

/// Whether the workspace is free, in one sentence.
///
/// Three endings and not two, because "nothing was holding it" and "what was
/// holding it is gone" send a reader to different places: the first says the hang
/// is somewhere this verb does not reach, and the second says to try the launch
/// again. Which of the three comes from the sweep's own [`Holding`], never from a
/// second reading of the lists above, so that no two lines of one report can
/// disagree about what was left standing.
///
/// **This closes the sweep and no longer closes the command.** It used to be the
/// exit code's sentence, back when the sweep was the whole verb; the delete that
/// follows it now is what `$?` answers for. The distinction is worth keeping in
/// view when editing either: "still held" above a workspace that was then deleted
/// is not a contradiction, it is the sweep saying what it left and the delete
/// stepping past it.
fn verdict(workspace_id: &str, sweep: &Sweep) -> String {
    match sweep.holding {
        Holding::StillHeld { .. } => format!("Workspace {workspace_id} is still held."),
        Holding::Free if sweep.signalled.is_empty() => {
            format!("Nothing on this host is holding workspace {workspace_id}.")
        }
        Holding::Free => format!("Workspace {workspace_id} is no longer held."),
    }
}

/// Why the delete that `kill` ends in was not attempted.
///
/// Said rather than silently skipped, because the verb advertises the removal and
/// somebody who typed it and got no delete is owed which half stopped. The second
/// sentence is the part that is not obvious: a `devpod delete` over a lock it
/// cannot take does not *fail*, it blocks on the same five-second line that sent
/// this person here, so attempting it would answer a hang with a hang.
///
/// **It sends the reader back to `kill`, not on to `rm`.** `rm`'s delete carries
/// devpod's defaults and devpod's patience, which on a workspace whose lock is
/// still held is the unbounded wait this whole verb exists to avoid — so naming it
/// here would hand somebody the exact command the transcript behind this feature
/// had to be Ctrl-C'd out of. The lines above already name what is holding it.
pub(crate) fn kill_delete_withheld(workspace_id: &str) -> String {
    format!(
        "Not deleting {workspace_id}: it is still held, and devpod's delete would wait on the \
         lock with no deadline behind it. Deal with what is named above, then run 'dl \
         {workspace_id} kill' again."
    )
}

/// A host `dl <ws> kill` cannot work on, and which tool is missing.
///
/// Its own sentence rather than an empty sweep, because the two would look alike
/// and mean opposite things: a host with no `ps` has not established that nothing
/// is holding the workspace, it has failed to look.
pub(crate) fn kill_unavailable(cannot: &HostCannot) -> String {
    match cannot {
        HostCannot::ReadItsProcessTable(TableUnreadable::NoPs) => {
            "error: `dl <workspace> kill` reads the host's process table with `ps`, and this \
             host has none"
                .to_owned()
        }
        HostCannot::ReadItsProcessTable(TableUnreadable::Refused { exit, stderr }) => {
            let said = match stderr.trim() {
                "" => format!("it exited {}", exit_status(*exit)),
                said => said.to_owned(),
            };
            format!("error: `ps` would not read this host's process table: {said}")
        }
        HostCannot::ReadItsProcessTable(TableUnreadable::NotStarted(failure)) => format!(
            "error: `ps` could not be run ({})",
            os_error_phrase(failure)
        ),
        HostCannot::SendASignal(NoSignal::NoKillHere) => {
            "error: something is holding this workspace and this host has no `kill` to signal \
             it with"
                .to_owned()
        }
        // Its own sentence, because the one above sends its reader looking for a
        // program that is already installed. A `kill` the OS would not start is a
        // fact about this run, not about the machine's toolchain.
        HostCannot::SendASignal(NoSignal::NotRun(failure)) => format!(
            "error: something is holding this workspace and `kill` could not be run ({})",
            os_error_phrase(failure)
        ),
    }
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

/// Whether the plan above the leaving list is still a decision.
///
/// `dl --purge` prints the block and *then* asks, so every line of it is something
/// the reader can still prevent. `dl --purge -y` answered on the command line, and
/// the same lines are a record of what is about to happen instead. One sentence in
/// the block turns on that difference, which is why the renderer is told rather
/// than left to print advice into a run that has stopped taking any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Confirmation {
    /// `Are you sure? [y/N]` is coming.
    WillBeAsked,
    /// `-y` answered it before the plan was printed.
    AnsweredOnTheLine,
}

/// What removing the cache costs the workspaces the purge is *not* deleting.
///
/// Said in the block that lists them, because it is a reason to answer `n` and
/// remove one of them properly first (devlaunch#461). Two things are true of a
/// survivor and only the first is obvious: it keeps working, and dl stops knowing
/// anything about it.
///
/// **The volume names are named because they are now in the cache**
/// (devlaunch#456, merged while this was open). dl keeps its own copy of the two
/// volumes a workspace's devcontainer made, under the cache, so a purge that
/// leaves a foreign workspace standing destroys the copy of *its* names while
/// leaving its volumes -- which is the exact case #452 predicted this sentence
/// would have to cover. What is not lost is the ordinary route: `dl <ws> rm`
/// reads devpod's own `workspace_result.json` under `DEVPOD_HOME`, which a purge
/// does not touch, so a survivor deleted through dl still takes its volumes with
/// it. The copy is what `--prune` reclaims from *after* devpod has forgotten a
/// workspace, and that is the reach a purge costs it.
const SURVIVORS_KEEP_WORKING: &str = "Removing the cache also drops what dl recorded about them, the copy of their volume \
     names included. They keep working, and `dl <workspace> rm` still removes one and its \
     volumes while devpod still lists it.";

/// The sentence under that one, in the tense the run has earned.
///
/// Where the loss is a loss rather than untidiness. A clone a pre-#467 dl placed
/// under `worktree.repos_dir` is outside the cache, so the workspace opening it is
/// foreign here and stays; the record naming that directory is *inside* the cache
/// and goes. Afterwards `dl <ws> rm` answers `NothingRecorded` and leaves the tree
/// standing, and nothing else on the machine mentions it.
///
/// **Two spellings of one fact, and what separates them is whether it is still
/// actionable.** Printed above the question, "remove such a workspace now" is the
/// action the whole block exists to offer. Printed under `-y` it would be asking
/// for something the same run makes impossible three lines later, which is advice
/// arriving after the door shut. The subject clause is the same either way; only
/// what follows from it moves.
fn stranded_clones(confirmation: Confirmation) -> &'static str {
    match confirmation {
        Confirmation::WillBeAsked => {
            "A clone an older dl placed outside the cache is named only by a record in there, \
             though, so remove such a workspace now if the clone should go with it."
        }
        Confirmation::AnsweredOnTheLine => {
            "A clone an older dl placed outside the cache is named only by a record in there, \
             though, so from here on `dl <workspace> rm` takes such a workspace and leaves its \
             clone standing."
        }
    }
}

/// One survivor's line in the leaving list.
///
/// A function rather than a `format!` in the loop so that the sample output in
/// `docs/cleanup.md` can be diffed against the real shape of the line. The page is
/// a second hand-maintained copy of it, and `the_cleanup_page_quotes_what_a_purge_
/// really_prints` is the test the standing rule asks for beside such a copy.
fn leaving_line(id: &str, source: &str) -> String {
    format!("  - {id}: {source}")
}

/// What a purge would take, printed before the question is asked.
///
/// The workspaces devlaunch did not create are *named* rather than merely left out
/// of the count: a user who asked for a clean slate and gets survivors should
/// learn it while saying no is still an option.
///
/// **Named by their source and not only by their id** (devlaunch#461). An id is
/// what devpod addresses a workspace by and carries nothing about where it came
/// from, so `someones-project` reads the same whether it is a `dl ./project` of
/// yours, a `dl <git-url>`, or a workspace somebody made with `devpod up` -- and
/// this is the one screen where that difference is being decided on. The source is
/// the same string `dl --ls` puts in its `SOURCE` column, from the same reading of
/// it, so the two surfaces cannot describe one workspace differently.
pub(crate) fn purge_plan_lines(plan: &PurgePlan, confirmation: Confirmation) -> Vec<String> {
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
                .map(|workspace| leaving_line(&workspace.id, &left_standing_source(workspace))),
        );
        lines.push(String::new());
        lines.push(SURVIVORS_KEEP_WORKING.to_owned());
        lines.push(stranded_clones(confirmation).to_owned());
    }
    lines.push(String::new());
    lines
}

/// Where one surviving workspace came from, as the leaving list names it.
///
/// [`describe_source`](listing::describe_source) is what `dl --ls` reads, and the
/// detail alone carries the answer for the two arms that have one: a path is a
/// path and a URL is a URL, and neither needs the `TYPE` column's word repeated
/// beside it. The third arm does, because devpod's own object is not a source in
/// any readable sense and would otherwise sit after a colon looking like one.
fn left_standing_source(workspace: &Workspace) -> String {
    let described = listing::describe_source(workspace.source());
    match described.kind {
        SourceKind::Local | SourceKind::Git => described.detail,
        SourceKind::Unknown => format!("a source dl cannot read, {}", described.detail),
    }
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
            occasion,
            refusal,
        } => Line::Err(volumes_not_removed(workspace_id, *occasion, refusal)),
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
                line = format!("{line} -- {}; removing anyway", standing_words(despite));
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
    if !plan.reclaiming().is_empty() {
        // Every name, not a count. These are volumes, and a volume is not an image:
        // deleting one that still matters is data loss rather than a rebuild, so
        // what is about to be removed is on the screen before the question is asked.
        lines.push(format!(
            "Reclaiming the Docker volumes of {} workspace(s) devpod no longer lists:",
            plan.reclaiming().len()
        ));
        for reclaimable in plan.reclaiming() {
            lines.push(format!(
                "  - {}: {}",
                reclaimable.workspace_id,
                reclaimable
                    .names
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
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
/// site is (devlaunch#426).
///
/// Its own section under the clone plan rather than rows mixed into it, because
/// these are a different kind of thing: every one of them is inside a clone the
/// run has just said it is *not* touching, and the rules that reach them are
/// their own. Nothing at all is printed when there is nothing to say, which is
/// every host that has never run an agent in a workspace.
///
/// One line per standing site, attributed to the site rather than to each
/// ancestor it pins: a parent reported as "kept" with the child unnamed is the
/// invisible straggler, and a three-deep chain pinned by one site is one line.
fn worktree_plan_lines(sweep: &WorktreeSweep) -> Vec<String> {
    if sweep.nothing_to_say() {
        return Vec::new();
    }
    let derivable = sweep
        .clones()
        .iter()
        .flat_map(|found| found.derivatives())
        .filter(|it| it.derivable().is_some())
        .count();
    let mut lines = vec![
        if derivable == 0 {
            format!(
                "Agent git worktrees inside the clones above -- {}:",
                describe_usage(&sweep.freed())
            )
        } else {
            // Two figures because they are two claims about two disjoint sets
            // of directories, and each says which set it is about. One number
            // covering both would describe neither, and an unlabelled pair
            // reads as a total and a part of it.
            format!(
                "Agent git worktrees inside the clones above -- {} in worktrees that go, and \
                 {} in regenerable subtrees inside the ones that stay:",
                describe_usage(&sweep.freed()),
                describe_usage(&sweep.derivatives_freed())
            )
        },
        String::new(),
    ];
    for found in sweep.clones() {
        lines.push(format!("  {}:", found.clone_path().display()));
        for going in found.going() {
            let mut line = match going.what() {
                Collectable::Directory(directory) => format!(
                    "    - removing {} ({}), and dropping its {} registration(s)",
                    directory.at().display(),
                    describe_usage(directory.usage()),
                    directory.forgets().len()
                ),
                Collectable::Registration(registration) => format!(
                    "    - forgetting the registration for {}: nothing is at it in this \
                     clone, and its commits are all somewhere else",
                    registration.place().as_str()
                ),
            };
            // What `--force-worktrees` is answering, on the line of the unit it
            // answers for. Without it the plan reads the same for a worktree
            // holding an afternoon's work as for a finished one.
            if let WorktreePromotion::Insisted { despite } = going.promotion() {
                line = format!("{line} -- {}; removing anyway", standing_words(despite));
            }
            lines.push(line);
        }
        for standing in found.standing() {
            lines.push(format!(
                "    - leaving {}: {} -- add --force-worktrees to remove it anyway",
                standing.at().display(),
                standing.reasons().describe()
            ));
        }
        // Each derivative by name and by size, before the question is asked.
        // These sit inside worktrees the run has just said it is leaving, so
        // the line has to say which directory it means and what it costs, or
        // the y/N is answering a total nobody can decompose.
        for tagged in found.derivatives() {
            let at = found.clone_path().join(tagged.at().as_str());
            lines.push(match tagged.standing() {
                None => format!(
                    "    - reclaiming {} ({}): {}",
                    at.display(),
                    describe_usage(tagged.usage()),
                    tagged
                        .derivable()
                        .map(|it| it.recipe().describe())
                        .unwrap_or_default()
                ),
                Some(why) => format!(
                    "    - leaving {} ({}): {}",
                    at.display(),
                    describe_usage(tagged.usage()),
                    why
                ),
            });
        }
    }
    lines.push(String::new());
    // Said once, rather than implied by every line above it. `--prune` is a
    // local command and deliberately does not fetch, so "nothing else reaches
    // these commits" is a statement about the last fetch and not about the
    // forge now.
    lines.push(
        "Whether a worktree's commits are anywhere else is as of the last fetch into the \
         repository cache; --prune does not fetch."
            .to_owned(),
    );
    if derivable > 0 {
        // What is being consented to, said once. The bytes come back the moment
        // a person runs the command in the line above their own directory, and
        // the tag is the creating program's own declaration rather than
        // anything dl inferred from a directory name.
        lines.push(
            "A regenerable subtree is one whose creator wrote a CACHEDIR.TAG into it and \
             whose lockfile is still beside it; putting one back is one command and no \
             network beyond the shared package cache."
                .to_owned(),
        );
    }
    lines.push(String::new());
    lines
}

/// Everything a standing says, as the clause a keep or an insistence line
/// interpolates. Both kinds of reason, never one standing in for the other.
fn standing_words(standing: &WorktreeStanding) -> String {
    let mut parts = Vec::new();
    if let Some(holds) = standing.would_lose() {
        parts.push(format!("holds {holds}"));
    }
    if let Some(blank) = standing.could_not_tell() {
        parts.push(format!("could not be proved safe: {blank}"));
    }
    if parts.is_empty() {
        // A standing is non-empty by construction; this is the match staying
        // total rather than a case anything reaches.
        return standing.describe();
    }
    parts.join(", and ")
}

/// What the run did about the agent worktrees.
///
/// The withheld lines say *that this was not so when the plan was printed*,
/// which is the whole of what a second classification has to tell somebody who
/// has already read the first one -- and here it is not a rare race: a container
/// is not a participant in devlaunch's repository lock, so it can write into a
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
    if report.forgotten > 0 {
        lines.push(format!(
            "Dropped {} worktree registration(s), each by the path git printed for it.",
            report.forgotten
        ));
    }
    if !report.reclaimed.is_empty() {
        lines.push(format!(
            "Reclaimed {} regenerable subtree(s) inside the worktrees that stayed -- {}.",
            report.reclaimed.len(),
            describe_usage(&report.derivatives_freed())
        ));
    }
    for withheld in &report.withheld_derivatives {
        lines.push(format!(
            "Left {}: {}. That was not so when the plan above was printed.",
            withheld.path.display(),
            withheld.because.describe()
        ));
    }
    for withheld in &report.withheld {
        lines.push(format!(
            "Left {}: {} -- add --force-worktrees to remove it anyway. That was not so when \
             the plan above was printed.",
            withheld.path.display(),
            withheld.because.describe()
        ));
    }
    for refused in &report.forget_refused {
        lines.push(format!(
            "git would not drop the registration for {}: {}. The next --prune will offer it \
             again.",
            refused.registered.display(),
            refused.reason
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
    if !report.refused_derivatives.is_empty() {
        let by_hand: Vec<std::path::PathBuf> = report
            .refused_derivatives
            .iter()
            .map(|refusal| refusal.path.clone())
            .collect();
        // Its own heading, because the worktree holding each of these is
        // standing and was never being removed: filed under the one above, the
        // report would name a worktree that did come away, or one nobody
        // touched. What is left is a part-removed subtree, so the sentence says
        // the one command that puts it back.
        lines.extend(report_refusals(
            report.refused_derivatives.iter(),
            "Some regenerable subtrees would not come away. The worktrees holding them \
             are untouched, and a subtree left part-removed is restored by re-running \
             its own install. These refused:",
            &by_hand,
        ));
    }
    lines
}

/// Why one clone directory is staying, as the report says it.
fn kept_because(because: &KeptBecause) -> String {
    match because {
        KeptBecause::StillOpened { workspace_id } => {
            format!("workspace {workspace_id} still opens it")
        }
        KeptBecause::Objected(standing) => {
            format!(
                "{} -- add --force to remove it anyway",
                standing_words(standing)
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
    if !report.reclaimed.is_empty() {
        lines.push(format!(
            "Reclaimed the Docker volumes of {} workspace(s) devpod no longer lists.",
            report.reclaimed.len()
        ));
    }
    for kept in &report.volumes_kept {
        lines.push(match &kept.because {
            // Said in the plan's own words, because the plan is what promised it:
            // a workspace back in devpod's listing is the volume half of the
            // withheld line above, and for the same reason.
            VolumesKeptBecause::ListedAgain => format!(
                "Left the Docker volumes of {}: devpod lists that workspace again. That was \
                 not so when the plan above was printed.",
                kept.workspace_id
            ),
            VolumesKeptBecause::Refused(refusal) => {
                volumes_not_removed(&kept.workspace_id, SweepOccasion::KeptCopy, refusal)
            }
        });
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
        LaunchNotice::ClaudeProfileNotForwarded { name } => format!(
            "Ignoring --claude-profile {}: this workspace's Claude configuration is not the \
             host's to forward into, so `claude` runs as whichever account that configuration \
             holds. A workspace that predates the check picks it up after one `up`.",
            python_repr(name)
        ),

        // --- the session (warning at 3845, info at 3864/3891, debug at 3875)
        LaunchNotice::NoTerminalAlias {
            workspace_id,
            config,
        } => format!(
            "No devpod ssh host entry for {workspace_id} in {}, so this command gets no terminal; \
             interactive programs may exit immediately. `dl {workspace_id} restart` republishes it.",
            config.display()
        ),
        // The advice here is deliberately *qualified* where NoTerminalAlias's is
        // flat, and that difference is the whole reason these are two arms. A
        // restart republishes an entry into a config that exists; against a config
        // that is not there it only helps if devpod publishes into the same file dl
        // read, and when it does not, the same notice comes back.
        LaunchNotice::NoDevpodSshConfig {
            workspace_id,
            looked_in,
        } => format!(
            "No ssh config at {}, which is where dl expects `devpod up` to publish its host \
             aliases, so {workspace_id} has no alias and this command gets no terminal; \
             interactive programs may exit immediately. `dl {workspace_id} restart` publishes \
             there if that is the file devpod writes; if this comes back, DEVPOD_SSH_CONFIG or \
             devpod's ssh-config context options name a different one.",
            looked_in.display()
        ),
        LaunchNotice::SshConfigUnlocatable => {
            "This machine has no home directory and nothing names an ssh config \
             (DEVPOD_SSH_CONFIG, or devpod's SSH_CONFIG_PATH context option), so dl cannot tell \
             where `devpod up` publishes its host aliases. This command gets no terminal; \
             interactive programs may exit immediately."
                .to_owned()
        }
        LaunchNotice::SshCommand { argv } => format!("SSH command: {}", argv.join(" ")),
        LaunchNotice::SessionManagerReady { pane_id, socket } => format!(
            "Session manager: reporting agents in this workspace to pane {pane_id} over {socket}"
        ),
        // A warning and not a debug line: the whole point of the feature is that a
        // manager which silently sees nothing is expensive, so the launch that
        // could not wire it up says so once.
        LaunchNotice::SessionManagerUnavailable { reason } => {
            format!("Session manager: agents in this workspace will not be visible; {reason}")
        }
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

        // --- the herdr tab (no level either: a command, not a sentence)
        //
        // Same reason as the title above, plus one: this stage is best-effort, so
        // a line saying it happened would be a claim the sink cannot back.
        LaunchNotice::HerdrTab(_) => return None,

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
        // The herdr tab, which no escape addresses. The other half of naming the
        // terminal, and the binary's for the same reason: core writes to no stream
        // and runs no command it was not handed a runner for.
        //
        // **Spawned and not waited on.** Every failure here is survivable and none
        // of them is the launch's -- a stale tab id, a server that has exited, a
        // binary that moved -- so nothing is checked, and a `herdr` that accepted
        // the connection and never answered must not be able to hold a workspace
        // hostage. The cost of not waiting is that dl does not reap it: one
        // short-lived entry in the process table, against a hang with no timeout to
        // bound it. Every stream is closed so it cannot write over the title that
        // was just flushed, or read from a stdin the session is about to want.
        if let LaunchNotice::HerdrTab(rename) = &notice {
            // `split_first` rather than `argv[0]`, so the "a command always has a
            // program" invariant is checked here instead of borrowed from the
            // shape of the vector the other crate built.
            if let Some((program, args)) = rename.argv().as_deref().and_then(<[&str]>::split_first)
            {
                let _ = process::Command::new(program)
                    .args(args)
                    .stdin(process::Stdio::null())
                    .stdout(process::Stdio::null())
                    .stderr(process::Stdio::null())
                    .spawn();
            }
            return;
        }
        if let Some(line) = launch_notice(&notice) {
            eprintln!("{line}");
        }
    }
}

/// A removal's notices through the same sink, at the moment each one happens.
///
/// Streamed rather than collected because a removal's lines are a sequence and not
/// a summary: what the guard found, which workspace is going, and what became of
/// its clone, each said while the step it describes is the one under way. Collected
/// into a vector they all arrived after `devpod delete` had returned, which put
/// "Removing workspace X..." after the wait it exists to explain.
impl Notices<LifecycleNotice> for Saying {
    fn say(&mut self, notice: LifecycleNotice) {
        if let Some(line) = lifecycle_notice(&notice) {
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

/// The records' open reports through the same sink, at the moment it opens them.
///
/// Which is once per command, because the open is: a `ColdPath` that has already
/// been asked answers from what it holds, so a damaged `metadata.json` is described
/// once however many verbs go looking at it.
impl Notices<RecordsNotice> for Saying {
    fn say(&mut self, notice: RecordsNotice) {
        for line in records_notice(&notice) {
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
        LaunchRefusal::IdCollision {
            workspace_id,
            owner,
            repo,
            branch,
            recorded_owner,
            recorded_repo,
            recorded_branch,
        } => Some(id_collision(
            workspace_id,
            &format!("{owner}/{repo}@{branch}"),
            &format!("{recorded_owner}/{recorded_repo}@{recorded_branch}"),
        )),
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
/// reader after a problem they do not have. Which refusal is which is not decided
/// here: [`GitFailure::RepositoryNotFound`] is read off git's stderr in
/// `clients::git`, where git's words are already being read, and this asks for
/// the arm. A renderer that classified would be a second place git's wordings
/// live.
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
    if git.how() != GitFailure::RepositoryNotFound {
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
        BranchNotNamed::Cold(refused) => cold_refused(refused),
        BranchNotNamed::Repository(refused) => ensure_repo_failure(refused),
    }
}

fn not_prepared(error: &NotPrepared) -> String {
    match error {
        NotPrepared::Cold(refused) => cold_refused(refused),
        NotPrepared::Preparation(refused) => prepare_cold_failure(refused),
    }
}

/// Why the cold path could not be opened, without the `error: ` prefix.
///
/// Quoted inside core's own refusals — `Repository 'owner/repo': <this>` — which is
/// why the prefix is the caller's, the way [`startup_reason`] is. The words are here
/// and not in core: `ColdRefused` is a sum over the reasons since #340, and this is
/// the match that turns each arm into the sentence Python printed for it.
fn cold_refused(refused: &ColdRefused) -> String {
    match refused {
        ColdRefused::Startup(error) => startup_reason(error),
        ColdRefused::NoColdPath => "the cold path is not available to this caller".to_owned(),
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
        // Names what was typed, where it was looked, and the one command that fixes
        // it. The directory is worth printing because the usual cause is a profile
        // that exists with nothing logged in to it, which reads as "the profile is
        // right there, why can dl not see it".
        //
        // The last sentence is the part that is not obvious from the failure: dl
        // could have forwarded the default login and did not, on purpose.
        SessionRefused::ClaudeProfile {
            name,
            problem: ClaudeProfileProblem::NoCredential { directory },
        } => format!(
            "error: --claude-profile {name}: no Claude credential in {directory}. Log \
             that profile in with 'CLAUDE_CONFIG_DIR={directory} claude', or drop the \
             flag to forward the default login. Refusing rather than forwarding a \
             different account."
        ),
        SessionRefused::ClaudeProfile {
            name,
            problem: ClaudeProfileProblem::NotAName,
        } => format!(
            "error: --claude-profile {}: a profile name is one directory component of \
             letters, digits, '.', '_' and '-', and cannot begin with '.' or '-'.",
            python_repr(name)
        ),
        SessionRefused::ClaudeProfile {
            name,
            problem: ClaudeProfileProblem::NoRoot,
        } => format!(
            "error: --claude-profile {name}: this host resolves no Claude profiles \
             directory, because it names no home directory and set neither \
             CLAUDE_PROFILES_DIR nor DEVLAUNCH_CLAUDE_PROFILES_DIR."
        ),
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

    use devlaunch_core::flows::kill::{HostProcess, Signalled};
    use devlaunch_core::flows::launch::{HerdrTabRename, TerminalTitle};
    use devlaunch_core::flows::listing::{SourceDescription, SourceKind};

    use super::*;

    /// The three sentences a refused `--claude-profile` produces.
    ///
    /// Worth pinning as text rather than as "it errored", because the whole argument
    /// for refusing instead of falling back is that the person reading this can act on
    /// it. A refusal nobody can act on is a worse outcome than the silent fallback it
    /// replaced.
    #[test]
    fn a_refused_profile_says_what_to_do_about_it() {
        let no_credential = session_refusal(&SessionRefused::ClaudeProfile {
            name: "work".to_owned(),
            problem: ClaudeProfileProblem::NoCredential {
                directory: "/home/me/.claude-profiles/work".to_owned(),
            },
        });
        assert!(
            no_credential.starts_with("error: --claude-profile work: "),
            "{no_credential}"
        );
        // The directory, twice: once as where it looked and once inside the command
        // that fixes it. `CLAUDE_CONFIG_DIR` takes a directory, so a message naming
        // the credential file here would be telling somebody to point it at a .json.
        assert!(
            no_credential.contains("CLAUDE_CONFIG_DIR=/home/me/.claude-profiles/work claude"),
            "{no_credential}"
        );
        // And the fact that is not deducible from the failure itself.
        assert!(
            no_credential.contains("Refusing rather than forwarding"),
            "{no_credential}"
        );

        let not_a_name = session_refusal(&SessionRefused::ClaudeProfile {
            name: "../../etc".to_owned(),
            problem: ClaudeProfileProblem::NotAName,
        });
        // Quoted, so a name full of dots and slashes reads as one argument rather than
        // as prose that happens to contain them.
        assert!(not_a_name.contains("'../../etc'"), "{not_a_name}");
        assert!(
            not_a_name.contains("one directory component"),
            "{not_a_name}"
        );

        let no_root = session_refusal(&SessionRefused::ClaudeProfile {
            name: "work".to_owned(),
            problem: ClaudeProfileProblem::NoRoot,
        });
        // Names both variables, because a host in this state has set neither and
        // either one answers it.
        assert!(no_root.contains("CLAUDE_PROFILES_DIR"), "{no_root}");
        assert!(
            no_root.contains("DEVLAUNCH_CLAUDE_PROFILES_DIR"),
            "{no_root}"
        );
    }

    #[test]
    fn no_refusal_message_carries_a_credential() {
        // The refusal is built from a name the user typed and a directory they made,
        // and neither is a secret -- but this is the one path that has a token in
        // scope one call away, so it is worth an assertion rather than a reading.
        for problem in [
            ClaudeProfileProblem::NotAName,
            ClaudeProfileProblem::NoRoot,
            ClaudeProfileProblem::NoCredential {
                directory: "/home/me/.claude-profiles/work".to_owned(),
            },
        ] {
            let said = session_refusal(&SessionRefused::ClaudeProfile {
                name: "work".to_owned(),
                problem,
            });
            assert!(!said.contains("sk-ant"), "{said}");
            assert!(!said.contains("accessToken"), "{said}");
            assert!(!said.contains(".credentials.json"), "{said}");
        }
    }

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

    // ------------------------------------------- the cold path's typed refusal

    /// The other half of devlaunch#339: core carries the reason, and this module
    /// is where it becomes a sentence.
    ///
    /// Every arm is asserted whole rather than by substring, because the claim the
    /// typing was made under is that the words did not move: `ColdRefused` used to
    /// arrive here already rendered, and these are the exact strings it used to
    /// arrive with. A `contains` would pass while a rewrite quietly changed the
    /// line a user reads.
    #[test]
    fn every_arm_of_a_cold_refusal_renders_the_sentence_it_used_to_carry() {
        assert_eq!(
            cold_refused(&ColdRefused::Startup(StartupError::NoHomeDirectory)),
            "this machine names no home directory, so dl cannot find its cache"
        );
        assert_eq!(
            cold_refused(&ColdRefused::Startup(StartupError::Config(
                config::ConfigError::NotToml {
                    path: PathBuf::from("/cfg/devlaunch/config.toml"),
                    reason: "expected `.`, `=`".to_owned(),
                }
            ))),
            "/cfg/devlaunch/config.toml is not usable: expected `.`, `=`"
        );
        assert_eq!(
            cold_refused(&ColdRefused::Startup(StartupError::Metadata(
                metadata::MetadataError::CreateDir {
                    path: PathBuf::from("/cache/devlaunch"),
                    failure: metadata::OsFailure {
                        kind: std::io::ErrorKind::NotADirectory,
                        message: "Not a directory (os error 20)".to_owned(),
                    },
                }
            ))),
            "could not create the directory for dl's records at /cache/devlaunch \
             (Not a directory (os error 20))"
        );
        // The arm that replaced a literal written in core. Same words, said here.
        assert_eq!(
            cold_refused(&ColdRefused::NoColdPath),
            "the cold path is not available to this caller"
        );
    }

    /// And the refusal reaches the user inside the line the launch refuses with.
    ///
    /// `Repository '{owner}/{repo}': …` is Python's sentence and the reason it is a
    /// reason phrase rather than a sentence of its own: the prefix belongs to the
    /// caller, so the two must compose exactly here.
    #[test]
    fn a_cold_refusal_is_quoted_into_the_launch_refusal_that_carries_it() {
        let line = launch_refusal(&LaunchRefusal::BranchNotNamed {
            owner: "blooop".to_owned(),
            repo: "devlaunch".to_owned(),
            error: BranchNotNamed::Cold(ColdRefused::Startup(StartupError::NoHomeDirectory)),
        });

        assert_eq!(
            line.as_deref(),
            Some(
                "Repository 'blooop/devlaunch': this machine names no home directory, \
                 so dl cannot find its cache"
            )
        );
    }

    /// A `config.toml` that could not be read reports the OS's own words.
    ///
    /// Pinned because #340 changed what carries them: `ConfigError::Unreadable` held
    /// an `io::Error` and now holds an `OsFailure`, so that the refusal can be
    /// cloned into `ColdRefused`. `OsFailure::message` is `io::Error::to_string()`,
    /// and this is what says the line did not move.
    #[test]
    fn an_unreadable_config_still_reads_as_the_os_error_it_was() {
        let refused: config::ConfigError = config::ConfigError::Unreadable {
            path: PathBuf::from("/cfg/devlaunch/config.toml"),
            source: std::io::Error::from_raw_os_error(13).into(),
        };

        assert_eq!(
            config_error(&refused),
            format!(
                "could not read /cfg/devlaunch/config.toml ({})",
                std::io::Error::from_raw_os_error(13)
            )
        );
    }

    // ------------------------------------------------------- the retired keys

    #[test]
    fn a_retired_repos_dir_is_named_with_the_directory_it_pointed_at() {
        // The whole of #467's migration: nothing on disk is touched, so this line
        // is the only thing that will ever mention that tree again.
        let lines = retired_keys(&[config::RetiredKey::ReposDir {
            named: "/srv/clones".to_owned(),
        }]);

        assert_eq!(lines.len(), 1, "{lines:?}");
        let said = &lines[0];
        assert!(said.contains("worktree.repos_dir"), "{said}");
        assert!(said.contains("/srv/clones"), "{said}");
        assert!(
            said.contains("XDG_CACHE_HOME"),
            "it has to point at what does move the cache: {said}"
        );
        assert!(
            said.contains("Nothing in that tree was moved or removed"),
            "a user must not be left wondering whether dl deleted it: {said}"
        );
    }

    #[test]
    fn a_config_naming_nothing_retired_says_nothing() {
        assert!(retired_keys(&[]).is_empty());
    }

    // ------------------------------------------------------------- --purge

    #[test]
    fn the_cleanup_page_quotes_what_a_purge_really_prints() {
        // `docs/cleanup.md` reproduces the block `--purge` prints above its
        // question, which makes the page a second hand-maintained copy of it --
        // and a sample output that has drifted from the command is worse than no
        // sample. This is the diff test the standing rule asks for beside such a
        // copy. If that section moves to another page, this path moves with it,
        // in the same change.
        //
        // The survivor line is in here as well as the two sentences, because the
        // line is what this change is about: the sample would go on reading
        // `- pythontemplate` on its own if the renderer's format ever went back to
        // an id, and nothing else would notice.
        let page = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/cleanup.md"),
        )
        .expect("docs/cleanup.md");
        let quoted = [
            SURVIVORS_KEEP_WORKING.to_owned(),
            stranded_clones(Confirmation::WillBeAsked).to_owned(),
            leaving_line("pythontemplate", "https://github.com/blooop/pythontemplate"),
            leaving_line("my-hand-made-workspace", "/home/you/projects/thing"),
        ];
        for said in quoted {
            assert!(
                page.contains(&said),
                "docs/cleanup.md no longer quotes what the purge says: {said}"
            );
        }
        // And the `-y` spelling is deliberately *not* quoted there: the page
        // describes it in prose instead, so there is no second copy of it to
        // drift.
        assert!(
            !page.contains(stranded_clones(Confirmation::AnsweredOnTheLine)),
            "the page grew a copy of the -y sentence; guard it here or take it out"
        );
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

    /// DEL, the one non-printable ASCII character serde hands to the fragment
    /// writer rather than escaping itself.
    ///
    /// `--ls --json` is a wire format `wf` parses, so when this document was
    /// spelled by a formatter of its own it carried the same divergence core's
    /// did, and it was closed in both at once (devlaunch#349). The assertion has
    /// not moved since; what has moved is what stands behind it, and that is the
    /// point of leaving it exactly as it was. Expectation from `json.dumps`.
    #[test]
    fn del_is_escaped_as_python_escapes_it() {
        assert_eq!(
            python_json_document(&serde_json::json!("a\u{7f}b")),
            r#""a\u007fb""#
        );
    }

    /// The whole ASCII range at once, against the line Python wrote for it.
    ///
    /// This sweep went in while `dl` still had an escaping loop of its own, to
    /// hold it character-for-character equal to core's: nothing bare that
    /// `json.dumps` escapes, nothing escaped that it leaves bare. The loop is gone
    /// (devlaunch#346) and the sweep is unchanged, which turns it from a
    /// cross-check between two copies into the evidence that retiring one of them
    /// moved no byte of a document `wf` parses. Expectation is the literal
    /// `json.dumps` printed for `''.join(chr(c) for c in range(0x80))`.
    #[test]
    fn every_ascii_character_is_spelled_the_way_python_spells_it() {
        let all_of_ascii: String = (0u8..=0x7f).map(char::from).collect();
        assert_eq!(
            python_json_document(&serde_json::json!(all_of_ascii)),
            r##""\u0000\u0001\u0002\u0003\u0004\u0005\u0006\u0007\b\t\n\u000b\f\r\u000e\u000f\u0010\u0011\u0012\u0013\u0014\u0015\u0016\u0017\u0018\u0019\u001a\u001b\u001c\u001d\u001e\u001f !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\u007f""##
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
    fn an_id_collision_names_both_branches_the_id_and_the_way_out() {
        // The only action available to the reader is to rename one of the two
        // branches, so the sentence has to hand them both of them: the two rows are
        // the same 47 characters in `dl --ls`, and a message that named one side
        // would leave them hunting for the other.
        let line = launch_refusal(&LaunchRefusal::IdCollision {
            workspace_id: "devlaunch-release-999999999999999999999911-dq8q".to_owned(),
            owner: "blooop".to_owned(),
            repo: "devlaunch".to_owned(),
            branch: "release/999999999999999999999911783".to_owned(),
            recorded_owner: "blooop".to_owned(),
            recorded_repo: "devlaunch".to_owned(),
            recorded_branch: "release/999999999999999999999911630".to_owned(),
        })
        .expect("a refusal dl has to say itself");

        assert!(
            line.contains("'devlaunch-release-999999999999999999999911-dq8q'"),
            "{line}"
        );
        assert!(
            line.contains("'blooop/devlaunch@release/999999999999999999999911783'"),
            "{line}"
        );
        assert!(
            line.contains("'blooop/devlaunch@release/999999999999999999999911630'"),
            "{line}"
        );
        assert!(line.contains("Rename one of the two branches"), "{line}");
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
    fn the_herdr_tab_rename_is_not_a_line_either() {
        // It is a command to run, not a sentence to print. A line here would put
        // `herdr tab rename ...` into every collected report and into the tests
        // that read a launch's words, and would say it whether or not it worked --
        // which is the one thing this stage promises not to claim.
        assert_eq!(
            launch_notice(&LaunchNotice::HerdrTab(HerdrTabRename::Run {
                bin: "herdr".to_owned(),
                tab_id: "w8:tB".to_owned(),
                label: "devlaunch@nb3".to_owned(),
            })),
            None
        );
        assert_eq!(
            launch_notice(&LaunchNotice::HerdrTab(HerdrTabRename::Off)),
            None
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

    /// The three sentences devlaunch#421 split `NoAlias` into, rendered.
    fn the_three_pty_notices() -> (String, String, String) {
        let path = std::path::PathBuf::from("/scratch/ssh_config");
        (
            launch_notice(&LaunchNotice::NoTerminalAlias {
                workspace_id: "myws".to_owned(),
                config: path.clone(),
            })
            .expect("a sentence"),
            launch_notice(&LaunchNotice::NoDevpodSshConfig {
                workspace_id: "myws".to_owned(),
                looked_in: path,
            })
            .expect("a sentence"),
            launch_notice(&LaunchNotice::SshConfigUnlocatable).expect("a sentence"),
        )
    }

    #[test]
    fn each_way_of_losing_the_pty_transport_names_the_file_dl_read() {
        // Each sentence has to say which of the three happened, and every one of
        // them names a path or says why there is none.
        let (alias_absent, no_config, nowhere) = the_three_pty_notices();

        assert!(
            alias_absent.contains("/scratch/ssh_config"),
            "{alias_absent}"
        );
        assert!(no_config.contains("/scratch/ssh_config"), "{no_config}");
        assert_ne!(
            alias_absent, no_config,
            "one sentence for both is what let the bug ship"
        );
        assert!(nowhere.contains("DEVPOD_SSH_CONFIG"), "{nowhere}");
        for line in [&alias_absent, &no_config, &nowhere] {
            assert!(line.contains("no terminal"), "{line}");
        }
    }

    #[test]
    fn the_advice_each_pty_notice_gives_is_the_advice_that_arm_can_honour() {
        // The reason the split is worth a type is that the *advice* differs, so
        // "the three strings differ" is not the assertion the split needs -- three
        // wrong-but-different sentences pass it, and that is how a
        // `NoDevpodSshConfig` reading "`dl myws restart` writes it" got past a
        // green test while the enum doc, docs/cli.md and the CHANGELOG all said a
        // restart is not the fix for this arm. So the claim is pinned instead of
        // its distinctness.
        let (alias_absent, no_config, nowhere) = the_three_pty_notices();

        // Read the config, alias missing from it: a restart puts it back, flatly.
        assert!(
            alias_absent.contains("`dl myws restart` republishes it"),
            "{alias_absent}"
        );

        // Nothing readable where dl looked: a restart writes *that file* only if
        // that is where devpod publishes, so the advice is offered conditionally
        // and names what to check when it is not. Nothing here may promise a
        // restart outright.
        assert!(
            no_config
                .contains("`dl myws restart` publishes there if that is the file devpod writes"),
            "{no_config}"
        );
        assert!(
            no_config.contains("DEVPOD_SSH_CONFIG"),
            "the arm that may be dl reading the wrong file has to name the variable that decides it: {no_config}"
        );
        assert!(
            !no_config.contains("restart` writes it"),
            "an unconditional restart is the claim three other places in this repo deny: {no_config}"
        );

        // Nowhere to look at all: there is no workspace-level fix, so no restart
        // is offered.
        assert!(
            !nowhere.contains("restart"),
            "a machine with no home directory has nothing a restart would change: {nowhere}"
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
    fn a_pack_the_sweep_could_not_do_names_the_repository_and_gits_own_words() {
        // The sweep walks every repository in one detached process, so a line that
        // did not name one would be unactionable. It is a notice and not an error
        // because the fetch beside it succeeded: the cost of the refusal is a
        // filesystem block per ref until the next sweep, and nothing else.
        let said = |notice: CacheNotice| cache_notice(&notice).expect("a line");

        assert_eq!(
            said(CacheNotice::RefsNotPacked {
                owner: "blooop".to_owned(),
                repo: "devlaunch".to_owned(),
                reason: "fatal: unable to create 'packed-refs.lock': Permission denied".to_owned(),
            }),
            "Could not pack the refs of blooop/devlaunch: fatal: unable to create \
             'packed-refs.lock': Permission denied"
        );
    }

    #[test]
    fn the_last_sweeps_note_reads_as_one_line_naming_the_repository() {
        // The same refusal as the notice above, a run later. The notice went to the
        // detached child's null stderr; this is what somebody actually reads, so it
        // has to name the repository on its own — nothing around it does.
        use devlaunch_core::domain::model::SweepNote;

        let note = |trouble, said: Option<&str>| SweptRepoNote {
            owner: "blooop".to_owned(),
            repo: "devlaunch".to_owned(),
            note: SweepNote {
                trouble,
                said: said.map(str::to_owned),
            },
        };

        assert_eq!(
            sweep_notes(&[note(
                SweepTrouble::RefsNotPacked,
                Some("fatal: unable to create 'packed-refs.lock': Permission denied"),
            )]),
            [
                "Last cache sweep of blooop/devlaunch: could not pack the refs it fetched: \
                 fatal: unable to create 'packed-refs.lock': Permission denied"
            ]
        );
        // Nothing spoke, so nothing is quoted: a line ending in a bare colon would
        // read as a refusal whose words were lost.
        assert_eq!(
            sweep_notes(&[note(SweepTrouble::FetchTimedOut, None)]),
            ["Last cache sweep of blooop/devlaunch: ran out of time fetching"]
        );
        assert!(sweep_notes(&[]).is_empty(), "a clean cache says nothing");
    }

    #[test]
    fn every_sweep_trouble_has_words_of_its_own() {
        // Five arms, five sentences, and no two the same: which condition it was is
        // the whole of what the record carries when git said nothing.
        let words = [
            SweepTrouble::RefsNotPacked,
            SweepTrouble::FetchRefused,
            SweepTrouble::FetchTimedOut,
            SweepTrouble::CloneMissing,
            SweepTrouble::NotRecorded,
        ]
        .map(sweep_trouble);
        let mut sorted = words.to_vec();
        sorted.sort_unstable();
        sorted.dedup();

        assert_eq!(sorted.len(), words.len(), "{words:?}");
        assert!(words.iter().all(|line| !line.is_empty()));
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

    /// The delete's own two lines, which are the only ones a workspace with no
    /// clone recorded under it has: the clone notices beside them do not fire, and
    /// what named it before was `devpod delete`'s stdout and nothing else.
    #[test]
    fn a_delete_names_the_workspace_going_in_and_names_it_again_once_it_is_gone() {
        assert_eq!(
            removing("devlaunch-main-3j1t"),
            "Removing workspace devlaunch-main-3j1t..."
        );
        assert_eq!(
            removed("devlaunch-main-3j1t", Insistence::NotInsisted),
            "Removed workspace devlaunch-main-3j1t."
        );
        // `--force` carries devpod's `--ignore-not-found`, so a success establishes
        // absence and not a removal: there is nothing in the answer to tell a real
        // delete from a workspace that was never there, and a path spec never asked
        // devpod either. The words say only what was established.
        assert_eq!(
            removed("devlaunch-main-3j1t", Insistence::Insisted),
            "Workspace devlaunch-main-3j1t is gone."
        );
    }

    /// The words a pick is reported in. That a real pick reaches them at all is
    /// `tests/picker.rs`'s, which drives the binary on a pty; this is the wording,
    /// including the two rows an id cannot tell apart, which no fake devpod listing
    /// has to offer for the question to be settled.
    ///
    /// **The row is on the line beside the id, and that is the whole point of the
    /// line.** An id is `<repo-slug>-<ref-slug>-<suffix>`
    /// ([`devlaunch_core::domain::workspace_id`]) and the *owner is not in it* —
    /// which is exactly the column the picker draws first and the one a fork is told
    /// from its upstream by. A receipt naming the id alone leaves
    /// `blooop | devlaunch | main` and `myfork | devlaunch | main` reporting the
    /// same words with four characters of hash between them, and the hash was
    /// deliberately never on screen while the row was being chosen.
    #[test]
    fn a_pick_names_the_row_that_was_chosen_beside_the_id_it_resolved_to() {
        /// The picks as the picker hands them over: `(row, id)` written in that
        /// order, and named on the way into `Chosen` so a pair written backwards is
        /// a compile error here rather than a receipt with its halves swapped.
        fn taken(picks: &[(&str, &str)]) -> NonEmpty<Chosen> {
            NonEmpty::of(picks.iter().map(|(row, workspace_id)| Chosen {
                workspace_id: (*workspace_id).to_owned(),
                row: (*row).to_owned(),
                // `picked` renders the row beside the id and reads nothing else.
                triple: None,
            }))
            .expect("at least one pick")
        }

        assert_eq!(
            picked(
                "rm",
                &taken(&[("blooop | devlaunch | main", "devlaunch-main-3j1t")])
            ),
            ["Picked blooop | devlaunch | main -> devlaunch-main-3j1t"]
        );
        // The two rows an id cannot tell apart, which is the case the row column is
        // carried for: same repo, same ref, different owner, and the ids differ only
        // in the hash.
        assert_eq!(
            picked(
                "rm",
                &taken(&[
                    ("blooop | devlaunch | main", "devlaunch-main-3j1t"),
                    ("myfork | devlaunch | main", "devlaunch-main-7q0x"),
                    ("- | someones-project", "someones-project"),
                ])
            ),
            [
                "Picked 3 workspaces for rm:",
                "  blooop | devlaunch | main -> devlaunch-main-3j1t",
                "  myfork | devlaunch | main -> devlaunch-main-7q0x",
                "  - | someones-project -> someones-project",
            ]
        );
        // The order is the picker's, which is the order the rows were marked in and
        // the order the batch is applied in. Re-sorting the heading would describe a
        // run that did not happen.
        assert_eq!(
            picked("stop", &taken(&[("- | b", "b"), ("- | a", "a")])),
            [
                "Picked 2 workspaces for stop:",
                "  - | b -> b",
                "  - | a -> a",
            ]
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
                occasion: SweepOccasion::DevpodResult,
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
                occasion: SweepOccasion::DevpodResult,
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
                occasion: SweepOccasion::DevpodResult,
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
                occasion: SweepOccasion::DevpodResult,
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

    // ---------------------------------------------------------- dl <ws> kill

    fn held_by(pid: u32, command: &str) -> HostProcess {
        HostProcess {
            pid,
            parent: 1,
            command: command.to_owned(),
        }
    }

    fn nothing_swept() -> Sweep {
        Sweep {
            signalled: Vec::new(),
            marker: Marker::Absent,
            containers: Containers::NoneRunning,
            holding: Holding::Free,
        }
    }

    /// The whole point of the verb, printed. A hammer that swings silently is
    /// worse than the hang it was reached for: what was killed is the only
    /// evidence a person has that the workspace is free now.
    #[test]
    fn a_sweep_names_every_process_it_killed_and_how_far_it_had_to_go() {
        let sweep = Sweep {
            signalled: vec![
                Signalled {
                    process: held_by(732_721, "devpod up my-ws --ide none"),
                    ending: Ending::Terminated,
                },
                Signalled {
                    process: held_by(732_722, "devpod ssh my-ws"),
                    ending: Ending::Killed,
                },
            ],
            ..nothing_swept()
        };

        assert_eq!(
            killed("my-ws", &sweep),
            [
                "  732721 gone (SIGTERM): devpod up my-ws --ide none",
                "  732722 gone (SIGKILL): devpod ssh my-ws",
                "Workspace my-ws is no longer held.",
            ]
        );
    }

    /// Nothing found is a finding: the hang is somewhere this verb does not
    /// reach, and the line has to say so rather than print an empty report.
    #[test]
    fn a_sweep_that_found_nothing_says_so_rather_than_printing_nothing() {
        assert_eq!(
            killed("my-ws", &nothing_swept()),
            ["Nothing on this host is holding workspace my-ws."]
        );
    }

    /// Every kind of holder leaves the workspace held, and the closing line says so
    /// for all of them. What each one means for the delete underneath differs, and
    /// that is not this line's to say: [`Standing`] is where it is said.
    ///
    /// Every kind of holder at once, and each with its own phrasing, because the
    /// three ask the reader for three different things: wait for the build, ignore
    /// the session, go and look at the orphan. The survivor appears once, in the
    /// signal line that says how far the escalation got, rather than a second time
    /// as the orphan it also is.
    #[test]
    fn a_workspace_something_still_holds_is_reported_held() {
        let survivor = held_by(732_721, "devpod up my-ws");
        let sweep = Sweep {
            signalled: vec![Signalled {
                process: survivor.clone(),
                ending: Ending::Survived,
            }],
            marker: Marker::LeftForALiveHolder,
            containers: Containers::LeftForALiveBuild,
            holding: Holding::StillHeld {
                holders: vec![
                    Standing::AnOrphan(survivor),
                    Standing::ABuild(HostProcess {
                        pid: 5001,
                        parent: 5000,
                        command: "devpod up my-ws".to_owned(),
                    }),
                    Standing::ASession(HostProcess {
                        pid: 6001,
                        parent: 6000,
                        command: "devpod ssh my-ws".to_owned(),
                    }),
                    Standing::AnOrphan(HostProcess {
                        pid: 7001,
                        parent: 1,
                        command: "devpod helper my-ws".to_owned(),
                    }),
                ],
            },
        };

        assert_eq!(
            killed("my-ws", &sweep),
            [
                "  732721 still running after SIGKILL: devpod up my-ws",
                "  5001 left alone, this is a live build: devpod up my-ws",
                "  6001 left alone, something is still watching it: devpod ssh my-ws",
                "  7001 still holding it, and nothing is waiting on it: devpod helper my-ws",
                "Left devpod's busy marker alone: something still holds this workspace.",
                "Left this workspace's containers alone: they belong to that build.",
                "Workspace my-ws is still held.",
            ]
        );
    }

    /// The delete the verb ends in, withheld, and it has to say why the thing that
    /// stopped it would not have been fixed by trying: a `devpod delete` over a
    /// lock it cannot take blocks rather than fails, so a reader who sees no delete
    /// and no reason would reasonably just run one.
    ///
    /// And what it sends them back to is `kill`, asserted rather than left to
    /// wording, because `rm` is the one thing it must not name: `rm`'s delete has
    /// no deadline, so on a workspace whose lock is still held it is the unbounded
    /// wait this verb exists to avoid.
    #[test]
    fn a_withheld_delete_sends_the_reader_back_to_kill_and_never_to_rm() {
        let said = kill_delete_withheld("my-ws");

        assert_eq!(
            said,
            "Not deleting my-ws: it is still held, and devpod's delete would wait on the lock \
             with no deadline behind it. Deal with what is named above, then run 'dl my-ws kill' \
             again."
        );
        assert!(!said.contains("rm"), "the reader was sent to rm: {said}");
    }

    /// A `devpod delete` that refused is the one failure with a hammer left to
    /// reach for, so it names it. The wedge and the devcontainer.json that moved
    /// look identical from out here, which is why both ways out are offered rather
    /// than one of them guessed at.
    #[test]
    fn a_refused_delete_offers_the_kill_that_would_clear_a_wedge() {
        assert_eq!(
            delete_refused("my-ws", Swept::NotYet),
            "devpod could not delete my-ws; keeping the local clone so it stays retryable. If its \
             devcontainer.json moved, restore the path or run: devpod delete my-ws --force. If \
             something on this host is holding it instead, run: dl my-ws kill"
        );
    }

    /// And not when the sweep is already on screen above it, which is `kill`'s own
    /// delete: the advice would be to re-run the command being read.
    #[test]
    fn a_refused_delete_that_a_sweep_already_preceded_offers_no_kill() {
        assert!(!delete_refused("my-ws", Swept::Already).contains("kill"));
    }

    /// The deadline firing, said as the state it left rather than as the timeout,
    /// which the line above it already gives. The clone being named is the half a
    /// reader most needs: a delete that stopped in the middle is alarming exactly
    /// until you know nothing on disk went with it.
    #[test]
    fn a_delete_that_ran_out_of_time_says_what_is_still_there() {
        assert_eq!(
            delete_timed_out("my-ws"),
            "my-ws and its clone are still here, and devpod may have got part of the way through. \
             Run 'dl my-ws kill' again to pick up where it stopped."
        );
    }

    /// The stale file the issue is really about, and the path, because somebody
    /// who has been deleting it by hand deserves to see which one dl took.
    #[test]
    fn a_removed_busy_marker_is_named_by_path() {
        let sweep = Sweep {
            marker: Marker::Removed(PathBuf::from("/home/x/.devpod/agent/w/workspace.lock")),
            containers: Containers::Killed(vec!["abc123".to_owned()]),
            ..nothing_swept()
        };

        assert_eq!(
            killed("my-ws", &sweep),
            [
                "Removed devpod's stale busy marker: /home/x/.devpod/agent/w/workspace.lock",
                "Killed 1 container: abc123",
                "Nothing on this host is holding workspace my-ws.",
            ]
        );
    }

    /// A host this verb cannot work on says which tool it is missing, rather
    /// than reporting a sweep that found nothing.
    #[test]
    fn a_host_this_verb_cannot_work_on_names_what_it_is_missing() {
        assert_eq!(
            kill_unavailable(&HostCannot::ReadItsProcessTable(TableUnreadable::NoPs)),
            "error: `dl <workspace> kill` reads the host's process table with `ps`, and this \
             host has none"
        );
        assert_eq!(
            kill_unavailable(&HostCannot::SendASignal(NoSignal::NoKillHere)),
            "error: something is holding this workspace and this host has no `kill` to signal \
             it with"
        );
    }

    /// The three ways a tool can be there and still leave the verb with nothing
    /// to work on. Each is a different sentence, because a `ps` that refused, a
    /// `ps` that could not be started and a `kill` the OS would not run send the
    /// reader to three different places — and none of them to "install `ps`".
    #[test]
    fn a_tool_that_is_there_and_would_not_answer_is_not_a_tool_that_is_missing() {
        assert_eq!(
            kill_unavailable(&HostCannot::ReadItsProcessTable(TableUnreadable::Refused {
                exit: Exit::Code(1),
                stderr: "ps: unsupported option\n".to_owned(),
            })),
            "error: `ps` would not read this host's process table: ps: unsupported option"
        );
        assert_eq!(
            kill_unavailable(&HostCannot::ReadItsProcessTable(
                TableUnreadable::NotStarted(OsFailure {
                    kind: std::io::ErrorKind::TimedOut,
                    errno: None,
                })
            )),
            "error: `ps` could not be run (TimedOut)"
        );
        assert_eq!(
            kill_unavailable(&HostCannot::SendASignal(NoSignal::NotRun(OsFailure {
                kind: std::io::ErrorKind::PermissionDenied,
                errno: None,
            }))),
            "error: something is holding this workspace and `kill` could not be run \
             (PermissionDenied)"
        );
    }

    /// A `ps` that refused without writing anything still has to say something,
    /// and the exit status is what is left. The fallback exists because a
    /// half-empty sentence reads as a bug in dl rather than a fact about the host.
    #[test]
    fn a_refusal_with_nothing_written_falls_back_to_the_exit_status() {
        assert_eq!(
            kill_unavailable(&HostCannot::ReadItsProcessTable(TableUnreadable::Refused {
                exit: Exit::Code(2),
                stderr: String::new(),
            })),
            "error: `ps` would not read this host's process table: it exited 2"
        );
    }

    /// The marker that would not go, and the containers that would not die: both
    /// are lines rather than refusals, because the rest of the sweep happened and
    /// the person needs to know which part of it did not.
    #[test]
    fn the_two_things_that_would_not_move_are_named_with_the_reason() {
        let sweep = Sweep {
            marker: Marker::Unremovable {
                path: PathBuf::from("/home/x/.devpod/agent/w/workspace.lock"),
                reason: "permission denied".to_owned(),
            },
            containers: Containers::Standing(ContainerRefusal::Refused {
                exit: Exit::Code(1),
                stderr: "Error response from daemon: no such container\n".to_owned(),
            }),
            ..nothing_swept()
        };

        assert_eq!(
            killed("my-ws", &sweep),
            [
                "Could not remove devpod's busy marker at \
                 /home/x/.devpod/agent/w/workspace.lock (permission denied)",
                "Could not kill this workspace's containers: Error response from daemon: no such \
                 container",
                "Nothing on this host is holding workspace my-ws.",
            ]
        );
    }

    /// docker's two other ways of not delivering: one that refused silently, and
    /// one that never ran. The first falls back to the exit status for the reason
    /// `ps`'s does; the second is not a docker that refused and must not read as
    /// one.
    #[test]
    fn a_docker_that_said_nothing_and_a_docker_that_never_ran_still_say_something() {
        let silent = Sweep {
            containers: Containers::Standing(ContainerRefusal::Refused {
                exit: Exit::Code(125),
                stderr: String::new(),
            }),
            ..nothing_swept()
        };
        assert_eq!(
            killed("my-ws", &silent)[0],
            "Could not kill this workspace's containers: docker exited 125"
        );

        let never_ran = Sweep {
            containers: Containers::Standing(ContainerRefusal::NotRun {
                failure: OsFailure {
                    kind: std::io::ErrorKind::TimedOut,
                    errno: None,
                },
            }),
            ..nothing_swept()
        };
        assert_eq!(
            killed("my-ws", &never_ran)[0],
            "Could not kill this workspace's containers: could not run docker (TimedOut)"
        );
    }
}
