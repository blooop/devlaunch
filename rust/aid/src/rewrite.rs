//! An `aid` command line, rewritten into a `dl` one.
//!
//! Ported from `devlaunch/aid.py`. Nothing here spawns anything, reads anything or
//! decides anything about a container: the whole module is one string-to-strings
//! function and the agent table it reads. That is the point — an `aid` that drove
//! containers itself is an `aid` that builds one `dl` would have reused, which is
//! the drift `aid.py` was rewritten to end.

use dl::shell;

/// How one coding agent is started inside the workspace.
///
/// Split three ways because not every part of the line belongs everywhere: `env`
/// and `command` always run, while `prompt_flags` only joins them when there is a
/// prompt — gemini's way of taking an initial prompt is a flag that is a syntax
/// error without one.
struct Agent {
    /// The words the agent is started with.
    command: &'static [&'static str],
    /// Words that only make sense beside a prompt.
    prompt_flags: &'static [&'static str],
    /// Variables set for the agent's process and nothing else, sorted by name —
    /// Python sorts them, and the order is in the payload's bytes.
    env: &'static [(&'static str, &'static str)],
}

/// The agent this build starts when nothing picks one.
pub(crate) const DEFAULT_AGENT: &str = "claude";

/// The variable that changes the default, for people who do not want to type a flag
/// every time. A `--flag` on the command line still wins.
pub(crate) const AGENT_ENV_VAR: &str = "DEVLAUNCH_AID_AGENT";

/// The variable that turns [`TABS_FLAG`] off for every line.
///
/// **A "no" variable rather than an opt-in one**, because tabs are the default, and
/// the name follows the family already in core: `DEVLAUNCH_NO_ZELLIJ`,
/// `DEVLAUNCH_NO_TOOLS`, `DEVLAUNCH_NO_TTY`. Read the way those are: anything but
/// empty, `0`, `false` or `no` means yes, turn it off.
pub(crate) const NO_TABS_ENV_VAR: &str = "DEVLAUNCH_AID_NO_TABS";

/// Run the agent inside a zellij session in the container, so a second terminal in
/// that container is a keystroke rather than another launch from the host.
///
/// **On by default**, so the flag's job is not to switch the behaviour on but to say
/// you meant it — which is the whole of what [`UsageError::TabsWithRemoval`] turns
/// on. [`NO_TABS_FLAG`] is the way to spell "not this line".
///
/// aid's own flags, and the only ones that are: every other unrecognised `--word`
/// ahead of the spec is passed through to dl untouched ([`parse_aid_args`]), and
/// these two are not, because dl has never heard of them. They belong on this side
/// of the seam even so -- what they change is the command aid builds, and nothing
/// about how the workspace is obtained, which is the line aid's whole design is
/// drawn on.
///
/// dl's own `DEVLAUNCH_ZELLIJ` is the same session and the opposite arrangement: it
/// puts a session *beside* a command so an agent can open panes into it, and this
/// puts the agent *inside* one so a person can open tabs beside it.
const TABS_FLAG: &str = "--tabs";

/// Run the agent on its own, the way it ran before tabs were the default.
const NO_TABS_FLAG: &str = "--no-tabs";

/// The zellij session the agent runs in.
///
/// **The same name dl's `DEVLAUNCH_ZELLIJ` creates**, and deliberately the same
/// session: one container has one `devlaunch` session, so an agent that opens a pane
/// with `zellij -s devlaunch action new-pane` opens it among the tabs the human is
/// looking at rather than in a second session nobody is attached to.
///
/// The other copy of the literal is `devlaunch_core::flows::launch::ZELLIJ_SESSION`.
/// Two copies rather than one because core's public surface is snapshotted
/// (`devlaunch-core/public-api.*.txt`) and promoting a `pub(crate)` constant across
/// the crate boundary to share it is a contract change for a nine-letter word. What
/// holds them together is that each crate pins the literal in a test of its own, so
/// changing one reddens the other.
pub(crate) const TABS_SESSION: &str = "devlaunch";

/// Base command per agent. The prompt, when there is one, is appended as a single
/// quoted argument; each of these CLIs takes an initial prompt that way and then
/// drops into its interactive session.
///
/// claude is started with `--dangerously-skip-permissions`: the whole point of a dl
/// workspace is that the agent is already inside a disposable container with only
/// this repo in it, so the per-tool prompts it would otherwise ask on the host buy
/// nothing and stop an unattended `aid owner/repo fix the bug` dead.
///
/// `IS_SANDBOX=1` is what makes that flag usable at all here. claude refuses it
/// outright under uid 0 — "cannot be used with root/sudo privileges", exit 1 — and
/// plenty of devcontainers run as root, so without this aid would not start in them
/// at all. The variable is claude's own way of being told the refusal is answering
/// for a machine that isn't there, which is exactly a dl workspace.
///
/// `CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1` is the other half of dl naming the
/// terminal after the workspace, and without it that name lasts about a second.
/// claude writes the terminal title continuously, from its own read of what the
/// session is doing; a multiplexer takes the last writer's word for it, so the two
/// are not two signals but one contest, and claude wins every round after the
/// first. Turning claude's off is what makes dl's stick, and that is the trade
/// worth taking: which workspace a pane *is* does not change, while what claude is
/// doing in it is already on screen inside the pane.
///
/// Set here rather than forwarded from the host because aid is what decided to
/// start claude, so aid is what owes the consequence -- a `dl <ws> -- claude ...`
/// somebody typed themselves is their command, not aid's to rewrite. It also costs
/// no new machinery: this table is already an env prefix on a payload that runs
/// under `bash -lc`.
///
/// In the order Python's dict is written, which is the order `--help` lists the
/// flags in after sorting.
const AGENTS: &[(&str, Agent)] = &[
    (
        "claude",
        Agent {
            command: &["claude", "--dangerously-skip-permissions"],
            prompt_flags: &[],
            env: &[
                ("CLAUDE_CODE_DISABLE_TERMINAL_TITLE", "1"),
                ("IS_SANDBOX", "1"),
            ],
        },
    ),
    (
        "codex",
        Agent {
            command: &["codex"],
            prompt_flags: &[],
            env: &[],
        },
    ),
    (
        "gemini",
        Agent {
            command: &["gemini"],
            prompt_flags: &["--prompt-interactive"],
            env: &[],
        },
    ),
];

/// `dl` options whose value is a separate argument.
///
/// aid splits its own command line before handing it to dl and has to tell such a
/// value from the workspace spec. Python keeps the list in `dl.py`
/// (`DL_VALUE_OPTIONS`) next to the parsing it describes, and it is one entry long;
/// here it is the one thing aid knows about dl's grammar.
const DL_VALUE_OPTIONS: &[&str] = &["--devcontainer"];

/// The modifier the suffix options take, peeled only in their company.
///
/// On its own at the end of a line `--force` is prompt text. It is also the one
/// word that must not reach dl as a *leading* option on its own: dl reads argv
/// positionally to decide whether `--force` is the force flag or the workspace's
/// name, and a `--force` in the workspace slot is a workspace called `--force`.
const SUFFIX_MODIFIERS: &[&str] = &["--force"];

/// dl options a line may end with, which ride *with* the prompt.
///
/// aid's rule is that everything after the spec is prompt, flags and all, which is
/// what lets a prompt go unquoted. These are the one exception, and it is bounded to
/// earn it — see [`peel_suffix`]. They exist because appending to a recalled line is
/// the cheap edit a shell offers and rewriting the front of one is not, so "and then
/// delete it" has to be spellable as a suffix or it is not spellable at all.
///
/// **The prompt survives it, which is the whole difference from what `--rm` used to
/// be.** It means "run the prompt *and then* delete the workspace" — the commonest
/// thing anybody wants of an agent line: send it in, let it work, get the disk back.
///
/// Bounded three ways to earn the exception: only at the very end of the line, only
/// these exact words, and only as whole argv words. `aid <ws> explain the --rm flag`
/// ends on `flag` and is untouched, and a quoted `aid <ws> 'why --rm'` is one
/// argument that is not `--rm`. Divergence row 32.
const SUFFIX_OPTIONS: &[&str] = &[REMOVAL_FLAG];

/// The flag that ties the workspace's life to the session's.
///
/// Named rather than spelled twice: [`SUFFIX_OPTIONS`] peels it and
/// [`UsageError::TabsWithRemoval`] refuses it beside [`TABS_FLAG`], and those two
/// must not be able to disagree about which word they mean.
const REMOVAL_FLAG: &str = "--rm";

/// The retired spellings, peeled for one reason: so dl refuses them by name.
///
/// Left out of the peel they would fall into the *prompt* — aid joins every
/// post-spec word — and `aid <ws> 'review this pr' --autorm` would quietly ask an
/// agent to read `--autorm` where it used to delete the workspace. Peeled, they ride
/// to dl ahead of nothing at all: [`Task::Retired`] builds no agent command, so dl
/// refuses the spelling before a workspace is booted or a prompt is asked for.
///
/// They are not [`SUFFIX_OPTIONS`] because that list is what a *running* line may
/// carry, and both of these end the line instead. Divergence row 32.
const SUFFIX_RETIRED: &[&str] = &["--stop", "--autorm"];

/// Suffix options aid keeps for itself instead of handing on.
///
/// [`SUFFIX_OPTIONS`] ride *to dl*; these never reach it. They are peeled for the
/// same reason and by the same rule -- appending to a recalled line is the cheap
/// edit a shell offers, and rewriting the front of one is not -- and then answered
/// here.
///
/// A word in this list is peeled **independently of** the modifier rule below. That
/// is not a detail: `--force` alone at the end of a line is prompt text, and it has
/// to stay prompt text when a `--tabs` is appended after it, or `aid <ws> talk about
/// --force --tabs` would send dl a `--force` it reads as an unknown verb.
const SUFFIX_AID_OPTIONS: &[&str] = &[TABS_FLAG, NO_TABS_FLAG];

/// Whether the agent runs inside a zellij session, and who decided.
///
/// Three arms rather than a bool, because "on" and "on because you asked" are not
/// the same answer to `--rm`. A line that spelled [`TABS_FLAG`] and `--rm` asked for
/// two things that cannot both happen and is refused by name; a line that only
/// spelled `--rm` asked for one thing, and gets it, with tabs standing down. A bool
/// would have to answer both cases the same way, and either reading is wrong for the
/// other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tabs {
    /// Nobody said. On, because a terminal beside the agent is the thing you wanted
    /// often enough to make it the default.
    Default,
    /// [`TABS_FLAG`] on this line.
    Asked,
    /// [`NO_TABS_FLAG`] on this line, or [`NO_TABS_ENV_VAR`] set, or a `--rm` that
    /// stood the default down.
    Off,
}

impl Tabs {
    /// Whether the agent command gets wrapped.
    pub(crate) fn wanted(self) -> bool {
        matches!(self, Self::Default | Self::Asked)
    }

    /// The answer a word on the command line spells, if it spells one.
    fn from_flag(word: &str) -> Option<Self> {
        match word {
            TABS_FLAG => Some(Self::Asked),
            NO_TABS_FLAG => Some(Self::Off),
            _ => None,
        }
    }
}

/// The names a `--flag` can pick an agent by, sorted — for the help and for the
/// refusal that lists them.
pub(crate) fn agent_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = AGENTS.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    names
}

/// The aid command line could not be understood.
///
/// Two arms, one per thing a person can get wrong. Python's `UsageError` carries the
/// sentence; here the sentence is [`crate::render`]'s and this is what happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UsageError {
    /// No workspace anywhere on the command line.
    NoWorkspace,
    /// `DEVLAUNCH_AID_AGENT` names an agent this build has never heard of. Only the
    /// environment can reach this: a `--flag` that is not an agent's is passed
    /// through to dl, as any other unknown flag is.
    UnknownAgentInEnvironment { name: String },
    /// `--tabs` and `--rm` on one line, a pair that would destroy the work it was
    /// asked to do.
    ///
    /// `--rm` fires when the session dl handed over *ends*, and `--tabs` makes
    /// detaching one of the ways to end it -- so an `Alt d` out of a session whose
    /// agent is still working would delete the workspace from under it, silently,
    /// the way an unattended `--force` would. The two flags mean opposite things
    /// about how long a workspace lives, and the refusal says so rather than
    /// quietly picking one. It is the rule `--rm` and `--force` are already held to.
    TabsWithRemoval,
}

/// An aid command line, split into the pieces the dl one is built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AidArgs {
    /// The workspace, in any spelling dl accepts. Always present: the parse refuses
    /// a command line without one, so there is no "maybe a workspace" state here.
    pub(crate) spec: String,
    /// dl options seen before the spec (`--devcontainer x`), passed through as-is.
    pub(crate) dl_options: Vec<String>,
    /// dl options that must land *after* the spec, from [`Suffix::options`].
    ///
    /// A separate list from `dl_options` because the position is load-bearing rather
    /// than cosmetic: dl reads argv positionally to decide whether a `--force` is the
    /// force flag or the workspace's name, so a flag peeled off the end of the line
    /// has to go back *behind* the spec. `--autorm` would survive either side; the
    /// `--force` that can accompany it would not, and one list that is right for both
    /// beats two rules for one list.
    pub(crate) spec_options: Vec<String>,
    /// Whether the agent runs inside a zellij session in the container.
    ///
    /// Beside [`Task`] rather than an arm of it: it does not change *what* the line
    /// asks for, only what the agent is started inside, and a retired-spelling line
    /// that carries it still has nothing to start.
    pub(crate) tabs: Tabs,
    pub(crate) task: Task,
}

/// What the line asks dl to do with the workspace.
///
/// A sum rather than a prompt beside a bool, because the second arm has no use for
/// a prompt and building one for it would be building a thing to ignore: a line
/// carrying a retired spelling starts no agent, so there is nothing to prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Task {
    /// Start an agent, with this prompt. Empty is a session with no prompt rather
    /// than a prompt that is empty.
    Agent { agent: String, prompt: String },
    /// A line spelling a flag this build has retired ([`SUFFIX_RETIRED`]).
    ///
    /// Carries nothing: what it asks dl for is the *refusal*, which is dl's sentence
    /// to write and needs no argument from here. Naming it as its own arm rather
    /// than letting the line through as an agent is what keeps the interactive flow
    /// from booting a workspace and asking for a prompt on the way to exit 1.
    Retired,
}

impl AidArgs {
    /// The agent this line starts, when it starts one.
    ///
    /// `None` is a retired-spelling line, which starts none — the distinction the
    /// caller needs when reporting an agent name the environment invented.
    pub(crate) fn agent(&self) -> Option<&str> {
        match &self.task {
            Task::Agent { agent, .. } => Some(agent),
            Task::Retired => None,
        }
    }

    /// The same line with the prompt the interactive editor collected.
    ///
    /// A retired-spelling line is returned unchanged — it has no prompt to replace,
    /// and the interactive flow never reaches one — so this cannot turn a refusal
    /// back into a launch.
    pub(crate) fn with_prompt(mut self, typed: String) -> Self {
        if let Task::Agent { prompt, .. } = &mut self.task {
            *prompt = typed;
        }
        self
    }
}

/// The dl command line that boots the workspace without attaching to it.
///
/// `[<dl options>…, <spec>, "up"]` — dl's own warm-up verb, which is idempotent
/// and hands over no session. This is what the interactive flow runs in the
/// background while the prompt is being typed, so it deliberately carries **no
/// suffix options**: `--rm` beside `up` is a pair dl refuses by name, `up` hands
/// over no session for a removal to wait on anyway, and the flag still rides on the
/// foreground attach line where it means what it means.
pub(crate) fn build_boot_args(parsed: &AidArgs) -> Vec<String> {
    let mut args = parsed.dl_options.clone();
    args.push(parsed.spec.clone());
    args.push("up".to_owned());
    args
}

/// Whether a `DEVLAUNCH_*` switch of this family is on.
///
/// dl's rule and dl's exact list: anything but empty, `0`, `false` or `no` means
/// yes, after stripping and case-folding. Spelled here rather than reached for
/// across the crate boundary, for the reason `aid/Cargo.toml` gives for that
/// boundary existing -- and for the reason core's own switches each keep a copy:
/// two escape hatches answering to one shared constant are one edit away from
/// becoming one escape hatch.
fn switched_on(value: Option<&str>) -> bool {
    // `str::trim` plus the four separators Python's `str.strip` also takes, which is
    // what core's `osext::strip` means and what the values were read by there.
    let stripped = value
        .unwrap_or("")
        .trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c));
    !matches!(stripped.to_lowercase().as_str(), "" | "0" | "false" | "no")
}

/// Whether the line asks for the workspace to be deleted when the session ends.
///
/// Both lists, because `--rm` reaches aid from either side of the spec: peeled off
/// the end into the suffix options, or passed through from ahead of it as one more
/// unrecognised dl flag.
fn names_a_removal(dl_options: &[String], spec_options: &[String]) -> bool {
    dl_options
        .iter()
        .chain(spec_options)
        .any(|word| word == REMOVAL_FLAG)
}

/// The agent to use when no flag picks one.
pub(crate) fn default_agent(environment: Option<&str>) -> Result<String, UsageError> {
    let name = environment.unwrap_or("").trim();
    if name.is_empty() {
        return Ok(DEFAULT_AGENT.to_owned());
    }
    if !AGENTS.iter().any(|(known, _)| *known == name) {
        return Err(UsageError::UnknownAgentInEnvironment {
            name: name.to_owned(),
        });
    }
    Ok(name.to_owned())
}

/// Whether a peeled run names a spelling this build has retired.
///
/// Derived from the run rather than carried beside it: a `retired` field would be a
/// second copy of a fact the words already state, and the two could disagree.
fn names_a_retired_spelling(options: &[String]) -> bool {
    options
        .iter()
        .any(|word| SUFFIX_RETIRED.contains(&word.as_str()))
}

/// Split a trailing run of dl flags off the end of a command line.
///
/// The one exception to "everything after the spec is prompt", and bounded three
/// ways to earn it: only at the very *end* of the line, only the exact words in
/// [`SUFFIX_OPTIONS`], [`SUFFIX_RETIRED`] and [`SUFFIX_MODIFIERS`], and only when an
/// option or a retired spelling is among them. A prompt whose last word happens to
/// be `--force` is untouched, and so is one that merely mentions `--rm` inside it —
/// `aid <ws> fix the --rm flag` ends on `flag`, and a quoted `aid <ws> 'drop --rm'`
/// is one argument that is not `--rm`.
///
/// A `--force` in the run is peeled with them, which is what gets `aid <ws> <prompt>
/// --rm --force` the sentence dl has for that pair instead of a `--force` silently
/// glued onto the prompt.
///
/// **Options come out ahead of modifiers, whichever order they were typed in**, and
/// that is not cosmetic. dl recovers `--force`'s meaning from its *position* in the
/// word stream — index 1 is the verb slot, and a `--force` there is an unknown verb
/// rather than the modifier. aid emits the spec at index 0, so a `--force` reaching
/// index 1 is exactly `aid <ws> <prompt> --force --autorm`, which used to answer
/// `Unknown command '--force'` about a line whose real problem is the pair. One
/// option ahead of it puts it at index 2, where dl reads it as the modifier and
/// refuses the pair by name. Reordering two flags is safe in a way reordering words
/// is not: clap takes them in any order, and only the positional recovery cares.
///
/// Answers `None` rather than an empty suffix, so the caller has no "peeled
/// nothing" case to tell from "peeled something".
///
/// **Divergence row 30**, aid's half: Python joined every post-spec word into the
/// prompt with no exception, so `aid <ws> <prompt> --rm` asked an agent to read
/// `--rm`.
fn peel_suffix(argv: &[String]) -> Option<Peeled> {
    let is_suffix = |word: &str| {
        SUFFIX_OPTIONS.contains(&word)
            || SUFFIX_RETIRED.contains(&word)
            || SUFFIX_MODIFIERS.contains(&word)
            || SUFFIX_AID_OPTIONS.contains(&word)
    };
    let mut at = argv.len();
    while at > 0 && is_suffix(argv[at - 1].as_str()) {
        at -= 1;
    }
    let run = &argv[at..];
    // aid's own words come out first and are answered here; what is left is the run
    // dl's rules are applied to, exactly as if they had never been typed. That order
    // is what keeps `--tabs` from changing how the words beside it are read.
    //
    // The *last* of them wins, so `--tabs --no-tabs` reads left to right like any
    // other pair of contradicting flags.
    let tabs = run
        .iter()
        .filter_map(|word| Tabs::from_flag(word))
        .next_back();
    let rest: Vec<String> = run
        .iter()
        .filter(|word| !SUFFIX_AID_OPTIONS.contains(&word.as_str()))
        .cloned()
        .collect();
    let names = |list: &[&str]| rest.iter().any(|word| list.contains(&word.as_str()));
    // A run of nothing but modifiers is prompt text, which is the rule `--force`
    // alone has always been read by — and still is with a `--tabs` beside it, which
    // is why this asks `rest` rather than `run`.
    let carries_dl_options = names(SUFFIX_OPTIONS) || names(SUFFIX_RETIRED);
    if tabs.is_none() && !carries_dl_options {
        return None;
    }
    let mut line: Vec<String> = argv[..at].to_vec();
    if !carries_dl_options {
        // Nothing here was dl's after all: the modifiers go back to being the prompt
        // words they were, minus the `--tabs` that was never part of it.
        line.extend(rest);
        return Some(Peeled {
            line,
            dl_options: Vec::new(),
            tabs,
        });
    }
    // Modifiers held back and appended, so an option always precedes one — see the
    // positional argument above. Typed order is kept *within* each group.
    let mut options: Vec<String> = Vec::new();
    let mut modifiers: Vec<String> = Vec::new();
    for word in rest {
        if SUFFIX_MODIFIERS.contains(&word.as_str()) {
            modifiers.push(word);
        } else {
            options.push(word);
        }
    }
    options.append(&mut modifiers);
    Some(Peeled {
        line,
        dl_options: options,
        tabs,
    })
}

/// What [`peel_suffix`] took off the end of a command line.
///
/// Three fields rather than a tuple because the first two are both `Vec<String>` and
/// swapping them would compile.
struct Peeled {
    /// The command line with the trailing run removed — and with the words that
    /// turned out to be prompt after all put back.
    line: Vec<String>,
    /// The peeled options that ride on to dl, ordered as [`peel_suffix`] describes.
    dl_options: Vec<String>,
    /// What the run said about tabs, if it said anything.
    tabs: Option<Tabs>,
}

/// Split an aid command line into agent, dl options, workspace spec, and the task.
///
/// The first argument that is neither an agent flag nor a dl option is the workspace
/// spec; everything after it is the prompt, flags and all, so a prompt never has to
/// be quoted to protect it from aid's own parsing — except a trailing dl option,
/// which [`peel_suffix`] takes off first and which rides beside the prompt.
///
/// A `--rm` can also arrive *before* the spec (`aid --rm owner/repo fix it`), which
/// is the same request written the other way round: an unrecognised leading flag is
/// passed through to dl, and dl takes it in either position.
pub(crate) fn parse_aid_args(
    argv: &[String],
    environment: Option<&str>,
    no_tabs_environment: Option<&str>,
) -> Result<AidArgs, UsageError> {
    // Resolved before anything else, and kept even for a line that turns out to
    // start no agent: a `DEVLAUNCH_AID_AGENT` naming an agent that does not exist
    // is broken regardless of what this particular line asked for.
    let mut agent = default_agent(environment)?;
    // The environment is the starting position, and a flag on the line moves it in
    // either direction from there.
    let mut tabs = if switched_on(no_tabs_environment) {
        Tabs::Off
    } else {
        Tabs::Default
    };
    let peeled = peel_suffix(argv);
    let (line, trailing): (&[String], Vec<String>) = match &peeled {
        Some(peeled) => (&peeled.line, peeled.dl_options.clone()),
        None => (argv, Vec::new()),
    };
    let mut dl_options: Vec<String> = Vec::new();
    let mut spec: Option<String> = None;
    let mut at = 0;
    while at < line.len() {
        let word = line[at].as_str();
        if let Some(named) = agent_flag(word) {
            agent = named.to_owned();
            at += 1;
            continue;
        }
        // Ahead of the pass-through below, which is what keeps them from reaching
        // dl: these are aid's own flags and dl has no entry for either.
        if let Some(asked) = Tabs::from_flag(word) {
            tabs = asked;
            at += 1;
            continue;
        }
        if DL_VALUE_OPTIONS.contains(&word) {
            // Take the value with it; dl reports a missing one.
            dl_options.extend(line[at..line.len().min(at + 2)].iter().cloned());
            at += 2;
            continue;
        }
        if word.starts_with('-') {
            // aid does not need to know every dl flag to stay out of its way.
            dl_options.push(word.to_owned());
            at += 1;
            continue;
        }
        spec = Some(word.to_owned());
        at += 1;
        break;
    }
    let Some(spec) = spec else {
        return Err(UsageError::NoWorkspace);
    };
    let prompt = line[at.min(line.len())..].join(" ");
    // Applied after the loop, because a flag peeled off the *end* of the line is
    // later on it than one written ahead of the spec: `aid --tabs <ws> … --no-tabs`
    // reads left to right like every other pair.
    if let Some(peeled) = peeled.as_ref().and_then(|peeled| peeled.tabs) {
        tabs = peeled;
    }
    let retired = names_a_retired_spelling(&trailing);
    // `--rm` and tabs cannot both happen: `--rm` deletes the workspace when the
    // session ends, and tabs make detaching one of the ways to end it, so an `Alt+d`
    // out of a session whose agent is still working would delete the workspace from
    // under it.
    //
    // Which of the two gives way is decided by who asked. A line that spelled
    // `--tabs` asked for both and is refused by name; a line that only spelled
    // `--rm` asked for one thing and gets it, with the default standing down. That
    // asymmetry is the whole reason [`Tabs`] has three arms: refusing every `--rm`
    // because tabs became the default would break the flag for everyone who was
    // already using it, and silently deleting a workspace somebody explicitly asked
    // to keep a terminal in is the accident this guard exists to stop.
    if names_a_removal(&dl_options, &trailing) {
        if tabs == Tabs::Asked {
            return Err(UsageError::TabsWithRemoval);
        }
        tabs = Tabs::Off;
    }
    Ok(AidArgs {
        spec,
        dl_options,
        spec_options: trailing,
        tabs,
        task: if retired {
            Task::Retired
        } else {
            Task::Agent { agent, prompt }
        },
    })
}

/// Which agent `--gemini` and friends name.
fn agent_flag(word: &str) -> Option<&'static str> {
    let named = word.strip_prefix("--")?;
    AGENTS
        .iter()
        .map(|(name, _)| *name)
        .find(|name| *name == named)
}

/// The shell command that starts the agent inside the workspace.
///
/// One shell string, because that is what dl's `-- <command>` form takes. The prompt
/// is quoted here rather than reassembled by the caller, so the words the user typed
/// reach the agent as the single argument they meant.
///
/// `None` is an agent this build has no entry for, which only a caller inventing a
/// name can produce — [`parse_aid_args`] answers with a name from the table.
pub(crate) fn build_agent_command(agent: &str, prompt: &str) -> Option<String> {
    let (_, started) = AGENTS.iter().find(|(name, _)| *name == agent)?;
    // No prompt to be interactive about: start the agent's plain session, without
    // the flags that only make sense alongside one.
    let mut words: Vec<&str> = started.command.to_vec();
    if !prompt.is_empty() {
        words.extend(started.prompt_flags.iter().copied());
        words.push(prompt);
    }
    // Assignments prefixing a command set the variables for that command only, so
    // the agent is the one process that sees them and nothing in the login shell dl
    // runs this under is changed.
    let mut line: Vec<String> = started
        .env
        .iter()
        .map(|(name, value)| format!("{name}={}", shell::quote(value)))
        .collect();
    line.push(shell::join(words));
    Some(line.join(" "))
}

/// The zellij configuration a nested session is given.
///
/// **Every binding is `Alt`, and that is the whole design.** This session is opened
/// from inside another multiplexer more often than not -- that is what it is for --
/// and the outer one reads every key first, so a session keeping zellij's defaults
/// is one whose `Ctrl t` never arrives. `Alt` is taken by neither zellij's defaults
/// (`Ctrl p/t/n/h/s/o/g/q`) nor tmux's prefix, so the inner session answers its own
/// keys without the outer one being put into a pass-through mode first. Nesting
/// stops being modal, which is the difference between this and "press the lock key,
/// then press the key you wanted".
///
/// `clear-defaults=true` is the other half: what is not bound here is not captured
/// at all, so every other key belongs to the program in the pane. A nested
/// multiplexer holding eight prefixes is a nested multiplexer breaking eight things
/// in the agent running inside it.
///
/// The three lines that are not keys are each here for a reason a launch would
/// otherwise show you:
///
/// - `show_startup_tips false` -- a fresh session otherwise opens a floating "About
///   Zellij" pane *over* the agent, which is the first thing the flag would be
///   judged by.
/// - `on_force_close "detach"` -- the connection dying has to leave the agent
///   working, since surviving a dropped connection is half of why the session is
///   there. It is already zellij's default; it is spelled out because the feature
///   leans on it.
/// - `default_layout "compact"` -- one status line rather than a tab bar *and* a
///   status bar, because the outer session is already spending rows on its own.
///
/// Deliberately not a theme and not a plugin: this file is what makes the keys work.
/// A container whose zellij is already configured says so with `ZELLIJ_CONFIG_FILE`,
/// which [`tabs_command`] hands over to untouched.
const TABS_CONFIG: &[&str] = &[
    "// Written by aid. This session runs inside another multiplexer, so every",
    "// binding here is Alt: the outer session reads Ctrl and the function keys first.",
    "default_layout \"compact\"",
    "pane_frames false",
    "mouse_mode true",
    "show_startup_tips false",
    "show_release_notes false",
    "on_force_close \"detach\"",
    "keybinds clear-defaults=true {",
    "    normal {",
    "        bind \"Alt t\" { NewTab; }",
    "        bind \"Alt n\" { GoToNextTab; }",
    "        bind \"Alt p\" { GoToPreviousTab; }",
    "        bind \"Alt d\" { Detach; }",
    "        bind \"Alt 1\" { GoToTab 1; }",
    "        bind \"Alt 2\" { GoToTab 2; }",
    "        bind \"Alt 3\" { GoToTab 3; }",
    "        bind \"Alt 4\" { GoToTab 4; }",
    "        bind \"Alt 5\" { GoToTab 5; }",
    "    }",
    "}",
];

/// The name of the tab the agent runs in.
///
/// **One tab, and no second one opened for you**, which is a decision that changed
/// when tabs became the default. A pre-opened `shell` tab advertises `Alt t` nicely,
/// and it also means quitting the agent no longer ends the session: the last tab is
/// still standing, so you are left inside zellij needing a second exit. That is a
/// fine price for a flag somebody typed and the wrong one to charge every `aid`
/// session, so the session is one tab that closes when the agent does
/// (`close_on_exit=true`), and how you leave is what it has always been -- quit the
/// agent and you are back on the host. Press `Alt t` and you have opted into a
/// session that outlives it.
const TABS_AGENT_TAB: &str = "agent";

/// What a container with no zellij is told, once per launch.
///
/// **Silence is the wrong answer here, and finding that out cost a round trip.**
/// The three pre-checks exist so the flag never costs a launch, and two of them
/// explain themselves: a piped run has no terminal to say anything to, and a
/// `--no-tabs` line asked for this. The third does not. zellij arrives on the setup
/// pass, which runs on `devpod up` and is therefore skipped by every attach to a
/// workspace that is already running -- so a container that has never been up since
/// the stage landed has no zellij, and nothing about the launch looks different.
/// Somebody presses `Alt+t`, nothing happens, and there is no thread to pull.
///
/// One line, on stderr, only when a terminal is there to read it, naming the command
/// that fixes it. It repeats until the workspace is restarted, which is the point:
/// it is not a warning about a risk, it is a thing to go and do.
const TABS_NO_ZELLIJ_NOTICE: &str = "devlaunch: no zellij in this workspace, so the agent \
     has no tabs beside it -- 'dl <workspace> restart' installs it";

/// The word that ends the heredoc carrying the agent command.
///
/// Named rather than written twice, because [`tabs_command`] has to *check* it: the
/// prompt editor drains a pasted newline into the prompt, so an agent command can be
/// more than one line, and a line of it equal to this word would end the heredoc
/// early and leave the rest of the prompt standing as shell. Absurd to type on
/// purpose and free to rule out, which is what a guard is for.
const TABS_AGENT_DELIMITER: &str = "DEVLAUNCH_TABS_AGENT";

/// The variable naming the file the agent's exit status is written to.
///
/// zellij exits 0 whether the session ended on a clean agent or a failing one, and
/// `dl <ws> -- cmd` reports the command's own status -- a contract this must not
/// quietly drop just by being the default. So the agent's script records `$?` and
/// the shell around zellij exits with it.
///
/// It travels as an environment variable rather than a path written into the script
/// because a pane inherits the environment of the shell that started the session,
/// which keeps the script two fixed lines with nothing interpolated into either.
const TABS_STATUS_VAR: &str = "DEVLAUNCH_TABS_STATUS";

/// `agent_command`, wrapped so it runs inside a zellij session in the container.
///
/// # The shape, and why this one
///
/// Three files are written into devlaunch's cache directory and then zellij is
/// asked for a session built from them: a config ([`TABS_CONFIG`]), a one-line
/// script holding `agent_command`, and a layout naming that script. The session is
/// created with `-n <layout>` -- `--new-session-with-layout`, which is the only
/// spelling that *creates*; a `-s <name> --layout <file>` adds tabs to a session
/// that has to exist already and answers `There is no active session!` when it does
/// not.
///
/// Two things fall out of building it from a layout rather than from a running
/// session's actions, and both were arrived at by watching the other way fail:
///
/// - **The agent command is never quoted into anything but a file.** It reaches the
///   pane as `bash -l <path>`, so [`shell::quote`]'s quoting is the only quoting
///   involved and a prompt holding a quote, a backslash or a `$` needs no second
///   escaping scheme layered on top. Naming the command *in* the layout would have
///   put that same text inside a KDL string, whose escapes differ between KDL 1 and
///   2 -- a versioned escaping rule underneath somebody's prompt.
/// - **Nothing depends on what is focused.** A layout states which tab has focus.
///   The actions that would have built the same thing (`rename-tab`, `go-to-tab`)
///   read the focused tab, and on a session with no client attached `rename-tab`
///   refuses outright: *"Cannot rename the focused tab: no client is attached to
///   this session."*
///
/// # What it refuses to break
///
/// Three pre-checks, and failing any of them runs `agent_command` exactly as it
/// would have run without the flag:
///
/// - **No terminal** (`[ -t 1 ]`) -- a piped run, or `DEVLAUNCH_NO_TTY=1`, has
///   nothing to attach a session to.
/// - **No zellij** -- a container the provisioning stage never reached, which core's
///   own zellij script is already written to tolerate.
/// - **Nowhere to write the three files** -- a read-only or full cache directory.
///
/// Past those three, zellij's exit status is the launch's, and deliberately: a
/// zellij that is installed, has a config it could write and still cannot open a
/// session is a broken container, and quietly starting the agent bare would hide
/// that rather than report it. The rule is "cost the feature, not the launch" up to
/// the point where the feature is the only thing that can explain the failure.
///
/// `ZELLIJ_CONFIG_FILE` wins over [`TABS_CONFIG`], which is the escape hatch for a
/// container whose zellij is already set up the way its owner wants. It is zellij's
/// own variable rather than a new `DEVLAUNCH_*` one because zellij already reads it
/// (`-c, --config … [env: ZELLIJ_CONFIG_FILE=]`), and a switch that already exists
/// beats a switch that has to be learned.
pub(crate) fn tabs_command(agent_command: &str) -> String {
    // A command carrying the delimiter on a line of its own cannot go in the heredoc,
    // so it goes nowhere: the agent runs unwrapped, which is the answer the three
    // pre-checks give and for the same reason.
    if agent_command
        .lines()
        .any(|line| line == TABS_AGENT_DELIMITER)
    {
        return agent_command.to_owned();
    }
    let session = TABS_SESSION;
    let agent_tab = TABS_AGENT_TAB;
    let status_var = TABS_STATUS_VAR;
    let delimiter = TABS_AGENT_DELIMITER;
    let notice = shell::quote(TABS_NO_ZELLIJ_NOTICE);
    let config = TABS_CONFIG.join("\n");
    // Newlines and quoted heredocs, not `;` and a `printf` per line, and the reason
    // is the *echo*. dl quotes this whole command back at the terminal on every
    // launch (`aid -> dl …`), so every single quote inside it returns as the
    // four-character `'"'"'` -- and a config written by `printf '%s\n' 'line'
    // 'line'` is forty of them before anything has run. A heredoc body is quoted by
    // nothing and holds no single quote of its own, KDL's strings being
    // double-quoted, so what comes back is the script as written.
    //
    // The layout's heredoc is the one deliberately *unquoted* delimiter: `$dldir`
    // has to expand, since the script's path is not known until the container's
    // `$HOME` is. Nothing else in that body is a `$`, a backtick or a backslash.
    format!(
        "\
zj() {{ if [ -n \"$dlcfg\" ]; then zellij --config \"$dlcfg\" \"$@\"; else zellij \"$@\"; fi; }}
dlcfg=\"${{ZELLIJ_CONFIG_FILE:-}}\"
dldir=\"${{XDG_CACHE_HOME:-$HOME/.cache}}/devlaunch\"
dltabs=\"\"
if [ -t 1 ] && ! command -v zellij >/dev/null 2>&1; then echo {notice} >&2; fi
{status_var}=\"$dldir/tabs-status\"
export {status_var}
if [ -t 1 ] && command -v zellij >/dev/null 2>&1 && mkdir -p \"$dldir\" 2>/dev/null; then
  dltabs=1
  if [ -z \"$dlcfg\" ]; then
    dlcfg=\"$dldir/tabs.kdl\"
    {{ cat > \"$dlcfg\"; }} 2>/dev/null <<\"DEVLAUNCH_TABS_CONFIG\" || dlcfg=\"\"
{config}
DEVLAUNCH_TABS_CONFIG
  fi
  {{ cat > \"$dldir/tabs-agent.sh\"; }} 2>/dev/null <<\"{delimiter}\" || dltabs=\"\"
{agent_command}
echo $? > \"${{{status_var}:-/dev/null}}\"
{delimiter}
  {{ cat > \"$dldir/tabs-layout.kdl\"; }} 2>/dev/null <<DEVLAUNCH_TABS_LAYOUT || dltabs=\"\"
layout {{
    tab name=\"{agent_tab}\" focus=true {{
        pane command=\"bash\" close_on_exit=true {{
            args \"-l\" \"$dldir/tabs-agent.sh\"
        }}
    }}
}}
DEVLAUNCH_TABS_LAYOUT
fi
if [ -z \"$dltabs\" ]; then
  {agent_command}
else
  rm -f \"${status_var}\" 2>/dev/null
  if zj ls -s 2>/dev/null | grep -qx {session}; then
    zj attach {session}
  else
    zj -s {session} -n \"$dldir/tabs-layout.kdl\"
  fi
  dlrc=$?
  dlsaved=\"$(cat \"${status_var}\" 2>/dev/null)\"
  case \"$dlsaved\" in \"\"|*[!0-9]*) ;; *) dlrc=\"$dlsaved\" ;; esac
  exit \"$dlrc\"
fi"
    )
}

/// The dl command line that does the work.
///
/// `[<dl options>…, <spec>, "--", <agent command>]` — the shape `dl` reads back by
/// joining everything after `--` with spaces, which is why the agent command is one
/// argument and its quoting lives inside it.
///
/// `--rm` lands between the spec and the `--`, so the agent still gets its prompt and
/// dl still gets the flag: `[…, <spec>, "--rm", "--", <agent command>]`.
///
/// A retired-spelling line is `[<dl options>…, <spec>, <options>…]` and has **no `--`
/// tail**: there is no agent to start, and a command tail would only give dl a second
/// thing to complain about ahead of the spelling that is the actual problem. The
/// options still go *after* the spec so that a `--force` among them lands where dl
/// reads it as the modifier rather than as the workspace's name.
pub(crate) fn build_dl_args(parsed: &AidArgs) -> Option<Vec<String>> {
    let mut args = parsed.dl_options.clone();
    args.push(parsed.spec.clone());
    // Behind the spec and ahead of the verb flags, which is where dl reads them as
    // modifiers rather than as the workspace's name.
    args.extend(parsed.spec_options.iter().cloned());
    match &parsed.task {
        Task::Agent { agent, prompt } => {
            args.push("--".to_owned());
            let command = build_agent_command(agent, prompt)?;
            // The wrap goes on last and on the whole command, so what runs inside the
            // session is byte for byte what would have run without the flag.
            args.push(if parsed.tabs.wanted() {
                tabs_command(&command)
            } else {
                command
            });
        }
        Task::Retired => {}
    }
    Some(args)
}

#[cfg(test)]
mod tests {
    //! `test/unit/test_aid.py`'s three parsing classes, which are the whole of aid's
    //! own behaviour: what the command line splits into, what shell command comes
    //! out, and what dl is handed.

    use super::*;

    fn words(argv: &[&str]) -> Vec<String> {
        argv.iter().map(|word| (*word).to_owned()).collect()
    }

    fn parsed(argv: &[&str]) -> AidArgs {
        parse_aid_args(&words(argv), None, None).expect("a usable command line")
    }

    /// The prompt an agent line carries. Panics on a retired-spelling line, which
    /// carries none.
    fn prompt(parsed: &AidArgs) -> &str {
        match &parsed.task {
            Task::Agent { prompt, .. } => prompt,
            Task::Retired => panic!("a retired-spelling line has no prompt"),
        }
    }

    // ------------------------------------------------------- tabs

    fn tabs_parsed(argv: &[&str]) -> AidArgs {
        parse_aid_args(&words(argv), None, None).expect("a usable command line")
    }

    #[test]
    fn a_line_that_says_nothing_about_tabs_gets_them() {
        let parsed = tabs_parsed(&["owner/repo", "fix", "the", "bug"]);

        assert_eq!(parsed.tabs, Tabs::Default);
        assert!(parsed.tabs.wanted());
    }

    #[test]
    fn the_flags_are_aids_own_and_neither_reaches_dl() {
        for flag in [TABS_FLAG, NO_TABS_FLAG] {
            let parsed = tabs_parsed(&[flag, "owner/repo", "fix", "the", "bug"]);

            // dl has no entry for either, so passing one through the way an unknown
            // flag is passed through would end the launch on dl's refusal.
            assert!(
                parsed.dl_options.is_empty(),
                "{flag}: {:?}",
                parsed.dl_options
            );
            assert!(
                parsed.spec_options.is_empty(),
                "{flag}: {:?}",
                parsed.spec_options
            );
            assert_eq!(parsed.spec, "owner/repo");
            assert_eq!(prompt(&parsed), "fix the bug");
            let built = build_dl_args(&parsed).expect("a dl command line");
            assert!(!built.iter().any(|word| word == flag), "{flag}: {built:?}");
        }
    }

    #[test]
    fn no_tabs_turns_them_off_from_either_end_of_the_line() {
        assert_eq!(
            tabs_parsed(&["--no-tabs", "owner/repo", "fix"]).tabs,
            Tabs::Off
        );

        let appended = tabs_parsed(&["owner/repo", "fix", "the", "bug", "--no-tabs"]);
        assert_eq!(appended.tabs, Tabs::Off);
        assert_eq!(prompt(&appended), "fix the bug");
    }

    #[test]
    fn the_last_of_two_contradicting_flags_wins() {
        // Read left to right, like any other pair — including across the spec, where
        // one was written ahead of it and the other peeled off the end.
        assert_eq!(
            tabs_parsed(&["owner/repo", "fix", "--tabs", "--no-tabs"]).tabs,
            Tabs::Off
        );
        assert_eq!(
            tabs_parsed(&["owner/repo", "fix", "--no-tabs", "--tabs"]).tabs,
            Tabs::Asked
        );
        assert_eq!(
            tabs_parsed(&["--tabs", "owner/repo", "fix", "--no-tabs"]).tabs,
            Tabs::Off
        );
        assert_eq!(
            tabs_parsed(&["--no-tabs", "owner/repo", "fix", "--tabs"]).tabs,
            Tabs::Asked
        );
    }

    #[test]
    fn a_prompt_that_merely_mentions_a_tabs_flag_is_still_a_prompt() {
        // The bound every suffix option is held to: the run has to reach the *end*
        // of the line.
        let parsed = tabs_parsed(&["owner/repo", "explain", "--tabs", "to", "me"]);

        assert_eq!(parsed.tabs, Tabs::Default);
        assert_eq!(prompt(&parsed), "explain --tabs to me");
    }

    #[test]
    fn a_trailing_force_beside_a_tabs_flag_is_still_prompt_text() {
        // `--force` alone at the end of a line has always been prompt text, and a
        // `--no-tabs` appended after it must not promote it into a flag — dl reads a
        // `--force` arriving in the verb slot as an unknown verb and refuses the
        // whole launch.
        let parsed = tabs_parsed(&["owner/repo", "talk", "about", "--force", "--no-tabs"]);

        assert_eq!(parsed.tabs, Tabs::Off);
        assert_eq!(prompt(&parsed), "talk about --force");
        assert!(parsed.spec_options.is_empty(), "{:?}", parsed.spec_options);
    }

    #[test]
    fn a_tabs_flag_rides_beside_the_options_that_do_reach_dl() {
        let parsed = tabs_parsed(&["owner/repo", "fix", "it", "--autorm", "--tabs", "--force"]);

        assert_eq!(parsed.tabs, Tabs::Asked);
        assert_eq!(parsed.task, Task::Retired);
        assert_eq!(parsed.spec_options, words(&["--autorm", "--force"]));
    }

    // ------------------------------------------------------- tabs beside --rm

    #[test]
    fn rm_on_its_own_stands_the_default_down_rather_than_being_refused() {
        // The compatibility case, and the reason `Tabs` has three arms: `--rm` was a
        // working flag before tabs were the default, and every line still using it
        // has to keep working exactly as it did.
        let parsed = tabs_parsed(&["owner/repo", "fix", "it", "--rm"]);

        assert_eq!(parsed.tabs, Tabs::Off);
        assert_eq!(prompt(&parsed), "fix it");
        // …and `--rm` still reaches dl, behind the spec where dl reads it.
        assert_eq!(parsed.spec_options, words(&["--rm"]));
        let built = build_dl_args(&parsed).expect("a dl command line");
        assert_eq!(
            built.last().map(String::as_str),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions 'fix it'"
            )
        );
    }

    #[test]
    fn an_explicit_tabs_beside_rm_is_refused_from_either_side_of_the_spec() {
        for argv in [
            &["owner/repo", "fix", "it", "--tabs", "--rm"][..],
            &["owner/repo", "fix", "it", "--rm", "--tabs"][..],
            &["--rm", "--tabs", "owner/repo", "fix", "it"][..],
            &["--tabs", "owner/repo", "fix", "it", "--rm"][..],
        ] {
            assert_eq!(
                parse_aid_args(&words(argv), None, None),
                Err(UsageError::TabsWithRemoval),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn no_tabs_beside_rm_is_the_pair_that_always_worked() {
        let parsed = tabs_parsed(&["owner/repo", "fix", "it", "--no-tabs", "--rm"]);

        assert_eq!(parsed.tabs, Tabs::Off);
        assert_eq!(parsed.spec_options, words(&["--rm"]));
    }

    #[test]
    fn the_environment_turns_tabs_off_and_the_falsey_spellings_leave_them_on() {
        let off = parse_aid_args(&words(&["owner/repo"]), None, Some("1")).expect("a line");
        assert_eq!(off.tabs, Tabs::Off);

        for value in ["", "0", "false", "no", "  NO  ", "False"] {
            let on = parse_aid_args(&words(&["owner/repo"]), None, Some(value)).expect("a line");
            assert_eq!(on.tabs, Tabs::Default, "{value:?} should leave tabs on");
        }
        // The flag still wins over a standing preference, in both directions.
        let asked =
            parse_aid_args(&words(&["--tabs", "owner/repo"]), None, Some("1")).expect("a line");
        assert_eq!(asked.tabs, Tabs::Asked);
        // …and an environment that says no makes `--rm` an ordinary line again.
        let removed =
            parse_aid_args(&words(&["owner/repo", "go", "--rm"]), None, Some("1")).expect("a line");
        assert_eq!(removed.tabs, Tabs::Off);
    }

    // ------------------------------------------------------- the wrapped command

    #[test]
    fn without_tabs_the_command_is_exactly_what_it_always_was() {
        let plain = tabs_parsed(&["--no-tabs", "owner/repo", "fix", "the", "bug"]);
        let built = build_dl_args(&plain).expect("a dl command line");

        assert_eq!(
            built.last().map(String::as_str),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions 'fix the bug'"
            )
        );
    }

    #[test]
    fn the_wrapped_command_runs_the_same_agent_command_on_both_of_its_paths() {
        let agent = build_agent_command("claude", "fix the bug").expect("an agent command");
        let wrapped = tabs_command(&agent);

        // Once inside the session's script and once as the fallback, byte for byte:
        // what runs with a session has to be what would have run without one.
        assert_eq!(wrapped.matches(agent.as_str()).count(), 2, "{wrapped}");
    }

    #[test]
    fn the_wrapped_command_gives_the_agent_back_when_a_session_is_impossible() {
        let wrapped = tabs_command("AGENT");

        // The three pre-checks, each of which ends with the agent running plainly.
        // They matter more now than they did as a flag: every `aid` line takes this
        // path, so a container with no zellij has to be indistinguishable from
        // before.
        assert!(wrapped.contains("[ -t 1 ]"), "{wrapped}");
        assert!(wrapped.contains("command -v zellij"), "{wrapped}");
        assert!(wrapped.contains("mkdir -p \"$dldir\""), "{wrapped}");
        assert!(wrapped.contains("if [ -z \"$dltabs\" ]"), "{wrapped}");
    }

    #[test]
    fn the_wrapped_command_creates_a_session_once_and_attaches_to_it_after() {
        let wrapped = tabs_command("AGENT");

        // `-n` is the only spelling that creates: `-s <name> --layout <file>` adds
        // tabs to a session that already exists and answers "There is no active
        // session!" when it does not.
        assert!(
            wrapped.contains(&format!("zj -s {TABS_SESSION} -n ")),
            "{wrapped}"
        );
        // …and the existence check is what keeps a re-launch from starting a second
        // agent beside the one still working.
        assert!(
            wrapped.contains(&format!("zj ls -s 2>/dev/null | grep -qx {TABS_SESSION}")),
            "{wrapped}"
        );
        assert!(
            wrapped.contains(&format!("zj attach {TABS_SESSION}")),
            "{wrapped}"
        );
    }

    #[test]
    fn the_session_is_one_tab_that_closes_when_the_agent_does() {
        let wrapped = tabs_command("AGENT");

        // How you leave has to be what it always was: quit the agent, and the last
        // tab closing ends the session and hands the terminal back. A second tab
        // opened for you would leave somebody inside zellij after the agent stopped.
        assert!(wrapped.contains("close_on_exit=true"), "{wrapped}");
        assert_eq!(wrapped.matches("    tab name=").count(), 1, "{wrapped}");
    }

    #[test]
    fn the_agents_exit_status_survives_the_session() {
        let wrapped = tabs_command("AGENT");

        // zellij exits 0 whether the agent quit clean or failing, and
        // `dl <ws> -- cmd` reports the command's own status. The script records it
        // and the shell around zellij exits with it.
        assert!(
            wrapped.contains(&format!("echo $? > \"${{{TABS_STATUS_VAR}:-/dev/null}}\"")),
            "{wrapped}"
        );
        assert!(
            wrapped.contains(&format!("export {TABS_STATUS_VAR}")),
            "{wrapped}"
        );
        // A stale status from the last session must not be read as this one's.
        assert!(
            wrapped.contains(&format!("rm -f \"${TABS_STATUS_VAR}\"")),
            "{wrapped}"
        );
        // …and anything that is not a number is not an exit code.
        assert!(wrapped.contains("*[!0-9]*"), "{wrapped}");
    }

    #[test]
    fn a_prompt_full_of_quoting_survives_into_the_session() {
        // The reason the command travels as a file rather than inside the layout's
        // KDL: this prompt has a single quote, a double quote, a backslash and a `$`,
        // and the only quoting it meets is `shell::quote`'s.
        let prompt = r#"fix o'brien's "$PATH" \ bug"#;
        let agent = build_agent_command("claude", prompt).expect("an agent command");
        let wrapped = tabs_command(&agent);

        assert!(wrapped.contains(&agent), "{wrapped}");
        // The heredoc body opens with the agent command on a line of its own, so
        // nothing in the prompt can be read as the delimiter that ends it.
        assert!(
            wrapped.contains(&format!("\n{agent}\necho $? > ")),
            "{wrapped}"
        );
    }

    #[test]
    fn a_container_with_no_zellij_is_told_so_rather_than_quietly_doing_nothing() {
        let wrapped = tabs_command("AGENT");

        // Guarded on *both* halves: a terminal to read it, and zellij actually
        // missing. A piped run has nobody to tell, and a container that has zellij
        // has nothing to say.
        assert!(
            wrapped.contains("if [ -t 1 ] && ! command -v zellij >/dev/null 2>&1; then echo "),
            "{wrapped}"
        );
        assert!(wrapped.contains(">&2; fi"), "{wrapped}");
        // …and it names the command that fixes it, because the fix is a thing to go
        // and do rather than a risk to be aware of.
        assert!(
            TABS_NO_ZELLIJ_NOTICE.contains("restart"),
            "{TABS_NO_ZELLIJ_NOTICE}"
        );
    }

    #[test]
    fn a_multi_line_prompt_survives_the_heredoc_that_carries_it() {
        // The editor drains a pasted newline into the prompt, so an agent command is
        // not always one line. The quoting spans the newline and the heredoc carries
        // it, which is the whole reason the command is a *body* rather than a KDL
        // string or an argument.
        let agent =
            build_agent_command("claude", "fix this\nand then that").expect("an agent command");
        let wrapped = tabs_command(&agent);

        assert!(wrapped.contains(&agent), "{wrapped}");
        assert!(
            wrapped.contains(&format!("\n{agent}\necho $? > ")),
            "{wrapped}"
        );
    }

    #[test]
    fn a_prompt_that_spells_the_delimiter_gets_no_session_rather_than_a_broken_one() {
        // A line of the prompt equal to the heredoc's delimiter would end the body
        // early and leave the rest standing as shell. Absurd to type on purpose, and
        // answered the way the pre-checks are answered: no session, and the agent
        // runs exactly as it would have.
        let agent = build_agent_command("claude", &format!("fix\n{TABS_AGENT_DELIMITER}\nthis"))
            .expect("an agent command");
        let wrapped = tabs_command(&agent);

        assert_eq!(wrapped, agent);
    }

    #[test]
    fn the_session_is_the_one_dl_already_names() {
        // The other copy of this literal is
        // `devlaunch_core::flows::launch::ZELLIJ_SESSION`, pinned on that side by
        // `the_zellij_wrap_ensures_a_session_beside_the_command`. Two copies, two
        // tests, so moving one reddens the other — see [`TABS_SESSION`].
        assert_eq!(TABS_SESSION, "devlaunch");
    }

    #[test]
    fn every_binding_the_nested_session_takes_is_an_alt_binding() {
        // The whole point of the config: an outer zellij or tmux reads Ctrl and the
        // function keys first, so a Ctrl binding here is a binding that never
        // arrives. `clear-defaults` is what stops zellij supplying its own.
        let config = TABS_CONFIG.join("\n");
        assert!(config.contains("keybinds clear-defaults=true"), "{config}");
        for line in TABS_CONFIG.iter().filter(|line| line.contains("bind ")) {
            assert!(line.contains("bind \"Alt "), "{line} is not an Alt binding");
        }
        // A floating "About Zellij" over the agent is what this one is worth.
        assert!(config.contains("show_startup_tips false"), "{config}");
        // The connection dying has to leave the agent working.
        assert!(config.contains("on_force_close \"detach\""), "{config}");
    }

    // ------------------------------------------------------- the split

    #[test]
    fn a_spec_on_its_own_is_the_whole_command_line() {
        let parsed = parsed(&["owner/repo"]);

        assert_eq!(parsed.spec, "owner/repo");
        assert_eq!(prompt(&parsed), "");
        assert!(parsed.dl_options.is_empty());
        assert_eq!(parsed.agent(), Some(DEFAULT_AGENT));
    }

    #[test]
    fn the_prompt_is_everything_after_the_spec_joined_with_spaces() {
        assert_eq!(
            prompt(&parsed(&["owner/repo", "fix", "the", "bug"])),
            "fix the bug"
        );
        // A prompt the host's shell already unquoted arrives as one word, and stays
        // one word.
        assert_eq!(
            prompt(&parsed(&["owner/repo", "fix the bug"])),
            "fix the bug"
        );
    }

    #[test]
    fn a_flag_after_the_spec_belongs_to_the_prompt() {
        let parsed = parsed(&["owner/repo", "explain", "--verbose", "mode"]);

        assert!(parsed.dl_options.is_empty());
        assert_eq!(prompt(&parsed), "explain --verbose mode");
    }

    #[test]
    fn an_agent_flag_picks_the_agent_and_is_not_passed_on() {
        let parsed = parsed(&["--gemini", "owner/repo", "hi"]);

        assert_eq!(parsed.agent(), Some("gemini"));
        assert_eq!(parsed.spec, "owner/repo");
        assert_eq!(prompt(&parsed), "hi");
        assert!(parsed.dl_options.is_empty());
    }

    #[test]
    fn a_flag_on_the_command_line_beats_the_environments_default() {
        let chosen = parse_aid_args(&words(&["--codex", "owner/repo"]), Some("gemini"), None)
            .expect("a usable command line");

        assert_eq!(chosen.agent(), Some("codex"));
    }

    #[test]
    fn the_environment_sets_the_default_agent() {
        let chosen = parse_aid_args(&words(&["owner/repo"]), Some("gemini"), None)
            .expect("a usable command line");

        assert_eq!(chosen.agent(), Some("gemini"));
        // And an unset or blank variable is no choice at all rather than an agent
        // called "": Python `.strip()`s it and falls back.
        for blank in [None, Some(""), Some("  ")] {
            assert_eq!(
                parse_aid_args(&words(&["owner/repo"]), blank, None)
                    .expect("a usable command line")
                    .agent(),
                Some(DEFAULT_AGENT)
            );
        }
    }

    #[test]
    fn an_agent_the_environment_invented_is_refused() {
        assert_eq!(
            parse_aid_args(&words(&["owner/repo"]), Some("nope"), None),
            Err(UsageError::UnknownAgentInEnvironment {
                name: "nope".to_owned()
            })
        );
    }

    #[test]
    fn a_dl_option_takes_its_value_with_it_and_neither_is_the_spec() {
        let parsed = parsed(&["--devcontainer", "robot", "owner/repo", "hi"]);

        assert_eq!(parsed.dl_options, ["--devcontainer", "robot"]);
        assert_eq!(parsed.spec, "owner/repo");
        assert_eq!(prompt(&parsed), "hi");
    }

    #[test]
    fn an_unknown_leading_flag_goes_to_dl() {
        // aid does not need to know every dl flag to stay out of its way — and a dl
        // that does not know it either is the one that says so.
        let parsed = parsed(&["--shared", "owner/repo"]);

        assert_eq!(parsed.dl_options, ["--shared"]);
        assert_eq!(parsed.spec, "owner/repo");
    }

    #[test]
    fn a_command_line_with_no_workspace_is_refused() {
        for argv in [vec!["--claude"], vec!["--devcontainer", "robot"], vec![]] {
            assert_eq!(
                parse_aid_args(&words(&argv), None, None),
                Err(UsageError::NoWorkspace),
                "{argv:?}"
            );
        }
    }

    // ------------------------------------------------- the retired spellings

    #[test]
    fn a_retired_spelling_starts_no_agent_and_is_handed_to_dl_to_refuse() {
        // The transitional line: `--autorm` recalled from history, now spelled
        // `--rm`. Peeled rather than joined into the prompt, so the person gets dl's
        // sentence naming the new spelling instead of an agent asked to read
        // `--autorm` — and with no `--` tail, so dl refuses before a workspace is
        // booted or an agent runs.
        assert_eq!(
            parsed(&["owner/repo", "fix the bug", "--autorm"]).task,
            Task::Retired
        );
        assert_eq!(
            build_dl_args(&parsed(&["owner/repo", "fix the bug", "--autorm"]))
                .expect("a retired-spelling line needs no agent"),
            ["owner/repo", "--autorm"]
        );
        assert_eq!(
            build_dl_args(&parsed(&[
                "--devcontainer",
                "robot",
                "owner/repo",
                "hi",
                "--stop",
            ]))
            .expect("a retired-spelling line needs no agent"),
            ["--devcontainer", "robot", "owner/repo", "--stop"]
        );
    }

    #[test]
    fn a_prompt_that_merely_mentions_a_suffix_flag_is_still_a_prompt() {
        // The bound the peel is worth having: only the exact word, only at the very
        // end. Everything else stays what aid has always made it.
        assert_eq!(
            prompt(&parsed(&["owner/repo", "fix", "the", "--rm", "flag"])),
            "fix the --rm flag"
        );
        assert_eq!(
            prompt(&parsed(&["owner/repo", "explain --rm"])),
            "explain --rm"
        );
        // `--force` alone names no option, so it is not a suffix — and a `--force`
        // handed to dl as a leading option would be read as the workspace's name.
        assert_eq!(
            prompt(&parsed(&["owner/repo", "do it", "--force"])),
            "do it --force"
        );
    }

    #[test]
    fn a_suffix_flag_with_no_workspace_is_still_no_workspace() {
        for argv in [vec!["--rm"], vec!["--stop", "--force"]] {
            assert_eq!(
                parse_aid_args(&words(&argv), None, None),
                Err(UsageError::NoWorkspace),
                "{argv:?}"
            );
        }
    }

    // --------------------------------------------- the agent's command

    #[test]
    fn claude_is_started_sandboxed_with_and_without_a_prompt() {
        // The flag alone is not enough, and the gap is silent: claude exits 1 with
        // "cannot be used with root/sudo privileges" when it sees
        // --dangerously-skip-permissions under uid 0, and a devcontainer running as
        // root is ordinary.
        assert_eq!(
            build_agent_command("claude", "fix the bug").as_deref(),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions 'fix the bug'"
            )
        );
        assert_eq!(
            build_agent_command("claude", "").as_deref(),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions"
            )
        );
    }

    #[test]
    fn an_agent_that_needs_no_variable_gets_none() {
        for agent in ["codex", "gemini"] {
            let command = build_agent_command(agent, "hi").expect("a known agent");
            assert!(!command.contains("IS_SANDBOX"), "{command}");
        }
    }

    #[test]
    fn gemini_gets_its_interactive_flag_only_beside_a_prompt() {
        assert_eq!(
            build_agent_command("gemini", "hi").as_deref(),
            Some("gemini --prompt-interactive hi")
        );
        assert_eq!(build_agent_command("gemini", "").as_deref(), Some("gemini"));
    }

    #[test]
    fn a_prompt_is_one_argument_however_it_is_spelled() {
        // Python's `shlex.quote` spelling, byte for byte: the payload travels in
        // argv, and a second command cannot be smuggled into it.
        assert_eq!(
            build_agent_command("claude", "don't break \"this\"").as_deref(),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions 'don'\"'\"'t break \"this\"'"
            )
        );
        assert_eq!(
            build_agent_command("claude", "hi; rm -rf /").as_deref(),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions 'hi; rm -rf /'"
            )
        );
    }

    #[test]
    fn an_agent_nothing_knows_has_no_command() {
        assert_eq!(build_agent_command("clippy", "hi"), None);
    }

    // ------------------------------------------- the dl command line

    #[test]
    fn the_dl_command_line_is_options_then_spec_then_the_command() {
        // `--no-tabs` throughout: what these assert is the *shape* of the line and
        // the agent command inside it, and the session wrap is a separate question
        // asked by the tabs tests above.
        assert_eq!(
            build_dl_args(&parsed(&["--no-tabs", "owner/repo@branch", "fix", "it"]))
                .expect("a known agent"),
            [
                "owner/repo@branch",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions 'fix it'",
            ]
        );
        assert_eq!(
            build_dl_args(&parsed(&[
                "--no-tabs",
                "--devcontainer",
                "robot",
                "owner/repo"
            ]))
            .expect("a known agent"),
            [
                "--devcontainer",
                "robot",
                "owner/repo",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions",
            ]
        );
    }

    #[test]
    fn dl_reads_the_command_back_whole() {
        // The prompt survives dl's own parsing of `-- <command>`: dl joins
        // everything after `--` with spaces, so the quoting aid applies has to live
        // inside a single argument rather than be spread across several.
        let args = build_dl_args(&parsed(&[
            "--no-tabs",
            "owner/repo",
            "fix",
            "the",
            "flaky",
            "test",
        ]))
        .expect("a known agent");
        let after = args
            .iter()
            .position(|word| word == "--")
            .expect("the -- separator");

        assert_eq!(
            args[after + 1..].join(" "),
            "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions 'fix the flaky test'"
        );
    }

    #[test]
    fn a_wrapped_command_is_still_exactly_one_argument() {
        // dl joins everything after `--` with spaces, so a command carrying newlines
        // has to arrive as a single argv word or the heredocs in it are cut apart by
        // the join. This is the same promise `dl_reads_the_command_back_whole` makes
        // for the unwrapped command, asked of the one every line now gets.
        let args = build_dl_args(&parsed(&["owner/repo", "fix", "it"])).expect("a known agent");
        let after = args
            .iter()
            .position(|word| word == "--")
            .expect("the -- separator");

        assert_eq!(args[after + 1..].len(), 1, "{args:?}");
        assert!(args[after + 1].contains('\n'), "{args:?}");
        assert_eq!(args[after + 1..].join(" "), args[after + 1]);
    }

    #[test]
    fn every_agent_in_the_table_is_reachable_by_its_own_flag() {
        for name in agent_names() {
            let chosen = parse_aid_args(&words(&[&format!("--{name}"), "owner/repo"]), None, None)
                .expect("a usable command line");

            assert_eq!(chosen.agent(), Some(name));
            assert!(build_agent_command(name, "hi").is_some(), "{name}");
        }
    }

    // ------------------------------------------------- the suffix option

    #[test]
    fn a_trailing_rm_keeps_the_prompt_and_rides_beside_it() {
        // The line somebody actually types — send the agent in, and have the
        // workspace go when it is done — so the prompt has to survive the flag
        // rather than lose to it, which under row 30 it did not.
        let chosen = parsed(&["owner/repo@fix/x", "fix the flaky test", "--rm"]);

        assert_eq!(prompt(&chosen), "fix the flaky test");
        assert_eq!(chosen.spec_options, ["--rm"]);
        assert_eq!(chosen.agent(), Some("claude"));
    }

    #[test]
    fn rm_lands_between_the_spec_and_the_agent_command() {
        // Behind the spec, because that is where dl reads a flag as a modifier
        // rather than as the workspace's name, and ahead of the `--`, because
        // everything after that belongs to the workspace's command.
        let built = build_dl_args(&parsed(&["owner/repo", "fix it", "--rm"]))
            .expect("an agent line builds");

        assert_eq!(
            built,
            [
                "owner/repo",
                "--rm",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions 'fix it'"
            ]
        );
    }

    #[test]
    fn rm_before_the_spec_is_the_same_request() {
        // An unknown leading flag is passed through to dl, which is all `--rm`
        // needs: dl accepts it in any position, unlike `--force`.
        let built =
            build_dl_args(&parsed(&["--rm", "owner/repo", "fix it"])).expect("an agent line");

        assert_eq!(built[0], "--rm");
        assert_eq!(built[1], "owner/repo");
    }

    #[test]
    fn rm_with_no_prompt_is_a_session_that_cleans_up_after_itself() {
        let chosen = parsed(&["owner/repo", "--rm"]);

        assert_eq!(prompt(&chosen), "");
        assert_eq!(chosen.spec_options, ["--rm"]);
    }

    #[test]
    fn a_prompt_that_merely_mentions_rm_is_still_a_prompt() {
        // The bound the peel is worth having: only the exact word, only at the very
        // end, only as a whole argv word.
        assert_eq!(
            prompt(&parsed(&["owner/repo", "explain the --rm flag"])),
            "explain the --rm flag"
        );
        assert_eq!(
            prompt(&parsed(&["owner/repo", "explain", "--rm", "please"])),
            "explain --rm please"
        );
        assert!(
            parsed(&["owner/repo", "explain the --rm flag"])
                .spec_options
                .is_empty()
        );
    }

    #[test]
    fn force_alone_at_the_end_of_a_line_is_still_prompt_text() {
        // A run of nothing but modifiers is not a suffix, which is the rule that
        // keeps `--force` out of a prompt's way.
        assert_eq!(
            prompt(&parsed(&["owner/repo", "use the --force"])),
            "use the --force"
        );
        assert_eq!(
            prompt(&parsed(&["owner/repo", "use the", "--force"])),
            "use the --force"
        );
    }

    #[test]
    fn rm_and_force_are_handed_to_dl_whole_so_dl_can_refuse_the_pair() {
        // dl refuses `--force` beside `--rm` by name. Peeling both is what gets
        // the person that sentence, where leaving `--force` in the prompt would glue
        // it silently onto what the agent reads.
        let chosen = parsed(&["owner/repo", "fix it", "--rm", "--force"]);

        assert_eq!(prompt(&chosen), "fix it");
        assert_eq!(chosen.spec_options, ["--rm", "--force"]);
    }

    #[test]
    fn force_never_lands_in_dls_verb_slot_whichever_order_it_was_typed_in() {
        // The order the flags come out in is dl's positional reading, not the user's
        // typing: dl recovers `--force`'s meaning from where it sits, and aid's spec
        // takes index 0, so a `--force` emitted next would be read as an unknown
        // *verb* — answering `Unknown command '--force'` about a line whose real
        // problem is the pair. An option ahead of it puts it at index 2, where dl
        // reads it as the modifier and refuses the pair by name.
        let typed_backwards = parsed(&["owner/repo", "fix it", "--force", "--rm"]);

        assert_eq!(typed_backwards.spec_options, ["--rm", "--force"]);
        assert_eq!(prompt(&typed_backwards), "fix it");
        let built = build_dl_args(&typed_backwards).expect("an agent line");
        assert_eq!(
            built.iter().position(|word| word == "--force"),
            Some(2),
            "--force reached dl's verb slot: {built:?}"
        );
    }

    #[test]
    fn a_retired_spelling_beside_the_live_one_is_still_dls_to_refuse() {
        // `--rm --autorm` is one request written twice, half of it in a spelling
        // this build has retired — and naming which half is dl's job, not aid's.
        // So the whole run travels and no agent command is built.
        let chosen = parsed(&["owner/repo", "review this pr", "--rm", "--autorm"]);

        assert_eq!(chosen.task, Task::Retired);
        assert_eq!(chosen.spec_options, ["--rm", "--autorm"]);
        assert_eq!(
            build_dl_args(&chosen).expect("a retired-spelling line"),
            ["owner/repo", "--rm", "--autorm"]
        );
    }

    #[test]
    fn the_help_names_rm_and_says_the_agent_still_runs() {
        let help = crate::help();

        assert!(help.contains("--rm"), "{help}");
        assert!(help.contains("the agent still runs"), "{help}");
        // And it names the verb, because "delete it now" no longer has an aid
        // spelling at all and the help is where somebody looks for where it went.
        assert!(help.contains("dl <workspace> rm"), "{help}");
    }

    // ------------------------------------------------ the interactive line

    #[test]
    fn the_boot_line_is_options_then_spec_then_up_and_nothing_else() {
        // The background boot must not carry the suffix options: `--autorm` beside
        // `up` is a pair dl refuses, and the flag's meaning belongs to the attach.
        let chosen = parsed(&[
            "--devcontainer",
            "robot",
            "owner/repo@fix/x",
            "--autorm",
            "--force",
        ]);

        assert_eq!(
            build_boot_args(&chosen),
            ["--devcontainer", "robot", "owner/repo@fix/x", "up"]
        );
        // The peeled pair still rides on the attach line, untouched.
        assert_eq!(chosen.spec_options, ["--autorm", "--force"]);
    }

    #[test]
    fn an_unknown_leading_flag_boots_with_the_same_flag() {
        // Whatever dl makes of it, the boot and the attach must be the same
        // launch, so the flag goes to both or the two could open different
        // workspaces.
        assert_eq!(
            build_boot_args(&parsed(&["--shared", "owner/repo"])),
            ["--shared", "owner/repo", "up"]
        );
    }

    #[test]
    fn a_typed_prompt_lands_where_an_argv_prompt_would_have() {
        // The whole point of `with_prompt`: the editor's text goes through the
        // same quoting and the same prompt flags as a prompt typed on the command
        // line, so the two spellings cannot drift.
        let chosen = parsed(&["owner/repo", "--rm"]).with_prompt("fix the bug".to_owned());

        assert_eq!(prompt(&chosen), "fix the bug");
        assert_eq!(
            build_dl_args(&chosen).expect("an agent line"),
            [
                "owner/repo",
                "--rm",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions 'fix the bug'"
            ]
        );
    }

    #[test]
    fn a_typed_prompt_keeps_each_agents_own_prompt_grammar() {
        let chosen = parse_aid_args(&words(&["--gemini", "--no-tabs", "owner/repo"]), None, None)
            .expect("a usable command line")
            .with_prompt("explain this".to_owned());

        assert_eq!(
            build_dl_args(&chosen).expect("an agent line"),
            [
                "owner/repo",
                "--",
                "gemini --prompt-interactive 'explain this'"
            ]
        );
    }

    #[test]
    fn an_empty_submission_is_the_plain_session_it_always_was() {
        let chosen = parsed(&["--no-tabs", "owner/repo"]).with_prompt(String::new());

        assert_eq!(
            build_dl_args(&chosen).expect("an agent line"),
            [
                "owner/repo",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions"
            ]
        );
    }
}
