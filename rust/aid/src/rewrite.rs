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
/// In the order Python's dict is written, which is the order `--help` lists the
/// flags in after sorting.
const AGENTS: &[(&str, Agent)] = &[
    (
        "claude",
        Agent {
            command: &["claude", "--dangerously-skip-permissions"],
            prompt_flags: &[],
            env: &[("IS_SANDBOX", "1")],
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

/// The dl workspace verbs a line may end with, and win with.
///
/// aid's rule is that everything after the spec is prompt, flags and all, which is
/// what lets a prompt go unquoted. These two are the one exception, and it is
/// bounded to earn it — see [`peel_suffix`]. They exist because appending to a
/// recalled line is the cheap edit a shell offers and rewriting the front of one is
/// not, so "and now delete it" has to be spellable as a suffix or it is not
/// spellable at all.
const SUFFIX_VERBS: &[&str] = &["--rm", "--stop"];

/// The modifier those verbs take, peeled only in their company.
///
/// On its own at the end of a line `--force` is prompt text. It is also the one
/// word that must not reach dl as a *leading* option on its own: dl reads argv
/// positionally to decide whether `--force` is the force flag or the workspace's
/// name, and a `--force` in the workspace slot is a workspace called `--force`.
const SUFFIX_MODIFIERS: &[&str] = &["--force"];

/// dl options a line may end with that ride *with* the prompt instead of beating it.
///
/// The difference from [`SUFFIX_VERBS`] is the whole reason this is a third list and
/// not a fourth entry in that one. `--rm` appended to a recalled line means "forget
/// the prompt, delete it instead", so it displaces the prompt and owes a sentence
/// saying so. `--autorm` means "run the prompt *and then* delete it", which is the
/// commonest thing anybody wants of an agent line — send it in, let it work, get the
/// disk back — so the prompt has to survive the flag.
///
/// Bounded the same three ways to earn the same exception: only at the very end of
/// the line, only these exact words, and only as whole argv words. `aid <ws> explain
/// the --autorm flag` ends on `flag` and is untouched, and a quoted `aid <ws> 'why
/// --autorm'` is one argument that is not `--autorm`.
const SUFFIX_OPTIONS: &[&str] = &["--autorm"];

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
/// A sum rather than a prompt beside a list of flags, because the two are
/// exclusive and each carries what the other has no use for: a `--rm` line starts
/// no agent and so has no prompt to give one, and an agent line has no verb flags.
/// The prompt the verb beat is *kept* on that arm rather than dropped, because a
/// line that deletes a workspace owes the person the sentence naming the words it
/// did not act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Task {
    /// Start an agent, with this prompt. Empty is a session with no prompt rather
    /// than a prompt that is empty.
    Agent { agent: String, prompt: String },
    /// A dl workspace verb, spelled as a flag, which wins over the prompt.
    Verb {
        /// The verb and any modifier it came with, handed to dl as they were
        /// typed. Never empty: no flags is no verb, which is the other arm.
        flags: Vec<String>,
        /// The prompt it overrode, or empty when the line carried none.
        overridden: String,
    },
}

impl Task {
    /// The verb flag this line won with, for the notice that names it.
    ///
    /// `Some` for every [`Task::Verb`] by construction — the arm is only built
    /// from a peel that found one.
    pub(crate) fn verb_flag(&self) -> Option<&str> {
        match self {
            Self::Agent { .. } => None,
            Self::Verb { flags, .. } => flags
                .iter()
                .map(String::as_str)
                .find(|word| SUFFIX_VERBS.contains(word)),
        }
    }
}

impl AidArgs {
    /// The agent this line starts, when it starts one.
    ///
    /// `None` is a verb line, which starts none — the distinction the caller needs
    /// when reporting an agent name the environment invented.
    pub(crate) fn agent(&self) -> Option<&str> {
        match &self.task {
            Task::Agent { agent, .. } => Some(agent),
            Task::Verb { .. } => None,
        }
    }
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

/// What a trailing run of dl flags turned out to be.
///
/// Two lists rather than one, because the halves land in different places and mean
/// different things to the prompt: [`Suffix::verbs`] replaces it, and
/// [`Suffix::options`] runs beside it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Suffix {
    /// A verb and any modifier it came with. Non-empty means the prompt lost.
    verbs: Vec<String>,
    /// dl options that leave the prompt standing.
    options: Vec<String>,
}

/// Split a trailing run of dl flags off the end of a command line.
///
/// The one exception to "everything after the spec is prompt", and bounded three
/// ways to earn it: only at the very *end* of the line, only the exact words in
/// [`SUFFIX_VERBS`], [`SUFFIX_MODIFIERS`] and [`SUFFIX_OPTIONS`], and only when a
/// verb or an option is among them. A prompt whose last word happens to be `--force`
/// is untouched, and so is one that merely mentions `--rm` inside it — `aid <ws> fix
/// the --rm flag` ends on `flag`, and a quoted `aid <ws> 'drop --rm'` is one argument
/// that is not `--rm`.
///
/// A `--force` in the run goes to [`Suffix::verbs`] when a verb is there to modify
/// and to [`Suffix::options`] when there is not. That second case is `aid <ws>
/// <prompt> --autorm --force`, which is a pair dl refuses by name — and handing it
/// to dl whole is what gets the person that sentence instead of a `--force` silently
/// glued onto their prompt.
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
fn peel_suffix(argv: &[String]) -> Option<(&[String], Suffix)> {
    let is_suffix = |word: &str| {
        SUFFIX_VERBS.contains(&word)
            || SUFFIX_MODIFIERS.contains(&word)
            || SUFFIX_OPTIONS.contains(&word)
    };
    let mut at = argv.len();
    while at > 0 && is_suffix(argv[at - 1].as_str()) {
        at -= 1;
    }
    let run = &argv[at..];
    let has_verb = run.iter().any(|word| SUFFIX_VERBS.contains(&word.as_str()));
    let has_option = run
        .iter()
        .any(|word| SUFFIX_OPTIONS.contains(&word.as_str()));
    // A run of nothing but modifiers is prompt text, which is the rule `--force`
    // alone has always been read by.
    if !has_verb && !has_option {
        return None;
    }
    let mut suffix = Suffix::default();
    // Modifiers held back and appended, so an option always precedes one — see the
    // positional argument above. Typed order is kept *within* each group.
    let mut modifiers: Vec<String> = Vec::new();
    for word in run {
        if SUFFIX_OPTIONS.contains(&word.as_str()) {
            suffix.options.push(word.clone());
        } else if has_verb {
            suffix.verbs.push(word.clone());
        } else {
            modifiers.push(word.clone());
        }
    }
    suffix.options.append(&mut modifiers);
    Some((&argv[..at], suffix))
}

/// Split an aid command line into agent, dl options, workspace spec, and the task.
///
/// The first argument that is neither an agent flag nor a dl option is the workspace
/// spec; everything after it is the prompt, flags and all, so a prompt never has to
/// be quoted to protect it from aid's own parsing — except a trailing verb flag,
/// which [`peel_suffix`] takes off first and which wins over the prompt.
///
/// A verb flag can also arrive *before* the spec (`aid --rm owner/repo`), which is
/// the same request written the other way round. It is collected as a verb rather
/// than passed through as an ordinary dl option, because the pass-through would
/// hand dl a verb and an agent command at once and dl refuses that pair.
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
        None => (argv, Suffix::default()),
    };
    let mut leading: Vec<String> = Vec::new();
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
        if DL_VALUE_OPTIONS.contains(&word) {
            // Take the value with it; dl reports a missing one.
            dl_options.extend(line[at..line.len().min(at + 2)].iter().cloned());
            at += 2;
            continue;
        }
        if SUFFIX_VERBS.contains(&word) {
            leading.push(word.to_owned());
            at += 1;
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
    let flags: Vec<String> = leading.into_iter().chain(trailing.verbs).collect();
    Ok(AidArgs {
        spec,
        dl_options,
        spec_options: trailing.options,
        task: if flags.is_empty() {
            Task::Agent { agent, prompt }
        } else {
            Task::Verb {
                flags,
                overridden: prompt,
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

/// The dl command line that does the work.
///
/// `[<dl options>…, <spec>, "--", <agent command>]` — the shape `dl` reads back by
/// joining everything after `--` with spaces, which is why the agent command is one
/// argument and its quoting lives inside it.
///
/// `--autorm` lands between the spec and the `--`, so the agent still gets its prompt
/// and dl still gets the flag: `[…, <spec>, "--autorm", "--", <agent command>]`.
///
/// A verb line is `[<dl options>…, <spec>, <verb flags>…]` and has **no `--` tail**:
/// dl refuses a command beside a verb, and rightly, because the point of the suffix
/// is that no agent is being started. The verb flags go *after* the spec so that a
/// `--force` among them lands where dl reads it as the modifier rather than as the
/// workspace's name.
pub(crate) fn build_dl_args(parsed: &AidArgs) -> Option<Vec<String>> {
    let mut args = parsed.dl_options.clone();
    args.push(parsed.spec.clone());
    // Behind the spec and ahead of the verb flags, which is where dl reads them as
    // modifiers rather than as the workspace's name.
    args.extend(parsed.spec_options.iter().cloned());
    match &parsed.task {
        Task::Agent { agent, prompt } => {
            args.push("--".to_owned());
            args.push(build_agent_command(agent, prompt)?);
        }
        Task::Verb { flags, .. } => args.extend(flags.iter().cloned()),
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

    /// The prompt an agent line carries. Panics on a verb line, which has none —
    /// the tests that mean to look at one use [`overridden`].
    fn prompt(parsed: &AidArgs) -> &str {
        match &parsed.task {
            Task::Agent { prompt, .. } => prompt,
            Task::Verb { .. } => panic!("a verb line has no prompt"),
        }
    }

    /// The verb flags a line ends with, and the prompt they beat.
    fn overridden(argv: &[&str]) -> (Vec<String>, String) {
        match parsed(argv).task {
            Task::Agent { .. } => panic!("an agent line overrides nothing"),
            Task::Verb { flags, overridden } => (flags, overridden),
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

    // --------------------------------------------------- the suffix verb

    #[test]
    fn a_trailing_verb_flag_wins_over_the_prompt_and_keeps_it() {
        // The line this exists for: a recalled `aid` command with `--rm --force`
        // typed at the end. The prompt is kept rather than dropped, because the
        // notice has to be able to name it.
        let (flags, beaten) = overridden(&[
            "kinisi-robotics/kinisi_ros@fix/x",
            "review this pr",
            "--rm",
            "--force",
        ]);

        assert_eq!(flags, ["--rm", "--force"]);
        assert_eq!(beaten, "review this pr");
    }

    #[test]
    fn a_verb_line_asks_dl_for_the_verb_and_starts_no_agent() {
        // No `--` tail at all: dl refuses a command beside a verb, and there is no
        // agent to run one. `--force` lands after the spec, where dl reads it as
        // the modifier rather than as the workspace's name.
        assert_eq!(
            build_dl_args(&parsed(&[
                "owner/repo@fix/x",
                "review this pr",
                "--rm",
                "--force"
            ]))
            .expect("a verb line needs no agent"),
            ["owner/repo@fix/x", "--rm", "--force"]
        );
        assert_eq!(
            build_dl_args(&parsed(&[
                "--devcontainer",
                "robot",
                "owner/repo",
                "hi",
                "--stop"
            ]))
            .expect("a verb line needs no agent"),
            ["--devcontainer", "robot", "owner/repo", "--stop"]
        );
    }

    #[test]
    fn a_verb_flag_before_the_spec_is_the_same_request() {
        // `aid --rm owner/repo` used to hand dl a verb *and* an agent command, a
        // pair dl refuses. It is collected as the verb instead.
        assert_eq!(
            build_dl_args(&parsed(&["--rm", "owner/repo"])).expect("a verb line"),
            ["owner/repo", "--rm"]
        );
        assert_eq!(overridden(&["--rm", "owner/repo"]).1, "");
    }

    #[test]
    fn a_prompt_that_merely_mentions_a_verb_flag_is_still_a_prompt() {
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
        // `--force` alone is not a verb, so it is not a suffix — and a `--force`
        // handed to dl as a leading option would be read as the workspace's name.
        assert_eq!(
            prompt(&parsed(&["owner/repo", "do it", "--force"])),
            "do it --force"
        );
    }

    #[test]
    fn a_verb_flag_with_no_workspace_is_still_no_workspace() {
        for argv in [vec!["--rm"], vec!["--stop", "--force"]] {
            assert_eq!(
                parse_aid_args(&words(&argv), None),
                Err(UsageError::NoWorkspace),
                "{argv:?}"
            );
        }
    }

    #[test]
    fn the_notice_names_the_verb_that_won() {
        let removing = parsed(&["owner/repo", "review this pr", "--rm", "--force"]);

        assert_eq!(removing.task.verb_flag(), Some("--rm"));
        assert_eq!(
            parsed(&["owner/repo", "hi"]).task.verb_flag(),
            None,
            "an agent line won nothing"
        );
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
            Some("IS_SANDBOX=1 claude --dangerously-skip-permissions 'fix the bug'")
        );
        assert_eq!(
            build_agent_command("claude", "").as_deref(),
            Some("IS_SANDBOX=1 claude --dangerously-skip-permissions")
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
            Some("IS_SANDBOX=1 claude --dangerously-skip-permissions 'don'\"'\"'t break \"this\"'")
        );
        assert_eq!(
            build_agent_command("claude", "hi; rm -rf /").as_deref(),
            Some("IS_SANDBOX=1 claude --dangerously-skip-permissions 'hi; rm -rf /'")
        );
    }

    #[test]
    fn an_agent_nothing_knows_has_no_command() {
        assert_eq!(build_agent_command("clippy", "hi"), None);
    }

    // ------------------------------------------- the dl command line

    #[test]
    fn the_dl_command_line_is_options_then_spec_then_the_command() {
        assert_eq!(
            build_dl_args(&parsed(&["owner/repo@branch", "fix", "it"])).expect("a known agent"),
            [
                "owner/repo@branch",
                "--",
                "IS_SANDBOX=1 claude --dangerously-skip-permissions 'fix it'",
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
                "IS_SANDBOX=1 claude --dangerously-skip-permissions",
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
            "IS_SANDBOX=1 claude --dangerously-skip-permissions 'fix the flaky test'"
        );
    }

    #[test]
    fn every_agent_in_the_table_is_reachable_by_its_own_flag() {
        for name in agent_names() {
            let chosen = parse_aid_args(&words(&[&format!("--{name}"), "owner/repo"]), None)
                .expect("a usable command line");

            assert_eq!(chosen.agent(), Some(name));
            assert!(build_agent_command(name, "hi").is_some(), "{name}");
        }
    }

    // ------------------------------------------------- the suffix option

    #[test]
    fn a_trailing_autorm_keeps_the_prompt_and_rides_beside_it() {
        // The difference from `--rm` in one assertion: this is the line somebody
        // actually types — send the agent in, and have the workspace go when it is
        // done — so the prompt has to survive the flag rather than lose to it.
        let chosen = parsed(&["owner/repo@fix/x", "fix the flaky test", "--autorm"]);

        assert_eq!(prompt(&chosen), "fix the flaky test");
        assert_eq!(chosen.spec_options, ["--autorm"]);
        assert_eq!(chosen.agent(), Some("claude"));
    }

    #[test]
    fn autorm_lands_between_the_spec_and_the_agent_command() {
        // Behind the spec, because that is where dl reads a flag as a modifier
        // rather than as the workspace's name, and ahead of the `--`, because
        // everything after that belongs to the workspace's command.
        let built = build_dl_args(&parsed(&["owner/repo", "fix it", "--autorm"]))
            .expect("an agent line builds");

        assert_eq!(
            built,
            [
                "owner/repo",
                "--autorm",
                "--",
                "IS_SANDBOX=1 claude --dangerously-skip-permissions 'fix it'"
            ]
        );
    }

    #[test]
    fn autorm_before_the_spec_is_the_same_request() {
        // An unknown leading flag is passed through to dl, which is all `--autorm`
        // needs: dl accepts it in any position, unlike `--force`.
        let built =
            build_dl_args(&parsed(&["--autorm", "owner/repo", "fix it"])).expect("an agent line");

        assert_eq!(built[0], "--autorm");
        assert_eq!(built[1], "owner/repo");
    }

    #[test]
    fn autorm_with_no_prompt_is_a_session_that_cleans_up_after_itself() {
        let chosen = parsed(&["owner/repo", "--autorm"]);

        assert_eq!(prompt(&chosen), "");
        assert_eq!(chosen.spec_options, ["--autorm"]);
    }

    #[test]
    fn a_prompt_that_merely_mentions_autorm_is_still_a_prompt() {
        // The same bound the verb peel is worth having: only the exact word, only at
        // the very end, only as a whole argv word.
        assert_eq!(
            prompt(&parsed(&["owner/repo", "explain the --autorm flag"])),
            "explain the --autorm flag"
        );
        assert_eq!(
            prompt(&parsed(&["owner/repo", "explain", "--autorm", "please"])),
            "explain --autorm please"
        );
        assert!(
            parsed(&["owner/repo", "explain the --autorm flag"])
                .spec_options
                .is_empty()
        );
    }

    #[test]
    fn force_alone_at_the_end_of_a_line_is_still_prompt_text() {
        // Unchanged by the third list: a run of nothing but modifiers is not a
        // suffix, which is the rule that keeps `--force` out of a prompt's way.
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
    fn autorm_and_force_are_handed_to_dl_whole_so_dl_can_refuse_the_pair() {
        // dl refuses `--force` beside `--autorm` by name. Peeling both is what gets
        // the person that sentence, where leaving `--force` in the prompt would glue
        // it silently onto what the agent reads.
        let chosen = parsed(&["owner/repo", "fix it", "--autorm", "--force"]);

        assert_eq!(prompt(&chosen), "fix it");
        assert_eq!(chosen.spec_options, ["--autorm", "--force"]);
    }

    #[test]
    fn force_never_lands_in_dls_verb_slot_whichever_order_it_was_typed_in() {
        // The order the flags come out in is dl's positional reading, not the user's
        // typing: dl recovers `--force`'s meaning from where it sits, and aid's spec
        // takes index 0, so a `--force` emitted next would be read as an unknown
        // *verb* — answering `Unknown command '--force'` about a line whose real
        // problem is the pair. An option ahead of it puts it at index 2, where dl
        // reads it as the modifier and refuses the pair by name.
        let typed_backwards = parsed(&["owner/repo", "fix it", "--force", "--autorm"]);

        assert_eq!(typed_backwards.spec_options, ["--autorm", "--force"]);
        assert_eq!(prompt(&typed_backwards), "fix it");
        let built = build_dl_args(&typed_backwards).expect("an agent line");
        assert_eq!(
            built.iter().position(|word| word == "--force"),
            Some(2),
            "--force reached dl's verb slot: {built:?}"
        );
    }

    #[test]
    fn a_verb_beside_autorm_keeps_the_verbs_own_reading() {
        // `--rm --autorm` is a contradiction, and it is dl's to name rather than
        // aid's: the verb still beats the prompt, the option still travels, and dl
        // answers "--autorm means nothing for rm".
        let chosen = parsed(&["owner/repo", "review this pr", "--rm", "--autorm"]);

        match &chosen.task {
            Task::Verb { flags, overridden } => {
                assert_eq!(flags, &["--rm"]);
                assert_eq!(overridden, "review this pr");
            }
            Task::Agent { .. } => panic!("the verb should still have won"),
        }
        assert_eq!(chosen.spec_options, ["--autorm"]);
        assert_eq!(
            build_dl_args(&chosen).expect("a verb line"),
            ["owner/repo", "--autorm", "--rm"]
        );
    }

    #[test]
    fn the_help_names_autorm_and_says_it_keeps_the_prompt() {
        let help = crate::help();

        assert!(help.contains("--autorm"), "{help}");
        assert!(help.contains("it keeps"), "{help}");
    }
}
