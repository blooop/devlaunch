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
//! # Why a script and not a symlink to `dl`
//!
//! The name was a symlink first, and `dl` read its own `argv[0]`. That cannot work
//! on the install CLAUDE.md names as how the host gets `dl`: **`pixi global` does
//! not symlink, it trampolines.** `~/.pixi/bin/dl` is a small binary that execs
//! `~/.pixi/envs/devlaunch/bin/dl`, and it replaces `argv[0]` with the target's own
//! path on the way. Measured here, with a trampoline pointed at `/bin/sleep`: the
//! child's `/proc/<pid>/cmdline` reads `/bin/sleep 30`, not the name that was run.
//!
//! Two failures came out of that, and neither is small. `current_exe()` answers
//! the *env* path, so `dl --install` put the link in `~/.pixi/envs/devlaunch/bin`
//! -- off `PATH`, and a directory `pixi global update` rebuilds, after which
//! `default_shell` names a path that does not exist and **every pane on the machine
//! stops opening**, at pane creation, with the ENOENT this module's other
//! measurement describes. And a link placed in `~/.pixi/bin` by hand fails the
//! other way: the trampoline rewrites `argv[0]`, so the name is never seen and
//! every pane silently gets a host shell.
//!
//! So the installed thing is a two-line script that runs `dl --herdr-shell`, and
//! nothing reads `argv[0]` any more. It resolves `dl` through `PATH` rather than
//! by absolute path for the same reason [`crate::session`]'s refresh does: an
//! absolute path is exactly what a `pixi global update` invalidates.

use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};

use crate::commands::Ending;

/// The name devlaunch installs for herdr's `default_shell`.
pub(crate) const NAME: &str = "dl-herdr-shell";

/// The script `dl --install` writes, and the whole of what herdr spawns.
///
/// Two things happen here that could not happen inside `dl`. It resolves `dl`
/// itself, so a machine whose `dl` has been uninstalled still opens the pane
/// rather than failing the spawn -- the one failure mode that costs a pane its
/// existence rather than its container, since a `default_shell` that will not
/// start is a pane that never appears. And it drops whatever argv herdr passed:
/// herdr passes none today (measured on 0.8.2, `$#` was 0), `dl --herdr-shell`
/// takes none, and a herdr that starts passing `-l` would otherwise reach clap as
/// an unexpected argument, exit 2, and close the pane.
pub(crate) const SCRIPT: &str = "\
#!/bin/sh
# Written by `dl --install`. Point herdr's [terminal] default_shell at this file.
#
# `dl` by name and not by path: `pixi global update` replaces the directory an
# absolute path would name, and a default_shell that has gone stale is a pane that
# will not open at all. The fallback is for the same reason -- a pane must open.
command -v dl >/dev/null 2>&1 && exec dl --herdr-shell
exec \"${SHELL:-/bin/sh}\"
";

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

/// Whether an ending means no session ever ran in this pane.
///
/// The pane shell's promise is that a failure costs the container and never the
/// pane, and the resolution keeps it: everything that can go wrong finding the
/// workspace answers [`PaneDestination::HostShell`](devlaunch_core::flows::session_manager::PaneDestination::HostShell).
/// The launch *after* it escaped that. A tab whose transport names a workspace dl
/// cannot address -- one devpod created and dl has no record of, or one deleted
/// since the session in the sibling pane started -- refused, printed its complaint
/// and exited non-zero, which closes the pane. Reproduced against live herdr
/// 0.8.2: `Unknown workspace 'not-a-real-ws-9zzz'`, exit 1, pane gone.
///
/// So the two endings that mean *nothing started* fall through to a shell, and the
/// ones that carry a session's own status do not. That distinction is the whole
/// point and is why this is not "fall through on any failure": a session that ran
/// and exited 130 is the person pressing Ctrl-C, and reopening a shell under them
/// would be devlaunch refusing to let a pane close when they asked it to.
///
/// The refusal is still printed, by the launch, above the shell this then opens.
/// They keep both the reason and the pane.
pub(crate) fn no_session_ran(ending: Ending) -> bool {
    match ending {
        // dl said no before anything started.
        Ending::Refused | Ending::DevpodMissing => true,
        // A session ran, or a child did, and the number is theirs.
        Ending::Done | Ending::Child(_) | Ending::Session(_) => false,
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

/// What installing the pane shell came to.
///
/// Four arms and three of them are fine. A machine whose `~/.local/bin` cannot be
/// written still gets a working `dl` and working completions; what it does not get
/// is the pane shell, and saying so is the whole of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Installed {
    /// The script was written.
    Written { path: PathBuf },
    /// It was already byte-identical to what this build writes.
    AlreadyCurrent { path: PathBuf },
    /// It was different, and now is not.
    Refreshed { path: PathBuf },
    /// It could not be written, and this is why.
    Refused { path: PathBuf, reason: String },
}

impl Installed {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Written { path }
            | Self::AlreadyCurrent { path }
            | Self::Refreshed { path }
            | Self::Refused { path, .. } => path,
        }
    }
}

/// Where the script goes.
///
/// `~/.local/bin` rather than beside `dl`, and the difference is the whole of the
/// bug above: "beside `dl`" resolves through `current_exe()`, which under `pixi
/// global` is an environment directory that `pixi global update` rebuilds. This
/// one is the user's own, is on `PATH` by convention, and nothing but the user
/// rewrites it. It is also where `dev.sh` already puts `dl-next`, for the same
/// reason.
pub(crate) fn install_path(home: Option<&Path>) -> Option<PathBuf> {
    Some(home?.join(".local").join("bin").join(NAME))
}

/// Write the script, and make it executable.
///
/// A re-run over a current install rewrites nothing and says so, which is
/// divergence row 10's rule for the completion script applied to this.
pub(crate) fn install(path: &Path) -> Installed {
    let existing = std::fs::read_to_string(path).ok();
    if existing.as_deref() == Some(SCRIPT) {
        return Installed::AlreadyCurrent {
            path: path.to_owned(),
        };
    }
    let refreshing = existing.is_some();
    if let Some(directory) = path.parent()
        && let Err(failure) = std::fs::create_dir_all(directory)
    {
        return Installed::Refused {
            path: path.to_owned(),
            reason: failure.to_string(),
        };
    }
    if let Err(failure) = std::fs::write(path, SCRIPT) {
        return Installed::Refused {
            path: path.to_owned(),
            reason: failure.to_string(),
        };
    }
    // A `default_shell` herdr cannot execute is a pane that does not open, so the
    // mode is part of the install rather than something the user is told to do.
    if let Err(failure) =
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
    {
        return Installed::Refused {
            path: path.to_owned(),
            reason: failure.to_string(),
        };
    }
    if refreshing {
        Installed::Refreshed {
            path: path.to_owned(),
        }
    } else {
        Installed::Written {
            path: path.to_owned(),
        }
    }
}

/// The line to paste into herdr's config, for a pane shell at this path.
///
/// Printed rather than written. `~/.config/herdr/config.toml` belongs to herdr and
/// is chezmoi-managed on the machine this was built for, so devlaunch editing it
/// would be devlaunch editing something it does not own -- the same argument that
/// puts the Claude Code hook at `/etc/claude-code/managed-settings.json` inside a
/// container rather than in the `~/.claude` a devcontainer may have bind-mounted.
pub(crate) fn config_line(script: &Path) -> String {
    format!("[terminal]\ndefault_shell = \"{}\"", script.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The split this rests on, stated over every arm so a new one has to answer
    /// for itself rather than defaulting into the fall-through.
    #[test]
    fn only_an_ending_with_no_session_behind_it_falls_through_to_a_shell() {
        // Nothing ran: the pane would otherwise close carrying dl's complaint.
        assert!(no_session_ran(Ending::Refused));
        assert!(no_session_ran(Ending::DevpodMissing));
        // Something ran, and the number is its own -- including the one a person
        // produces by pressing Ctrl-C, which must close the pane as they asked.
        assert!(!no_session_ran(Ending::Done));
        assert!(!no_session_ran(Ending::Session(0)));
        assert!(!no_session_ran(Ending::Session(130)));
        assert!(!no_session_ran(Ending::Child(
            devlaunch_core::runner::Exit::Code(1)
        )));
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

    /// The bug this shape exists to prevent, stated as the property that fixes
    /// it: nothing about the installed thing depends on the name it is run under.
    /// A `pixi global` trampoline replaces `argv[0]` with its target's own path
    /// (measured: a trampoline pointed at `/bin/sleep` gives a child whose
    /// `cmdline` is `/bin/sleep 30`), so a symlink read through `argv[0]` was
    /// never going to fire on the install CLAUDE.md documents.
    #[test]
    fn the_script_runs_dl_by_name_and_reads_no_argv_zero() {
        assert!(SCRIPT.contains("exec dl --herdr-shell"), "{SCRIPT}");
        assert!(
            !SCRIPT.contains("$0"),
            "the script reads the name it was run under: {SCRIPT}"
        );
        // No absolute path to dl, because that is what a pixi global update
        // invalidates -- and a default_shell naming a path that is gone is a pane
        // that never opens.
        assert!(!SCRIPT.contains("/dl "), "{SCRIPT}");
    }

    /// A pane must open. Both of the script's own failure modes end in a shell:
    /// a `dl` that is not installed, and the argv herdr may one day pass, which
    /// `dl --herdr-shell` would meet with a clap error and exit 2.
    #[test]
    fn the_script_opens_a_shell_when_dl_cannot_run_it() {
        assert!(SCRIPT.contains("command -v dl"), "{SCRIPT}");
        assert!(SCRIPT.contains(r#"exec "${SHELL:-/bin/sh}""#), "{SCRIPT}");
        assert!(
            !SCRIPT.contains("$@"),
            "the script forwards an argv dl refuses: {SCRIPT}"
        );
    }

    /// It is a shell script, and herdr spawns it directly.
    #[test]
    fn the_script_is_executable_and_says_what_runs_it() {
        assert!(SCRIPT.starts_with("#!/bin/sh\n"), "{SCRIPT}");
        let dir = tempfile::tempdir().expect("a temporary directory");
        let script = dir.path().join(NAME);

        assert_eq!(
            install(&script),
            Installed::Written {
                path: script.clone()
            }
        );
        let mode = <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::mode(
            &std::fs::metadata(&script).expect("written").permissions(),
        );
        assert_eq!(mode & 0o777, 0o755, "herdr could not execute it");
    }

    /// The script actually runs, and falls through to a shell when `dl` is not on
    /// the `PATH` it was given. Run rather than read, because the fallback is the
    /// half that keeps a pane from vanishing.
    #[test]
    fn the_script_falls_through_to_a_shell_with_no_dl_on_path() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let script = dir.path().join(NAME);
        install(&script);

        let ran = std::process::Command::new(&script)
            .env("PATH", dir.path())
            .env("SHELL", "/bin/echo")
            .output()
            .expect("the script runs");
        assert!(ran.status.success(), "{ran:?}");
    }

    /// Divergence row 10's rule: a re-run over a current install changes nothing
    /// and says so.
    #[test]
    fn a_script_that_is_already_current_is_left_alone() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let script = dir.path().join(NAME);

        assert!(matches!(install(&script), Installed::Written { .. }));
        assert_eq!(
            install(&script),
            Installed::AlreadyCurrent {
                path: script.clone()
            }
        );
    }

    /// What `dl --install` after an upgrade has to do.
    #[test]
    fn a_script_from_another_build_is_refreshed() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let script = dir.path().join(NAME);
        std::fs::write(&script, "#!/bin/sh\nexec dl --something-else\n").expect("an old script");

        assert_eq!(
            install(&script),
            Installed::Refreshed {
                path: script.clone()
            }
        );
        assert_eq!(std::fs::read_to_string(&script).expect("readable"), SCRIPT);
    }

    /// A home dl could not read names no install path, and the report says so
    /// rather than writing into whatever the working directory happens to be.
    #[test]
    fn no_home_directory_names_no_script() {
        assert_eq!(install_path(None), None);
        assert_eq!(
            install_path(Some(Path::new("/home/dev"))),
            Some(PathBuf::from("/home/dev/.local/bin/dl-herdr-shell"))
        );
    }

    /// The config line names the script and the field herdr reads. Both halves are
    /// another program's words, so they are pinned rather than described.
    #[test]
    fn the_config_line_is_the_field_herdr_reads() {
        assert_eq!(
            config_line(Path::new("/home/dev/.local/bin/dl-herdr-shell")),
            "[terminal]\ndefault_shell = \"/home/dev/.local/bin/dl-herdr-shell\""
        );
    }
}
