//! The shell a session manager's new pane opens.
//!
//! herdr spawns one program per pane it creates -- `[terminal] default_shell` in
//! its config -- and exports `HERDR_TAB_ID` into it. Put `dl-herdr-shell` there
//! and every pane asks one question on the way up: does this tab already hold a
//! devlaunch session? A pane in an `aid` tab lands in that workspace's container;
//! every other pane gets the shell it would have got anyway.
//!
//! # Why a second name and not a flag
//!
//! `default_shell` is an **executable**, not a command string. Measured on herdr
//! 0.8.2: a two-word value fails the spawn with `ENOENT` naming the whole string
//! as one path (`Unable to spawn /usr/bin/env DEVLAUNCH_PANE_PROBE=1 /bin/bash
//! because it doesn't exist on the filesystem`), and it fails at *workspace
//! create* rather than at config load -- `herdr config check` says `config: ok`
//! for it. So `default_shell = "dl --herdr-shell"` cannot work, and a name that
//! needs no arguments is what the field can hold.
//!
//! The flag still exists and is what the name resolves to, so there is one code
//! path and not two: [`crate::cli::Command::HerdrShell`] is reached either way.
//!
//! herdr passes the program **no arguments at all** (measured the same way:
//! `argc=0`), which is why the fall-through below execs the shell bare rather
//! than forwarding an argv it was never given. A herdr that starts passing `-l`
//! is a herdr this stops honouring, and the test named beside `ARGUMENTS_PASSED`
//! is where that shows up.

use std::ffi::OsStr;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};

use crate::commands::Ending;

/// The name devlaunch installs beside `dl` for herdr's `default_shell`.
///
/// A name and not a copy: `dl --install` links it to whatever `dl` is running, so
/// the pane shell is always the same build as the launcher it re-enters.
pub(crate) const NAME: &str = "dl-herdr-shell";

/// How many arguments herdr hands its `default_shell`.
///
/// Zero, measured on herdr 0.8.2 by pointing `default_shell` at a script that
/// logged its own `$#`. It is a constant here so that the number is written down
/// once with the measurement beside it: the fall-through execs `$SHELL` with no
/// arguments *because* there are none to forward, and if that ever stops being
/// true a login pane would silently stop being one.
#[cfg(test)]
pub(crate) const ARGUMENTS_PASSED: usize = 0;

/// Whether this process was started under the pane shell's name.
///
/// The file name and not the whole path, so a symlink, a hard link and a copy all
/// answer alike, and so does one reached through a `PATH` entry. Anything that is
/// not valid UTF-8 is not this name.
pub(crate) fn invoked_as(program: Option<&OsStr>) -> bool {
    program
        .map(Path::new)
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        == Some(NAME)
}

/// The shell to open when the tab holds no workspace.
///
/// `$SHELL`, then `/bin/sh`. herdr's own fallback is the same pair in the same
/// order, which is the point: a pane that falls through here must be the pane
/// herdr would have opened, not a second opinion about what a shell is.
pub(crate) fn host_shell(shell: Option<&str>) -> &str {
    match shell {
        Some(shell) if !shell.is_empty() => shell,
        _ => "/bin/sh",
    }
}

/// Hand the pane to an ordinary shell, and do not come back.
///
/// `exec` rather than a spawn-and-wait for two reasons that are really one. The
/// pane's foreground process is what herdr reads to decide what the pane holds,
/// and a `dl` sitting above the shell would be a process herdr has to see past
/// forever; and the pane should close when the shell exits, which is what
/// replacing this process gives for free.
///
/// The [`Ending`] is only ever reached when the `exec` itself failed, which means
/// there is no shell on this machine to run.
pub(crate) fn become_the_host_shell() -> Ending {
    let shell = std::env::var("SHELL").ok();
    let program = host_shell(shell.as_deref());
    let failure = std::process::Command::new(program).exec();
    eprintln!("dl: cannot open a shell: {program}: {failure}");
    Ending::Refused
}

// ===========================================================================
// putting the name where herdr's config can point at it
// ===========================================================================

/// What linking the pane shell beside `dl` came to.
///
/// Four arms and three of them are fine. A machine whose `dl` sits somewhere
/// unwritable still gets a working `dl` and a working `--install`; what it does
/// not get is the pane shell, and saying so is the whole of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Linked {
    /// The link was created.
    Created { path: PathBuf },
    /// It was already pointing at this build.
    AlreadyCurrent { path: PathBuf },
    /// It was pointing somewhere else, and now points here.
    Repointed { path: PathBuf },
    /// It could not be made, and this is why.
    Refused { path: PathBuf, reason: String },
}

impl Linked {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Created { path }
            | Self::AlreadyCurrent { path }
            | Self::Repointed { path }
            | Self::Refused { path, .. } => path,
        }
    }
}

/// Link [`NAME`] beside the `dl` this process is, pointing at it.
///
/// **Beside `dl` and not in a directory of devlaunch's own**, because the whole
/// value of the link is that it is on `PATH` wherever `dl` already is and is the
/// *same build* as the launcher a pane re-enters. A copy would be a second binary
/// to keep in step; a wrapper script would be a third spelling of the argv.
///
/// A link that already points here is left alone rather than rewritten, which is
/// divergence row 10's rule for the completion script applied to this: a re-run
/// over a current install should say nothing changed and change nothing.
///
/// `dl` is not always at a path that can be linked next to. `pixi global` puts it
/// under `~/.pixi/bin`, which is writable; a distribution package puts it in
/// `/usr/bin`, which is not, and a `current_exe` that the kernel has marked
/// ` (deleted)` names no directory at all. All of those are [`Linked::Refused`].
pub(crate) fn link_beside(dl: Option<&Path>) -> Linked {
    let Some(dl) = dl.filter(|path| path.is_absolute()) else {
        return Linked::Refused {
            path: PathBuf::from(NAME),
            reason: "this build's own path could not be read".to_owned(),
        };
    };
    let Some(directory) = dl.parent() else {
        return Linked::Refused {
            path: PathBuf::from(NAME),
            reason: format!("{} is not in a directory", dl.display()),
        };
    };
    let link = directory.join(NAME);
    match std::fs::read_link(&link) {
        Ok(target) if target == dl => return Linked::AlreadyCurrent { path: link },
        Ok(_) => {
            if let Err(failure) = std::fs::remove_file(&link) {
                return Linked::Refused {
                    path: link,
                    reason: failure.to_string(),
                };
            }
            return match std::os::unix::fs::symlink(dl, &link) {
                Ok(()) => Linked::Repointed { path: link },
                Err(failure) => Linked::Refused {
                    path: link,
                    reason: failure.to_string(),
                },
            };
        }
        // Not a symlink, or not there at all. `symlink` below answers both: it
        // succeeds where there was nothing and fails with `EEXIST` where there is
        // a file somebody else put there, which is a file dl must not remove.
        Err(_) => {}
    }
    match std::os::unix::fs::symlink(dl, &link) {
        Ok(()) => Linked::Created { path: link },
        Err(failure) => Linked::Refused {
            path: link,
            reason: failure.to_string(),
        },
    }
}

/// The line to paste into herdr's config, for a pane shell at this path.
///
/// Printed rather than written. `~/.config/herdr/config.toml` belongs to herdr and
/// is chezmoi-managed on the machine this was built for, so devlaunch editing it
/// would be devlaunch editing something it does not own -- the same argument that
/// puts the Claude Code hook at `/etc/claude-code/managed-settings.json` inside a
/// container rather than in the `~/.claude` a devcontainer may have bind-mounted.
pub(crate) fn config_line(link: &Path) -> String {
    format!("[terminal]\ndefault_shell = \"{}\"", link.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn the_installed_name_is_recognised_however_it_is_reached() {
        for program in [
            NAME,
            "/home/dev/.local/bin/dl-herdr-shell",
            "./dl-herdr-shell",
        ] {
            assert!(invoked_as(Some(&OsString::from(program))), "{program}");
        }
    }

    #[test]
    fn every_other_name_is_an_ordinary_dl() {
        for program in [
            "dl",
            "/usr/local/bin/dl",
            "aid",
            "dl-herdr-shell-2",
            "herdr-shell",
        ] {
            assert!(!invoked_as(Some(&OsString::from(program))), "{program}");
        }
        assert!(!invoked_as(None));
    }

    /// A `$SHELL` that is unset and one that is set to nothing are the same fact
    /// about this machine, and an `exec ""` is not a shell.
    #[test]
    fn an_absent_or_empty_shell_falls_back_to_sh() {
        assert_eq!(host_shell(None), "/bin/sh");
        assert_eq!(host_shell(Some("")), "/bin/sh");
        assert_eq!(host_shell(Some("/usr/bin/fish")), "/usr/bin/fish");
    }

    // ----------------------------------------------------- the installed name

    /// The link is made beside `dl` so it is on `PATH` wherever `dl` is, and so it
    /// is the same build the pane re-enters.
    #[test]
    fn the_link_lands_beside_the_binary_and_points_at_it() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let dl = dir.path().join("dl");
        std::fs::write(&dl, "").expect("a file standing in for the binary");

        let linked = link_beside(Some(&dl));
        assert_eq!(
            linked,
            Linked::Created {
                path: dir.path().join(NAME)
            }
        );
        assert_eq!(
            std::fs::read_link(linked.path()).expect("a symlink"),
            dl,
            "the link does not point at this build"
        );
    }

    /// Divergence row 10's rule, applied to the link: a re-run over a current
    /// install changes nothing and says so.
    #[test]
    fn a_link_that_already_points_here_is_left_alone() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let dl = dir.path().join("dl");
        std::fs::write(&dl, "").expect("a file standing in for the binary");

        assert!(matches!(link_beside(Some(&dl)), Linked::Created { .. }));
        assert_eq!(
            link_beside(Some(&dl)),
            Linked::AlreadyCurrent {
                path: dir.path().join(NAME)
            }
        );
    }

    /// What `dl --install` after a `pixi global update` has to do: the name is
    /// there and points at the build that is gone.
    #[test]
    fn a_link_to_another_build_is_repointed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let dl = dir.path().join("dl");
        std::fs::write(&dl, "").expect("a file standing in for the binary");
        std::os::unix::fs::symlink(dir.path().join("old-dl"), dir.path().join(NAME))
            .expect("a link to some other build");

        assert_eq!(
            link_beside(Some(&dl)),
            Linked::Repointed {
                path: dir.path().join(NAME)
            }
        );
        assert_eq!(
            std::fs::read_link(dir.path().join(NAME)).expect("a symlink"),
            dl
        );
    }

    /// A *file* at the name is somebody else's, and dl removes only its own link.
    /// `symlink` answers this for free with `EEXIST`, which is the reason the
    /// remove above is reached only from the `read_link` success arm.
    #[test]
    fn a_file_somebody_else_put_at_the_name_is_not_removed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let dl = dir.path().join("dl");
        std::fs::write(&dl, "").expect("a file standing in for the binary");
        let occupied = dir.path().join(NAME);
        std::fs::write(&occupied, "someone else's program").expect("a file at the name");

        assert!(matches!(link_beside(Some(&dl)), Linked::Refused { .. }));
        assert_eq!(
            std::fs::read_to_string(&occupied).expect("still there"),
            "someone else's program"
        );
    }

    /// A `current_exe()` dl could not read, and a relative one it must not resolve
    /// against whatever directory the pane happened to start in.
    #[test]
    fn a_path_that_is_not_an_absolute_one_is_refused() {
        assert!(matches!(link_beside(None), Linked::Refused { .. }));
        assert!(matches!(
            link_beside(Some(Path::new("dl"))),
            Linked::Refused { .. }
        ));
    }

    /// The config line names the link and the field herdr reads. Both halves are
    /// another program's words, so they are pinned rather than described.
    #[test]
    fn the_config_line_is_the_field_herdr_reads() {
        assert_eq!(
            config_line(Path::new("/home/dev/.pixi/bin/dl-herdr-shell")),
            "[terminal]\ndefault_shell = \"/home/dev/.pixi/bin/dl-herdr-shell\""
        );
    }

    /// The measurement this module's fall-through is built on, written down where
    /// a change to it has to be argued for rather than noticed.
    #[test]
    fn herdr_hands_its_pane_shell_no_arguments() {
        assert_eq!(ARGUMENTS_PASSED, 0);
    }
}
