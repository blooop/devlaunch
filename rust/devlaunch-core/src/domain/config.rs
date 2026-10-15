//! `config.toml`: the few knobs the worktree backend reads.
//!
//! Ported from `devlaunch/worktree/config.py`. A reader with defaults, and the
//! defaults are what almost every run uses. It has one writer, and that writer
//! touches only the `[dotfiles]` table, once: see [`write_dotfiles_setting`].
//!
//! Two properties carry over, both about not punishing a stale file:
//!
//! - **A missing file is not an error**, it is the defaults.
//! - **A key this build does not know is ignored, in silence.** `auto_fetch` was
//!   retired without ever gating anything, and a config still naming it keeps
//!   working with its other keys applied. There has never been an unknown-key
//!   warning here and a retired knob is not the thing to introduce nagging for.
//!
//! **`repos_dir` is the one exception, and the exception is what defines the
//! rule.** It is retired too (#467), but it *gated where the clones went*: a
//! user who set it has a real tree at a path dl no longer looks at. Nothing is
//! moved and nothing is deleted, so the whole of that migration is that the
//! directory is named once — see [`RetiredKey`]. Silence is what would strand a
//! tree, which is exactly what `auto_fetch`, having gated nothing, could not do.
//!
//! What does not carry over is Python's tolerance of a wrong *type*: an
//! `fetch_interval = "soon"` was accepted and broke later, arithmetic-first.
//! Here it is a typed refusal at load, which is the whole reason for a parse
//! step.

use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::metadata::OsFailure;
use super::xdg::{self, NoHomeDirectory};

/// Seconds between background fetches, when the file does not say.
const DEFAULT_FETCH_INTERVAL: u64 = 3600;

/// Days a workspace clone may go unused before a prune considers it.
const DEFAULT_PRUNE_AFTER_DAYS: u64 = 30;

/// Configuration for the worktree backend.
///
/// **No paths.** Where the clones go is derived from the cache directory
/// ([`clone_root_in`](super::xdg::clone_root_in)) and cannot be configured, so
/// this carries settings and never placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeConfig {
    pub enabled: bool,
    pub fetch_interval: u64,
    pub auto_prune: bool,
    pub prune_after_days: u64,
    /// Docker image for repositories with no `devcontainer.json`.
    pub fallback_image: Option<String>,
}

/// Why the configuration could not be read.
///
/// Not-TOML and wrong-typed are two arms rather than one `Malformed`, matching
/// the granularity divergence row 8 claims for the refusal: a file that is not
/// TOML at all and a TOML file whose one value has the wrong type are different
/// things to fix, and a caller holding one string could not tell which it had.
///
/// `Clone` and comparable, for [`metadata::MetadataError`](super::metadata::MetadataError)'s
/// reason: a refusal that travels inside another refusal has to be as copyable as
/// the one carrying it, and since #340 this one travels inside
/// [`ColdRefused`](crate::flows::launch::ColdRefused). That is what the OS side
/// being an [`OsFailure`] rather than an `io::Error` buys — the same words, from a
/// value that can be cloned and compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// This machine names no home directory, so no config path can be built.
    NoHomeDirectory,
    /// The file exists but could not be read.
    Unreadable { path: PathBuf, source: OsFailure },
    /// The file is not TOML at all. `reason` is the parser's own words, quoted
    /// as data.
    NotToml { path: PathBuf, reason: String },
    /// TOML, but a value is not of the type its key must be. `reason` is the
    /// deserializer's own words, quoted as data.
    WrongType { path: PathBuf, reason: String },
}

impl From<NoHomeDirectory> for ConfigError {
    fn from(_: NoHomeDirectory) -> Self {
        ConfigError::NoHomeDirectory
    }
}

/// A key this build no longer reads, found named in `config.toml`.
///
/// Carried out of the load rather than warned about inside it, like
/// [`metadata::Notice`](super::metadata::Notice): the fact is core's and the
/// sentence is the binary's (#251 §5).
///
/// One arm, and it is here rather than the file simply ignoring the key because
/// this key *had authority over where data went*. A key that gated nothing is
/// ignored in silence — see the module doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetiredKey {
    /// `worktree.repos_dir`, retired in #467. It used to decide where dl put its
    /// clones; it no longer does, and the tree it named is left exactly where its
    /// owner put it.
    ///
    /// `named` is the value verbatim, quoted as data — not resolved against the
    /// cache, and not compared with it. A comparison is the thing that can be
    /// silently wrong (a symlinked home, a `XDG_CACHE_HOME` that has moved since
    /// the tree was made), and being wrong in that direction is silence about a
    /// tree nothing else will ever name.
    ReposDir { named: String },
}

impl WorktreeConfig {
    /// The defaults, which are what almost every run uses.
    pub(crate) fn defaults() -> Self {
        Self {
            enabled: true,
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

/// The worktree configuration this machine is running with, and whatever of it
/// this build no longer reads.
///
/// Reads `config_home()/devlaunch/config.toml` if it is there, defaults if it is
/// not. **Pure but for that read**: loading a configuration creates no
/// directories, which it used to do for a `repos_dir` it could no longer be sure
/// of.
pub fn worktree_config() -> Result<(WorktreeConfig, Vec<RetiredKey>), ConfigError> {
    let path = config_path()?;
    worktree_config_at(&path, &WorktreeConfig::defaults())
}

/// The configuration in `path`, or `defaults` if there is no file there.
///
/// Pure but for the read, so this is what tests drive.
pub(crate) fn worktree_config_at(
    path: &Path,
    defaults: &WorktreeConfig,
) -> Result<(WorktreeConfig, Vec<RetiredKey>), ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((defaults.clone(), Vec::new()));
        }
        Err(error) => {
            return Err(ConfigError::Unreadable {
                path: path.to_path_buf(),
                source: error.into(),
            });
        }
    };
    parse_worktree_config(&text, defaults).map_err(|refused| match refused {
        Malformed::NotToml { reason } => ConfigError::NotToml {
            path: path.to_path_buf(),
            reason,
        },
        Malformed::WrongType { reason } => ConfigError::WrongType {
            path: path.to_path_buf(),
            reason,
        },
    })
}

/// Why `text` is not a readable configuration — [`ConfigError`]'s two parse arms
/// before a path is known.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Malformed {
    /// Not TOML at all.
    NotToml { reason: String },
    /// TOML with a value of the wrong type.
    WrongType { reason: String },
}

/// Read the `[worktree]` tables out of `text`, filling in from `defaults`, and
/// say which retired keys the text still names.
///
/// The error is the parser's message; the caller adds the path. Which arm it is
/// comes from a syntax-only pass first: a text no [`toml::Table`] can be read
/// from is not TOML, and anything the typed read then refuses about a valid
/// table is a wrong-typed value — the two reads are of the same text, so the
/// message is the same one the single-pass read produced.
pub(crate) fn parse_worktree_config(
    text: &str,
    defaults: &WorktreeConfig,
) -> Result<(WorktreeConfig, Vec<RetiredKey>), Malformed> {
    if let Err(error) = text.parse::<toml::Table>() {
        return Err(Malformed::NotToml {
            reason: error.to_string(),
        });
    }
    let document: StoredConfig = toml::from_str(text).map_err(|error| Malformed::WrongType {
        reason: error.to_string(),
    })?;
    let worktree = document.worktree.unwrap_or_default();
    let cleanup = worktree.cleanup.unwrap_or_default();
    let retired = worktree
        .repos_dir
        .map(|named| RetiredKey::ReposDir { named })
        .into_iter()
        .collect();
    Ok((
        WorktreeConfig {
            enabled: worktree.enabled.unwrap_or(defaults.enabled),
            fetch_interval: worktree.fetch_interval.unwrap_or(defaults.fetch_interval),
            auto_prune: cleanup.auto_prune.unwrap_or(defaults.auto_prune),
            prune_after_days: cleanup
                .prune_after_days
                .unwrap_or(defaults.prune_after_days),
            fallback_image: worktree
                .fallback_image
                .or_else(|| defaults.fallback_image.clone()),
        },
        retired,
    ))
}

/// The file's shape: every key optional, unknown keys ignored.
#[derive(Debug, Default, Deserialize)]
struct StoredConfig {
    worktree: Option<StoredWorktree>,
}

#[derive(Debug, Default, Deserialize)]
struct StoredWorktree {
    enabled: Option<bool>,
    /// Read only so that it can be *reported* ([`RetiredKey::ReposDir`]) — no
    /// value of it reaches a [`WorktreeConfig`]. Dropping the field would make
    /// serde skip the key like any other unknown one, which is the silence this
    /// arm exists to prevent. Still `Option<String>` rather than a permissive
    /// type, so a wrong-typed value refuses the load exactly as it did before.
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

// ===========================================================================
// `[dotfiles]`: the repository dl installs into the containers it creates
// ===========================================================================

/// The dotfiles repository dl passes to `devpod up` when that `up` makes a
/// container, read from the `[dotfiles]` table of `config.toml`.
///
/// dl's own setting rather than devpod's `DOTFILES_URL` context option, because
/// devpod reads that option on **every** `up` and re-runs the install script each
/// time, a stopped container's plain start included (devpod 0.26.3,
/// `pkg/dotfiles/dotfiles.go`). A `--dotfiles` flag dl passes only on a create
/// cannot stop that while the context option is set, so the setting has to live
/// somewhere devpod does not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DotfilesRepo {
    pub(crate) url: String,
    /// The install script inside the repository, when one is named.
    pub(crate) script: Option<String>,
}

/// What `config.toml` says about dotfiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DotfilesSetting {
    /// `[dotfiles] url` is set.
    Set(DotfilesRepo),
    /// No file, no table, or a table with no `url`.
    Unset,
    /// The file is there and could not be read as TOML. Its own arm, because the
    /// one thing dl must not do with a file it cannot read is write over it.
    Unreadable { reason: String },
}

#[derive(Debug, Default, Deserialize)]
struct StoredDotfilesDocument {
    dotfiles: Option<StoredDotfiles>,
}

#[derive(Debug, Default, Deserialize)]
struct StoredDotfiles {
    url: Option<String>,
    script: Option<String>,
}

/// Read `[dotfiles]` out of the config file at `path`.
pub(crate) fn dotfiles_setting_at(path: &Path) -> DotfilesSetting {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return DotfilesSetting::Unset,
        Err(error) => {
            return DotfilesSetting::Unreadable {
                reason: error.to_string(),
            };
        }
    };
    parse_dotfiles_setting(&text)
}

/// Read `[dotfiles]` out of a config text. An empty `url` is no `url`, the way
/// devpod treats an empty context option.
pub(crate) fn parse_dotfiles_setting(text: &str) -> DotfilesSetting {
    let document: StoredDotfilesDocument = match toml::from_str(text) {
        Ok(document) => document,
        Err(error) => {
            return DotfilesSetting::Unreadable {
                reason: error.to_string(),
            };
        }
    };
    let Some(stored) = document.dotfiles else {
        return DotfilesSetting::Unset;
    };
    match stored.url.filter(|url| !url.is_empty()) {
        Some(url) => DotfilesSetting::Set(DotfilesRepo {
            url,
            script: stored.script.filter(|script| !script.is_empty()),
        }),
        None => DotfilesSetting::Unset,
    }
}

/// Write `repo` into the `[dotfiles]` table of the config file at `path`,
/// keeping every other line of the file as it was.
///
/// The one writer `config.toml` has, and it writes once: the move of devpod's
/// `DOTFILES_URL` into dl's own setting. It refuses rather than overwrites when
/// the file already names a `url`, or cannot be parsed, so a value somebody typed
/// is never replaced by one dl found elsewhere. Written to a temporary file beside
/// the target and renamed over it, so a reader never sees half a file.
pub(crate) fn write_dotfiles_setting(path: &Path, repo: &DotfilesRepo) -> Result<(), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.to_string()),
    };
    let updated = with_dotfiles_setting(&text, repo)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    // Per process, so two dl runs making the same move never share a half file.
    let temporary = path.with_extension(format!("toml.{}.tmp", std::process::id()));
    std::fs::write(&temporary, updated).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        error.to_string()
    })
}

/// `text` with `[dotfiles]` set to `repo`, or why that is refused.
pub(crate) fn with_dotfiles_setting(text: &str, repo: &DotfilesRepo) -> Result<String, String> {
    if let DotfilesSetting::Set(existing) = parse_dotfiles_setting(text) {
        // Already what would be written: a second dl that raced the first to the
        // move. Nothing to change, and nothing to refuse.
        if existing == *repo {
            return Ok(text.to_owned());
        }
        return Err(format!(
            "it already sets [dotfiles] url = {:?}",
            existing.url
        ));
    }
    let mut document = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| error.to_string())?;
    let entry = document
        .entry("dotfiles")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(table) = entry.as_table_mut() else {
        return Err("its `dotfiles` key is not a table".to_owned());
    };
    table.insert("url", toml_edit::value(repo.url.as_str()));
    match &repo.script {
        Some(script) => {
            table.insert("script", toml_edit::value(script.as_str()));
        }
        None => {
            table.remove("script");
        }
    }
    Ok(document.to_string())
}

#[cfg(test)]
mod dotfiles_tests {
    use super::*;

    fn repo(url: &str, script: Option<&str>) -> DotfilesRepo {
        DotfilesRepo {
            url: url.to_owned(),
            script: script.map(str::to_owned),
        }
    }

    #[test]
    fn no_table_and_an_empty_url_are_both_unset() {
        assert_eq!(parse_dotfiles_setting(""), DotfilesSetting::Unset);
        assert_eq!(
            parse_dotfiles_setting("[worktree]\nenabled = true\n"),
            DotfilesSetting::Unset
        );
        assert_eq!(
            parse_dotfiles_setting("[dotfiles]\nurl = \"\"\n"),
            DotfilesSetting::Unset
        );
    }

    #[test]
    fn the_table_is_read_with_and_without_a_script() {
        assert_eq!(
            parse_dotfiles_setting("[dotfiles]\nurl = \"https://x/d\"\n"),
            DotfilesSetting::Set(repo("https://x/d", None))
        );
        assert_eq!(
            parse_dotfiles_setting("[dotfiles]\nurl = \"https://x/d\"\nscript = \"i.sh\"\n"),
            DotfilesSetting::Set(repo("https://x/d", Some("i.sh")))
        );
    }

    #[test]
    fn a_file_that_is_not_toml_is_unreadable_not_unset() {
        assert!(matches!(
            parse_dotfiles_setting("not = = toml"),
            DotfilesSetting::Unreadable { .. }
        ));
    }

    #[test]
    fn writing_keeps_the_rest_of_the_file_and_its_comments() {
        let text = "# mine\n[worktree]\nfetch_interval = 60 # often\n";
        let updated =
            with_dotfiles_setting(text, &repo("https://x/d", Some("i.sh"))).expect("written");
        assert!(updated.starts_with(text), "{updated}");
        assert_eq!(
            parse_dotfiles_setting(&updated),
            DotfilesSetting::Set(repo("https://x/d", Some("i.sh")))
        );
        let (config, _) = parse_worktree_config(&updated, &WorktreeConfig::defaults())
            .expect("still a readable config");
        assert_eq!(config.fetch_interval, 60);
    }

    #[test]
    fn writing_never_replaces_a_url_somebody_set() {
        let text = "[dotfiles]\nurl = \"https://mine/d\"\n";
        assert!(with_dotfiles_setting(text, &repo("https://other/d", None)).is_err());
    }

    #[test]
    fn writing_the_same_url_again_is_a_no_op_not_a_refusal() {
        let text = "[dotfiles]\nurl = \"https://x/d\"\n";
        assert_eq!(
            with_dotfiles_setting(text, &repo("https://x/d", None)).as_deref(),
            Ok(text)
        );
    }

    #[test]
    fn writing_never_touches_a_file_it_cannot_parse() {
        assert!(with_dotfiles_setting("not = = toml", &repo("https://x/d", None)).is_err());
    }

    #[test]
    fn the_file_is_created_with_its_directory_when_there_is_none() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("devlaunch").join("config.toml");
        write_dotfiles_setting(&path, &repo("https://x/d", None)).expect("written");
        assert_eq!(
            dotfiles_setting_at(&path),
            DotfilesSetting::Set(repo("https://x/d", None))
        );
    }
}

#[cfg(test)]
mod tests {
    //! The Python `test_worktree_config`, re-pinned here when it retired with the
    //! Python tree (#267).
    //!
    //! Python's tests construct `WorktreeConfig(...)` directly and check
    //! `to_dict`; both are gone here, because nothing in devlaunch writes this
    //! file and `to_dict` had no caller outside those tests. What is left — the
    //! defaults, the nested tables, the silence about unknown keys, the path the
    //! file is looked for at — is all of it that a run depends on.

    use super::*;

    fn defaults() -> WorktreeConfig {
        WorktreeConfig::defaults()
    }

    /// The settings a text reads as, for the tests that are not about retirement.
    fn parse(text: &str) -> WorktreeConfig {
        parse_worktree_config(text, &defaults())
            .expect("readable configuration")
            .0
    }

    /// The retired keys a text still names.
    fn retired(text: &str) -> Vec<RetiredKey> {
        parse_worktree_config(text, &defaults())
            .expect("readable configuration")
            .1
    }

    #[test]
    fn the_defaults_are_the_documented_ones() {
        let config = defaults();

        assert!(
            config.enabled,
            "the worktree backend is on unless told not to"
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

    // --- the retired keys --------------------------------------------------

    #[test]
    fn a_config_still_naming_repos_dir_reports_the_directory_it_named() {
        // The one key retired *with* a notice, because it decided where the
        // clones went: a user who set it has a tree at that path, and nothing
        // else will ever name it.
        assert_eq!(
            retired("[worktree]\nrepos_dir = \"/srv/clones\"\n"),
            vec![RetiredKey::ReposDir {
                named: "/srv/clones".to_owned()
            }]
        );
    }

    #[test]
    fn the_value_is_reported_exactly_as_the_file_wrote_it() {
        // Quoted as data, like every other reason this module carries. Nothing
        // resolves it, so nothing can resolve it *wrongly* and report a
        // directory the user never typed.
        assert_eq!(
            retired("[worktree]\nrepos_dir = \"~/custom/repos\"\n"),
            vec![RetiredKey::ReposDir {
                named: "~/custom/repos".to_owned()
            }]
        );
    }

    #[test]
    fn a_config_naming_repos_dir_still_loads_with_its_other_keys() {
        // Reported, not refused: the module's whole property is that a stale
        // file is not punished.
        let config = parse("[worktree]\nrepos_dir = \"/srv/clones\"\nfetch_interval = 7200\n");

        assert_eq!(config.fetch_interval, 7200);
        assert_eq!(
            config,
            WorktreeConfig {
                fetch_interval: 7200,
                ..defaults()
            }
        );
    }

    #[test]
    fn a_config_that_does_not_name_it_reports_nothing() {
        assert!(retired("").is_empty());
        assert!(retired("[worktree]\nfetch_interval = 7200\n").is_empty());
        assert!(
            retired("[worktree]\nauto_fetch = false\n").is_empty(),
            "a key that gated nothing is still ignored in silence"
        );
    }

    #[test]
    fn text_that_is_not_toml_is_refused_with_the_parsers_reason() {
        let refused =
            parse_worktree_config("[worktree\nenabled = true", &defaults()).expect_err("not TOML");

        match refused {
            Malformed::NotToml { reason } => assert!(!reason.is_empty()),
            other => panic!("not TOML at all, got {other:?}"),
        }
    }

    #[test]
    fn a_value_of_the_wrong_type_is_refused_rather_than_carried() {
        // Python accepted this and failed later, in arithmetic; a parse step
        // exists so the refusal happens where the value is read — and the arm
        // says the file is TOML, so the reader knows which half to fix.
        for text in [
            "[worktree]\nfetch_interval = \"soon\"\n",
            "[worktree]\nenabled = \"yes\"\n",
            "[worktree.cleanup]\nprune_after_days = -1\n",
        ] {
            let refused = parse_worktree_config(text, &defaults()).expect_err(text);
            assert!(matches!(refused, Malformed::WrongType { .. }), "{text}");
        }
    }

    // --- the file on disk --------------------------------------------------

    #[test]
    fn no_file_at_all_is_the_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");

        let (config, retired) =
            worktree_config_at(&dir.path().join("config.toml"), &defaults()).expect("the defaults");

        assert_eq!(config, defaults());
        assert!(retired.is_empty());
    }

    #[test]
    fn a_file_is_read_from_the_path_it_is_at() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[worktree]\nfetch_interval = 60\n").expect("the fixture");

        let (config, _retired) = worktree_config_at(&path, &defaults()).expect("readable");

        assert_eq!(config.fetch_interval, 60);
    }

    #[test]
    fn a_file_on_disk_carries_its_retired_keys_out_of_the_load() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[worktree]\nrepos_dir = \"/srv/clones\"\n").expect("the fixture");

        let (_config, retired) = worktree_config_at(&path, &defaults()).expect("readable");

        assert_eq!(
            retired,
            vec![RetiredKey::ReposDir {
                named: "/srv/clones".to_owned()
            }]
        );
    }

    #[test]
    fn a_malformed_file_names_itself_in_the_refusal() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[worktree").expect("the fixture");

        let failed = worktree_config_at(&path, &defaults()).expect_err("not TOML");

        match failed {
            ConfigError::NotToml {
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
}
