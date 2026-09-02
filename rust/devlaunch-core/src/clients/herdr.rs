//! What a session manager needs in order to see an agent dl started.
//!
//! A manager like [herdr](https://herdr.dev) classifies a pane in two steps: it
//! decides *which* agent the pane holds, and only then matches that agent's rules
//! against the pane's screen. Under dl the first step fails and takes the second
//! with it — the pane's foreground processes are `dl`, `ssh` and two `devpod`s,
//! with no agent among them — while the second step was never broken, because
//! `dl <ws> -- <agent>` pipes the agent's own TUI through the pane.
//!
//! So the whole of what dl owes a manager is the agent's *name*, and the place a
//! manager can read it is the environment of a host process it can see. That is
//! the ssh child dl spawns, not dl itself: a `setenv` after start does not rewrite
//! `/proc/<pid>/environ`, which the kernel fixes at exec, so dl's own row carries
//! nothing however early it is written. herdr walks the pane's whole foreground
//! process list, which is what makes the child enough — measured against herdr
//! 0.8.2 on a pane whose parent's environ was deliberately empty of it.
//!
//! `aid` already does this for the session it opens (`aid/src/main.rs`), because
//! aid *picked* the agent. dl is the other half: it did not pick the agent, but
//! when the command it was handed **is** an agent by name, saying so is a reading
//! of the command rather than a guess about it.

use super::gh::Forwarding;
use crate::runner::EnvSpec;

/// The variable a session manager reads to learn which agent a wrapper stands for.
///
/// herdr's name for it, and the one manager-specific word in core. It is written
/// for the ssh child and never forwarded into the container: herdr's own docs say
/// the hint "applies only to that foreground process" and that it "cannot see it
/// if you set it only inside a VM or container", so a name sent through the
/// transport would be a name nothing reads.
pub(crate) const AGENT_VAR: &str = "HERDR_AGENT";

/// Every agent devlaunch knows by name, in the spelling a session manager uses.
///
/// `aid`'s own table (`aid/src/rewrite.rs`) knows the same agents and much more
/// about each of them — the command, the prompt flags, the environment — so the
/// names exist twice by necessity: this const is `pub(crate)` and aid is a
/// different binary. `test/unit/test_session_manager.py` diffs the two lists, per
/// the standing rule in CLAUDE.md that a second hand-maintained copy of a fact
/// needs a test beside it doing exactly that.
///
/// A name here is a claim that a manager has detection rules under that label. It
/// is the reason this is a list and not "whatever program the command names":
/// `dl <ws> -- make test` naming an agent called `make` would tell a manager the
/// pane holds an agent it has never heard of, which is a worse answer than the
/// silence dl gives today.
pub(crate) const AGENT_NAMES: &[&str] = &["claude", "codex", "gemini"];

/// The agent a workspace command starts, when its program is one by name.
///
/// Reads the command the way a shell would begin to: leading `NAME=value`
/// assignments are the shell's, not the program's, so they are stepped over, and
/// the program itself is compared by its last path component so that
/// `/usr/local/bin/claude` and `claude` answer alike.
///
/// Everything past the program is ignored, prompt and flags included, because
/// nothing after the program can change which agent is about to run.
///
/// Deliberately not a parse. A command holding a pipe, a `&&` or a subshell gets
/// the answer for its *first* program, which is the one whose screen the pane will
/// hold at the moment it starts; a command that starts an agent halfway through a
/// chain is not named, and silence is the same answer dl gave before this existed.
pub(crate) fn agent_in(command: &str) -> Option<&'static str> {
    let program = command
        .split_whitespace()
        .find(|word| !is_assignment(word))?;
    let name = program.rsplit('/').next()?;
    AGENT_NAMES.iter().copied().find(|known| *known == name)
}

/// Whether a word is a shell assignment prefix rather than the program.
///
/// `FOO=bar claude` runs claude. The name half must be non-empty for the same
/// reason the shell requires it: `=x` is a program named `=x`, however unlikely,
/// and not an assignment.
fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => !name.is_empty(),
        None => false,
    }
}

/// Add the agent's name to what the ssh child will be run with.
///
/// The name goes in the child's environment and **not** in the `SendEnv` permit
/// list, which is why this extends only the environment half of a [`Forwarding`]
/// and leaves `args` alone. A permit list entry would ask the container for the
/// value, where nothing reads it, and would change the multiplexed control
/// socket's identity for a variable that never crosses the transport.
pub(crate) fn extend_openssh_forwarding(base: Forwarding, agent: Option<&str>) -> Forwarding {
    let Some(agent) = agent else {
        return base;
    };
    let Forwarding { args, env } = base;
    Forwarding {
        args,
        env: inherited(env).and(AGENT_VAR, agent),
    }
}

/// A base environment that is a parent environment.
///
/// `Forwarding::default()` carries `EnvSpec::default()`, which is already the
/// inherited one; this says so out loud for [`super::claude::extend_openssh_forwarding`]'s
/// reason — a change to `EnvSpec`'s default must not silently hand a session an
/// empty environment.
fn inherited(env: EnvSpec) -> EnvSpec {
    if env == EnvSpec::default() {
        EnvSpec::inherited()
    } else {
        env
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_command_names_its_agent() {
        assert_eq!(agent_in("claude"), Some("claude"));
        assert_eq!(agent_in("codex"), Some("codex"));
        assert_eq!(agent_in("gemini"), Some("gemini"));
    }

    /// The prompt is the common shape and must not hide the program.
    #[test]
    fn arguments_after_the_program_are_not_read() {
        assert_eq!(agent_in("claude 'fix the bug'"), Some("claude"));
        assert_eq!(
            agent_in("claude --dangerously-skip-permissions"),
            Some("claude")
        );
    }

    /// What `aid` composes reaches dl as a path in some layouts and a bare name in
    /// others, and the two are one agent.
    #[test]
    fn a_path_names_the_agent_its_last_component_does() {
        assert_eq!(agent_in("/usr/local/bin/claude"), Some("claude"));
        assert_eq!(agent_in("~/.local/bin/codex"), Some("codex"));
    }

    /// The shape `aid` uses for the two variables its table sets.
    #[test]
    fn assignments_are_stepped_over() {
        assert_eq!(agent_in("IS_SANDBOX=1 claude"), Some("claude"));
        assert_eq!(
            agent_in("CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1 IS_SANDBOX=1 claude"),
            Some("claude")
        );
    }

    /// A program named `=x` is a program. Only a non-empty name is an assignment.
    #[test]
    fn a_leading_equals_is_not_an_assignment() {
        assert_eq!(agent_in("=x claude"), None);
    }

    #[test]
    fn an_ordinary_command_names_nothing() {
        assert_eq!(agent_in("make test"), None);
        assert_eq!(agent_in("pixi run ci"), None);
        assert_eq!(agent_in(""), None);
        assert_eq!(agent_in("   "), None);
    }

    /// A name that merely contains an agent's is not that agent: a manager would
    /// be told to match claude's rules against something else's screen.
    #[test]
    fn a_longer_name_is_a_different_program() {
        assert_eq!(agent_in("claude-monitor"), None);
        assert_eq!(agent_in("myclaude"), None);
    }

    #[test]
    fn the_name_lands_in_the_environment_and_not_the_permit_list() {
        let forwarding = extend_openssh_forwarding(Forwarding::default(), Some("claude"));
        assert_eq!(
            forwarding.env,
            EnvSpec::inherited().and(AGENT_VAR, "claude"),
            "the ssh child is what a manager reads"
        );
        assert!(
            forwarding.args.is_empty(),
            "a name nothing in the container reads must not be asked of the container"
        );
    }

    /// A command that names no agent must leave the launch exactly as it was.
    #[test]
    fn no_agent_changes_nothing() {
        let base = Forwarding {
            args: vec!["GH_TOKEN".to_owned()],
            env: EnvSpec::inherited().and("GH_TOKEN", "gho_x"),
        };
        let extended = extend_openssh_forwarding(base.clone(), None);
        assert_eq!(extended.args, base.args);
        assert_eq!(extended.env, base.env);
    }

    /// The credentials already on the line survive: this extends a forwarding, it
    /// does not replace one.
    #[test]
    fn the_forwarded_credentials_survive() {
        let base = Forwarding {
            args: vec!["GH_TOKEN".to_owned()],
            env: EnvSpec::inherited().and("GH_TOKEN", "gho_x"),
        };
        let extended = extend_openssh_forwarding(base, Some("claude"));
        assert_eq!(extended.args, vec!["GH_TOKEN".to_owned()]);
        assert_eq!(
            extended.env,
            EnvSpec::inherited()
                .and("GH_TOKEN", "gho_x")
                .and(AGENT_VAR, "claude")
        );
    }
}
