//! One `render_*` per [`Command`] arm, and the exhaustive match that picks it.
//!
//! Adding a command to [`Command`] breaks [`dispatch`] until it is handled, which
//! is the whole reason the grammar is a sum rather than a bag of flags. The arms
//! whose flows land in a later milestone are still arms here: each one has its own
//! `render_*` with the milestone named, so wiring it up next wave replaces a body
//! rather than growing a dispatcher.

use std::path::Path;

use devlaunch_core::clients::devpod::ListingUnreadable;
use devlaunch_core::flows::completion::{self, FileState, InstallError, Installed, RcChange};
use devlaunch_core::flows::completion_cache::{self, Refreshed};
use devlaunch_core::flows::listing::{self, CommandContext, DlView, Sizes};
use devlaunch_core::runner::Runner;

use crate::cli::{Command, ListOutput, Verb};
use crate::render;
use crate::session::{self, Records, StartupError};

/// How a command ended, as an exit code.
///
/// Three arms, which is every ending the read-side commands have. The lifecycle
/// and launch verbs pass a child process's own status back to the shell, so M6/M7
/// add an arm for it — and that arm cannot be forgotten, because every `match` on
/// this type has to answer for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ending {
    /// It worked.
    Done,
    /// dl asked, and the answer was no. Python's `return 1`.
    Refused,
    /// devpod is not on PATH. The shell's own "command not found" code, which
    /// says more than a bare 1 and cannot be confused with a devpod command that
    /// ran and failed.
    DevpodMissing,
}

impl Ending {
    pub(crate) fn code(self) -> i32 {
        match self {
            Ending::Done => 0,
            Ending::Refused => 1,
            Ending::DevpodMissing => 127,
        }
    }
}

/// Run one command and say how it ended.
pub(crate) fn dispatch(runner: &dyn Runner, command: Command) -> Ending {
    let mut context = CommandContext::new(runner);
    match command {
        Command::Version => render_version(),
        Command::List { output, sizes } => render_list(runner, &mut context, output, sizes),
        Command::Repos => render_repos(&mut context),
        Command::CompletionData => render_completion_data(&mut context),
        Command::UpdateCache { force } => render_update_cache(&mut context, force),
        Command::Refresh => render_refresh(&mut context),
        Command::Install { rc } => render_install(&mut context, rc.as_deref()),
        Command::Prune { yes, force } => render_prune(yes, force),
        Command::Reconcile { yes } => render_reconcile(yes),
        Command::Purge { yes } => render_purge(yes),
        Command::Select { verb, .. } => render_select(verb),
        Command::Workspace { verb, .. } => render_workspace(verb),
    }
}

// ---------------------------------------------------------------------------
// --version
// ---------------------------------------------------------------------------

/// `dl <version>`, as Python prints it.
///
/// Python appended the install's provenance when it was notable — an editable pip
/// install names the tree it resolves to, which is how `dl` and `dl-next` are told
/// apart by output. A compiled binary has no PEP 610 metadata and no editable
/// install to describe, so there is nothing to append and the bare version is the
/// whole answer.
fn render_version() -> Ending {
    println!("dl {}", env!("CARGO_PKG_VERSION"));
    Ending::Done
}

// ---------------------------------------------------------------------------
// --ls
// ---------------------------------------------------------------------------

fn render_list(
    runner: &dyn Runner,
    context: &mut CommandContext<'_>,
    output: ListOutput,
    sizes: Sizes,
) -> Ending {
    match output {
        ListOutput::Table => render_table(context, sizes),
        ListOutput::Json => render_json(runner, context, sizes),
    }
}

/// The human table. Reads devpod and nothing else — no config, no records, no
/// migration, which is what keeps a listing one round trip.
fn render_table(context: &mut CommandContext<'_>, sizes: Sizes) -> Ending {
    let Some(cache) = ok_or_refuse(session::cache_dir()) else {
        return Ending::Refused;
    };
    match listing::workspace_table(context, &cache, sizes) {
        Err(refused) => refuse_listing(&refused),
        Ok(table) => {
            for line in render::table_lines(&table, sizes) {
                println!("{line}");
            }
            Ending::Done
        }
    }
}

/// The `--ls --json` document.
///
/// Grade A: `wf` parses this, so the field names, the key order and which fields
/// are null where are a contract. The document is core's; the two-space
/// indentation and the `ensure_ascii` escaping are the rendering.
fn render_json(runner: &dyn Runner, context: &mut CommandContext<'_>, sizes: Sizes) -> Ending {
    let Some(cache) = ok_or_refuse(session::cache_dir()) else {
        return Ending::Refused;
    };
    let records = match session::open_records(runner) {
        Err(refused) => return refuse_startup(&refused),
        Ok(records) => records,
    };
    report(&records);
    let view = DlView {
        cache_dir: &cache,
        storage: &records.storage,
        clones: &records.clones,
    };
    match listing::enriched_listing(context, &view, sizes) {
        Err(refused) => refuse_listing(&refused),
        Ok(rows) => {
            println!(
                "{}",
                render::python_json_document(&listing::json_document(&rows))
            );
            Ending::Done
        }
    }
}

// ---------------------------------------------------------------------------
// the completion commands
// ---------------------------------------------------------------------------

/// The known `owner/repo` strings, one per line.
///
/// Reads the completion cache when there is one and asks devpod when there is not.
/// **Divergence row 11 lands here:** a cache document with no `repos` key reads as
/// a cache with no repos, where Python's key check fell through to asking devpod.
/// The distinguishing input is a `completions.json` written by something other
/// than dl; every cache dl writes carries all four keys.
fn render_repos(context: &mut CommandContext<'_>) -> Ending {
    let Some(cache) = ok_or_refuse(session::cache_dir()) else {
        return Ending::Refused;
    };
    if let Some(cached) =
        completion_cache::read_completion_cache(&completion_cache::cache_path(&cache))
    {
        for repo in &cached.repos {
            println!("{repo}");
        }
        return Ending::Done;
    }
    // No cache: discover from the workspaces devpod lists. Composed here rather
    // than taken from `listing::known_repos` so the workspaces whose source dl
    // could not read are named — the skip is stated, and the sentence stating it
    // is the binary's.
    let workspaces = match context.workspaces() {
        Err(refused) => return refuse_listing(&refused),
        Ok(workspaces) => workspaces,
    };
    let git = context.git();
    let found = listing::discover_repos_from_workspaces(&git, &workspaces);
    for skipped in &found.unreadable {
        eprintln!(
            "Not looking for a repo in workspace '{}': devpod describes its source as {}, \
             which devlaunch cannot read.",
            skipped.workspace_id, skipped.payload
        );
    }
    for repo in listing::flatten_repos(&found.repos) {
        println!("{repo}");
    }
    Ending::Done
}

/// The whole completion cache as one JSON line, refreshing it if there is none.
///
/// **Divergence row 11 again:** the four known keys are re-serialized, so a newer
/// writer's extra keys are dropped rather than echoed.
fn render_completion_data(context: &mut CommandContext<'_>) -> Ending {
    let Some(cache) = ok_or_refuse(session::cache_dir()) else {
        return Ending::Refused;
    };
    if let Some(cached) =
        completion_cache::read_completion_cache(&completion_cache::cache_path(&cache))
    {
        println!("{}", cached.as_json_line());
        return Ending::Done;
    }
    let refreshed = match refresh_cache(context, &cache) {
        Err(ending) => return ending,
        Ok(refreshed) => refreshed,
    };
    println!("{}", refreshed.data.as_json_line());
    Ending::Done
}

/// The silent refresh a background child runs.
///
/// The TTL is re-checked here as well as in the parent that spawned it: two
/// parents can both see a stale cache before either child has written one, and the
/// second sweep would be pure waste. `--force` marks a refresh that follows a
/// workspace change, where the cache is wrong however new it is.
fn render_update_cache(context: &mut CommandContext<'_>, force: bool) -> Ending {
    let Some(cache) = ok_or_refuse(session::cache_dir()) else {
        return Ending::Refused;
    };
    if !force && completion_cache::completion_cache_is_fresh(&completion_cache::cache_path(&cache))
    {
        return Ending::Done;
    }
    if let Err(ending) = refresh_cache(context, &cache) {
        return ending;
    }
    // TODO(M6): `sweep_repo_fetches` runs here — completions first, freshness
    // second: the cache is what the user's next keystroke reads, while the fetch
    // sweep is for the launch after that. Both are on the same hour, so a child
    // that gets this far does both or, when it exits early above, neither.
    Ending::Done
}

/// The same refresh, with the two lines a person asked for.
fn render_refresh(context: &mut CommandContext<'_>) -> Ending {
    let Some(cache) = ok_or_refuse(session::cache_dir()) else {
        return Ending::Refused;
    };
    println!("Refreshing completion cache...");
    let refreshed = match refresh_cache(context, &cache) {
        Err(ending) => return ending,
        Ok(refreshed) => refreshed,
    };
    println!(
        "Cache updated: {} workspaces found",
        refreshed.data.workspaces.len()
    );
    Ending::Done
}

/// Rebuild the cache, or say why the command cannot go on.
///
/// The one refusal that stops it is a devpod that is not installed. Every other
/// unreadable listing costs the refresh its workspace *names* and nothing else —
/// the repos, owners and branches come off the local disk — so it is reported and
/// the cache is written from what could still be seen. Refusing there would mean
/// an unreachable devpod stops `dl --install` from installing completions at all.
fn refresh_cache(context: &mut CommandContext<'_>, cache: &Path) -> Result<Refreshed, Ending> {
    let config = match session::worktree_config() {
        Err(refused) => return Err(refuse_startup(&StartupError::Config(refused))),
        Ok(config) => config,
    };
    // Asked before the refresh rather than after, because a missing devpod has to
    // stop this before anything is *written*: in Python the missing-binary refusal
    // travels out of the refresh's very first `devpod list` and past every local
    // handler, so a machine with no devpod ends with no completion cache rather
    // than an empty one. Asking here is free on the path that works — the
    // command's snapshot answers the refresh's own read — and costs one extra
    // refused call on the path that does not.
    if let Err(refused) = context.workspaces()
        && render::is_devpod_missing(&refused)
    {
        return Err(refuse_listing(&refused));
    }
    let refreshed = completion_cache::update_completion_cache(context, cache, &config.repos_dir);
    if let Some(refused) = &refreshed.listing_refused {
        // Every other unreadable listing is reported and stepped over: see the
        // note above the signature.
        eprintln!("Completing without workspace names: {}", short(refused));
    }
    for skipped in &refreshed.unreadable_sources {
        eprintln!(
            "Not looking for a repo in workspace '{}': devpod describes its source as {}, \
             which devlaunch cannot read.",
            skipped.workspace_id, skipped.payload
        );
    }
    // `not_written` is deliberately silent, as Python's `except OSError: pass` is:
    // a completion cache that could not be written is not a reason to say anything
    // about the command that was warming it. The events are there for a caller
    // that decides otherwise.
    Ok(refreshed)
}

// ---------------------------------------------------------------------------
// --install
// ---------------------------------------------------------------------------

/// Install or refresh the shell completions.
///
/// The cache is rebuilt first so completions work on the very next keystroke,
/// which is what Python does and the reason `--install` reads the workspace list
/// at all.
///
/// **Divergence row 10 lands here:** a re-run over an already-current install
/// rewrites nothing and says so. Python rewrote byte-identical files and touched
/// the rc file's mtime on every run, so it could only ever report an update.
fn render_install(context: &mut CommandContext<'_>, rc: Option<&Path>) -> Ending {
    let Some(cache) = ok_or_refuse(session::cache_dir()) else {
        return Ending::Refused;
    };
    if let Err(ending) = refresh_cache(context, &cache) {
        return ending;
    }
    match completion::install(rc) {
        Err(refused) => {
            eprintln!(
                "Failed to install completions: {}",
                install_failure(&refused)
            );
            Ending::Refused
        }
        Ok(installed) => {
            report_install(&installed);
            Ending::Done
        }
    }
}

fn report_install(installed: &Installed) {
    let script = installed.script.display();
    let rc = installed.rc.display();
    match installed.script_state {
        FileState::Written => eprintln!("Wrote completion script to {script}"),
        FileState::AlreadyCurrent => {
            eprintln!("Completion script at {script} is already current");
        }
    }
    match installed.rc_change {
        RcChange::Added => eprintln!("Added completion source block to {rc}"),
        RcChange::Refreshed => eprintln!("Refreshed the completion source block in {rc}"),
        RcChange::AlreadyInstalled => eprintln!("{rc} already sources it"),
    }
    eprintln!("Run 'source {rc}' or restart your terminal to enable completion");
    let untouched = matches!(installed.script_state, FileState::AlreadyCurrent)
        && matches!(installed.rc_change, RcChange::AlreadyInstalled);
    if untouched {
        println!(
            "[devlaunch] Autocomplete is already installed and current. Run 'source {rc}' or \
             restart your terminal if completion is not working yet."
        );
    } else {
        println!(
            "[devlaunch] Autocomplete has been updated. Run 'source {rc}' or restart your \
             terminal to enable completion."
        );
    }
}

fn install_failure(error: &InstallError) -> String {
    match error {
        InstallError::NoHomeDirectory => {
            "this machine names no home directory, so there is nowhere to install to".to_owned()
        }
        InstallError::CreateDirectory { path, source } => {
            format!("could not create {} ({source})", path.display())
        }
        InstallError::WriteScript { path, source } => {
            format!("could not write {} ({source})", path.display())
        }
        InstallError::ReadRc { path, source } => {
            format!("could not read {} ({source})", path.display())
        }
        InstallError::WriteRc { path, source } => {
            format!("could not write {} ({source})", path.display())
        }
    }
}

// ---------------------------------------------------------------------------
// the seams: one per command whose flow lands later
// ---------------------------------------------------------------------------

/// `dl --prune [-y] [--force]` — M6 (lifecycle flows).
fn render_prune(_yes: bool, _force: bool) -> Ending {
    not_yet("--prune", "M6")
}

/// `dl --reconcile [-y]` — M6.
fn render_reconcile(_yes: bool) -> Ending {
    not_yet("--reconcile", "M6")
}

/// `dl --purge [-y]` — M6.
fn render_purge(_yes: bool) -> Ending {
    not_yet("--purge", "M6")
}

/// A verb with no workspace named: the embedded fuzzy picker — M8.
fn render_select(_verb: Verb) -> Ending {
    not_yet("the interactive workspace selector", "M8")
}

/// A workspace and a verb. Which milestone owns the flow depends on the verb, so
/// the match is here rather than one message for all of them.
fn render_workspace(verb: Verb) -> Ending {
    let milestone = match verb {
        // The lifecycle flows.
        Verb::Stop | Verb::Remove { .. } => "M6",
        // Launch: clone, `devpod up`, fast attach, `-- <cmd>` through
        // `devpod ssh --command`.
        Verb::Attach
        | Verb::Run(_)
        | Verb::Up
        | Verb::Code
        | Verb::Recreate
        | Verb::Restart
        | Verb::Reset
        | Verb::Dotfiles => "M7",
    };
    not_yet(&format!("`dl <workspace> {}`", verb.word()), milestone)
}

/// The one sentence a command whose flow is not ported yet prints.
///
/// A refusal rather than a silent success, and it names the milestone so a user of
/// a mid-port build knows this is a build without the command rather than a
/// command that did nothing.
fn not_yet(what: &str, milestone: &str) -> Ending {
    eprintln!("dl: {what} is not in this build yet (the Rust port reaches it at {milestone}).");
    Ending::Refused
}

// ---------------------------------------------------------------------------
// the shared refusals
// ---------------------------------------------------------------------------

/// A refusal from the workspace listing, rendered and turned into an ending.
///
/// The one refusal worth its own exit code is a devpod that is not on PATH:
/// nothing dl can do will work until it is installed, and 127 says that to a
/// script without it having to read the message.
fn refuse_listing(refused: &ListingUnreadable) -> Ending {
    eprintln!("{}", render::listing_refusal(refused));
    if render::is_devpod_missing(refused) {
        Ending::DevpodMissing
    } else {
        Ending::Refused
    }
}

/// The listing refusal without the `error: ` prefix, for the places it is quoted
/// inside a sentence of its own.
fn short(refused: &ListingUnreadable) -> String {
    let line = render::listing_refusal(refused);
    line.strip_prefix("error: ").unwrap_or(&line).to_owned()
}

fn refuse_startup(refused: &StartupError) -> Ending {
    let line = match refused {
        StartupError::NoHomeDirectory => {
            "this machine names no home directory, so dl cannot find its cache".to_owned()
        }
        StartupError::Config(error) => render::config_error(error),
        StartupError::Metadata(error) => render::metadata_error(error),
    };
    eprintln!("error: {line}");
    Ending::Refused
}

/// Everything a metadata load and the cache migration had to say.
fn report(records: &Records<'_>) {
    for line in render::metadata_notices(&records.notices) {
        eprintln!("{line}");
    }
    if let Some(refused) = &records.migration_refused {
        eprintln!(
            "Could not migrate the workspace cache: {}",
            render::metadata_error(refused)
        );
    }
}

/// A startup answer, or nothing once the refusal has been reported.
fn ok_or_refuse<T, E: Into<StartupError>>(answer: Result<T, E>) -> Option<T> {
    match answer {
        Ok(value) => Some(value),
        Err(error) => {
            refuse_startup(&error.into());
            None
        }
    }
}
