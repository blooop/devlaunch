//! One `render_*` per [`Command`] arm, and the exhaustive match that picks it.
//!
//! Adding a command to [`Command`] breaks [`dispatch`] until it is handled, which
//! is the whole reason the grammar is a sum rather than a bag of flags. The arms
//! whose flows land in a later milestone are still arms here: each one has its own
//! `render_*` with the milestone named, so wiring it up next wave replaces a body
//! rather than growing a dispatcher.

use std::io::Write as _;
use std::path::Path;

use devlaunch_core::clients::devpod::{ListingUnreadable, NotRun};
use devlaunch_core::domain::spec::DevcontainerPath;
use devlaunch_core::flows::completion::{self, FileState, InstallError, Installed, RcChange};
use devlaunch_core::flows::completion_cache::{self, Refreshed};
use devlaunch_core::flows::launch::LaunchNotice;
use devlaunch_core::flows::lifecycle::{
    self, ChildWork, DeleteOutcome, Guarded, Insistence, LifecycleNotice, PruneError, PruneOutcome,
    Refresh, RefreshReason, StopOutcome,
};
use devlaunch_core::flows::listing::{self, CommandContext, DlView, Sizes};
use devlaunch_core::flows::repo_manager::CacheNotice;
use devlaunch_core::runner::{Exit, Runner};

use crate::cli::{self, Command, ListOutput, RmOnExit, Verb};
use crate::cold::ColdPath;
use crate::launch::{self, Family, Reached};
use crate::render;
use crate::select;
use crate::session::{self, Records, StartupError};
use crate::target::{self, Unaddressable};

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
    /// A devpod that ran, and this is how *it* ended. The lifecycle verbs hand the
    /// child's own status back to the shell, as Python returned `result.returncode`
    /// — so a script that reads `$?` after `dl <ws> stop` reads devpod's answer and
    /// not dl's opinion of it.
    Child(Exit),
    /// A session ran, and this is the number Python returned for it.
    ///
    /// Its own arm rather than a [`Ending::Child`] carrying an [`Exit`], because a
    /// session's ending is already the answer to "whose number is this": core's
    /// [`Session`](devlaunch_core::flows::launch::Session) has resolved the three
    /// processes a status can come from into the one number Python returns,
    /// negative-for-a-signal included, and re-deriving it here from an `Exit` would
    /// be a second opinion about a question already settled.
    Session(i32),
}

impl Ending {
    pub(crate) fn code(self) -> i32 {
        match self {
            Ending::Done => 0,
            Ending::Refused => 1,
            Ending::DevpodMissing => 127,
            // Python's `sys.exit(result.returncode)`, negative status included: a
            // child killed by SIGINT returned -2 there and exits 254 here, because
            // both truncate the status to its low eight bits.
            Ending::Child(Exit::Code(code)) => code,
            Ending::Child(Exit::Signal(signal)) => -signal,
            Ending::Session(status) => status,
        }
    }
}

/// Run one command and say how it ended.
///
/// `cache` is devlaunch's cache directory, resolved once by the caller: every
/// command below either reads it or removes it, and resolving it per command is how
/// two halves of one run come to disagree about where the cache is.
///
/// `refresh` is the one background refresh this process may spawn. It is threaded
/// through rather than made here because Python's latch is process-wide: a command
/// that warmed the cache on the way in must not spawn a second child on the way
/// out.
pub(crate) fn dispatch(
    runner: &dyn Runner,
    cache: &Path,
    refresh: &mut Refresh<'_>,
    command: Command,
) -> Ending {
    let mut context = CommandContext::new(runner);
    match command {
        Command::Version => render_version(),
        Command::List { output, sizes } => render_list(runner, &mut context, cache, output, sizes),
        Command::Repos => render_repos(&mut context, cache),
        Command::CompletionData => render_completion_data(&mut context, cache),
        Command::UpdateCache { force } => render_update_cache(runner, &mut context, cache, force),
        Command::Refresh => render_refresh(&mut context, cache),
        Command::Install { rc } => render_install(&mut context, cache, rc.as_deref()),
        Command::Prune { yes, force } => render_prune(runner, &mut context, yes, force),
        Command::Reconcile { yes } => render_reconcile(runner, &mut context, refresh, yes),
        Command::Purge { yes } => render_purge(&mut context, cache, yes),
        Command::Select { verb, devcontainer } => render_select(
            runner,
            &mut context,
            cache,
            refresh,
            verb,
            devcontainer.as_ref(),
        ),
        Command::Workspace {
            target,
            verb,
            devcontainer,
        } => render_workspace(
            runner,
            &mut context,
            cache,
            refresh,
            &target,
            verb,
            devcontainer.as_ref(),
        ),
    }
}

/// The commands a machine with no cache directory can still run.
///
/// One of them, and it is the one that needs nothing: `dl --version` answers from
/// the binary itself. Everything else reads or writes something under the cache, so
/// it refuses with the reason rather than half-running.
pub(crate) fn without_a_cache_directory(command: Command) -> Ending {
    match command {
        Command::Version => render_version(),
        _ => refuse_startup(&StartupError::NoHomeDirectory),
    }
}

// ---------------------------------------------------------------------------
// --version
// ---------------------------------------------------------------------------

/// `dl <version>`, plus [`crate::BUILD_MARKER`] when this is not a released build.
///
/// The marker is empty in everything that ships, so a released `dl` prints the
/// bare version and nothing else — which is what the packaging job asserts. A
/// working-tree build made by `./dev.sh` prints `-dev` after it, so `dl-next` and
/// `dl` are distinguishable by output and not only by the name they were typed
/// under (#268).
fn render_version() -> Ending {
    println!("dl {}{}", crate::VERSION, crate::BUILD_MARKER);
    Ending::Done
}

// ---------------------------------------------------------------------------
// --ls
// ---------------------------------------------------------------------------

fn render_list(
    runner: &dyn Runner,
    context: &mut CommandContext<'_>,
    cache: &Path,
    output: ListOutput,
    sizes: Sizes,
) -> Ending {
    match output {
        ListOutput::Table => render_table(context, cache, sizes),
        ListOutput::Json => render_json(runner, context, cache, sizes),
    }
}

/// The human table. Reads devpod and nothing else — no config, no records, no
/// migration, which is what keeps a listing one round trip.
fn render_table(context: &mut CommandContext<'_>, cache: &Path, sizes: Sizes) -> Ending {
    match listing::workspace_table(context, cache, sizes) {
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
fn render_json(
    runner: &dyn Runner,
    context: &mut CommandContext<'_>,
    cache: &Path,
    sizes: Sizes,
) -> Ending {
    let records = match session::open_records(runner) {
        Err(refused) => return refuse_startup(&refused),
        Ok(records) => records,
    };
    report(&records);
    // The resolver rather than the manager, because naming a record's clone can
    // itself have something to say (`CacheNotice::CloneNotNamed`) and the row-
    // building loop has nowhere to put it. Python logs that warning; M5c dropped
    // it, for want of a public `CacheNotice`.
    let directories = lifecycle::CloneDirectories::of(&records.clones);
    let view = DlView {
        cache_dir: cache,
        storage: &records.storage,
        clones: &directories,
    };
    let listed = listing::enriched_listing(context, &view, sizes);
    say_cache(directories.take_notices());
    match listed {
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
fn render_repos(context: &mut CommandContext<'_>, cache: &Path) -> Ending {
    if let Some(cached) =
        completion_cache::read_completion_cache(&completion_cache::cache_path(cache))
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
fn render_completion_data(context: &mut CommandContext<'_>, cache: &Path) -> Ending {
    if let Some(cached) =
        completion_cache::read_completion_cache(&completion_cache::cache_path(cache))
    {
        println!("{}", cached.as_json_line());
        return Ending::Done;
    }
    let refreshed = match refresh_cache(context, cache) {
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
fn render_update_cache(
    runner: &dyn Runner,
    context: &mut CommandContext<'_>,
    cache: &Path,
    force: bool,
) -> Ending {
    let reason = if force {
        RefreshReason::Forced
    } else {
        RefreshReason::IfStale
    };
    match lifecycle::child_work(&completion_cache::cache_path(cache), reason) {
        ChildWork::NothingToDo => Ending::Done,
        ChildWork::RefreshAndSweep => {
            if let Err(ending) = refresh_cache(context, cache) {
                return ending;
            }
            // Completions first, freshness second: the cache is what the user's
            // next keystroke reads, while the fetch sweep is for the launch after
            // that. Both are on the same hour, so a child that gets this far does
            // both or, when it exits early above, neither.
            let mut records = match session::open_records(runner) {
                Err(refused) => return refuse_startup(&refused),
                Ok(records) => records,
            };
            report(&records);
            announce_lock_waits(&mut records);
            let Records {
                storage, clones, ..
            } = &mut records;
            // The sweep's own per-repository arms are Python's `logging.debug` and
            // are counted rather than printed; the notices under them are the
            // `logging.warning` half and are said.
            let swept = lifecycle::sweep_repo_fetches(clones.repo_manager(), storage);
            say(&swept.notices);
            Ending::Done
        }
    }
}

/// The same refresh, with the two lines a person asked for.
fn render_refresh(context: &mut CommandContext<'_>, cache: &Path) -> Ending {
    println!("Refreshing completion cache...");
    let refreshed = match refresh_cache(context, cache) {
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
fn render_install(context: &mut CommandContext<'_>, cache: &Path, rc: Option<&Path>) -> Ending {
    if let Err(ending) = refresh_cache(context, cache) {
        return ending;
    }
    match completion::install(rc) {
        Err(refused) => {
            // Python logged `Wrote completion script to ...` before the rc step
            // that then raised, so a failure after the script was written still
            // says the script landed (P17). The error carries that fact on
            // exactly the arms it is true for.
            if let Some(written) = refused.written_script() {
                report_script_state(&written.path, written.state);
            }
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

/// The script-state line, shared by the success report and the P17 failure path.
fn report_script_state(script: &Path, state: FileState) {
    match state {
        FileState::Written => eprintln!("Wrote completion script to {}", script.display()),
        FileState::AlreadyCurrent => {
            eprintln!(
                "Completion script at {} is already current",
                script.display()
            );
        }
    }
}

fn report_install(installed: &Installed) {
    let rc = installed.rc.display();
    report_script_state(&installed.script, installed.script_state);
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
        InstallError::CreateScriptDirectory { path, source }
        | InstallError::CreateRcDirectory { path, source, .. } => {
            format!("could not create {} ({source})", path.display())
        }
        InstallError::WriteScript { path, source } => {
            format!("could not write {} ({source})", path.display())
        }
        InstallError::ReadRc { path, source, .. } => {
            format!("could not read {} ({source})", path.display())
        }
        InstallError::WriteRc { path, source, .. } => {
            format!("could not write {} ({source})", path.display())
        }
    }
}

// ---------------------------------------------------------------------------
// the lifecycle verbs: dl <ws> stop, dl <ws> rm
// ---------------------------------------------------------------------------

/// A workspace and a verb.
///
/// One [`ColdPath`] for the whole command, whichever verb this is: the lifecycle
/// verbs and the launch verbs both resolve the target through the same two calls
/// into core, and both have to be able to open dl's records without either of them
/// opening a second copy.
#[allow(clippy::too_many_arguments)]
fn render_workspace<'r>(
    runner: &'r dyn Runner,
    context: &mut CommandContext<'r>,
    cache: &Path,
    refresh: &mut Refresh<'_>,
    target: &str,
    verb: Verb,
    devcontainer: Option<&DevcontainerPath>,
) -> Ending {
    let mut cold = ColdPath::new(runner);
    match launch::family(&verb) {
        Family::Stop => {
            devcontainer_ignored(devcontainer.is_some(), "stop");
            render_stop(runner, context, refresh, &mut cold, target)
        }
        Family::Remove { force } => {
            devcontainer_ignored(devcontainer.is_some(), "rm");
            let insistence = if force {
                Insistence::Insisted
            } else {
                Insistence::NotInsisted
            };
            render_remove(
                runner, context, cache, refresh, &mut cold, target, insistence,
            )
        }
        // Launch: clone, `devpod up`, fast attach, `-- <cmd>` through
        // `devpod ssh --command`.
        Family::Launch { verb: launched, rm } => {
            let ran = launch::render_launch(
                context,
                cache,
                refresh,
                &mut cold,
                target,
                &launched,
                devcontainer,
            );
            after_the_session(runner, context, cache, refresh, &mut cold, target, rm, ran)
        }
    }
}

/// `--rm`: the workspace, once the session it was opened for has ended.
///
/// Three decisions live here, and each one is the answer to a question the flag
/// cannot dodge.
///
/// **When.** Whenever the launch got as far as asking devpod for the workspace —
/// [`Reached::TheWorkspace`] — and not before. That is deliberately *not* "when a
/// session ran": the flag's job is that no workspace this line brought into being
/// outlives it, and the ways a launch ends badly after devpod has been asked are
/// exactly the ways it leaves one behind.
///
/// A `devpod up` that dies in `postCreateCommand` is the case that matters, and the
/// one an earlier draft of this got wrong by keying on the exit code. It leaves the
/// container **running**, devpod's record written and the clone cut — which is why
/// [`lifecycle::create_record`] exists at all — so an unattended
/// `dl owner/repo --rm -- make test` against a broken devcontainer would leak
/// precisely the workspace the flag was reached for. A session devpod refused
/// outright, and an OpenSSH that is not installed, leave the same thing behind and
/// are collected for the same reason.
///
/// The other side of the line is [`Reached::Nothing`]: an unsafe spec, a workspace
/// nothing answers to, a default branch that could not be named, a host-side clone
/// that was never cut, a devpod that could not be run. Those created nothing, and a
/// removal attempted anyway would answer one refusal with a second unrelated one.
///
/// **Which workspace.** [`target::resolve`], the same resolution `dl <ws> rm` uses,
/// rather than anything carried out of the launch. That is a round trip this path
/// could have saved — the launch already named the workspace — and it buys the one
/// thing worth more than a round trip on a path that deletes: there is exactly one
/// answer to "which workspace is `<target>`", so `--rm` and the `rm` verb cannot
/// disagree about what they are removing. It is also cheap where it matters: by now the
/// workspace exists and its record is written, so a bare `owner/repo` reads its
/// default branch off the record rather than the network.
///
/// **Whose exit code.** The launch's, always. `dl owner/repo --rm -- make test`
/// is read by a script that wants the test's answer, and a cleanup that refused is
/// not the test failing. The refusal is loud on stderr and the workspace is still
/// there, which is the recoverable state — `dl <target> rm --force` is one line away,
/// and [`render::removal_refusal`] is already the sentence that says so.
#[allow(clippy::too_many_arguments)]
fn after_the_session<'r>(
    runner: &'r dyn Runner,
    context: &mut CommandContext<'r>,
    cache: &Path,
    refresh: &mut Refresh<'_>,
    cold: &mut ColdPath<'r>,
    target: &str,
    rm: RmOnExit,
    ran: launch::Ran,
) -> Ending {
    let launch::Ran { ending, reached } = ran;
    let RmOnExit::Yes = rm else {
        return ending;
    };
    let Reached::TheWorkspace = reached else {
        return ending;
    };
    // Said before the removal rather than after it: what it names is the reason
    // somebody may want to hit Ctrl-C, and a notice that arrives once the container
    // is already gone is a receipt rather than a warning.
    eprintln!("{}", render::rm_on_exit_removing(target));
    // The launch has already spent this command's one refresh — `run_attach` forces
    // one the moment the session returns — and that child is describing a world this
    // removal is about to change. Without re-arming, the cache a user's next
    // keystroke reads goes on offering the workspace that has just been deleted,
    // which is the one name that should have stopped being offered. Re-armed *here*
    // rather than inside the removal, because it is this command having two state
    // changes that earns the second child, not deleting as such.
    refresh.rearm();
    let _: Ending = render_remove(
        runner,
        context,
        cache,
        refresh,
        cold,
        target,
        Insistence::NotInsisted,
    );
    ending
}

/// A config choice a verb that opens no workspace cannot honour, said rather than
/// discarded.
fn devcontainer_ignored(given: bool, verb: &str) {
    if given {
        eprintln!("Ignoring --devcontainer: it does not apply to '{verb}'.");
    }
}

/// `dl <ws> stop` — one `devpod stop`, and devpod's own status back.
fn render_stop<'r>(
    runner: &'r dyn Runner,
    context: &mut CommandContext<'r>,
    refresh: &mut Refresh<'_>,
    cold: &mut ColdPath<'r>,
    target: &str,
) -> Ending {
    let addressed = match target::resolve(runner, context, cold, target) {
        Err(refused) => return refuse_target(&refused),
        Ok(addressed) => addressed,
    };
    // The metadata load's own notices were said by the `ColdPath` at the moment it
    // opened the store, which is where they happened.
    say_launch(&addressed.notices);
    match lifecycle::workspace_stop(context, refresh, &addressed.workspace_id) {
        Err(not_run) => refuse_devpod("stop", &not_run),
        // devpod's own diagnostics are already on this process's stderr — the call
        // inherits the streams — so a refusal has nothing to add but the status.
        Ok(StopOutcome::Stopped) => Ending::Done,
        Ok(StopOutcome::DevpodRefused { exit }) => Ending::Child(exit),
    }
}

/// `dl <ws> rm [--force]` — the guard, the delete, and the clone with it.
#[allow(clippy::too_many_arguments)]
fn render_remove<'r>(
    runner: &'r dyn Runner,
    context: &mut CommandContext<'r>,
    cache: &Path,
    refresh: &mut Refresh<'_>,
    cold: &mut ColdPath<'r>,
    target: &str,
    insistence: Insistence,
) -> Ending {
    let addressed = match target::resolve(runner, context, cold, target) {
        Err(refused) => return refuse_target(&refused),
        Ok(addressed) => addressed,
    };
    say_launch(&addressed.notices);
    // The delete needs the records whatever the resolution needed, so a resolution
    // that did not open them opens them here — through the same `ColdPath`, which is
    // what keeps one command from holding two views of `metadata.json`.
    let records = match cold.records() {
        Err(refused) => return refuse_startup(&refused),
        Ok(records) => records,
    };
    let workspace_id = addressed.workspace_id;
    let mut notices: Vec<LifecycleNotice> = Vec::new();

    // The one thing dl refuses on its own account. Asked only when `--force` was
    // not typed: an insisted delete does not need the answer, and the probe is a
    // `git status` and a `git log` per clone.
    if let Insistence::NotInsisted = insistence {
        let unsaved = lifecycle::unsaved_work_in(
            &records.clones,
            &records.storage,
            &context.git(),
            cache,
            &workspace_id,
            &mut notices,
        );
        say(&notices);
        notices.clear();
        if let Guarded::Refused(refusal) =
            lifecycle::guard_removal(&workspace_id, unsaved, insistence)
        {
            eprintln!("{}", render::removal_refusal(&refusal, target));
            return Ending::Refused;
        }
    }

    let Records {
        storage, clones, ..
    } = records;
    let deleted = lifecycle::workspace_delete(
        context,
        refresh,
        clones,
        storage,
        // Resolved here rather than in core, for the reason every other environment
        // answer is: the process that knows what its environment says hands the
        // answer down. `None` is a machine with no home directory, where devpod has
        // no records to read and so no volume names to derive.
        lifecycle::devpod_home().as_deref(),
        &workspace_id,
        insistence,
        &mut notices,
    );
    match deleted {
        Err(not_run) => refuse_devpod("delete", &not_run),
        Ok(DeleteOutcome::DevpodRefused { exit }) => {
            // The local clone is kept, so the delete stays retryable: devpod
            // re-parses the workspace's devcontainer.json to tear the container
            // down, and removing the clone regardless strands it for good.
            eprintln!("{}", render::delete_refused(&workspace_id));
            say(&notices);
            Ending::Child(exit)
        }
        Ok(DeleteOutcome::Deleted { .. }) => {
            say(&notices);
            Ending::Done
        }
    }
}

// ---------------------------------------------------------------------------
// --purge
// ---------------------------------------------------------------------------

/// How a cleanup command's body ended.
///
/// The two are told apart because only one of them ends on the docker boundary.
/// Python's structure decides it: the sentence is printed by the wrapper *after*
/// the body returns, so every exit code the body returns gets it — and every
/// failure the body *raised* out of the command skips it, which is a devpod that
/// could not be run, a listing that could not be read, and a lock that could not
/// be taken. Those never got as far as looking at a directory, so there is no
/// report for the boundary to belong under.
enum Cleanup {
    Ended(Ending),
    Raised(Ending),
}

impl Cleanup {
    /// The ending, with the boundary said if this was an ending that has one.
    fn with_the_boundary(self) -> Ending {
        match self {
            Cleanup::Ended(ending) => {
                println!("{}", render::DOCKER_BOUNDARY);
                ending
            }
            Cleanup::Raised(ending) => ending,
        }
    }
}

/// `dl --purge [-y]` — the workspaces devlaunch created, and its whole cache.
///
/// Every ending it *returns* goes through the docker boundary, including the abort:
/// the listing above the question is a report of what a purge would take, and a
/// report is exactly where the disk it would *not* take is worth naming — more so
/// after a `n`, since somebody who just said no is still looking for the gigabytes.
fn render_purge(context: &mut CommandContext<'_>, cache: &Path, yes: bool) -> Ending {
    purge_devlaunch_data(context, cache, yes).with_the_boundary()
}

fn purge_devlaunch_data(context: &mut CommandContext<'_>, cache: &Path, yes: bool) -> Cleanup {
    let plan = match lifecycle::purge_plan(context, cache) {
        Err(refused) => return Cleanup::Raised(refuse_listing(&refused)),
        Ok(plan) => plan,
    };
    print(&render::purge_plan_lines(&plan));
    if !yes && !confirmed("Are you sure? [y/N] ") {
        println!("Aborted.");
        return Cleanup::Ended(Ending::Done);
    }
    // Read after the question, because a purge that was declined asks devpod
    // nothing and reads nothing.
    let devpod_home = lifecycle::devpod_home();
    let purged = lifecycle::purge_all_data(context, &plan, devpod_home.as_deref(), &mut |step| {
        match render::purge_step(&step) {
            // Said before the round trip that may take a while, which is why this
            // is a callback and not a report: "Deleting workspace X" assembled
            // afterwards cannot be said in time.
            render::Line::Out(line) => println!("{line}"),
            render::Line::Err(line) => eprintln!("{line}"),
        }
    });
    match purged {
        // Raised out of the command in Python, and it takes the report with it:
        // nothing after this point would work either.
        Err(not_run) => Cleanup::Raised(refuse_devpod("delete", &not_run)),
        Ok(outcome) => {
            print(&render::purge_outcome(&outcome));
            if outcome.finished() {
                Cleanup::Ended(Ending::Done)
            } else {
                // Not 0: a clone the user was told would go is still on disk. Which
                // of the two failures happened is in the report above, where
                // somebody can act on it.
                Cleanup::Ended(Ending::Refused)
            }
        }
    }
}

/// Print the contended-lock wait notices Python prints, so a maintenance command
/// that sits blocked on another dl run's lock says why it has gone quiet rather
/// than leaving an empty stderr (concurrency review R7).
///
/// Python takes these two locks with a `waiting_note`: the per-repo lock a
/// `--prune`/`--reconcile` holds while it weighs a repository's clones
/// (`dl.py` `_repo_lock`), and the metadata lock every save takes
/// (`storage.exclusive`). Each line is printed once, before the blocking
/// acquisition — which is the only moment "this run is now waiting" can be said.
/// The strings are byte-for-byte Python's (`worktree/locks.py:89`,
/// `worktree/storage.py:105`).
fn announce_lock_waits(records: &mut Records<'_>) {
    records.clones.on_repo_lock_wait(|owner, repo| {
        eprintln!("dl: waiting for another dl run preparing {owner}/{repo}");
    });
    records.storage.on_metadata_lock_wait(|| {
        eprintln!("dl: waiting for another dl run updating the workspace list")
    });
}

// ---------------------------------------------------------------------------
// --prune
// ---------------------------------------------------------------------------

/// `dl --prune [-y] [--force]` — the clone directories no live workspace opens.
///
/// It ends where `--purge` ends and in the same words, for every ending it returns:
/// answering `n` is this command's read-only view, and a report is exactly where the
/// boundary belongs. **Divergence row 14** takes the one ending that did not print
/// it — Python's own refusal of an unknown option — because clap refuses the flag
/// before this function is reached, and exits 2 where Python exited 1.
fn render_prune(
    runner: &dyn Runner,
    context: &mut CommandContext<'_>,
    yes: bool,
    force: bool,
) -> Ending {
    prune_clone_directories(runner, context, yes, force).with_the_boundary()
}

fn prune_clone_directories(
    runner: &dyn Runner,
    context: &mut CommandContext<'_>,
    yes: bool,
    force: bool,
) -> Cleanup {
    let mut records = match session::open_records(runner) {
        Err(refused) => return Cleanup::Raised(refuse_startup(&refused)),
        Ok(records) => records,
    };
    report(&records);
    announce_lock_waits(&mut records);
    let workspaces = match context.workspaces() {
        Err(refused) => return Cleanup::Raised(refuse_listing(&refused)),
        Ok(workspaces) => workspaces,
    };
    let placement = lifecycle::ClonePlacement::resolve(&records.clones, &workspaces);
    if let Some(unlocatable) = placement.unlocatable() {
        print(&render::report_unlocatable(
            &unlocatable,
            "--prune",
            "Nothing was removed",
        ));
        return Cleanup::Ended(Ending::Refused);
    }
    let insistence = if force {
        Insistence::Insisted
    } else {
        Insistence::NotInsisted
    };
    let mut notices: Vec<LifecycleNotice> = Vec::new();
    let plan = match lifecycle::prune_plan(
        &records.clones,
        &records.storage,
        &workspaces,
        &placement,
        insistence,
        &mut notices,
    ) {
        Err(refused) => return Cleanup::Raised(refuse_prune(&refused)),
        Ok(plan) => plan,
    };
    say(&notices);
    notices.clear();
    print(&render::prune_plan_lines(&plan));
    if plan.nothing_to_do() {
        return Cleanup::Ended(Ending::Done);
    }
    if !yes && !confirmed("Are you sure? [y/N] ") {
        println!("Aborted.");
        return Cleanup::Ended(Ending::Done);
    }
    let Records {
        storage, clones, ..
    } = &mut records;
    let acted = lifecycle::prune_clones(context, clones, storage, &plan, &mut notices);
    say(&notices);
    match acted {
        Err(refused) => Cleanup::Raised(refuse_prune(&refused)),
        Ok(PruneOutcome::Unlocatable(unlocatable)) => {
            print(&render::report_unlocatable(
                &unlocatable,
                "--prune",
                "Nothing was removed",
            ));
            Cleanup::Ended(Ending::Refused)
        }
        Ok(PruneOutcome::Acted(report)) => {
            print(&render::prune_report_lines(&report));
            Cleanup::Ended(if report.finished() {
                Ending::Done
            } else {
                // Not 0: a directory the user was told would go is still on disk.
                // The clones that did go are still gone, which is why this is a
                // report and not an abort.
                Ending::Refused
            })
        }
    }
}

/// Why a prune could not be carried out.
fn refuse_prune(refused: &PruneError) -> Ending {
    match refused {
        // Fatal rather than skipped: a scan that silently left out a repository
        // would report a plan that is not the plan.
        PruneError::Lock(error) => {
            eprintln!(
                "error: {}",
                render::lock_refusal(error, "the repository lock")
            );
            Ending::Refused
        }
        PruneError::Listing(refused) => refuse_listing(refused),
    }
}

// ---------------------------------------------------------------------------
// --reconcile
// ---------------------------------------------------------------------------

/// `dl --reconcile [-y]` — re-point the devpod records the id-scheme change
/// orphaned (devlaunch#88). Deletes nothing, and never prints the docker boundary:
/// it frees no disk to have a boundary about.
fn render_reconcile(
    runner: &dyn Runner,
    context: &mut CommandContext<'_>,
    refresh: &mut Refresh<'_>,
    yes: bool,
) -> Ending {
    let mut records = match session::open_records(runner) {
        Err(refused) => return refuse_startup(&refused),
        Ok(records) => records,
    };
    report(&records);
    announce_lock_waits(&mut records);
    let workspaces = match context.workspaces() {
        Err(refused) => return refuse_listing(&refused),
        Ok(workspaces) => workspaces,
    };
    let placement = lifecycle::ClonePlacement::resolve(&records.clones, &workspaces);
    if let Some(unlocatable) = placement.unlocatable() {
        // `--prune`'s stop, for `--prune`'s reason: a clone that cannot be shown to
        // be free is one no orphan can be given.
        print(&render::report_unlocatable(
            &unlocatable,
            "--reconcile",
            "Nothing was re-pointed",
        ));
        return Ending::Refused;
    }
    let mut notices: Vec<LifecycleNotice> = Vec::new();
    let plan = lifecycle::reconcile_plan(
        &records.clones,
        &records.storage,
        &workspaces,
        &placement,
        &mut notices,
    );
    say(&notices);
    notices.clear();
    print(&render::reconcile_plan_lines(&plan));
    if plan.adopting().is_empty() {
        // Nothing to consent to. A report of workspaces dl will not touch is
        // already complete, and asking about it would imply an action it has.
        return Ending::Done;
    }
    if !yes && !confirmed("Re-point these? [y/N] ") {
        println!("Aborted.");
        return Ending::Done;
    }
    let Some(devpod_home) = lifecycle::devpod_home() else {
        return refuse_startup(&StartupError::NoHomeDirectory);
    };
    let Records { storage, .. } = &mut records;
    let applied = lifecycle::apply_reconciliation(
        context,
        refresh,
        storage,
        &devpod_home,
        &plan,
        &mut notices,
    );
    // The report carries one ending per adoption, in the order they were
    // attempted — the plan's order, which is the order these lines happened in:
    // one adoption at a time, each either done or refused.
    for adoption in applied.adoptions() {
        match adoption {
            lifecycle::Adoption::Repointed(adoptable) => println!(
                "Re-pointed {} at {}",
                adoptable.workspace_id,
                adoptable.clone.display()
            ),
            lifecycle::Adoption::Refused {
                workspace_id,
                failure,
            } => eprintln!(
                "Could not re-point {workspace_id}: {}",
                render::repoint_failure(failure)
            ),
        }
    }
    say(&notices);
    if applied.finished() {
        Ending::Done
    } else {
        Ending::Refused
    }
}

// ---------------------------------------------------------------------------
// the selector
// ---------------------------------------------------------------------------

/// A verb with no workspace named: the embedded fuzzy picker chooses one — or,
/// for a verb that applies per workspace, several.
///
/// **Divergence row 21** decides where each pick goes: through the same path
/// `dl <ws> <verb>` takes, rather than Python's straight-to-`workspace_up`. One
/// `devpod status` buys the fast attach every other entry already pays for, and the
/// verb the selector was opened with is honoured — `dl --stop` picks a workspace and
/// stops it.
///
/// Whether the picker takes one row or many is the verb's to say
/// ([`Verb::several_at_once`]): `dl rm` lets TAB mark five dead workspaces and
/// clears them in one visit, while a verb that ends in a session takes one. A batch
/// is applied in the order the rows were taken, every workspace attempted whatever
/// happened to the ones before it — the point of marking five is that one refusal
/// (say, unsaved work) must not silently drop the other four. The command's ending
/// is the first that was not [`Ending::Done`], so a script still learns something
/// failed and the specific code of the first failure survives.
///
/// A pick that never came is Python's ending exactly: the help on stdout and exit 1
/// (`dl.py` 4457-4462). The help is clap's (**row 3**).
fn render_select<'r>(
    runner: &'r dyn Runner,
    context: &mut CommandContext<'r>,
    cache: &Path,
    refresh: &mut Refresh<'_>,
    verb: Verb,
    devcontainer: Option<&DevcontainerPath>,
) -> Ending {
    let workspaces = match context.workspaces() {
        Err(refused) => return refuse_listing(&refused),
        Ok(workspaces) => workspaces,
    };
    let arity = if verb.several_at_once() {
        select::Arity::Several
    } else {
        select::Arity::One
    };
    match select::pick(&workspaces, arity, cache) {
        select::Pick::Chose(workspace_ids) => {
            let mut ending = Ending::Done;
            for (already_acted, workspace_id) in workspace_ids.iter().enumerate() {
                // Each workspace after the first is one more state change after
                // whatever refresh the last one spawned, so the child indexing the
                // old world must not be the last word — the same reasoning as
                // `--rm`'s re-arm in `after_the_session`. A no-op for the
                // single pick every verb used to be.
                if already_acted > 0 {
                    refresh.rearm();
                }
                let ran = render_workspace(
                    runner,
                    context,
                    cache,
                    refresh,
                    workspace_id,
                    verb.clone(),
                    devcontainer,
                );
                if matches!(ending, Ending::Done) {
                    ending = ran;
                }
            }
            ending
        }
        // Nothing to say: a user who quit the picker watched themselves do it, and
        // read the invitation on the way past.
        select::Pick::Quit => no_pick(),
        select::Pick::NoWorkspaces => {
            eprintln!("No workspaces found. Create one with: dl owner/repo or dl ./path");
            no_pick()
        }
        // The one run that needs the invitation on stdout. No terminal means no
        // picker was drawn, so its header was never drawn either, and stdout is the
        // only surface left — the one case where printing the line is not writing it
        // under a screen that is about to cover it.
        select::Pick::NoTerminal => {
            println!("{}", select::invitation(arity));
            no_pick()
        }
    }
}

/// Python's ending for a selector that chose nothing: the help, and exit 1.
fn no_pick() -> Ending {
    let _ = <cli::Cli as clap::CommandFactory>::command().print_help();
    Ending::Refused
}

// ---------------------------------------------------------------------------
// printing, and the one question
// ---------------------------------------------------------------------------

/// Lines to stdout, in order.
fn print(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

/// The storage flows' notices to stderr, for the callers that collect those on
/// their own rather than through a lifecycle flow.
fn say_cache(notices: Vec<CacheNotice>) {
    let wrapped: Vec<LifecycleNotice> = notices.into_iter().map(LifecycleNotice::Cache).collect();
    say(&wrapped);
}

/// Notices to stderr, in the order they happened.
fn say(notices: &[LifecycleNotice]) {
    for line in render::lifecycle_notices(notices) {
        eprintln!("{line}");
    }
}

/// The same, for the notices the shared target resolution reports in.
fn say_launch(notices: &[LaunchNotice]) {
    for line in render::launch_notices(notices) {
        eprintln!("{line}");
    }
}

/// The `[y/N]` question, asked on stdout and answered on stdin.
///
/// The prompt goes to stdout without a newline and is flushed before the read, as
/// Python's `input()` does, so a piped run reads the plan, the question and the
/// answer in one stream in that order.
///
/// Anything but `y` or `yes` is no, and so is a stdin that has nothing left to
/// give: a closed stdin is an answer that never comes, and the only safe reading of
/// no answer is no.
fn confirmed(question: &str) -> bool {
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
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

/// A target no workspace answers to, in the words Python refused it with.
fn refuse_target(refused: &Unaddressable) -> Ending {
    match refused {
        Unaddressable::Unknown { target } => {
            eprintln!("{}", render::unknown_workspace(target));
            Ending::Refused
        }
        Unaddressable::Ambiguous { target, candidates } => {
            eprintln!("{}", render::ambiguous_workspace(target, candidates));
            Ending::Refused
        }
        Unaddressable::Name(name) => {
            eprintln!("{}", render::unsafe_name(name));
            Ending::Refused
        }
        Unaddressable::Listing(refused) => refuse_listing(refused),
        Unaddressable::DevpodNotRun(refused) => {
            // Named by the subcommand that did not happen, like every other
            // could-not-run line; `NotInstalled` is the one that exits 127.
            eprintln!("{}", render::devpod_not_run("status", refused));
            if matches!(refused, NotRun::NotInstalled) {
                Ending::DevpodMissing
            } else {
                Ending::Refused
            }
        }
        Unaddressable::Startup(refused) => refuse_startup(refused),
    }
}

/// A devpod call that never ran at all.
///
/// The one refusal worth its own exit code is again a devpod that is not on PATH;
/// everything else is a one-line diagnostic naming the call that did not happen,
/// where Python raised out of `subprocess` and printed a traceback (divergence row
/// 4).
fn refuse_devpod(call: &str, refused: &NotRun) -> Ending {
    eprintln!("{}", render::devpod_not_run(call, refused));
    if matches!(refused, NotRun::NotInstalled) {
        Ending::DevpodMissing
    } else {
        Ending::Refused
    }
}

fn refuse_startup(refused: &StartupError) -> Ending {
    eprintln!("error: {}", render::startup_reason(refused));
    Ending::Refused
}

/// Everything a metadata load and the cache migration had to say.
pub(crate) fn report(records: &Records<'_>) {
    for line in render::metadata_notices(&records.notices) {
        eprintln!("{line}");
    }
    // The cache migration's notices, said after the load's and before any refusal:
    // Python's factory ran the load then `migrate_cache`, which announced inside
    // itself. On an already-current cache there is no report and nothing to say.
    if let Some(report) = &records.migration {
        for line in render::migration_notices(report) {
            eprintln!("{line}");
        }
    }
    if let Some(refused) = &records.migration_refused {
        eprintln!(
            "Could not migrate the workspace cache: {}",
            render::metadata_error(refused)
        );
    }
}
