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
    pub(crate) agent: String,
    /// dl options seen before the spec (`--devcontainer x`), passed through as-is.
    pub(crate) dl_options: Vec<String>,
    /// Everything after the spec, joined with single spaces. Empty when there was
    /// none, which is a session with no prompt rather than a prompt that is empty.
    pub(crate) prompt: String,
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

/// Split an aid command line into agent, dl options, workspace spec and prompt.
///
/// The first argument that is neither an agent flag nor a dl option is the workspace
/// spec; everything after it is the prompt, flags and all, so a prompt never has to
/// be quoted to protect it from aid's own parsing.
pub(crate) fn parse_aid_args(
    argv: &[String],
    environment: Option<&str>,
) -> Result<AidArgs, UsageError> {
    let mut agent = default_agent(environment)?;
    let mut dl_options: Vec<String> = Vec::new();
    let mut spec: Option<String> = None;
    let mut at = 0;
    while at < argv.len() {
        let word = argv[at].as_str();
        if let Some(named) = agent_flag(word) {
            agent = named.to_owned();
            at += 1;
            continue;
        }
        if DL_VALUE_OPTIONS.contains(&word) {
            // Take the value with it; dl reports a missing one.
            dl_options.extend(argv[at..argv.len().min(at + 2)].iter().cloned());
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
    Ok(AidArgs {
        spec,
        agent,
        dl_options,
        prompt: argv[at.min(argv.len())..].join(" "),
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
pub(crate) fn build_dl_args(parsed: &AidArgs) -> Option<Vec<String>> {
    let command = build_agent_command(&parsed.agent, &parsed.prompt)?;
    let mut args = parsed.dl_options.clone();
    args.push(parsed.spec.clone());
    args.push("--".to_owned());
    args.push(command);
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

    // ------------------------------------------------------- the split

    #[test]
    fn a_spec_on_its_own_is_the_whole_command_line() {
        let parsed = parsed(&["owner/repo"]);

        assert_eq!(parsed.spec, "owner/repo");
        assert_eq!(parsed.prompt, "");
        assert!(parsed.dl_options.is_empty());
        assert_eq!(parsed.agent, DEFAULT_AGENT);
    }

    #[test]
    fn the_prompt_is_everything_after_the_spec_joined_with_spaces() {
        assert_eq!(
            parsed(&["owner/repo", "fix", "the", "bug"]).prompt,
            "fix the bug"
        );
        // A prompt the host's shell already unquoted arrives as one word, and stays
        // one word.
        assert_eq!(parsed(&["owner/repo", "fix the bug"]).prompt, "fix the bug");
    }

    #[test]
    fn a_flag_after_the_spec_belongs_to_the_prompt() {
        let parsed = parsed(&["owner/repo", "explain", "--verbose", "mode"]);

        assert!(parsed.dl_options.is_empty());
        assert_eq!(parsed.prompt, "explain --verbose mode");
    }

    #[test]
    fn an_agent_flag_picks_the_agent_and_is_not_passed_on() {
        let parsed = parsed(&["--gemini", "owner/repo", "hi"]);

        assert_eq!(parsed.agent, "gemini");
        assert_eq!(parsed.spec, "owner/repo");
        assert_eq!(parsed.prompt, "hi");
        assert!(parsed.dl_options.is_empty());
    }

    #[test]
    fn a_flag_on_the_command_line_beats_the_environments_default() {
        let chosen = parse_aid_args(&words(&["--codex", "owner/repo"]), Some("gemini"))
            .expect("a usable command line");

        assert_eq!(chosen.agent, "codex");
    }

    #[test]
    fn the_environment_sets_the_default_agent() {
        let chosen =
            parse_aid_args(&words(&["owner/repo"]), Some("gemini")).expect("a usable command line");

        assert_eq!(chosen.agent, "gemini");
        // And an unset or blank variable is no choice at all rather than an agent
        // called "": Python `.strip()`s it and falls back.
        for blank in [None, Some(""), Some("  ")] {
            assert_eq!(
                parse_aid_args(&words(&["owner/repo"]), blank)
                    .expect("a usable command line")
                    .agent,
                DEFAULT_AGENT
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
        assert_eq!(parsed.prompt, "hi");
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

            assert_eq!(chosen.agent, name);
            assert!(build_agent_command(name, "hi").is_some(), "{name}");
        }
    }
}
