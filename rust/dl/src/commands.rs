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
use devlaunch_core::clients::devpod_home::DevpodHome;
use devlaunch_core::domain::config;
use devlaunch_core::domain::spec::DevcontainerPath;
use devlaunch_core::domain::workspace_id::WorkspaceId;
use devlaunch_core::domain::xdg;
use devlaunch_core::flows::completion::{self, FileState, InstallError, Installed, RcChange};
use devlaunch_core::flows::completion_cache::{self, Refreshed};
use devlaunch_core::flows::kept_copies::KeptCopies;
use devlaunch_core::flows::kill;
use devlaunch_core::flows::launch::{ColdPath, LaunchNotice};
use devlaunch_core::flows::lifecycle::{
    self, ChildWork, DeleteStalled, Insistence, LifecycleNotice, PruneError, PruneOutcome, Refresh,
    RefreshReason, Removal, RemoveOutcome, StopOutcome,
};
use devlaunch_core::flows::listing::{self, CommandContext, DlView, Sizes};
use devlaunch_core::flows::records::{Records, StartupError, open_records, open_storage};
use devlaunch_core::flows::repo_manager::CacheNotice;
use devlaunch_core::runner::{Exit, Runner};

use crate::cli::{self, Command, ListOutput, RmOnExit, Verb};
use crate::hangup;
use crate::launch::{self, Family, Reached};
use crate::render;
use crate::render::Swept;
use crate::select;
use crate::target::{self, Unaddressable, Vetting};

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
        Command::Prune { yes, force } => render_prune(runner, &mut context, cache, yes, force),
        Command::Reconcile { yes } => render_reconcile(runner, &mut context, refresh, yes),
        Command::Purge { yes } => render_purge(&mut context, cache, yes),
        // The two arms that name a verb are the two that can be `rme`, and the
        // hangup is asked *here* rather than inside either of them: a picked batch
        // is one command over several workspaces, and the shell it was typed in is
        // hung up once, when the last of them has gone. See [`crate::hangup`].
        Command::Select { verb, devcontainer } => {
            let after = verb.after_removal();
            let ending = render_select(
                runner,
                &mut context,
                cache,
                refresh,
                verb,
                devcontainer.as_ref(),
            );
            hangup::after_the_command(after, ending)
        }
        Command::Workspace {
            target,
            verb,
            devcontainer,
        } => {
            let after = verb.after_removal();
            let ending = render_workspace(
                runner,
                &mut context,
                cache,
                refresh,
                &target,
                verb,
                devcontainer.as_ref(),
                // A target named on the command line is resolved by the launch
                // itself; only the picker arrives knowing more than it says.
                None,
            );
            hangup::after_the_command(after, ending)
        }
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

/// The human table, and whatever the last background sweep left in the record.
///
/// Still one devpod round trip, still no config and no cache migration — the one
/// thing it reads besides devpod is `metadata.json`, and only for the sweep notes
/// under the table. That read is the point of devlaunch#480: the sweep is a
/// detached child whose stderr is `/dev/null`, so the record is the only place a
/// complaint of its can be left, and `--ls` is the only surface anybody reads.
fn render_table(context: &mut CommandContext<'_>, cache: &Path, sizes: Sizes) -> Ending {
    let table = match listing::workspace_table(context, cache, sizes) {
        Err(refused) => return refuse_listing(&refused),
        Ok(table) => table,
    };
    for line in render::table_lines(&table, sizes) {
        println!("{line}");
    }
    say_sweep_notes();
    Ending::Done
}

/// The lines under the table, on stderr where every other notice goes.
///
/// A record that will not open costs the notes and nothing else: `--ls` answers
/// out of devpod, and a listing that refused because a note could not be fetched
/// would be the tail wagging the dog. The load's own notices are said, so a
/// `metadata.json` this run quarantines is never moved aside in silence.
fn say_sweep_notes() {
    let Ok((storage, notices)) = open_storage() else {
        return;
    };
    for line in render::metadata_notices(&notices) {
        eprintln!("{line}");
    }
    for line in render::sweep_notes(&listing::outstanding_sweep_notes(&storage)) {
        eprintln!("{line}");
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
    let records = match open_records(runner) {
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
            let mut records = match open_records(runner) {
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
    let refreshed =
        completion_cache::update_completion_cache(context, cache, &xdg::clone_root_in(cache));
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
    recognised: Option<WorkspaceId>,
) -> Ending {
    // The open's own notices are said where they happen, by the same printer every
    // other notice goes through: a damaged `metadata.json` has something to say
    // before the verb it is holding up gets anywhere.
    let mut opening = render::Saying;
    let mut cold = ColdPath::new(runner, &mut opening);
    // The word the line used, asked of the verb rather than written out per arm.
    // Two of the three arms are one word each and could be spelled here, but
    // `Family::Remove` covers both `rm` and `rme`, and a notice that quotes a word
    // the line does not carry explains nothing about the line.
    let word = verb.word();
    match launch::family(&verb) {
        Family::Stop => {
            devcontainer_ignored(devcontainer.is_some(), word);
            render_stop(runner, context, refresh, &mut cold, target)
        }
        Family::Kill => {
            devcontainer_ignored(devcontainer.is_some(), word);
            render_kill(runner, context, cache, refresh, &mut cold, target, word)
        }
        Family::Remove { force } => {
            devcontainer_ignored(devcontainer.is_some(), word);
            render_remove(
                runner,
                context,
                cache,
                refresh,
                &mut cold,
                target,
                removal_of(force),
                word,
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
                recognised,
            );
            after_the_session(runner, context, cache, refresh, &mut cold, target, rm, ran)
        }
    }
}

/// Whether a sweep already ran in front of this removal.
///
/// Read off the removal rather than passed beside it, because it is a total
/// function of one: [`Removal::Wedged`] *is* the removal that stands behind a
/// sweep. As a second argument it was a pair that could be written wrong in four
/// ways, and the wrong one produces a `kill` whose own refusal tells you to run
/// `kill`.
///
/// A free function rather than a method, because [`Removal`] is core's now: which
/// of the three removals this is belongs to the removal, and whether a sweep has
/// already printed above belongs to the screen.
fn swept(removal: Removal) -> Swept {
    match removal {
        Removal::Guarded | Removal::Insisted => Swept::NotYet,
        Removal::Wedged => Swept::Already,
    }
}

/// The removal `--force` asks for, for the one verb the flag still reaches.
fn removal_of(force: bool) -> Removal {
    if force {
        Removal::Insisted
    } else {
        Removal::Guarded
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
/// `clients::devpod_home::create_record` exists at all — so an unattended
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
    cold: &mut ColdPath<'r, '_>,
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
        // The guarded one, deliberately: `--rm` is the throwaway workspace and this
        // is still the moment work that exists nowhere else would be destroyed by a
        // flag typed before the session began, so it stops exactly where the verb
        // does.
        Removal::Guarded,
        // `rm`, not the flag: this removal is `--rm`'s, and the way past a guard
        // that refuses it is the verb — the flag deliberately does not take
        // `--force` (see the grammar's `RmForced`), so the line it offers must be
        // one that does.
        "rm",
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
    cold: &mut ColdPath<'r, '_>,
    target: &str,
) -> Ending {
    let addressed = match target::resolve(runner, context, cold, target, Vetting::ByDevpod) {
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

/// `dl <ws> kill` — the hammer, and then the workspace.
///
/// **The removal is the verb's second half rather than a convenience bolted to
/// it**, and the order is the argument for putting it here: the sweep is what
/// clears the lock, and clearing the lock is what lets a `devpod delete` through.
/// Typing `kill` and then `rm` was two commands because the first one had no way
/// to finish the thought — and on the run that prompted this, the first one did
/// nothing at all and the second one deleted the workspace unaided, which is two
/// commands to reach the state either of them was asked for.
///
/// **Withheld over whatever still holds the workspace's lock**
/// ([`kill::Sweep::blocks_a_delete`]), which is a question about the host rather
/// than about what this sweep managed to signal. A session with somebody behind it
/// is not that: it takes the flock and gives it back, which is why `dl <ws> rm`
/// deletes a workspace somebody is sitting in. A live `devpod up` and an orphan
/// both are. [`kill::Standing`] is where that judgement lives, and it lives there
/// rather than here because the sweep above already acts on the same distinction
/// when it decides what to signal.
///
/// **A [`Refresh`], which this verb used not to carry.** It removes a workspace
/// now, so the completion cache's listing goes stale on it exactly as `rm`'s does.
/// The reason it carried none before was that it changed nothing devpod records,
/// and that reason is spent.
///
/// **The target is resolved without asking devpod anything, wherever it can be.**
/// Everything else in the family resolves through a `devpod status`, and here
/// that would be the one call the verb must not make: the workspace somebody
/// types this at is the workspace whose devpod has stopped answering, and a
/// `status` on it has no deadline behind it. A bare workspace id needs no round
/// trip at all — the name *is* the id, and [`target::Vetting::Unnecessary`] is
/// what says so — while `dl owner/repo kill` still has to ask devpod which
/// workspace the triple resolved to, and that ask carries a deadline and falls
/// back to the derived id rather than refusing when it runs out. The resolution
/// is still the shared one, so it cannot disagree with `stop`'s about which
/// workspace this is; what changed is only how long it may take and how much of
/// it is worth a round trip.
#[allow(clippy::too_many_arguments)]
fn render_kill<'r>(
    runner: &'r dyn Runner,
    context: &mut CommandContext<'r>,
    cache: &Path,
    refresh: &mut Refresh<'_>,
    cold: &mut ColdPath<'r, '_>,
    target: &str,
    word: &str,
) -> Ending {
    let addressed = match target::resolve(runner, context, cold, target, Vetting::Unnecessary) {
        Err(refused) => return refuse_target(&refused),
        Ok(addressed) => addressed,
    };
    say_launch(&addressed.notices);
    let workspace_id = addressed.workspace_id;
    eprintln!("{}", render::killing(&workspace_id));
    let killed = kill::workspace_kill(
        runner,
        // Resolved here rather than in core, as every other environment answer
        // is. `None` is a machine with no home directory, where devpod keeps no
        // records and so addresses no busy marker.
        DevpodHome::locate().as_ref(),
        &workspace_id,
        // The grace period, really spent. Core takes it as a function so its own
        // tests do not have to.
        &mut std::thread::sleep,
    );
    let sweep = match killed {
        kill::Killed::Unavailable(cannot) => {
            eprintln!("{}", render::kill_unavailable(&cannot));
            return Ending::Refused;
        }
        kill::Killed::Swept(sweep) => sweep,
    };
    for line in render::killed(&workspace_id, &sweep) {
        eprintln!("{line}");
    }
    if sweep.blocks_a_delete() {
        eprintln!("{}", render::kill_delete_withheld(&workspace_id));
        return Ending::Refused;
    }
    // The exit code from here down is the removal's, which is the change of meaning
    // the second half brings: it used to say whether the workspace was free. A
    // `dl <ws> kill && ...` that read the old sense reads the stronger one now, since
    // a workspace that has been deleted is not held by anything.
    remove_addressed(
        context,
        cache,
        refresh,
        cold,
        &workspace_id,
        target,
        Removal::Wedged,
        word,
    )
}

/// `dl <ws> rm [--force]` — the guard, the delete, and the clone with it.
#[allow(clippy::too_many_arguments)]
fn render_remove<'r>(
    runner: &'r dyn Runner,
    context: &mut CommandContext<'r>,
    cache: &Path,
    refresh: &mut Refresh<'_>,
    cold: &mut ColdPath<'r, '_>,
    target: &str,
    removal: Removal,
    word: &str,
) -> Ending {
    let addressed = match target::resolve(runner, context, cold, target, Vetting::ByDevpod) {
        Err(refused) => return refuse_target(&refused),
        Ok(addressed) => addressed,
    };
    say_launch(&addressed.notices);
    remove_addressed(
        context,
        cache,
        refresh,
        cold,
        &addressed.workspace_id,
        target,
        removal,
        word,
    )
}

/// The guard and the delete, for a workspace something else has already named.
///
/// Split from [`render_remove`] for one caller, and the split is exactly where it
/// is because of what that caller may not do: `kill` resolves its target with
/// [`Vetting::Unnecessary`] on purpose — the workspace it is typed at is the one
/// whose devpod has stopped answering, and `render_remove`'s [`Vetting::ByDevpod`]
/// is a `devpod status` with nothing behind it. So the resolution is the half that
/// differs and everything from the records down is the half that must not: one
/// guard, one delete, one set of lines, whichever word reached them.
#[allow(clippy::too_many_arguments)]
fn remove_addressed<'r>(
    context: &mut CommandContext<'r>,
    cache: &Path,
    refresh: &mut Refresh<'_>,
    cold: &mut ColdPath<'r, '_>,
    workspace_id: &str,
    target: &str,
    removal: Removal,
    word: &str,
) -> Ending {
    // The removal needs the records whatever the resolution needed, so a resolution
    // that did not open them opens them here — through the same `ColdPath`, which is
    // what keeps one command from holding two views of `metadata.json`.
    let records = match cold.records() {
        Err(refused) => return refuse_startup(&refused),
        Ok(records) => records,
    };
    let Records {
        storage, clones, ..
    } = records;
    // Said as they happen rather than collected, because the removal's own lines are
    // interleaved with the guard's and the clone's and the order *is* the report:
    // what was found, which workspace is going, and what became of its clone. A
    // vector here would print all of it after the delete had already returned.
    let mut notices = render::Saying;
    let removed = lifecycle::workspace_remove(
        context,
        refresh,
        clones,
        storage,
        cache,
        // Resolved here rather than in core, for the reason every other environment
        // answer is: the process that knows what its environment says hands the
        // answer down. `None` is a machine with no home directory, where devpod has
        // no records to read and so no volume names to derive.
        DevpodHome::locate().as_ref(),
        // The same cache directory everything else here hangs off, so the copy this
        // delete drops is the one a launch of this workspace wrote.
        &KeptCopies::under(cache),
        workspace_id,
        removal,
        // Printed from inside the call rather than said through the sink beside it,
        // because the whole value of the sentence is its timing: the delete it is
        // about has not returned and, until somebody acts on this, is not going to.
        &mut |DeleteStalled::OnTheLock| eprintln!("{}", render::delete_blocked(workspace_id, word)),
        &mut notices,
    );
    match removed {
        Err(not_run) => {
            let ending = refuse_devpod("delete", &not_run);
            // The one refusal whose own sentence describes nothing: `devpod delete
            // did not answer in time` is true of a deadline that only this
            // removal sets, and it leaves unsaid the two things somebody has to
            // know next — that the workspace and its clone are still there, and
            // that a devpod killed a minute in may have got part of the way.
            if let NotRun::TimedOut = not_run {
                eprintln!("{}", render::delete_timed_out(workspace_id));
            }
            ending
        }
        // The one thing dl refuses on its own account. Only `rm` reaches this arm:
        // `rm --force` never looks and `kill` says it and goes on, both of which are
        // decided inside the removal now rather than sequenced out here.
        Ok(RemoveOutcome::Refused(refusal)) => {
            eprintln!("{}", render::removal_refusal(&refusal, target, word));
            Ending::Refused
        }
        Ok(RemoveOutcome::DevpodRefused { exit }) => {
            // The local clone is kept, so the delete stays retryable: devpod
            // re-parses the workspace's devcontainer.json to tear the container
            // down, and removing the clone regardless strands it for good.
            eprintln!("{}", render::delete_refused(workspace_id, swept(removal)));
            Ending::Child(exit)
        }
        Ok(RemoveOutcome::Deleted { .. }) => {
            // After the clone's own lines, because it closes the removal: what a
            // reader wants from the end of one workspace's block is which workspace
            // it was. The insistence is passed because it decides what this exit code
            // established — see [`render::removed`].
            eprintln!("{}", render::removed(workspace_id, removal.insistence()));
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

/// `worktree.repos_dir`'s notice on a path that opens no records (devlaunch#461).
///
/// [`report`] says this for every command that opens dl's records, which is where
/// it belongs and is not here: a purge reads `metadata.json` for nothing, and
/// opening it would run the cache migration, writing records into the tree this
/// command is about to remove and into one an aborted purge was asked to leave
/// alone. So the config is read on its own, which creates nothing and touches
/// nothing.
///
/// It is worth saying *here* in particular, and #467 left the decision to this
/// ticket. A clone under that retired root is not devlaunch's by the only test
/// `--purge` has, so the purge leaves it and removes the record that was the last
/// thing pointing at it. Where a workspace still opens such a clone the leaving
/// list now names the path; where none does, this line is the only mention that
/// tree will ever get.
///
/// A `config.toml` that cannot be read says nothing rather than refusing: a purge
/// does not otherwise need the file, and the next command that opens dl's records
/// is where a broken config is somebody's problem.
fn say_retired_keys() {
    if let Ok((_, retired)) = config::worktree_config() {
        for line in render::retired_keys(&retired) {
            eprintln!("{line}");
        }
    }
}

fn purge_devlaunch_data(context: &mut CommandContext<'_>, cache: &Path, yes: bool) -> Cleanup {
    say_retired_keys();
    let plan = match lifecycle::purge_plan(context, cache) {
        Err(refused) => return Cleanup::Raised(refuse_listing(&refused)),
        Ok(plan) => plan,
    };
    // The plan is told which of the two it is. Everything in it is preventable
    // while the question is still coming, and one sentence of it offers an action
    // that only an interactive reader can still take.
    let confirmation = if yes {
        render::Confirmation::AnsweredOnTheLine
    } else {
        render::Confirmation::WillBeAsked
    };
    print(&render::purge_plan_lines(&plan, confirmation));
    if !yes && !confirmed("Are you sure? [y/N] ") {
        println!("Aborted.");
        return Cleanup::Ended(Ending::Done);
    }
    // Read after the question, because a purge that was declined asks devpod
    // nothing and reads nothing.
    let devpod_home = DevpodHome::locate();
    let purged = lifecycle::purge_all_data(context, &plan, devpod_home.as_ref(), &mut |step| {
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
    cache: &Path,
    yes: bool,
    force: bool,
) -> Ending {
    prune_clone_directories(runner, context, cache, yes, force).with_the_boundary()
}

fn prune_clone_directories(
    runner: &dyn Runner,
    context: &mut CommandContext<'_>,
    cache: &Path,
    yes: bool,
    force: bool,
) -> Cleanup {
    let mut records = match open_records(runner) {
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
    // devlaunch's copies of what devpod substituted, under the cache the binary
    // already resolved. This is what makes a run pointed at a scratch
    // `XDG_CACHE_HOME` find no copies and so remove no volume.
    let copies = KeptCopies::under(cache);
    let plan = match lifecycle::prune_plan(
        &records.clones,
        &records.storage,
        &workspaces,
        &copies,
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
    let acted = lifecycle::prune_clones(context, clones, storage, &copies, &plan, &mut notices);
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
    let mut records = match open_records(runner) {
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
    let Some(devpod_home) = DevpodHome::locate() else {
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
            // stderr with the refusals rather than stdout with the adoptions:
            // the workspace opens the right clone now, but dl's half of the
            // repair did not happen, and the exit code says so.
            lifecycle::Adoption::Unrecorded { workspace_id } => {
                eprintln!("Re-pointed {workspace_id}; dl's own record was not updated")
            }
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
        select::Pick::Chose(chosen) => {
            // Said before the first workspace is touched, because skim has taken its
            // screen back by now and nothing else in the run ever puts the row and
            // the workspace id side by side — the id has no owner in it, and the row
            // is not what devpod is addressed by.
            for line in render::picked(verb.word(), &chosen) {
                eprintln!("{line}");
            }
            let mut ending = Ending::Done;
            for (already_acted, pick) in chosen.iter().enumerate() {
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
                    &pick.workspace_id,
                    verb.clone(),
                    devcontainer,
                    // The picker knows what it drew: this row's clone said it is
                    // this triple, and the launch it is about to start knows only
                    // the id. See `Launch::recognised_as`.
                    pick.triple.clone(),
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

/// Everything the config load, the metadata load and the cache migration had to
/// say.
///
/// For the commands that open the records directly rather than through a
/// [`ColdPath`] — which says the same notices through [`render::Saying`] at the
/// moment the open happens. The order is core's, and it is the order Python's
/// factory produced them in: the config's retired keys, then the load's notices,
/// then the migration's, then any refusal of it.
pub(crate) fn report(records: &Records<'_>) {
    for notice in &records.reported {
        for line in render::records_notice(notice) {
            eprintln!("{line}");
        }
    }
}
