//! The host's Codex login, forwarded into a workspace.
//!
//! [`super::claude`]'s job for the other agent `aid` can start, and mostly the same
//! shape: a value the host produces, checked once at the boundary, delivered through
//! devpod's environment so the secret never reaches an argv another user could read.
//! Read that module first. Where this one differs is in what codex will accept, and
//! that difference is the whole of what follows.
//!
//! # Why a file is written, and what is left out of it
//!
//! Claude Code authenticates from `CLAUDE_CODE_OAUTH_TOKEN` with an otherwise empty
//! `$HOME`, so forwarding one variable is the whole job there. Codex has no such
//! variable. Measured against codex-cli 0.154.0 it authenticates from
//! `$CODEX_HOME/auth.json` and nothing else, and `codex login --with-access-token`
//! is not a way in either: it wants an agent identity JWT and refuses an OAuth
//! access token outright, with "agent identity JWT payload is not valid JSON".
//!
//! So the container gets a file. What makes that acceptable is *which* file. The
//! host's is never copied. The one written in its place is built here and holds
//! `tokens.id_token`, `tokens.access_token` and `tokens.account_id` with an **empty
//! `tokens.refresh_token`**. Measured against the same version, codex authenticates
//! from exactly that, and fails when the id token is left out too, so it is the
//! smallest file that works.
//!
//! The refresh token is the point of the omission. It is what a container could use
//! to rotate the host's own ChatGPT login out from under the user, and it is why
//! [`super::claude`] refuses to write `~/.claude/.credentials.json` into a container
//! at all. Claude Code's variable let that module avoid a file entirely; codex
//! leaves no such option, so the next best thing is a file that cannot mint another
//! credential. What lands on the container's disk expires in hours and renews only
//! by another launch.
//!
//! The empty refresh token doubles as the mark of who wrote the file. A `codex
//! login` run *inside* a workspace writes a real one, so the payload in
//! [`crate::flows::launch`] overwrites only a file that is absent or one of dl's
//! own. A login somebody made in there by hand is left alone, which is the rule
//! [`crate::flows::provision`] already keeps for a mounted Claude config.
//!
//! # Why only the sessions dl opens
//!
//! Unchanged from [`super::claude`], and for its reasons: this rides `--send-env` on
//! the sessions devlaunch itself opens, so a stranger's `postCreateCommand` never
//! runs with the user's Codex login in reach, nothing is persisted in devpod's
//! workspace configuration, and every session rebuilds the file from the host's own,
//! which is what makes refresh-on-start free for a token measured in hours.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::gh::{Forwarding, forwarding_disabled};
use crate::runner::EnvSpec;

/// The variable set inside the container, holding the file to write.
///
/// A `DEVLAUNCH_` name because there is no codex-documented one to borrow: this
/// carries devlaunch's own redacted `auth.json`, not a credential codex has a
/// spelling for. `CODEX_ACCESS_TOKEN` was the first guess and the wrong one. Codex
/// reads no such variable, and the `--with-access-token` flag its `--help` names it
/// beside wants a different kind of token entirely (see the [module note](self)).
pub(crate) const AUTH_VAR: &str = "DEVLAUNCH_CODEX_AUTH";

/// The spelling of the redacted refresh token, as this module writes it and as the
/// container-side payload greps for it.
///
/// One constant because it is one fact read from two sides. The host writes compact
/// JSON through `serde_json`, so the byte sequence is predictable, and
/// [`crate::flows::launch`] tests the container's file against it to tell a file dl
/// wrote from a login somebody made by hand. Two spellings of this would eventually
/// clobber a real login.
pub(crate) const REDACTED_REFRESH: &str = r#""refresh_token":"""#;

/// Set this to opt a machine out of forwarding the Codex login entirely.
///
/// Beside [`super::claude::DISABLE_VAR`] and parsed by the same
/// [`forwarding_disabled`], so `=0` cannot mean one thing in one variable and
/// something else in the other. Separate from claude's because the two logins are
/// separate: a host may well want to lend one agent its account and not the other.
pub(crate) const DISABLE_VAR: &str = "DEVLAUNCH_NO_CODEX_TOKEN";

/// `$CODEX_HOME`: Codex's own name for where its configuration lives.
///
/// Honoured rather than set, exactly as [`super::claude`] honours
/// `$CLAUDE_CONFIG_DIR` and for the identical reason: Codex reads this before
/// `$HOME/.codex`, so a host that has moved its configuration has moved the
/// credential too, and reading `$HOME/.codex` regardless would inspect a directory
/// Codex does not use. That asymmetry cost claude a bug report; there is no reason
/// to re-earn it here. The container side honours it too, in shell, as
/// `${CODEX_HOME:-$HOME/.codex}`.
pub(crate) const CONFIG_DIR_VAR: &str = "CODEX_HOME";

/// The configuration directory's name under `$HOME`, when the variable says nothing.
const CONFIG_RELPATH: &str = ".codex";

/// The credential file's name inside whichever directory the above resolves to.
pub(crate) const AUTH_FILENAME: &str = "auth.json";

/// The keys read from the host's file, and the keys written to the container's.
///
/// `refresh_token` appears here because it is written *empty*, never read. Every
/// other key in the host's file is dropped rather than carried: an unknown key is
/// not something to forward on the chance codex wants it.
const TOKENS_KEY: &str = "tokens";
const ID_TOKEN_KEY: &str = "id_token";
const ACCESS_TOKEN_KEY: &str = "access_token";
const REFRESH_TOKEN_KEY: &str = "refresh_token";
const ACCOUNT_ID_KEY: &str = "account_id";
const AUTH_MODE_KEY: &str = "auth_mode";
/// When the host last refreshed, carried so the container does not immediately try
/// to refresh for itself.
///
/// Dropping it was a real bug and worth naming: codex reads this to decide whether
/// the access token is due for renewal, and a file without it is a file due for
/// renewal *now*. With an empty refresh token that renewal fails, so a workspace got
/// `Failed to refresh token: 400 Bad Request: Invalid 'refresh_token': empty string`
/// instead of a working agent. Carried as it is found, so a host whose token really
/// is old still hands over a file codex will decline to use rather than one it will
/// wrongly trust.
const LAST_REFRESH_KEY: &str = "last_refresh";

/// The `auth.json` a workspace is given: the host's tokens, minus the one that can
/// mint more.
///
/// Its own type for the reason [`super::claude::Token`] has one: the check belongs
/// at the boundary, once, rather than at each place that forwards it. Holding the
/// finished JSON rather than the fields keeps the redaction in one place too, so
/// there is no route to a session that assembles the file itself and forgets which
/// key to blank.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Credential(String);

/// Redacted, deliberately, exactly as [`super::claude::Token`] and
/// [`super::gh::Token`] are. A credential must not reach a log line by any route,
/// and a derived `Debug` is the route nobody writes on purpose: [`CredentialLookup`]
/// and [`Forwarding`] both derive one and both hold this.
impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Credential(<redacted>)")
    }
}

impl Credential {
    /// The file text, for the one caller that puts it in a session's environment.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// A token, if `raw` is one.
///
/// Not exposed: the tokens are only ever read on the way into a [`Credential`], and
/// the check exists so that a truncated or hand-edited `auth.json` reads as
/// unreadable rather than producing a file that fails to authenticate for reasons
/// nobody can see. Both are JWTs in practice, so the flat-ASCII rule
/// [`super::claude::Token::parse`] applies with `~` added for the base64url variants
/// some issuers use.
fn token(raw: &serde_json::Value) -> Option<&str> {
    let raw = raw.as_str()?;
    let flat = !raw.is_empty()
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '~'));
    flat.then_some(raw)
}

/// The values on the host this decision reads.
///
/// Parameters rather than reads of the process environment, exactly as
/// [`super::claude::HostEnv`] and [`super::gh::HostEnv`] are, so the decision is a
/// function of its inputs and a test can state the host it means instead of mutating
/// an environment the whole binary shares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HostEnv {
    /// `DEVLAUNCH_NO_CODEX_TOKEN`.
    pub(crate) disable: Option<String>,
    /// `DEVLAUNCH_CODEX_AUTH`, read before the file on disk.
    ///
    /// The hatch that lets a `dl` running *inside* a workspace forward what it was
    /// handed, which is the courtesy [`super::claude::HostEnv`] extends through its
    /// own token field.
    pub(crate) auth: Option<String>,
    /// `CODEX_HOME`, read *after* the inherited file and *before* `$HOME`.
    ///
    /// Below the hatch because both are ambient and the hatch has to keep winning.
    /// Above `$HOME` because it is what Codex itself prefers.
    ///
    /// An `OsString` and not a `String` for the reason
    /// [`super::claude::HostEnv`]'s `config_dir` gives: this one is a **path**, and
    /// [`crate::osext::env_str`]'s lossy decode would put U+FFFD where a non-UTF-8
    /// byte was, fail to open, and report [`NoCredential::NotLoggedIn`] on a host
    /// holding a perfectly good login.
    pub(crate) config_dir: Option<OsString>,
}

impl HostEnv {
    /// What this process's environment says.
    pub(crate) fn from_process() -> Self {
        Self {
            disable: crate::osext::env_str(DISABLE_VAR),
            auth: crate::osext::env_str(AUTH_VAR),
            config_dir: std::env::var_os(CONFIG_DIR_VAR),
        }
    }
}

/// Why there is no Codex login to forward.
///
/// [`super::claude::NoToken`] minus the two profile arms, which have no counterpart:
/// `aid` has no `--codex-profile`, so there is no named account whose absence must
/// stop a launch. Only [`Self::Unreadable`] is worth a warning. Opting out is a
/// choice, and a host that never logged in is a fact about the host, but an
/// `auth.json` that is there and yields nothing is a problem the user can fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NoCredential {
    /// `DEVLAUNCH_NO_CODEX_TOKEN` is set.
    OptedOut,
    /// No `auth.json`. A host that has not run `codex login`.
    NotLoggedIn,
    /// The file is there and did not yield the two tokens codex needs.
    ///
    /// Carries the path, never the contents: a malformed credential file is still a
    /// credential file, and quoting it into a diagnostic is how a secret reaches a
    /// log. An API-key host lands here too, and deliberately: an `auth.json` whose
    /// `auth_mode` is `apikey` has no `tokens` object at all. That reads as
    /// "unreadable" while meaning "logged in a way this cannot forward", and the
    /// consequence is the same either way, which is what the message says.
    Unreadable(String),
}

/// The host's Codex login, or why there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CredentialLookup {
    Found(Credential),
    Missing(NoCredential),
}

/// The `auth.json` to give a workspace, or why there is none to give.
///
/// `home` is a parameter and not a read of `$HOME` so a test can state the host it
/// means. No subprocess and no timing span, exactly as
/// [`super::claude::resolve_token`] has none: this is one file read, and there is no
/// CLI to ask.
pub(crate) fn resolve_credential(home: Option<&Path>, host: &HostEnv) -> CredentialLookup {
    if forwarding_disabled(host.disable.as_deref()) {
        return CredentialLookup::Missing(NoCredential::OptedOut);
    }
    // The ambient hatch, above the file for the reason `HostEnv::auth` documents. It
    // is redacted again rather than trusted: a nested `dl` must not be able to widen
    // what travels by exporting a file with a live refresh token in it.
    if let Some(inherited) = host.auth.as_deref().and_then(redact) {
        return CredentialLookup::Found(inherited);
    }
    let Some(dir) = config_dir(home, host) else {
        return CredentialLookup::Missing(NoCredential::NotLoggedIn);
    };
    let path = dir.join(AUTH_FILENAME);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return CredentialLookup::Missing(NoCredential::NotLoggedIn);
    };
    match redact(&text) {
        Some(credential) => CredentialLookup::Found(credential),
        None => CredentialLookup::Missing(NoCredential::Unreadable(path.display().to_string())),
    }
}

/// Which configuration directory the credential is read from.
///
/// `$CODEX_HOME` *replaces* the default rather than being tried ahead of it, which
/// is [`super::claude`]'s rule and matters for the same reason: a host with two
/// logins that fell back to `$HOME/.codex` when the named directory held no
/// credential would forward the wrong account, which is a worse defect than ignoring
/// the variable and harder to see. An empty value counts as unset, matching
/// [`crate::domain::xdg`]'s rule and what a shell exporting a bare variable means.
fn config_dir(home: Option<&Path>, host: &HostEnv) -> Option<PathBuf> {
    match host.config_dir.as_deref() {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => home.map(|home| home.join(CONFIG_RELPATH)),
    }
}

/// Build the file a workspace gets from the text of the file the host has.
///
/// Deliberately not a `Deserialize` round trip of the whole document, for
/// [`super::claude`]'s reason: the file belongs to Codex and gains keys on its own
/// schedule. Reading the values that are wanted and *composing a new document* is
/// also what makes the redaction total rather than a deletion somebody has to
/// remember. There is no path through here that copies the host's file, so there is
/// no path that carries a key nobody checked.
///
/// `None` unless both tokens are there and both are token-shaped. Either one missing
/// is a file codex will not authenticate from, and the caller reports that rather
/// than giving a container a file that cannot work.
fn redact(text: &str) -> Option<Credential> {
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    let tokens = parsed.get(TOKENS_KEY)?;
    let id_token = token(tokens.get(ID_TOKEN_KEY)?)?;
    let access_token = token(tokens.get(ACCESS_TOKEN_KEY)?)?;
    let mut carried = serde_json::Map::new();
    carried.insert(ID_TOKEN_KEY.to_owned(), id_token.into());
    carried.insert(ACCESS_TOKEN_KEY.to_owned(), access_token.into());
    // The omission this module exists for. Empty rather than absent, because it is
    // also the mark that says dl wrote this file.
    carried.insert(REFRESH_TOKEN_KEY.to_owned(), "".into());
    // Carried when it is a string and dropped otherwise: codex authenticates without
    // it, so a host whose file has none is not a host with no login.
    if let Some(account) = tokens.get(ACCOUNT_ID_KEY).and_then(|id| id.as_str()) {
        carried.insert(ACCOUNT_ID_KEY.to_owned(), account.into());
    }
    let mut written = serde_json::Map::new();
    written.insert(
        AUTH_MODE_KEY.to_owned(),
        parsed
            .get(AUTH_MODE_KEY)
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    written.insert(TOKENS_KEY.to_owned(), carried.into());
    written.insert(
        LAST_REFRESH_KEY.to_owned(),
        parsed
            .get(LAST_REFRESH_KEY)
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    let text = serde_json::to_string(&serde_json::Value::Object(written)).ok()?;
    // The container side greps for this, so a change in how serde spells an empty
    // string has to fail here rather than in a workspace.
    debug_assert!(text.contains(REDACTED_REFRESH), "{text}");
    Some(Credential(text))
}

/// Add the Codex login to the flags and environment a session is opened with.
///
/// Extends rather than replaces, for [`super::claude::extend_ssh_forwarding`]'s
/// reason: a session already carries gh's forwarding and possibly claude's, and all
/// of them are independent. `--send-env` names the variable in argv and the value
/// travels in the environment, which is the whole discipline this shares with the
/// other two: only the name is ever readable by another user.
pub(crate) fn extend_ssh_forwarding(base: Forwarding, auth: Option<&Credential>) -> Forwarding {
    let Some(auth) = auth else {
        return base;
    };
    let Forwarding { mut args, env } = base;
    args.push("--send-env".to_owned());
    args.push(AUTH_VAR.to_owned());
    Forwarding {
        args,
        env: inherited(env).and(AUTH_VAR, auth.as_str()),
    }
}

/// The same, for the OpenSSH transport, whose flags are bare variable names.
///
/// Both transports have to carry this for
/// [`super::claude::extend_openssh_forwarding`]'s reason, and `aid --codex` is
/// precisely the route that proves it: `dl <ws> -- codex` goes to OpenSSH rather
/// than `devpod ssh`, because devpod never requests a pty and codex refuses a
/// non-terminal stdin. Forwarding on one and not the other would leave the single
/// most likely codex command in a workspace without a login.
pub(crate) fn extend_openssh_forwarding(base: Forwarding, auth: Option<&Credential>) -> Forwarding {
    let Some(auth) = auth else {
        return base;
    };
    let Forwarding { mut args, env } = base;
    args.push(AUTH_VAR.to_owned());
    Forwarding {
        args,
        env: inherited(env).and(AUTH_VAR, auth.as_str()),
    }
}

/// A base environment that is a parent environment.
///
/// [`super::claude`] carries the same helper for the same reason:
/// `Forwarding::default()` holds `EnvSpec::default()`, which is already the
/// inherited one, and this exists so that reading the code does not require knowing
/// that.
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

    /// The shape of a real `auth.json`, with all three tokens present, so a test
    /// that asserts what travels is asserting against the file the host actually
    /// has.
    fn auth_json(access: &str) -> String {
        format!(
            r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{"id_token":"id.tok.en","access_token":"{access}","refresh_token":"refresh.tok.en","account_id":"acct"}},"last_refresh":"2026-09-10T00:00:00Z"}}"#
        )
    }

    /// A host whose `~/.codex/auth.json` holds `access`.
    fn logged_in(access: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("a scratch home");
        let dir = home.path().join(CONFIG_RELPATH);
        std::fs::create_dir_all(&dir).expect("a scratch config dir");
        std::fs::write(dir.join(AUTH_FILENAME), auth_json(access)).expect("a scratch credential");
        home
    }

    fn found(home: Option<&Path>, host: &HostEnv) -> Credential {
        match resolve_credential(home, host) {
            CredentialLookup::Found(credential) => credential,
            other => panic!("expected a credential, got {other:?}"),
        }
    }

    /// The two tokens codex needs, measured against codex-cli 0.154.0: it
    /// authenticates from these and fails when the id token is left out.
    #[test]
    fn both_tokens_codex_needs_are_carried() {
        let home = logged_in("acc.ess.token");
        let file = found(Some(home.path()), &HostEnv::default());
        assert!(file.as_str().contains("acc.ess.token"), "{file:?}");
        assert!(file.as_str().contains("id.tok.en"), "{file:?}");
        assert!(file.as_str().contains("acct"), "{file:?}");
    }

    /// The point of the module, asserted rather than described.
    #[test]
    fn the_refresh_token_never_leaves_the_host() {
        let home = logged_in("acc.ess.token");
        let file = found(Some(home.path()), &HostEnv::default());
        assert!(
            !file.as_str().contains("refresh.tok.en"),
            "the refresh token reached a workspace: {file:?}"
        );
        assert!(file.as_str().contains(REDACTED_REFRESH), "{file:?}");
        let forwarded = extend_ssh_forwarding(Forwarding::default(), Some(&file));
        assert!(
            !format!("{:?}", forwarded.env).contains("refresh.tok.en"),
            "the refresh token reached a session: {forwarded:?}"
        );
    }

    /// Carried because codex reads it to decide whether to refresh, and a file
    /// without it is one codex tries to refresh immediately, which cannot work
    /// against an empty refresh token.
    #[test]
    fn the_last_refresh_time_is_carried() {
        let home = logged_in("acc.ess.token");
        let file = found(Some(home.path()), &HostEnv::default());
        assert!(
            file.as_str().contains("2026-09-10T00:00:00Z"),
            "a file with no last_refresh is refreshed at once: {file:?}"
        );
    }

    /// Composed, not filtered: a key the host's file carries and this module does
    /// not read must not appear in what travels, whatever it is.
    #[test]
    fn nothing_unread_is_carried_along() {
        let home = tempfile::tempdir().expect("a scratch home");
        let dir = home.path().join(CONFIG_RELPATH);
        std::fs::create_dir_all(&dir).expect("a scratch config dir");
        std::fs::write(
            dir.join(AUTH_FILENAME),
            r#"{"auth_mode":"chatgpt","tokens":{"id_token":"id.tok.en","access_token":"acc.ess","refresh_token":"r","a_key_from_the_future":"surprise"},"another":"surprise"}"#,
        )
        .expect("a scratch credential");
        let file = found(Some(home.path()), &HostEnv::default());
        assert!(!file.as_str().contains("surprise"), "{file:?}");
    }

    /// A file codex cannot authenticate from is reported, not written.
    #[test]
    fn a_file_missing_the_id_token_is_unreadable() {
        let home = tempfile::tempdir().expect("a scratch home");
        let dir = home.path().join(CONFIG_RELPATH);
        std::fs::create_dir_all(&dir).expect("a scratch config dir");
        std::fs::write(
            dir.join(AUTH_FILENAME),
            r#"{"tokens":{"access_token":"acc.ess","refresh_token":"r"}}"#,
        )
        .expect("a scratch credential");
        assert!(matches!(
            resolve_credential(Some(home.path()), &HostEnv::default()),
            CredentialLookup::Missing(NoCredential::Unreadable(_))
        ));
    }

    #[test]
    fn a_host_that_never_logged_in_has_nothing_to_forward() {
        let home = tempfile::tempdir().expect("a scratch home");
        assert_eq!(
            resolve_credential(Some(home.path()), &HostEnv::default()),
            CredentialLookup::Missing(NoCredential::NotLoggedIn)
        );
    }

    /// An API-key host lands in `Unreadable` by design; the arm's doc says so, and
    /// this holds that claim to the file such a host actually has.
    #[test]
    fn an_api_key_host_has_no_oauth_login_to_forward() {
        let home = tempfile::tempdir().expect("a scratch home");
        let dir = home.path().join(CONFIG_RELPATH);
        std::fs::create_dir_all(&dir).expect("a scratch config dir");
        std::fs::write(
            dir.join(AUTH_FILENAME),
            r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-not-a-forwarded-thing"}"#,
        )
        .expect("a scratch credential");
        let CredentialLookup::Missing(NoCredential::Unreadable(path)) =
            resolve_credential(Some(home.path()), &HostEnv::default())
        else {
            panic!("an api-key host has no oauth login");
        };
        assert!(path.ends_with(AUTH_FILENAME), "{path}");
    }

    #[test]
    fn the_opt_out_beats_a_logged_in_host() {
        let home = logged_in("acc.ess.token");
        let host = HostEnv {
            disable: Some("1".to_owned()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_credential(Some(home.path()), &host),
            CredentialLookup::Missing(NoCredential::OptedOut)
        );
    }

    #[test]
    fn an_inherited_file_beats_the_one_on_disk() {
        let home = logged_in("from.the.file");
        let host = HostEnv {
            auth: Some(auth_json("from.the.environment")),
            ..HostEnv::default()
        };
        let file = found(Some(home.path()), &host);
        assert!(file.as_str().contains("from.the.environment"), "{file:?}");
    }

    /// A nested `dl` must not be able to widen what travels by exporting a file with
    /// a live refresh token in it.
    #[test]
    fn an_inherited_file_is_redacted_again() {
        let host = HostEnv {
            auth: Some(auth_json("from.the.environment")),
            ..HostEnv::default()
        };
        let file = found(None, &host);
        assert!(!file.as_str().contains("refresh.tok.en"), "{file:?}");
    }

    /// `$CODEX_HOME` replaces the default rather than being tried ahead of it, so a
    /// host with two logins cannot silently forward the wrong one.
    #[test]
    fn codex_home_replaces_the_default_rather_than_preceding_it() {
        let home = logged_in("the.default.login");
        let moved = tempfile::tempdir().expect("a scratch config dir");
        let host = HostEnv {
            config_dir: Some(moved.path().as_os_str().to_owned()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_credential(Some(home.path()), &host),
            CredentialLookup::Missing(NoCredential::NotLoggedIn),
            "the default login was forwarded from a host that had moved its config"
        );
    }

    #[test]
    fn an_empty_codex_home_counts_as_unset() {
        let home = logged_in("the.default.login");
        let host = HostEnv {
            config_dir: Some(OsString::new()),
            ..HostEnv::default()
        };
        let file = found(Some(home.path()), &host);
        assert!(file.as_str().contains("the.default.login"), "{file:?}");
    }

    #[test]
    fn a_credential_is_redacted_in_debug() {
        let home = logged_in("acc.ess.token");
        let file = found(Some(home.path()), &HostEnv::default());
        assert_eq!(format!("{file:?}"), "Credential(<redacted>)");
    }

    #[test]
    fn nothing_is_forwarded_without_a_credential() {
        let base = Forwarding::default();
        assert_eq!(extend_ssh_forwarding(base.clone(), None), base);
        assert_eq!(extend_openssh_forwarding(base.clone(), None), base);
    }

    #[test]
    fn each_transport_spells_the_flag_its_own_way() {
        let home = logged_in("acc.ess.token");
        let file = found(Some(home.path()), &HostEnv::default());
        let devpod = extend_ssh_forwarding(Forwarding::default(), Some(&file));
        assert_eq!(
            devpod.args,
            vec!["--send-env".to_owned(), AUTH_VAR.to_owned()]
        );
        let openssh = extend_openssh_forwarding(Forwarding::default(), Some(&file));
        assert_eq!(openssh.args, vec![AUTH_VAR.to_owned()]);
    }

    /// The two disable variables are parsed by one function, so `=0` cannot mean
    /// opted out here and opted in there.
    #[test]
    fn the_opt_out_reads_the_same_values_claudes_does() {
        for falsey in ["", "0", "false", "no"] {
            let host = HostEnv {
                disable: Some(falsey.to_owned()),
                ..HostEnv::default()
            };
            let home = logged_in("acc.ess.token");
            assert!(
                matches!(
                    resolve_credential(Some(home.path()), &host),
                    CredentialLookup::Found(_)
                ),
                "{falsey:?} opted a host out"
            );
        }
    }
}
