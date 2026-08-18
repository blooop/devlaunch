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
//! places used to spell it out identically: dl's own cache directory, the
//! worktree config's default `repos_dir`, and the metadata file's default path.
//! They have to agree because `dl --purge` reads the first to decide which
//! workspaces are devlaunch's — workspaces whose clones the other two put on
//! disk — so a copy that drifted would make a purge silently stop recognising
//! its own work.
//!
//! Ported from `devlaunch/xdg.py`; see docs/rust-rewrite-plan.md (M2).

// Callers land in M4 (storage flows) onward; until then the port's own tests
// are the only consumers of this module.
#![allow(dead_code)]

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
        std::env::home_dir(),
        ".config",
    )
}

/// `$XDG_CACHE_HOME`, or the `~/.cache` the spec falls back to.
pub(crate) fn cache_home() -> Result<PathBuf, NoHomeDirectory> {
    resolve(
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::home_dir(),
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
