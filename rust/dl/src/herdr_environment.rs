use std::fs;
use std::io;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use devlaunch_core::domain::xdg;
use devlaunch_core::flows::herdr_environment::{self, ClaudeConfig, Store};
use devlaunch_core::osext;

use crate::cli::HerdrEnvAction;
use crate::commands::Ending;
use crate::pane_shell;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn home() -> io::Result<PathBuf> {
    osext::home_dir().ok_or_else(|| invalid("HOME is required"))
}

fn store(workspace: Option<&str>) -> io::Result<(Store, String)> {
    if std::env::var("HERDR_ENV").as_deref() != Ok("1") {
        return Err(invalid("run this command inside Herdr on the host"));
    }
    let socket = std::env::var_os("HERDR_SOCKET_PATH")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("HERDR_SOCKET_PATH is required"))?;
    let workspace = workspace
        .map(str::to_owned)
        .or_else(|| std::env::var("HERDR_WORKSPACE_ID").ok())
        .ok_or_else(|| invalid("HERDR_WORKSPACE_ID or --herdr-workspace is required"))?;
    let root = std::env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(home()?.join(".local/state"));
    Ok((
        Store::new(&root, &PathBuf::from(socket), &workspace)?,
        workspace,
    ))
}

fn report(result: io::Result<()>) -> Ending {
    match result {
        Ok(()) => Ending::Done,
        Err(error) => {
            eprintln!("dl: {error}");
            Ending::Refused
        }
    }
}

fn selected_workspace_exists(workspace: &str) -> io::Result<()> {
    let binary = devlaunch_core::clients::herdr_binary_from_process()
        .ok_or_else(|| invalid("HERDR_BIN_PATH is required"))?;
    let status = Command::new(binary)
        .args(["workspace", "get", workspace])
        .stdout(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(invalid(&format!(
            "Herdr could not find the selected workspace {workspace}"
        )));
    }
    Ok(())
}

pub(crate) fn manage(action: &HerdrEnvAction, workspace: Option<&str>) -> Ending {
    report(manage_inner(action, workspace))
}

fn manage_inner(action: &HerdrEnvAction, workspace: Option<&str>) -> io::Result<()> {
    let (store, workspace) = store(workspace)?;
    match action {
        HerdrEnvAction::Show => {
            println!("{}", serde_json::to_string_pretty(&store.read()?)?);
            return Ok(());
        }
        HerdrEnvAction::Clear => {
            selected_workspace_exists(&workspace)?;
            store.clear()?;
        }
        HerdrEnvAction::Set { key, value } => {
            selected_workspace_exists(&workspace)?;
            store.update(|environment| environment.set(key, Some(value.clone())))?;
        }
        HerdrEnvAction::Unset { key } => {
            selected_workspace_exists(&workspace)?;
            store.update(|environment| environment.set(key, None))?;
        }
        HerdrEnvAction::Profile { name } => {
            selected_workspace_exists(&workspace)?;
            let root =
                xdg::claude_profiles_root().map_err(|_| invalid("no Claude profiles directory"))?;
            store.update(|environment| environment.select_profile(name, &root))?;
        }
    }
    eprintln!(
        "Saved environment for {workspace}. New shells use it; running processes keep their environment."
    );
    Ok(())
}

pub(crate) fn open_pane() -> Ending {
    let result = (|| -> io::Result<()> {
        let environment = if std::env::var("HERDR_ENV").as_deref() == Ok("1") {
            // A selector this cannot resolve names no saved state, so there is
            // nothing to apply and nothing to refuse over. A saved state that is
            // broken still refuses, below: that one is the person's own.
            match store(None) {
                Ok((store, _)) => store.read()?,
                Err(error) => {
                    eprintln!("dl: no Herdr workspace environment: {error}");
                    Default::default()
                }
            }
        } else {
            Default::default()
        };
        let mut command = Command::new(std::env::current_exe()?);
        command.arg("--herdr-shell-ready");
        for (key, value) in environment.variables() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        match environment.claude() {
            ClaudeConfig::Inherited => {}
            ClaudeConfig::Cleared => {
                command.env_remove("CLAUDE_CONFIG_DIR");
            }
            ClaudeConfig::Directory(value) => {
                command.env("CLAUDE_CONFIG_DIR", value);
            }
            ClaudeConfig::ServerDefault => {}
            ClaudeConfig::Profile(name) => {
                let root = xdg::claude_profiles_root()
                    .map_err(|_| invalid("no Claude profiles directory"))?;
                command.env(
                    "CLAUDE_CONFIG_DIR",
                    herdr_environment::profile_directory(name, &root)?,
                );
            }
        }
        if let Some(profile) = environment.claude().profile_name() {
            command.args(["--claude-profile", profile]);
        }
        Err(command.exec())
    })();
    report(result)
}

enum ConfigOwner {
    Chezmoi(PathBuf),
    Unmanaged,
    ProbeFailed(String),
}

/// `chezmoi source-path <file>` exits nonzero both for a file chezmoi does not
/// manage and for a chezmoi that cannot answer at all -- an unparseable or
/// unreadable `chezmoi.toml`, a source path that is not a directory. Collapsing
/// the two writes over a managed config whenever chezmoi is broken, which is the
/// case the refusal exists for. A bare `chezmoi source-path` separates them
/// without reading English out of stderr: it prints the source directory when
/// chezmoi is healthy, and fails with the same complaint when it is not.
fn chezmoi_source(config: &Path) -> io::Result<ConfigOwner> {
    if fs::symlink_metadata(config).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Ok(ConfigOwner::Unmanaged);
    }
    let config = if config.is_absolute() {
        config.to_owned()
    } else {
        std::env::current_dir()?.join(config)
    };
    let probe = |argument: Option<&Path>| {
        let mut command = Command::new("chezmoi");
        command.arg("source-path");
        if let Some(argument) = argument {
            command.arg(argument);
        }
        command.output()
    };
    let output = match probe(Some(&config)) {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ConfigOwner::Unmanaged);
        }
        Err(error) => return Err(error),
    };
    if !output.status.success() {
        let healthy = probe(None).is_ok_and(|output| output.status.success());
        if healthy {
            return Ok(ConfigOwner::Unmanaged);
        }
        let complaint = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Ok(ConfigOwner::ProbeFailed(if complaint.is_empty() {
            format!("chezmoi source-path exited with {}", output.status)
        } else {
            complaint
        }));
    }
    let source = String::from_utf8(output.stdout)
        .map_err(|_| invalid("chezmoi returned a source path that is not UTF-8"))?;
    let source = source.trim();
    if source.is_empty() {
        return Err(invalid("chezmoi returned an empty source path"));
    }
    Ok(ConfigOwner::Chezmoi(PathBuf::from(source)))
}

pub(crate) fn setup() -> Ending {
    report((|| {
        let home = home()?;
        let script = pane_shell::install_path(Some(&home)).expect("home supplied");
        let fallback_config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("herdr/config.toml");
        let config = std::env::var_os("HERDR_CONFIG_PATH")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or(fallback_config);
        match chezmoi_source(&config)? {
            ConfigOwner::Chezmoi(source) => {
                return Err(invalid(&format!(
                    "Herdr config is managed by chezmoi from {}; set terminal.default_shell to {} there, apply it, then run `dl --install`",
                    source.display(),
                    script.display()
                )));
            }
            ConfigOwner::ProbeFailed(complaint) => {
                return Err(invalid(&format!(
                    "chezmoi could not say whether it manages {}, so it was left alone -- fix chezmoi, or unset it from PATH, and run setup again. chezmoi said: {complaint}",
                    config.display()
                )));
            }
            ConfigOwner::Unmanaged => {}
        }
        // Validate and write the config only after the executable is available.
        if let pane_shell::Installed::Refused { reason, .. } = pane_shell::install(&script) {
            return Err(io::Error::other(reason));
        }
        let changed = herdr_environment::configure(&config, &script, &home)?;
        eprintln!(
            "Herdr pane shell configured in {}{}",
            config.display(),
            if changed { "" } else { " (already current)" }
        );
        eprintln!(
            "Run `herdr server reload-config` for the session to use it. Select a login with `dl --herdr-env profile NAME` in each workspace."
        );
        Ok(())
    })())
}
