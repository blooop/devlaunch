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
const VERBS: [(&str, VerbWord); 8] = [
    ("up", VerbWord::Up),
    ("stop", VerbWord::Stop),
    ("rm", VerbWord::Remove),
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
/// name* — `dl prune <ws>` would report an unknown workspace called `prune`, and
/// `dl prune <ws> --force --rm` would delete a workspace called `prune` instead of
/// `<ws>`. Kept here, the word still cannot be a target, still says what to type
/// instead, and a suffix verb meaning the same thing can absorb it silently.
const RETIRED: [(&str, RetiredWord); 1] = [("prune", RetiredWord::Prune)];

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

    /// The verb this word used to mean.
    ///
    /// Not for carrying out — the word is refused — but so a *suffix* flag asking
    /// for the same thing can absorb it instead of reporting that it displaced
    /// something. `dl prune <ws> --force --rm` says delete twice and means it once.
    fn was(self) -> VerbWord {
        match self {
            Self::Prune => VerbWord::Remove,
        }
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
    Remove,
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
            Self::Remove => Verb::Remove { force },
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
/// is *defined* for — which is what makes `dl <ws> code --autorm` unwritable rather
/// than merely refused, and what stops every other arm carrying a field the
/// dispatcher has to remember to ignore.
///
/// **Two arms and not five, and that is a scope decision rather than a mechanical
/// one.** `restart`, `recreate` and `reset` also end in a session
/// (`LaunchVerb::attaches`), so the removal would work behind them; they are out
/// because `--autorm` means *the throwaway workspace* — open one, use it, let it go —
/// and not *clean up after whichever verb this was*. Widening it later is adding
/// arms here; the refusal names the two forms rather than claiming those verbs hand
/// over nothing, because they do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Autorm {
    /// `--autorm`: remove the workspace when the session ends, guard and all.
    OnExit,
    /// The default: the workspace outlives the session, as it always has.
    No,
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
        autorm: Autorm,
    },
    /// `dl <ws> -- <command>` — one command, inside the workspace.
    Run(NonEmpty<String>, Autorm),
    Up,
    Stop,
    /// `rm`. `force` is `--force`: delete despite unsaved work, and count an
    /// already-absent workspace as deleted.
    Remove {
        force: bool,
    },
    Code,
    Recreate,
    Restart,
    Reset,
    Dotfiles,
}

impl Verb {
    /// The word this verb is spelled with, for a diagnostic that names it.
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Verb::Attach { .. } => "attach",
            Verb::Run(..) => "--",
            Verb::Up => "up",
            Verb::Stop => "stop",
            Verb::Remove { .. } => "rm",
            Verb::Code => "code",
            Verb::Recreate => "recreate",
            Verb::Restart => "restart",
            Verb::Reset => "reset",
            Verb::Dotfiles => "dotfiles",
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
    /// A verb with no workspace named: the fuzzy selector picks one (M8).
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

/// Words a flag-spelled verb overrode.
///
/// `--rm` and `--stop` are the *suffix* form of their verbs: appended to a line
/// that already said something else, they win. What they displaced is carried
/// here rather than dropped, because a line that deletes a workspace must not also
/// swallow an instruction it did not carry out — the point of typing the suffix is
/// that the rest of the line is stale, and the risk is that it is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Overridden {
    /// The flag that won, spelled as it was typed.
    pub(crate) flag: &'static str,
    /// The words it displaced, in the order they were typed. [`NonEmpty`]
    /// because nothing displaced is no override at all, not an override of
    /// nothing — which is what the `None` beside it says.
    pub(crate) words: NonEmpty<String>,
}

/// One resolved command line: what to do, and what saying so overrode.
///
/// A pair rather than a field on [`Command`], because only the two flag-spelled
/// verbs can override anything and every other arm would carry an `Option` that is
/// always `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) command: Command,
    pub(crate) overridden: Option<Overridden>,
}

impl Command {
    /// This command, having overridden nothing — every arm but the flag-spelled
    /// verbs'.
    fn alone(self) -> Resolved {
        Resolved {
            command: self,
            overridden: None,
        }
    }
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
    /// `--autorm` on a command the flag is not defined for.
    ///
    /// Refused rather than ignored, and `code` is why. `dl <ws> code --autorm`
    /// returns the moment devpod has told VS Code where to connect — *before* the
    /// editor is attached — so honouring it there would delete the container out
    /// from under a window that is still opening. A silently-dropped flag would
    /// read as "it ran and kept the workspace", which is the one reading that
    /// makes somebody type it again.
    AutormNotAllowed { command: &'static str },
    /// `--force` beside `--autorm`.
    ///
    /// The pair looks like it should compose and must not: the unsaved-work guard
    /// is the whole reason `--autorm` is safe to leave on a recalled line, and a
    /// `--force` habitually appended to that line would destroy work hours later,
    /// unattended, with nobody watching the sentence that explained it.
    AutormForced,
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

    /// Stop a workspace without deleting it. May be appended to a line that
    /// already said something else, and wins over it.
    #[arg(long, group = "what")]
    stop: bool,
    /// Delete a workspace. Refuses if its clone holds work that is nowhere else.
    /// May be appended to a line that already said something else, and wins.
    #[arg(long, group = "what")]
    rm: bool,

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
    /// Stops at work that is nowhere else, exactly as `rm` does.
    #[arg(long)]
    autorm: bool,
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
  dl blooop/devlaunch --autorm       Open it, and delete it when the shell exits
  dl --ls --json                     Every workspace, machine-readable

Workspace commands (dl <workspace> <verb>, or dl <verb> <workspace>):
  up                                 Start it without attaching
  stop                               Stop it
  rm                                 Delete it. Refuses if its clone holds
                                     uncommitted or unpushed work, or if git
                                     cannot read the clone to find out; add
                                     --force to delete it anyway. --force also
                                     counts an already-absent workspace as
                                     deleted, like rm -f.
  code                               Open it in VS Code
  restart                            Stop and start it (no rebuild)
  recreate                           Recreate the container
  reset                              Clean slate: remove everything, recreate
  dotfiles                           Refresh dotfiles (chezmoi update)
  -- <command>                       Run one command inside it

A verb with no workspace named picks one interactively.

--stop and --rm are the same two verbs as flags, and unlike the words they may be
appended to a line that already says something else, which then loses:

  dl owner/repo 'review this pr' --rm --force
  dl stop owner/repo --rm

Both act on owner/repo. It is the shape for recalling the previous line and typing
at the end of it rather than rewriting its front. Whatever the suffix beat is named
on stderr before anything is removed, so a deliberate one and a slip do not look
alike.

'prune' was a second spelling of the rm verb and is retired: it collided with the
--prune flag below, which removes clone directories and no workspace at all. Typing
it says so and names both.

--autorm is the throwaway workspace: dl <ws> --autorm and dl <ws> --autorm -- <cmd>
delete the workspace and its clone once the session ends, the way docker run --rm
does. It stops at work that is nowhere else — the same check dl <ws> rm makes — and
then leaves the workspace standing and says so, which is what makes it safe to leave
on a recalled line. The exit code is the session's, not the removal's.

Those two forms and no others: every verb above refuses --autorm rather than ignoring
it, and 'code' is the one worth knowing about, since it returns while VS Code is still
connecting. --force does not compose with it either — use dl <ws> rm --force when you
mean to delete despite the work. Best-effort by nature: a Ctrl-C during the build, or
a closed terminal, ends dl before the session does and leaves the workspace behind.";

/// What no flag and no verb names: the variables that change a launch.
///
/// Last, below the options table, because reading it is what somebody does once
/// and then puts in a shell profile.
const ENVIRONMENT: &str = "Environment:
  DEVLAUNCH_TIMING=1|json            Write a timing summary to stderr
  DEVLAUNCH_NO_GH_TOKEN=1            Do not forward the host's gh login
  DEVLAUNCH_DOTFILES_ON_ATTACH=1     Refresh dotfiles before every attach";

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
    Stop,
    Remove,
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
            (self.stop, Chosen::Stop),
            (self.rm, Chosen::Remove),
            (self.repos, Chosen::Repos),
            (self.completion_data, Chosen::CompletionData),
            (self.update_cache, Chosen::UpdateCache),
        ]
        .into_iter()
        .find_map(|(given, chosen)| given.then_some(chosen))
    }
}

/// The command `cli` asks for.
///
/// Total over [`Cli`]: every accepted command line is either a [`Command`] or a
/// named [`GrammarError`], and nothing is left to be worked out later.
pub(crate) fn resolve(cli: Cli, argv: &[String]) -> Result<Resolved, GrammarError> {
    match cli.chosen() {
        // The two flag-spelled verbs are the verb-first grammar under another
        // name, so they go through the same words handling and inherit its
        // target-or-selector rule — and, being flags, they are also the one form
        // that can be appended to a line that already said something else.
        Some(Chosen::Stop) => verb_command(&cli, VerbWord::Stop, flag_of(Chosen::Stop)),
        Some(Chosen::Remove) => verb_command(&cli, VerbWord::Remove, flag_of(Chosen::Remove)),
        Some(global) => global_command(&cli, global).map(Command::alone),
        None => workspace_command(cli, argv).map(Command::alone),
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
    if cli.autorm {
        return Err(GrammarError::AutormNotAllowed { command: name });
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
        // Handled by `resolve` before this function is reached: they are verbs,
        // not global commands, and the compiler is the reason that stays true.
        Chosen::Stop | Chosen::Remove => {
            return Err(GrammarError::TargetNotAllowed { command: name });
        }
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
        Chosen::Stop => "--stop",
        Chosen::Remove => "--rm",
        Chosen::Repos => "--repos",
        Chosen::CompletionData => "--completion-data",
        Chosen::UpdateCache => "--update-cache",
    }
}

/// `dl --stop [<target>]` and `dl --rm [<target>]`: the verb-first grammar, and
/// the suffix form of it.
///
/// Unlike the bare words, these two may be *appended* to a line that already said
/// something else, and they win. That is the whole reason they are flags: a shell
/// makes recalling the previous line and typing at the end of it cheap, and
/// rewriting the front of a recalled line expensive, so the only shape that can
/// mean "and now delete it" is a suffix. `dl <ws> 'review this pr' --rm` and
/// `dl prune <ws> --force --rm` both remove `<ws>`, where the first used to be
/// [`GrammarError::UnknownVerb`] and the second took `prune` as the *workspace*.
///
/// The displaced words are named rather than dropped in silence — see
/// [`Overridden`] — with one exception: a word spelling the same verb the flag
/// does displaced nothing, because it is the line saying one thing twice, which is
/// exactly what appending `--rm` to a `prune` line produces.
///
/// **Divergence row 30.** Row 15 made the flag spellings work at all; this is the
/// one thing they do that the words cannot.
fn verb_command(cli: &Cli, word: VerbWord, flag: &'static str) -> Result<Resolved, GrammarError> {
    let verb = word.with(cli.force);
    if !cli.command.is_empty() {
        return Err(GrammarError::CommandNotAllowed { verb: verb.word() });
    }
    // Before `--yes`, because a `--stop --autorm` line is the more confused of the
    // two and the flag it is confused about is the one worth naming.
    if cli.autorm {
        return Err(GrammarError::AutormNotAllowed {
            command: verb.word(),
        });
    }
    if cli.yes {
        return Err(GrammarError::ModifierNotAllowed {
            modifier: "--yes",
            command: verb.word(),
        });
    }
    let devcontainer = devcontainer_of(cli)?;
    let (target, displaced) = pick_target(&cli.words, word);
    Ok(Resolved {
        command: match target {
            None => Command::Select { verb, devcontainer },
            Some(target) => Command::Workspace {
                target,
                verb,
                devcontainer,
            },
        },
        overridden: NonEmpty::of(displaced).map(|words| Overridden { flag, words }),
    })
}

/// The workspace on a `--rm`/`--stop` line, and the words that flag overrode.
///
/// The target is the first word that is *not* a verb, so a verb word standing ahead
/// of the workspace is not mistaken for it. Everything after that first word is
/// displaced, and so is any verb word naming a different verb — except a word
/// spelling `flag`'s own verb, which is redundant rather than displaced.
///
/// No target at all is the selector, as it is for a bare verb: `dl --rm prune` is
/// the remove verb spelled twice and names no workspace, so it picks one.
fn pick_target(words: &[String], flag: VerbWord) -> (Option<String>, Vec<String>) {
    let mut target = None;
    let mut displaced = Vec::new();
    for word in words {
        // A retired verb word stands for what it used to mean, so it is not a
        // workspace name here either. `dl prune <ws> --force --rm` is the line this
        // whole suffix form exists for, and `prune` is exactly the word in it that
        // is no longer a verb — reading it as the target would delete the wrong
        // thing, and displacing it would report noise about a line that asked for
        // one thing twice.
        match VerbWord::of(word).or_else(|| RetiredWord::of(word).map(RetiredWord::was)) {
            Some(spelled) if spelled == flag => {}
            Some(_) => displaced.push(word.clone()),
            None if target.is_none() => target = Some(word.clone()),
            None => displaced.push(word.clone()),
        }
    }
    (target, displaced)
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
    // `--autorm` is a word in that stream — so `dl <ws> --autorm --force` reads
    // `--force` as trailing (the pair, correctly) while `dl --autorm --force <ws>`
    // reads it as the *verb* slot and answers `Unknown command '--force'` about a
    // workspace called `--autorm`, which explains nothing about a line whose real
    // problem is the pair. Asked here, every spelling of the pair gets the sentence
    // that names it.
    //
    // What this gives up is `dl --force --autorm`, which used to be an attach on a
    // workspace *named* `--force` (the slot-0 reading Python's parity preserves) and
    // is now the refused pair. That is the better answer: a line carrying both flags
    // is somebody expecting `--force` to license the removal, not somebody who named
    // a workspace `--force`. The parity reading is untouched for every line that does
    // not also say `--autorm`.
    if cli.autorm && cli.force {
        return Err(GrammarError::AutormForced);
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
                    // `Autorm::No` by the check above, not by choice: a line holding
                    // both flags was refused before it got here.
                    verb: Verb::Attach {
                        autorm: autorm_of(&cli),
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
            // with `--autorm` can be told from a workspace that happens to be
            // called `--force`.
            ForcePlace::Trailing => {}
        }
    }
    let autorm = autorm_of(&cli);
    let run = NonEmpty::of(cli.command.iter().cloned()).map(|words| Verb::Run(words, autorm));
    let attach = || Verb::Attach { autorm };
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
    // The verb words the grammar just resolved carry no [`Autorm`] — only the two
    // attach spellings can — so a `--autorm` that reached one of them was written
    // and could not be honoured. Read off the *verb* rather than the words, so
    // `dl up <ws> --autorm` and `dl <ws> up --autorm` are refused alike.
    if cli.autorm && !matches!(verb, Verb::Attach { .. } | Verb::Run(..)) {
        return Err(GrammarError::AutormNotAllowed {
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
fn autorm_of(cli: &Cli) -> Autorm {
    if cli.autorm {
        Autorm::OnExit
    } else {
        Autorm::No
    }
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

    /// The command a line resolves to, with what it overrode set aside — which is
    /// `None` for everything but the two flag-spelled verbs, and what
    /// [`full`] is for when it is not.
    fn parse(argv: &[&str]) -> Result<Command, GrammarError> {
        full(argv).map(|resolved| resolved.command)
    }

    fn full(argv: &[&str]) -> Result<Resolved, GrammarError> {
        let cli = Cli::try_parse_from(std::iter::once("dl").chain(argv.iter().copied()))
            .unwrap_or_else(|error| panic!("clap refused {argv:?}: {error}"));
        let raw: Vec<String> = argv.iter().map(|word| (*word).to_owned()).collect();
        resolve(cli, &raw)
    }

    /// The words a line overrode, in order, or `None` when it overrode nothing.
    fn overridden(argv: &[&str]) -> Option<(&'static str, Vec<String>)> {
        let resolved = full(argv).expect("a usable command line");
        resolved
            .overridden
            .map(|had| (had.flag, had.words.iter().cloned().collect()))
    }

    fn refused(argv: &[&str]) -> clap::error::ErrorKind {
        Cli::try_parse_from(std::iter::once("dl").chain(argv.iter().copied()))
            .expect_err("clap should refuse this")
            .kind()
    }

    /// `dl <ws>` — the plain attach, which is what almost every case below wants.
    fn attach() -> Verb {
        Verb::Attach { autorm: Autorm::No }
    }

    fn run(words: &[&str]) -> Verb {
        run_with(words, Autorm::No)
    }

    fn run_with(words: &[&str], autorm: Autorm) -> Verb {
        Verb::Run(
            NonEmpty::of(words.iter().map(|word| word.to_string())).expect("a command"),
            autorm,
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
            Ok(workspace("ws", Verb::Remove { force: true }))
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
    fn a_retired_word_is_never_read_as_the_workspace() {
        // The failure mode the RETIRED table exists to prevent: dropped from the
        // verbs and nothing else, `prune` would be an ordinary word — a workspace
        // name in every one of these lines.
        assert_eq!(
            parse(&["prune", "ws", "--force", "--rm"]),
            Ok(workspace("ws", Verb::Remove { force: true })),
            "the recalled line still removes ws, not a workspace called prune"
        );
        // And it displaced nothing: `prune` and `--rm` asked for the same thing.
        assert_eq!(overridden(&["prune", "ws", "--rm"]), None);
        // Asking for something else with it does displace it, and says so.
        assert_eq!(
            overridden(&["prune", "ws", "--stop"]),
            Some(("--stop", vec!["prune".to_owned()]))
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
    fn the_flag_spelled_verbs_are_the_verb_first_grammar() {
        assert_eq!(parse(&["--stop", "ws"]), Ok(workspace("ws", Verb::Stop)));
        assert_eq!(
            parse(&["--rm", "ws", "--force"]),
            Ok(workspace("ws", Verb::Remove { force: true }))
        );
        assert_eq!(
            parse(&["--stop"]),
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

    // ================================================ the flag-spelled verbs as a suffix

    #[test]
    fn a_suffix_verb_overrides_a_stale_word_rather_than_refusing_the_line() {
        // The shape this exists for: an `aid`-style line recalled from history,
        // with `--rm` typed at the end of it. Before, two words neither of which
        // is a verb was `UnknownVerb` and the line did nothing.
        assert_eq!(
            parse(&["owner/repo@fix/x", "review this pr", "--rm", "--force"]),
            Ok(workspace("owner/repo@fix/x", Verb::Remove { force: true }))
        );
        assert_eq!(
            overridden(&["owner/repo@fix/x", "review this pr", "--rm"]),
            Some(("--rm", vec!["review this pr".to_owned()]))
        );
    }

    #[test]
    fn a_leading_verb_word_is_not_mistaken_for_the_workspace() {
        // What `dl prune <ws> --force` + `--rm --force` recalls to. `--rm` used to
        // read `prune` as the target and report an unknown workspace called that.
        assert_eq!(
            parse(&["prune", "ws", "--force", "--rm"]),
            Ok(workspace("ws", Verb::Remove { force: true }))
        );
        // And it displaced nothing: `prune` and `--rm` are one verb spelled twice.
        assert_eq!(overridden(&["prune", "ws", "--rm"]), None);
        assert_eq!(overridden(&["ws", "--rm"]), None);
    }

    #[test]
    fn a_suffix_verb_overrides_a_different_verb_and_says_so() {
        // `dl <ws> stop` recalled with `--rm` appended: the flag wins, and the
        // verb it beat is named, because the two are not the same request.
        assert_eq!(
            parse(&["ws", "stop", "--rm"]),
            Ok(workspace("ws", Verb::Remove { force: false }))
        );
        assert_eq!(
            overridden(&["ws", "stop", "--rm"]),
            Some(("--rm", vec!["stop".to_owned()]))
        );
        assert_eq!(
            overridden(&["ws", "rm", "--stop"]),
            Some(("--stop", vec!["rm".to_owned()]))
        );
    }

    #[test]
    fn a_suffix_verb_with_only_its_own_verb_word_picks_a_workspace() {
        // `dl --rm prune` names no workspace at all — it is the verb twice — so it
        // opens the selector, as the bare `dl prune` does.
        assert_eq!(
            parse(&["--rm", "prune"]),
            Ok(Command::Select {
                verb: Verb::Remove { force: false },
                devcontainer: None
            })
        );
    }

    #[test]
    fn the_notice_names_every_word_the_suffix_beat() {
        assert_eq!(
            crate::render::overridden_notice("--rm", &["review this pr".to_owned()]),
            "--rm overrode the rest of the line: 'review this pr' was not acted on."
        );
        assert_eq!(
            crate::render::overridden_notice(
                "--stop",
                &["code".to_owned(), "and the rest".to_owned()]
            ),
            "--stop overrode the rest of the line: 'code', 'and the rest' were not acted on."
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

    // ================================================================== --autorm

    #[test]
    fn autorm_is_carried_by_the_two_forms_that_hand_over_a_session() {
        assert_eq!(
            parse(&["ws", "--autorm"]),
            Ok(workspace(
                "ws",
                Verb::Attach {
                    autorm: Autorm::OnExit
                }
            ))
        );
        assert_eq!(
            parse(&["ws", "--autorm", "--", "make", "test"]),
            Ok(workspace("ws", run_with(&["make", "test"], Autorm::OnExit)))
        );
        // Position is not the grammar's business: clap strips the flag wherever it
        // sits, and only `--force` has a slot that means something.
        assert_eq!(
            parse(&["--autorm", "ws"]),
            Ok(workspace(
                "ws",
                Verb::Attach {
                    autorm: Autorm::OnExit
                }
            ))
        );
    }

    #[test]
    fn autorm_with_no_workspace_named_picks_one_and_keeps_the_flag() {
        assert_eq!(
            parse(&["--autorm"]),
            Ok(Command::Select {
                verb: Verb::Attach {
                    autorm: Autorm::OnExit
                },
                devcontainer: None
            })
        );
    }

    #[test]
    fn a_dash_dash_with_nothing_after_it_still_carries_the_flag() {
        // The empty command is not a command, and the attach it collapses to is
        // still the attach that was asked to clean up after itself.
        assert_eq!(
            parse(&["ws", "--autorm", "--"]),
            Ok(workspace(
                "ws",
                Verb::Attach {
                    autorm: Autorm::OnExit
                }
            ))
        );
    }

    #[test]
    fn every_verb_word_refuses_the_flag() {
        // `code` is why this is a refusal and not a shrug: it returns the moment
        // devpod has told VS Code where to connect, so honouring `--autorm` there
        // would delete the container out from under a window still opening.
        //
        // `restart`, `recreate` and `reset` are in this list for a different reason
        // and it is worth not conflating them: those three *do* end in a session, so
        // the removal would work behind them. They are refused because the flag is
        // the throwaway workspace rather than a cleanup modifier on every verb that
        // ends in a shell — a scope decision, which is why the sentence names the
        // two forms that work instead of claiming these hand over nothing.
        for word in [
            "up", "stop", "rm", "code", "restart", "recreate", "reset", "dotfiles",
        ] {
            assert_eq!(
                parse(&["ws", word, "--autorm"]),
                Err(GrammarError::AutormNotAllowed { command: word }),
                "dl ws {word} --autorm"
            );
            // And from the other position, since the verb wins from either.
            assert_eq!(
                parse(&[word, "ws", "--autorm"]),
                Err(GrammarError::AutormNotAllowed { command: word }),
                "dl {word} ws --autorm"
            );
        }
    }

    #[test]
    fn the_flag_spelled_verbs_refuse_it_too() {
        // `--rm` and `--stop` are the same two verbs and get the same answer, under
        // the word rather than the flag: the sentence names the verb, not the spelling.
        assert_eq!(
            parse(&["ws", "--rm", "--autorm"]),
            Err(GrammarError::AutormNotAllowed { command: "rm" })
        );
        assert_eq!(
            parse(&["ws", "--stop", "--autorm"]),
            Err(GrammarError::AutormNotAllowed { command: "stop" })
        );
    }

    #[test]
    fn a_command_that_opens_no_workspace_refuses_it() {
        assert_eq!(
            parse(&["--ls", "--autorm"]),
            Err(GrammarError::AutormNotAllowed { command: "--ls" })
        );
        assert_eq!(
            parse(&["--purge", "--autorm"]),
            Err(GrammarError::AutormNotAllowed { command: "--purge" })
        );
    }

    #[test]
    fn force_does_not_compose_with_autorm() {
        // The pair looks like it should and must not: the guard is what makes the
        // flag safe to leave on a recalled line, and a habitual `--force` beside it
        // would destroy work later, unattended, with nobody reading the sentence.
        assert_eq!(
            parse(&["ws", "--autorm", "--force"]),
            Err(GrammarError::AutormForced)
        );
        assert_eq!(
            parse(&["ws", "--autorm", "--force", "--", "make"]),
            Err(GrammarError::AutormForced)
        );
    }

    #[test]
    fn the_pair_is_refused_wherever_force_sits_in_the_line() {
        // `--force`'s meaning is recovered from its position, and `--autorm` is a word
        // in the stream that position is counted in — so without this the same pair
        // gets three different answers depending on where it was typed, two of them
        // describing the wrong problem: `Unknown command '--force'` about a workspace
        // called `--autorm`, and `Unknown workspace '--force'`.
        for line in [
            vec!["ws", "--autorm", "--force"],
            vec!["--autorm", "--force", "ws"],
            vec!["--force", "--autorm", "ws"],
            vec!["--autorm", "ws", "--force"],
            // No workspace at all: the selector's line, refused the same way rather
            // than opening a picker whose choice cannot be honoured.
            vec!["--force", "--autorm"],
        ] {
            assert_eq!(
                parse(&line),
                Err(GrammarError::AutormForced),
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
        assert!(help.contains("dl blooop/devlaunch --autorm"), "{help}");
        assert!(
            help.contains("--autorm is the throwaway workspace"),
            "{help}"
        );
    }
}
