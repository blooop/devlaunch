//! `config.toml`: the few knobs the worktree backend reads.
//!
//! Ported from `devlaunch/worktree/config.py`. There is no writer — nothing in
//! devlaunch creates or edits this file — so this is a reader with defaults, and
//! the defaults are what almost every run uses.
//!
//! Two properties carry over, both about not punishing a stale file:
//!
//! - **A missing file is not an error**, it is the defaults.
//! - **A key this build does not know is ignored, in silence.** `auto_fetch` was
//!   retired without ever gating anything, and a config still naming it keeps
//!   working with its other keys applied. There has never been an unknown-key
//!   warning here and a retired knob is not the thing to introduce nagging for.
//!
//! What does not carry over is Python's tolerance of a wrong *type*: an
//! `fetch_interval = "soon"` was accepted and broke later, arithmetic-first.
//! Here it is a typed refusal at load, which is the whole reason for a parse
//! step.

use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::xdg::{self, NoHomeDirectory};

/// Seconds between background fetches, when the file does not say.
const DEFAULT_FETCH_INTERVAL: u64 = 3600;

/// Days a workspace clone may go unused before a prune considers it.
const DEFAULT_PRUNE_AFTER_DAYS: u64 = 30;

/// Configuration for the worktree backend.
///
/// All data lives under `repos_dir`:
///
/// ```text
/// repos_dir/owner/repo/          the bare git repository
/// repos_dir/owner/repo/clones/   the workspace clones, one per branch
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeConfig {
    pub enabled: bool,
    pub repos_dir: PathBuf,
    pub fetch_interval: u64,
    pub auto_prune: bool,
    pub prune_after_days: u64,
    /// Docker image for repositories with no `devcontainer.json`.
    pub fallback_image: Option<String>,
}

/// Why the configuration could not be read.
#[derive(Debug)]
pub enum ConfigError {
    /// This machine names no home directory, so no config path can be built.
    NoHomeDirectory,
    /// The file exists but could not be read.
    Unreadable { path: PathBuf, source: io::Error },
    /// The file is not TOML, or a value is not of the type its key must be.
    /// `reason` is the parser's own words, quoted as data.
    Malformed { path: PathBuf, reason: String },
}

impl From<NoHomeDirectory> for ConfigError {
    fn from(_: NoHomeDirectory) -> Self {
        ConfigError::NoHomeDirectory
    }
}

impl WorktreeConfig {
    /// The defaults, with `repos_dir` under `cache` — `devlaunch_cache()/repos`.
    pub(crate) fn defaults_in(cache: &Path) -> Self {
        Self {
            enabled: true,
            repos_dir: cache.join("repos"),
            fetch_interval: DEFAULT_FETCH_INTERVAL,
            auto_prune: true,
            prune_after_days: DEFAULT_PRUNE_AFTER_DAYS,
            fallback_image: None,
        }
    }
}

/// Where `config.toml` is looked for.
pub(crate) fn config_path() -> Result<PathBuf, NoHomeDirectory> {
    xdg::config_home().map(|config| config.join("devlaunch").join("config.toml"))
}

/// The worktree configuration this machine is running with.
///
/// Reads `config_home()/devlaunch/config.toml` if it is there, defaults if it is
/// not, and then makes sure `repos_dir` exists — see [`ensure_repos_dir`] for
/// why that is a side effect of loading rather than of using it.
pub fn worktree_config() -> Result<WorktreeConfig, ConfigError> {
    let path = config_path()?;
    let defaults = WorktreeConfig::defaults_in(&xdg::devlaunch_cache()?);
    let config = worktree_config_at(&path, &defaults)?;
    ensure_repos_dir(&config.repos_dir);
    Ok(config)
}

/// The configuration in `path`, or `defaults` if there is no file there.
///
/// Pure but for the read: nothing is created, so this is what tests drive.
pub(crate) fn worktree_config_at(
    path: &Path,
    defaults: &WorktreeConfig,
) -> Result<WorktreeConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(defaults.clone()),
        Err(error) => {
            return Err(ConfigError::Unreadable {
                path: path.to_path_buf(),
                source: error,
            });
        }
    };
    parse_worktree_config(&text, defaults).map_err(|reason| ConfigError::Malformed {
        path: path.to_path_buf(),
        reason,
    })
}

/// Read the `[worktree]` tables out of `text`, filling in from `defaults`.
///
/// The error is the parser's message; the caller adds the path.
pub(crate) fn parse_worktree_config(
    text: &str,
    defaults: &WorktreeConfig,
) -> Result<WorktreeConfig, String> {
    let document: StoredConfig = toml::from_str(text).map_err(|error| error.to_string())?;
    let worktree = document.worktree.unwrap_or_default();
    let cleanup = worktree.cleanup.unwrap_or_default();
    Ok(WorktreeConfig {
        enabled: worktree.enabled.unwrap_or(defaults.enabled),
        repos_dir: worktree
            .repos_dir
            .map(|raw| expand_tilde(&raw))
            .unwrap_or_else(|| defaults.repos_dir.clone()),
        fetch_interval: worktree.fetch_interval.unwrap_or(defaults.fetch_interval),
        auto_prune: cleanup.auto_prune.unwrap_or(defaults.auto_prune),
        prune_after_days: cleanup
            .prune_after_days
            .unwrap_or(defaults.prune_after_days),
        fallback_image: worktree
            .fallback_image
            .or_else(|| defaults.fallback_image.clone()),
    })
}

/// Create `repos_dir` if it is somewhere devlaunch may create directories.
///
/// Under the home directory or under `/tmp` and nowhere else, and failures are
/// ignored: this is a convenience for a first run, not a step anything depends
/// on, and a config pointing at a directory this user cannot create is the
/// caller's problem to report when it actually needs it.
///
/// Python does this in the constructor, so every construction of a config had
/// the side effect. Here it is a step of loading, which is the same moment
/// without the surprise.
pub(crate) fn ensure_repos_dir(repos_dir: &Path) {
    if is_ours_to_create(repos_dir) {
        let _ = std::fs::create_dir_all(repos_dir);
    }
}

/// Whether a path is somewhere this may create directories unasked.
fn is_ours_to_create(path: &Path) -> bool {
    if path.starts_with("/tmp") {
        return true;
    }
    crate::osext::home_dir().is_some_and(|home| path.starts_with(&home))
}

/// `~` and `~/…` against `$HOME`, as Python's `expanduser` reads them.
///
/// `~user` is left alone: resolving another user's home needs the password
/// database, and a `repos_dir` naming somebody else's home is not a case
/// devlaunch has.
fn expand_tilde(raw: &str) -> PathBuf {
    let Some(rest) = raw.strip_prefix('~') else {
        return PathBuf::from(raw);
    };
    let Some(home) = crate::osext::home_dir() else {
        return PathBuf::from(raw);
    };
    match rest {
        "" => home,
        rest => match rest.strip_prefix('/') {
            Some(relative) => home.join(relative),
            // `~other/path`: not ours to resolve.
            None => PathBuf::from(raw),
        },
    }
}

/// The file's shape: every key optional, unknown keys ignored.
#[derive(Debug, Default, Deserialize)]
struct StoredConfig {
    worktree: Option<StoredWorktree>,
}

#[derive(Debug, Default, Deserialize)]
struct StoredWorktree {
    enabled: Option<bool>,
    repos_dir: Option<String>,
    fetch_interval: Option<u64>,
    fallback_image: Option<String>,
    cleanup: Option<StoredCleanup>,
}

#[derive(Debug, Default, Deserialize)]
struct StoredCleanup {
    auto_prune: Option<bool>,
    prune_after_days: Option<u64>,
}

#[cfg(test)]
mod tests {
    //! `test/test_worktree_config.py`, re-pinned.
    //!
    //! Python's tests construct `WorktreeConfig(...)` directly and check
    //! `to_dict`; both are gone here, because nothing in devlaunch writes this
    //! file and `to_dict` had no caller outside those tests. What is left — the
    //! defaults, the nested tables, the silence about unknown keys, the path the
    //! file is looked for at — is all of it that a run depends on.

    use super::*;

    /// The defaults a machine with this cache directory would have.
    fn defaults() -> WorktreeConfig {
        WorktreeConfig::defaults_in(Path::new("/home/someone/.cache/devlaunch"))
    }

    fn parse(text: &str) -> WorktreeConfig {
        parse_worktree_config(text, &defaults()).expect("readable configuration")
    }

    #[test]
    fn the_defaults_are_the_documented_ones() {
        let config = defaults();

        assert!(
            config.enabled,
            "the worktree backend is on unless told not to"
        );
        assert_eq!(
            config.repos_dir,
            PathBuf::from("/home/someone/.cache/devlaunch/repos")
        );
        assert_eq!(config.fetch_interval, 3600);
        assert!(config.auto_prune);
        assert_eq!(config.prune_after_days, 30);
        assert_eq!(config.fallback_image, None);
    }

    #[test]
    fn an_empty_document_is_the_defaults() {
        assert_eq!(parse(""), defaults());
        assert_eq!(parse("[worktree]\n"), defaults());
        assert_eq!(parse("[worktree]\n[worktree.cleanup]\n"), defaults());
    }

    #[test]
    fn every_knob_can_be_set() {
        let config = parse(
            r#"
            [worktree]
            enabled = false
            repos_dir = "/custom/repos"
            fetch_interval = 7200
            fallback_image = "ubuntu:22.04"

            [worktree.cleanup]
            auto_prune = false
            prune_after_days = 60
            "#,
        );

        assert_eq!(
            config,
            WorktreeConfig {
                enabled: false,
                repos_dir: PathBuf::from("/custom/repos"),
                fetch_interval: 7200,
                auto_prune: false,
                prune_after_days: 60,
                fallback_image: Some("ubuntu:22.04".to_owned()),
            }
        );
    }

    #[test]
    fn a_key_the_file_does_not_set_keeps_its_default() {
        let config = parse("[worktree]\nenabled = false\n");

        assert!(!config.enabled);
        assert_eq!(config.repos_dir, defaults().repos_dir);
        assert_eq!(config.fetch_interval, 3600);
        assert!(config.auto_prune);
        assert_eq!(config.prune_after_days, 30);
    }

    #[test]
    fn a_cleanup_table_with_one_key_keeps_the_other_default() {
        let config = parse("[worktree.cleanup]\nprune_after_days = 7\n");

        assert_eq!(config.prune_after_days, 7);
        assert!(config.auto_prune);
    }

    #[test]
    fn a_config_still_naming_the_retired_auto_fetch_knob_loads_with_its_other_keys() {
        // Deleting an inert knob must not turn someone's existing config.toml
        // into an error, and there is no unknown-key warning to introduce.
        let config = parse(
            r#"
            [worktree]
            auto_fetch = false
            enabled = false
            fetch_interval = 7200
            "#,
        );

        assert!(!config.enabled);
        assert_eq!(config.fetch_interval, 7200);
    }

    #[test]
    fn a_repos_dir_starting_with_a_tilde_is_expanded() {
        let home = std::env::home_dir().expect("this machine has a home directory");

        let config = parse("[worktree]\nrepos_dir = \"~/custom/repos\"\n");

        assert_eq!(config.repos_dir, home.join("custom/repos"));
        assert_eq!(
            parse("[worktree]\nrepos_dir = \"~\"\n").repos_dir,
            home,
            "a bare tilde is the home directory itself"
        );
        assert_eq!(
            parse("[worktree]\nrepos_dir = \"~someone/repos\"\n").repos_dir,
            PathBuf::from("~someone/repos"),
            "another user's home is not ours to resolve"
        );
    }

    #[test]
    fn text_that_is_not_toml_is_refused_with_the_parsers_reason() {
        let reason =
            parse_worktree_config("[worktree\nenabled = true", &defaults()).expect_err("not TOML");

        assert!(!reason.is_empty());
    }

    #[test]
    fn a_value_of_the_wrong_type_is_refused_rather_than_carried() {
        // Python accepted this and failed later, in arithmetic; a parse step
        // exists so the refusal happens where the value is read.
        for text in [
            "[worktree]\nfetch_interval = \"soon\"\n",
            "[worktree]\nenabled = \"yes\"\n",
            "[worktree.cleanup]\nprune_after_days = -1\n",
        ] {
            parse_worktree_config(text, &defaults()).expect_err(text);
        }
    }

    // --- the file on disk --------------------------------------------------

    #[test]
    fn no_file_at_all_is_the_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");

        let config =
            worktree_config_at(&dir.path().join("config.toml"), &defaults()).expect("the defaults");

        assert_eq!(config, defaults());
    }

    #[test]
    fn a_file_is_read_from_the_path_it_is_at() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[worktree]\nfetch_interval = 60\n").expect("the fixture");

        let config = worktree_config_at(&path, &defaults()).expect("readable");

        assert_eq!(config.fetch_interval, 60);
    }

    #[test]
    fn a_malformed_file_names_itself_in_the_refusal() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[worktree").expect("the fixture");

        let failed = worktree_config_at(&path, &defaults()).expect_err("not TOML");

        match failed {
            ConfigError::Malformed {
                path: named,
                reason,
            } => {
                assert_eq!(named, path);
                assert!(!reason.is_empty());
            }
            other => panic!("a malformed file, got {other:?}"),
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_silently_the_defaults() {
        // A directory where the file should be: readable-looking, unreadable.
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        std::fs::create_dir(&path).expect("the fixture");

        let failed = worktree_config_at(&path, &defaults()).expect_err("not a file");

        assert!(
            matches!(failed, ConfigError::Unreadable { .. }),
            "{failed:?}"
        );
    }

    #[test]
    fn the_config_file_is_one_directory_under_the_config_home() {
        // Read-only against the real environment: which directory the XDG
        // variables resolve to is `xdg`'s business and is tested there.
        assert_eq!(
            config_path(),
            xdg::config_home().map(|home| home.join("devlaunch").join("config.toml"))
        );
    }

    // --- creating repos_dir ------------------------------------------------

    #[test]
    fn a_repos_dir_under_tmp_is_created_on_demand() {
        let dir = tempfile::Builder::new()
            .prefix("devlaunch-config-test")
            .tempdir_in("/tmp")
            .expect("a temp dir under /tmp");
        let repos = dir.path().join("repos");

        ensure_repos_dir(&repos);

        assert!(repos.is_dir());
    }

    #[test]
    fn a_repos_dir_somewhere_else_is_left_to_whoever_owns_it() {
        // Nothing is created outside the home directory and /tmp, and a failure
        // to create is ignored rather than raised — a first run's convenience
        // must not become a run's error.
        let elsewhere = Path::new("/proc/devlaunch-should-not-create/repos");

        assert!(!is_ours_to_create(elsewhere));
        ensure_repos_dir(elsewhere);

        assert!(!elsewhere.exists());
    }

    #[test]
    fn the_home_directory_is_ours_to_create_in() {
        let home = std::env::home_dir().expect("this machine has a home directory");

        assert!(is_ours_to_create(&home.join(".cache/devlaunch/repos")));
        assert!(is_ours_to_create(Path::new("/tmp/anything")));
        assert!(!is_ours_to_create(Path::new("/etc/devlaunch")));
    }
}
