//! `dl --herdr-setup`: everything a new machine needs for Herdr and devlaunch to
//! work together, from devlaunch alone.
//!
//! devlaunch ships as a conda package and a wheel that carry the binaries and
//! nothing else, so every file this installs is compiled in (`assets/`) and
//! written out here. Each step is idempotent: it writes only when the content on
//! disk differs, and a second run reports every step `current`.
//!
//! The steps, in order:
//!
//! 1. the pane shell `dl-herdr-shell` (what `default_shell` names);
//! 2. `status.sh` and the agent-queue plugin, under
//!    `$XDG_DATA_HOME/devlaunch/herdr/`, then `herdr plugin link` and the
//!    plugin's startup hook, which linking does not fire;
//! 3. the Herdr config, merged ([`config`]), and refused when chezmoi owns it;
//! 4. `~/.local/bin/herdr` pointing at `~/.pixi/bin/herdr`, so `herdr machine
//!    add` does not push a stale copy;
//! 5. Claude Code: herdr's integration, the tab-title Stop hook, its
//!    `settings.json` entry ([`claude`]), and herdr's skill;
//! 6. kitty's F-key fix, in a file of its own that `kitty.conf` includes.
//!
//! It never starts, stops or reloads a Herdr server. It says when a reload is
//! needed.

mod claude;
mod config;

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::cli::HerdrSetupOptions;
use crate::commands::Ending;
use crate::herdr_environment::{ConfigOwner, chezmoi_source};
use crate::pane_shell;

const STATUS_SCRIPT: &str = include_str!("assets/status.sh");
const PLUGIN_MANIFEST: &str = include_str!("assets/agent-queue/herdr-plugin.toml");
const PLUGIN_VIEW: &str = include_str!("assets/agent-queue/view.sh");
const PLUGIN_ID: &str = "local.agent-queue";
const KITTY_FIX: &str = include_str!("assets/kitty.conf");
const KITTY_FILE: &str = "devlaunch-herdr.conf";

/// Quote `word` for a POSIX shell, leaving a plain path readable.
pub(crate) fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// How one step ended.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Something was written (or, in a dry run, would be).
    Changed(String),
    /// Already as it should be.
    Current(String),
    /// Deliberately not done, and why.
    Skipped(String),
    /// Tried and failed, and why. The run exits non-zero.
    Failed(String),
}

/// Whether a file on disk already holds what this build writes.
#[derive(Debug, PartialEq, Eq)]
enum Written {
    Current,
    Changed,
}

/// The mode a written file ends with.
#[derive(Clone, Copy)]
enum Mode {
    /// Always this mode: the file is devlaunch's.
    Fixed(u32),
    /// The mode it already had, else this one: the file is the user's.
    Keep(u32),
}

struct Paths {
    home: PathBuf,
    pane_shell: PathBuf,
    kit: PathBuf,
    herdr_config: PathBuf,
    claude: PathBuf,
    kitty: PathBuf,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

impl Paths {
    fn from_process(home: PathBuf) -> Self {
        let config_home = env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let data_home = env_path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
        Self {
            pane_shell: pane_shell::install_path(Some(&home)).expect("home supplied"),
            kit: data_home.join("devlaunch/herdr"),
            herdr_config: env_path("HERDR_CONFIG_PATH")
                .unwrap_or_else(|| config_home.join("herdr/config.toml")),
            // The same two places `herdr integration install claude` resolves, so
            // the hook, the settings and herdr's own hook land side by side.
            claude: env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            kitty: env_path("KITTY_CONFIG_DIRECTORY").unwrap_or_else(|| config_home.join("kitty")),
            home,
        }
    }

    fn status_script(&self) -> PathBuf {
        self.kit.join("status.sh")
    }

    fn plugin(&self) -> PathBuf {
        self.kit.join("plugins/agent-queue")
    }

    fn hook_script(&self) -> PathBuf {
        self.claude.join("hooks").join(claude::HOOK_NAME)
    }
}

struct Setup {
    options: HerdrSetupOptions,
    paths: Paths,
    herdr: String,
    failed: bool,
    config_changed: bool,
}

impl Setup {
    fn report(&mut self, step: &str, outcome: Outcome) {
        let (status, detail) = match outcome {
            Outcome::Changed(detail) if self.options.dry_run => ("planned", detail),
            Outcome::Changed(detail) => ("changed", detail),
            Outcome::Current(detail) => ("current", detail),
            Outcome::Skipped(detail) => ("skipped", detail),
            Outcome::Failed(detail) => {
                self.failed = true;
                ("failed", detail)
            }
        };
        eprintln!("  {status:<8} {step}: {}", self.tilde(&detail));
    }

    /// `detail` with the home directory spelled `~`, so a line fits a terminal.
    /// Text in backticks is left as it is: it is what the user pastes, and a
    /// `~` there is not what a later run expects.
    fn tilde(&self, detail: &str) -> String {
        let home = self.paths.home.to_string_lossy();
        if home.len() < 2 {
            return detail.to_owned();
        }
        let home = format!("{home}/");
        detail
            .split('`')
            .enumerate()
            .map(|(index, part)| {
                if index % 2 == 0 {
                    part.replace(&home, "~/")
                } else {
                    part.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("`")
    }

    /// "wrote" or "would write", for the message of a change.
    fn did(&self, done: &str, planned: &str) -> String {
        if self.options.dry_run {
            planned.to_owned()
        } else {
            done.to_owned()
        }
    }

    fn write(&self, path: &Path, bytes: &[u8], mode: Mode) -> io::Result<Written> {
        write_if_changed(path, bytes, mode, self.options.dry_run)
    }

    /// Report a whole-file install of one of devlaunch's own files, and give back
    /// its path when it is in place (or, in a dry run, would be).
    fn install_file(&mut self, step: &str, path: &Path, bytes: &str, mode: u32) -> Option<PathBuf> {
        let (outcome, installed) = match self.write(path, bytes.as_bytes(), Mode::Fixed(mode)) {
            Ok(Written::Current) => (Outcome::Current(path.display().to_string()), true),
            Ok(Written::Changed) => (
                Outcome::Changed(format!(
                    "{} {}",
                    self.did("wrote", "would write"),
                    path.display()
                )),
                true,
            ),
            Err(error) => (
                Outcome::Failed(format!("{}: {error}", path.display())),
                false,
            ),
        };
        self.report(step, outcome);
        installed.then(|| path.to_owned())
    }

    fn herdr(&self, args: &[&str]) -> io::Result<Output> {
        Command::new(&self.herdr)
            .args(args)
            .stdin(Stdio::null())
            .output()
    }
}

/// Write `bytes` to `path` when they differ from what is there, by a rename from
/// a temporary file in the same directory. A symlink is written through: its
/// target is replaced and the link is kept. A dangling one is an error.
fn write_if_changed(path: &Path, bytes: &[u8], mode: Mode, dry_run: bool) -> io::Result<Written> {
    let target = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::canonicalize(path)?,
        _ => path.to_owned(),
    };
    let existing = match fs::read(&target) {
        Ok(existing) => Some(existing),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let current_mode = fs::metadata(&target)
        .ok()
        .map(|metadata| metadata.permissions().mode() & 0o7777);
    let wanted_mode = match mode {
        Mode::Fixed(mode) => mode,
        Mode::Keep(fallback) => current_mode.unwrap_or(fallback),
    };
    if existing.as_deref() == Some(bytes) && current_mode == Some(wanted_mode) {
        return Ok(Written::Current);
    }
    if dry_run {
        return Ok(Written::Changed);
    }
    let directory = target
        .parent()
        .ok_or_else(|| io::Error::other("a file to write has no directory"))?;
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    io::Write::write_all(&mut temporary, bytes)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(wanted_mode))?;
    temporary.persist(&target).map_err(|e| e.error)?;
    Ok(Written::Changed)
}

/// Read a file the user owns, following a symlink; `None` when it does not exist.
fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if fs::symlink_metadata(path).is_ok() {
                // A link to nothing: writing through it would create a file
                // somewhere the user did not choose.
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "{} is a symlink to a file that does not exist",
                        path.display()
                    ),
                ));
            }
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// `Some(outcome)` when chezmoi owns `path` or cannot say, so it must not be edited.
fn chezmoi_refusal(path: &Path, instruction: &str) -> Option<Outcome> {
    match chezmoi_source(path) {
        Ok(ConfigOwner::Unmanaged) => None,
        Ok(ConfigOwner::Chezmoi(source)) => Some(Outcome::Skipped(format!(
            "chezmoi manages it from {}; {instruction} there and apply it",
            source.display()
        ))),
        Ok(ConfigOwner::ProbeFailed(complaint)) => Some(Outcome::Failed(format!(
            "chezmoi could not say whether it manages {}, so it was left alone. Fix chezmoi, or take it off PATH, and run setup again. chezmoi said: {complaint}",
            path.display()
        ))),
        Err(error) => Some(Outcome::Failed(format!(
            "could not ask chezmoi about {}: {error}",
            path.display()
        ))),
    }
}

pub(crate) fn setup(options: HerdrSetupOptions) -> Ending {
    let Some(home) = devlaunch_core::osext::home_dir() else {
        eprintln!("dl: HOME is required");
        return Ending::Refused;
    };
    let mut setup = Setup {
        options,
        paths: Paths::from_process(home),
        herdr: devlaunch_core::clients::herdr_binary_from_process()
            .unwrap_or_else(|| "herdr".to_owned()),
        failed: false,
        config_changed: false,
    };
    eprintln!(
        "Installing the Herdr kit{}",
        if options.dry_run {
            " (dry run: nothing is written)"
        } else {
            ""
        }
    );
    let shell = pane_shell_step(&mut setup);
    let status = kit_files(&mut setup);
    plugin_link(&mut setup);
    herdr_config(&mut setup, shell, status);
    local_bin_link(&mut setup);
    claude_steps(&mut setup);
    kitty_steps(&mut setup);

    if options.dry_run {
        eprintln!("Dry run: nothing was written. Run `dl --herdr-setup` to apply it.");
    } else if setup.config_changed {
        eprintln!(
            "Run `herdr server reload-config` for a running Herdr session to use the new config."
        );
    }
    eprintln!("Select a Claude login for a Herdr workspace with `dl --herdr-env profile NAME`.");
    eprintln!(
        "Optional: set DEVLAUNCH_HERDR=1 so that an agent started inside a workspace reports to its Herdr pane. It is off by default."
    );
    if setup.failed {
        Ending::Refused
    } else {
        Ending::Done
    }
}

/// The installed pane shell, or `None` when this run could not put it in place.
fn pane_shell_step(setup: &mut Setup) -> Option<PathBuf> {
    let path = setup.paths.pane_shell.clone();
    if setup.options.dry_run {
        let current = fs::read_to_string(&path).ok().as_deref() == Some(pane_shell::SCRIPT);
        let outcome = if current {
            Outcome::Current(path.display().to_string())
        } else {
            Outcome::Changed(format!("would write {}", path.display()))
        };
        setup.report("pane shell", outcome);
        return Some(path);
    }
    let (outcome, installed) = match pane_shell::install(&path) {
        pane_shell::Installed::AlreadyCurrent { path } => {
            (Outcome::Current(path.display().to_string()), Some(path))
        }
        pane_shell::Installed::Written { path } | pane_shell::Installed::Refreshed { path } => (
            Outcome::Changed(format!("wrote {}", path.display())),
            Some(path),
        ),
        pane_shell::Installed::Refused { path, reason } => (
            Outcome::Failed(format!("{}: {reason}", path.display())),
            None,
        ),
    };
    setup.report("pane shell", outcome);
    installed
}

/// The installed `status.sh`, or `None` when this run could not put it in place.
fn kit_files(setup: &mut Setup) -> Option<PathBuf> {
    let status = setup.paths.status_script();
    let status = setup.install_file("status segment", &status, STATUS_SCRIPT, 0o755);
    let plugin = setup.paths.plugin();
    setup.install_file(
        "agent-queue plugin",
        &plugin.join("herdr-plugin.toml"),
        PLUGIN_MANIFEST,
        0o644,
    );
    setup.install_file(
        "agent-queue plugin",
        &plugin.join("view.sh"),
        PLUGIN_VIEW,
        0o755,
    );
    status
}

/// The root a plugin id is linked from, out of `herdr plugin list --json`.
fn linked_root(listing: &str, id: &str) -> Result<Option<String>, String> {
    let listing: serde_json::Value =
        serde_json::from_str(listing).map_err(|e| format!("unreadable plugin list: {e}"))?;
    let plugins = listing
        .pointer("/result/plugins")
        .and_then(serde_json::Value::as_array)
        .ok_or("the plugin list has no result.plugins")?;
    Ok(plugins
        .iter()
        .find(|plugin| plugin.get("plugin_id").and_then(serde_json::Value::as_str) == Some(id))
        .map(|plugin| {
            plugin
                .get("plugin_root")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("an unknown path")
                .to_owned()
        }))
}

fn same_path(one: &Path, other: &Path) -> bool {
    one == other
        || matches!(
            (fs::canonicalize(one), fs::canonicalize(other)),
            (Ok(one), Ok(other)) if one == other
        )
}

fn herdr_missing(setup: &Setup, error: &io::Error) -> Outcome {
    if error.kind() == io::ErrorKind::NotFound {
        Outcome::Skipped(format!("{} is not installed or not on PATH", setup.herdr))
    } else {
        Outcome::Failed(format!("could not run {}: {error}", setup.herdr))
    }
}

fn stderr_of(output: &Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if text.is_empty() {
        format!("exit status {}", output.status)
    } else {
        text
    }
}

fn plugin_link(setup: &mut Setup) {
    let step = "plugin link";
    let plugin = setup.paths.plugin();
    let listing = match setup.herdr(&["plugin", "list", "--json"]) {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            let outcome = Outcome::Failed(format!(
                "`herdr plugin list` failed: {}",
                stderr_of(&output)
            ));
            return setup.report(step, outcome);
        }
        Err(error) => {
            let outcome = herdr_missing(setup, &error);
            return setup.report(step, outcome);
        }
    };
    let outcome = match linked_root(&String::from_utf8_lossy(&listing.stdout), PLUGIN_ID) {
        Err(reason) => Outcome::Failed(reason),
        Ok(Some(root)) if same_path(Path::new(&root), &plugin) => {
            Outcome::Current(format!("{PLUGIN_ID} is linked from {root}"))
        }
        Ok(Some(root)) => Outcome::Skipped(format!(
            "{PLUGIN_ID} is already linked from {root}, which is left as it is"
        )),
        Ok(None) if setup.options.dry_run => Outcome::Changed(format!(
            "would link {} and run its startup hook",
            plugin.display()
        )),
        Ok(None) => link_plugin(setup, &plugin),
    };
    setup.report(step, outcome);
}

fn link_plugin(setup: &Setup, plugin: &Path) -> Outcome {
    let directory = plugin.to_string_lossy();
    match setup.herdr(&["plugin", "link", &directory]) {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            return Outcome::Failed(format!(
                "`herdr plugin link` failed: {}",
                stderr_of(&output)
            ));
        }
        Err(error) => return Outcome::Failed(format!("could not run {}: {error}", setup.herdr)),
    }
    // `plugin link` does not fire the [[startup]] hook; only a server start does.
    // The hook exits 0 with a note when no server is running.
    let hook = Command::new(plugin.join("view.sh"))
        .arg("set")
        .current_dir(plugin)
        .stdin(Stdio::null())
        .output();
    let said = match hook {
        Ok(output) => {
            let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
            said.push_str(&String::from_utf8_lossy(&output.stderr));
            said.lines().last().unwrap_or_default().trim().to_owned()
        }
        Err(error) => format!("the startup hook did not run: {error}"),
    };
    Outcome::Changed(format!("linked {}; startup hook: {said}", plugin.display()))
}

fn herdr_config(setup: &mut Setup, shell: Option<PathBuf>, status: Option<PathBuf>) {
    let step = "herdr config";
    let (shell, status) = match (shell, status) {
        (Some(shell), Some(status)) => (shell, status),
        (None, _) => {
            let outcome = Outcome::Skipped("the pane shell is not installed".to_owned());
            return setup.report(step, outcome);
        }
        (_, None) => {
            let outcome = Outcome::Skipped("the status segment is not installed".to_owned());
            return setup.report(step, outcome);
        }
    };
    let path = setup.paths.herdr_config.clone();
    let targets = config::Targets {
        shell: shell.to_string_lossy().into_owned(),
        prototype_shell: setup
            .paths
            .home
            .join(".local/bin/herdr-workspace-shell")
            .to_string_lossy()
            .into_owned(),
        status: shell_quote(&status.to_string_lossy()),
    };
    if let Some(outcome) = chezmoi_refusal(
        &path,
        &format!(
            "set terminal.default_shell to `{}` (and take what you want from `dl --herdr-setup`'s packaged config)",
            targets.shell
        ),
    ) {
        return setup.report(step, outcome);
    }
    let original = match read_optional(&path) {
        Ok(original) => original,
        Err(error) => return setup.report(step, Outcome::Failed(error.to_string())),
    };
    let merged = match config::merge(original.as_deref().unwrap_or_default(), &targets) {
        Ok(merged) => merged,
        Err(reason) => return setup.report(step, Outcome::Failed(reason)),
    };
    for note in &merged.kept {
        eprintln!("    kept: {note}");
    }
    if original.as_deref() == Some(merged.text.as_str()) {
        return setup.report(step, Outcome::Current(path.display().to_string()));
    }
    match setup.write(&path, merged.text.as_bytes(), Mode::Keep(0o644)) {
        Ok(_) => {
            let mut detail = format!(
                "{} {}",
                setup.did("updated", "would update"),
                path.display()
            );
            if merged.fresh {
                detail.push_str(" with devlaunch's packaged config");
            } else if !merged.managed.is_empty() {
                detail.push_str(&format!("; set {}", merged.managed.join(", ")));
            }
            if !merged.added.is_empty() {
                detail.push_str(&format!("; added {} default key(s)", merged.added.len()));
            }
            setup.config_changed = true;
            setup.report(step, Outcome::Changed(detail));
            if setup.options.dry_run {
                if merged.fresh {
                    eprintln!("    add: every key in the packaged config");
                }
                for added in &merged.added {
                    eprintln!("    add: {added}");
                }
            }
        }
        Err(error) => setup.report(
            step,
            Outcome::Failed(format!("{}: {error}", path.display())),
        ),
    }
}

fn local_bin_link(setup: &mut Setup) {
    let step = "herdr on ~/.local/bin";
    let pixi = setup.paths.home.join(".pixi/bin/herdr");
    let local = setup.paths.home.join(".local/bin/herdr");
    let outcome = if fs::metadata(&pixi).is_err() {
        Outcome::Skipped(format!("no {}", pixi.display()))
    } else {
        match fs::read_link(&local) {
            Ok(target) if target == pixi => {
                Outcome::Current(format!("{} -> {}", local.display(), pixi.display()))
            }
            Ok(_) => Outcome::Skipped(format!(
                "{} already exists and is left alone",
                local.display()
            )),
            Err(_) if fs::symlink_metadata(&local).is_ok() => Outcome::Skipped(format!(
                "{} already exists and is left alone",
                local.display()
            )),
            Err(_) if setup.options.dry_run => Outcome::Changed(format!(
                "would link {} -> {}",
                local.display(),
                pixi.display()
            )),
            Err(_) => match local
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| std::os::unix::fs::symlink(&pixi, &local))
            {
                Ok(()) => {
                    Outcome::Changed(format!("linked {} -> {}", local.display(), pixi.display()))
                }
                Err(error) => Outcome::Failed(format!("{}: {error}", local.display())),
            },
        }
    };
    setup.report(step, outcome);
}

/// What `herdr integration status` says about Claude, as the word that decides.
fn claude_integration(status: &str) -> Option<&str> {
    let line = status.lines().find(|line| line.starts_with("claude:"))?;
    let state = line.trim_start_matches("claude:").trim();
    Some(state)
}

fn claude_steps(setup: &mut Setup) {
    if !setup.options.claude {
        return setup.report("claude", Outcome::Skipped("--no-claude".to_owned()));
    }
    let claude = setup.paths.claude.clone();
    if !claude.is_dir() {
        return setup.report(
            "claude",
            Outcome::Skipped(format!(
                "no Claude Code config directory at {}",
                claude.display()
            )),
        );
    }
    integration(setup);
    if settings(setup) == TabTitle::Ours {
        let hook = setup.paths.hook_script();
        setup.install_file("claude tab-title hook", &hook, claude::HOOK_SCRIPT, 0o755);
    }
    skill(setup);
}

fn integration(setup: &mut Setup) {
    let step = "claude integration";
    let status = match setup.herdr(&["integration", "status"]) {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        Ok(output) => {
            let outcome = Outcome::Failed(format!(
                "`herdr integration status` failed: {}",
                stderr_of(&output)
            ));
            return setup.report(step, outcome);
        }
        Err(error) => {
            let outcome = herdr_missing(setup, &error);
            return setup.report(step, outcome);
        }
    };
    let state = claude_integration(&status)
        .unwrap_or("not reported")
        .to_owned();
    // Only when it is not current: each install used to append another copy of its
    // SessionStart entry to settings.json.
    if state.starts_with("current") {
        return setup.report(step, Outcome::Current(state));
    }
    let settings = setup.paths.claude.join("settings.json");
    let instruction = format!(
        "add the {} hooks that `herdr integration install claude` writes",
        claude::HERDR_HOOK
    );
    if let Some(outcome) = chezmoi_refusal(&settings, &instruction) {
        return setup.report(step, outcome);
    }
    if setup.options.dry_run {
        return setup.report(
            step,
            Outcome::Changed(format!(
                "would run `herdr integration install claude` ({state})"
            )),
        );
    }
    let outcome = match setup.herdr(&["integration", "install", "claude"]) {
        Ok(output) if output.status.success() => Outcome::Changed(format!(
            "ran `herdr integration install claude` (was {state})"
        )),
        Ok(output) => Outcome::Failed(format!(
            "`herdr integration install claude` failed: {}",
            stderr_of(&output)
        )),
        Err(error) => Outcome::Failed(format!("could not run {}: {error}", setup.herdr)),
    };
    setup.report(step, outcome);
}

/// Whose Stop hook sets the tab title, as settings.json says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabTitle {
    /// devlaunch's, so its script is installed.
    Ours,
    /// A herdr-tab-title.sh from elsewhere, so devlaunch's script is not installed.
    Other,
    /// settings.json could not be read, so no script is installed that nothing runs.
    Unknown,
}

/// Merge the Stop hook into settings.json, and say whose tab-title hook it holds.
fn settings(setup: &mut Setup) -> TabTitle {
    let step = "claude settings";
    let path = setup.paths.claude.join("settings.json");
    let hook = setup.paths.hook_script();
    let command = claude::hook_command(&hook.to_string_lossy());
    // Read and merge first, writing nothing: whether a tab-title hook from
    // elsewhere is registered decides the script step even when chezmoi owns
    // this file and the merge cannot be written.
    let read = read_optional(&path)
        .map_err(|error| error.to_string())
        .and_then(|original| {
            claude::merge(original.as_deref(), &command)
                .map(|merged| (original, merged))
                .map_err(|reason| format!("{}: {reason}", path.display()))
        });
    let tab_title = match &read {
        Ok((_, merged)) if merged.other_tab_title => TabTitle::Other,
        Ok(_) => TabTitle::Ours,
        Err(_) => TabTitle::Unknown,
    };
    let other_tab_title = tab_title == TabTitle::Other;
    if other_tab_title {
        setup.report(
            "claude tab-title hook",
            Outcome::Skipped(
                "a herdr-tab-title.sh Stop hook is already registered, so devlaunch's is not added"
                    .to_owned(),
            ),
        );
    }
    let add = format!("add a Stop hook with the command `{command}`");
    let instruction = match &read {
        Ok((_, merged)) => {
            let mut steps = Vec::new();
            if merged.registered && !merged.runs_ours {
                steps.push(add);
            }
            if merged.removed > 0 {
                steps.push(format!("remove {} duplicate hook(s)", merged.removed));
            }
            (!steps.is_empty()).then(|| steps.join(" and "))
        }
        Err(_) => Some(add),
    };
    if let Some(outcome) = chezmoi_refusal(&path, instruction.as_deref().unwrap_or_default()) {
        let outcome = match (outcome, instruction) {
            (Outcome::Skipped(_), None) => {
                Outcome::Current("chezmoi manages it, and it needs no change".to_owned())
            }
            (outcome, _) => outcome,
        };
        setup.report(step, outcome);
        return tab_title;
    }
    let (original, merged) = match read {
        Ok(read) => read,
        Err(reason) => {
            setup.report(step, Outcome::Failed(reason));
            return tab_title;
        }
    };
    let Some(text) = merged.text else {
        setup.report(step, Outcome::Current(path.display().to_string()));
        return tab_title;
    };
    let mut changes = Vec::new();
    if merged.registered {
        changes.push("registered the tab-title Stop hook".to_owned());
    }
    if merged.removed > 0 {
        changes.push(format!("removed {} duplicate hook(s)", merged.removed));
    }
    let outcome = match backup_once(setup, &path, original.as_deref()).and_then(|backup| {
        setup
            .write(&path, text.as_bytes(), Mode::Keep(0o644))
            .map(|_| backup)
    }) {
        Ok(backup) => {
            let mut detail = format!(
                "{} {}: {}",
                setup.did("updated", "would update"),
                path.display(),
                changes.join(", ")
            );
            if let Some(backup) = backup {
                detail.push_str(&format!(
                    "; {} {}",
                    setup.did("backed up the original to", "would back up the original to"),
                    backup.display()
                ));
            }
            Outcome::Changed(detail)
        }
        Err(error) => Outcome::Failed(format!("{}: {error}", path.display())),
    };
    setup.report(step, outcome);
    tab_title
}

/// Copy settings.json aside the first time devlaunch changes it, and never again.
fn backup_once(setup: &Setup, path: &Path, original: Option<&str>) -> io::Result<Option<PathBuf>> {
    let Some(original) = original else {
        return Ok(None);
    };
    let backup = path.with_file_name("settings.json.devlaunch-backup");
    if fs::symlink_metadata(&backup).is_ok() {
        return Ok(None);
    }
    if !setup.options.dry_run {
        write_if_changed(&backup, original.as_bytes(), Mode::Keep(0o600), false)?;
    }
    Ok(Some(backup))
}

fn skill(setup: &mut Setup) {
    let step = "claude herdr skill";
    let path = setup.paths.claude.join("skills/herdr/SKILL.md");
    // A herdr that dropped or broke `--skill` prints a usage error or nothing;
    // writing either over a working skill is worse than keeping the old one.
    let skill = match setup.herdr(&["--skill"]) {
        Ok(output)
            if output.status.success() && !output.stdout.iter().all(u8::is_ascii_whitespace) =>
        {
            output.stdout
        }
        Ok(output) if output.status.success() => {
            let outcome = Outcome::Skipped(
                "`herdr --skill` printed nothing; the existing skill is kept".to_owned(),
            );
            return setup.report(step, outcome);
        }
        Ok(output) => {
            let outcome = Outcome::Skipped(format!(
                "`herdr --skill` failed ({}); the existing skill is kept",
                stderr_of(&output)
            ));
            return setup.report(step, outcome);
        }
        Err(error) => {
            let outcome = herdr_missing(setup, &error);
            return setup.report(step, outcome);
        }
    };
    let outcome = match setup.write(&path, &skill, Mode::Keep(0o644)) {
        Ok(Written::Current) => Outcome::Current(path.display().to_string()),
        Ok(Written::Changed) => Outcome::Changed(format!(
            "{} {}",
            setup.did("wrote", "would write"),
            path.display()
        )),
        Err(error) => Outcome::Failed(format!("{}: {error}", path.display())),
    };
    setup.report(step, outcome);
}

fn has_include(kitty_conf: &str) -> bool {
    kitty_conf
        .lines()
        .any(|line| line.split_whitespace().collect::<Vec<_>>() == ["include", KITTY_FILE])
}

fn kitty_steps(setup: &mut Setup) {
    if !setup.options.kitty {
        return setup.report("kitty", Outcome::Skipped("--no-kitty".to_owned()));
    }
    let directory = setup.paths.kitty.clone();
    if !directory.is_dir() {
        return setup.report(
            "kitty",
            Outcome::Skipped(format!(
                "no kitty config directory at {}",
                directory.display()
            )),
        );
    }
    setup.install_file(
        "kitty F-key fix",
        &directory.join(KITTY_FILE),
        KITTY_FIX,
        0o644,
    );

    let step = "kitty.conf include";
    let line = format!("include {KITTY_FILE}");
    let conf = directory.join("kitty.conf");
    let original = match read_optional(&conf) {
        Ok(original) => original,
        Err(error) => return setup.report(step, Outcome::Failed(error.to_string())),
    };
    if original.as_deref().is_some_and(has_include) {
        return setup.report(
            step,
            Outcome::Current(format!("{} has `{line}`", conf.display())),
        );
    }
    if original.is_some()
        && let Some(outcome) = chezmoi_refusal(&conf, &format!("add the line `{line}`"))
    {
        return setup.report(step, outcome);
    }
    let mut text = original.unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str("# The F-key fix for herdr, written by `dl --herdr-setup`.\n");
    text.push_str(&line);
    text.push('\n');
    let outcome = match setup.write(&conf, text.as_bytes(), Mode::Keep(0o644)) {
        Ok(_) => Outcome::Changed(format!(
            "{} `{line}` to {}",
            setup.did("added", "would add"),
            conf.display()
        )),
        Err(error) => Outcome::Failed(format!("{}: {error}", conf.display())),
    };
    setup.report(step, outcome);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_stay_readable_and_others_are_quoted() {
        assert_eq!(
            shell_quote("/home/u/.local/share/x.sh"),
            "/home/u/.local/share/x.sh"
        );
        assert_eq!(shell_quote("/home/my user/x.sh"), "'/home/my user/x.sh'");
        assert_eq!(shell_quote("/it's"), r"'/it'\''s'");
    }

    #[test]
    fn the_plugin_list_names_where_an_id_is_linked_from() {
        let listing = r#"{"id":"cli:plugin","result":{"plugins":[{"plugin_id":"local.agent-queue","plugin_root":"/p/agent-queue"},{"plugin_id":"local.telemetry","plugin_root":"/p/t"}],"type":"plugin_list"}}"#;
        assert_eq!(
            linked_root(listing, PLUGIN_ID).unwrap().as_deref(),
            Some("/p/agent-queue")
        );
        assert_eq!(linked_root(listing, "local.other").unwrap(), None);
        assert!(linked_root("{}", PLUGIN_ID).is_err());
    }

    #[test]
    fn the_claude_line_of_integration_status_is_read() {
        let status = "pi: outdated (v8 < v9) (/h/.pi/x.ts)\nclaude: current (v10) (/h/.claude/hooks/herdr-agent-state.sh)\n";
        assert_eq!(
            claude_integration(status),
            Some("current (v10) (/h/.claude/hooks/herdr-agent-state.sh)")
        );
        assert_eq!(claude_integration("pi: current\n"), None);
    }

    #[test]
    fn the_include_line_is_found_however_it_is_spaced() {
        assert!(has_include(
            "font_size 12\n  include   devlaunch-herdr.conf  \n"
        ));
        assert!(!has_include("# include devlaunch-herdr.conf\n"));
        assert!(!has_include("include other.conf\n"));
    }

    #[test]
    fn a_file_is_written_once_through_a_symlink_and_keeps_the_link() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("real.toml");
        let link = root.path().join("link.toml");
        fs::write(&target, "old").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink("real.toml", &link).unwrap();
        assert_eq!(
            write_if_changed(&link, b"new", Mode::Keep(0o644), true).unwrap(),
            Written::Changed
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "old",
            "a dry run writes nothing"
        );
        assert_eq!(
            write_if_changed(&link, b"new", Mode::Keep(0o644), false).unwrap(),
            Written::Changed
        );
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("real.toml"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(
            write_if_changed(&link, b"new", Mode::Keep(0o644), false).unwrap(),
            Written::Current
        );
    }

    #[test]
    fn a_script_with_the_right_bytes_and_the_wrong_mode_is_fixed() {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("status.sh");
        fs::write(&script, STATUS_SCRIPT).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            write_if_changed(&script, STATUS_SCRIPT.as_bytes(), Mode::Fixed(0o755), false).unwrap(),
            Written::Changed
        );
        assert_eq!(
            fs::metadata(&script).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn a_dangling_symlink_is_refused_and_left() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("config.toml");
        std::os::unix::fs::symlink("missing.toml", &link).unwrap();
        assert!(read_optional(&link).is_err());
        assert!(write_if_changed(&link, b"x", Mode::Keep(0o644), false).is_err());
        assert!(!root.path().join("missing.toml").exists());
    }
}
