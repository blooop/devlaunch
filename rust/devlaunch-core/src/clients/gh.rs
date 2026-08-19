//! Carry the host's GitHub CLI credentials into every workspace devlaunch opens.
//!
//! Ported from `devlaunch/gh_auth.py`; see docs/rust-rewrite-plan.md (M3).
//!
//! devpod forwards the ssh agent and a git credential helper, but nothing carries
//! `gh` authentication, so `gh` starts out logged out in every container. A
//! devcontainer.json can bind-mount `~/.config/gh`, but that only helps the
//! projects that opted in, the mount target has to name the container user's home
//! directory, and it hands over nothing when the host keeps its token in a
//! keyring. A token in the environment needs no cooperation from the image, the
//! devcontainer.json or the container user, and `gh auth token` sources it either
//! way.
//!
//! # The token never touches argv
//!
//! Both routes into a workspace carry the value out of band, because `ps` shows a
//! command line to every other user on the host for as long as it runs — and
//! `devpod up` runs for minutes while an image builds:
//!
//! - `devpod up` gets [`StagedToken`]: a file only this user can read, named by
//!   `--workspace-env-file`, removed when the value is dropped.
//! - `devpod ssh` and OpenSSH get a name only — `--send-env GH_TOKEN`,
//!   `-o SendEnv=GH_TOKEN` — and the value travels in the child's environment
//!   ([`Forwarding::env`]).
//!
//! # No warnings, only events
//!
//! Python warns from inside `resolve_token`, and each way of having no token
//! warns differently: a refusing gh names the config directory gh consulted (a
//! run that scoped `XDG_CONFIG_HOME` hides the host's login, and `gh auth login`
//! is exactly the wrong remedy), a hung gh says the launch went on without it,
//! and junk on stdout says so **without repeating the junk** — whatever gh
//! printed may be a malformed credential, and a warning is not a place to put one.
//!
//! Core writes no English (#251 §5), so the reason travels as an arm of
//! [`TokenLookup`] and the words are the `dl` binary's. Two of Python's decisions
//! are kept as *type* decisions rather than message decisions:
//! [`GhEvent::NotAToken`] carries nothing at all, and [`Token`] does not print
//! itself.
//!
//! # What is deliberately absent
//!
//! Python memoizes the lookup for the life of the process, so that a single `dl`
//! run handing the token to both `devpod up` and `devpod ssh` unlocks a keyring
//! once and warns once. That is per-command state, the same kind as the memoized
//! `devpod list`, and it belongs to the flow that makes both calls — which is
//! also the thing that can decide to render one warning for a launch rather than
//! one per ask.

use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::runner::interrupt;
use crate::runner::{EnvSpec, Exit, Invocation, OsFailure, Outcome, Runner, SpawnSpec};
use crate::timing;

/// The program asked for the host's token.
pub(crate) const PROGRAM: &str = "gh";

/// The variable set inside the container. `gh` consults it before its config file.
pub(crate) const TOKEN_VAR: &str = "GH_TOKEN";

/// Set this to opt a machine out of forwarding entirely.
pub(crate) const DISABLE_VAR: &str = "DEVLAUNCH_NO_GH_TOKEN";

/// gh may have to unlock a keyring, so it does not get to stall a workspace
/// forever.
pub(crate) const GH_TIMEOUT: Duration = Duration::from_secs(10);

/// The values on the host this decision reads.
///
/// Parameters rather than reads of the process environment, so the decision is a
/// function of its inputs — and so a test states the host it means instead of
/// mutating an environment every other test in the binary shares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HostEnv {
    /// `DEVLAUNCH_NO_GH_TOKEN`.
    pub(crate) disable: Option<String>,
    /// `GH_TOKEN`. Read before `GITHUB_TOKEN`, and honouring it is also what lets
    /// a `dl` running inside a devlaunch workspace pass its own forwarded token
    /// further down.
    pub(crate) gh_token: Option<String>,
    /// `GITHUB_TOKEN`.
    pub(crate) github_token: Option<String>,
}

impl HostEnv {
    /// What this process's environment says.
    pub(crate) fn from_process() -> Self {
        Self {
            disable: crate::osext::env_str(DISABLE_VAR),
            gh_token: crate::osext::env_str(TOKEN_VAR),
            github_token: crate::osext::env_str("GITHUB_TOKEN"),
        }
    }

    /// The token the host exported, if either variable holds one.
    fn exported(&self) -> Option<(TokenSource, Token)> {
        [
            (TOKEN_VAR, &self.gh_token),
            ("GITHUB_TOKEN", &self.github_token),
        ]
        .into_iter()
        .find_map(|(variable, value)| {
            let token = Token::parse(value.as_deref()?)?;
            Some((TokenSource::Environment { variable }, token))
        })
    }
}

/// Whether the user opted this machine out of gh token forwarding.
///
/// Anything but the handful of falsey spellings opts out, because a variable a
/// user went out of their way to set means what they set it for. `_FALSEY` is
/// duplicated in `tty_session.py` and `dl.py` too; the shape is deliberately the
/// same at all three (see [`super::ssh::tty_disabled`]).
pub(crate) fn forwarding_disabled(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(value) => !FALSEY.contains(&crate::osext::strip(value).to_lowercase().as_str()),
    }
}

/// The values that mean "no" rather than "set, therefore yes".
const FALSEY: [&str; 4] = ["", "0", "false", "no"];

/// A value that has the shape every GitHub token has.
///
/// Its own type because the check has to happen once, at the boundary, rather
/// than at each of the three places that forward it: what `gh auth token` prints
/// is not necessarily a credential — a wrapper script that printed a message
/// would otherwise become the workspace's `GH_TOKEN`.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Token(String);

impl Token {
    /// `raw`, trimmed, if what is left is a token.
    ///
    /// Every GitHub token form is a flat ASCII string; anything else came from a
    /// broken gh install or a wrapper script that printed on stdout.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let trimmed = crate::osext::strip(raw);
        let flat = !trimmed.is_empty()
            && trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
        flat.then(|| Self(trimmed.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Redacted, deliberately. A credential must not reach a log line by any route,
/// and a derived `Debug` is the route nobody writes on purpose.
impl std::fmt::Debug for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Token(<redacted>)")
    }
}

/// Where a token came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenSource {
    /// The host exported it, so it cost no subprocess.
    Environment { variable: &'static str },
    /// `gh auth token` produced it, from a file or a keyring.
    GhCli,
}

/// gh could not be asked at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhUnavailable {
    /// It was still running when its deadline ran out — a keyring that never
    /// unlocked, most likely.
    TimedOut,
    Blocked(OsFailure),
}

/// Why this workspace is opening without a GitHub login, when that is worth
/// saying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhEvent {
    CouldNotRun(GhUnavailable),
    /// `gh auth token` ran and refused.
    ///
    /// The rendering names the config directory gh read: a run that scoped
    /// `XDG_CONFIG_HOME` to a scratch directory hides the host's gh login, so gh
    /// refuses even though the user is logged in — and `gh auth login` is exactly
    /// the wrong remedy for that.
    Refused {
        exit: Exit,
    },
    /// gh printed something that is not a token.
    ///
    /// Carries nothing on purpose: what gh printed may be a malformed credential,
    /// and the only safe thing to report is that it was not usable.
    NotAToken,
}

/// What asking the host for its GitHub token produced.
///
/// Three arms rather than a token beside an optional complaint: a lookup that
/// found a token has nothing to complain about, and a product of the two would
/// make that pair representable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TokenLookup {
    Found {
        token: Token,
        from: TokenSource,
    },
    /// Nothing to forward, and nothing to say about it: the user opted out (their
    /// own instruction, and announcing it back is noise), or gh is not installed
    /// (a choice, not a failure).
    NothingToForward,
    /// Nothing to forward, and the user should be told why.
    Unavailable(GhEvent),
}

impl TokenLookup {
    pub(crate) fn token(&self) -> Option<&Token> {
        match self {
            Self::Found { token, .. } => Some(token),
            Self::NothingToForward | Self::Unavailable(_) => None,
        }
    }
}

/// The host's GitHub token, or why there is none to forward.
pub(crate) fn resolve_token(runner: &dyn Runner, host: &HostEnv) -> TokenLookup {
    if forwarding_disabled(host.disable.as_deref()) {
        return TokenLookup::NothingToForward;
    }
    if let Some((from, token)) = host.exported() {
        return TokenLookup::Found { token, from };
    }
    token_from_gh_cli(runner)
}

/// Ask the gh CLI for the host's token.
///
/// The one trip in this module, and the one place it is timed. Python spans it
/// here too, inside `_token_from_gh_cli` and past the `shutil.which` guard, under
/// `host-prep`: the token is the host's to produce, and the trip is charged to that
/// owner even when it happens in the middle of the attach that needed it.
fn token_from_gh_cli(runner: &dyn Runner) -> TokenLookup {
    let spec = SpawnSpec::new(Invocation::new(PROGRAM).with_args(["auth", "token"]))
        // gh must not eat stdin that belongs to the command `dl` was asked to
        // run, and must not leave the terminal in a state of its own.
        .with_stdin_null()
        .with_timeout(GH_TIMEOUT);
    let started = Instant::now();
    let answered = runner.capture(&spec);
    let took = started.elapsed();
    // Measured by hand rather than with a span guard, because what is spanned is
    // the trip that *happened*: Python's `shutil.which` guard means a host with no
    // gh spans nothing and opens no stage, and here that is known only once the
    // spawn has answered.
    if !matches!(answered, Outcome::ProgramNotFound) {
        let _stage = timing::stage(timing::Stage::HostPrep);
        timing::record("gh auth token", took);
    }
    match answered {
        Outcome::Ran { exit, io } if exit.is_success() => match Token::parse(&io.stdout) {
            Some(token) => TokenLookup::Found {
                token,
                from: TokenSource::GhCli,
            },
            None => TokenLookup::Unavailable(GhEvent::NotAToken),
        },
        Outcome::Ran { exit, .. } => TokenLookup::Unavailable(GhEvent::Refused { exit }),
        // No gh, no login, nothing to report: Python checks `shutil.which` for
        // this and the runner has already answered the same question.
        Outcome::ProgramNotFound => TokenLookup::NothingToForward,
        Outcome::TimedOut => {
            TokenLookup::Unavailable(GhEvent::CouldNotRun(GhUnavailable::TimedOut))
        }
        Outcome::NotStarted(failure) => {
            TokenLookup::Unavailable(GhEvent::CouldNotRun(GhUnavailable::Blocked(failure)))
        }
    }
}

/// The token in a file only this user can read, for `devpod up`.
///
/// `--workspace-env-file` is a devpod flag of its own, so it adds to whatever the
/// user configured through `--workspace-env` instead of displacing it. devpod
/// re-applies the workspace env on every `up`, so a token that has since changed
/// on the host reaches even a container that is already running.
///
/// The file is removed when this value is dropped, which is what Python's
/// context manager does — including on the path where devpod fails. It is also
/// registered for interrupt-time cleanup ([`interrupt::register_file`]): a drop
/// does not run on `_exit(130)`, so a Ctrl-C during the minutes-long `devpod up`
/// this file is handed to would otherwise leave the plaintext token on disk
/// (concurrency review F2/H4/R8).
#[derive(Debug)]
pub(crate) struct StagedToken {
    // `file` is declared before `_cleanup` so it drops first: the tempfile
    // removes the file, and only then is the interrupt slot released — there is
    // never a moment where the slot is clear but the file is still on disk.
    file: tempfile::NamedTempFile,
    /// Keeps the path registered for interrupt-time `unlink` for exactly this
    /// token's lifetime. `None` only if every slot was full or the path had a
    /// NUL, which costs the interrupt cleanup nothing else can (the ordinary
    /// drop still removes the file on a clean exit).
    _cleanup: Option<interrupt::Registration>,
}

impl StagedToken {
    /// Write `token` to a private file, or say why that failed.
    ///
    /// Forwarding a credential is a convenience, so a full or read-only temp
    /// directory has to cost the workspace its gh login and not its launch —
    /// which is the caller's decision to make, and is why this returns the
    /// refusal rather than swallowing it.
    pub(crate) fn stage(token: &Token) -> std::io::Result<Self> {
        // 0600 from the start, as `mkstemp` gives it: a credential must never be
        // world-readable, not even for the moment before a chmod.
        let mut file = tempfile::Builder::new()
            .prefix("devlaunch-gh-")
            .suffix(".env")
            .tempfile_in(crate::osext::temp_dir())?;
        writeln!(file, "{TOKEN_VAR}={}", token.as_str())?;
        file.flush()?;
        let cleanup = interrupt::register_file(file.path());
        Ok(Self {
            file,
            _cleanup: cleanup,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        self.file.path()
    }

    /// The `devpod up` flags that put the host's token in the workspace env.
    pub(crate) fn up_args(&self) -> Vec<String> {
        vec![
            "--workspace-env-file".to_owned(),
            self.path().to_string_lossy().into_owned(),
        ]
    }
}

/// Flags for a child, plus the environment it must be run with.
///
/// The names go in argv and the values go in the environment, which is the whole
/// discipline: only the name of the variable is ever readable by another user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Forwarding {
    pub(crate) args: Vec<String>,
    pub(crate) env: EnvSpec,
}

/// `devpod ssh` flags plus the environment devpod must be run with.
///
/// This covers attaching to a workspace that is already running, which skips
/// `devpod up` and its workspace env entirely. devpod lets a workspace env value
/// win over `--send-env`, so this tops up workspaces devlaunch never created
/// rather than overriding a token `devpod up` just delivered — and cannot refresh
/// one either: a running workspace whose token was revoked needs a restart.
pub(crate) fn ssh_forwarding(token: Option<&Token>) -> Forwarding {
    forwarding(token, |_| {
        vec!["--send-env".to_owned(), TOKEN_VAR.to_owned()]
    })
}

/// The same forwarding for the OpenSSH transport that carries a terminal.
///
/// Names only: `clients::ssh` turns them into `-o SendEnv=NAME`, and OpenSSH
/// reads the values from its own environment.
pub(crate) fn openssh_forwarding(token: Option<&Token>) -> Forwarding {
    forwarding(token, |_| vec![TOKEN_VAR.to_owned()])
}

/// The shared half: no token means no flags and an environment nobody touched.
fn forwarding(token: Option<&Token>, args: impl FnOnce(&Token) -> Vec<String>) -> Forwarding {
    match token {
        None => Forwarding::default(),
        Some(token) => Forwarding {
            args: args(token),
            // Python copies `os.environ` because its `env=` replaces the whole
            // environment and would otherwise drop PATH; a parent base plus one
            // entry says the same thing without the copy.
            env: EnvSpec::inherited().and(TOKEN_VAR, token.as_str()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{EnvBase, StdinPlan};
    use crate::testing::ScriptedRunner;
    use devlaunch_test_support::{Call, Response};

    /// A host with nothing exported: no opt-out and no token, which is the state
    /// the forwarding is interesting in.
    fn bare_host() -> HostEnv {
        HostEnv::default()
    }

    fn a_token(raw: &str) -> Token {
        Token::parse(raw).expect("a token")
    }

    // ------------------------------------------------------ where it comes from

    #[test]
    fn a_token_the_host_exported_is_already_the_answer_gh_would_give() {
        let fake = ScriptedRunner::new();
        let host = HostEnv {
            gh_token: Some("gho_fromenv".to_owned()),
            ..bare_host()
        };

        let lookup = resolve_token(&fake, &host);

        assert_eq!(lookup.token().map(Token::as_str), Some("gho_fromenv"));
        assert_eq!(fake.call_count(), 0, "gh was not worth a subprocess");
    }

    #[test]
    fn the_source_of_the_token_travels_with_it() {
        let fake = ScriptedRunner::new();
        let host = HostEnv {
            gh_token: Some("gho_fromenv".to_owned()),
            ..bare_host()
        };

        match resolve_token(&fake, &host) {
            TokenLookup::Found { from, .. } => assert_eq!(
                from,
                TokenSource::Environment {
                    variable: "GH_TOKEN"
                }
            ),
            other => panic!("expected a token, got {other:?}"),
        }
    }

    #[test]
    fn github_token_is_accepted_too_and_gh_token_is_read_first() {
        let fake = ScriptedRunner::new();
        let only_github = HostEnv {
            github_token: Some("ghp_fromenv".to_owned()),
            ..bare_host()
        };
        let both = HostEnv {
            gh_token: Some("gho_first".to_owned()),
            github_token: Some("ghp_second".to_owned()),
            ..bare_host()
        };

        assert_eq!(
            resolve_token(&fake, &only_github)
                .token()
                .map(Token::as_str),
            Some("ghp_fromenv")
        );
        assert_eq!(
            resolve_token(&fake, &both).token().map(Token::as_str),
            Some("gho_first")
        );
    }

    #[test]
    fn an_exported_value_is_read_the_way_a_shell_left_it() {
        // Python strips the value, so a variable set from command substitution
        // does not forward a trailing newline as part of the credential.
        let fake = ScriptedRunner::new();
        let host = HostEnv {
            gh_token: Some("  gho_padded \n".to_owned()),
            ..bare_host()
        };

        assert_eq!(
            resolve_token(&fake, &host).token().map(Token::as_str),
            Some("gho_padded")
        );
    }

    #[test]
    fn an_exported_value_that_is_not_a_token_is_not_forwarded_as_one() {
        // A shell profile that set GH_TOKEN to a sentence must not become the
        // workspace's credential; gh is asked instead.
        let fake = ScriptedRunner::new()
            .with_script(["gh", "auth", "token"], Response::stdout("gho_fromcli\n"));
        let host = HostEnv {
            gh_token: Some("not a token".to_owned()),
            ..bare_host()
        };

        assert_eq!(
            resolve_token(&fake, &host).token().map(Token::as_str),
            Some("gho_fromcli")
        );
    }

    #[test]
    fn the_gh_cli_is_asked_with_no_terminal_and_a_deadline() {
        // gh sources the token whether the host keeps it in a file or a keyring.
        // It must not eat stdin belonging to the command `dl` was asked to run,
        // and it may have to unlock a keyring, so it does not get forever.
        let fake = ScriptedRunner::new()
            .with_script(["gh", "auth", "token"], Response::stdout("gho_fromcli\n"));

        let lookup = resolve_token(&fake, &bare_host());

        assert_eq!(lookup.token().map(Token::as_str), Some("gho_fromcli"));
        assert_eq!(
            fake.argvs(),
            vec![vec!["gh".to_owned(), "auth".to_owned(), "token".to_owned()]]
        );
        let call = fake.only_call();
        assert!(matches!(call, Call::Capture(_)), "{call:?}");
        let spec = call.spec().expect("gh is never detached");
        assert_eq!(spec.stdin, StdinPlan::Null);
        assert_eq!(spec.timeout, Some(GH_TIMEOUT));
    }

    #[test]
    fn a_host_with_no_gh_installed_forwards_nothing_and_says_nothing() {
        // Not installing gh is a choice, not a failure; nagging about it is noise.
        let fake = ScriptedRunner::new().with_missing("gh");

        assert_eq!(
            resolve_token(&fake, &bare_host()),
            TokenLookup::NothingToForward
        );
    }

    #[test]
    fn a_gh_that_refused_is_reported_so_the_workspace_does_not_open_logged_out_in_silence() {
        let fake = ScriptedRunner::new().with_script(["gh", "auth", "token"], Response::exited(1));

        assert_eq!(
            resolve_token(&fake, &bare_host()),
            TokenLookup::Unavailable(GhEvent::Refused {
                exit: Exit::Code(1)
            })
        );
    }

    #[test]
    fn prose_on_stdout_is_not_mistaken_for_a_token() {
        // A wrapper script that prints a message must not become GH_TOKEN.
        let fake = ScriptedRunner::new().with_script(
            ["gh", "auth", "token"],
            Response::stdout("error: sorry, ~SECRETish~ value here\n"),
        );

        let lookup = resolve_token(&fake, &bare_host());

        assert_eq!(lookup, TokenLookup::Unavailable(GhEvent::NotAToken));
        assert!(
            !format!("{lookup:?}").contains("SECRETish"),
            "whatever gh printed may be a malformed credential: {lookup:?}"
        );
    }

    #[test]
    fn a_hung_gh_does_not_hang_the_launch() {
        // gh may block on a locked keyring; a workspace must still open.
        let fake = ScriptedRunner::new().with_script(["gh"], Response::TimedOut);

        assert_eq!(
            resolve_token(&fake, &bare_host()),
            TokenLookup::Unavailable(GhEvent::CouldNotRun(GhUnavailable::TimedOut))
        );
    }

    #[test]
    fn a_gh_the_os_would_not_start_is_reported_with_its_errno() {
        let failure = OsFailure {
            kind: std::io::ErrorKind::PermissionDenied,
            errno: Some(13),
        };
        let fake = ScriptedRunner::new().with_script(["gh"], Response::NotStarted(failure));

        assert_eq!(
            resolve_token(&fake, &bare_host()),
            TokenLookup::Unavailable(GhEvent::CouldNotRun(GhUnavailable::Blocked(failure)))
        );
    }

    // -------------------------------------------------------- the timing span
    //
    // `test/test_timing.py`'s `TestTransportAndGitGhCallsAreNamed` and
    // `TestHostPrepIsAStage` assert this from outside; Python's span and stage sit
    // inside `_token_from_gh_cli`, so this is the module that owns them.

    /// The stages and spans a measured `record` produced.
    fn measured(record: impl FnOnce()) -> timing::Document {
        let _serialized = timing::exclusive();
        timing::install(Some(timing::Registry::start(
            timing::Mode::Document,
            timing::Seam::default(),
            0.0,
        )));
        record();
        timing::emit()
            .expect("a report from an installed registry")
            .document()
            .expect("document mode was asked for")
            .clone()
    }

    fn stage_names(document: &timing::Document) -> Vec<&str> {
        document.stages.iter().map(|stage| stage.stage).collect()
    }

    fn span_labels(document: &timing::Document, stage: &str) -> Vec<String> {
        document
            .stages
            .iter()
            .filter(|record| record.stage == stage)
            .flat_map(|record| record.spans.iter().map(|span| span.label.clone()))
            .collect()
    }

    #[test]
    fn the_token_round_trip_is_named_and_charged_to_host_prep() {
        // Host prep is an owner, not a region of the timeline: the token is the
        // host's to produce, and the trip is charged to that owner wherever on the
        // launch it happens.
        let fake = ScriptedRunner::new().with_script(
            ["gh", "auth", "token"],
            Response::stdout(format!("gho_{}\n", "a".repeat(36))),
        );

        let document = measured(|| {
            assert!(resolve_token(&fake, &bare_host()).token().is_some());
        });

        assert_eq!(stage_names(&document), ["host-prep"]);
        assert_eq!(span_labels(&document, "host-prep"), ["gh auth token"]);
    }

    #[test]
    fn a_gh_that_refused_is_still_named_and_timed() {
        // The span wraps the trip, not the answer: a refusal took time too.
        let fake = ScriptedRunner::new().with_script(["gh"], Response::exited(1));

        let document = measured(|| {
            assert!(resolve_token(&fake, &bare_host()).token().is_none());
        });

        assert_eq!(span_labels(&document, "host-prep"), ["gh auth token"]);
    }

    #[test]
    fn a_host_with_no_gh_records_no_span_and_opens_no_stage() {
        // Python's `shutil.which` guard means a machine without gh never reaches
        // the span; here the spawn is what answers, and nothing is charged for it.
        let fake = ScriptedRunner::new().with_missing("gh");

        let document = measured(|| {
            assert_eq!(
                resolve_token(&fake, &bare_host()),
                TokenLookup::NothingToForward
            );
        });

        assert_eq!(stage_names(&document), [] as [&str; 0]);
    }

    #[test]
    fn a_token_the_host_exported_costs_no_span_at_all() {
        // `resolve_token` answers from the environment without spawning, and
        // Python's span sits inside the branch that spawns.
        let fake = ScriptedRunner::new();
        let host = HostEnv {
            gh_token: Some("gho_fromenv".to_owned()),
            ..bare_host()
        };

        let document = measured(|| {
            assert!(resolve_token(&fake, &host).token().is_some());
        });

        assert_eq!(stage_names(&document), [] as [&str; 0]);
    }

    // ------------------------------------------------------------ the opt-out

    #[test]
    fn opting_out_beats_an_available_token_and_asks_gh_nothing() {
        // The opt-out is the user's own instruction; announcing it back is noise.
        let fake = ScriptedRunner::new();
        let host = HostEnv {
            disable: Some("1".to_owned()),
            gh_token: Some("gho_fromenv".to_owned()),
            ..bare_host()
        };

        assert_eq!(resolve_token(&fake, &host), TokenLookup::NothingToForward);
        assert_eq!(fake.call_count(), 0);
    }

    #[test]
    fn a_falsey_opt_out_leaves_forwarding_on() {
        for value in ["", "0", "false", "no", " NO ", "FALSE"] {
            assert!(
                !forwarding_disabled(Some(value)),
                "{value:?} does not opt out"
            );
        }
        assert!(!forwarding_disabled(None), "unset leaves forwarding on");
        assert!(forwarding_disabled(Some("1")));
        assert!(forwarding_disabled(Some("yes")));
    }

    #[test]
    fn the_control_separators_python_strips_do_not_invert_the_switch() {
        // Python's str.strip() removes U+001C–U+001F; str::trim does not. A "0"
        // wrapped in them is still a falsey opt-out (forwarding stays on), not a
        // set-therefore-yes opt-out. `\u{1c}` alone must strip to empty, which is
        // falsey too.
        for value in [
            "\u{1c}0\u{1c}",
            "\u{1f}false\u{1f}",
            "\u{1c}",
            "\u{1e}no\u{1d}",
        ] {
            assert!(
                !forwarding_disabled(Some(value)),
                "{value:?} strips to a falsey value"
            );
        }
    }

    #[test]
    fn a_token_wrapped_in_control_separators_still_parses() {
        // A valid token surrounded by U+001C–U+001F must survive: str::trim would
        // leave the separators in place and the shape check would reject it.
        let token = Token::parse("\u{1c}gho_secret\u{1f}").expect("a token strips clean");
        assert_eq!(token.as_str(), "gho_secret");
    }

    // -------------------------------------------------------------- the token

    #[test]
    fn every_github_token_form_is_a_flat_ascii_string() {
        for raw in [
            "gho_16C7e42F292c6912E7710c838347Ae178B4a",
            "ghp_A-b.c_d",
            "github_pat_11ABC_xyz.123",
        ] {
            assert_eq!(
                Token::parse(raw).map(|t| t.as_str().to_owned()),
                Some(raw.to_owned())
            );
        }
    }

    #[test]
    fn anything_that_is_not_a_token_is_refused_rather_than_forwarded() {
        for raw in [
            "",
            "   ",
            "not a token",
            "error: not logged in",
            "gho_x\ngho_y",
            "gho_x;rm -rf /",
        ] {
            assert_eq!(Token::parse(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn a_token_never_prints_itself() {
        // A credential in scope must not reach a log line by any route, and a
        // derived `Debug` is the route nobody writes on purpose.
        let token = a_token("gho_secret");

        assert!(!format!("{token:?}").contains("gho_secret"), "{token:?}");
    }

    // ------------------------------------------------------- what devpod gets

    #[test]
    fn the_token_travels_to_devpod_up_in_a_file_and_never_in_argv() {
        // `devpod up` can run for minutes while an image builds, and its argv is
        // readable by every user on the host for that whole time.
        let staged = StagedToken::stage(&a_token("gho_secret")).expect("a staged token");

        let args = staged.up_args();

        assert_eq!(args[0], "--workspace-env-file");
        assert!(
            !args.join(" ").contains("gho_secret"),
            "the token must not reach argv: {args:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&args[1]).expect("the staged file"),
            "GH_TOKEN=gho_secret\n"
        );
    }

    #[test]
    fn the_staged_file_is_private_to_this_user() {
        use std::os::unix::fs::PermissionsExt as _;
        let staged = StagedToken::stage(&a_token("gho_secret")).expect("a staged token");

        let mode = std::fs::metadata(staged.path())
            .expect("the staged file")
            .permissions()
            .mode();

        assert_eq!(mode & 0o077, 0, "group and other can read it: {mode:o}");
    }

    #[test]
    fn the_staged_file_does_not_outlive_the_command() {
        let path = {
            let staged = StagedToken::stage(&a_token("gho_secret")).expect("a staged token");
            staged.path().to_owned()
        };

        assert!(
            !path.exists(),
            "a credential left on disk is the failure this exists to avoid"
        );
    }

    #[test]
    fn only_the_variable_name_reaches_devpod_ssh() {
        // `--send-env` names the variable; devpod reads the value from its own
        // environment. This covers attaching to a workspace that is already
        // running, which skips `devpod up` and its workspace env entirely.
        let token = a_token("gho_secret");

        let forwarding = ssh_forwarding(Some(&token));

        assert_eq!(forwarding.args, vec!["--send-env", TOKEN_VAR]);
        assert!(!forwarding.args.join(" ").contains("gho_secret"));
        assert_eq!(
            forwarding.env.entries.get(TOKEN_VAR).map(String::as_str),
            Some("gho_secret")
        );
    }

    #[test]
    fn the_rest_of_the_environment_is_preserved() {
        // Python builds `{**os.environ, GH_TOKEN: token}` because its `env=`
        // replaces devpod's whole environment, so it has to carry PATH along.
        // The runner's spec says the same thing without the copy.
        let token = a_token("gho_secret");

        assert_eq!(ssh_forwarding(Some(&token)).env.base, EnvBase::Parent);
    }

    #[test]
    fn no_token_leaves_devpod_untouched() {
        let forwarding = ssh_forwarding(None);

        assert!(forwarding.args.is_empty());
        assert_eq!(forwarding.env, EnvSpec::inherited());
    }

    #[test]
    fn the_openssh_transport_forwards_the_same_token_by_name() {
        // Interactive payloads reach the workspace through `ssh` rather than
        // `devpod ssh`, which spells the same idea `-o SendEnv=NAME`. Only the
        // names are returned; `clients::ssh` turns them into flags.
        let token = a_token("gho_secret");

        let forwarding = openssh_forwarding(Some(&token));

        assert_eq!(forwarding.args, vec![TOKEN_VAR.to_owned()]);
        assert_eq!(
            forwarding.env.entries.get(TOKEN_VAR).map(String::as_str),
            Some("gho_secret")
        );
        assert!(openssh_forwarding(None).args.is_empty());
    }
}
