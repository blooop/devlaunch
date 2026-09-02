//! Where the XDG base directories point on this machine.
//!
//! Two unrelated places ask for the config home: the worktree loader, which
//! reads `config.toml` under it, and the gh-token warning, which names it so a
//! user whose shell scoped the variable can see why `gh auth token` refused.
//! Those two have to agree — a warning that names one directory while the loader
//! reads another is worse than no warning — so they share the answer rather than
//! each spelling it.
//!
//! The cache home has the same problem and one more caller's worth of it. Three
//! places used to spell it out identically: dl's own cache directory, the clone
//! root, and the metadata file's default path. They have to agree because
//! `dl --purge` reads the first to decide which workspaces are devlaunch's —
//! workspaces whose clones the other two put on disk — so a copy that drifted
//! would make a purge silently stop recognising its own work.
//!
//! Unifying the *spelling* left one way for them to drift anyway: `config.toml`
//! could name a `repos_dir` outside the cache, which put dl's own clones
//! somewhere ownership does not recognise. That key is retired (#467), so the
//! clone root is now a function of the cache directory ([`clone_root_in`]) and
//! there is no input left that can separate them.
//!
//! Ported from `devlaunch/xdg.py`; see docs/rust-rewrite-plan.md (M2).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// This machine names no home directory, so a spec fallback cannot be built.
///
/// Python raises `RuntimeError` from `Path.home()` here. Returning it keeps the
/// functions total: the alternative — a relative `.config` — resolves against
/// the working directory, which is the defect the module exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoHomeDirectory;

/// `$XDG_CONFIG_HOME`, or the `~/.config` the spec falls back to.
pub fn config_home() -> Result<PathBuf, NoHomeDirectory> {
    resolve(
        std::env::var_os("XDG_CONFIG_HOME"),
        crate::osext::home_dir(),
        ".config",
    )
}

/// `$XDG_CACHE_HOME`, or the `~/.cache` the spec falls back to.
pub(crate) fn cache_home() -> Result<PathBuf, NoHomeDirectory> {
    resolve(
        std::env::var_os("XDG_CACHE_HOME"),
        crate::osext::home_dir(),
        ".cache",
    )
}

/// Everything devlaunch stores on this machine, under one directory.
///
/// The bare repo clones, the workspace clones, the completion caches and
/// `metadata.json` all live here, and `dl --purge` removes exactly this. One
/// function rather than a copy per caller, because a purge decides what is its
/// own to delete by asking whether a workspace's source is inside it.
pub fn devlaunch_cache() -> Result<PathBuf, NoHomeDirectory> {
    cache_home().map(|cache| devlaunch_cache_in(&cache))
}

fn devlaunch_cache_in(cache_home: &Path) -> PathBuf {
    cache_home.join("devlaunch")
}

/// Where named Claude profiles live: `~/.claude-profiles`, or wherever
/// `$CLAUDE_PROFILES_DIR` says.
///
/// **This is somebody else's directory and devlaunch only reads it.** The layout is
/// `claude-as`'s (a per-account `CLAUDE_CONFIG_DIR` per subdirectory, each holding the
/// `.credentials.json` a login writes), and the variable is `claude-as`'s too, so it
/// is honoured rather than set -- the arrangement [`super::super::clients::ssh::CONFIG_VAR`]
/// already has with devpod's own. Inventing a devlaunch-shaped root instead was the
/// first version of this and it was wrong: it made a *third* location for one concept
/// and would have asked a user with working profiles to log every account in again
/// somewhere new.
///
/// So there is no writer here, deliberately, matching [`super::config`]'s note about
/// `config.toml`. Creating a profile, seeding its shared config and deleting it belong
/// to whatever made the directory; devlaunch reads one file out of it.
///
/// **Nothing devlaunch deletes can reach it.** `dl --purge` removes
/// [`devlaunch_cache`] entire and `dl --prune` walks [`clone_root_in`] inside that, so
/// a login was never in their path and still is not:
/// `the_profiles_root_is_nowhere_a_purge_or_a_prune_can_reach` holds it.
///
/// `$DEVLAUNCH_CLAUDE_PROFILES_DIR` is devlaunch's own and wins over both, which is
/// what lets a test and a scratch run read and complete their own profiles instead of
/// the real credentials. An empty value counts as unset in every case, the rule every
/// variable here follows.
pub fn claude_profiles_root() -> Result<PathBuf, NoHomeDirectory> {
    if let Some(scoped) = non_empty(CLAUDE_PROFILES_DIR_VAR) {
        return Ok(PathBuf::from(scoped));
    }
    if let Some(theirs) = non_empty(FOREIGN_PROFILES_DIR_VAR) {
        return Ok(PathBuf::from(theirs));
    }
    crate::osext::home_dir()
        .map(|home| claude_profiles_root_in(&home))
        .ok_or(NoHomeDirectory)
}

/// The placement half of [`claude_profiles_root`], as a function of the home directory.
///
/// Split out for [`devlaunch_cache_in`]'s reason: the decision is then a function of
/// its input, so a test can state the machine it means instead of mutating an
/// environment every other test in the binary shares.
fn claude_profiles_root_in(home: &Path) -> PathBuf {
    home.join(CLAUDE_PROFILES_LEAF)
}

/// A variable's value, if it has one that is not empty.
fn non_empty(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// devlaunch's own override, which scopes a scratch run away from real credentials.
const CLAUDE_PROFILES_DIR_VAR: &str = "DEVLAUNCH_CLAUDE_PROFILES_DIR";

/// `claude-as`'s own variable for the same directory, honoured rather than set.
const FOREIGN_PROFILES_DIR_VAR: &str = "CLAUDE_PROFILES_DIR";

/// The leaf [`claude_profiles_root`] ends in under `$HOME`, named once.
const CLAUDE_PROFILES_LEAF: &str = ".claude-profiles";

/// The one directory devlaunch clones into, under the cache directory ownership
/// is decided by.
///
/// A function of `cache` and of nothing else, which is the whole point: every
/// clone dl makes is inside the directory
/// [`is_devlaunch_clone`](crate::flows::listing) tests against, so a clone of
/// dl's own that the listing reads as someone else's has no representation. It
/// used to be configurable (`worktree.repos_dir`), and a value outside the cache
/// took `devlaunch`, `SIZE` and `unsaved` out together.
///
/// Takes the cache directory rather than resolving one, so a caller cannot scan
/// under one cache while deciding ownership against another.
pub fn clone_root_in(cache: &Path) -> PathBuf {
    cache.join("repos")
}

/// The whole XDG rule, as a function of its two inputs.
///
/// An empty value counts as unset, which is what the XDG basedir spec says and
/// what a shell exporting the variable with no value means. Reading it any other
/// way resolves the path relative to the working directory instead.
fn resolve(
    variable: Option<OsString>,
    home: Option<PathBuf>,
    leaf: &str,
) -> Result<PathBuf, NoHomeDirectory> {
    match variable {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        _ => home.map(|home| home.join(leaf)).ok_or(NoHomeDirectory),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    fn home() -> PathBuf {
        PathBuf::from("/home/someone")
    }

    #[test]
    fn config_home_is_the_variable_when_it_is_set() {
        assert_eq!(
            resolve(Some(OsString::from("/xdg/config")), Some(home()), ".config"),
            Ok(PathBuf::from("/xdg/config"))
        );
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        assert_eq!(
            resolve(Some(OsString::new()), Some(home()), ".config"),
            Ok(PathBuf::from("/home/someone/.config"))
        );
    }

    #[test]
    fn an_absent_variable_falls_back_to_the_home_leaf() {
        assert_eq!(
            resolve(None, Some(home()), ".cache"),
            Ok(PathBuf::from("/home/someone/.cache"))
        );
    }

    #[test]
    fn no_home_is_an_error_rather_than_a_relative_path() {
        assert_eq!(resolve(None, None, ".config"), Err(NoHomeDirectory));
        assert_eq!(
            resolve(Some(OsString::new()), None, ".cache"),
            Err(NoHomeDirectory)
        );
    }

    #[test]
    fn a_set_variable_needs_no_home() {
        assert_eq!(
            resolve(Some(OsString::from("/xdg/cache")), None, ".cache"),
            Ok(PathBuf::from("/xdg/cache"))
        );
    }

    #[test]
    fn a_non_utf8_variable_still_names_a_directory() {
        // Python reads the environment with surrogateescape, so a path that is not
        // UTF-8 still resolves. `var_os` keeps the bytes; `var` would have dropped
        // the whole directory on the floor.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let raw = OsString::from_vec(vec![b'/', 0xff, b'x']);
            assert_eq!(
                resolve(Some(raw.clone()), Some(home()), ".cache"),
                Ok(PathBuf::from(raw))
            );
        }
    }

    #[test]
    fn the_profiles_root_is_nowhere_a_purge_or_a_prune_can_reach() {
        // Mandatory rather than incidental. `dl --purge` deletes `devlaunch_cache()`
        // entire and `dl --prune` walks `clone_root_in` inside it, so a credential
        // under either is one flag away from deletion and nothing regenerates a
        // login. This asserts the placement rather than trusting whoever moves it
        // next.
        let profiles = claude_profiles_root_in(Path::new("/h"));
        let devlaunch = devlaunch_cache_in(Path::new("/k"));
        let clones = clone_root_in(&devlaunch);
        assert!(
            !profiles.starts_with(&devlaunch),
            "{} is inside {}",
            profiles.display(),
            devlaunch.display()
        );
        assert!(!profiles.starts_with(&clones));
        // And it is `claude-as`'s directory, not one devlaunch invented: a second
        // location for one concept would ask a user with working profiles to log
        // every account in again somewhere new.
        assert_eq!(profiles, PathBuf::from("/h/.claude-profiles"));
    }

    #[test]
    fn devlaunch_cache_is_one_directory_under_the_cache_home() {
        assert_eq!(
            devlaunch_cache_in(Path::new("/xdg/cache")),
            PathBuf::from("/xdg/cache/devlaunch")
        );
    }

    #[test]
    fn the_real_environment_resolves_to_absolute_directories() {
        // Read-only: the process environment is shared with every other test in the
        // binary, so these observe it rather than mutating it. Everything the port
        // has to get right about the XDG rules is decided by `resolve`, which is a
        // pure function of the variable and the home directory.
        for got in [config_home(), cache_home(), devlaunch_cache()] {
            let path = got.expect("this machine has a home directory");
            assert!(path.is_absolute(), "{path:?}");
        }
        assert_eq!(devlaunch_cache(), cache_home().map(|c| c.join("devlaunch")),);
    }
}
