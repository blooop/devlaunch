//! The Claude logins this host can forward, as a person would want them listed.
//!
//! `--claude-profile <name>` picks one of these
//! ([`crate::clients::claude::resolve_token`]); this is the read side, and it exists
//! for one reason. **A profile's directory name is chosen by a person and verified by
//! nothing.** A profile called `work` holding a personal login is indistinguishable
//! from a correct one until something reads the account out of it, and the failure it
//! causes is the exact failure profiles exist to prevent: work pushed from the wrong
//! identity, found out about later and somewhere else.
//!
//! So a row is a name *and* the account behind it, read from the state file Claude
//! Code keeps beside the credential. The name is what you type; the account is what
//! you get.
//!
//! # What this deliberately does not do
//!
//! **No writer, and none is coming.** Creating a profile, seeding the configuration it
//! shares with the main login, and deleting it belong to whatever manages the
//! directory ([`crate::domain::xdg::claude_profiles_root`] names the arrangement).
//! This enumerates and reads.
//!
//! **No token is read.** [`ProfileState`] answers "is there a credential here" from
//! the file's existence, never from its contents, so nothing in a listing has touched
//! a secret. That is a smaller claim than it sounds and worth keeping: a listing is
//! the surface most likely to grow a `--json` and end up somewhere it should not.

use std::path::{Path, PathBuf};

use crate::clients::claude;

/// The account behind a profile, at a path a caller outside this crate can name.
///
/// [`ProfileSummary::account`] is a public field of this type and `clients::claude` is
/// `pub(crate)`, so without the re-export the field is readable while its type is not
/// nameable: a consumer can reach `summary.account.email` and cannot write the type of
/// what they are holding, or a function that takes one. `dl` never noticed because it
/// only ever reaches through the field.
///
/// Re-exported here rather than by making `clients::claude` public, which would put the
/// token machinery on the same surface for no reason.
pub use crate::clients::claude::Account;

/// The name that means "the login this host uses anyway".
///
/// Spelled here as well as in [`crate::clients::claude`] because a listing has to
/// offer it as a row and the resolver has to answer for it, and
/// `the_default_row_is_the_name_the_resolver_answers_for` diffs the two rather than
/// leaving one to drift.
pub const DEFAULT_PROFILE: &str = "default";

/// Whether a profile can be launched with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileState {
    /// A credential file is there. Whether it still *works* is a question only the
    /// server can answer, and an access token lives hours, so this is deliberately
    /// "has one" rather than "has a valid one".
    Authed,
    /// The directory exists and holds no credential: the ordinary state immediately
    /// after something created it and before anyone logged in. Naming it is the whole
    /// point, since a launch naming this profile refuses.
    NoCredential,
}

/// One row of the listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileSummary {
    /// What you type after `--claude-profile`.
    pub name: String,
    /// The config directory it names, for a reader who needs to go and look.
    pub path: PathBuf,
    pub state: ProfileState,
    /// Who it is signed in as, when the state file says. `None` is not an error: a
    /// profile never logged in to has none, and neither does one whose file Claude
    /// Code has since reshaped.
    pub account: Option<Account>,
}

/// Every profile this host offers, `default` first and the rest by name.
///
/// `default` leads because it is the login every other row is an alternative to, and
/// it is present even when no profiles directory exists at all, which is the state
/// almost every host is in.
///
/// Paths rather than reads of the environment, so a test states the machine it means.
/// `unnamed` is the config directory the default login resolves to
/// (`$CLAUDE_CONFIG_DIR`, else `~/.claude`), and `None` is a host that names neither.
pub fn summarise(profiles_root: Option<&Path>, unnamed: Option<&Path>) -> Vec<ProfileSummary> {
    let mut rows = Vec::new();
    if let Some(unnamed) = unnamed {
        rows.push(row(DEFAULT_PROFILE.to_owned(), unnamed.to_path_buf()));
    }
    let Some(root) = profiles_root else {
        return rows;
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        // A root that does not exist is a host with no profiles, not a failure: the
        // directory is created by whatever manages the profiles, and most hosts never
        // have one.
        return rows;
    };
    let mut named: Vec<ProfileSummary> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            // A directory the resolver would refuse is not offered, so the listing and
            // the launch cannot disagree about what a profile is. `default` is excluded
            // by the same call, since the resolver never consults a directory of that
            // name.
            (claude::profile_name_is_offerable(&name)).then(|| row(name, entry.path()))
        })
        .collect();
    named.sort_by(|a, b| a.name.cmp(&b.name));
    rows.extend(named);
    rows
}

/// [`summarise`], for this machine.
///
/// The impure half, kept to one function so everything above it is a function of its
/// inputs: the root through [`crate::domain::xdg::claude_profiles_root`], the unnamed
/// login's directory through the client that owns that decision. Empty means a host
/// naming neither a home directory nor an override, which is the one state with
/// nothing at all to list.
pub fn from_process() -> Vec<ProfileSummary> {
    let root = crate::domain::xdg::claude_profiles_root().ok();
    let unnamed = claude::unnamed_config_dir_from_process();
    summarise(root.as_deref(), unnamed.as_deref())
}

fn row(name: String, path: PathBuf) -> ProfileSummary {
    let state = if claude::has_credential(&path) {
        ProfileState::Authed
    } else {
        ProfileState::NoCredential
    };
    ProfileSummary {
        name,
        path: path.clone(),
        state,
        account: claude::account_at(&path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile directory, optionally with a credential and an account.
    fn profile(root: &Path, name: &str, credential: bool, email: Option<&str>) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("a profile dir");
        if credential {
            std::fs::write(
                dir.join(".credentials.json"),
                r#"{"claudeAiOauth":{"accessToken":"not-a-real-token"}}"#,
            )
            .expect("a credential");
        }
        if let Some(email) = email {
            std::fs::write(
                dir.join(".claude.json"),
                format!(
                    r#"{{"oauthAccount":{{"emailAddress":"{email}",
                       "organizationName":"Someorg","seatTier":"team"}},"other":1}}"#
                ),
            )
            .expect("an account file");
        }
        dir
    }

    #[test]
    fn a_profile_is_named_with_the_account_behind_it() {
        // The whole point: the name is chosen by a person and verified by nothing, so
        // the row carries who it is actually signed in as.
        let root = tempfile::tempdir().expect("a scratch root");
        profile(root.path(), "work", true, Some("someone@example.com"));
        let rows = summarise(Some(root.path()), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "work");
        assert_eq!(rows[0].state, ProfileState::Authed);
        let account = rows[0].account.as_ref().expect("an account");
        assert_eq!(account.email.as_deref(), Some("someone@example.com"));
        assert_eq!(account.organization.as_deref(), Some("Someorg"));
        assert_eq!(account.seat_tier.as_deref(), Some("team"));
    }

    #[test]
    fn a_profile_made_and_never_logged_in_to_says_so() {
        // The ordinary state right after something creates one, and a launch naming it
        // refuses, so the listing has to distinguish it rather than omit it.
        let root = tempfile::tempdir().expect("a scratch root");
        profile(root.path(), "fresh", false, None);
        let rows = summarise(Some(root.path()), None);
        assert_eq!(rows[0].state, ProfileState::NoCredential);
        assert_eq!(rows[0].account, None);
    }

    #[test]
    fn an_account_file_that_says_nothing_useful_is_not_an_error() {
        // Three absences with one answer, because they read alike to somebody drawing a
        // table: no file, not JSON, and no oauthAccount.
        let root = tempfile::tempdir().expect("a scratch root");
        let junk = profile(root.path(), "junk", true, None);
        std::fs::write(junk.join(".claude.json"), "not json at all").expect("a file");
        let empty = profile(root.path(), "empty", true, None);
        std::fs::write(empty.join(".claude.json"), r#"{"somethingElse":1}"#).expect("a file");
        for row in summarise(Some(root.path()), None) {
            assert_eq!(row.account, None, "{}", row.name);
            // Still launchable: a missing label says nothing about the credential.
            assert_eq!(row.state, ProfileState::Authed, "{}", row.name);
        }
    }

    #[test]
    fn the_default_row_leads_and_is_there_with_no_profiles_at_all() {
        // Almost every host is in this state, and `default` is the login every other
        // row is an alternative to.
        let home = tempfile::tempdir().expect("a scratch home");
        let unnamed = home.path().join(".claude");
        std::fs::create_dir_all(&unnamed).expect("a config dir");
        std::fs::write(
            unnamed.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"not-a-real-token"}}"#,
        )
        .expect("a credential");
        let rows = summarise(None, Some(&unnamed));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, DEFAULT_PROFILE);
        assert_eq!(rows[0].state, ProfileState::Authed);
    }

    #[test]
    fn the_default_row_is_the_name_the_resolver_answers_for() {
        // Two spellings of one fact, diffed rather than left to drift: a listing that
        // offered a word the resolver did not answer for would be offering a launch
        // that refuses.
        assert_eq!(DEFAULT_PROFILE, claude::default_profile_name());
    }

    #[test]
    fn the_listing_offers_only_names_a_launch_would_accept() {
        // The listing and the launch must not disagree about what a profile is. A
        // directory the resolver refuses is not offered, and `default` is excluded
        // because the resolver never consults a directory of that name.
        let root = tempfile::tempdir().expect("a scratch root");
        for name in ["work", "default", "has space", ".hidden-ish", "-flag"] {
            profile(root.path(), name, true, None);
        }
        let rows = summarise(Some(root.path()), None);
        let offered: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(offered, ["work"]);
    }

    #[test]
    fn rows_are_sorted_so_a_listing_does_not_reorder_itself() {
        // `read_dir` order is the filesystem's and varies between machines and between
        // runs on one machine.
        let root = tempfile::tempdir().expect("a scratch root");
        for name in ["zeta", "alpha", "mid"] {
            profile(root.path(), name, true, None);
        }
        let rows = summarise(Some(root.path()), None);
        let offered: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(offered, ["alpha", "mid", "zeta"]);
    }
}
