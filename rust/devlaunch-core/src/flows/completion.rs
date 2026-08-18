//! Installing shell autocompletion: the rc-file edit, and the script it sources.
//!
//! `dl --install` writes two files and only two. The first is the completion
//! script itself — the shipped bash payload, copied to
//! `~/.config/devlaunch/completions.sh` (or wherever `DEVLAUNCH_COMPLETION_FILE`
//! says). The second is the user's rc file, which gets one three-line block that
//! sources the first:
//!
//! ```text
//! # >>> devlaunch completions >>>
//! source "/home/someone/.config/devlaunch/completions.sh"
//! # <<< devlaunch completions <<<
//! ```
//!
//! **The install is idempotent because it removes before it adds.** Every
//! devlaunch block — this one and the two shapes earlier versions wrote — is
//! stripped from the rc file first, so running `dl --install` twice leaves one
//! block rather than two, and running it after an upgrade replaces the old shape
//! instead of stacking on it. That is also what makes a changed script path take
//! effect: the block is rewritten, not appended to.
//!
//! Ported from `devlaunch/completion.py` and `devlaunch/completion_loader.py`;
//! see docs/rust-rewrite-plan.md (M5).
//!
//! # Three seams worth naming
//!
//! - **The payload is shared, not copied.** [`DL_COMPLETION_BASH`] is
//!   `include_str!` of `devlaunch/completions/dl.bash` — the same file the Python
//!   package ships — so while both binaries exist there is one copy of the bash
//!   and they cannot drift. At cutover the file moves under `rust/` and only this
//!   one path changes; nothing else here knows where it came from.
//! - **What the payload reads is not this module's business.** The script sources
//!   `${XDG_CACHE_HOME:-$HOME/.cache}/devlaunch/completions.bash` at completion
//!   time, and *writing* that cache is the listing side's job (Python's
//!   `dl.py`, ported in M5b). `dl --install` warms it before calling in here;
//!   this module would install a working block over an empty cache, which is
//!   what makes the completion still offer flags on a first run.
//! - **Nothing here prints or exits.** Python logged, printed one sentence, and
//!   answered `0` or `1`. Here the outcome is [`Installed`] — which says what the
//!   install actually did — or a typed [`InstallError`], and the words and the
//!   exit code belong to the binary.

// The `--install` path in the binary lands in M5b.
#![allow(dead_code)] // consumed from M5b on

use std::io;
use std::path::{Path, PathBuf};

use crate::domain::xdg::NoHomeDirectory;

/// The shipped bash completion payload, embedded at compile time.
///
/// One source of truth: the file the Python package ships. Python resolved it as
/// package data at runtime (`importlib.resources`), which could fail; embedding
/// it removes that failure entirely — a build that compiled has the script.
pub(crate) const DL_COMPLETION_BASH: &str =
    include_str!("../../../../devlaunch/completions/dl.bash");

/// The override for where the completion script is written.
pub(crate) const COMPLETION_FILE_VAR: &str = "DEVLAUNCH_COMPLETION_FILE";

/// The first line of the block this installs, and the marker it finds an earlier
/// install by.
const BLOCK_START: &str = "# >>> devlaunch completions >>>";

/// The block's last line.
const BLOCK_END: &str = "# <<< devlaunch completions <<<";

/// Block shapes earlier versions wrote, cleaned up on the way past.
const LEGACY_BLOCKS: [(&str, &str); 2] = [
    ("# dl completion", "# end dl completion"),
    ("# dp completion", "# end dp completion"),
];

/// Lines an even earlier version appended on their own, with no block around
/// them. Matched as a prefix, because they were written with and without
/// trailing junk.
const LEGACY_LINES: [&str; 2] = [
    "complete -F _dl_completion dl",
    "complete -F _dp_completion dp",
];

/// What an install did to one file.
///
/// Reported rather than assumed, because "already installed" is the ordinary
/// outcome — a user who runs `dl --install` again, or an upgrade whose payload did
/// not change — and a file rewritten with identical bytes is not something to
/// announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// The file's content was not what it should be, and has been replaced.
    Written,
    /// The file already held exactly this content; nothing was written.
    AlreadyCurrent,
}

impl FileState {
    /// The state of a file that should hold `wanted` and held `found`.
    fn of(found: Option<&str>, wanted: &str) -> Self {
        if found == Some(wanted) {
            FileState::AlreadyCurrent
        } else {
            FileState::Written
        }
    }
}

/// What the rc file looked like before the install, and so what the install did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcChange {
    /// The rc file already carried exactly this block; it is untouched.
    AlreadyInstalled,
    /// No devlaunch block was there, and one has been appended.
    Added,
    /// A devlaunch block was there and has been replaced — an earlier shape, or
    /// the same shape naming a different script path.
    Refreshed,
}

/// What one install left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Where the completion script now is.
    pub script: PathBuf,
    pub script_state: FileState,
    /// The rc file that sources it — what the binary tells the user to `source`.
    pub rc: PathBuf,
    pub rc_change: RcChange,
}

/// Which step of installing failed, and what the OS said about it.
///
/// Python collapsed every `OSError` into one logged line and exit 1. The steps
/// are separated here because they fail for different reasons and only the binary
/// can decide which of them is worth a user's attention.
#[derive(Debug)]
pub enum InstallError {
    /// This machine names no home directory, so no default path can be built.
    NoHomeDirectory,
    /// A parent directory of one of the two files could not be created.
    CreateDirectory { path: PathBuf, source: io::Error },
    /// The completion script could not be written.
    WriteScript { path: PathBuf, source: io::Error },
    /// The rc file exists but could not be read — so it cannot be edited either,
    /// and overwriting it would delete whatever is in it.
    ReadRc { path: PathBuf, source: io::Error },
    /// The rc file could not be written.
    WriteRc { path: PathBuf, source: io::Error },
}

impl From<NoHomeDirectory> for InstallError {
    fn from(_: NoHomeDirectory) -> Self {
        InstallError::NoHomeDirectory
    }
}

/// Install or refresh the completions, with `rc_path` if the caller named one.
///
/// The default rc file is `~/.bashrc`; a caller that names another one gets it
/// used as given, which is what `dl --install <path>` is for.
pub fn install(rc_path: Option<&Path>) -> Result<Installed, InstallError> {
    let script = completion_file_path()?;
    let rc = match rc_path {
        Some(named) => expand_tilde(named),
        None => default_rc_path()?,
    };
    install_into(&script, &rc)
}

/// [`install`], against two paths already decided.
///
/// The whole install with no environment left in it, which is what the tests
/// drive: every rule about *which* files are edited is in the two resolvers
/// above, and every rule about *what is written* is in here.
pub(crate) fn install_into(script: &Path, rc: &Path) -> Result<Installed, InstallError> {
    ensure_parent(script)?;
    let script_state = write_if_changed(script, &completion_script()).map_err(|source| {
        InstallError::WriteScript {
            path: script.to_path_buf(),
            source,
        }
    })?;

    // The rc file's directory first, so a path that cannot hold a file at all
    // says which directory refused rather than reporting a read that never had a
    // chance — and so the read below is only ever about the file itself.
    ensure_parent(rc)?;
    let existing = read_if_there(rc).map_err(|source| InstallError::ReadRc {
        path: rc.to_path_buf(),
        source,
    })?;
    let wanted = rc_with_block(existing.as_deref().unwrap_or(""), script);
    let rc_change = change_from(existing.as_deref(), &wanted);
    write_if_changed(rc, &wanted).map_err(|source| InstallError::WriteRc {
        path: rc.to_path_buf(),
        source,
    })?;

    Ok(Installed {
        script: script.to_path_buf(),
        script_state,
        rc: rc.to_path_buf(),
        rc_change,
    })
}

/// The text of the completion script, as it is written to disk.
///
/// Exactly one trailing newline, whatever the shipped file ends with, so a
/// re-install of an unchanged payload is byte-identical.
///
/// Python's loader took the script's *name* and resolved it as package data,
/// which could fail for a name nothing ships. One script ships, so there is no
/// name to get wrong and no failure to handle.
pub(crate) fn completion_script() -> String {
    format!("{}\n", DL_COMPLETION_BASH.trim_end())
}

/// Where the completion script is written on this machine.
pub(crate) fn completion_file_path() -> Result<PathBuf, NoHomeDirectory> {
    completion_file_in(
        std::env::var(COMPLETION_FILE_VAR).ok().as_deref(),
        std::env::home_dir(),
    )
}

/// The whole rule, as a function of its two inputs.
///
/// `~/.config/devlaunch/completions.sh` — the home directory rather than
/// `$XDG_CONFIG_HOME`, which is what Python's `Path.home() / ".config"` does.
/// Reading the XDG variable here would move the file for everyone who sets it,
/// and their rc files still source the old path.
pub(crate) fn completion_file_in(
    override_path: Option<&str>,
    home: Option<PathBuf>,
) -> Result<PathBuf, NoHomeDirectory> {
    match override_path {
        // Empty counts as unset, which is what a shell exporting the variable
        // with no value means — and what Python's falsy check did.
        Some(named) if !named.is_empty() => Ok(expand_tilde(Path::new(named))),
        _ => home
            .map(|home| {
                home.join(".config")
                    .join("devlaunch")
                    .join("completions.sh")
            })
            .ok_or(NoHomeDirectory),
    }
}

/// The rc file an install edits when the caller names none.
pub(crate) fn default_rc_path() -> Result<PathBuf, NoHomeDirectory> {
    std::env::home_dir()
        .map(|home| home.join(".bashrc"))
        .ok_or(NoHomeDirectory)
}

/// The rc file's content after this install: every devlaunch block and legacy
/// line removed, and the current block appended.
///
/// Pure, and the only place the file's shape is decided. Whatever else is in the
/// rc file is kept, in order, with one blank line between it and the block.
pub(crate) fn rc_with_block(existing: &str, script: &Path) -> String {
    let mut cleaned = without_block(existing, BLOCK_START, BLOCK_END);
    for (start, end) in LEGACY_BLOCKS {
        cleaned = without_block(&cleaned, start, end);
    }
    let kept: Vec<&str> = cleaned
        .lines()
        .filter(|line| {
            let line = line.trim();
            !LEGACY_LINES.iter().any(|legacy| line.starts_with(legacy))
        })
        .collect();
    let kept = kept.join("\n");
    let kept = kept.trim();
    let block = block_for(script);
    if kept.is_empty() {
        format!("{block}\n")
    } else {
        format!("{kept}\n\n{block}\n")
    }
}

/// The three lines this install owns.
fn block_for(script: &Path) -> String {
    format!("{BLOCK_START}\n{}\n{BLOCK_END}", source_line(script))
}

/// The line that makes an interactive bash load the script.
///
/// Quoted, so a path with a space in it still sources, and any `"` in the path is
/// escaped rather than ending the string early. A path that is not UTF-8 is
/// written as the replacement characters `to_string_lossy` gives it: an rc file is
/// text a shell parses, and there is no byte-exact spelling of it that bash would
/// read back.
pub(crate) fn source_line(script: &Path) -> String {
    format!(
        r#"source "{}""#,
        script.to_string_lossy().replace('"', "\\\"")
    )
}

/// `text` with every line of the block from `start` to `end` removed.
///
/// A `start` with no `end` after it takes the rest of the file with it. That is
/// the recovery path for a block someone truncated by hand: the alternative is to
/// leave half a block behind and write a second one after it, and a half block
/// whose `source` line is gone does nothing while looking installed.
fn without_block(text: &str, start: &str, end: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        let marker = line.trim();
        if skipping {
            if marker == end {
                skipping = false;
            }
            continue;
        }
        if marker == start {
            skipping = true;
            continue;
        }
        kept.push(line);
    }
    kept.join("\n")
}

/// Which of the three things this install is doing to the rc file.
fn change_from(existing: Option<&str>, wanted: &str) -> RcChange {
    let existing = existing.unwrap_or("");
    if existing == wanted {
        return RcChange::AlreadyInstalled;
    }
    let had_devlaunch = existing.lines().any(|line| {
        let line = line.trim();
        line == BLOCK_START
            || LEGACY_BLOCKS.iter().any(|(start, _)| line == *start)
            || LEGACY_LINES.iter().any(|legacy| line.starts_with(legacy))
    });
    if had_devlaunch {
        RcChange::Refreshed
    } else {
        RcChange::Added
    }
}

/// The file's content, or `None` if there is no file there.
fn read_if_there(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Create the directory `path` will live in, if it names one that is not there.
fn ensure_parent(path: &Path) -> Result<(), InstallError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| InstallError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

/// Write `wanted` to `path` unless it is already exactly that.
///
/// Not written when it is already there, so a re-install leaves the file's
/// timestamps alone — and [`FileState`] is how the caller learns which happened.
/// A file that cannot be read is a file whose content is unknown, which is a
/// difference: it gets written.
fn write_if_changed(path: &Path, wanted: &str) -> io::Result<FileState> {
    let found = read_if_there(path).ok().flatten();
    let state = FileState::of(found.as_deref(), wanted);
    if state == FileState::Written {
        std::fs::write(path, wanted)?;
    }
    Ok(state)
}

/// `~` and `~/…` against the home directory, as Python's `expanduser` reads them.
///
/// `~user` is left alone: resolving another user's home needs the password
/// database, and an rc file or completion path naming somebody else's home is not
/// a case devlaunch has. The same rule as `domain::config`'s, which is private to
/// that module; the two collapse into one helper when the last Python caller goes.
fn expand_tilde(path: &Path) -> PathBuf {
    let Some(raw) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(rest) = raw.strip_prefix('~') else {
        return path.to_path_buf();
    };
    let Some(home) = std::env::home_dir() else {
        return path.to_path_buf();
    };
    match rest {
        "" => home,
        rest => match rest.strip_prefix('/') {
            Some(relative) => home.join(relative),
            None => path.to_path_buf(),
        },
    }
}

#[cfg(test)]
mod tests {
    //! What the install side of the completions pins.
    //!
    //! `test/test_bash_completion.py` is about the *payload* — it sources
    //! `dl.bash` in a real bash and drives `_dl_completion` against a cache file
    //! — so it is language-agnostic and keeps passing unchanged: the file it
    //! sources is the file [`DL_COMPLETION_BASH`] embeds. Nothing in it touches
    //! `install_completions`, which is why the install has its own tests here and
    //! that file's ledger row is about the bash.
    //!
    //! The boundary drawn: this module writes the block and the script. Writing
    //! the *cache* the script reads at completion time
    //! (`$XDG_CACHE_HOME/devlaunch/completions.bash`, and the JSON beside it) is
    //! the listing side's, and lands with `--completion-data` in M5b. The one
    //! test here that looks at the payload's text is the pin on that seam.

    use super::*;
    use std::fs;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temp dir")
    }

    fn rc_of(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("bashrc")
    }

    fn script_of(dir: &tempfile::TempDir) -> PathBuf {
        dir.path()
            .join("config")
            .join("devlaunch")
            .join("completions.sh")
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("the file this install wrote")
    }

    fn blocks_in(text: &str) -> usize {
        text.lines()
            .filter(|line| line.trim() == BLOCK_START)
            .count()
    }

    // --- the two files an install writes ----------------------------------

    #[test]
    fn an_install_writes_the_script_and_a_block_that_sources_it() {
        let dir = temp_dir();
        let script = script_of(&dir);
        let rc = rc_of(&dir);

        let installed = install_into(&script, &rc).expect("the install");

        assert_eq!(installed.script_state, FileState::Written);
        assert_eq!(installed.rc_change, RcChange::Added);
        assert_eq!(read(&script), completion_script());
        assert_eq!(
            read(&rc),
            format!(
                "{BLOCK_START}\nsource \"{}\"\n{BLOCK_END}\n",
                script.display()
            )
        );
    }

    #[test]
    fn the_script_directory_and_the_rc_directory_are_created_on_demand() {
        // A first install on a fresh machine has neither.
        let dir = temp_dir();
        let script = dir.path().join("a").join("b").join("completions.sh");
        let rc = dir.path().join("c").join("bashrc");

        install_into(&script, &rc).expect("the install");

        assert!(script.exists() && rc.exists());
    }

    #[test]
    fn the_script_is_the_shipped_payload_with_one_trailing_newline() {
        let written = completion_script();

        assert!(written.starts_with(DL_COMPLETION_BASH.trim_end()));
        assert!(written.ends_with('\n') && !written.ends_with("\n\n"));
        assert!(
            written.contains("_dl_completion()"),
            "the payload defines the function bash will call"
        );
    }

    #[test]
    fn the_payload_reads_the_cache_the_listing_side_writes() {
        // The seam to M5b, pinned from this side: whatever writes
        // `completions.bash` has to put it exactly here, because the shipped
        // script sources that path and nothing negotiates it at runtime.
        assert!(
            DL_COMPLETION_BASH.contains(r#"${XDG_CACHE_HOME:-$HOME/.cache}/devlaunch"#),
            "the payload's cache directory moved"
        );
        assert!(DL_COMPLETION_BASH.contains("completions.bash"));
    }

    // --- idempotence ------------------------------------------------------

    #[test]
    fn installing_twice_leaves_one_block_and_says_it_was_already_there() {
        let dir = temp_dir();
        let script = script_of(&dir);
        let rc = rc_of(&dir);

        install_into(&script, &rc).expect("the first install");
        let after = read(&rc);
        let again = install_into(&script, &rc).expect("the second install");

        assert_eq!(again.rc_change, RcChange::AlreadyInstalled);
        assert_eq!(again.script_state, FileState::AlreadyCurrent);
        assert_eq!(read(&rc), after, "the rc file is untouched");
        assert_eq!(blocks_in(&read(&rc)), 1);
    }

    #[test]
    fn a_block_naming_another_script_is_replaced_rather_than_joined() {
        // What makes a moved completion file take effect: the block is rewritten.
        let dir = temp_dir();
        let rc = rc_of(&dir);
        fs::write(
            &rc,
            format!("{BLOCK_START}\nsource \"/gone/completions.sh\"\n{BLOCK_END}\n"),
        )
        .expect("an rc file with a stale block");
        let script = script_of(&dir);

        let installed = install_into(&script, &rc).expect("the install");

        let after = read(&rc);
        assert_eq!(installed.rc_change, RcChange::Refreshed);
        assert_eq!(blocks_in(&after), 1);
        assert!(!after.contains("/gone/completions.sh"), "{after}");
        assert!(after.contains(&source_line(&script)), "{after}");
    }

    #[test]
    fn whatever_else_is_in_the_rc_file_is_kept_above_the_block() {
        let dir = temp_dir();
        let rc = rc_of(&dir);
        fs::write(&rc, "export EDITOR=vi\nalias ll='ls -l'\n").expect("an rc file");
        let script = script_of(&dir);

        install_into(&script, &rc).expect("the install");

        let after = read(&rc);
        assert!(
            after.starts_with("export EDITOR=vi\nalias ll='ls -l'\n\n"),
            "{after}"
        );
        assert!(after.ends_with(&format!("{BLOCK_END}\n")), "{after}");
    }

    #[test]
    fn an_rc_file_that_is_not_there_is_created_holding_only_the_block() {
        let dir = temp_dir();
        let rc = rc_of(&dir);
        assert!(!rc.exists());

        let installed = install_into(&script_of(&dir), &rc).expect("the install");

        assert_eq!(installed.rc_change, RcChange::Added);
        assert_eq!(read(&rc).lines().count(), 3);
    }

    // --- the shapes earlier versions left behind ---------------------------

    #[test]
    fn a_legacy_block_is_removed_rather_than_left_beside_the_new_one() {
        let dir = temp_dir();
        let rc = rc_of(&dir);
        fs::write(
            &rc,
            "# dl completion\ncomplete -F _dl_completion dl\n# end dl completion\n\
             # dp completion\ncomplete -F _dp_completion dp\n# end dp completion\n",
        )
        .expect("an rc file from an older install");

        let installed = install_into(&script_of(&dir), &rc).expect("the install");

        let after = read(&rc);
        assert_eq!(installed.rc_change, RcChange::Refreshed);
        assert!(!after.contains("_dl_completion"), "{after}");
        assert!(!after.contains("# dp completion"), "{after}");
        assert_eq!(blocks_in(&after), 1);
    }

    #[test]
    fn a_legacy_line_with_no_block_around_it_is_removed_too() {
        let dir = temp_dir();
        let rc = rc_of(&dir);
        fs::write(
            &rc,
            "export EDITOR=vi\ncomplete -F _dl_completion dl\n  complete -F _dp_completion dp\n",
        )
        .expect("an rc file from an even older install");

        let installed = install_into(&script_of(&dir), &rc).expect("the install");

        let after = read(&rc);
        assert_eq!(installed.rc_change, RcChange::Refreshed);
        assert!(after.starts_with("export EDITOR=vi\n\n"), "{after}");
        assert!(!after.contains("complete -F"), "{after}");
    }

    #[test]
    fn a_block_someone_truncated_takes_the_rest_of_the_file_with_it() {
        // A start marker with no end is not a block, and leaving half of one
        // behind would look installed while sourcing nothing.
        let script = Path::new("/home/someone/.config/devlaunch/completions.sh");

        let after = rc_with_block(
            &format!("export EDITOR=vi\n{BLOCK_START}\nsource \"/gone\"\n"),
            script,
        );

        assert!(after.starts_with("export EDITOR=vi\n\n"), "{after}");
        assert_eq!(blocks_in(&after), 1);
        assert!(!after.contains("/gone"), "{after}");
    }

    #[test]
    fn a_marker_indented_by_hand_is_still_the_marker() {
        let script = Path::new("/x/completions.sh");

        let after = rc_with_block(
            &format!("   {BLOCK_START}\n  source \"/gone\"\n\t{BLOCK_END}\nexport EDITOR=vi\n"),
            script,
        );

        assert_eq!(blocks_in(&after), 1);
        assert!(after.starts_with("export EDITOR=vi\n\n"), "{after}");
    }

    #[test]
    fn the_rc_file_always_ends_in_exactly_one_newline_after_the_block() {
        let script = Path::new("/x/completions.sh");

        for existing in ["", "\n\n\n", "export EDITOR=vi", "export EDITOR=vi\n\n\n"] {
            let after = rc_with_block(existing, script);
            assert!(after.ends_with(&format!("{BLOCK_END}\n")), "{after:?}");
            assert!(!after.ends_with("\n\n"), "{after:?}");
        }
    }

    // --- the source line ---------------------------------------------------

    #[test]
    fn a_path_with_a_quote_in_it_cannot_end_the_source_line_early() {
        assert_eq!(
            source_line(Path::new(r#"/home/some"one/completions.sh"#)),
            r#"source "/home/some\"one/completions.sh""#
        );
    }

    #[test]
    fn a_path_with_a_space_in_it_is_quoted() {
        assert_eq!(
            source_line(Path::new("/home/some one/completions.sh")),
            r#"source "/home/some one/completions.sh""#
        );
    }

    // --- where the files are ----------------------------------------------

    #[test]
    fn the_script_lives_under_the_home_directory_by_default() {
        // The home directory, not `$XDG_CONFIG_HOME`: moving it would leave every
        // installed rc file sourcing a path nothing writes any more.
        assert_eq!(
            completion_file_in(None, Some(PathBuf::from("/home/someone"))),
            Ok(PathBuf::from(
                "/home/someone/.config/devlaunch/completions.sh"
            ))
        );
    }

    #[test]
    fn the_override_names_the_script_when_it_is_set() {
        assert_eq!(
            completion_file_in(Some("/tmp/scratch/completions.sh"), None),
            Ok(PathBuf::from("/tmp/scratch/completions.sh"))
        );
    }

    #[test]
    fn an_empty_override_counts_as_unset() {
        assert_eq!(
            completion_file_in(Some(""), Some(PathBuf::from("/home/someone"))),
            Ok(PathBuf::from(
                "/home/someone/.config/devlaunch/completions.sh"
            ))
        );
    }

    #[test]
    fn no_home_is_an_error_rather_than_a_relative_path() {
        assert_eq!(completion_file_in(None, None), Err(NoHomeDirectory));
        assert_eq!(completion_file_in(Some(""), None), Err(NoHomeDirectory));
    }

    #[test]
    fn the_paths_the_environment_resolves_to_are_absolute() {
        // Read-only: the process environment is shared with every other test in
        // this binary, so this observes it rather than mutating it. The rules
        // themselves are pinned on `completion_file_in` above.
        for path in [completion_file_path(), default_rc_path()] {
            let path = path.expect("this machine has a home directory");
            assert!(path.is_absolute(), "{path:?}");
        }
    }

    // --- what fails, and how it says so ------------------------------------

    #[test]
    fn an_rc_file_that_cannot_be_read_is_not_overwritten() {
        // Reading it is how the install keeps what is in it; a file it cannot
        // read is a file it must not rewrite from nothing.
        let dir = temp_dir();
        let rc = dir.path().join("bashrc-directory");
        fs::create_dir(&rc).expect("something that is not a file");

        let refused = install_into(&script_of(&dir), &rc).expect_err("no install");

        match refused {
            InstallError::ReadRc { path, .. } => assert_eq!(path, rc),
            other => panic!("expected a read refusal, got {other:?}"),
        }
        assert!(rc.is_dir(), "and it is still there");
    }

    #[test]
    fn a_script_directory_that_cannot_be_created_says_which_one() {
        let dir = temp_dir();
        let blocked = dir.path().join("in-the-way");
        fs::write(&blocked, "not a directory").expect("a file where a directory is wanted");
        let script = blocked.join("devlaunch").join("completions.sh");

        let refused = install_into(&script, &rc_of(&dir)).expect_err("no install");

        match refused {
            InstallError::CreateDirectory { path, .. } => {
                assert_eq!(path, blocked.join("devlaunch"));
            }
            other => panic!("expected a directory refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_rc_file_that_cannot_be_written_says_so_as_the_rc_file() {
        let dir = temp_dir();
        let rc = dir.path().join("no-such-dir").join("nested");
        fs::write(dir.path().join("no-such-dir"), "in the way").expect("a file in the way");

        let refused = install_into(&script_of(&dir), &rc).expect_err("no install");

        match refused {
            InstallError::CreateDirectory { .. } => {}
            other => panic!("expected a directory refusal, got {other:?}"),
        }
    }
}
