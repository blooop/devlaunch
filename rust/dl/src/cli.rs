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
const VERBS: [(&str, VerbWord); 9] = [
    ("up", VerbWord::Up),
    ("stop", VerbWord::Stop),
    ("rm", VerbWord::Remove),
    // Python's second spelling of `rm`, kept: `dl <ws> prune` is in the help text
    // and in scripts. It is a workspace verb and unrelated to `dl --prune`, which
    // removes clone directories and no workspace at all.
    ("prune", VerbWord::Remove),
    ("code", VerbWord::Code),
    ("recreate", VerbWord::Recreate),
    ("restart", VerbWord::Restart),
    ("reset", VerbWord::Reset),
    ("dotfiles", VerbWord::Dotfiles),
];

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

/// What is being asked of one workspace.
///
/// [`Verb::Run`] carries a [`NonEmpty`] rather than a `Vec`, because `dl <ws> --`
/// with nothing after it is [`Verb::Attach`] and not a run of the empty command —
/// which is exactly what Python decides (`len(args) > 2`), and what an empty
/// vector beside an `Attach` arm could not say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verb {
    /// `dl <ws>` — bring it up and hand over an interactive shell.
    Attach,
    /// `dl <ws> -- <command>` — one command, inside the workspace.
    Run(NonEmpty<String>),
    Up,
    Stop,
    /// `rm` / `prune`. `force` is `--force`: delete despite unsaved work, and
    /// count an already-absent workspace as deleted.
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
            Verb::Attach => "attach",
            Verb::Run(_) => "--",
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

/// A command line clap accepted but `dl` cannot make a command of.
///
/// Kept apart from clap's own errors because the two exit differently: clap's
/// usage errors are exit 2, its convention, while these are the shapes Python
/// also refused and refused with exit 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GrammarError {
    /// Two words, neither of which is a verb.
    UnknownVerb { target: String, word: String },
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

    /// Stop a workspace without deleting it.
    #[arg(long, group = "what")]
    stop: bool,
    /// Delete a workspace. Refuses if its clone holds work that is nowhere else.
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
  dl --ls --json                     Every workspace, machine-readable

Workspace commands (dl <workspace> <verb>, or dl <verb> <workspace>):
  up                                 Start it without attaching
  stop                               Stop it
  rm, prune                          Delete it. Refuses if its clone holds
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

A verb with no workspace named picks one interactively.";

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
pub(crate) fn resolve(cli: Cli, argv: &[String]) -> Result<Command, GrammarError> {
    match cli.chosen() {
        // The two flag-spelled verbs are the verb-first grammar under another
        // name, so they go through the same words handling and inherit its
        // target-or-selector rule.
        Some(Chosen::Stop) => verb_command(&cli, VerbWord::Stop),
        Some(Chosen::Remove) => verb_command(&cli, VerbWord::Remove),
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

/// `dl --stop [<target>]` and `dl --rm [<target>]`: one word at most, and it is
/// the target.
fn verb_command(cli: &Cli, word: VerbWord) -> Result<Command, GrammarError> {
    let verb = word.with(cli.force);
    if !cli.command.is_empty() {
        return Err(GrammarError::CommandNotAllowed { verb: verb.word() });
    }
    if cli.yes {
        return Err(GrammarError::ModifierNotAllowed {
            modifier: "--yes",
            command: verb.word(),
        });
    }
    if cli.words.len() > 1 {
        return Err(GrammarError::UnknownVerb {
            target: cli.words[0].clone(),
            word: cli.words[1].clone(),
        });
    }
    let devcontainer = devcontainer_of(cli)?;
    Ok(match cli.words.first() {
        None => Command::Select { verb, devcontainer },
        Some(target) => Command::Workspace {
            target: target.clone(),
            verb,
            devcontainer,
        },
    })
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
                    verb: Verb::Attach,
                    devcontainer,
                });
            }
            ForcePlace::VerbSlot { target } => {
                return Err(GrammarError::UnknownVerb {
                    target,
                    word: "--force".to_owned(),
                });
            }
            ForcePlace::Trailing => {}
        }
    }
    let run = NonEmpty::of(cli.command.iter().cloned()).map(Verb::Run);
    let (target, verb) = match cli.words.as_slice() {
        [] => (None, run.unwrap_or(Verb::Attach)),
        [only] => match VerbWord::of(only) {
            Some(word) => (None, word.with(cli.force)),
            None => (Some(only.clone()), run.unwrap_or(Verb::Attach)),
        },
        [first, second] => match (VerbWord::of(first), VerbWord::of(second)) {
            // Row 1: the verb wins wherever it is, and the other word is the
            // target. A leading verb is checked first, so `dl stop code` stops
            // the workspace named `code` rather than opening the one named
            // `stop` in an editor.
            (Some(word), _) => (Some(second.clone()), word.with(cli.force)),
            (None, Some(word)) => (Some(first.clone()), word.with(cli.force)),
            (None, None) => {
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
    if !cli.command.is_empty() && !matches!(verb, Verb::Run(_)) {
        return Err(GrammarError::CommandNotAllowed { verb: verb.word() });
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

    fn run(words: &[&str]) -> Verb {
        Verb::Run(NonEmpty::of(words.iter().map(|word| word.to_string())).expect("a command"))
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
    fn prune_is_the_second_spelling_of_rm_as_a_verb() {
        assert_eq!(
            parse(&["ws", "prune"]),
            Ok(workspace("ws", Verb::Remove { force: false }))
        );
        assert_eq!(
            parse(&["ws", "rm", "--force"]),
            Ok(workspace("ws", Verb::Remove { force: true }))
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
        assert_eq!(parse(&["ws"]), Ok(workspace("ws", Verb::Attach)));
        assert_eq!(
            parse(&["owner/repo@feature/x"]),
            Ok(workspace("owner/repo@feature/x", Verb::Attach))
        );
        assert_eq!(parse(&["./here"]), Ok(workspace("./here", Verb::Attach)));
    }

    #[test]
    fn no_arguments_at_all_opens_the_selector() {
        assert_eq!(
            parse(&[]),
            Ok(Command::Select {
                verb: Verb::Attach,
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
        assert_eq!(parse(&["ws", "--"]), Ok(workspace("ws", Verb::Attach)));
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
}
