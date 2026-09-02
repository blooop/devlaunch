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
//! # Which credential, on a host that has moved it
//!
//! `$CLAUDE_CONFIG_DIR` is honoured here as Claude Code honours it, and as the
//! container-side probe in [`crate::flows::provision`] has always honoured it. The
//! host side reading `$HOME/.claude` regardless was an asymmetry with one visible
//! symptom: a host that had moved its configuration reported [`NoToken::NotLoggedIn`]
//! while holding a perfectly good login.
//!
//! The order is opt-out, then any exported `CLAUDE_CODE_OAUTH_TOKEN`, then
//! `$CLAUDE_CONFIG_DIR`, then `$HOME/.claude`. The exported token stays above the
//! variable because both are ambient and that hatch is what lets a `dl` inside a
//! workspace forward what it was handed; see [`config_dir`] for why the variable
//! *replaces* the default rather than being tried ahead of it.
//!
//! It also means a session devlaunch did not open — `devpod ssh` by hand, VS Code
//! through `dl <ws> code` — does not get it. Those already have the host's real
//! `claude` available to them by other means, and widening this to cover them
//! would mean widening it to cover `postCreateCommand` too.

use std::path::{Path, PathBuf};

use super::gh::{Forwarding, forwarding_disabled};
use crate::runner::EnvSpec;

/// The variable set inside the container. Claude Code consults it before any
/// credential file, which is exactly why the file is left alone: see the
/// [module note](self) and [`super::super::flows::provision::ClaudeConfig`].
pub(crate) const TOKEN_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// Set this to opt a machine out of forwarding the Claude login entirely.
pub(crate) const DISABLE_VAR: &str = "DEVLAUNCH_NO_CLAUDE_TOKEN";

/// `$CLAUDE_CONFIG_DIR`: Claude Code's own name for where its configuration lives.
///
/// Honoured rather than set, the way [`super::ssh::CONFIG_VAR`] is: Claude Code reads
/// this before `$HOME/.claude`, so a host that has moved its configuration has moved
/// the credential too, and reading `$HOME/.claude` regardless would inspect a
/// directory Claude Code does not use. The container side of this has always honoured
/// it -- see [`crate::flows::provision`]'s `claude_config_lines`, whose
/// `CLAUDE_CONFIG_DIR:-$HOME/.claude` is the same rule in shell -- and the host side
/// ignoring it was the asymmetry this closes.
const CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";

/// The configuration directory's name under `$HOME`, when the variable says nothing.
///
/// Deliberately *not* shared with [`crate::flows::provision`]'s `CLAUDE_CONFIG_RELPATH`,
/// which spells the same string about the *container*. Two machines, two facts: that one
/// is Claude Code's default inside a container devlaunch did not build, this one is a
/// path on the host devlaunch is running on. A comment in each names the other, which is
/// the right amount of coupling for a coincidence of spelling.
const CONFIG_RELPATH: &str = ".claude";

/// The credential file's name inside whichever directory the above resolves to.
const CREDENTIALS_FILENAME: &str = ".credentials.json";

/// The key the OAuth credential sits under, and the field wanted from it.
const OAUTH_KEY: &str = "claudeAiOauth";
const ACCESS_TOKEN_KEY: &str = "accessToken";

/// Claude Code's own account and state file, beside the credential in a config
/// directory. Read for a *label*, never for a credential: nothing in it is secret and
/// nothing in it is forwarded anywhere.
const ACCOUNT_FILENAME: &str = ".claude.json";

/// The object in [`ACCOUNT_FILENAME`] describing who is signed in, and the three
/// fields worth showing a person choosing between profiles.
const ACCOUNT_KEY: &str = "oauthAccount";
const EMAIL_KEY: &str = "emailAddress";
/// Not shown to anyone: an opaque id is noise in a table. Read so that two profiles
/// holding one account can be *said* to, which no pair of display fields proves --
/// two logins of one organisation share an `organizationName` and are still different
/// accounts.
const ACCOUNT_UUID_KEY: &str = "accountUuid";
const ORGANIZATION_KEY: &str = "organizationName";
const SEAT_KEY: &str = "seatTier";

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

/// A value that can be one directory component under the profiles root.
///
/// Its own type for [`Token`]'s reason: the check belongs at the boundary, once. A
/// name is joined onto a path, so anything that could climb out of the profiles root
/// or name something other than a leaf is refused rather than cleaned up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProfileName(String);

impl ProfileName {
    /// `raw` if it can be a leaf directory name worth offering, else nothing.
    ///
    /// The same flat-ASCII set [`Token::parse`] accepts, and two leading characters
    /// refused outright:
    ///
    /// - **`-`**, because a name that looks like a flag reads as one everywhere it is
    ///   later printed or passed on.
    /// - **`.`**, which covers `.` and `..` without special-casing them and settles a
    ///   disagreement three places were having. A glob of `<root>/*/` does not match a
    ///   dot-directory, so neither the shell completion nor the tool that manages the
    ///   directory lists one, while this check used to accept it: `--claude-profile
    ///   .hidden` was a profile you could launch and never see offered. A hidden
    ///   profile is a trap rather than a feature, and one rule here makes the resolver,
    ///   the listing and the completion agree.
    ///
    /// Slightly stricter than the `^[A-Za-z0-9._-]+$` the managing tool validates with,
    /// which accepts a leading dot it then never lists. Refusing by a named rule beats
    /// honouring a name nothing shows you.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let flat = !raw.is_empty()
            && !raw.starts_with(['-', '.'])
            && raw
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
        flat.then(|| Self(raw.to_owned()))
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
    /// `CLAUDE_CONFIG_DIR`, read *after* the exported token and *before* `$HOME`.
    ///
    /// Below the token because both are ambient, and the existing hatch above has to
    /// keep winning so a `dl` launched from inside a workspace still forwards what it
    /// was given. Above `$HOME` because it is what Claude Code itself prefers.
    pub(crate) config_dir: Option<String>,
    /// `--claude-profile`, if one was typed.
    ///
    /// **The one field here that is not a read of the environment**, which is what
    /// puts it above [`Self::token`]: it was typed on this command line, for this
    /// launch, and nothing ambient should beat an explicit argument. A nested `dl`
    /// naming a profile means it, and the inherited token is exactly what it is
    /// overriding. [`HostEnv::from_process`] leaves it `None` for that reason.
    pub(crate) profile: Option<String>,
}

impl HostEnv {
    /// What this process's environment says.
    pub(crate) fn from_process() -> Self {
        Self {
            disable: crate::osext::env_str(DISABLE_VAR),
            token: crate::osext::env_str(TOKEN_VAR),
            config_dir: crate::osext::env_str(CONFIG_DIR_VAR),
            // Not a read of the environment, deliberately: a profile is typed on a
            // command line and reaches this struct from the CLI, which is what puts
            // it above the ambient token. See [`Self::profile`].
            profile: None,
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
    /// A profile was named and yielded no token.
    ///
    /// Its own arm rather than [`Self::Unreadable`], because this is the one refusal
    /// that must **stop** a launch's forwarding rather than quietly leaving it
    /// unforwarded. The user named an account; forwarding the default one instead is
    /// the mistake profiles exist to prevent, and it is invisible until the day it
    /// pushes to the wrong place. Carries the name so the message can quote what was
    /// typed, which is a profile name and never a credential.
    ProfileUnreadable { name: String, path: String },
    /// A profile name that could not be a directory component.
    ///
    /// Refused at the boundary rather than sanitised, for [`Token::parse`]'s reason:
    /// `--claude-profile ../../etc` must be a refusal naming the rule, not a
    /// traversal that happens to fail later on a read.
    ProfileNotAName(String),
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
/// this is one file read, and there is no CLI to ask. Which directory that read lands
/// in is [`config_dir`]'s decision.
pub(crate) fn resolve_token(
    home: Option<&Path>,
    profiles_root: Option<&Path>,
    host: &HostEnv,
) -> TokenLookup {
    if forwarding_disabled(host.disable.as_deref()) {
        return TokenLookup::Missing(NoToken::OptedOut);
    }
    if let Some(named) = host
        .profile
        .as_deref()
        .filter(|named| *named != DEFAULT_PROFILE)
    {
        return from_profile(named, profiles_root);
    }
    if let Some(token) = host.token.as_deref().and_then(Token::parse) {
        return TokenLookup::Found(token);
    }
    let Some(dir) = config_dir(home, host) else {
        return TokenLookup::Missing(NoToken::NotLoggedIn);
    };
    let path = dir.join(CREDENTIALS_FILENAME);
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

/// The profile name that means "the login this host uses anyway".
///
/// `claude-as default` runs `claude` with no `CLAUDE_CONFIG_DIR` at all rather than
/// looking for a directory of that name, and this matches it: `--claude-profile
/// default` resolves the unnamed credential below and **never** consults
/// `<root>/default/`, even if one exists. Worth having as a word rather than as the
/// absence of a flag, because a picker needs something to select and a recalled line
/// needs a way to say "not the profile I used last time".
const DEFAULT_PROFILE: &str = "default";

/// The token a named profile holds, or the reason it holds none.
///
/// **Nothing here falls through to another credential**, and that is the whole point
/// of the feature. Two accounts on one machine is what profiles are for, so a typo
/// that silently forwarded the other one would be worse than a launch that stops: the
/// launch you can see, and the wrong account you find out about later, somewhere else.
fn from_profile(named: &str, profiles_root: Option<&Path>) -> TokenLookup {
    let Some(name) = ProfileName::parse(named) else {
        return TokenLookup::Missing(NoToken::ProfileNotAName(named.to_owned()));
    };
    // No root resolves on a machine that names no home directory and set no override.
    // A profile was still typed, so this is that refusal and not `NotLoggedIn`.
    let Some(root) = profiles_root else {
        return TokenLookup::Missing(NoToken::ProfileUnreadable {
            name: name.as_str().to_owned(),
            path: String::new(),
        });
    };
    let path = root.join(name.as_str()).join(CREDENTIALS_FILENAME);
    let unreadable = || {
        TokenLookup::Missing(NoToken::ProfileUnreadable {
            name: name.as_str().to_owned(),
            path: path.display().to_string(),
        })
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return unreadable();
    };
    match token_from_credentials(&text) {
        Some(token) => TokenLookup::Found(token),
        None => unreadable(),
    }
}

/// The directory this host keeps its Claude configuration in, if it names one.
///
/// `$CLAUDE_CONFIG_DIR` when it names something, `$HOME/.claude` otherwise, and
/// nothing at all on a machine with no home directory and no variable -- which is a
/// real state, since `dl` runs with `XDG_CACHE_HOME` set and no home.
///
/// **The variable replaces the default rather than being tried before it.** Claude Code
/// does not fall back from `$CLAUDE_CONFIG_DIR` to `$HOME/.claude`, and a fallback here
/// would forward a credential out of a directory Claude Code is not reading -- the same
/// defect as ignoring the variable, and harder to see, because it only shows up on a
/// host with two logins. An empty value counts as unset, matching
/// [`crate::domain::xdg`]'s rule and what a shell exporting a bare variable means.
fn config_dir(home: Option<&Path>, host: &HostEnv) -> Option<PathBuf> {
    match host.config_dir.as_deref() {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => home.map(|home| home.join(CONFIG_RELPATH)),
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

/// Who a Claude config directory is signed in as.
///
/// **A label, not a credential.** The whole reason this type exists is that a profile's
/// directory name is chosen by a person and verified by nothing: a profile called
/// `work` holding a personal login is indistinguishable from a correct one until
/// something reads the account out. Every field is optional because the file is Claude
/// Code's, gains keys on its own schedule, and may be absent entirely in a profile
/// that has been created but never logged in to.
///
/// Not redacted, unlike [`Token`], and deliberately: an email address is identity
/// rather than a secret, it is what makes the label worth printing, and it never
/// leaves the host. Nothing here is ever forwarded into a container.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    pub email: Option<String>,
    pub organization: Option<String>,
    pub seat_tier: Option<String>,
    /// The account's own id, for telling two profiles of one account apart from two
    /// profiles that merely look alike. Never displayed.
    pub account_uuid: Option<String>,
}

impl Account {
    /// Whether anything **worth showing** was learned, so a caller can tell "signed in
    /// as somebody I cannot name" from "signed in as nobody".
    ///
    /// Deliberately blind to [`Self::account_uuid`]: an id alone renders as an empty
    /// column, so a file carrying only that is no better than a file carrying nothing,
    /// and claiming two such profiles are "the same account" would be a claim about
    /// two blanks.
    pub fn is_empty(&self) -> bool {
        self.email.is_none() && self.organization.is_none() && self.seat_tier.is_none()
    }
}

/// The account a config directory names, if its state file says.
///
/// `None` covers every ordinary absence at once, because they are one answer to a
/// caller drawing a table: no state file (a profile made and never logged in to), a
/// file that is not JSON, or a file with no `oauthAccount`. Deliberately not a
/// `Deserialize` struct over the whole file, for [`token_from_credentials`]'s reason:
/// the file belongs to Claude Code and has 70-odd keys this does not read.
pub(crate) fn account_at(config_dir: &Path) -> Option<Account> {
    let text = std::fs::read_to_string(config_dir.join(ACCOUNT_FILENAME)).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
    let account = parsed.get(ACCOUNT_KEY)?;
    let field = |key: &str| {
        account
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let found = Account {
        email: field(EMAIL_KEY),
        organization: field(ORGANIZATION_KEY),
        seat_tier: field(SEAT_KEY),
        account_uuid: field(ACCOUNT_UUID_KEY),
    };
    (!found.is_empty()).then_some(found)
}

/// The config directory the *unnamed* login resolves to on this machine.
///
/// `$CLAUDE_CONFIG_DIR`, else `$HOME/.claude`, else nothing on a host that names
/// neither, which is a real state: `dl` runs with `XDG_CACHE_HOME` set and no home.
///
/// Reads the process environment, unlike everything else here that takes a
/// [`HostEnv`], and the exception is the caller: a listing is a read-only command with
/// no `Host` to hand, and the alternative is exporting `HostEnv` and `config_dir` so a
/// binary can rebuild an answer this module already knows.
pub(crate) fn unnamed_config_dir_from_process() -> Option<PathBuf> {
    config_dir(
        crate::osext::home_dir().as_deref(),
        &HostEnv::from_process(),
    )
}

/// The word a listing offers for the unnamed credential.
///
/// A reader of [`DEFAULT_PROFILE`] for the one test that diffs it against
/// [`crate::flows::claude_profiles::DEFAULT_PROFILE`]. Test-only on purpose: the
/// listing spells its own row name and this exists so the two cannot drift, not so
/// anything reads the name through here at runtime.
#[cfg(test)]
pub(crate) fn default_profile_name() -> &'static str {
    DEFAULT_PROFILE
}

/// Whether a directory name is one a listing should offer.
///
/// The listing and the launch must not disagree about what a profile is, so this is
/// asked of the same [`ProfileName`] check the launch applies, plus the one exclusion
/// only a listing needs: `default` is a name the resolver answers for **without**
/// consulting a directory, so a directory of that name is not a profile and offering
/// it would offer a launch that ignores it.
pub(crate) fn profile_name_is_offerable(name: &str) -> bool {
    name != DEFAULT_PROFILE && ProfileName::parse(name).is_some()
}

/// Whether a config directory holds a credential at all.
///
/// The distinction a listing needs and [`resolve_token`] does not: a profile directory
/// that exists with no credential in it is the ordinary state right after something
/// created it, and saying "not logged in" is more use than saying nothing.
pub(crate) fn has_credential(config_dir: &Path) -> bool {
    config_dir.join(CREDENTIALS_FILENAME).is_file()
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
        let found = resolve_token(Some(home.path()), None, &HostEnv::default());
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
            resolve_token(Some(home.path()), None, &host),
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
                    resolve_token(Some(home.path()), None, &host),
                    TokenLookup::Found(_)
                ),
                "{falsey:?}"
            );
        }
    }

    /// A credential file at an arbitrary directory, as `$CLAUDE_CONFIG_DIR` would name.
    fn credential_dir_holding(token: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a scratch config dir");
        std::fs::write(
            dir.path().join(CREDENTIALS_FILENAME),
            format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}"}}}}"#),
        )
        .expect("a credential");
        dir
    }

    /// A profiles root holding one named profile whose credential carries `token`.
    fn profiles_root_with(name: &str, token: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("a scratch profiles root");
        let dir = root.path().join(name);
        std::fs::create_dir_all(&dir).expect("a profile dir");
        std::fs::write(
            dir.join(CREDENTIALS_FILENAME),
            format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}"}}}}"#),
        )
        .expect("a credential");
        root
    }

    #[test]
    fn a_named_profile_is_read_from_its_own_directory() {
        let root = profiles_root_with("work", "not-a-real-work-token");
        let host = HostEnv {
            profile: Some("work".to_owned()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(None, Some(root.path()), &host),
            TokenLookup::Found(Token("not-a-real-work-token".to_owned()))
        );
    }

    #[test]
    fn the_default_profile_names_the_unnamed_credential() {
        // `claude-as default` runs claude with no CLAUDE_CONFIG_DIR rather than looking
        // for a directory called default, and this matches it. A picker needs a word for
        // "the ordinary login", and a recalled line needs a way to say "not the profile
        // I used last time".
        let home = logged_in("not-a-real-home-token");
        let root = profiles_root_with("default", "not-a-real-directory-token");
        let host = HostEnv {
            profile: Some("default".to_owned()),
            ..HostEnv::default()
        };
        // The home credential, and emphatically not `<root>/default/`, which exists
        // here precisely so the test can tell the two apart.
        assert_eq!(
            resolve_token(Some(home.path()), Some(root.path()), &host),
            TokenLookup::Found(Token("not-a-real-home-token".to_owned()))
        );
    }

    #[test]
    fn the_default_profile_still_honours_the_config_dir_and_the_opt_out() {
        // It resolves the unnamed credential, so it picks up everything that decides
        // which one that is rather than jumping straight to `$HOME/.claude`.
        let moved = credential_dir_holding("not-a-real-moved-token");
        let host = HostEnv {
            profile: Some("default".to_owned()),
            config_dir: Some(moved.path().display().to_string()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(None, None, &host),
            TokenLookup::Found(Token("not-a-real-moved-token".to_owned()))
        );
        let opted_out = HostEnv {
            disable: Some("1".to_owned()),
            profile: Some("default".to_owned()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(None, None, &opted_out),
            TokenLookup::Missing(NoToken::OptedOut)
        );
    }

    #[test]
    fn a_named_profile_that_yields_nothing_refuses_rather_than_falling_back() {
        // The most important test in the feature. Two accounts on one machine is what
        // profiles are for, so a typo must stop the launch rather than forward the
        // other account: the launch you see, the wrong account you find out about
        // later and somewhere else.
        let home = logged_in("not-a-real-home-token");
        let root = profiles_root_with("work", "not-a-real-work-token");
        for named in ["typo", "work2"] {
            let host = HostEnv {
                profile: Some(named.to_owned()),
                ..HostEnv::default()
            };
            let lookup = resolve_token(Some(home.path()), Some(root.path()), &host);
            assert!(
                matches!(
                    &lookup,
                    TokenLookup::Missing(NoToken::ProfileUnreadable { name, .. }) if name == named
                ),
                "{named}: {lookup:?}"
            );
        }
    }

    #[test]
    fn a_profile_whose_credential_is_junk_refuses_under_its_own_name() {
        // Distinguished from `Unreadable`, which is the same file failing on the
        // unnamed path: a profile was asked for, so the refusal says which one.
        let root = tempfile::tempdir().expect("a scratch profiles root");
        let dir = root.path().join("work");
        std::fs::create_dir_all(&dir).expect("a profile dir");
        std::fs::write(dir.join(CREDENTIALS_FILENAME), "not json at all").expect("a file");
        let host = HostEnv {
            profile: Some("work".to_owned()),
            ..HostEnv::default()
        };
        assert!(matches!(
            resolve_token(None, Some(root.path()), &host),
            TokenLookup::Missing(NoToken::ProfileUnreadable { .. })
        ));
    }

    #[test]
    fn a_named_profile_beats_an_inherited_token_and_the_config_dir() {
        // Typed on this command line, for this launch, so nothing ambient beats it.
        // A nested `dl` naming a profile means it, and the inherited token is exactly
        // what it is overriding.
        let root = profiles_root_with("work", "not-a-real-work-token");
        let moved = credential_dir_holding("not-a-real-moved-token");
        let host = HostEnv {
            token: Some("not-a-real-exported-token".to_owned()),
            config_dir: Some(moved.path().display().to_string()),
            profile: Some("work".to_owned()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(None, Some(root.path()), &host),
            TokenLookup::Found(Token("not-a-real-work-token".to_owned()))
        );
    }

    #[test]
    fn the_opt_out_is_read_before_the_profile_is() {
        // Set, therefore meant: a machine that has opted out has no account to choose,
        // so the opt-out stays first even against an explicit argument.
        let root = profiles_root_with("work", "not-a-real-work-token");
        let host = HostEnv {
            disable: Some("1".to_owned()),
            profile: Some("work".to_owned()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(None, Some(root.path()), &host),
            TokenLookup::Missing(NoToken::OptedOut)
        );
    }

    #[test]
    fn a_profile_name_that_is_not_a_leaf_is_refused_by_name() {
        // Refused at the boundary, and no read is attempted: the point is that a name
        // which could climb out of the profiles root never reaches a path join.
        let root = profiles_root_with("work", "not-a-real-work-token");
        for named in [
            "",
            ".",
            "..",
            "../..",
            "a/b",
            "/etc",
            "..\\windows",
            "-flag",
            // A dot-directory is refused rather than being a profile nothing lists:
            // a `<root>/*/` glob does not match one, so the completion and the
            // managing tool never offer it.
            ".hidden",
            "has space",
            "n\u{0}ul",
        ] {
            let host = HostEnv {
                profile: Some(named.to_owned()),
                ..HostEnv::default()
            };
            let lookup = resolve_token(None, Some(root.path()), &host);
            assert!(
                matches!(&lookup, TokenLookup::Missing(NoToken::ProfileNotAName(n)) if n == named),
                "{named:?}: {lookup:?}"
            );
        }
    }

    #[test]
    fn an_ordinary_profile_name_is_accepted() {
        // The other half of the rule above, so it cannot be tightened into refusing
        // everything and still pass.
        for named in ["work", "work-2", "work_2", "Work.2", "a"] {
            assert_eq!(
                ProfileName::parse(named).as_ref().map(ProfileName::as_str),
                Some(named),
                "{named:?}"
            );
        }
    }

    #[test]
    fn a_profile_with_no_root_to_look_in_refuses_as_a_profile() {
        // A machine that names no home directory and set no override. A profile was
        // still typed, so this is that refusal rather than `NotLoggedIn`.
        let host = HostEnv {
            profile: Some("work".to_owned()),
            ..HostEnv::default()
        };
        assert!(matches!(
            resolve_token(None, None, &host),
            TokenLookup::Missing(NoToken::ProfileUnreadable { .. })
        ));
    }

    #[test]
    fn the_refresh_token_does_not_travel_from_a_profile_either() {
        let root = tempfile::tempdir().expect("a scratch profiles root");
        let dir = root.path().join("work");
        std::fs::create_dir_all(&dir).expect("a profile dir");
        std::fs::write(
            dir.join(CREDENTIALS_FILENAME),
            r#"{"claudeAiOauth":{"accessToken":"not-a-real-access-token",
               "refreshToken":"not-a-real-refresh-token"}}"#,
        )
        .expect("a credential");
        let host = HostEnv {
            profile: Some("work".to_owned()),
            ..HostEnv::default()
        };
        let TokenLookup::Found(token) = resolve_token(None, Some(root.path()), &host) else {
            panic!("the profile credential should have been read");
        };
        assert!(!token.as_str().contains("refresh"), "{token:?}");
    }

    #[test]
    fn a_profile_refusal_never_prints_a_token() {
        // `ProfileUnreadable` carries a name and a path, and both are safe. This is
        // the guard against someone later adding the value to the message.
        let root = tempfile::tempdir().expect("a scratch profiles root");
        let host = HostEnv {
            profile: Some("work".to_owned()),
            token: Some("not-a-real-secret-token".to_owned()),
            ..HostEnv::default()
        };
        let lookup = resolve_token(None, Some(root.path()), &host);
        assert!(!format!("{lookup:?}").contains("secret"), "{lookup:?}");
    }

    #[test]
    fn the_host_honours_claude_config_dir() {
        // The asymmetry this closes: the container-side probe has always read this
        // variable and the host side did not, so a host that moved its configuration
        // reported NotLoggedIn while sitting on a perfectly good login.
        let moved = credential_dir_holding("not-a-real-moved-token");
        let host = HostEnv {
            config_dir: Some(moved.path().display().to_string()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(None, None, &host),
            TokenLookup::Found(Token("not-a-real-moved-token".to_owned()))
        );
    }

    #[test]
    fn the_config_dir_is_read_instead_of_home_rather_than_as_well() {
        // Claude Code does not fall back from `$CLAUDE_CONFIG_DIR` to `$HOME/.claude`,
        // and neither does this: falling back would forward a credential out of a
        // directory Claude Code is not reading, which is the same defect as ignoring
        // the variable, only harder to see. Two accounts on one host is exactly when
        // it would bite.
        let home = logged_in("not-a-real-home-token");
        let moved = credential_dir_holding("not-a-real-moved-token");
        let host = HostEnv {
            config_dir: Some(moved.path().display().to_string()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(Some(home.path()), None, &host),
            TokenLookup::Found(Token("not-a-real-moved-token".to_owned()))
        );

        // And an empty one names no directory at all, which is what the XDG rule
        // already says elsewhere and what a shell exporting a bare variable means.
        let empty = HostEnv {
            config_dir: Some(String::new()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(Some(home.path()), None, &empty),
            TokenLookup::Found(Token("not-a-real-home-token".to_owned()))
        );
    }

    #[test]
    fn a_config_dir_naming_nothing_is_not_logged_in_rather_than_a_fallback() {
        // Ambient rather than typed, so its absence is the quiet arm and not a
        // warning: the same reason a missing `$HOME/.claude` is NotLoggedIn.
        let empty_dir = tempfile::tempdir().expect("a scratch config dir");
        let home = logged_in("not-a-real-home-token");
        let host = HostEnv {
            config_dir: Some(empty_dir.path().display().to_string()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(Some(home.path()), None, &host),
            TokenLookup::Missing(NoToken::NotLoggedIn)
        );
    }

    #[test]
    fn an_inherited_token_beats_the_config_dir() {
        // Both are ambient, and the exported token keeps winning so that a `dl`
        // launched from inside a workspace forwards the one it was handed.
        let moved = credential_dir_holding("not-a-real-moved-token");
        let host = HostEnv {
            token: Some("not-a-real-exported-token".to_owned()),
            config_dir: Some(moved.path().display().to_string()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(None, None, &host),
            TokenLookup::Found(Token("not-a-real-exported-token".to_owned()))
        );
    }

    #[test]
    fn the_opt_out_is_read_before_the_config_dir_is() {
        // Set, therefore meant. A machine that has opted out has no account to choose.
        let moved = credential_dir_holding("not-a-real-moved-token");
        let host = HostEnv {
            disable: Some("1".to_owned()),
            config_dir: Some(moved.path().display().to_string()),
            ..HostEnv::default()
        };
        assert_eq!(
            resolve_token(None, None, &host),
            TokenLookup::Missing(NoToken::OptedOut)
        );
    }

    #[test]
    fn the_refresh_token_still_does_not_travel_from_a_config_dir() {
        // The module's whole reason for reading the file rather than shipping it,
        // asserted on the new path as well as the old one.
        let dir = tempfile::tempdir().expect("a scratch config dir");
        std::fs::write(
            dir.path().join(CREDENTIALS_FILENAME),
            r#"{"claudeAiOauth":{"accessToken":"not-a-real-access-token",
               "refreshToken":"not-a-real-refresh-token"}}"#,
        )
        .expect("a credential");
        let host = HostEnv {
            config_dir: Some(dir.path().display().to_string()),
            ..HostEnv::default()
        };
        let TokenLookup::Found(token) = resolve_token(None, None, &host) else {
            panic!("the credential should have been read");
        };
        assert!(!token.as_str().contains("refresh"), "{token:?}");
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
            resolve_token(None, None, &host),
            TokenLookup::Found(Token("not-a-real-exported-token".to_owned()))
        );
    }

    #[test]
    fn a_host_with_no_credential_is_not_logged_in_rather_than_broken() {
        // The macOS case, where the login is in the keychain, and the never-ran-claude
        // case. Both are ordinary and neither is worth a warning on every launch.
        let home = tempfile::tempdir().expect("a scratch home");
        assert_eq!(
            resolve_token(Some(home.path()), None, &HostEnv::default()),
            TokenLookup::Missing(NoToken::NotLoggedIn)
        );
        assert_eq!(
            resolve_token(None, None, &HostEnv::default()),
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
            std::fs::write(
                home.path().join(CONFIG_RELPATH).join(CREDENTIALS_FILENAME),
                text,
            )
            .expect("a file");
            assert!(
                matches!(
                    resolve_token(Some(home.path()), None, &HostEnv::default()),
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
