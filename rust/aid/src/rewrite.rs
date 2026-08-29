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
    /// The flag this agent starts a remotely drivable session with, if it has one.
    ///
    /// `None` is the honest answer for codex and gemini: Remote Control is Claude
    /// Code's feature and there is nothing to map it onto. Read as a capability
    /// rather than hard-coded as `agent == "claude"` so the refusal below and the
    /// command built here read the same table, and adding an agent that grows one
    /// is a row rather than a search.
    ///
    /// It is also what [`RemoteControlRequest::settle`] reads to decide whether a
    /// line asking for Remote Control is a launch or a refusal, so the capability is
    /// stated once and consulted from both ends.
    remote_control: Option<&'static str>,
}

/// The agent this build starts when nothing picks one.
pub(crate) const DEFAULT_AGENT: &str = "claude";

/// The variable that changes the default, for people who do not want to type a flag
/// every time. A `--flag` on the command line still wins.
pub(crate) const AGENT_ENV_VAR: &str = "DEVLAUNCH_AID_AGENT";

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
/// `CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1` is one of the pieces of dl naming the
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
/// dl now writes the same variable into the container's login profile from its
/// title stage, which is what covers the sessions this table cannot reach -- a
/// claude somebody starts for themselves. That rule is unchanged rather than
/// abandoned: an export is the environment a command inherits, not a rewrite of
/// anyone's command. This entry stays because the two do not reach the same
/// sessions. The profile export lands when a workspace enters Running and is read
/// by shells that start after it, so a container that was already up when dl
/// learned this needs a re-login; the prefix here works on the next `aid` either
/// way, and a prefix and an inherited value of the same variable agree.
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
            remote_control: Some("--remote-control"),
        },
    ),
    (
        "codex",
        Agent {
            command: &["codex"],
            prompt_flags: &[],
            env: &[],
            remote_control: None,
        },
    ),
    (
        "gemini",
        Agent {
            command: &["gemini"],
            prompt_flags: &["--prompt-interactive"],
            env: &[],
            remote_control: None,
        },
    ),
];

/// aid's own flag for Claude Code's Remote Control, which is not dl's and must not
/// reach it.
///
/// It has to be intercepted ahead of the "unknown leading flag goes to dl" rule for
/// that reason alone: dl has never heard of it and would exit 2 on contact. The
/// canonical spelling, which is the one the refusal names.
pub(crate) const REMOTE_CONTROL_FLAG: &str = "--remote-control";

/// Every spelling that asks for Remote Control by name.
///
/// `--remote` is here because it is what people type. The long spelling is four
/// words of hyphenated English and the short one is the obvious guess at it, and a
/// guess that falls through to "unknown leading flag" reaches dl, which exits 2
/// about a flag the person half-typed correctly. An alias costs a row.
const REMOTE_CONTROL_FLAGS: &[&str] = &[REMOTE_CONTROL_FLAG, "--remote"];

/// Every spelling that turns Remote Control off, with the same alias.
const NO_REMOTE_CONTROL_FLAGS: &[&str] = &["--no-remote-control", "--no-remote"];

/// The variable that turns the default off, for people who do not want a remotely
/// drivable session at all. A `--flag` on the command line still wins.
pub(crate) const REMOTE_CONTROL_ENV_VAR: &str = "DEVLAUNCH_AID_REMOTE_CONTROL";

/// The values that leave the default where it is, lowercased and trimmed.
const REMOTE_CONTROL_YES: &[&str] = &["1", "true", "on", "yes"];

/// The values that turn the default off.
const REMOTE_CONTROL_NO: &[&str] = &["0", "false", "off", "no"];

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
const SUFFIX_OPTIONS: &[&str] = &["--rm"];

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

/// The names a `--flag` can pick an agent by, sorted — for the help and for the
/// refusal that lists them.
pub(crate) fn agent_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = AGENTS.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    names
}

/// The values [`REMOTE_CONTROL_ENV_VAR`] takes, yes before no — for the refusal that
/// lists them. Read off the same two lists the parse reads, so the sentence cannot
/// offer a value the parse would reject.
pub(crate) fn remote_control_values() -> Vec<&'static str> {
    REMOTE_CONTROL_YES
        .iter()
        .chain(REMOTE_CONTROL_NO)
        .copied()
        .collect()
}

/// The aid command line could not be understood.
///
/// One arm per thing a person can get wrong. Python's `UsageError` carries the
/// sentence; here the sentence is [`crate::render`]'s and this is what happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UsageError {
    /// No workspace anywhere on the command line.
    NoWorkspace,
    /// `DEVLAUNCH_AID_AGENT` names an agent this build has never heard of. Only the
    /// environment can reach this: a `--flag` that is not an agent's is passed
    /// through to dl, as any other unknown flag is.
    UnknownAgentInEnvironment { name: String },
    /// `--remote-control` on a line that starts an agent with no Remote Control.
    ///
    /// Carries the agent so the sentence can name it, because the flag is not
    /// always what chose it: `DEVLAUNCH_AID_AGENT=codex` reaches this with nothing
    /// on the command line saying codex.
    ///
    /// Only a *typed* flag reaches this. The default that Remote Control now is
    /// cannot: [`RemoteControlRequest::Default`] beside an agent without one is
    /// silently off, because a default that refused would refuse every codex and
    /// gemini launch on the machine.
    RemoteControlUnsupported { agent: String },
    /// `DEVLAUNCH_AID_REMOTE_CONTROL` holds something that is neither a yes nor a no.
    ///
    /// Refused rather than read as one or the other, because both readings are a
    /// guess: `DEVLAUNCH_AID_REMOTE_CONTROL=claude` silently meaning *on* is a person
    /// who thinks they turned something off, and silently meaning *off* is a person
    /// who thinks they turned something on.
    UnknownRemoteControlInEnvironment { value: String },
}

/// The variables aid reads, resolved by the caller.
///
/// A struct rather than two `Option<&str>` parameters, because two adjacent optional
/// strings of the same type are two arguments a call site can swap with no compiler
/// anywhere to say so. Reading them is [`crate::main`]'s job, not this module's: the
/// whole of this file is a function from strings to strings, and an environment read
/// inside it is a fact a test cannot vary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Environment<'a> {
    /// `DEVLAUNCH_AID_AGENT`.
    pub(crate) agent: Option<&'a str>,
    /// `DEVLAUNCH_AID_REMOTE_CONTROL`.
    pub(crate) remote_control: Option<&'a str>,
}

/// What the command line asked of Remote Control, before any agent is consulted.
///
/// Three arms rather than a bool, because "nothing was said" and "it was asked for"
/// are the same *outcome* for claude and opposite outcomes for codex: one is silence
/// and the other is exit 1. A bool collapses them, and the collapse is the bug that
/// would turn the new default into a refusal on every non-claude launch.
///
/// Settled against the agent by [`RemoteControlRequest::settle`], which is a total
/// match over both sums rather than an `if`, so the row that must not refuse and the
/// row that must are two rows of the same table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteControlRequest {
    /// Nothing on the command line said either way, and the environment did not turn
    /// the default off. On where the agent has it, silently absent where it has not.
    Default,
    /// `--remote-control` (or `--remote`) was typed. On, or a refusal naming the
    /// agent that has not got it: the person asked for something by name.
    Insisted,
    /// `--no-remote-control` (or `--no-remote`) was typed, or the environment turned
    /// the default off. A plain local session, whichever agent this is.
    Off,
}

/// Whether the session that gets started is remotely drivable, settled.
///
/// Two arms where the request above has three, and that is the shape of the thing
/// rather than a loss: "nothing was said" and "it was asked for" are different
/// *questions* and the same *answer* once an agent is named. All three-ness lives in
/// [`RemoteControlRequest`], and the arm that cannot exist beside codex is kept out
/// by [`RemoteControlRequest::settle`] being a total match over both sums rather
/// than by anything downstream re-checking.
///
/// No session name is carried, because the name is not a second thing to decide: it
/// is always the spec the person typed, which [`AidArgs`] already holds, and two
/// fields that must agree are two fields that can disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteControl {
    /// Started with the agent's Remote Control flag, named after the workspace.
    On,
    /// The ordinary local session.
    Off,
}

impl RemoteControlRequest {
    /// The request, answered by the agent that will run.
    ///
    /// Exhaustive over both sums on purpose. The two rows that look alike are the
    /// ones worth reading twice: `Default` beside an agent with no Remote Control is
    /// **silence**, and `Insisted` beside the same agent is **exit 1**. That is the
    /// whole difference between a default and a request, and it is a row here rather
    /// than a condition somewhere else.
    fn settle(self, agent: &str) -> Result<RemoteControl, UsageError> {
        let capability = AGENTS
            .iter()
            .find(|(name, _)| *name == agent)
            .and_then(|(_, started)| started.remote_control);
        match (self, capability) {
            // Asked to be off, or an agent that has none and nobody said otherwise.
            (Self::Off, _) | (Self::Default, None) => Ok(RemoteControl::Off),
            (Self::Default | Self::Insisted, Some(_)) => Ok(RemoteControl::On),
            (Self::Insisted, None) => Err(UsageError::RemoteControlUnsupported {
                agent: agent.to_owned(),
            }),
        }
    }
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
    Agent {
        agent: String,
        prompt: String,
        /// Whether this session is started with Claude Code's Remote Control on,
        /// already settled against the agent.
        ///
        /// Settled rather than requested, and it lives here rather than on
        /// [`AidArgs`], because those two choices are what keep the illegal states
        /// out. On the task, because a [`Task::Retired`] line starts no agent at
        /// all, and a Remote Control state beside it would describe a session
        /// nobody opens. Settled, because [`RemoteControlRequest::settle`] has
        /// already answered the request against the agent's table row, so a parsed
        /// line cannot describe codex being started with a feature codex has never
        /// had, and no later stage has to ask again.
        remote_control: RemoteControl,
    },
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

/// Whether Remote Control is on by default, when no flag says.
///
/// Unset, empty or a yes leaves the default where the build put it, which is
/// [`RemoteControlRequest::Default`] — on for the agents that have it. A no turns it
/// off for every launch from this shell.
///
/// A yes answers `Default` rather than `Insisted` on purpose: the variable sets a
/// *default*, and a default that refused would make `DEVLAUNCH_AID_REMOTE_CONTROL=1`
/// in a profile break every `aid --codex` on the machine. Only a flag somebody typed
/// on the line in front of them insists.
fn default_remote_control(environment: Option<&str>) -> Result<RemoteControlRequest, UsageError> {
    let value = environment.unwrap_or("").trim().to_ascii_lowercase();
    if value.is_empty() || REMOTE_CONTROL_YES.contains(&value.as_str()) {
        return Ok(RemoteControlRequest::Default);
    }
    if REMOTE_CONTROL_NO.contains(&value.as_str()) {
        return Ok(RemoteControlRequest::Off);
    }
    Err(UsageError::UnknownRemoteControlInEnvironment {
        // The value as it was written, not as it was lowercased: the sentence quotes
        // back what somebody has to go and find in a profile.
        value: environment.unwrap_or("").trim().to_owned(),
    })
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

/// A peeled trailing run, split by whose flag each word is.
///
/// The split is the point. [`Self::options`] rides on to dl, and the remote-control
/// request does not: dl has never heard of `--no-remote-control` and exits 2 on
/// contact, so a run carrying one has to be read here and dropped rather than
/// forwarded whole.
struct Suffix<'a> {
    /// The line with the run taken off, which is what the prompt is built from.
    line: &'a [String],
    /// dl's own, forwarded after the spec.
    options: Vec<String>,
    /// What the run asked of Remote Control, or `None` if it said nothing.
    ///
    /// An `Option` rather than a [`RemoteControlRequest::Default`], because "said
    /// nothing" has to leave an earlier flag standing: `aid --no-remote <ws> fix it
    /// --rm` turned it off before the spec and the `--rm` run must not turn it back
    /// on.
    remote_control: Option<RemoteControlRequest>,
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
fn peel_suffix(argv: &[String]) -> Option<Suffix<'_>> {
    let is_suffix = |word: &str| {
        SUFFIX_OPTIONS.contains(&word)
            || SUFFIX_RETIRED.contains(&word)
            || SUFFIX_MODIFIERS.contains(&word)
            || REMOTE_CONTROL_FLAGS.contains(&word)
            || NO_REMOTE_CONTROL_FLAGS.contains(&word)
    };
    let mut at = argv.len();
    while at > 0 && is_suffix(argv[at - 1].as_str()) {
        at -= 1;
    }
    let run = &argv[at..];
    let names = |list: &[&str]| run.iter().any(|word| list.contains(&word.as_str()));
    // A run of nothing but modifiers is prompt text, which is the rule `--force`
    // alone has always been read by.
    if !names(SUFFIX_OPTIONS)
        && !names(SUFFIX_RETIRED)
        && !names(REMOTE_CONTROL_FLAGS)
        && !names(NO_REMOTE_CONTROL_FLAGS)
    {
        return None;
    }
    // Modifiers held back and appended, so an option always precedes one — see the
    // positional argument above. Typed order is kept *within* each group.
    let mut options: Vec<String> = Vec::new();
    let mut modifiers: Vec<String> = Vec::new();
    let mut remote_control = None;
    for word in run {
        if REMOTE_CONTROL_FLAGS.contains(&word.as_str()) {
            remote_control = Some(RemoteControlRequest::Insisted);
        } else if NO_REMOTE_CONTROL_FLAGS.contains(&word.as_str()) {
            remote_control = Some(RemoteControlRequest::Off);
        } else if SUFFIX_MODIFIERS.contains(&word.as_str()) {
            modifiers.push(word.clone());
        } else {
            options.push(word.clone());
        }
    }
    options.append(&mut modifiers);
    Some(Suffix {
        line: &argv[..at],
        options,
        remote_control,
    })
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
    environment: Environment<'_>,
) -> Result<AidArgs, UsageError> {
    // Resolved before anything else, and kept even for a line that turns out to
    // start no agent: a `DEVLAUNCH_AID_AGENT` naming an agent that does not exist
    // is broken regardless of what this particular line asked for. Same for a
    // `DEVLAUNCH_AID_REMOTE_CONTROL` that is neither a yes nor a no.
    let mut agent = default_agent(environment.agent)?;
    let mut remote_control = default_remote_control(environment.remote_control)?;
    let (line, trailing, trailing_remote_control) = match peel_suffix(argv) {
        Some(suffix) => (suffix.line, suffix.options, suffix.remote_control),
        None => (argv, Vec::new(), None),
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
        // Ahead of the pass-through below, because dl has never heard of these:
        // left to fall through they would reach dl as unknown options and exit 2.
        // After the spec they are prompt text like any other word, which is the rule
        // every flag on this line is read by. Last one typed wins, as it does for
        // the agent flags: a line is read left to right and re-read the same way.
        if REMOTE_CONTROL_FLAGS.contains(&word) {
            remote_control = RemoteControlRequest::Insisted;
            at += 1;
            continue;
        }
        if NO_REMOTE_CONTROL_FLAGS.contains(&word) {
            remote_control = RemoteControlRequest::Off;
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
    // A run at the end of the line was typed after everything before it, so it wins
    // for the same reason the last of two leading flags does. This is the position
    // the off switch is actually typed in: appending to a recalled line is the cheap
    // edit a shell offers, and rewriting the front of one is not.
    if let Some(asked) = trailing_remote_control {
        remote_control = asked;
    }
    // Settled after the workspace, because a line missing both is missing a
    // workspace first: that is the sentence which tells somebody what an aid line
    // looks like. Settled before the task is built, so the `RemoteControl` the task
    // carries is one an agent row supplied the flag for.
    let remote_control = remote_control.settle(&agent)?;
    let prompt = line[at.min(line.len())..].join(" ");
    let retired = names_a_retired_spelling(&trailing);
    Ok(AidArgs {
        spec,
        dl_options,
        spec_options: trailing,
        task: if retired {
            Task::Retired
        } else {
            Task::Agent {
                agent,
                prompt,
                remote_control,
            }
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
///
/// `remote_control` is the session name to start Claude Code's Remote Control under,
/// or `None` for the ordinary local session. A name rather than a bool because
/// claude's `--remote-control [name]` takes an *optional* one, so a bare flag ahead
/// of a prompt would eat the prompt as the session's name; passing the name always,
/// `=`-joined into one argv word, is what makes that impossible. An agent whose table
/// row has no Remote Control flag ignores the name rather than inventing one — a
/// state [`parse_aid_args`] settles before it can be built.
pub(crate) fn build_agent_command(
    agent: &str,
    prompt: &str,
    remote_control: Option<&str>,
) -> Option<String> {
    let (_, started) = AGENTS.iter().find(|(name, _)| *name == agent)?;
    // No prompt to be interactive about: start the agent's plain session, without
    // the flags that only make sense alongside one.
    let mut words: Vec<&str> = started.command.to_vec();
    let named_session = started
        .remote_control
        .zip(remote_control)
        .map(|(flag, name)| format!("{flag}={name}"));
    if let Some(named_session) = &named_session {
        words.push(named_session.as_str());
    }
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
        Task::Agent {
            agent,
            prompt,
            remote_control,
        } => {
            args.push("--".to_owned());
            // The session is named after the spec the person typed, so the list on
            // claude.ai reads as the workspaces they opened.
            let session = match remote_control {
                RemoteControl::On => Some(parsed.spec.as_str()),
                RemoteControl::Off => None,
            };
            args.push(build_agent_command(agent, prompt, session)?);
        }
        Task::Retired => {}
    }
    Some(args)
}

#[cfg(test)]
mod tests {
    //! The Python `test_aid`'s three parsing classes (retired with the Python tree
    //! in #267), which are the whole of aid's own behaviour: what the command line
    //! splits into, what shell command comes out, and what dl is handed.

    use super::*;

    fn words(argv: &[&str]) -> Vec<String> {
        argv.iter().map(|word| (*word).to_owned()).collect()
    }

    fn parsed(argv: &[&str]) -> AidArgs {
        parse_aid_args(&words(argv), Environment::default()).expect("a usable command line")
    }

    /// An environment naming an agent and saying nothing about Remote Control.
    fn agent_env(agent: Option<&str>) -> Environment<'_> {
        Environment {
            agent,
            remote_control: None,
        }
    }

    /// An environment setting the Remote Control default and naming no agent.
    fn remote_env(remote_control: &str) -> Environment<'_> {
        Environment {
            agent: None,
            remote_control: Some(remote_control),
        }
    }

    /// The prompt an agent line carries. Panics on a retired-spelling line, which
    /// carries none.
    fn prompt(parsed: &AidArgs) -> &str {
        match &parsed.task {
            Task::Agent { prompt, .. } => prompt,
            Task::Retired => panic!("a retired-spelling line has no prompt"),
        }
    }

    /// What the line settled Remote Control on. Panics on a retired-spelling line,
    /// which starts no session to drive.
    fn remote_control(parsed: &AidArgs) -> RemoteControl {
        match &parsed.task {
            Task::Agent { remote_control, .. } => *remote_control,
            Task::Retired => panic!("a retired-spelling line starts no agent"),
        }
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
        let chosen = parse_aid_args(
            &words(&["--codex", "owner/repo"]),
            agent_env(Some("gemini")),
        )
        .expect("a usable command line");

        assert_eq!(chosen.agent(), Some("codex"));
    }

    #[test]
    fn the_environment_sets_the_default_agent() {
        let chosen = parse_aid_args(&words(&["owner/repo"]), agent_env(Some("gemini")))
            .expect("a usable command line");

        assert_eq!(chosen.agent(), Some("gemini"));
        // And an unset or blank variable is no choice at all rather than an agent
        // called "": Python `.strip()`s it and falls back.
        for blank in [None, Some(""), Some("  ")] {
            assert_eq!(
                parse_aid_args(&words(&["owner/repo"]), agent_env(blank))
                    .expect("a usable command line")
                    .agent(),
                Some(DEFAULT_AGENT)
            );
        }
    }

    #[test]
    fn an_agent_the_environment_invented_is_refused() {
        assert_eq!(
            parse_aid_args(&words(&["owner/repo"]), agent_env(Some("nope"))),
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
                parse_aid_args(&words(&argv), Environment::default()),
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
                parse_aid_args(&words(&argv), Environment::default()),
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
            build_agent_command("claude", "fix the bug", None).as_deref(),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions 'fix the bug'"
            )
        );
        assert_eq!(
            build_agent_command("claude", "", None).as_deref(),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions"
            )
        );
    }

    #[test]
    fn an_agent_that_needs_no_variable_gets_none() {
        for agent in ["codex", "gemini"] {
            let command = build_agent_command(agent, "hi", None).expect("a known agent");
            assert!(!command.contains("IS_SANDBOX"), "{command}");
        }
    }

    #[test]
    fn gemini_gets_its_interactive_flag_only_beside_a_prompt() {
        assert_eq!(
            build_agent_command("gemini", "hi", None).as_deref(),
            Some("gemini --prompt-interactive hi")
        );
        assert_eq!(
            build_agent_command("gemini", "", None).as_deref(),
            Some("gemini")
        );
    }

    #[test]
    fn a_prompt_is_one_argument_however_it_is_spelled() {
        // Python's `shlex.quote` spelling, byte for byte: the payload travels in
        // argv, and a second command cannot be smuggled into it.
        assert_eq!(
            build_agent_command("claude", "don't break \"this\"", None).as_deref(),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions 'don'\"'\"'t break \"this\"'"
            )
        );
        assert_eq!(
            build_agent_command("claude", "hi; rm -rf /", None).as_deref(),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions 'hi; rm -rf /'"
            )
        );
    }

    #[test]
    fn an_agent_nothing_knows_has_no_command() {
        assert_eq!(build_agent_command("clippy", "hi", None), None);
    }

    // ------------------------------------------- the dl command line

    #[test]
    fn the_dl_command_line_is_options_then_spec_then_the_command() {
        assert_eq!(
            build_dl_args(&parsed(&["owner/repo@branch", "fix", "it"])).expect("a known agent"),
            [
                "owner/repo@branch",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions --remote-control=owner/repo@branch 'fix it'",
            ]
        );
        assert_eq!(
            build_dl_args(&parsed(&["--devcontainer", "robot", "owner/repo"]))
                .expect("a known agent"),
            [
                "--devcontainer",
                "robot",
                "owner/repo",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions --remote-control=owner/repo",
            ]
        );
    }

    #[test]
    fn dl_reads_the_command_back_whole() {
        // The prompt survives dl's own parsing of `-- <command>`: dl joins
        // everything after `--` with spaces, so the quoting aid applies has to live
        // inside a single argument rather than be spread across several.
        let args = build_dl_args(&parsed(&["owner/repo", "fix", "the", "flaky", "test"]))
            .expect("a known agent");
        let after = args
            .iter()
            .position(|word| word == "--")
            .expect("the -- separator");

        assert_eq!(
            args[after + 1..].join(" "),
            "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
             --dangerously-skip-permissions --remote-control=owner/repo \
             'fix the flaky test'"
        );
    }

    #[test]
    fn every_agent_in_the_table_is_reachable_by_its_own_flag() {
        for name in agent_names() {
            let chosen = parse_aid_args(
                &words(&[&format!("--{name}"), "owner/repo"]),
                Environment::default(),
            )
            .expect("a usable command line");

            assert_eq!(chosen.agent(), Some(name));
            assert!(build_agent_command(name, "hi", None).is_some(), "{name}");
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
                 --dangerously-skip-permissions --remote-control=owner/repo 'fix it'"
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
    fn the_off_switch_works_at_the_end_of_the_line_where_it_is_typed() {
        // The position it is actually typed in. Appending to a recalled line is the
        // cheap edit a shell offers, so an off switch that only worked ahead of the
        // spec was an off switch nobody could reach without retyping the line: it
        // started the session anyway *and* handed claude `--no-remote` to read.
        for spelling in ["--no-remote-control", "--no-remote"] {
            let chosen = parsed(&["owner/repo", "fix the bug", spelling]);

            assert_eq!(remote_control(&chosen), RemoteControl::Off, "{spelling}");
            assert_eq!(prompt(&chosen), "fix the bug", "{spelling}");
            // And it is aid's flag, so it must not ride on to dl, which has never
            // heard of it and exits 2 on contact.
            assert!(chosen.spec_options.is_empty(), "{spelling}");
        }
    }

    #[test]
    fn a_trailing_off_switch_beats_a_leading_on_one() {
        // Left to right, the same way two leading flags are read: the run at the end
        // was typed last, so it is the one that counts.
        let chosen = parsed(&["--remote-control", "owner/repo", "fix it", "--no-remote"]);

        assert_eq!(remote_control(&chosen), RemoteControl::Off);
        assert_eq!(prompt(&chosen), "fix it");
    }

    #[test]
    fn a_trailing_rm_leaves_an_earlier_off_switch_where_it_was() {
        // The reason the peel reports "said nothing" rather than "asked for the
        // default": a `--rm` run must not turn back on what a leading `--no-remote`
        // turned off.
        let chosen = parsed(&["--no-remote", "owner/repo", "fix it", "--rm"]);

        assert_eq!(remote_control(&chosen), RemoteControl::Off);
        assert_eq!(chosen.spec_options, ["--rm"]);
        assert_eq!(prompt(&chosen), "fix it");
    }

    #[test]
    fn the_on_switch_also_works_appended() {
        // Both directions, because the whole point of the peel is that appending
        // means what appending looks like it means.
        let chosen = parsed(&["--no-remote", "owner/repo", "fix it", "--remote"]);

        assert_eq!(remote_control(&chosen), RemoteControl::On);
        assert_eq!(prompt(&chosen), "fix it");
        assert!(chosen.spec_options.is_empty());
    }

    #[test]
    fn an_appended_off_switch_rides_beside_rm_without_reaching_dl() {
        // The two suffixes together, which is the line somebody recalls and adds to
        // twice. dl gets its flag and only its flag.
        let chosen = parsed(&["owner/repo", "fix it", "--rm", "--no-remote"]);

        assert_eq!(remote_control(&chosen), RemoteControl::Off);
        assert_eq!(chosen.spec_options, ["--rm"]);
        assert_eq!(prompt(&chosen), "fix it");
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

    // ------------------------------------------------- remote control

    #[test]
    fn a_bare_line_starts_claude_with_remote_control_on() {
        // The default, and the whole of this change: nothing typed, and the session
        // is drivable from claude.ai. Asserted as the exact string because every
        // piece of it is load-bearing — the flag, the `=`, the unquoted command
        // substitution, and where it sits relative to the prompt.
        assert_eq!(
            build_dl_args(&parsed(&[
                "owner/repo@branch",
                "fix",
                "the",
                "flaky",
                "test"
            ]))
            .expect("a known agent"),
            [
                "owner/repo@branch",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions --remote-control=owner/repo@branch \
                 'fix the flaky test'",
            ]
        );
        // And with no prompt, which is the launch this most often is: the flag is
        // not one of the ones that only make sense beside a prompt.
        assert_eq!(
            build_dl_args(&parsed(&["owner/repo@branch"])).expect("a known agent"),
            [
                "owner/repo@branch",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions --remote-control=owner/repo@branch",
            ]
        );
    }

    #[test]
    fn the_default_is_silently_off_for_an_agent_that_has_none() {
        // The regression that would break every non-claude launch on the machine.
        // A default is not a request: codex and gemini have no Remote Control, and
        // the answer to nobody asking for one is silence, not exit 1.
        for agent in ["codex", "gemini"] {
            let chosen = parsed(&[&format!("--{agent}"), "owner/repo", "hi"]);

            assert_eq!(remote_control(&chosen), RemoteControl::Off, "{agent}");
        }
        assert_eq!(
            build_dl_args(&parsed(&["--codex", "owner/repo", "hi"])).expect("a known agent"),
            ["owner/repo", "--", "codex hi"]
        );
        assert_eq!(
            build_dl_args(&parsed(&["--gemini", "owner/repo", "hi"])).expect("a known agent"),
            ["owner/repo", "--", "gemini --prompt-interactive hi"]
        );
        // And the same through the variable, which is how somebody who set it once
        // launches every line.
        let chosen = parse_aid_args(&words(&["owner/repo", "hi"]), agent_env(Some("codex")))
            .expect("a usable command line");
        assert_eq!(remote_control(&chosen), RemoteControl::Off);
    }

    #[test]
    fn remote_control_is_aids_own_flag_and_never_reaches_dl() {
        // dl has never heard of it, so falling through to the "unknown leading flag
        // goes to dl" rule would exit 2 rather than start anything. It is read
        // wherever the agent flags are read, and in either order beside them.
        for argv in [
            vec!["--remote-control", "owner/repo", "fix", "it"],
            vec!["--remote-control", "--claude", "owner/repo", "fix", "it"],
            vec!["--claude", "--remote-control", "owner/repo", "fix", "it"],
            vec!["--no-remote-control", "owner/repo", "fix", "it"],
            vec!["--remote", "owner/repo", "fix", "it"],
            vec!["--no-remote", "owner/repo", "fix", "it"],
        ] {
            let chosen = parsed(&argv);

            assert!(chosen.dl_options.is_empty(), "{argv:?}");
            assert_eq!(chosen.spec, "owner/repo", "{argv:?}");
            assert_eq!(prompt(&chosen), "fix it", "{argv:?}");
        }
    }

    #[test]
    fn the_short_spellings_parse_as_the_long_ones() {
        // `--remote` is what somebody types when they have typed `--remote-control`
        // once and cannot face it again. Falling through to dl, which is what an
        // unrecognised leading flag does, exits 2 on a line that is nearly right.
        for (long, short) in [
            ("--remote-control", "--remote"),
            ("--no-remote-control", "--no-remote"),
        ] {
            assert_eq!(
                parsed(&[long, "owner/repo", "fix", "it"]),
                parsed(&[short, "owner/repo", "fix", "it"]),
                "{short} did not parse as {long}"
            );
        }
        // Including the refusal, which is the alias's other half: a `--remote`
        // beside codex has to say the same thing the long spelling says.
        assert_eq!(
            parse_aid_args(
                &words(&["--codex", "--remote", "owner/repo"]),
                Environment::default()
            ),
            Err(UsageError::RemoteControlUnsupported {
                agent: "codex".to_owned()
            })
        );
    }

    #[test]
    fn no_remote_control_turns_it_off_and_leaves_nothing_behind() {
        // The off switch, in both spellings, asserted on the exact string: a flag
        // that turned it off "mostly" would be a session on claude.ai that nobody
        // meant to publish.
        for flag in ["--no-remote-control", "--no-remote"] {
            let built = build_dl_args(&parsed(&[flag, "owner/repo@fix/x", "fix", "it"]))
                .expect("a known agent");

            assert_eq!(
                built,
                [
                    "owner/repo@fix/x",
                    "--",
                    "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                     --dangerously-skip-permissions 'fix it'",
                ],
                "{flag}"
            );
            assert!(
                !built.iter().any(|word| word.contains("--remote-control")),
                "{flag} left a --remote-control behind: {built:?}"
            );
        }
        // And beside codex or gemini it is not a refusal: turning off what an agent
        // has not got is a no-op, not a mistake worth stopping a launch for.
        for agent in ["--codex", "--gemini"] {
            assert!(
                parse_aid_args(
                    &words(&[agent, "--no-remote-control", "owner/repo"]),
                    Environment::default()
                )
                .is_ok(),
                "{agent}"
            );
        }
    }

    #[test]
    fn the_last_remote_control_flag_typed_is_the_one_that_counts() {
        // Read left to right, like the agent flags, so a recalled line with
        // `--no-remote` appended to it means what appending it looks like it means.
        assert_eq!(
            remote_control(&parsed(&["--remote-control", "--no-remote", "owner/repo"])),
            RemoteControl::Off
        );
        assert_eq!(
            remote_control(&parsed(&["--no-remote", "--remote-control", "owner/repo"])),
            RemoteControl::On
        );
    }

    #[test]
    fn a_prompt_that_merely_mentions_the_switches_is_still_a_prompt() {
        // The bound that survives the peel, and the whole of it: only the exact word,
        // only as a whole argv word, and only in the run at the very *end* of the
        // line. Asking an agent about Remote Control is a thing somebody may well
        // want to do, and everything here is still the prompt it reads.
        for mentioned in [
            // Not at the end: the run stops at `please`, so nothing is peeled.
            vec!["owner/repo", "explain", "--remote-control", "please"],
            vec!["owner/repo", "explain", "--no-remote", "please"],
            // One argv word the host's shell already unquoted, which is not the flag.
            vec!["owner/repo", "explain --remote-control"],
            vec!["owner/repo", "why is --no-remote off"],
        ] {
            let chosen = parsed(&mentioned);

            assert!(
                prompt(&chosen).contains("remote"),
                "{mentioned:?} lost its prompt: {:?}",
                prompt(&chosen)
            );
            assert_eq!(
                remote_control(&chosen),
                RemoteControl::On,
                "{mentioned:?} moved the default"
            );
            assert!(chosen.spec_options.is_empty(), "{mentioned:?}");
        }
    }

    #[test]
    fn remote_control_with_no_workspace_is_still_no_workspace() {
        // The missing workspace is the first thing wrong with the line, and its
        // sentence is the one that says what an aid line looks like.
        for argv in [
            vec!["--remote-control"],
            vec!["--remote-control", "--codex"],
            vec!["--no-remote-control"],
            vec!["--remote"],
            vec!["--no-remote"],
        ] {
            assert_eq!(
                parse_aid_args(&words(&argv), Environment::default()),
                Err(UsageError::NoWorkspace),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn a_session_name_that_needs_quoting_is_still_one_word() {
        // The name travels through the same `shlex.quote` the prompt does, and the
        // whole `--flag=<name>` is what gets quoted — a name broken across two words
        // would leave claude reading the rest of the line as its own arguments.
        assert_eq!(
            build_agent_command("claude", "", Some("./my project")).as_deref(),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions '--remote-control=./my project'"
            )
        );
        assert_eq!(
            build_agent_command("claude", "hi", Some("owner/repo@it's-mine")).as_deref(),
            Some(
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions '--remote-control=owner/repo@it'\"'\"'s-mine' hi"
            )
        );
    }

    #[test]
    fn an_agent_without_remote_control_is_refused_when_the_flag_is_typed() {
        // Silently dropping a *typed* flag would start a session nobody can reach
        // from claude.ai, and passing it on would hand codex an argument it has no
        // meaning for. Refusing names the problem while the line can still be
        // retyped, and before a workspace is booted. The default is the opposite
        // case and stays silent: see the test above.
        for flag in ["--codex", "--gemini"] {
            let refused = parse_aid_args(
                &words(&[flag, "--remote-control", "owner/repo"]),
                Environment::default(),
            );

            assert_eq!(
                refused,
                Err(UsageError::RemoteControlUnsupported {
                    agent: flag.trim_start_matches('-').to_owned()
                }),
                "{flag}"
            );
        }
        // And the agent the *environment* picked, which is the case with nothing on
        // the command line naming an agent at all.
        assert_eq!(
            parse_aid_args(
                &words(&["--remote-control", "owner/repo"]),
                agent_env(Some("codex"))
            ),
            Err(UsageError::RemoteControlUnsupported {
                agent: "codex".to_owned()
            })
        );
        // A `--claude` after it still wins, so the refusal is about the agent the
        // line settles on rather than the order the flags were typed in.
        assert!(
            parse_aid_args(
                &words(&["--remote-control", "--claude", "owner/repo"]),
                agent_env(Some("codex"))
            )
            .is_ok()
        );
    }

    #[test]
    fn the_boot_line_carries_no_remote_control() {
        // `build_boot_args` is dl options and `up`: it starts no agent, so there is
        // no session for a name to belong to. Checked on a default-on line, which is
        // now every line.
        for argv in [
            vec!["owner/repo@fix/x"],
            vec!["--remote-control", "owner/repo@fix/x"],
        ] {
            assert_eq!(
                build_boot_args(&parsed(&argv)),
                ["owner/repo@fix/x", "up"],
                "{argv:?}"
            );
        }
    }

    // ------------------------------------- the remote control variable

    #[test]
    fn the_variable_turns_the_default_off_in_every_spelling_of_no() {
        // Case and surrounding space are the two things a profile line picks up on
        // its way through a shell, so neither may change the answer.
        for value in ["0", "false", "off", "no", "OFF", "  No  "] {
            let chosen = parse_aid_args(&words(&["owner/repo"]), remote_env(value))
                .expect("a usable command line");

            assert_eq!(remote_control(&chosen), RemoteControl::Off, "{value:?}");
        }
    }

    #[test]
    fn the_variable_saying_yes_leaves_the_default_where_it_already_is() {
        // Including empty and unset, which is the overwhelmingly common case.
        for value in [
            None,
            Some(""),
            Some("  "),
            Some("1"),
            Some("true"),
            Some("on"),
            Some("YES"),
        ] {
            let chosen = parse_aid_args(
                &words(&["owner/repo"]),
                Environment {
                    agent: None,
                    remote_control: value,
                },
            )
            .expect("a usable command line");

            assert_eq!(remote_control(&chosen), RemoteControl::On, "{value:?}");
        }
        // A yes is a *default*, not a request: it must not turn every codex launch
        // on the machine into a refusal, which is what reading it as `Insisted`
        // would do.
        let codex = parse_aid_args(
            &words(&["--codex", "owner/repo"]),
            Environment {
                agent: None,
                remote_control: Some("1"),
            },
        )
        .expect("a usable command line");

        assert_eq!(remote_control(&codex), RemoteControl::Off);
    }

    #[test]
    fn a_variable_that_is_neither_a_yes_nor_a_no_is_refused() {
        // Both readings of `DEVLAUNCH_AID_REMOTE_CONTROL=claude` are a guess, and
        // both leave somebody sure they set something they did not.
        for value in ["claude", "2", "yes please", "-"] {
            assert_eq!(
                parse_aid_args(&words(&["owner/repo"]), remote_env(value)),
                Err(UsageError::UnknownRemoteControlInEnvironment {
                    value: value.to_owned()
                }),
                "{value:?}"
            );
        }
    }

    #[test]
    fn a_flag_on_the_command_line_beats_the_variable_in_both_directions() {
        // The rule `DEVLAUNCH_AID_AGENT` is already read by: the variable is a
        // default, and the line in front of you is not.
        let insisted = parse_aid_args(&words(&["--remote-control", "owner/repo"]), remote_env("0"))
            .expect("a usable command line");

        assert_eq!(remote_control(&insisted), RemoteControl::On);

        let refused_it = parse_aid_args(&words(&["--no-remote", "owner/repo"]), remote_env("1"))
            .expect("a usable command line");

        assert_eq!(remote_control(&refused_it), RemoteControl::Off);
    }

    #[test]
    fn a_broken_variable_is_refused_even_on_a_line_that_starts_no_agent() {
        // Same rule as the agent variable: a variable that cannot be read is broken
        // regardless of what this particular line asked for, and the retired
        // spelling would otherwise hide it behind dl's refusal.
        assert_eq!(
            parse_aid_args(
                &words(&["owner/repo", "hi", "--autorm"]),
                remote_env("maybe")
            ),
            Err(UsageError::UnknownRemoteControlInEnvironment {
                value: "maybe".to_owned()
            })
        );
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
                 --dangerously-skip-permissions --remote-control=owner/repo 'fix the bug'"
            ]
        );
    }

    #[test]
    fn a_typed_prompt_keeps_each_agents_own_prompt_grammar() {
        let chosen = parse_aid_args(&words(&["--gemini", "owner/repo"]), Environment::default())
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
    fn a_typed_prompt_keeps_whatever_the_line_settled_remote_control_on() {
        // The interactive path is the promptless launch, which is where most
        // `aid <ws>` lines go, so both states have to survive the editor: a session
        // that came back local would be one nobody can pick up elsewhere, and one
        // that came back drivable would be one somebody turned off.
        let on = parsed(&["owner/repo@fix/x"]).with_prompt("fix the bug".to_owned());

        assert_eq!(remote_control(&on), RemoteControl::On);
        assert_eq!(
            build_dl_args(&on).expect("an agent line"),
            [
                "owner/repo@fix/x",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions --remote-control=owner/repo@fix/x 'fix the bug'",
            ]
        );

        let off =
            parsed(&["--no-remote-control", "owner/repo@fix/x"]).with_prompt("fix it".to_owned());

        assert_eq!(remote_control(&off), RemoteControl::Off);
        assert_eq!(
            build_dl_args(&off).expect("an agent line"),
            [
                "owner/repo@fix/x",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions 'fix it'",
            ]
        );
    }

    #[test]
    fn an_empty_submission_is_the_plain_session_it_always_was() {
        let chosen = parsed(&["owner/repo"]).with_prompt(String::new());

        assert_eq!(
            build_dl_args(&chosen).expect("an agent line"),
            [
                "owner/repo",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions --remote-control=owner/repo"
            ]
        );
    }
}
