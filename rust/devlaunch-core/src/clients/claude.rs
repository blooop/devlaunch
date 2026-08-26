//! The host's Claude login, forwarded into a workspace.
//!
//! The same job [`super::gh`] does for GitHub, and deliberately the same shape: a
//! value the host produces, checked once at the boundary, delivered through
//! devpod's environment so the secret never reaches an argv another user could
//! read. What differs is where it comes from and how far it travels, and both
//! differences are the point.
//!
//! # Why an environment variable and not the credential file
//!
//! Claude Code reads `CLAUDE_CODE_OAUTH_TOKEN` and, measured against 2.1.246, it
//! authenticates from that variable alone with an otherwise empty `$HOME`: no
//! credential file is consulted and none is written. Writing
//! `~/.claude/.credentials.json` into a container would work too, and it is the
//! wrong trade twice over. It leaves a secret on the container's disk, where a
//! `docker cp` or a stray image commit carries it off. And that file holds the
//! *refresh* token as well as the access token, so a container that refreshed it
//! could rotate the host's own login out from under the user. A variable holds one
//! short-lived access token and nothing else.
//!
//! The cost is that the access token is short-lived — hours, not days — and a
//! variable cannot be refreshed in place. A workspace left open past its token's
//! expiry has a dead one until the next launch replaces it, which is the caveat
//! [`super::gh`] already documents for `GH_TOKEN` and for the same reason.
//!
//! # Why only the sessions dl opens
//!
//! `GH_TOKEN` goes into devpod's *workspace* environment at `up`, which is what
//! makes it available to a repo's `postCreateCommand`. This one deliberately does
//! not. It rides `--send-env` on the sessions devlaunch itself opens, so:
//!
//! - `dl someone/repo` does not run a stranger's `postCreateCommand` with the
//!   user's Claude login in reach. That is a narrower trust boundary than the gh
//!   token's, not a wider one.
//! - Nothing is persisted in devpod's workspace configuration, so there is no
//!   stale token to find later.
//! - Every session re-reads the host's credential, which is what makes
//!   refresh-on-start free.
//!
//! It also means a session devlaunch did not open — `devpod ssh` by hand, VS Code
//! through `dl <ws> code` — does not get it. Those already have the host's real
//! `claude` available to them by other means, and widening this to cover them
//! would mean widening it to cover `postCreateCommand` too.

use std::path::Path;

use super::gh::{Forwarding, forwarding_disabled};
use crate::runner::EnvSpec;

/// The variable set inside the container. Claude Code consults it before any
/// credential file, which is exactly why the file is left alone: see the
/// [module note](self) and [`super::super::flows::provision::ClaudeConfig`].
pub(crate) const TOKEN_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// Set this to opt a machine out of forwarding the Claude login entirely.
pub(crate) const DISABLE_VAR: &str = "DEVLAUNCH_NO_CLAUDE_TOKEN";

/// Where the host keeps the credential, relative to `$HOME`.
const CREDENTIALS_RELPATH: &str = ".claude/.credentials.json";

/// The key the OAuth credential sits under, and the field wanted from it.
const OAUTH_KEY: &str = "claudeAiOauth";
const ACCESS_TOKEN_KEY: &str = "accessToken";

/// A value that has the shape a Claude OAuth token has.
///
/// Its own type for the reason [`super::gh::Token`] has one: the check belongs at
/// the boundary, once, rather than at each place that forwards it. A credential
/// file that has been truncated, hand-edited, or written by some other tool
/// produces something that is not a token, and the difference between "no token"
/// and "a token-shaped nothing" is the difference between a warning and a
/// container that fails to authenticate for reasons nobody can see.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Token(String);

/// Redacted, deliberately, exactly as [`super::gh::Token`] is. A credential must not
/// reach a log line by any route, and a derived `Debug` is the route nobody writes
/// on purpose -- [`TokenLookup`] and [`Forwarding`] both derive one and both hold
/// this.
impl std::fmt::Debug for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Token(<redacted>)")
    }
}

impl Token {
    /// `raw`, trimmed, if what is left is a token.
    ///
    /// The same flat-ASCII rule [`super::gh::Token::parse`] applies, and for the
    /// same reason: every form of this credential is a flat ASCII string, so
    /// anything else did not come from a credential.
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

/// The values on the host this decision reads.
///
/// Parameters rather than reads of the process environment, exactly as
/// [`super::gh::HostEnv`] is, so the decision is a function of its inputs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HostEnv {
    /// `DEVLAUNCH_NO_CLAUDE_TOKEN`.
    pub(crate) disable: Option<String>,
    /// `CLAUDE_CODE_OAUTH_TOKEN`. Read before the credential file, which is also
    /// what lets a `dl` running inside a workspace pass its own forwarded token
    /// further down — the same courtesy [`super::gh::HostEnv`] extends.
    pub(crate) token: Option<String>,
}

impl HostEnv {
    /// What this process's environment says.
    pub(crate) fn from_process() -> Self {
        Self {
            disable: crate::osext::env_str(DISABLE_VAR),
            token: crate::osext::env_str(TOKEN_VAR),
        }
    }
}

/// Why there is no Claude login to forward.
///
/// Named rather than a bare `None`, because these read very differently to a user
/// and only one of them is worth a warning: opting out is a choice, and a host
/// that never logged in is a fact about the host, but a credential file that is
/// there and unreadable is a problem the user can fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NoToken {
    /// `DEVLAUNCH_NO_CLAUDE_TOKEN` is set.
    OptedOut,
    /// No credential file. A host that has not run `claude`, or a macOS host,
    /// where the login lives in the login keychain and not on the filesystem.
    NotLoggedIn,
    /// The file is there and did not yield a token.
    Unreadable(String),
}

/// The host's Claude token, or why there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TokenLookup {
    Found(Token),
    Missing(NoToken),
}

/// The host's Claude token, or why there is none to forward.
///
/// `home` is a parameter and not a read of `$HOME` so a test can state the host it
/// means. No subprocess and no timing span, unlike [`super::gh::resolve_token`]:
/// this is one file read, and there is no CLI to ask.
pub(crate) fn resolve_token(home: Option<&Path>, host: &HostEnv) -> TokenLookup {
    if forwarding_disabled(host.disable.as_deref()) {
        return TokenLookup::Missing(NoToken::OptedOut);
    }
    if let Some(token) = host.token.as_deref().and_then(Token::parse) {
        return TokenLookup::Found(token);
    }
    let Some(home) = home else {
        return TokenLookup::Missing(NoToken::NotLoggedIn);
    };
    let path = home.join(CREDENTIALS_RELPATH);
    let Ok(text) = std::fs::read_to_string(&path) else {
        // Not distinguished from "no such file", because the two are the same
        // answer for the same reason: on macOS the credential is in the login
        // keychain, so its absence from the filesystem is the normal case and not
        // worth a warning on every launch.
        return TokenLookup::Missing(NoToken::NotLoggedIn);
    };
    match token_from_credentials(&text) {
        Some(token) => TokenLookup::Found(token),
        None => TokenLookup::Missing(NoToken::Unreadable(path.display().to_string())),
    }
}

/// Pull the access token out of a credential file's text.
///
/// Deliberately not a `Deserialize` struct for the whole file. The file belongs to
/// Claude Code and gains keys on its own schedule; a struct would either have to
/// name them all or carry `#[serde(default)]` on fields devlaunch does not read.
/// Two lookups through `serde_json::Value` say what is wanted and ignore the rest.
fn token_from_credentials(text: &str) -> Option<Token> {
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    Token::parse(parsed.get(OAUTH_KEY)?.get(ACCESS_TOKEN_KEY)?.as_str()?)
}

/// Add the Claude login to the flags and environment a session is opened with.
///
/// Extends rather than replaces, because a session already carries
/// [`super::gh::ssh_forwarding`]'s and the two are independent: either, both or
/// neither may have something to send. `--send-env` names the variable in argv and
/// the value travels in the environment, which is the whole discipline this shares
/// with gh — only the name is ever readable by another user.
pub(crate) fn extend_ssh_forwarding(base: Forwarding, token: Option<&Token>) -> Forwarding {
    let Some(token) = token else {
        return base;
    };
    let Forwarding { mut args, env } = base;
    args.push("--send-env".to_owned());
    args.push(TOKEN_VAR.to_owned());
    Forwarding {
        args,
        env: inherited(env).and(TOKEN_VAR, token.as_str()),
    }
}

/// The same, for the OpenSSH transport, whose flags are bare variable names.
///
/// The two transports both have to carry this: `dl <ws> -- claude` goes to OpenSSH
/// rather than `devpod ssh`, because devpod never requests a pty and `claude` reads
/// a pipe as a non-interactive invocation. Forwarding on one and not the other would
/// leave the single most likely command in a workspace without a login.
pub(crate) fn extend_openssh_forwarding(base: Forwarding, token: Option<&Token>) -> Forwarding {
    let Some(token) = token else {
        return base;
    };
    let Forwarding { mut args, env } = base;
    args.push(TOKEN_VAR.to_owned());
    Forwarding {
        args,
        env: inherited(env).and(TOKEN_VAR, token.as_str()),
    }
}

/// A base environment that is a parent environment.
///
/// `Forwarding::default()` carries `EnvSpec::default()`, which is already the
/// inherited one -- this exists so that reading the code does not require knowing
/// that, and so a change to `EnvSpec`'s default cannot silently hand a session an
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

    /// A host whose `~/.claude/.credentials.json` holds `token`.
    fn logged_in(token: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("a scratch home");
        let dir = home.path().join(".claude");
        std::fs::create_dir_all(&dir).expect("a config dir");
        std::fs::write(
            dir.join(".credentials.json"),
            format!(
                r#"{{"claudeAiOauth":{{"accessToken":"{token}","refreshToken":"not-a-real-refresh-token",
                   "expiresAt":1,"scopes":["user:inference"],"subscriptionType":"team"}},
                   "mcpOAuth":{{}}}}"#
            ),
        )
        .expect("a credential");
        home
    }

    #[test]
    fn the_access_token_is_what_travels_and_the_refresh_token_is_not() {
        // The whole reason this reads the file rather than shipping it: the file
        // holds a refresh token, and a container that refreshed one could rotate the
        // host's own login away. Only the access token is ever named.
        let home = logged_in("not-a-real-access-token");
        let found = resolve_token(Some(home.path()), &HostEnv::default());
        assert_eq!(
            found,
            TokenLookup::Found(Token("not-a-real-access-token".to_owned()))
        );
        let TokenLookup::Found(token) = found else {
            unreachable!()
        };
        assert!(!token.as_str().contains("refresh"));
    }

    #[test]
    fn the_opt_out_is_read_before_the_credential_is() {
        // Set, therefore meant: `DEVLAUNCH_NO_CLAUDE_TOKEN=1` on a host that is
        // perfectly well logged in forwards nothing, and says which of the three
        // reasons it was.
        let home = logged_in("not-a-real-access-token");
        let host = HostEnv {
            disable: Some("1".to_owned()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(Some(home.path()), &host),
            TokenLookup::Missing(NoToken::OptedOut)
        );
    }

    #[test]
    fn a_falsey_opt_out_is_not_an_opt_out() {
        // The spelling `gh` uses, shared deliberately: `DEVLAUNCH_NO_CLAUDE_TOKEN=0`
        // is somebody turning it off, not on.
        let home = logged_in("not-a-real-access-token");
        for falsey in ["", "0", "false", "no"] {
            let host = HostEnv {
                disable: Some(falsey.to_owned()),
                ..HostEnv::default()
            };
            assert!(
                matches!(
                    resolve_token(Some(home.path()), &host),
                    TokenLookup::Found(_)
                ),
                "{falsey:?}"
            );
        }
    }

    #[test]
    fn a_host_that_exported_one_itself_passes_it_further_down() {
        // A `dl` running inside a workspace has the variable and no credential file,
        // and forwarding it on is what lets a workspace launch a workspace.
        let host = HostEnv {
            token: Some("  not-a-real-exported-token\n".to_owned()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(None, &host),
            TokenLookup::Found(Token("not-a-real-exported-token".to_owned()))
        );
    }

    #[test]
    fn a_host_with_no_credential_is_not_logged_in_rather_than_broken() {
        // The macOS case, where the login is in the keychain, and the never-ran-claude
        // case. Both are ordinary and neither is worth a warning on every launch.
        let home = tempfile::tempdir().expect("a scratch home");
        assert_eq!(
            resolve_token(Some(home.path()), &HostEnv::default()),
            TokenLookup::Missing(NoToken::NotLoggedIn)
        );
        assert_eq!(
            resolve_token(None, &HostEnv::default()),
            TokenLookup::Missing(NoToken::NotLoggedIn)
        );
    }

    #[test]
    fn a_credential_that_is_there_and_yields_nothing_is_worth_saying() {
        // The one of the three that a user can act on, so it is the one that names a
        // path. A file Claude Code has not written yet, a truncated write, a hand-edit.
        for text in [
            "",
            "not json at all",
            r#"{"claudeAiOauth":{}}"#,
            r#"{"claudeAiOauth":{"accessToken":""}}"#,
            r#"{"claudeAiOauth":{"accessToken":"has a space"}}"#,
            r#"{"somethingElse":{"accessToken":"not-a-real-token"}}"#,
        ] {
            let home = tempfile::tempdir().expect("a scratch home");
            std::fs::create_dir_all(home.path().join(".claude")).expect("a config dir");
            std::fs::write(home.path().join(CREDENTIALS_RELPATH), text).expect("a file");
            assert!(
                matches!(
                    resolve_token(Some(home.path()), &HostEnv::default()),
                    TokenLookup::Missing(NoToken::Unreadable(_))
                ),
                "{text:?}"
            );
        }
    }

    #[test]
    fn nothing_to_forward_leaves_the_session_exactly_as_it_was() {
        let base = Forwarding {
            args: vec!["--send-env".to_owned(), "GH_TOKEN".to_owned()],
            env: EnvSpec::inherited().and("GH_TOKEN", "gho_x"),
        };
        assert_eq!(extend_ssh_forwarding(base.clone(), None), base);
    }

    #[test]
    fn the_name_goes_in_argv_and_the_value_does_not() {
        // The whole discipline this shares with gh: another user reading `ps` sees
        // which variable is being sent and never what is in it.
        let token = Token("not-a-real-secret-token".to_owned());
        let extended = extend_ssh_forwarding(Forwarding::default(), Some(&token));
        assert_eq!(
            extended.args,
            vec!["--send-env".to_owned(), TOKEN_VAR.to_owned()]
        );
        assert!(!extended.args.iter().any(|arg| arg.contains("secret")));
        assert_eq!(
            extended.env,
            EnvSpec::inherited().and(TOKEN_VAR, "not-a-real-secret-token")
        );
    }

    #[test]
    fn a_token_never_prints_itself() {
        // The rule `gh::Token` states and tests, and this type had lost it to a
        // derived `Debug`. `TokenLookup` and `Forwarding` both derive one and both
        // hold a `Token`, so the derive was a live route to a log line.
        let token = Token("not-a-real-secret-token".to_owned());
        assert!(!format!("{token:?}").contains("secret"), "{token:?}");
        let lookup = TokenLookup::Found(token);
        assert!(!format!("{lookup:?}").contains("secret"), "{lookup:?}");
    }

    #[test]
    fn the_openssh_transport_names_the_variable_and_keeps_the_value_out_of_argv() {
        // The transport `dl <ws> -- claude` actually goes down, whose flags are bare
        // variable names rather than `--send-env` pairs. It had no test at all.
        let token = Token("not-a-real-secret-token".to_owned());
        let extended = extend_openssh_forwarding(Forwarding::default(), Some(&token));
        assert_eq!(extended.args, vec![TOKEN_VAR.to_owned()]);
        assert!(!extended.args.iter().any(|arg| arg.contains("secret")));
        assert_eq!(
            extended.env,
            EnvSpec::inherited().and(TOKEN_VAR, "not-a-real-secret-token")
        );
    }

    #[test]
    fn the_openssh_transport_leaves_a_session_with_nothing_to_send_alone() {
        let base = Forwarding {
            args: vec!["GH_TOKEN".to_owned()],
            env: EnvSpec::inherited().and("GH_TOKEN", "gho_x"),
        };
        assert_eq!(extend_openssh_forwarding(base.clone(), None), base);
        let token = Token("not-a-real-secret-token".to_owned());
        let extended = extend_openssh_forwarding(base, Some(&token));
        assert_eq!(
            extended.args,
            vec!["GH_TOKEN".to_owned(), TOKEN_VAR.to_owned()]
        );
    }

    #[test]
    fn extending_a_session_that_already_forwards_gh_keeps_both() {
        let token = Token("not-a-real-secret-token".to_owned());
        let base = Forwarding {
            args: vec!["--send-env".to_owned(), "GH_TOKEN".to_owned()],
            env: EnvSpec::inherited().and("GH_TOKEN", "gho_x"),
        };
        let extended = extend_ssh_forwarding(base, Some(&token));
        assert_eq!(
            extended.args,
            vec![
                "--send-env".to_owned(),
                "GH_TOKEN".to_owned(),
                "--send-env".to_owned(),
                TOKEN_VAR.to_owned(),
            ]
        );
        assert_eq!(
            extended.env,
            EnvSpec::inherited()
                .and("GH_TOKEN", "gho_x")
                .and(TOKEN_VAR, "not-a-real-secret-token")
        );
    }
}
