use std::fs;
use std::io;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use devlaunch_core::domain::xdg;
use devlaunch_core::flows::herdr_environment::{self, Store};
use devlaunch_core::osext;

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

pub(crate) fn manage(words: &[String], workspace: Option<&str>) -> Ending {
    report(manage_inner(words, workspace))
}

fn manage_inner(words: &[String], workspace: Option<&str>) -> io::Result<()> {
    let (store, workspace) = store(workspace)?;
    if words == ["show"] {
        println!("{}", serde_json::to_string_pretty(&store.read()?)?);
        return Ok(());
    }
    let valid = matches!(words, [action] if action == "clear")
        || matches!(words, [action, _] if matches!(action.as_str(), "set" | "unset" | "profile"));
    if !valid {
        return Err(invalid(
            "use --herdr-env set KEY=VALUE, unset KEY, profile NAME, show, or clear",
        ));
    }
    let binary = devlaunch_core::clients::herdr_binary_from_process()
        .ok_or_else(|| invalid("HERDR_BIN_PATH is required"))?;
    let status = Command::new(binary)
        .args(["workspace", "get", &workspace])
        .stdout(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(invalid("Herdr could not find the selected workspace"));
    }
    match words[0].as_str() {
        "clear" => store.clear()?,
        "set" => {
            let (key, value) = words[1]
                .split_once('=')
                .ok_or_else(|| invalid("set expects KEY=VALUE"))?;
            store.update(|environment| environment.set(key, Some(value.to_owned())))?;
        }
        "unset" => store.update(|environment| environment.set(&words[1], None))?,
        "profile" => {
            let root =
                xdg::claude_profiles_root().map_err(|_| invalid("no Claude profiles directory"))?;
            store.update(|environment| environment.select_profile(&words[1], &root))?;
        }
        _ => unreachable!("validated action"),
    }
    eprintln!(
        "Saved environment for {workspace}. New shells use it; running processes keep their environment."
    );
    Ok(())
}

pub(crate) fn open_pane() -> Ending {
    let result = (|| -> io::Result<()> {
        let environment = if std::env::var("HERDR_ENV").as_deref() == Ok("1") {
            store(None)?.0.read()?
        } else {
            Default::default()
        };
        environment.check_profile_directory()?;
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
        if let Some(profile) = environment.profile() {
            command.args(["--claude-profile", profile]);
        }
        Err(command.exec())
    })();
    report(result)
}

fn chezmoi_source(config: &Path) -> io::Result<Option<PathBuf>> {
    if fs::symlink_metadata(config).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Ok(None);
    }
    let config = if config.is_absolute() {
        config.to_owned()
    } else {
        std::env::current_dir()?.join(config)
    };
    let output = match Command::new("chezmoi")
        .args(["source-path"])
        .arg(&config)
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !output.status.success() {
        return Ok(None);
    }
    let source = String::from_utf8(output.stdout)
        .map_err(|_| invalid("chezmoi returned a source path that is not UTF-8"))?;
    let source = source.trim();
    if source.is_empty() {
        return Err(invalid("chezmoi returned an empty source path"));
    }
    Ok(Some(PathBuf::from(source)))
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
        if let Some(source) = chezmoi_source(&config)? {
            return Err(invalid(&format!(
                "Herdr config is managed by chezmoi from {}; set terminal.default_shell to {} there, apply it, then run `dl --install`",
                source.display(),
                script.display()
            )));
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
