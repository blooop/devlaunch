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
/// that reason alone: dl has never heard of it and would exit 2 on contact.
pub(crate) const REMOTE_CONTROL_FLAG: &str = "--remote-control";

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
    RemoteControlUnsupported { agent: String },
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
        /// Whether the session is started with Claude Code's Remote Control on.
        ///
        /// A bool, and it lives here rather than on [`AidArgs`], because those two
        /// choices are what keep the illegal states out. On the task, because a
        /// [`Task::Retired`] line starts no agent at all, and a remote-control bool
        /// beside it would be a flag on a session nobody opens. A bool rather than
        /// a session name, because the name is not a second thing to decide: it is
        /// always the spec the person typed, which [`AidArgs`] already holds, and
        /// two fields that must agree are two fields that can disagree.
        ///
        /// `true` implies the agent has Remote Control: [`parse_aid_args`] refuses
        /// the pairing outright ([`UsageError::RemoteControlUnsupported`]), so no
        /// parsed line reaches [`build_agent_command`] asking codex for a feature
        /// it has never had.
        remote_control: bool,
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
fn peel_suffix(argv: &[String]) -> Option<(&[String], Vec<String>)> {
    let is_suffix = |word: &str| {
        SUFFIX_OPTIONS.contains(&word)
            || SUFFIX_RETIRED.contains(&word)
            || SUFFIX_MODIFIERS.contains(&word)
    };
    let mut at = argv.len();
    while at > 0 && is_suffix(argv[at - 1].as_str()) {
        at -= 1;
    }
    let run = &argv[at..];
    let names = |list: &[&str]| run.iter().any(|word| list.contains(&word.as_str()));
    // A run of nothing but modifiers is prompt text, which is the rule `--force`
    // alone has always been read by.
    if !names(SUFFIX_OPTIONS) && !names(SUFFIX_RETIRED) {
        return None;
    }
    // Modifiers held back and appended, so an option always precedes one — see the
    // positional argument above. Typed order is kept *within* each group.
    let mut options: Vec<String> = Vec::new();
    let mut modifiers: Vec<String> = Vec::new();
    for word in run {
        if SUFFIX_MODIFIERS.contains(&word.as_str()) {
            modifiers.push(word.clone());
        } else {
            options.push(word.clone());
        }
    }
    options.append(&mut modifiers);
    Some((&argv[..at], options))
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
) -> Result<AidArgs, UsageError> {
    // Resolved before anything else, and kept even for a line that turns out to
    // start no agent: a `DEVLAUNCH_AID_AGENT` naming an agent that does not exist
    // is broken regardless of what this particular line asked for.
    let mut agent = default_agent(environment)?;
    let (line, trailing) = match peel_suffix(argv) {
        Some((line, trailing)) => (line, trailing),
        None => (argv, Vec::new()),
    };
    let mut dl_options: Vec<String> = Vec::new();
    let mut spec: Option<String> = None;
    let mut remote_control = false;
    let mut at = 0;
    while at < line.len() {
        let word = line[at].as_str();
        if let Some(named) = agent_flag(word) {
            agent = named.to_owned();
            at += 1;
            continue;
        }
        // Ahead of the pass-through below, because dl has never heard of this one:
        // left to fall through it would reach dl as an unknown option and exit 2.
        // After the spec it is prompt text like any other word, which is the rule
        // every flag on this line is read by.
        if word == REMOTE_CONTROL_FLAG {
            remote_control = true;
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
    // Checked after the workspace, because a line missing both is missing a
    // workspace first: that is the sentence which tells somebody what an aid line
    // looks like. Checked before the task is built, so a `true` in
    // `Task::Agent::remote_control` can only ever sit beside an agent that has it.
    if remote_control && !starts_remote_control(&agent) {
        return Err(UsageError::RemoteControlUnsupported { agent });
    }
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

/// Whether this agent can be started with Remote Control on.
///
/// Read off the table's `remote_control` entry rather than compared against
/// `"claude"`, so the one place that decides is the row. An agent no table entry
/// knows cannot start it either, which is the answer a name the environment
/// invented deserves.
fn starts_remote_control(agent: &str) -> bool {
    AGENTS
        .iter()
        .any(|(name, started)| *name == agent && started.remote_control.is_some())
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
/// state [`parse_aid_args`] refuses before it can be built.
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
            let session = remote_control.then(|| parsed.spec.as_str());
            args.push(build_agent_command(agent, prompt, session)?);
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
        parse_aid_args(&words(argv), None).expect("a usable command line")
    }

    /// The prompt an agent line carries. Panics on a retired-spelling line, which
    /// carries none.
    fn prompt(parsed: &AidArgs) -> &str {
        match &parsed.task {
            Task::Agent { prompt, .. } => prompt,
            Task::Retired => panic!("a retired-spelling line has no prompt"),
        }
    }

    /// Whether the line asks for Remote Control. Panics on a retired-spelling line,
    /// which starts no session to drive.
    fn remote_control(parsed: &AidArgs) -> bool {
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
        let chosen = parse_aid_args(&words(&["--codex", "owner/repo"]), Some("gemini"))
            .expect("a usable command line");

        assert_eq!(chosen.agent(), Some("codex"));
    }

    #[test]
    fn the_environment_sets_the_default_agent() {
        let chosen =
            parse_aid_args(&words(&["owner/repo"]), Some("gemini")).expect("a usable command line");

        assert_eq!(chosen.agent(), Some("gemini"));
        // And an unset or blank variable is no choice at all rather than an agent
        // called "": Python `.strip()`s it and falls back.
        for blank in [None, Some(""), Some("  ")] {
            assert_eq!(
                parse_aid_args(&words(&["owner/repo"]), blank)
                    .expect("a usable command line")
                    .agent(),
                Some(DEFAULT_AGENT)
            );
        }
    }

    #[test]
    fn an_agent_the_environment_invented_is_refused() {
        assert_eq!(
            parse_aid_args(&words(&["owner/repo"]), Some("nope")),
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
                parse_aid_args(&words(&argv), None),
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
                parse_aid_args(&words(&argv), None),
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
                 --dangerously-skip-permissions 'fix it'",
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
                 --dangerously-skip-permissions",
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
             --dangerously-skip-permissions 'fix the flaky test'"
        );
    }

    #[test]
    fn every_agent_in_the_table_is_reachable_by_its_own_flag() {
        for name in agent_names() {
            let chosen = parse_aid_args(&words(&[&format!("--{name}"), "owner/repo"]), None)
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

    // ------------------------------------------------- remote control

    #[test]
    fn remote_control_is_aids_own_flag_and_never_reaches_dl() {
        // dl has never heard of it, so falling through to the "unknown leading flag
        // goes to dl" rule would exit 2 rather than start anything. It is read
        // wherever the agent flags are read, and in either order beside them.
        for argv in [
            vec!["--remote-control", "owner/repo", "fix", "it"],
            vec!["--remote-control", "--claude", "owner/repo", "fix", "it"],
            vec!["--claude", "--remote-control", "owner/repo", "fix", "it"],
        ] {
            let chosen = parsed(&argv);

            assert!(remote_control(&chosen), "{argv:?}");
            assert!(chosen.dl_options.is_empty(), "{argv:?}");
            assert_eq!(chosen.spec, "owner/repo", "{argv:?}");
            assert_eq!(prompt(&chosen), "fix it", "{argv:?}");
        }
    }

    #[test]
    fn remote_control_after_the_spec_is_prompt_text() {
        // Everything after the spec is prompt, flags and all — the rule that lets a
        // prompt go unquoted. This flag buys no exception to it: only `--rm` and the
        // retired spellings are peeled off the end, and asking an agent about Remote
        // Control is a thing somebody may well want to do.
        let chosen = parsed(&["owner/repo", "explain", "--remote-control"]);

        assert_eq!(prompt(&chosen), "explain --remote-control");
        assert!(!remote_control(&chosen));
        assert!(chosen.spec_options.is_empty());
    }

    #[test]
    fn remote_control_with_no_workspace_is_still_no_workspace() {
        // The missing workspace is the first thing wrong with the line, and its
        // sentence is the one that says what an aid line looks like.
        for argv in [
            vec!["--remote-control"],
            vec!["--remote-control", "--codex"],
        ] {
            assert_eq!(
                parse_aid_args(&words(&argv), None),
                Err(UsageError::NoWorkspace),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn remote_control_names_the_session_after_the_workspace() {
        // `--remote-control [name]` takes an *optional* name, so a bare flag ahead of
        // a prompt would swallow the prompt as the session's name. The name is
        // therefore always emitted, and `=`-joined so no following word can be read
        // as it.
        assert_eq!(
            build_dl_args(&parsed(&[
                "--remote-control",
                "owner/repo@fix/x",
                "fix",
                "it"
            ]))
            .expect("a known agent"),
            [
                "owner/repo@fix/x",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions --remote-control=owner/repo@fix/x 'fix it'",
            ]
        );
        // And with no prompt: the flag is not one of the ones that only make sense
        // beside one, so it is there either way.
        assert_eq!(
            build_dl_args(&parsed(&["--remote-control", "owner/repo@fix/x"]))
                .expect("a known agent"),
            [
                "owner/repo@fix/x",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions --remote-control=owner/repo@fix/x",
            ]
        );
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
    fn an_agent_without_remote_control_is_refused_rather_than_started_without_it() {
        // Silently dropping the flag would start a session nobody can reach from
        // claude.ai, and passing it on would hand codex an argument it has no
        // meaning for. Refusing names the problem while the line can still be
        // retyped, and before a workspace is booted.
        for flag in ["--codex", "--gemini"] {
            let refused = parse_aid_args(&words(&[flag, "--remote-control", "owner/repo"]), None);

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
            parse_aid_args(&words(&["--remote-control", "owner/repo"]), Some("codex")),
            Err(UsageError::RemoteControlUnsupported {
                agent: "codex".to_owned()
            })
        );
        // A `--claude` after it still wins, so the refusal is about the agent the
        // line settles on rather than the order the flags were typed in.
        assert!(
            parse_aid_args(
                &words(&["--remote-control", "--claude", "owner/repo"]),
                Some("codex")
            )
            .is_ok()
        );
    }

    #[test]
    fn the_boot_line_carries_no_remote_control() {
        // `build_boot_args` is dl options and `up`: it starts no agent, so there is
        // no session for a name to belong to.
        let chosen = parsed(&["--remote-control", "owner/repo@fix/x"]);

        assert_eq!(build_boot_args(&chosen), ["owner/repo@fix/x", "up"]);
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
        let chosen = parse_aid_args(&words(&["--gemini", "owner/repo"]), None)
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
    fn a_typed_prompt_lands_beside_remote_control_rather_than_instead_of_it() {
        // The interactive path is where a `--remote-control` line most often goes:
        // it is the promptless launch. The flag has to survive the editor, or the
        // session that comes back is a local one nobody can pick up elsewhere.
        let chosen =
            parsed(&["--remote-control", "owner/repo@fix/x"]).with_prompt("fix the bug".to_owned());

        assert!(remote_control(&chosen));
        assert_eq!(
            build_dl_args(&chosen).expect("an agent line"),
            [
                "owner/repo@fix/x",
                "--",
                "CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude \
                 --dangerously-skip-permissions --remote-control=owner/repo@fix/x 'fix the bug'",
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
                 --dangerously-skip-permissions"
            ]
        );
    }
}
