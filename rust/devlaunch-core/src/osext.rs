//! The process boundary, ported to match Python's `os`/`posixpath` rather than
//! Rust std's defaults.
//!
//! A leaf like `runner`: it depends on nothing else in the crate, and every
//! layer above may read the host through it. Its whole reason to exist is that
//! `std::env::var`, `std::env::home_dir`, and `std::env::temp_dir` each diverge
//! from the Python `dl` at an input boundary in a way that costs correctness or
//! a credential:
//!
//! Only [`env_str`] is linked below, because only [`env_str`] is `pub`. The
//! module is binary surface for that one reader; the other three stayed
//! `pub(crate)` (see the note on `pub mod osext` in `lib.rs`). A link from a
//! `pub` module's doc to a `pub(crate)` item renders as plain text whatever the
//! brackets say, and costs a `rustdoc::private_intra_doc_links` warning for the
//! privilege, so the brackets are off rather than pointing at nothing.
//!
//! - [`env_str`] reads a variable the way `os.environ.get` does — a non-UTF-8
//!   value is *present*, not absent. `std::env::var(..).ok()` reports a non-UTF-8
//!   value as unset, which turns an opt-out (`DEVLAUNCH_NO_GH_TOKEN`) into an
//!   opt-in and forwards a credential the user asked to withhold.
//! - `strip` trims the exact set `str.strip()` trims. Rust's `str::trim` uses
//!   the Unicode `White_Space` property, which omits U+001C–U+001F; Python's
//!   `str.isspace()` includes them. Those four codepoints otherwise invert every
//!   `DEVLAUNCH_*` switch and reject an otherwise-valid `GH_TOKEN`.
//! - `home_dir` matches `posixpath.expanduser("~")`: a present-but-empty `HOME`
//!   expands to `/`, and only an *absent* `HOME` consults the password database.
//!   `std::env::home_dir` treats empty and absent alike (both fall to the passwd
//!   entry), which reaches the real home when the caller cleared `HOME`.
//! - `temp_dir` validates the directory and honours `TMPDIR`/`TEMP`/`TMP` with
//!   the `/tmp` family as the fallback, the way `tempfile.gettempdir()` does. A
//!   non-existent `TMPDIR` otherwise makes the token-staging file fail to create
//!   and silently costs the workspace its gh login.
//! - `system_words` reads a failure the way `OSError.strerror` gives it.
//!   `std::io::Error`'s `Display` appends `" (os error {errno})"`, which is the
//!   errno repeated at a person who is being shown the path it happened to.

use std::ffi::OsString;
use std::path::PathBuf;

/// Read an environment variable as a string, lossily, the way `os.environ.get`
/// does: a value that is present but not valid UTF-8 is `Some` (with U+FFFD for
/// the undecodable bytes), never `None`.
///
/// binary surface -- not part of the frozen wf API (#251 section 7)
///
/// The one item of this module the binaries can see, because they have
/// `DEVLAUNCH_*` variables of their own and no way to spell this reading
/// correctly for themselves: `aid` read `DEVLAUNCH_AID_AGENT` with
/// `std::env::var(..).ok()` and so started the default agent for a value it
/// should have refused by name. It reaches this through `dl`, which re-exports
/// it beside `shell` and `python_repr`.
pub fn env_str(name: &str) -> Option<String> {
    from_os(std::env::var_os(name))
}

/// The lossy-decode half of [`env_str`], split out so a test can hand it a
/// non-UTF-8 value without mutating the process environment every other test in
/// the binary shares.
fn from_os(value: Option<OsString>) -> Option<String> {
    value.map(|value| value.to_string_lossy().into_owned())
}

/// Trim leading and trailing whitespace, matching Python's `str.strip()`.
///
/// The set is `char::is_whitespace()` plus U+001C–U+001F (file/group/record/unit
/// separators), which Python's `str.isspace()` counts and the Unicode
/// `White_Space` property that backs `str::trim` does not.
pub(crate) fn strip(value: &str) -> &str {
    value.trim_matches(is_python_space)
}

/// One codepoint's worth of the [`strip`] predicate.
fn is_python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// The home directory, matching `posixpath.expanduser("~")` / `pathlib.Path.home()`.
///
/// A present `HOME` — even empty — wins: its trailing slashes are stripped and an
/// empty result becomes `/`, exactly as `expanduser` does. Only an absent `HOME`
/// consults the password database.
pub(crate) fn home_dir() -> Option<PathBuf> {
    home_from(std::env::var_os("HOME"), std::env::home_dir)
}

/// The pure core of [`home_dir`]: `present` is what `$HOME` holds (or its
/// absence), `passwd` is consulted only when `$HOME` is absent. Split out so the
/// three [`home_dir`] cases can be pinned without mutating the environment.
fn home_from(
    present: Option<OsString>,
    passwd: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    match present {
        Some(home) => {
            let home = home.to_string_lossy();
            let trimmed = home.trim_end_matches('/');
            Some(PathBuf::from(if trimmed.is_empty() {
                "/"
            } else {
                trimmed
            }))
        }
        None => passwd(),
    }
}

/// A usable temporary directory, matching `tempfile.gettempdir()`.
///
/// `TMPDIR`, `TEMP`, then `TMP` (each only if non-empty), then `/tmp`,
/// `/var/tmp`, `/usr/tmp`, then the current directory — the first that exists and
/// accepts a file. A directory that does not exist or cannot be written is
/// skipped rather than returned, so a stale `TMPDIR` cannot cost the caller its
/// write.
pub(crate) fn temp_dir() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for name in ["TMPDIR", "TEMP", "TMP"] {
        if let Some(value) = env_str(name)
            && !value.is_empty()
        {
            candidates.push(PathBuf::from(value));
        }
    }
    candidates.push(PathBuf::from("/tmp"));
    candidates.push(PathBuf::from("/var/tmp"));
    candidates.push(PathBuf::from("/usr/tmp"));
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    pick_temp_dir(&candidates).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// The first candidate that exists and accepts a probe file. Split from
/// [`temp_dir`] so the validation can be pinned over real directories.
fn pick_temp_dir(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|dir| usable(dir.as_path())).cloned()
}

/// Whether a temp file can actually be created in `dir` — the same test
/// `gettempdir` makes, which is why a non-existent `TMPDIR` falls through.
fn usable(dir: &std::path::Path) -> bool {
    dir.is_dir()
        && tempfile::Builder::new()
            .prefix(".devlaunch-probe-")
            .tempfile_in(dir)
            .is_ok()
}

/// The system's own words for a failure, as Python's `OSError.strerror` gives
/// them.
///
/// [`std::io::Error`]'s `Display` is `"{strerror} (os error {errno})"` for an OS
/// error, and the errno is already carried by the arm that holds this string —
/// where it is carried at all. So the suffix is dropped: the reason is printed to
/// a person beside the path it is about, and `Permission denied` is the whole of
/// what they need from it.
///
/// It lives here rather than beside its first caller because it is a reading of
/// the host, like everything else in this module, and because a leaf is the only
/// place both a flow (`flows::repo_manager`) and a client
/// (`clients::devpod_home`) may name: the crate's layers run strictly downward,
/// and a client never names a flow.
pub(crate) fn system_words(error: &std::io::Error) -> String {
    let text = error.to_string();
    match error.raw_os_error() {
        Some(errno) => text
            .strip_suffix(&format!(" (os error {errno})"))
            .unwrap_or(&text)
            .to_owned(),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn a_non_utf8_value_is_present_not_absent() {
        // DEVLAUNCH_NO_GH_TOKEN="1\xff": a user opted out. `var().ok()` would
        // read this as unset and forward the token anyway.
        let raw = OsStr::from_bytes(b"1\xff").to_owned();
        let read = from_os(Some(raw)).expect("a present value stays present");
        assert!(!read.is_empty(), "the opt-out value must not vanish");
    }

    #[test]
    fn an_absent_value_is_none() {
        assert_eq!(from_os(None), None);
    }

    #[test]
    fn strip_removes_the_four_control_separators() {
        // The exact codepoints str.strip() removes and str::trim does not.
        for c in ['\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}'] {
            let raw = format!("{c}1{c}");
            assert_eq!(strip(&raw), "1", "U+{:04X} must strip", c as u32);
            // Guard the premise: std's trim leaves these in place.
            assert_ne!(raw.trim(), "1");
        }
    }

    #[test]
    fn strip_still_removes_ordinary_whitespace() {
        assert_eq!(strip("  x\t\n"), "x");
    }

    #[test]
    fn empty_home_expands_to_root() {
        // posixpath.expanduser("~") with HOME="" is "/", not the passwd home.
        let got = home_from(Some(OsString::from("")), || {
            panic!("passwd must not be consulted when HOME is present")
        });
        assert_eq!(got, Some(PathBuf::from("/")));
    }

    #[test]
    fn present_home_has_trailing_slashes_stripped() {
        let got = home_from(Some(OsString::from("/home/x/")), || None);
        assert_eq!(got, Some(PathBuf::from("/home/x")));
    }

    #[test]
    fn absent_home_consults_passwd() {
        let got = home_from(None, || Some(PathBuf::from("/home/from-passwd")));
        assert_eq!(got, Some(PathBuf::from("/home/from-passwd")));
    }

    #[test]
    fn a_nonexistent_first_candidate_is_skipped() {
        // The P8 credential case: a stale TMPDIR must not be returned; the real
        // directory behind it must be.
        let real = tempfile::tempdir().expect("a real temp dir");
        let stale = real.path().join("does-not-exist");
        let picked = pick_temp_dir(&[stale, real.path().to_path_buf()]);
        assert_eq!(picked.as_deref(), Some(real.path()));
    }
}
