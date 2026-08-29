//! The fake devpod behind the fake runner: a workspace state machine.
//!
//! The Python `devpod_mock` fixture's design (retired with the Python tree in
//! #267), re-homed behind the typed
//! [`Runner`](devlaunch_runner::Runner) seam and brought up to the
//! fidelity `test/fixtures/devpod_shim.py` proved out. What matters is that the
//! shapes are real devpod's shapes, because the code under test parses them:
//!
//! - `list --output json` is an **array of workspace objects carrying no state
//!   field**. Real devpod answers state only to `status`, per workspace, and a
//!   listing that included it would let a port pass tests while reading a field
//!   devpod does not send.
//! - `ssh` **starts a stopped workspace**, as real devpod does.
//! - a workspace it has never heard of, or a command it does not know, **exits 1
//!   with the refusal on stderr** rather than quietly succeeding.
//!
//! Anything a test wants to happen differently — a mid-provision failure,
//! malformed output, a provider error — is scripted in the response table,
//! which short-circuits this machine entirely.
//!
//! This is not the only fake devpod in the repo, and that is what
//! `test/fixtures/devpod/conformance.json` is for: an argv→outcome table this
//! machine and `test/fixtures/devpod_shim.py` are both driven over, so neither
//! can quietly become stricter than real devpod again — which is how a `delete
//! --ignore-not-found` that refused survived here for months after the shim was
//! fixed for it. A behaviour change in here belongs in a corpus row first; see
//! the `conformance` module below.
//!
//! Those two are the whole population the corpus has to cover, one per language,
//! and the reason is what each of the other argv readers is. The `Runner`
//! wrappers the unit tests define reach *this* machine through
//! [`FakeRunner`](crate::FakeRunner) rather than faking devpod themselves; the
//! ones that answer without consulting argv at all — `flows::provision`'s
//! `Trips` — are recorders, absent here rather than missing. The response table
//! above is the third argv reader, and it is deliberately outside the corpus: a
//! scripted entry is one test saying what it wants back from one call, not a
//! claim about what real devpod does, and pinning it against a fixture would
//! freeze the exception rather than the behaviour. What the corpus holds is
//! everything that answers *as devpod*.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::json;

use crate::response::Response;

/// Whether a workspace's container is up. devpod's own two words for it; the
/// richer container-state vocabulary (with its `Unknown` arm) belongs to the
/// domain model, not to this fake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceState {
    Running,
    Stopped,
}

impl WorkspaceState {
    /// The word devpod prints and puts in its JSON.
    pub fn as_devpod_word(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Stopped => "Stopped",
        }
    }
}

/// Where a workspace came from. devpod records a path one way and a URL another,
/// and `dl --purge` decides ownership by looking at exactly this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    LocalFolder(String),
    GitRepository(String),
}

impl Source {
    /// Which of the two devpod would record for `raw`.
    pub fn classify(raw: &str) -> Self {
        let looks_remote = raw.contains("://") || raw.starts_with("git@");
        if !looks_remote
            && (raw.starts_with('/')
                || raw.starts_with('.')
                || raw.starts_with('~')
                || Path::new(raw).exists())
        {
            Self::LocalFolder(raw.to_string())
        } else {
            Self::GitRepository(raw.to_string())
        }
    }

    fn as_json(&self) -> serde_json::Value {
        match self {
            Self::LocalFolder(path) => json!({ "localFolder": path }),
            Self::GitRepository(url) => json!({ "gitRepository": url }),
        }
    }
}

/// One workspace, as this machine holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeWorkspace {
    pub id: String,
    pub source: Source,
    pub state: WorkspaceState,
    pub provider: String,
    pub ide: String,
    pub context: String,
    pub last_used: String,
}

impl FakeWorkspace {
    /// A workspace with devpod's defaults for everything a test did not name.
    pub fn new(id: impl Into<String>, source: Source, state: WorkspaceState) -> Self {
        let id = id.into();
        Self {
            id,
            source,
            state,
            provider: "docker".to_string(),
            ide: "none".to_string(),
            context: "default".to_string(),
            last_used: DEFAULT_STAMP.to_string(),
        }
    }

    /// The `devpod list` entry: everything but the state, which real devpod
    /// answers only to `status`.
    fn as_listing(&self) -> serde_json::Value {
        json!({
            "id": self.id,
            "source": self.source.as_json(),
            "lastUsed": self.last_used,
            "provider": { "name": self.provider },
            "ide": { "name": self.ide },
            "context": self.context,
        })
    }
}

/// The stamp a workspace gets unless a test says otherwise. Fixed rather than
/// "now", so a test may compare a whole listing without consulting a clock.
pub const DEFAULT_STAMP: &str = "2026-01-01T00:00:00+0000";

/// The value-taking flags every subcommand inherits, per real devpod v0.26.1's
/// global flag block. `--debug` and `--silent` are bare and so are absent.
const GLOBAL_VALUE_FLAGS: &[&str] = &["--context", "--devpod-home", "--log-output", "--provider"];

/// `devpod up` flags that take a separate value, so the positional source can be
/// found by skipping them.
///
/// Taken from `devpod up --help` at v0.26.1 rather than grown one bug at a time:
/// every flag it types `string`, `strings`, `stringArray` or a named type is
/// here, and the booleans (`--recreate`, `--reset`, `--open-ide`, …) are not,
/// because cobra takes a boolean's value only as `--flag=false`. Growing the
/// list by hand as flags were needed is what left `--init-env`, `--mount`,
/// `--dotfiles-script` and `--dotfiles-script-env` out of it, and real devpod
/// accepts those *before* the positional source, where reading one as bare makes
/// its value the workspace source.
const UP_VALUE_FLAGS: &[&str] = &[
    "--additional-features",
    "--devcontainer-id",
    "--devcontainer-image",
    "--devcontainer-path",
    "--dotfiles",
    "--dotfiles-script",
    "--dotfiles-script-env",
    "--dotfiles-script-env-file",
    "--extra-devcontainer-path",
    "--fallback-image",
    "--gidmap",
    "--git-clone-strategy",
    "--git-ssh-signing-key",
    "--id",
    "--ide",
    "--ide-option",
    "--init-env",
    "--machine",
    "--mount",
    "--prebuild-repository",
    "--provider-option",
    "--source",
    "--ssh-config",
    "--uidmap",
    "--userns",
    "--workspace-env",
    "--workspace-env-file",
];

/// `devpod ssh` flags that take a separate value, per `devpod ssh --help` at
/// v0.26.1. `--workdir` is the one `dl` sends and the one both fakes read as
/// bare, which made an attach with a working directory look like an attach to a
/// workspace named after that directory.
const SSH_VALUE_FLAGS: &[&str] = &[
    "--command",
    "--forward-ports",
    "-L",
    "--forward-ports-timeout",
    "--git-ssh-signing-key",
    "--reverse-forward-ports",
    "-R",
    "--send-env",
    "--set-env",
    "--ssh-keepalive-interval",
    "--term-mode",
    "--user",
    "--workdir",
];

/// `devpod delete` flags that take a separate value. `--force` and
/// `--ignore-not-found` are bare and `--grace-period` is a string, per
/// `devpod delete --help` at v0.26.1.
const DELETE_VALUE_FLAGS: &[&str] = &["--grace-period"];

/// `devpod status` flags that take a separate value. `--container-status` is a
/// boolean, per `devpod status --help` at v0.26.1.
const STATUS_VALUE_FLAGS: &[&str] = &["--output", "--timeout"];

/// `devpod stop` has no flags of its own at v0.26.1.
const STOP_VALUE_FLAGS: &[&str] = &[];

/// The workspaces and providers a fake devpod knows about.
#[derive(Clone, Debug)]
pub struct DevpodMachine {
    workspaces: BTreeMap<String, FakeWorkspace>,
    providers: BTreeSet<String>,
    /// What `up` stamps `lastUsed` with.
    pub stamp: String,
}

impl Default for DevpodMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl DevpodMachine {
    /// An empty machine with `docker` registered, which is the state a developer
    /// host is in.
    pub fn new() -> Self {
        Self {
            workspaces: BTreeMap::new(),
            providers: BTreeSet::from(["docker".to_string()]),
            stamp: DEFAULT_STAMP.to_string(),
        }
    }

    pub fn insert(&mut self, workspace: FakeWorkspace) {
        self.workspaces.insert(workspace.id.clone(), workspace);
    }

    pub fn remove(&mut self, id: &str) -> Option<FakeWorkspace> {
        self.workspaces.remove(id)
    }

    pub fn get(&self, id: &str) -> Option<&FakeWorkspace> {
        self.workspaces.get(id)
    }

    /// The state of one workspace, or `None` when there is no such workspace —
    /// the distinction `devpod status` answers with an exit code.
    pub fn state_of(&self, id: &str) -> Option<WorkspaceState> {
        self.workspaces.get(id).map(|workspace| workspace.state)
    }

    /// Set a workspace's state. Does nothing if there is no such workspace;
    /// a test that wants one creates it.
    pub fn set_state(&mut self, id: &str, state: WorkspaceState) {
        if let Some(workspace) = self.workspaces.get_mut(id) {
            workspace.state = state;
        }
    }

    pub fn ids(&self) -> Vec<String> {
        self.workspaces.keys().cloned().collect()
    }

    pub fn add_provider(&mut self, name: impl Into<String>) {
        self.providers.insert(name.into());
    }

    pub fn provider_names(&self) -> Vec<String> {
        self.providers.iter().cloned().collect()
    }

    /// Answer one `devpod` call, where `args` is the argv after `devpod`.
    pub fn answer(&mut self, args: &[String]) -> Response {
        let Some((command, rest)) = args.split_first() else {
            return refusal("devpod-fake: no command given");
        };
        match command.as_str() {
            "version" => Response::stdout("devpod version v0.26.1-fake\n"),
            "up" => self.up(rest),
            "stop" => self.stop(rest),
            "delete" => self.delete(rest),
            "list" => self.list(rest),
            "status" => self.status(rest),
            "ssh" => self.ssh(rest),
            "provider" => self.provider(rest),
            "context" => Self::context(rest),
            other => refusal(&format!(
                "devpod-fake: unknown command \"{other}\" for \"devpod\""
            )),
        }
    }

    fn up(&mut self, args: &[String]) -> Response {
        let (positionals, flags) = split_args(args, UP_VALUE_FLAGS);
        let Some(source) = positionals.first() else {
            return refusal("devpod-fake: up: no workspace source given");
        };
        let id = match flags.get("--id") {
            Some(Flag::Value(id)) => id.clone(),
            // A restart addresses an existing workspace by its id.
            _ if self.workspaces.contains_key(source) => source.clone(),
            _ => derive_id(source),
        };
        let stamp = self.stamp.clone();
        match self.workspaces.get_mut(&id) {
            Some(existing) => {
                existing.state = WorkspaceState::Running;
                existing.last_used = stamp;
            }
            None => {
                let mut workspace = FakeWorkspace::new(
                    id.clone(),
                    Source::classify(source),
                    WorkspaceState::Running,
                );
                workspace.last_used = stamp;
                if let Some(Flag::Value(ide)) = flags.get("--ide") {
                    workspace.ide = ide.clone();
                }
                self.insert(workspace);
            }
        }
        Response::stdout(format!("Workspace {id} is ready\n"))
    }

    fn stop(&mut self, args: &[String]) -> Response {
        let (positionals, _) = split_args(args, STOP_VALUE_FLAGS);
        let Some(id) = positionals.first() else {
            return refusal("devpod-fake: stop: no workspace given");
        };
        if !self.workspaces.contains_key(id) {
            return not_found("stop", id);
        }
        self.set_state(id, WorkspaceState::Stopped);
        Response::ok()
    }

    fn delete(&mut self, args: &[String]) -> Response {
        let (positionals, flags) = split_args(args, DELETE_VALUE_FLAGS);
        let Some(id) = positionals.first() else {
            return refusal("devpod-fake: delete: no workspace given");
        };
        if self.remove(id).is_none() {
            // `--ignore-not-found` makes a delete mean "ensure absent", the way
            // `rm -f` does, and real devpod v0.26.1 exits 0 for it. Refusing
            // instead is the one thing a fake devpod must not do: `dl <ws> rm
            // --force` passes the flag on every forced remove, so a run against
            // a workspace that was already gone failed here and succeeded
            // against the real thing. Nothing goes to stdout because the line
            // real devpod prints is a timestamped log line no fake can spell.
            if flags.contains_key("--ignore-not-found") {
                return Response::ok();
            }
            return not_found("delete", id);
        }
        Response::stdout(format!("Successfully deleted workspace {id}\n"))
    }

    fn list(&self, args: &[String]) -> Response {
        let entries: Vec<serde_json::Value> = self
            .workspaces
            .values()
            .map(FakeWorkspace::as_listing)
            .collect();
        if wants_json(args) {
            return Response::stdout(format!("{}\n", json!(entries)));
        }
        let table: String = self
            .workspaces
            .values()
            .map(|workspace| {
                format!(
                    "{}  {}  {}\n",
                    workspace.id, workspace.provider, workspace.last_used
                )
            })
            .collect();
        Response::stdout(table)
    }

    fn status(&self, args: &[String]) -> Response {
        let (positionals, _) = split_args(args, STATUS_VALUE_FLAGS);
        let Some(id) = positionals.first() else {
            return refusal("devpod-fake: status: no workspace given");
        };
        let Some(workspace) = self.workspaces.get(id) else {
            return not_found("status", id);
        };
        if wants_json(args) {
            return Response::stdout(format!(
                "{}\n",
                json!({
                    "id": workspace.id,
                    "context": workspace.context,
                    "provider": workspace.provider,
                    "state": workspace.state.as_devpod_word(),
                })
            ));
        }
        Response::stdout(format!(
            "Workspace {} is {}\n",
            workspace.id,
            workspace.state.as_devpod_word()
        ))
    }

    fn ssh(&mut self, args: &[String]) -> Response {
        let (positionals, _) = split_args(args, SSH_VALUE_FLAGS);
        let Some(id) = positionals.first() else {
            return refusal("devpod-fake: ssh: no workspace given");
        };
        if !self.workspaces.contains_key(id) {
            return not_found("ssh", id);
        }
        // Real devpod starts a stopped workspace to ssh into it, which is why
        // `dl` can attach without an `up` of its own.
        self.set_state(id, WorkspaceState::Running);
        // What the remote command printed is not something this can invent: a
        // test that needs output from `ssh --command` scripts it.
        Response::ok()
    }

    fn provider(&mut self, args: &[String]) -> Response {
        let Some((subcommand, rest)) = args.split_first() else {
            return refusal("devpod-fake: provider: missing subcommand");
        };
        match (subcommand.as_str(), rest.first()) {
            ("list", _) => {
                if wants_json(args) {
                    let listing: BTreeMap<&String, serde_json::Value> = self
                        .providers
                        .iter()
                        .map(|name| (name, json!({ "config": { "name": name } })))
                        .collect();
                    Response::stdout(format!("{}\n", json!(listing)))
                } else {
                    Response::stdout(
                        self.providers
                            .iter()
                            .map(|name| format!("{name}\n"))
                            .collect::<String>(),
                    )
                }
            }
            ("add", Some(name)) => {
                self.add_provider(name.clone());
                Response::ok()
            }
            ("use", Some(name)) => {
                if self.providers.contains(name) {
                    Response::ok()
                } else {
                    Response::exited(1)
                }
            }
            (other, _) => refusal(&format!(
                "devpod-fake: provider: unknown subcommand '{other}'"
            )),
        }
    }

    fn context(args: &[String]) -> Response {
        match args.first().map(String::as_str) {
            Some("options") => Response::stdout("{}\n"),
            other => refusal(&format!(
                "devpod-fake: context: unknown subcommand {other:?}"
            )),
        }
    }
}

/// devpod refused, and this is what it said about it. Exit 1 with the complaint
/// on stderr, as the shim does — Tier 2 is what keeps both honest about it.
fn refusal(message: &str) -> Response {
    Response::failed(1, format!("{message}\n"))
}

fn not_found(verb: &str, id: &str) -> Response {
    refusal(&format!(
        "devpod-fake: {verb}: couldn't find workspace {id}"
    ))
}

/// Whether this call asked for JSON, the way `dl` asks: `--output json`.
fn wants_json(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--output") && args.iter().any(|arg| arg == "json")
}

/// A flag either carried a value or stood on its own, and the two are different
/// facts about the call.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Flag {
    Value(String),
    Bare,
}

/// The positionals and flags of one devpod subcommand's argv.
///
/// `value_flags` is the subcommand's own set; every subcommand's globals are
/// added here, because real devpod inherits them everywhere.
fn split_args(args: &[String], value_flags: &[&str]) -> (Vec<String>, BTreeMap<String, Flag>) {
    let mut positionals = Vec::new();
    let mut flags = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if (value_flags.contains(&arg.as_str()) || GLOBAL_VALUE_FLAGS.contains(&arg.as_str()))
            && index + 1 < args.len()
        {
            flags.insert(arg.clone(), Flag::Value(args[index + 1].clone()));
            index += 2;
        } else if arg.starts_with('-') {
            match arg.split_once('=') {
                Some((name, value)) => {
                    flags.insert(name.to_string(), Flag::Value(value.to_string()));
                }
                None => {
                    flags.insert(arg.clone(), Flag::Bare);
                }
            }
            index += 1;
        } else {
            positionals.push(arg.clone());
            index += 1;
        }
    }
    (positionals, flags)
}

/// The id devpod makes up when `up` is given no `--id`: the source's last
/// path-ish segment, lowercased and squeezed to `[a-z0-9-]`.
fn derive_id(source: &str) -> String {
    let trimmed = source.trim_end_matches('/');
    let without_git = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let tail = without_git.rsplit('/').next().unwrap_or(without_git);
    let mut derived = String::new();
    for character in tail.to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            derived.push(character);
        } else if !derived.ends_with('-') {
            derived.push('-');
        }
    }
    let derived = derived.trim_matches('-').to_string();
    if derived.is_empty() {
        "workspace".to_string()
    } else {
        derived
    }
}

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod tests;
