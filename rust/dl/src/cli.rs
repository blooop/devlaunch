//! The argument grammar: argv in, one [`Command`] out.
//!
//! Two layers, and the split is the point. [`Cli`] is what clap accepts — a flat
//! bag of flags and up to two positional words, which is the shape a
//! command-line *is*. [`Command`] is what `dl` can be asked to do: one arm per
//! command, each carrying only what that command makes meaningful. [`resolve`] is
//! the one function between them, and it is pure, so every grammar decision is a
//! table a test can read.
//!
//! Adding a command means adding an arm, which breaks the renderer until it is
//! handled — the obligation the dispatcher exists to propagate.
//!
//! # Where this differs from Python, deliberately
//!
//! Three numbered rows of docs/rust-rewrite-plan.md land here:
//!
//! - **Row 1.** The verbs (`up`, `stop`, `rm`, …) are reserved words: `dl stop`
//!   is the stop verb with no target, not a workspace called `stop`. Python read
//!   the first word as a spec always, so `dl stop` looked for a workspace of that
//!   name. A verb with no target opens the fuzzy selector (M8).
//! - **Row 2.** clap's strictness: an unknown flag is refused rather than read as
//!   a workspace name, and there is no argparse-style prefix abbreviation. Where
//!   Python ignored a flag that did not apply (`dl --ls --json extra`), this
//!   refuses it.
//! - **Row 3.** `--help` is clap's layout, not the hand-rolled text.

use std::path::PathBuf;

use clap::Parser;
use devlaunch_core::domain::spec::{self, DevcontainerPath, DevcontainerRefError};
use devlaunch_core::domain::workspace_state::NonEmpty;
use devlaunch_core::flows::listing::Sizes;

/// The reserved verb words, and the verb each one names.
///
/// One table, read by both the workspace-first and the verb-first arm, so a word
/// cannot be a verb in one position and a workspace name in the other.
const VERBS: [(&str, VerbWord); 10] = [
    ("up", VerbWord::Up),
    ("stop", VerbWord::Stop),
    ("kill", VerbWord::Kill),
    ("rm", VerbWord::Remove),
    ("rme", VerbWord::RemoveAndExit),
    ("code", VerbWord::Code),
    ("recreate", VerbWord::Recreate),
    ("restart", VerbWord::Restart),
    ("reset", VerbWord::Reset),
    ("dotfiles", VerbWord::Dotfiles),
];

/// Words that were verbs once and are not any more.
///
/// **Recognised rather than forgotten, and that is the whole point of the table.**
/// A retired word simply dropped from [`VERBS`] would be read as a *workspace
/// name* — `dl prune <ws>` would report an unknown workspace called `prune`, naming
/// the word that moved as if it were the thing being looked for. Kept here, the word
/// still cannot be a target, and still says what to type instead.
const RETIRED: [(&str, RetiredWord); 1] = [("prune", RetiredWord::Prune)];

/// The flags that were commands once and are not any more.
///
/// **Recognised rather than removed, for the same reason [`RETIRED`] keeps `prune`.**
/// A flag simply deleted from [`Cli`] is clap's `unexpected argument` at exit 2, which
/// names the spelling and nothing else — and both of these moved *because* `--rm`
/// changed meaning, so the spelling is exactly what cannot explain itself. Kept here,
/// each one is refused at exit 1 by a sentence that says what to type.
///
/// Both are hidden on [`Cli`]: they are not options this build offers, only spellings
/// it still answers for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetiredFlag {
    /// `--stop`, the flag spelling of the `stop` verb.
    ///
    /// It went with row 30's suffix form rather than on its own account. The whole
    /// point of a flag-spelled verb was that it could be *appended* to a recalled
    /// line and beat it; once `--rm` means "delete when the session ends", a flag
    /// that instead cancels the line is the one thing that pair must not be —
    /// `--rm` and `--stop` look alike, and would then act oppositely.
    Stop,
    /// `--autorm`, which is now spelled `--rm`.
    Autorm,
}

/// A word this build no longer accepts as a verb.
///
/// An enum rather than a string, so the sentence each one needs is an exhaustive
/// match a compiler enforces: retiring a second word breaks the renderer until
/// somebody writes what to say about it, which is the obligation this module's docs
/// say the dispatcher exists to propagate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RetiredWord {
    /// `prune` as a workspace verb — Python's second spelling of `rm`.
    ///
    /// It went because of the collision, not because the spelling was redundant:
    /// `dl <ws> prune` deleted one workspace, and `dl --prune` removes clone
    /// directories and no workspace at all. One word, two unrelated commands, told
    /// apart by two dashes — and a person reaching for the wrong one either loses a
    /// workspace they meant to keep or is refused for a reason the message could
    /// not explain (`--prune takes no workspace: it is not a workspace command.`).
    /// `rm` had no such twin. Divergence row 31.
    Prune,
}

impl RetiredWord {
    fn of(word: &str) -> Option<Self> {
        RETIRED
            .iter()
            .find(|(spelling, _)| *spelling == word)
            .map(|(_, retired)| *retired)
    }

    /// The word to type instead.
    pub(crate) fn instead(self) -> &'static str {
        match self {
            Self::Prune => "rm",
        }
    }

    /// The retired spelling itself, for the diagnostic that quotes it.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Prune => "prune",
        }
    }
}

/// A verb as the *word* names it, before `--force` is folded in.
///
/// Separate from [`Verb`] because the table above cannot know whether `--force`
/// was given, and `Verb::Remove` must not be constructible without an answer to
/// that question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VerbWord {
    Up,
    Stop,
    Kill,
    Remove,
    RemoveAndExit,
    Code,
    Recreate,
    Restart,
    Reset,
    Dotfiles,
}

impl VerbWord {
    fn of(word: &str) -> Option<Self> {
        VERBS
            .iter()
            .find(|(spelling, _)| *spelling == word)
            .map(|(_, verb)| *verb)
    }

    fn with(self, force: bool) -> Verb {
        match self {
            Self::Up => Verb::Up,
            Self::Stop => Verb::Stop,
            Self::Kill => Verb::Kill,
            Self::Remove => Verb::Remove {
                force,
                after: AfterRemoval::LeaveTheShell,
            },
            Self::RemoveAndExit => Verb::Remove {
                force,
                after: AfterRemoval::HangUpTheShell,
            },
            Self::Code => Verb::Code,
            Self::Recreate => Verb::Recreate,
            Self::Restart => Verb::Restart,
            Self::Reset => Verb::Reset,
            Self::Dotfiles => Verb::Dotfiles,
        }
    }
}

/// Whether the workspace is deleted once the session it handed over has ended.
///
/// A named pair rather than a bool, and carried by the two arms of [`Verb`] the flag
/// is *defined* for — which is what makes `dl <ws> code --rm` unwritable rather than
/// merely refused, and what stops every other arm carrying a field the dispatcher has
/// to remember to ignore.
///
/// **This is `--rm`, and [`Verb::Remove`] is `rm`** — docker's split, which is where
/// the spelling comes from: `docker rm` deletes now, `docker run --rm` deletes once
/// what it ran has finished, and no docker subcommand takes a `--rm` meaning the
/// first of those. So here too the word is the only way to say "delete it now", the
/// flag the only way to say "delete it after", and neither spelling has to be read
/// twice to find out which was meant. Divergence row 32.
///
/// **Two arms and not five, and that is a scope decision rather than a mechanical
/// one.** `restart`, `recreate` and `reset` also end in a session
/// (`LaunchVerb::attaches`), so the removal would work behind them; they are out
/// because `--rm` means *the throwaway workspace* — open one, use it, let it go —
/// and not *clean up after whichever verb this was*. Widening it later is adding
/// arms here; the refusal names the two forms rather than claiming those verbs hand
/// over nothing, because they do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RmOnExit {
    /// `--rm`: remove the workspace when the session ends, guard and all.
    Yes,
    /// The default: the workspace outlives the session, as it always has.
    No,
}

/// What becomes of the shell that asked, once the removal is over.
///
/// A named pair rather than a bool, and it rides on [`Verb::Remove`] rather than
/// beside it, because there is exactly one verb it is defined for: `rme` is `rm`
/// and then the shell, and every other verb hands the shell back the way it found
/// it.
///
/// **This is a third thing, and not a third spelling of the two above.**
/// [`Verb::Remove`] deletes the workspace now, [`RmOnExit`] deletes it when a
/// session ends, and this one deletes it now and then ends the *shell* — which is
/// why it is the only one of the three that is not docker's. What it is for is the
/// terminal tab opened for one workspace: the removal takes a while, and the tab
/// has nothing left to do afterwards but be closed by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterRemoval {
    /// `rm`: nothing. `dl` exits and the shell it was called from carries on.
    LeaveTheShell,
    /// `rme`: SIGHUP to the process that started `dl`, so an interactive shell
    /// ends and the terminal it was sitting in goes with it. Only after a removal
    /// that worked; see [`crate::hangup`], which owns both halves of that.
    HangUpTheShell,
}

/// What is being asked of one workspace.
///
/// [`Verb::Run`] carries a [`NonEmpty`] rather than a `Vec`, because `dl <ws> --`
/// with nothing after it is [`Verb::Attach`] and not a run of the empty command —
/// which is exactly what Python decides (`len(args) > 2`), and what an empty
/// vector beside an `Attach` arm could not say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verb {
    /// `dl <ws>` — bring it up and hand over an interactive shell.
    Attach {
        rm: RmOnExit,
    },
    /// `dl <ws> -- <command>` — one command, inside the workspace.
    Run(NonEmpty<String>, RmOnExit),
    Up,
    Stop,
    /// `kill` — the hammer for a workspace that will not answer.
    ///
    /// Carries nothing, because it is asked of the host rather than of devpod:
    /// there is no `--force` to fold in and no state to consult first. `stop` is
    /// the polite version and needs a devpod that answers; this one is what is
    /// left when it does not.
    Kill,
    /// `rm`. `force` is `--force`: delete despite unsaved work, and count an
    /// already-absent workspace as deleted. `after` is which of the two words
    /// this was — `rm` or `rme` — and so what happens to the calling shell once
    /// the removal is done.
    ///
    /// One arm for both words rather than two, because they ask for the same
    /// removal: `rme` adds something *after* it, and a second arm would be a
    /// second copy of every decision the delete makes.
    Remove {
        force: bool,
        after: AfterRemoval,
    },
    Code,
    Recreate,
    Restart,
    Reset,
    Dotfiles,
}

impl Verb {
    /// Whether the selector may hand this verb several workspaces at once.
    ///
    /// Yes for the verbs that finish on their own: `up`, `stop`, `kill`, `rm`,
    /// `rme`, `code` and `dotfiles` apply to each workspace in turn and return, so
    /// `dl rm` can mark five dead workspaces and clear them in one visit — the
    /// same TAB-to-mark batch `fzf --multi` taught everyone. `rme` is on that list
    /// because the batch is exactly what it is for: five removals is the longest
    /// wait there is, and the shell it hangs up is hung up once, when the last of
    /// them has gone. `kill` is on that
    /// list on its own evidence rather than by resemblance: a machine that has
    /// been suspended, or one whose `dl` was killed by an OOM, wedges every
    /// workspace that was open at the time, and clearing them one line at a time
    /// is the visit this exists to save. That is the reason to want it; what
    /// makes it safe is narrower, and it is the sweep's own scope: each one only
    /// signals processes naming *its* workspace and spares anything with a live
    /// parent behind it, so five at once is five independent sweeps rather than
    /// one wider one. No for anything that ends in an
    /// interactive session — attach, `--`, and the three rebuild verbs, whose
    /// launch attaches when it is done (`LaunchVerb::attaches`): several of those
    /// would be sessions run back to back, each waiting on the last one's exit,
    /// which is a queue nobody asked the picker for.
    /// Exhaustive rather than a `matches!` with a default, so a new verb does not
    /// get single-select by omission: whoever adds the arm answers the question.
    pub(crate) fn several_at_once(&self) -> bool {
        match self {
            Verb::Up
            | Verb::Stop
            | Verb::Kill
            | Verb::Remove { .. }
            | Verb::Code
            | Verb::Dotfiles => true,
            Verb::Attach { .. } | Verb::Run(..) | Verb::Recreate | Verb::Restart | Verb::Reset => {
                false
            }
        }
    }

    /// The word this verb is spelled with, for a diagnostic that names it.
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Verb::Attach { .. } => "attach",
            Verb::Run(..) => "--",
            Verb::Up => "up",
            Verb::Stop => "stop",
            Verb::Kill => "kill",
            // The word that was typed, because this is what the diagnostics quote:
            // `dl <ws> rme --rm` has to be refused in the spelling the line used.
            Verb::Remove { after, .. } => match after {
                AfterRemoval::LeaveTheShell => "rm",
                AfterRemoval::HangUpTheShell => "rme",
            },
            Verb::Code => "code",
            Verb::Recreate => "recreate",
            Verb::Restart => "restart",
            Verb::Reset => "reset",
            Verb::Dotfiles => "dotfiles",
        }
    }

    /// What this verb asks of the calling shell once it is done.
    ///
    /// Only `rme` asks for anything, and the question is asked of the *verb* rather
    /// than read off the word, so a batch of picked rows and a named target answer
    /// it the same way. Exhaustive over the one arm that can say yes: a second verb
    /// that ends the shell would have to add itself here.
    pub(crate) fn after_removal(&self) -> AfterRemoval {
        match self {
            Verb::Remove { after, .. } => *after,
            Verb::Attach { .. }
            | Verb::Run(..)
            | Verb::Up
            | Verb::Stop
            | Verb::Kill
            | Verb::Code
            | Verb::Recreate
            | Verb::Restart
            | Verb::Reset
            | Verb::Dotfiles => AfterRemoval::LeaveTheShell,
        }
    }
}

/// Whether `--ls` was asked for a table or the JSON document.
///
/// A named pair rather than a bool at the call site: the two are different
/// renderings of the same reading, and one of them is a wire format `wf` parses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListOutput {
    Table,
    Json,
}

/// One `dl` command line, resolved.
///
/// Every arm is a command the dispatcher must handle, and nothing else is
/// representable: there is no "some flags were set" state to interpret later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// `dl --version`
    Version,
    /// `dl --ls [--json] [--size]`
    List { output: ListOutput, sizes: Sizes },
    /// `dl --repos` — the known `owner/repo` strings, for completion.
    Repos,
    /// `dl --completion-data` — the whole completion cache, as one JSON line.
    CompletionData,
    /// `dl --update-cache [--force]` — the silent background refresh.
    UpdateCache { force: bool },
    /// `dl --refresh` — the same refresh, with feedback.
    Refresh,
    /// `dl --install [<rc-file>]`
    Install { rc: Option<PathBuf> },
    /// `dl --prune [-y] [--force]`
    Prune { yes: bool, force: bool },
    /// `dl --reconcile [-y]`
    Reconcile { yes: bool },
    /// `dl --purge [-y]`
    Purge { yes: bool },
    /// A verb with no workspace named: the fuzzy selector picks one (M8) — or,
    /// for a verb that applies per workspace ([`Verb::several_at_once`]), several.
    Select {
        verb: Verb,
        devcontainer: Option<DevcontainerPath>,
    },
    /// A workspace, and what to do with it.
    Workspace {
        target: String,
        verb: Verb,
        devcontainer: Option<DevcontainerPath>,
    },
}

/// A command line clap accepted but `dl` cannot make a command of.
///
/// Kept apart from clap's own errors because the two exit differently: clap's
/// usage errors are exit 2, its convention, while these are the shapes Python
/// also refused and refused with exit 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GrammarError {
    /// Two words, neither of which is a verb.
    UnknownVerb { target: String, word: String },
    /// A word that was a verb in an earlier build and is not one now.
    RetiredVerb(RetiredWord),
    /// A flag that was a command in an earlier build and is not one now.
    RetiredFlag(RetiredFlag),
    /// A global command was given a workspace to act on.
    TargetNotAllowed { command: &'static str },
    /// A modifier that means nothing for the command it was given to.
    ModifierNotAllowed {
        modifier: &'static str,
        command: &'static str,
    },
    /// `-- <command>` beside a verb that does not run one.
    CommandNotAllowed { verb: &'static str },
    /// `--devcontainer` on a command that opens no workspace.
    DevcontainerNotAllowed { command: &'static str },
    /// `--rm` on a command the flag is not defined for.
    ///
    /// Refused rather than ignored, and `code` is why. `dl <ws> code --rm`
    /// returns the moment devpod has told VS Code where to connect — *before* the
    /// editor is attached — so honouring it there would delete the container out
    /// from under a window that is still opening. A silently-dropped flag would
    /// read as "it ran and kept the workspace", which is the one reading that
    /// makes somebody type it again.
    ///
    /// `dl <ws> rm --rm` reaches this too, and the sentence is the useful one there:
    /// the verb alone already deletes the workspace.
    RmNotAllowed { command: &'static str },
    /// `--force` beside `--rm`.
    ///
    /// The pair looks like it should compose and must not: the unsaved-work guard
    /// is the whole reason `--rm` is safe to leave on a recalled line, and a
    /// `--force` habitually appended to that line would destroy work hours later,
    /// unattended, with nobody watching the sentence that explained it.
    ///
    /// docker draws the line in the same place, which is worth knowing because it
    /// makes this a shape rather than a special case: `-f` is `docker rm`'s, and
    /// `docker run --rm` has no forcing flag at all.
    RmForced,
    /// The `--devcontainer` value cannot be a path.
    Devcontainer {
        raw: String,
        why: DevcontainerRefError,
    },
}

/// The flat command line, as clap accepts it.
///
/// Deliberately not a `Subcommand` enum: `dl` is a workspace-first grammar
/// (`dl owner/repo stop`) as well as a verb-first one, and clap's subcommands can
/// only be the first word. The positional words are taken raw and disambiguated
/// by [`resolve`].
#[derive(Debug, Parser)]
#[command(
    name = "dl",
    about = "Open a devcontainer workspace for a repo in one command.",
    long_about = "Open a devcontainer workspace for a repo in one command.\n\n\
        A workspace is one branch of one repo, checked out in its own clone and \
        opened as a devcontainer. `dl owner/repo@branch` creates it if it is not \
        there, starts it if it is stopped, and hands over a shell.",
    // `--version` is dispatched like every other command, so the summary that
    // prints it lives with the rest of the rendering rather than in clap.
    disable_version_flag = true,
    // Two hand-written blocks for both `-h` and `--help`: the verbs are a grammar
    // clap cannot describe from its own arguments — they are positional words — so
    // a help text without them would not document half the CLI, and README's
    // "regenerate against `dl --help`" rule would carry the hole forward.
    //
    // `GRAMMAR` is `before_help` rather than `after_help` so that it lands above
    // the options table, which is what the template below moves it there for: the
    // flags are the part of this CLI a person reaches for least, and clap's default
    // layout puts all of them between the usage line and the verbs and examples
    // somebody opened `--help` to find. The same reordering the README got, for the
    // same reason.
    before_help = GRAMMAR,
    after_help = ENVIRONMENT,
    // clap's own default template with `{before-help}` moved from the top to just
    // above `{all-args}`. Every marker here is one the default already uses — the
    // order is the only change — because an unknown marker is not a compile error,
    // it renders literally.
    help_template = "\
{about-with-newline}
{usage-heading} {usage}

{before-help}{all-args}{after-help}",
)]
pub(crate) struct Cli {
    /// The workspace to open, and optionally the verb to apply: `dl owner/repo`,
    /// `dl owner/repo@branch`, `dl ./path`, `dl <workspace-id> stop`, `dl stop
    /// <workspace-id>`.
    #[arg(value_name = "TARGET|VERB", num_args = 0..=2)]
    words: Vec<String>,

    /// A command to run inside the workspace instead of an interactive shell.
    #[arg(last = true, value_name = "COMMAND")]
    command: Vec<String>,

    /// List every workspace on this machine.
    #[arg(long, group = "what")]
    ls: bool,
    /// With `--ls`: the machine-readable listing, for tools that decide which
    /// workspaces to clean up.
    #[arg(long, requires = "ls")]
    json: bool,
    /// With `--ls`: what deleting each workspace's clone would free. Off by
    /// default — it walks every file in the clone.
    #[arg(long, requires = "ls")]
    size: bool,

    /// Rebuild the completion cache now, with feedback.
    #[arg(long, group = "what")]
    refresh: bool,
    /// Install the shell completions, in the given rc file or `~/.bashrc`.
    #[arg(long, group = "what")]
    install: bool,
    /// Remove the clone directories no workspace opens any more. Prints its plan
    /// and asks first.
    #[arg(long, group = "what")]
    prune: bool,
    /// Re-point devpod workspaces whose recorded source folder is missing. Prints
    /// its plan and asks first; deletes nothing.
    #[arg(long, group = "what")]
    reconcile: bool,
    /// Remove devlaunch's workspaces and caches.
    #[arg(long, group = "what")]
    purge: bool,
    /// Print the version.
    #[arg(long, group = "what")]
    version: bool,

    /// Retired: the flag spelling of the `stop` verb. Recognised so it can be
    /// refused with the word to use instead. Deliberately outside the `what` group,
    /// so `dl --ls --stop` reports the retirement rather than a group conflict about
    /// a flag that no longer names a command.
    #[arg(long, hide = true)]
    stop: bool,
    /// Retired: what `--rm` is now called.
    #[arg(long, hide = true)]
    autorm: bool,

    /// The known `owner/repo` strings, one per line (for shell completion).
    #[arg(long, group = "what", hide = true)]
    repos: bool,
    /// The whole completion cache as one JSON line (for shell completion).
    #[arg(long = "completion-data", group = "what", hide = true)]
    completion_data: bool,
    /// Rebuild the completion cache silently (the background refresh).
    #[arg(long = "update-cache", group = "what", hide = true)]
    update_cache: bool,

    /// Answer yes to the confirmation `--prune`, `--reconcile` and `--purge` ask.
    #[arg(short = 'y', long = "yes")]
    yes: bool,
    /// Go ahead despite work that is nowhere else (`rm`, `--prune`), or refresh
    /// regardless of the cache's age (`--update-cache`).
    #[arg(long)]
    force: bool,
    /// Use a non-default devcontainer.json. A bare name means
    /// `.devcontainer/<name>/devcontainer.json`. Stored with the workspace, so
    /// pass it once.
    #[arg(long, value_name = "VARIANT|PATH")]
    devcontainer: Option<String>,
    /// Delete the workspace once the session ends, like `docker run --rm`. Only
    /// for the two forms that hand one over: `dl <ws>` and `dl <ws> -- <command>`.
    /// Stops at work that is nowhere else, exactly as the `rm` verb does.
    #[arg(long)]
    rm: bool,
}

/// The half of the grammar clap's own argument list cannot show: the verbs are
/// positional words, so nothing in the options table names them.
///
/// Rendered above that options table (`before_help` plus the reordered
/// `help_template` on [`Cli`]), examples first, because this is the half somebody
/// opened `--help` to read.
const GRAMMAR: &str = "Examples:
  dl                                 Pick a workspace interactively
  dl blooop/devlaunch                Open it on its default branch
  dl blooop/devlaunch@fix/123        Open it on that branch, creating it if needed
  dl ./my-project                    Open a local folder
  dl blooop/devlaunch -- make test   Run one command inside the workspace
  dl blooop/devlaunch stop           Stop it
  dl stop blooop-devlaunch-main-1a2b Stop it by workspace id
  dl blooop/devlaunch --rm           Open it, and delete it when the shell exits
  dl --ls --json                     Every workspace, machine-readable

Workspace commands (dl <workspace> <verb>, or dl <verb> <workspace>):
  up                                 Start it without attaching
  stop                               Stop it
  kill                               Kill whatever is holding it, for a workspace
                                     that will not answer and a stop that hangs
                                     with it. Kills the host processes still
                                     holding it whose parent has died, clears
                                     devpod's stale busy marker once nothing is
                                     building, kills any container it still has
                                     running, and prints all of it
  rm                                 Delete it. Refuses if its clone holds
                                     uncommitted or unpushed work, or if git
                                     cannot read the clone to find out; add
                                     --force to delete it anyway. --force also
                                     counts an already-absent workspace as
                                     deleted, like rm -f.
  rme                                The same delete, and then the shell: on a
                                     removal that worked it hangs up whatever
                                     started dl, so the terminal tab it was typed
                                     in closes on its own
  code                               Open it in VS Code
  restart                            Stop and start it (no rebuild)
  recreate                           Recreate the container
  reset                              Clean slate: remove everything, recreate
  dotfiles                           Refresh dotfiles (chezmoi update)
  -- <command>                       Run one command inside it

A verb with no workspace named picks interactively. For up, stop, kill, rm, rme,
code and dotfiles, TAB marks several rows and the verb applies to each in turn — dl
rm can clear five workspaces in one visit. The forms that end in a session (attach,
--, restart, recreate, reset) take exactly one.

'prune' was a second spelling of the rm verb and is retired: it collided with the
--prune flag below, which removes clone directories and no workspace at all. Typing
it says so and names both.

--rm is the throwaway workspace, and it is docker's --rm rather than another spelling
of the rm verb above: dl <ws> --rm and dl <ws> --rm -- <cmd> hand over a session and
delete the workspace and its clone once it ends, exactly as docker run --rm does,
where the rm verb deletes one now. It stops at work that is nowhere else — the same
check the verb makes — and then leaves the workspace standing and says so, which is
what makes it safe to leave on a line you recall. The exit code is the session's, not
the removal's.

Those two forms and no others: every verb above refuses --rm rather than ignoring it,
and 'code' is the one worth knowing about, since it returns while VS Code is still
connecting. --force does not compose with it either — use dl <ws> rm --force when you
mean to delete despite the work, which is where docker keeps its -f too. Best-effort
by nature: a Ctrl-C during the build, or a closed terminal, ends dl before the session
does and leaves the workspace behind.

rme is neither of those two: it deletes the workspace now, as the rm verb does, and
then ends the shell that asked rather than a session. It is for the terminal tab
opened for one workspace, where the delete is a wait and the exit after it is a
keystroke. The removal is the verb's, guard included, and the hangup is only reached
if it worked — a refusal, or a devpod that would not finish, leaves the shell standing
with the reason on screen. `--force` is the exception, because it asks for absence
rather than for a removal: a forced rme of a workspace that was never there succeeds
and still closes the shell. What is hung up is dl's parent process, and which process that is
depends on the shell rather than on the line: a subshell running one command is
usually replaced by it, so $(dl <ws> rme) closes your terminal too, while the same
line with a redirection leaves a subshell to take the signal. dl prints the pid it
signalled rather than guessing. A nohup is refused outright, since disarming SIGHUP
is how a run outlives its terminal in the first place.

--stop is retired, and --autorm is what --rm is now called. Both are still recognised
and say so.";

/// What no flag and no verb names: the variables that change a launch.
///
/// Last, below the options table, because reading it is what somebody does once
/// and then puts in a shell profile.
const ENVIRONMENT: &str = "Environment:
  DEVLAUNCH_TIMING=1|json            Write a timing summary to stderr
  DEVLAUNCH_NO_GH_TOKEN=1            Do not forward the host's gh login
  DEVLAUNCH_DOTFILES_ON_ATTACH=1     Refresh dotfiles before every attach
  DEVLAUNCH_NO_TITLE=1               Do not name the terminal after the workspace";

/// Which command flag was given, if any.
///
/// A sum over the flags rather than a pile of bools carried into the dispatch:
/// clap's group makes at most one of them true, and this is where that becomes a
/// type instead of an assumption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chosen {
    Ls,
    Refresh,
    Install,
    Prune,
    Reconcile,
    Purge,
    Version,
    Repos,
    CompletionData,
    UpdateCache,
}

impl Cli {
    fn chosen(&self) -> Option<Chosen> {
        // In declaration order; clap's `command` group refuses two at once, so at
        // most one of these is true and the order is documentation, not priority.
        [
            (self.ls, Chosen::Ls),
            (self.refresh, Chosen::Refresh),
            (self.install, Chosen::Install),
            (self.prune, Chosen::Prune),
            (self.reconcile, Chosen::Reconcile),
            (self.purge, Chosen::Purge),
            (self.version, Chosen::Version),
            (self.repos, Chosen::Repos),
            (self.completion_data, Chosen::CompletionData),
            (self.update_cache, Chosen::UpdateCache),
        ]
        .into_iter()
        .find_map(|(given, chosen)| given.then_some(chosen))
    }

    /// The retired spelling this line used, if it used one.
    fn retired_flag(&self) -> Option<RetiredFlag> {
        [
            (self.stop, RetiredFlag::Stop),
            (self.autorm, RetiredFlag::Autorm),
        ]
        .into_iter()
        .find_map(|(given, retired)| given.then_some(retired))
    }
}

/// The command `cli` asks for.
///
/// Total over [`Cli`]: every accepted command line is either a [`Command`] or a
/// named [`GrammarError`], and nothing is left to be worked out later.
pub(crate) fn resolve(cli: Cli, argv: &[String]) -> Result<Command, GrammarError> {
    // First, because both retired spellings moved on account of what `--rm` means
    // now: a line carrying one of them is asking for a grammar this build has not
    // got, and every other diagnostic would describe the line it typed instead of
    // the reason it stopped working.
    if let Some(retired) = cli.retired_flag() {
        return Err(GrammarError::RetiredFlag(retired));
    }
    match cli.chosen() {
        Some(global) => global_command(&cli, global),
        None => workspace_command(cli, argv),
    }
}

/// Where `--force` sits in the positional word stream, as Python reads it.
///
/// clap accepts `--force` in any position and strips it, but Python never had a
/// `--force` flag: it read `sys.argv` positionally and asked `"--force" in
/// args[2:]` (dl.py:4726), with `args[0]` the workspace and `args[1]` the verb. So
/// a `--force` in the workspace slot became an unknown workspace, and one in the
/// verb slot an unknown command — both refusals (exit 1), where clap-parsed Rust
/// silently accepted the flag and *deleted*. This recovers the position clap threw
/// away, from the same word stream `argv_without_devcontainer` builds (the command
/// tail after `--` is not `dl`'s to read).
enum ForcePlace {
    /// `--force` in `args[0]`: the workspace name is `--force`.
    WorkspaceSlot,
    /// `--force` in `args[1]`: the verb is `--force`, and this is the target it
    /// followed.
    VerbSlot { target: String },
    /// `--force` in `args[2:]`, where it means what it says.
    Trailing,
}

fn force_placement(argv: &[String]) -> Option<ForcePlace> {
    let mut stream = Vec::new();
    let mut rest = argv.iter();
    while let Some(argument) = rest.next() {
        if argument == "--" {
            break;
        }
        if argument.starts_with("--devcontainer=") {
            continue;
        }
        if argument == "--devcontainer" {
            rest.next();
            continue;
        }
        stream.push(argument.as_str());
    }
    match stream.iter().position(|word| *word == "--force") {
        None => None,
        Some(0) => Some(ForcePlace::WorkspaceSlot),
        Some(1) => Some(ForcePlace::VerbSlot {
            target: stream[0].to_owned(),
        }),
        Some(_) => Some(ForcePlace::Trailing),
    }
}

/// A command that opens no workspace: no target, no `--`, no `--devcontainer`.
fn global_command(cli: &Cli, chosen: Chosen) -> Result<Command, GrammarError> {
    let name = flag_of(chosen);
    if cli.devcontainer.is_some() {
        return Err(GrammarError::DevcontainerNotAllowed { command: name });
    }
    if cli.rm {
        return Err(GrammarError::RmNotAllowed { command: name });
    }
    if !cli.command.is_empty() {
        return Err(GrammarError::CommandNotAllowed { verb: name });
    }
    // `--install` is the one that takes a word, and it is a path rather than a
    // workspace: Python read it off argv[1] the same way.
    let takes_word = matches!(chosen, Chosen::Install);
    if !takes_word && !cli.words.is_empty() {
        return Err(GrammarError::TargetNotAllowed { command: name });
    }
    let accepts_yes = matches!(chosen, Chosen::Prune | Chosen::Reconcile | Chosen::Purge);
    if cli.yes && !accepts_yes {
        return Err(GrammarError::ModifierNotAllowed {
            modifier: "--yes",
            command: name,
        });
    }
    let accepts_force = matches!(chosen, Chosen::Prune | Chosen::UpdateCache);
    if cli.force && !accepts_force {
        return Err(GrammarError::ModifierNotAllowed {
            modifier: "--force",
            command: name,
        });
    }
    Ok(match chosen {
        Chosen::Ls => Command::List {
            output: if cli.json {
                ListOutput::Json
            } else {
                ListOutput::Table
            },
            sizes: if cli.size {
                Sizes::Measure
            } else {
                Sizes::Skip
            },
        },
        Chosen::Refresh => Command::Refresh,
        Chosen::Install => Command::Install {
            rc: cli.words.first().map(PathBuf::from),
        },
        Chosen::Prune => Command::Prune {
            yes: cli.yes,
            force: cli.force,
        },
        Chosen::Reconcile => Command::Reconcile { yes: cli.yes },
        Chosen::Purge => Command::Purge { yes: cli.yes },
        Chosen::Version => Command::Version,
        Chosen::Repos => Command::Repos,
        Chosen::CompletionData => Command::CompletionData,
        Chosen::UpdateCache => Command::UpdateCache { force: cli.force },
    })
}

fn flag_of(chosen: Chosen) -> &'static str {
    match chosen {
        Chosen::Ls => "--ls",
        Chosen::Refresh => "--refresh",
        Chosen::Install => "--install",
        Chosen::Prune => "--prune",
        Chosen::Reconcile => "--reconcile",
        Chosen::Purge => "--purge",
        Chosen::Version => "--version",
        Chosen::Repos => "--repos",
        Chosen::CompletionData => "--completion-data",
        Chosen::UpdateCache => "--update-cache",
    }
}

/// The workspace-first and verb-first grammar: up to two words, plus `-- <cmd>`.
fn workspace_command(cli: Cli, argv: &[String]) -> Result<Command, GrammarError> {
    let devcontainer = devcontainer_of(&cli)?;
    if cli.yes {
        return Err(GrammarError::ModifierNotAllowed {
            modifier: "--yes",
            command: "a workspace command",
        });
    }
    // Before `force_placement`, and that ordering is the whole of the fix it is.
    // `--force`'s meaning is recovered from its *position* in the word stream, and
    // `--rm` is a word in that stream — so `dl <ws> --rm --force` reads `--force` as
    // trailing (the pair, correctly) while `dl --rm --force <ws>` reads it as the
    // *verb* slot and answers `Unknown command '--force'` about a workspace called
    // `--rm`, which explains nothing about a line whose real problem is the pair.
    // Asked here, every spelling of the pair gets the sentence that names it.
    //
    // What this gives up is `dl --force --rm`, which used to be an attach on a
    // workspace *named* `--force` (the slot-0 reading Python's parity preserves) and
    // is now the refused pair. That is the better answer: a line carrying both flags
    // is somebody expecting `--force` to license the removal, not somebody who named
    // a workspace `--force`. The parity reading is untouched for every line that does
    // not also say `--rm`.
    if cli.rm && cli.force {
        return Err(GrammarError::RmForced);
    }
    // `--force` only means force where Python read it — after the workspace and the
    // verb. Anywhere earlier it is the word in that slot, and the refusal is
    // Python's for that slot, not a silent forced delete.
    if let Some(place) = force_placement(argv) {
        match place {
            ForcePlace::WorkspaceSlot => {
                // The workspace named `--force`: routed through the normal target
                // path so it earns the same "Unknown workspace '--force'" a bare
                // unknown name does.
                return Ok(Command::Workspace {
                    target: "--force".to_owned(),
                    // `RmOnExit::No` by the check above, not by choice: a line
                    // holding both flags was refused before it got here.
                    verb: Verb::Attach {
                        rm: rm_on_exit_of(&cli),
                    },
                    devcontainer,
                });
            }
            ForcePlace::VerbSlot { target } => {
                // A *retired* verb in the workspace slot earns the retirement's
                // sentence instead. `dl prune --force` was the remove verb with
                // `--force` and no target — the selector form — and a diagnostic
                // naming `--force` explains nothing about why the line stopped
                // working.
                if let Some(retired) = RetiredWord::of(&target) {
                    return Err(GrammarError::RetiredVerb(retired));
                }
                return Err(GrammarError::UnknownVerb {
                    target,
                    word: "--force".to_owned(),
                });
            }
            // Where `--force` really is the modifier, and so the one place the pair
            // with `--rm` can be told from a workspace that happens to be
            // called `--force`.
            ForcePlace::Trailing => {}
        }
    }
    let rm = rm_on_exit_of(&cli);
    let run = NonEmpty::of(cli.command.iter().cloned()).map(|words| Verb::Run(words, rm));
    let attach = || Verb::Attach { rm };
    let (target, verb) = match cli.words.as_slice() {
        [] => (None, run.unwrap_or_else(attach)),
        [only] => match VerbWord::of(only) {
            Some(word) => (None, word.with(cli.force)),
            // A retired word alone is the retirement's diagnostic, not a workspace
            // of that name: `dl prune` used to be the remove verb with no target.
            None => match RetiredWord::of(only) {
                Some(retired) => return Err(GrammarError::RetiredVerb(retired)),
                None => (Some(only.clone()), run.unwrap_or_else(attach)),
            },
        },
        [first, second] => match (VerbWord::of(first), VerbWord::of(second)) {
            // Row 1: the verb wins wherever it is, and the other word is the
            // target. A leading verb is checked first, so `dl stop code` stops
            // the workspace named `code` rather than opening the one named
            // `stop` in an editor.
            (Some(word), _) => (Some(second.clone()), word.with(cli.force)),
            (None, Some(word)) => (Some(first.clone()), word.with(cli.force)),
            (None, None) => {
                // A retired word in either slot earns the retirement's sentence
                // rather than "Unknown command": `dl <ws> prune` and
                // `dl prune <ws>` were both the delete verb, and the person who
                // typed one is owed the word that replaced it. A retired word
                // *beside a live verb* is not reached here — the live verb wins
                // from either position, so `dl stop prune` still stops a workspace
                // named `prune`, as it always did.
                if let Some(retired) = RetiredWord::of(first).or_else(|| RetiredWord::of(second)) {
                    return Err(GrammarError::RetiredVerb(retired));
                }
                return Err(GrammarError::UnknownVerb {
                    target: first.clone(),
                    word: second.clone(),
                });
            }
        },
        // clap caps the positionals at two, so a third word never arrives here.
        _ => {
            return Err(GrammarError::UnknownVerb {
                target: cli.words[0].clone(),
                word: cli.words[2].clone(),
            });
        }
    };
    // A verb that is not the attach family cannot also carry a command: Python
    // discarded the command silently, which is the shape row 2 refuses.
    if !cli.command.is_empty() && !matches!(verb, Verb::Run(..)) {
        return Err(GrammarError::CommandNotAllowed { verb: verb.word() });
    }
    // The verb words the grammar just resolved carry no [`RmOnExit`] — only the two
    // attach spellings can — so a `--rm` that reached one of them was written and
    // could not be honoured. Read off the *verb* rather than the words, so
    // `dl up <ws> --rm` and `dl <ws> up --rm` are refused alike. `dl <ws> rm --rm`
    // lands here too: the word and the flag are two different requests, and saying so
    // beats quietly treating the pair as one of them.
    if cli.rm && !matches!(verb, Verb::Attach { .. } | Verb::Run(..)) {
        return Err(GrammarError::RmNotAllowed {
            command: verb.word(),
        });
    }
    Ok(match target {
        None => Command::Select { verb, devcontainer },
        Some(target) => Command::Workspace {
            target,
            verb,
            devcontainer,
        },
    })
}

/// Whether this line asked for the workspace to go when the session does.
fn rm_on_exit_of(cli: &Cli) -> RmOnExit {
    if cli.rm { RmOnExit::Yes } else { RmOnExit::No }
}

fn devcontainer_of(cli: &Cli) -> Result<Option<DevcontainerPath>, GrammarError> {
    match &cli.devcontainer {
        None => Ok(None),
        Some(raw) => spec::resolve_devcontainer_ref(raw)
            .map(Some)
            .map_err(|why| GrammarError::Devcontainer {
                raw: raw.clone(),
                why,
            }),
    }
}

/// The argv `wants_startup_cache_refresh` is asked about.
///
/// Python asks the predicate *after* pulling `--devcontainer` out of the argument
/// list, so `dl --devcontainer x --ls` still warms the cache. The predicate is a
/// pure function of argv and has to answer before the command runs, so the same
/// stripping happens here rather than after clap: what clap produces is a
/// [`Cli`], and the predicate's question is about the words.
///
/// Scanning stops at the first bare `--`, because everything after it is the
/// command the workspace runs and must not be read as `dl`'s own flags.
pub(crate) fn argv_without_devcontainer(argv: &[String]) -> Vec<&str> {
    let mut kept = Vec::with_capacity(argv.len());
    let mut rest = argv.iter();
    while let Some(argument) = rest.next() {
        if argument == "--" {
            kept.push(argument.as_str());
            kept.extend(rest.map(String::as_str));
            break;
        }
        if argument.starts_with("--devcontainer=") {
            continue;
        }
        if argument == "--devcontainer" {
            // Its value, whatever it is. A missing value is clap's error to
            // report, not this scan's.
            rest.next();
            continue;
        }
        kept.push(argument.as_str());
    }
    kept
}

#[cfg(test)]
mod tests {
    //! The grammar decisions, as tables.
    //!
    //! What `test/test_dl.py`'s dispatch expectations and the module docstring's
    //! usage block say, re-expressed against [`resolve`] — plus the three
    //! divergence rows the grammar carries, each with the input that distinguishes
    //! it from Python.

    use super::*;

    /// The command a line resolves to.
    fn parse(argv: &[&str]) -> Result<Command, GrammarError> {
        let cli = Cli::try_parse_from(std::iter::once("dl").chain(argv.iter().copied()))
            .unwrap_or_else(|error| panic!("clap refused {argv:?}: {error}"));
        let raw: Vec<String> = argv.iter().map(|word| (*word).to_owned()).collect();
        resolve(cli, &raw)
    }

    fn refused(argv: &[&str]) -> clap::error::ErrorKind {
        Cli::try_parse_from(std::iter::once("dl").chain(argv.iter().copied()))
            .expect_err("clap should refuse this")
            .kind()
    }

    /// `dl <ws>` — the plain attach, which is what almost every case below wants.
    fn attach() -> Verb {
        Verb::Attach { rm: RmOnExit::No }
    }

    /// `rm`, and `rme` beside it: the two words are one arm apart, and every
    /// expectation below that names one is a claim about which.
    fn remove(force: bool) -> Verb {
        Verb::Remove {
            force,
            after: AfterRemoval::LeaveTheShell,
        }
    }

    fn remove_and_exit(force: bool) -> Verb {
        Verb::Remove {
            force,
            after: AfterRemoval::HangUpTheShell,
        }
    }

    fn run(words: &[&str]) -> Verb {
        run_with(words, RmOnExit::No)
    }

    fn run_with(words: &[&str], rm: RmOnExit) -> Verb {
        Verb::Run(
            NonEmpty::of(words.iter().map(|word| word.to_string())).expect("a command"),
            rm,
        )
    }

    fn workspace(target: &str, verb: Verb) -> Command {
        Command::Workspace {
            target: target.to_owned(),
            verb,
            devcontainer: None,
        }
    }

    // ===================================================== reserved verbs (row 1)

    #[test]
    fn the_verb_wins_from_either_position() {
        // The precedence table. `dl stop` is the verb with no target, which is
        // the whole of divergence row 1: Python read `stop` as a workspace name
        // and went looking for a workspace called that.
        let cases: [(&[&str], Command); 8] = [
            (
                &["stop"],
                Command::Select {
                    verb: Verb::Stop,
                    devcontainer: None,
                },
            ),
            (&["stop", "ws"], workspace("ws", Verb::Stop)),
            (&["ws", "stop"], workspace("ws", Verb::Stop)),
            (&["up", "ws"], workspace("ws", Verb::Up)),
            (&["ws", "up"], workspace("ws", Verb::Up)),
            (&["ws", "code"], workspace("ws", Verb::Code)),
            (&["ws", "dotfiles"], workspace("ws", Verb::Dotfiles)),
            (&["owner/repo@branch", "restart"], {
                workspace("owner/repo@branch", Verb::Restart)
            }),
        ];
        for (argv, expected) in cases {
            assert_eq!(parse(argv), Ok(expected), "dl {}", argv.join(" "));
        }
    }

    /// The hammer for a workspace that will not answer, and it reads from either
    /// position like every other verb: `dl kill <ws>` is what somebody types while
    /// the wedged `dl` is still hanging in another terminal.
    #[test]
    fn kill_is_a_verb_from_either_position() {
        assert_eq!(parse(&["ws", "kill"]), Ok(workspace("ws", Verb::Kill)));
        assert_eq!(parse(&["kill", "ws"]), Ok(workspace("ws", Verb::Kill)));
    }

    /// Several wedged workspaces in one visit, for the reason `rm` gets the same
    /// answer: the verb finishes on its own and hands nothing over.
    #[test]
    fn kill_may_be_marked_for_several_workspaces_at_once() {
        assert!(Verb::Kill.several_at_once());
    }

    #[test]
    fn a_leading_verb_wins_over_a_trailing_one() {
        // Both words are verbs, so the rule has to say which. The leading one is
        // the verb and the trailing one is the target, because that is what
        // `dl stop code` reads as to anyone typing it.
        assert_eq!(parse(&["stop", "code"]), Ok(workspace("code", Verb::Stop)));
    }

    #[test]
    fn rm_is_the_only_spelling_of_the_delete_verb() {
        assert_eq!(
            parse(&["ws", "rm", "--force"]),
            Ok(workspace("ws", remove(true)))
        );
    }

    // ============================================================ rme: rm, and out

    #[test]
    fn rme_is_the_delete_verb_and_then_the_shell() {
        // The same removal, from either position and with `--force` folded in the
        // same way: everything that makes `rm` what it is is `rme`'s too, and the
        // one difference is what the dispatcher does when the command is over.
        assert_eq!(
            parse(&["ws", "rme"]),
            Ok(workspace("ws", remove_and_exit(false)))
        );
        assert_eq!(
            parse(&["rme", "ws"]),
            Ok(workspace("ws", remove_and_exit(false)))
        );
        assert_eq!(
            parse(&["ws", "rme", "--force"]),
            Ok(workspace("ws", remove_and_exit(true)))
        );
        // And with no target it is the picker, per verb rather than per word: the
        // batch is what the wait it saves is longest for.
        assert_eq!(
            parse(&["rme"]),
            Ok(Command::Select {
                verb: remove_and_exit(false),
                devcontainer: None
            })
        );
        assert!(remove_and_exit(false).several_at_once());
    }

    #[test]
    fn only_rme_asks_for_the_shell_and_it_asks_whatever_else_the_line_says() {
        // The dispatcher reads this off the verb and acts on it once, so the two
        // words have to be told apart here and nowhere else. `--force` is not part
        // of the question: a forced `rme` is still an `rme`.
        assert_eq!(
            remove_and_exit(false).after_removal(),
            AfterRemoval::HangUpTheShell
        );
        assert_eq!(
            remove_and_exit(true).after_removal(),
            AfterRemoval::HangUpTheShell
        );
        assert_eq!(remove(false).after_removal(), AfterRemoval::LeaveTheShell);
        // Every other verb leaves the shell alone, including the two that end in a
        // session of their own.
        for verb in [
            attach(),
            run(&["make", "test"]),
            Verb::Up,
            Verb::Stop,
            Verb::Kill,
            Verb::Code,
            Verb::Recreate,
            Verb::Restart,
            Verb::Reset,
            Verb::Dotfiles,
        ] {
            assert_eq!(
                verb.after_removal(),
                AfterRemoval::LeaveTheShell,
                "dl <ws> {} asked for the shell",
                verb.word()
            );
        }
    }

    #[test]
    fn rme_is_quoted_as_rme_in_a_diagnostic_about_it() {
        // The two words share an arm, so the sentence has to read the arm's field
        // to name the line it is about. `dl <ws> rme --rm` is the case: a refusal
        // naming `rm` would quote a word that is not on the line.
        assert_eq!(remove_and_exit(false).word(), "rme");
        assert_eq!(remove(false).word(), "rm");
        assert_eq!(
            parse(&["ws", "rme", "--rm"]),
            Err(GrammarError::RmNotAllowed { command: "rme" })
        );
    }

    #[test]
    fn rme_is_a_verb_and_not_a_workspace_of_that_name() {
        // Row 1, for the word this change adds: the verb wins from either slot, so
        // `dl rme rme` is the removal of a workspace called `rme`, and nothing here
        // can be reached by naming a workspace after it.
        assert_eq!(
            parse(&["rme", "rme"]),
            Ok(workspace("rme", remove_and_exit(false)))
        );
    }

    // ================================================== the retired prune spelling

    #[test]
    fn prune_is_refused_as_a_verb_from_either_position() {
        // It was Python's second spelling of `rm`. It went because `dl --prune` is
        // an unrelated command, so the sentence names both.
        for argv in [&["prune"][..], &["ws", "prune"][..], &["prune", "ws"][..]] {
            assert_eq!(
                parse(argv),
                Err(GrammarError::RetiredVerb(RetiredWord::Prune)),
                "dl {}",
                argv.join(" ")
            );
        }
    }

    #[test]
    fn a_recalled_prune_line_is_the_retirement_and_the_pair_is_named_ahead_of_it() {
        // The two readings a `dl prune <ws> --force` line recalled with `--rm`
        // appended can have, and they are not the same one. Row 30 made the whole
        // line remove `<ws>`; with the override gone, the words are read again and
        // `prune` is the retirement — *unless* `--force` is there too, in which case
        // the pair is what the line got wrong first and the more confused half.
        assert_eq!(
            parse(&["prune", "ws", "--rm"]),
            Err(GrammarError::RetiredVerb(RetiredWord::Prune))
        );
        assert_eq!(
            parse(&["prune", "ws", "--force", "--rm"]),
            Err(GrammarError::RmForced)
        );
    }

    #[test]
    fn a_retired_word_is_never_read_as_the_workspace() {
        // The failure mode the RETIRED table exists to prevent: dropped from the
        // verbs and nothing else, `prune` would be an ordinary word — a workspace
        // name here, and an unknown-workspace refusal naming the word that moved
        // instead of saying that it moved.
        assert_eq!(
            parse(&["prune", "ws"]),
            Err(GrammarError::RetiredVerb(RetiredWord::Prune))
        );
        assert_eq!(
            parse(&["ws", "prune", "--force"]),
            Err(GrammarError::RetiredVerb(RetiredWord::Prune))
        );
    }

    #[test]
    fn the_retired_word_beats_a_force_in_the_verb_slot() {
        // `dl prune --force` was the remove verb with no target and `--force`, so
        // the retirement is what stopped it working — not the `--force` that
        // `force_placement` would otherwise name.
        assert_eq!(
            parse(&["prune", "--force"]),
            Err(GrammarError::RetiredVerb(RetiredWord::Prune))
        );
        // A word that was never a verb keeps the diagnostic it always had.
        assert_eq!(
            parse(&["ws", "--force"]),
            Err(GrammarError::UnknownVerb {
                target: "ws".to_owned(),
                word: "--force".to_owned()
            })
        );
    }

    #[test]
    fn a_live_verb_beside_the_retired_word_still_wins_from_either_position() {
        // Unchanged from before the retirement: the leading verb wins and the other
        // word is the target, so a workspace that really is called `prune` is still
        // reachable.
        assert_eq!(
            parse(&["stop", "prune"]),
            Ok(workspace("prune", Verb::Stop))
        );
        assert_eq!(
            parse(&["prune", "stop"]),
            Ok(workspace("prune", Verb::Stop))
        );
    }

    #[test]
    fn the_bare_prune_flag_is_the_cache_command_not_the_verb() {
        assert_eq!(
            parse(&["--prune"]),
            Ok(Command::Prune {
                yes: false,
                force: false
            })
        );
        assert_eq!(
            parse(&["--prune", "-y", "--force"]),
            Ok(Command::Prune {
                yes: true,
                force: true
            })
        );
    }

    #[test]
    fn a_word_that_is_not_a_verb_is_a_target() {
        assert_eq!(parse(&["ws"]), Ok(workspace("ws", attach())));
        assert_eq!(
            parse(&["owner/repo@feature/x"]),
            Ok(workspace("owner/repo@feature/x", attach()))
        );
        assert_eq!(parse(&["./here"]), Ok(workspace("./here", attach())));
    }

    #[test]
    fn no_arguments_at_all_opens_the_selector() {
        assert_eq!(
            parse(&[]),
            Ok(Command::Select {
                verb: attach(),
                devcontainer: None
            })
        );
    }

    #[test]
    fn two_words_and_no_verb_between_them_is_refused_as_python_refuses_it() {
        assert_eq!(
            parse(&["ws", "wat"]),
            Err(GrammarError::UnknownVerb {
                target: "ws".to_owned(),
                word: "wat".to_owned()
            })
        );
    }

    // ======================================================= the trailing command

    #[test]
    fn everything_after_the_first_dash_dash_is_the_workspace_command() {
        assert_eq!(
            parse(&["ws", "--", "make", "test"]),
            Ok(workspace("ws", run(&["make", "test"])))
        );
    }

    #[test]
    fn a_flag_after_dash_dash_belongs_to_the_workspace_command() {
        assert_eq!(
            parse(&["ws", "--", "ls", "--size", "--devcontainer", "x"]),
            Ok(workspace(
                "ws",
                run(&["ls", "--size", "--devcontainer", "x"])
            ))
        );
    }

    #[test]
    fn a_dash_dash_with_nothing_after_it_is_an_ordinary_attach() {
        // Python's `len(args) > 2`: an empty command is not a command. The
        // `NonEmpty` in `Verb::Run` is what makes the other reading unwritable.
        assert_eq!(parse(&["ws", "--"]), Ok(workspace("ws", attach())));
    }

    #[test]
    fn a_command_cannot_be_given_to_a_verb_that_does_not_run_one() {
        assert_eq!(
            parse(&["ws", "stop", "--", "make"]),
            Err(GrammarError::CommandNotAllowed { verb: "stop" })
        );
    }

    // ============================================================== --ls and kin

    #[test]
    fn the_listing_flags_compose() {
        let cases: [(&[&str], Command); 4] = [
            (
                &["--ls"],
                Command::List {
                    output: ListOutput::Table,
                    sizes: Sizes::Skip,
                },
            ),
            (
                &["--ls", "--json"],
                Command::List {
                    output: ListOutput::Json,
                    sizes: Sizes::Skip,
                },
            ),
            (
                &["--ls", "--size"],
                Command::List {
                    output: ListOutput::Table,
                    sizes: Sizes::Measure,
                },
            ),
            (
                &["--ls", "--json", "--size"],
                Command::List {
                    output: ListOutput::Json,
                    sizes: Sizes::Measure,
                },
            ),
        ];
        for (argv, expected) in cases {
            assert_eq!(parse(argv), Ok(expected), "dl {}", argv.join(" "));
        }
    }

    #[test]
    fn the_read_side_commands_resolve() {
        assert_eq!(parse(&["--version"]), Ok(Command::Version));
        assert_eq!(parse(&["--repos"]), Ok(Command::Repos));
        assert_eq!(parse(&["--completion-data"]), Ok(Command::CompletionData));
        assert_eq!(parse(&["--refresh"]), Ok(Command::Refresh));
        assert_eq!(
            parse(&["--update-cache"]),
            Ok(Command::UpdateCache { force: false })
        );
        assert_eq!(
            parse(&["--update-cache", "--force"]),
            Ok(Command::UpdateCache { force: true })
        );
        assert_eq!(parse(&["--install"]), Ok(Command::Install { rc: None }));
        assert_eq!(
            parse(&["--install", "~/.zshrc"]),
            Ok(Command::Install {
                rc: Some(PathBuf::from("~/.zshrc"))
            })
        );
        assert_eq!(parse(&["--purge"]), Ok(Command::Purge { yes: false }));
        assert_eq!(parse(&["--purge", "-y"]), Ok(Command::Purge { yes: true }));
        assert_eq!(
            parse(&["--reconcile", "--yes"]),
            Ok(Command::Reconcile { yes: true })
        );
    }

    #[test]
    fn a_global_command_takes_no_workspace() {
        assert_eq!(
            parse(&["--ls", "ws"]),
            Err(GrammarError::TargetNotAllowed { command: "--ls" })
        );
        assert_eq!(
            parse(&["--purge", "ws"]),
            Err(GrammarError::TargetNotAllowed { command: "--purge" })
        );
    }

    #[test]
    fn a_modifier_is_refused_where_it_means_nothing() {
        assert_eq!(
            parse(&["--ls", "-y"]),
            Err(GrammarError::ModifierNotAllowed {
                modifier: "--yes",
                command: "--ls"
            })
        );
        assert_eq!(
            parse(&["--purge", "--force"]),
            Err(GrammarError::ModifierNotAllowed {
                modifier: "--force",
                command: "--purge"
            })
        );
    }

    #[test]
    fn the_flag_spelled_stop_verb_is_retired_from_every_position() {
        // Row 15 made `dl --stop <ws>` work and row 30 made it appendable; row 32
        // takes the spelling back, because `--rm` now modifies a session and a
        // look-alike flag that cancels the line instead is the one thing the pair
        // must not be. Every position, so no line finds a corner where it survives.
        for argv in [
            &["--stop"][..],
            &["--stop", "ws"][..],
            &["ws", "--stop"][..],
            &["ws", "--stop", "--force"][..],
        ] {
            assert_eq!(
                parse(argv),
                Err(GrammarError::RetiredFlag(RetiredFlag::Stop)),
                "dl {}",
                argv.join(" ")
            );
        }
    }

    #[test]
    fn autorm_is_retired_in_favour_of_the_rm_it_became() {
        // The same behaviour under a new spelling, so the refusal is the only thing
        // standing between a recalled `--autorm` line and a silent no-op.
        for argv in [
            &["--autorm"][..],
            &["ws", "--autorm"][..],
            &["--autorm", "ws"][..],
            &["ws", "--autorm", "--", "make", "test"][..],
        ] {
            assert_eq!(
                parse(argv),
                Err(GrammarError::RetiredFlag(RetiredFlag::Autorm)),
                "dl {}",
                argv.join(" ")
            );
        }
    }

    #[test]
    fn a_retired_flag_is_answered_before_anything_else_the_line_got_wrong() {
        // Both spellings moved *because* `--rm` changed meaning, so the retirement
        // is the reason the line stopped working and every other diagnostic would
        // describe something the person did not do. Asked first for that reason,
        // and these are the lines that would otherwise answer something else: a
        // group conflict, and a `--force` in the workspace slot.
        assert_eq!(
            parse(&["--ls", "--stop"]),
            Err(GrammarError::RetiredFlag(RetiredFlag::Stop))
        );
        assert_eq!(
            parse(&["--force", "--autorm"]),
            Err(GrammarError::RetiredFlag(RetiredFlag::Autorm))
        );
    }

    #[test]
    fn the_words_the_retired_flags_stood_for_still_work() {
        assert_eq!(parse(&["stop", "ws"]), Ok(workspace("ws", Verb::Stop)));
        assert_eq!(
            parse(&["rm", "ws", "--force"]),
            Ok(workspace("ws", remove(true)))
        );
        assert_eq!(
            parse(&["stop"]),
            Ok(Command::Select {
                verb: Verb::Stop,
                devcontainer: None
            })
        );
    }

    // ============================================================= --devcontainer

    #[test]
    fn a_bare_devcontainer_name_becomes_the_variant_path() {
        let Ok(Command::Workspace {
            devcontainer: Some(path),
            ..
        }) = parse(&["ws", "--devcontainer", "robot"])
        else {
            panic!("expected a workspace command carrying a devcontainer");
        };
        assert_eq!(path.as_str(), ".devcontainer/robot/devcontainer.json");
    }

    #[test]
    fn a_devcontainer_path_is_used_as_given() {
        for raw in ["a/b.json", "custom.json"] {
            let Ok(Command::Workspace {
                devcontainer: Some(path),
                ..
            }) = parse(&["ws", "--devcontainer", raw])
            else {
                panic!("expected a workspace command carrying a devcontainer");
            };
            assert_eq!(path.as_str(), raw);
        }
    }

    #[test]
    fn a_blank_devcontainer_is_refused() {
        assert_eq!(
            parse(&["ws", "--devcontainer", " "]),
            Err(GrammarError::Devcontainer {
                raw: " ".to_owned(),
                why: DevcontainerRefError::Missing
            })
        );
    }

    #[test]
    fn devcontainer_is_refused_on_a_command_that_opens_nothing() {
        assert_eq!(
            parse(&["--ls", "--devcontainer", "robot"]),
            Err(GrammarError::DevcontainerNotAllowed { command: "--ls" })
        );
    }

    // ================================================== clap's strictness (row 2)

    #[test]
    fn an_unknown_flag_is_refused_rather_than_read_as_a_workspace() {
        // Python's dispatch read `--nope` as a workspace spec and reported an
        // unknown workspace. The distinguishing input is any misspelt flag.
        assert_eq!(
            refused(&["--nope"]),
            clap::error::ErrorKind::UnknownArgument
        );
    }

    #[test]
    fn a_long_flag_cannot_be_abbreviated() {
        assert_eq!(
            refused(&["--comp"]),
            clap::error::ErrorKind::UnknownArgument
        );
    }

    #[test]
    fn two_commands_at_once_are_refused() {
        assert_eq!(
            refused(&["--ls", "--purge"]),
            clap::error::ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn json_and_size_are_only_meaningful_with_ls() {
        assert_eq!(
            refused(&["--json"]),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            refused(&["--size"]),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn a_third_positional_word_is_refused() {
        assert_eq!(
            refused(&["ws", "stop", "extra"]),
            clap::error::ErrorKind::TooManyValues
        );
    }

    // ============================================ the cache-refresh argv stripping

    // ============================================ a stale word beside the --rm flag

    #[test]
    fn a_word_the_grammar_cannot_read_is_still_refused_beside_the_flag() {
        // The line row 30 taught `--rm` to rescue, and row 32 hands back: an
        // `aid`-style line recalled with the flag typed at the end. `--rm` no
        // longer overrides anything, so the two words that are not a verb are
        // what they were before — a refusal that names the word and how to run it.
        assert_eq!(
            parse(&["owner/repo@fix/x", "review this pr", "--rm"]),
            Err(GrammarError::UnknownVerb {
                target: "owner/repo@fix/x".to_owned(),
                word: "review this pr".to_owned()
            })
        );
    }

    #[test]
    fn the_refresh_predicate_sees_argv_with_devcontainer_removed() {
        let cases: [(&[&str], &[&str]); 5] = [
            (&["--ls"], &["--ls"]),
            (&["--devcontainer", "robot", "--ls"], &["--ls"]),
            (&["--devcontainer=robot", "--ls"], &["--ls"]),
            (&["ws", "up"], &["ws", "up"]),
            // Everything after a bare `--` is the workspace's command and is
            // left alone, flags of the same name included.
            (
                &["ws", "--", "dl", "--devcontainer", "x"],
                &["ws", "--", "dl", "--devcontainer", "x"],
            ),
        ];
        for (argv, expected) in cases {
            let owned: Vec<String> = argv.iter().map(|word| word.to_string()).collect();
            assert_eq!(argv_without_devcontainer(&owned), expected, "{argv:?}");
        }
    }

    /// Divergence row 3 licenses clap's layout; *which* clap layout is still a
    /// decision, and this is it — the verbs and examples somebody opened `--help`
    /// to find come above the options table, not below it.
    ///
    /// `help_template` is a string clap resolves at render time, so a marker that
    /// stops resolving is not a compile error — it renders as its own literal text
    /// and the section it stood for goes missing, which is what every `find` below
    /// is also checking for.
    #[test]
    fn help_puts_the_grammar_above_the_options_table() {
        use clap::CommandFactory;

        let help = Cli::command().render_long_help().to_string();
        let examples = help.find("Examples:").expect("no examples");
        let verbs = help.find("Workspace commands").expect("no verb list");
        let options = help.find("Options:").expect("no options table");
        let environment = help.find("Environment:").expect("no environment");

        assert!(examples < verbs, "{help}");
        assert!(verbs < options, "{help}");
        assert!(options < environment, "{help}");
        assert!(help.starts_with("Open a devcontainer"), "{help}");
        assert!(help.contains("Usage: dl "), "{help}");
    }

    // ======================================================================= --rm

    #[test]
    fn rm_is_carried_by_the_two_forms_that_hand_over_a_session() {
        assert_eq!(
            parse(&["ws", "--rm"]),
            Ok(workspace("ws", Verb::Attach { rm: RmOnExit::Yes }))
        );
        assert_eq!(
            parse(&["ws", "--rm", "--", "make", "test"]),
            Ok(workspace("ws", run_with(&["make", "test"], RmOnExit::Yes)))
        );
        // Position is not the grammar's business: clap strips the flag wherever it
        // sits, and only `--force` has a slot that means something.
        assert_eq!(
            parse(&["--rm", "ws"]),
            Ok(workspace("ws", Verb::Attach { rm: RmOnExit::Yes }))
        );
    }

    #[test]
    fn rm_with_no_workspace_named_picks_one_and_keeps_the_flag() {
        assert_eq!(
            parse(&["--rm"]),
            Ok(Command::Select {
                verb: Verb::Attach { rm: RmOnExit::Yes },
                devcontainer: None
            })
        );
    }

    #[test]
    fn a_dash_dash_with_nothing_after_it_still_carries_the_flag() {
        // The empty command is not a command, and the attach it collapses to is
        // still the attach that was asked to clean up after itself.
        assert_eq!(
            parse(&["ws", "--rm", "--"]),
            Ok(workspace("ws", Verb::Attach { rm: RmOnExit::Yes }))
        );
    }

    #[test]
    fn every_verb_word_refuses_the_flag() {
        // `code` is why this is a refusal and not a shrug: it returns the moment
        // devpod has told VS Code where to connect, so honouring `--rm` there
        // would delete the container out from under a window still opening.
        //
        // `restart`, `recreate` and `reset` are in this list for a different reason
        // and it is worth not conflating them: those three *do* end in a session, so
        // the removal would work behind them. They are refused because the flag is
        // the throwaway workspace rather than a cleanup modifier on every verb that
        // ends in a shell — a scope decision, which is why the sentence names the
        // two forms that work instead of claiming these hand over nothing.
        for word in [
            "up", "stop", "rm", "rme", "code", "restart", "recreate", "reset", "dotfiles",
        ] {
            assert_eq!(
                parse(&["ws", word, "--rm"]),
                Err(GrammarError::RmNotAllowed { command: word }),
                "dl ws {word} --rm"
            );
            // And from the other position, since the verb wins from either.
            assert_eq!(
                parse(&[word, "ws", "--rm"]),
                Err(GrammarError::RmNotAllowed { command: word }),
                "dl {word} ws --rm"
            );
        }
    }

    #[test]
    fn the_rm_verb_beside_the_rm_flag_is_two_requests_and_says_so() {
        // docker's split, in the one line where a person can trip over it: the word
        // deletes now, the flag deletes when a session ends, and `dl ws rm --rm` has
        // asked for both. Refused rather than silently treated as either, and the
        // sentence names the verb — which is the spelling that already does what the
        // line most likely wanted.
        assert_eq!(
            parse(&["ws", "rm", "--rm"]),
            Err(GrammarError::RmNotAllowed { command: "rm" })
        );
        assert_eq!(
            parse(&["rm", "ws", "--rm", "--force"]),
            Err(GrammarError::RmForced),
            "the pair is named ahead of the verb, which is the more confused half"
        );
    }

    #[test]
    fn a_command_that_opens_no_workspace_refuses_it() {
        assert_eq!(
            parse(&["--ls", "--rm"]),
            Err(GrammarError::RmNotAllowed { command: "--ls" })
        );
        assert_eq!(
            parse(&["--purge", "--rm"]),
            Err(GrammarError::RmNotAllowed { command: "--purge" })
        );
    }

    #[test]
    fn force_does_not_compose_with_rm() {
        // The pair looks like it should and must not: the guard is what makes the
        // flag safe to leave on a recalled line, and a habitual `--force` beside it
        // would destroy work later, unattended, with nobody reading the sentence.
        assert_eq!(
            parse(&["ws", "--rm", "--force"]),
            Err(GrammarError::RmForced)
        );
        assert_eq!(
            parse(&["ws", "--rm", "--force", "--", "make"]),
            Err(GrammarError::RmForced)
        );
    }

    #[test]
    fn the_pair_is_refused_wherever_force_sits_in_the_line() {
        // `--force`'s meaning is recovered from its position, and `--rm` is a word
        // in the stream that position is counted in — so without this the same pair
        // gets three different answers depending on where it was typed, two of them
        // describing the wrong problem: `Unknown command '--force'` about a workspace
        // called `--rm`, and `Unknown workspace '--force'`.
        for line in [
            vec!["ws", "--rm", "--force"],
            vec!["--rm", "--force", "ws"],
            vec!["--force", "--rm", "ws"],
            vec!["--rm", "ws", "--force"],
            // No workspace at all: the selector's line, refused the same way rather
            // than opening a picker whose choice cannot be honoured.
            vec!["--force", "--rm"],
        ] {
            assert_eq!(
                parse(&line),
                Err(GrammarError::RmForced),
                "dl {}",
                line.join(" ")
            );
        }
    }

    #[test]
    fn a_workspace_called_force_still_reads_that_way_without_autorm() {
        // The parity readings `force_placement` exists to recover are untouched for
        // every line that does not also say `--autorm`. Slot 0 is a *name*, and the
        // line earns the same refusal a bare unknown name does; slot 1 is the *verb*,
        // so `dl ws --force` is an unknown command and not a silent forced anything.
        assert_eq!(
            parse(&["--force"]),
            Ok(workspace("--force", attach())),
            "the slot-0 reading was lost"
        );
        assert_eq!(
            parse(&["ws", "--force"]),
            Err(GrammarError::UnknownVerb {
                target: "ws".to_owned(),
                word: "--force".to_owned()
            }),
            "the slot-1 reading was lost"
        );
    }

    #[test]
    fn autorm_after_the_dash_dash_belongs_to_the_workspace_command() {
        // Everything after `--` is the command's, flags included: this runs a
        // program called `--autorm` and removes nothing.
        assert_eq!(
            parse(&["ws", "--", "--autorm"]),
            Ok(workspace("ws", run(&["--autorm"])))
        );
    }

    #[test]
    fn the_help_names_the_flag_where_somebody_would_look_for_it() {
        use clap::CommandFactory;

        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("dl blooop/devlaunch --rm"), "{help}");
        assert!(help.contains("--rm is the throwaway workspace"), "{help}");
        // And the retired spellings are named there rather than only in the refusal,
        // because the help is where somebody goes to find out what replaced one.
        assert!(
            help.contains("--stop is retired, and --autorm is what --rm is now called"),
            "{help}"
        );
        // Hidden means hidden: neither is an option this build offers.
        assert!(!help.contains("      --stop\n"), "{help}");
        assert!(!help.contains("      --autorm\n"), "{help}");
    }
}
