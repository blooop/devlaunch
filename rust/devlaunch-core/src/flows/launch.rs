//! Opening a workspace: from a spec on the command line to a session in a
//! container.
//!
//! Ported from the launch half of `dl.py` — `workspace_up`, `workspace_ssh`,
//! `attach_workspace`, `dotfiles_update`, `get_context_options`,
//! `_pixi_cache_up_args`, `_launch_lock_path` and the spec-resolution and
//! fast-attach arms of `_run_cli`. See docs/rust-rewrite-plan.md (M7). This is
//! the one flow the plan calls *designed fresh* rather than ported shape-first:
//! `dl.py` spreads it over a 300-line `if` chain in `_run_cli` plus four helpers
//! that pass each other tuples, and what it is actually made of is four stages.
//!
//! # Four stages, and each one's outcome is what the next matches on
//!
//! ```text
//!   spec  ──plan──▶  Plan  ──resolve──▶  Resolution  ──prepare──▶  Placement
//!                                                                      │
//!                                             ┌────────up─────────────-┘
//!                                             ▼
//!                                        UpOutcome  ──attach──▶  Session
//! ```
//!
//! - [`plan`] classifies the spec. Pure, and the only place that decides whether
//!   a launch *could* be cold.
//! - [`resolve_triple`] asks devpod which workspace a triple is, and answers
//!   [`Resolution`]: `Warm` when devpod knows it, `Cold` when it does not. The
//!   warm arm carries a [`Placement`] and has read no `metadata.json` — that is
//!   devlaunch#145, and here it is a property of the signature rather than a
//!   promise in a docstring (see [`ColdMachinery`]).
//! - [`prepare`] is the cold arm's host-side work, delegated whole to
//!   [`WorkspaceCloneManager::prepare_cold`] (one repo lock for the clone, the
//!   fetch, the branch and the workspace clone — devlaunch#200).
//! - [`workspace_up`] is `devpod up`, serialized against a sibling launch of the
//!   same workspace on a per-workspace lock.
//! - [`attach_workspace`] hands the workspace over: one round trip, and it is the
//!   session.
//!
//! # What the sums buy over `dl.py`'s tuples
//!
//! `_run_cli` carries four correlated locals — `workspace_spec`, `workspace_id`,
//! `custom_id`, `known_state` — and reads the fast-attach condition off two of
//! them (`custom_id is None and known_state == "Running"`). Three of the sixteen
//! combinations are meaningful; the rest are unreachable only because three
//! separate arms happen to fill them consistently. [`Placement`] is those three
//! combinations and nothing else, so "is this launch warm" is a question about
//! one value.
//!
//! Same for the `up` request. Python takes `workspace_id`, `workspace_identity`,
//! `recreate: bool` and `reset: bool`; [`Naming`] is the three shapes the first
//! two are ever in (and makes "an `--id` with no identity" unrepresentable), and
//! [`Rebuild`] is a sum so `--recreate --reset` cannot be asked for at once.
//!
//! # Nothing here prints
//!
//! Every `logging.*` call and every `print` on this path is an arm of
//! [`LaunchNotice`] carrying exactly what that line interpolated; the words, the
//! levels and the exit codes are the `dl` binary's (#251 §5). The one thing that
//! *is* a string here is a remote payload — `bash -lc <quoted>` — because those
//! bytes are a contract with a shell rather than prose for a person.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::clients::devpod::{self, Call, ContainerState, ListingUnreadable, NotRun};
use crate::clients::gh::{self, GhEvent, StagedToken, Token, TokenLookup};
use crate::clients::ssh;
use crate::domain::locks::{self, Contention, LockError};
use crate::domain::metadata::MetadataStorage;
use crate::domain::spec::{self, DevcontainerPath, SpecIdentity, WorkspaceSpec};
use crate::domain::workspace_id::{NamePart, UnsafeName, WorkspaceId, validate_ref_name};
use crate::flows::lifecycle::{
    self, KnownWorkspace, LifecycleNotice, Refresh, RefreshReason, StopOutcome,
};
use crate::flows::listing::CommandContext;
use crate::flows::provision::DevpodMissing;
use crate::flows::repo_manager::CacheNotice;
use crate::flows::repo_manager::EnsureRepoError;
use crate::flows::workspace_clone::{PrepareColdError, WorkspaceCloneManager};
use crate::notices::{Notices, Wrapped};
use crate::runner::{Exit, Runner};
use crate::shell;
use crate::timing;

// ===========================================================================
// the vocabulary the host is read through
// ===========================================================================

/// `DEVLAUNCH_DOTFILES_ON_ATTACH`: refresh the dotfiles before handing over an
/// interactive shell.
pub(crate) const DOTFILES_ON_ATTACH_VAR: &str = "DEVLAUNCH_DOTFILES_ON_ATTACH";

/// `DEVLAUNCH_ZELLIJ`: put `dl <spec> -- <cmd>` beside a zellij session (#242).
pub(crate) const ZELLIJ_WRAP_VAR: &str = "DEVLAUNCH_ZELLIJ";

/// The one session name every workspace uses.
///
/// One fixed name rather than one per workspace, because a zellij server lives
/// *inside* a container and dies with it: two workspaces cannot collide on this
/// name, since neither can see the other's sessions. That makes the name a
/// constant a human can type without looking it up, which is the whole of the
/// documented interface — `zellij -s devlaunch action new-pane -- <cmd>`.
pub(crate) const ZELLIJ_SESSION: &str = "devlaunch";

/// The variable a project's host-side `initializeCommand` reads to tell branch
/// workspaces apart; devpod gives the hook no workspace identity of its own (see
/// docs/devcontainer-projects.md).
pub(crate) const WORKSPACE_ID_VAR: &str = "DEVLAUNCH_WORKSPACE_ID";

/// What the opt-in refresh may spend before it gives up and hands over the shell.
///
/// Generous enough that a real `chezmoi update` plus `pixi global sync` finishes
/// inside it on a working remote, short enough that an unreachable one is a pause
/// rather than a hang. Not a knob: the failure it guards is already survivable —
/// the refresh is best-effort and the shell arrives either way.
pub(crate) const DOTFILES_ATTACH_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a cached copy of devpod's context options is believed.
pub(crate) const CONTEXT_OPTIONS_TTL: Duration = Duration::from_secs(3600);

/// Where the shared pixi package cache is bound inside a container, and the value
/// `PIXI_CACHE_DIR` takes there.
///
/// Outside every home directory, which devlaunch#240 measured the cost of getting
/// wrong: a bind target whose parent the image does not ship is created by the
/// runtime as root, so pointing this into `~/.cache` handed containers a
/// root-owned home cache. `/var/tmp` and not `/var/cache/devlaunch` on two
/// properties only a `1777` parent gives the leaf — nothing above it is invented,
/// and the path still works with nothing mounted on it, since devpod re-applies
/// `--workspace-env` on every `up` while a mount only lands at creation.
pub(crate) const PIXI_CACHE_TARGET: &str = "/var/tmp/devlaunch-pixi";

/// The leaf under devlaunch's cache directory that the launch locks live in.
///
/// Its own directory rather than the repo cache: this lock is keyed by workspace,
/// exists for workspaces that have no clone under the cache at all (paths, URLs),
/// and must not look like a repo to the cache's walkers.
pub(crate) const LAUNCH_LOCK_DIR: &str = "launch-locks";

/// Everything on the host this flow reads, gathered once by the caller.
///
/// Values rather than reads of the process environment, so every decision below is
/// a function of its inputs — and so a test states the host it means instead of
/// mutating an environment the rest of the binary shares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Host {
    pub(crate) gh: gh::HostEnv,
    /// `DEVLAUNCH_DOTFILES_ON_ATTACH`.
    pub(crate) dotfiles_on_attach: Option<String>,
    /// `DEVLAUNCH_ZELLIJ`.
    pub(crate) zellij: Option<String>,
    /// `DEVLAUNCH_NO_TTY`.
    pub(crate) no_tty: Option<String>,
    pub(crate) stdin_tty: bool,
    pub(crate) stdout_tty: bool,
    /// `~/.ssh/config`, where devpod publishes its host aliases. `None` on a
    /// machine with no home directory, which reads the same as a config with no
    /// alias in it: fall back to the devpod transport.
    pub(crate) ssh_config: Option<PathBuf>,
    /// Everything devlaunch stores: the launch locks, the shared pixi cache and
    /// the context-options cache all hang off this.
    pub(crate) cache_dir: PathBuf,
    /// devpod's own home, whose `config.yaml` mtime expires the options cache.
    pub(crate) devpod_home: Option<PathBuf>,
}

impl Host {
    /// The host this process is running on, with `cache_dir` supplied.
    ///
    /// The cache directory is a parameter because resolving it can fail (no home
    /// directory) and the binary has already had to resolve it for everything
    /// else; a second answer here could disagree with the first.
    pub fn from_process(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            gh: gh::HostEnv::from_process(),
            dotfiles_on_attach: crate::osext::env_str(DOTFILES_ON_ATTACH_VAR),
            zellij: crate::osext::env_str(ZELLIJ_WRAP_VAR),
            no_tty: crate::osext::env_str(ssh::DISABLE_VAR),
            stdin_tty: is_a_terminal(libc::STDIN_FILENO),
            stdout_tty: is_a_terminal(libc::STDOUT_FILENO),
            ssh_config: ssh::config_path(),
            cache_dir: cache_dir.into(),
            devpod_home: lifecycle::devpod_home(),
        }
    }

    /// The lock two `up`s of one workspace serialize on.
    pub(crate) fn launch_lock_path(&self, workspace_id: &str) -> PathBuf {
        self.cache_dir
            .join(LAUNCH_LOCK_DIR)
            .join(format!("{workspace_id}.lock"))
    }

    /// The host directory containers share their downloaded pixi packages through.
    ///
    /// Under devlaunch's own cache dir, so it follows `XDG_CACHE_HOME` like the
    /// rest of dl's storage and `dl --purge` takes it away with everything else —
    /// correct by construction, because this is a pure cache and the worst a
    /// deletion costs is the next container's downloads.
    pub(crate) fn pixi_cache_source(&self) -> PathBuf {
        self.cache_dir.join("pixi")
    }

    /// Where the answer to `devpod context options` is remembered between runs.
    pub(crate) fn context_options_cache(&self) -> PathBuf {
        self.cache_dir.join("context-options.json")
    }

    /// devpod's own config file, which holds every context and its options.
    pub(crate) fn devpod_config(&self) -> Option<PathBuf> {
        self.devpod_home
            .as_ref()
            .map(|home| home.join("config.yaml"))
    }
}

fn is_a_terminal(descriptor: std::ffi::c_int) -> bool {
    // SAFETY: `isatty` reads one descriptor and returns a flag; "not a terminal"
    // and "no such descriptor" are the same answer to this question.
    unsafe { libc::isatty(descriptor) == 1 }
}

/// Python's `_FALSEY`: the values that mean "no" rather than "set, therefore yes".
///
/// `dl.py`, `tty_session.py` and `gh_auth.py` each keep their own copy, and so do
/// [`ssh::tty_disabled`] and [`gh::forwarding_disabled`]; the shape is
/// deliberately the same at all of them, because two escape hatches answering to
/// one shared constant are one edit away from becoming one escape hatch.
const FALSEY: [&str; 4] = ["", "0", "false", "no"];

/// Whether a `DEVLAUNCH_*` switch of this family is on.
///
/// Off unless switched on, and the default falls out of the reading rather than
/// needing to be stated: unset reads as the empty string, which is a denial.
fn switched_on(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(value) => !FALSEY.contains(&crate::osext::strip(value).to_lowercase().as_str()),
    }
}

// ===========================================================================
// the notices
// ===========================================================================

/// Something the launch did that the `dl` binary may want to report.
///
/// One vocabulary for the whole flow, because a single launch produces notices
/// from every stage of it. Every arm is one `logging.*` call `dl.py` made,
/// carrying what that line interpolated and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchNotice {
    // --- the shared pixi cache (dl.py `_pixi_cache_up_args`)
    /// The shared pixi cache could not be created, so each container downloads
    /// its own packages. A warning and not a debug line: the launch survives, but
    /// the degradation is invisible and permanent until the cache home is
    /// writable again.
    PixiCacheNotCreated { source: PathBuf, reason: String },
    /// `mkdir` reported success and the path is not a directory after all — an
    /// `exist_ok` hit on a plain file, or something deleting it between the two
    /// calls. Narrow, and honestly so.
    PixiCacheNotADirectory { source: PathBuf },

    // --- the launch lock (dl.py `workspace_up`)
    /// Another launch of this workspace holds the lock, and this one is about to
    /// wait. Handed over *before* the blocking acquisition, which is the only
    /// moment at which "this run is now waiting" can be reported at all.
    WaitingForSiblingLaunch { workspace_id: String },
    /// The launch lock could not be taken, so this `up` is unserialized. Nothing
    /// worth failing a launch over: serialization guards a race that may not even
    /// be happening.
    LaunchLockUnavailable {
        workspace_id: String,
        reason: String,
    },
    /// The sibling this launch waited on had already brought the workspace up, so
    /// `devpod up` was not re-run.
    BroughtUpBySibling { workspace_id: String },

    // --- the token (gh_auth.py, memoized here)
    /// The host has no GitHub token to forward, and this is why. One per launch
    /// however often the token is asked for — see [`HostToken`].
    NoGitHubToken(GhEvent),
    /// The token could not be written to the private file `devpod up` reads it
    /// from, so this workspace opens without a GitHub login. Python distinguishes
    /// "could not create" from "could not write"; `tempfile` does both in one
    /// call, so there is one arm.
    TokenNotStaged { reason: String },

    // --- the session (dl.py `workspace_ssh`)
    /// dl is on a terminal but devpod published no ssh alias for this workspace,
    /// so this command gets no pty and interactive programs may exit immediately.
    NoTerminalAlias { workspace_id: String },
    /// The argv of the session about to start, program included.
    SshCommand { argv: Vec<String> },
    /// devpod itself failed the session; its own diagnostics are already on the
    /// user's stderr.
    DevpodSessionFailed { exit: Exit },

    // --- the launch's own arms (dl.py `_run_cli`)
    /// The workspace is already running, so this launch attaches straight to it.
    AlreadyRunningAttaching { workspace_id: String },
    /// `dl <ws> up` found it already running: nothing to build and nothing to
    /// wait for.
    AlreadyRunning { workspace_id: String },
    /// `dl <ws> dotfiles` has to bring the workspace up before it can refresh
    /// anything.
    StartingForDotfiles { workspace_id: String },
    /// A `--devcontainer` choice cannot be honoured: the workspace is already
    /// running, and switching config means recreating it.
    DevcontainerIgnoredRunning { workspace_id: String, spec: String },

    // --- passed through from the layers below
    /// Something one of the storage flows reported on the way through.
    Cache(CacheNotice),
    /// Something a lifecycle flow reported on the way through — in practice
    /// [`LifecycleNotice::AddressingRecordedWorkspace`] (devlaunch#88).
    Lifecycle(LifecycleNotice),
}

/// The launch's channel, as a storage flow's.
///
/// A sink handed down rather than a vector collected and appended: a bare clone of
/// a large repository takes minutes, and the line explaining the wait is worth
/// nothing after it.
fn as_cache<'a>(
    notices: &'a mut dyn Notices<LaunchNotice>,
) -> Wrapped<'a, CacheNotice, LaunchNotice> {
    Wrapped::new(notices, LaunchNotice::Cache)
}

/// The same, as a lifecycle flow's.
fn as_lifecycle<'a>(
    notices: &'a mut dyn Notices<LaunchNotice>,
) -> Wrapped<'a, LifecycleNotice, LaunchNotice> {
    Wrapped::new(notices, from_lifecycle)
}

/// One lifecycle notice in the launch's vocabulary.
///
/// A cache notice that arrived wrapped is unwrapped, so there is one
/// representation of it here rather than two routes to the same fact.
fn from_lifecycle(notice: LifecycleNotice) -> LaunchNotice {
    match notice {
        LifecycleNotice::Cache(cache) => LaunchNotice::Cache(cache),
        other => LaunchNotice::Lifecycle(other),
    }
}

// ===========================================================================
// devpod's context options, cached on disk
// ===========================================================================

/// The devpod context options that have a value set.
///
/// `clients::devpod` deliberately leaves the caching out — it is storage, and it
/// is per-context state — so this is where the TTL and the staleness check live.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContextOptions(BTreeMap<String, String>);

impl ContextOptions {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn from_map(options: BTreeMap<String, String>) -> Self {
        Self(options)
    }

    /// The repository `devpod up` installs dotfiles from, if the context sets one.
    pub(crate) fn dotfiles_url(&self) -> Option<&str> {
        self.0.get("DOTFILES_URL").map(String::as_str)
    }

    /// The install script inside that repository, if the context names one.
    pub(crate) fn dotfiles_script(&self) -> Option<&str> {
        self.0.get("DOTFILES_SCRIPT").map(String::as_str)
    }

    /// The `devpod up` flags these options contribute, in Python's order.
    fn up_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(url) = self.dotfiles_url() {
            args.push("--dotfiles".to_owned());
            args.push(url.to_owned());
        }
        if let Some(script) = self.dotfiles_script() {
            args.push("--dotfiles-script".to_owned());
            args.push(script.to_owned());
        }
        args
    }
}

/// The devpod context options, from the disk cache when it can be believed.
///
/// Only an answer devpod actually gave is cached, so a failed or unreadable read
/// costs nothing worse than the uncached behaviour: the empty set.
///
/// The TTL is not the only thing that expires it. These options are *per context*
/// and this is one cache file, so `devpod context use <other>` would otherwise
/// feed the previous context's dotfiles settings to `devpod up` for up to an
/// hour — a wrong answer nobody could connect to a cache they did not know
/// existed. Both switching context and setting an option rewrite devpod's config
/// file, so a cache older than that file is stale whatever its age. One stat, and
/// no round trip to find out.
pub(crate) fn context_options(
    runner: &dyn Runner,
    cache_path: &Path,
    devpod_config: Option<&Path>,
    now: SystemTime,
) -> ContextOptions {
    if let Some(cached) = cached_options(cache_path, devpod_config, now) {
        return cached;
    }
    let Ok(options) = devpod::context_options(runner) else {
        return ContextOptions::default();
    };
    write_options_cache(cache_path, &options);
    ContextOptions(options)
}

/// The cached options, if the cache exists, is young enough, and is newer than
/// devpod's own config file.
///
/// Read typed rather than sniffed (the divergence-row-11 family): Python accepts
/// any JSON object and keeps whatever the values are, so a hand-edited cache with
/// a number in it would reach a `devpod up` flag. A cache that is not
/// `{string: string}` reads here as no cache at all, and devpod is asked.
fn cached_options(
    cache_path: &Path,
    devpod_config: Option<&Path>,
    now: SystemTime,
) -> Option<ContextOptions> {
    let cached_at = std::fs::metadata(cache_path)
        .and_then(|meta| meta.modified())
        .ok()?;
    // No config file to disagree with; the TTL is the whole test.
    let config_changed = devpod_config
        .and_then(|config| std::fs::metadata(config).ok())
        .and_then(|meta| meta.modified().ok())
        .is_some_and(|changed_at| changed_at > cached_at);
    if config_changed {
        return None;
    }
    let age = now.duration_since(cached_at).ok()?;
    if age >= CONTEXT_OPTIONS_TTL {
        return None;
    }
    let text = std::fs::read_to_string(cache_path).ok()?;
    serde_json::from_str::<BTreeMap<String, String>>(&text)
        .ok()
        .map(ContextOptions)
}

/// Remember what devpod said, or say nothing about failing to.
///
/// A cache that cannot be written costs the next launch a round trip and nothing
/// else, which is not worth a word to the user.
fn write_options_cache(cache_path: &Path, options: &BTreeMap<String, String>) {
    let Ok(json) = serde_json::to_string(options) else {
        return;
    };
    if let Some(parent) = cache_path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    // Python writes `<name>.tmp` beside the cache and renames it, so a reader
    // never sees a half-written document.
    let temporary = cache_path.with_extension("tmp");
    if std::fs::write(&temporary, json).is_ok() {
        let _ = std::fs::rename(&temporary, cache_path);
    }
}

// ===========================================================================
// the shared pixi package cache
// ===========================================================================

/// Whether a container can be put on the host's shared pixi package cache.
///
/// Every container's dotfiles install runs `pixi global sync`, and on an empty
/// cache that is 62–113s and 1.2GB of network per container fetching what the
/// last one already fetched. One host directory bound into all of them makes the
/// second container's sync an 18–28s unpack from disk (devlaunch#232).
///
/// A sum rather than an `Option<PathBuf>` beside a warning, because each way of
/// failing names a different directory state and both have to be reportable: the
/// launch survives either way, and a silent degradation here is permanent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PixiCache {
    /// The directory is there; these flags put a container on it.
    Shared { source: PathBuf },
    /// The directory could not be created, so each container downloads its own.
    NotCreated { source: PathBuf, reason: String },
    /// `mkdir` succeeded and the path is not a directory anyway.
    NotADirectory { source: PathBuf },
}

impl PixiCache {
    /// Make sure the shared cache is there, and say what came of it.
    ///
    /// The source directory is created here rather than left to the container
    /// runtime. Runtimes disagree about a bind source that does not exist —
    /// refused outright by some, created as root by others — and a root-owned
    /// directory is one the container cannot write a package into, which is this
    /// feature failing silently and slowly.
    ///
    /// A directory that cannot be created costs the sharing and not the launch,
    /// the same call the launch lock makes: a full disk or a read-only cache home
    /// must not turn an `up` that would have worked into a failure.
    pub(crate) fn ensure(source: PathBuf) -> Self {
        if let Err(error) = std::fs::create_dir_all(&source) {
            return Self::NotCreated {
                reason: error.to_string(),
                source,
            };
        }
        if !source.is_dir() {
            // Belt to the mkdir's braces, and the window it covers is the
            // microseconds between the two calls rather than the wide one that
            // follows: the launch lock and the token both run after this, so a
            // cache home swept during *them* still reaches devpod as a mount
            // source that has gone. Emitting no mount is the right answer in both
            // cases; only the small window is reached here.
            return Self::NotADirectory { source };
        }
        Self::Shared { source }
    }

    /// The `devpod up` flags that put a container on the shared cache.
    ///
    /// Three of them, and the third is not a duplicate of the second: devpod
    /// gives the dotfiles install script an environment of its own, so a variable
    /// set only for the workspace never reaches the `pixi global sync` that is the
    /// whole consumer of this cache.
    pub(crate) fn up_args(&self) -> Vec<String> {
        match self {
            Self::Shared { source } => vec![
                "--mount".to_owned(),
                format!(
                    "type=bind,source={},target={PIXI_CACHE_TARGET}",
                    source.display()
                ),
                "--workspace-env".to_owned(),
                format!("PIXI_CACHE_DIR={PIXI_CACHE_TARGET}"),
                "--dotfiles-script-env".to_owned(),
                format!("PIXI_CACHE_DIR={PIXI_CACHE_TARGET}"),
            ],
            Self::NotCreated { .. } | Self::NotADirectory { .. } => Vec::new(),
        }
    }

    /// The notice this outcome is worth, if any.
    pub(crate) fn notice(&self) -> Option<LaunchNotice> {
        match self {
            Self::Shared { .. } => None,
            Self::NotCreated { source, reason } => Some(LaunchNotice::PixiCacheNotCreated {
                source: source.clone(),
                reason: reason.clone(),
            }),
            Self::NotADirectory { source } => Some(LaunchNotice::PixiCacheNotADirectory {
                source: source.clone(),
            }),
        }
    }
}

// ===========================================================================
// the host's GitHub token, asked for once per launch
// ===========================================================================

/// The host's GitHub token, resolved at most once for the whole launch.
///
/// Python memoizes `resolve_token` for the life of the process so that a single
/// run handing the token to both `devpod up` and `devpod ssh` unlocks a keyring
/// once and warns once. `clients::gh` deliberately left that out — it is
/// per-command state, the same kind as the memoized `devpod list` — so it lives
/// here, where the thing that makes both calls is.
///
/// The "warns once" half is the same fact: the event is produced by the *first*
/// ask and by no later one, so a launch that forwards the token twice reports one
/// notice rather than two.
#[derive(Debug, Default)]
pub struct HostToken {
    asked: OnceCell<TokenLookup>,
}

impl HostToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// The lookup, asking gh only the first time.
    ///
    /// The notice — if the lookup has one to make — is pushed only on the ask that
    /// performed it.
    pub(crate) fn lookup(
        &self,
        runner: &dyn Runner,
        host: &gh::HostEnv,
        notices: &mut dyn Notices<LaunchNotice>,
    ) -> &TokenLookup {
        if let Some(remembered) = self.asked.get() {
            return remembered;
        }
        // Not timed here: `clients::gh` spans the trip at the spawn, which is the
        // only place that can tell an ask that reached gh from one `resolve_token`
        // answered out of the environment.
        let lookup = gh::resolve_token(runner, host);
        if let TokenLookup::Unavailable(event) = &lookup {
            notices.say(LaunchNotice::NoGitHubToken(*event));
        }
        // A second caller racing in loses the value it computed, which is fine:
        // both are answers to the same question, and only one notice was pushed.
        let _ = self.asked.set(lookup);
        self.asked.get().expect("the lookup was just stored")
    }

    /// The token, or nothing to forward.
    pub(crate) fn token(
        &self,
        runner: &dyn Runner,
        host: &gh::HostEnv,
        notices: &mut dyn Notices<LaunchNotice>,
    ) -> Option<&Token> {
        self.lookup(runner, host, notices).token()
    }
}

// ===========================================================================
// the `devpod up` request
// ===========================================================================

/// Which IDE devpod should open, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ide<'a> {
    /// `--ide none`. dl attaches a terminal shell, so devpod's configured default
    /// IDE (usually vscode) must not open a window on every `dl <ws>`.
    NoIde,
    /// `--ide <name>` — `dl <ws> code` asks for vscode.
    Named(&'a str),
}

impl<'a> Ide<'a> {
    /// The word devpod is given. Passed always, never omitted: the default has to
    /// be stated or devpod applies its own.
    pub(crate) fn word(self) -> &'a str {
        match self {
            Self::NoIde => "none",
            Self::Named(name) => name,
        }
    }
}

/// How much of the container this `up` is asking devpod to rebuild.
///
/// A sum rather than Python's `recreate: bool, reset: bool`, which spells four
/// states for three meanings and leaves `--recreate --reset` askable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Rebuild {
    /// Reuse whatever is there — the everyday launch.
    #[default]
    Reuse,
    /// `--recreate`: rebuild the container from the devcontainer config.
    Recreate,
    /// `--reset`: clean slate.
    Reset,
}

impl Rebuild {
    fn flag(self) -> Option<&'static str> {
        match self {
            Self::Reuse => None,
            Self::Recreate => Some("--recreate"),
            Self::Reset => Some("--reset"),
        }
    }
}

/// How devpod is told which workspace this is.
///
/// Python carries `workspace_id` (devpod's `--id`, passed only when creating) and
/// `workspace_identity` (the id it is known by either way) as two independent
/// optionals, then reduces them with `workspace_identity or workspace_id`. Of the
/// four combinations three are meaningful and one — an `--id` with no identity —
/// is unreachable only by the callers' good behaviour. These are the three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Naming<'a> {
    /// A create: `--id <id>` goes to devpod, and that id is also the identity.
    Create { workspace_id: &'a str },
    /// devpod already knows the workspace: no `--id`, and the identity is the id
    /// it answers to.
    Known { workspace_id: &'a str },
    /// Nothing names it. No launch lock to key, no `DEVLAUNCH_WORKSPACE_ID` stamp
    /// and no tools to lend — the caller shapes that reach here are not the
    /// concurrent-launch ones.
    Anonymous,
}

impl<'a> Naming<'a> {
    /// devpod's `--id`, which is passed only when creating.
    fn create_as(self) -> Option<&'a str> {
        match self {
            Self::Create { workspace_id } => Some(workspace_id),
            Self::Known { .. } | Self::Anonymous => None,
        }
    }

    /// The id the workspace is known by, when anything knows it.
    pub fn identity(self) -> Option<&'a str> {
        match self {
            Self::Create { workspace_id } | Self::Known { workspace_id } => Some(workspace_id),
            Self::Anonymous => None,
        }
    }
}

/// One `devpod up`, as data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UpRequest<'a> {
    /// What devpod is given positionally: a clone directory on the cold path, a
    /// path or URL for a spec devpod clones itself, a workspace id for one it
    /// already has.
    pub(crate) source: &'a str,
    pub(crate) naming: Naming<'a>,
    pub(crate) ide: Ide<'a>,
    pub(crate) rebuild: Rebuild,
    /// A `devcontainer.json` path from [`spec::resolve_devcontainer_ref`].
    pub(crate) devcontainer: Option<&'a DevcontainerPath>,
}

impl<'a> UpRequest<'a> {
    /// The everyday launch: reuse the container, open no IDE, take the default
    /// devcontainer.
    pub(crate) fn new(source: &'a str, naming: Naming<'a>) -> Self {
        Self {
            source,
            naming,
            ide: Ide::NoIde,
            rebuild: Rebuild::Reuse,
            devcontainer: None,
        }
    }

    #[must_use]
    pub(crate) fn with_ide(mut self, ide: Ide<'a>) -> Self {
        self.ide = ide;
        self
    }

    #[must_use]
    pub(crate) fn with_rebuild(mut self, rebuild: Rebuild) -> Self {
        self.rebuild = rebuild;
        self
    }

    #[must_use]
    pub(crate) fn with_devcontainer(mut self, devcontainer: Option<&'a DevcontainerPath>) -> Self {
        self.devcontainer = devcontainer;
        self
    }

    /// Whether this call is here for something a sibling launch cannot have done
    /// for it.
    ///
    /// An IDE to open, a rebuild, a reset and a devcontainer variant are all
    /// requests a running workspace is not the answer to — and the variant
    /// especially, since skipping it would hand the user the default container
    /// while they asked for another one and say nothing about it.
    fn wants_more_than_a_running_workspace(&self) -> bool {
        !matches!(self.ide, Ide::NoIde)
            || !matches!(self.rebuild, Rebuild::Reuse)
            || self.devcontainer.is_some()
    }
}

/// The whole `devpod up` argv tail, in Python's exact order.
///
/// `token` goes last because Python appends it last, inside the launch lock, from
/// the context manager that owns the private file it names.
pub(crate) fn up_args(
    request: &UpRequest<'_>,
    options: &ContextOptions,
    pixi: &PixiCache,
    token: &[String],
) -> Vec<String> {
    let mut args = vec!["up".to_owned(), request.source.to_owned()];
    if let Some(id) = request.naming.create_as() {
        args.push("--id".to_owned());
        args.push(id.to_owned());
    }
    args.push("--ide".to_owned());
    args.push(request.ide.word().to_owned());
    if let Some(devcontainer) = request.devcontainer {
        args.push("--devcontainer-path".to_owned());
        args.push(devcontainer.as_str().to_owned());
    }
    if let Some(identity) = request.naming.identity() {
        args.push("--init-env".to_owned());
        args.push(format!("{WORKSPACE_ID_VAR}={identity}"));
    }
    if let Some(flag) = request.rebuild.flag() {
        args.push(flag.to_owned());
    }
    args.extend(options.up_args());
    args.extend(pixi.up_args());
    args.extend(token.iter().cloned());
    args
}

// ===========================================================================
// serializing two `up`s of one workspace
// ===========================================================================

/// How this `up` is serialized against a sibling launch of the same workspace.
///
/// wayfinder fires a background `dl <ws> up` the moment a launch is staged, and
/// the human's second enter runs the launch itself seconds later — so two
/// concurrent `devpod up`s of one workspace is an everyday event rather than an
/// edge case, and one devpod does not promise to survive.
///
/// Four arms, and only one of them licenses the state re-check: a launch that
/// *had* to wait knows the world may have changed while it did, where the three
/// that walked in know their earlier reads still stand.
#[derive(Debug)]
pub(crate) enum Serialization {
    /// The lock was free: no sibling was mid-launch.
    WalkedIn {
        /// The flock itself. Never read; dropping it is the release.
        #[allow(dead_code)]
        guard: locks::LockGuard,
    },
    /// A sibling held it, and this launch waited this long.
    Queued {
        /// The flock itself. Never read; dropping it is the release.
        #[allow(dead_code)]
        guard: locks::LockGuard,
        /// Held for the #251 §7 public-API freeze — how long `up` queued, which
        /// the binary will report. Nothing reads it yet.
        #[allow(dead_code)]
        waited: Duration,
    },
    /// Nothing named the workspace, so there was nothing to key a lock on.
    Unkeyed,
    /// The lock file could not be opened. An unwritable cache directory — a
    /// container writing as another uid is a documented occurrence in this very
    /// cache — or a full or read-only disk. Serialization guards against a race
    /// that may not even be happening, so taking the whole command down in front
    /// of a `devpod up` that would have worked is the worse answer.
    Unavailable {
        /// Held for the #251 §7 public-API freeze — why `up` ran unserialized.
        /// Nothing reads it yet.
        #[allow(dead_code)]
        reason: String,
    },
}

impl Serialization {
    /// Whether this launch had to queue behind a sibling — Python's `waited`.
    ///
    /// Held for the #251 §7 public-API freeze — part of `up`. Only tests ask today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn waited(&self) -> bool {
        matches!(self, Self::Queued { .. })
    }
}

/// Take the per-workspace launch lock, or say why this `up` runs unserialized.
///
/// The lock is held until the returned value is dropped, which is what keeps the
/// tools this launch lends from landing after a sibling's attach: Python holds it
/// across `provision_tools` for the same reason.
pub(crate) fn serialize_launch(
    host: &Host,
    naming: Naming<'_>,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Serialization {
    let Some(identity) = naming.identity() else {
        return Serialization::Unkeyed;
    };
    let lock_path = host.launch_lock_path(identity);
    let held = locks::hold_lock_watching(&lock_path, |_| {
        notices.say(LaunchNotice::WaitingForSiblingLaunch {
            workspace_id: identity.to_owned(),
        });
    });
    match held {
        Ok(guard) => match guard.contention() {
            Contention::WalkedIn => Serialization::WalkedIn { guard },
            Contention::Queued { waited } => {
                // The one measurement only the blocking call can make.
                timing::record("lock wait", waited);
                Serialization::Queued { guard, waited }
            }
        },
        Err(error) => {
            let reason = lock_reason(&error);
            notices.say(LaunchNotice::LaunchLockUnavailable {
                workspace_id: identity.to_owned(),
                reason: reason.clone(),
            });
            Serialization::Unavailable { reason }
        }
    }
}

fn lock_reason(error: &LockError) -> String {
    match error {
        LockError::CreateParent { failure, .. }
        | LockError::Open { failure, .. }
        | LockError::Acquire { failure, .. } => failure.message.clone(),
    }
}

// ===========================================================================
// lending the tools in
// ===========================================================================

/// Lending the host's tools into a running container — `tools.provision_tools`.
///
/// A trait rather than a call into [`crate::flows::provision`], for two reasons.
/// The provisioning flow is a milestone of its own (M8) and this one must not
/// wait on it; and it is the one collaborator a launch test wants to *observe*
/// without a container to install into, which is exactly what Python's tests
/// patch.
///
/// It answers *one* thing, and it is not whether the tools landed: whether the pass
/// worked is reported by the pass itself (devlaunch#167/#168) and a launch does not
/// branch on it — the tools are best-effort, and a workspace with no tools is still
/// a workspace. What it answers is [`DevpodMissing`], because that is not a failure
/// of the thing being attempted: Python gives `DevpodNotInstalled` a class its
/// `except OSError` cannot catch, so it travels out of the launch and `main()`
/// renders exit 127 for it. A trait returning `()` could not carry that, and the
/// binary had to keep a `Cell` beside the launch to reconstruct it — after the
/// session Python never reached.
pub trait Provision {
    fn provision_tools(&self, runner: &dyn Runner, workspace_id: &str)
    -> Result<(), DevpodMissing>;
}

/// A launch that lends nothing — `DEVLAUNCH_NO_TOOLS`, and every test that is not
/// about provisioning.
/// Held for the #251 §7 public-API freeze — the `up` a caller asks for with
/// nothing lent. Only tests choose it today.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Default)]
pub(crate) struct NoProvisioning;

impl Provision for NoProvisioning {
    fn provision_tools(
        &self,
        _runner: &dyn Runner,
        _workspace_id: &str,
    ) -> Result<(), DevpodMissing> {
        Ok(())
    }
}

// ===========================================================================
// `devpod up`
// ===========================================================================

/// What bringing the workspace up turned out to be.
///
/// Python answers with a `CompletedProcess` whose `returncode` is 0 for both of
/// the first two arms, which is what made the prewarm's worth invisible: the one
/// fact a caller needs — did *this* launch pay for the container — is exactly
/// what the return value dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpOutcome {
    /// `devpod up` ran and succeeded.
    Started,
    /// The sibling this launch waited on had already brought the workspace up, so
    /// re-running `devpod up` would re-walk a whole container lifecycle to arrive
    /// where the workspace already is. Nothing was asked of devpod but the state
    /// re-check the wait bought.
    SkippedSiblingWon,
    /// `devpod up` ran and refused. devpod's own diagnostics are already on the
    /// user's stderr — the call inherits this process's streams, as Python's does.
    Refused { exit: Exit },
}

impl UpOutcome {
    /// Whether the workspace can be attached to.
    ///
    /// Held for the #251 §7 public-API freeze — part of `up`. No caller yet.
    #[allow(dead_code)]
    pub(crate) fn succeeded(self) -> bool {
        !matches!(self, Self::Refused { .. })
    }
}

/// Start or create a workspace.
///
/// One `up` per workspace at a time, serialized over the per-workspace launch
/// lock (see [`serialize_launch`]). The loser waits; and a loser that *had* to
/// wait re-checks the state before doing anything, because the most likely reason
/// for the wait is that the winner just brought this very workspace up. That
/// re-check is one status round trip paid only on contention; the everyday
/// uncontended `up` pays nothing but the flock itself.
///
/// The skip does not apply when the call is there for a side effect the sibling
/// cannot have had — see [`UpRequest::wants_more_than_a_running_workspace`].
///
/// Charged to the `devpod-up` stage, as Python's `@timing.staged("devpod-up")`
/// charges it, and failed only when devpod never ran: a devpod that answered with
/// a refusal completed the stage.
pub(crate) fn workspace_up(
    context: &mut CommandContext<'_>,
    host: &Host,
    token: &HostToken,
    provision: &dyn Provision,
    request: &UpRequest<'_>,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<UpOutcome, NotRun> {
    timing::stage_result(timing::Stage::DevpodUp, || {
        up_under_stage(context, host, token, provision, request, notices)
    })
}

fn up_under_stage(
    context: &mut CommandContext<'_>,
    host: &Host,
    token: &HostToken,
    provision: &dyn Provision,
    request: &UpRequest<'_>,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<UpOutcome, NotRun> {
    let options = context_options(
        context.runner(),
        &host.context_options_cache(),
        host.devpod_config().as_deref(),
        SystemTime::now(),
    );
    let pixi = PixiCache::ensure(host.pixi_cache_source());
    notices.say_all(pixi.notice());

    // Taken here, so the lock covers the state re-check, the `up` and the tools:
    // a launch waiting on a prewarm must not attach before the tools land.
    let serialization = serialize_launch(host, request.naming, notices);

    if let Some(identity) = request.naming.identity()
        && serialization.waited()
        && !request.wants_more_than_a_running_workspace()
        && is_running(context.runner(), identity)
    {
        notices.say(LaunchNotice::BroughtUpBySibling {
            workspace_id: identity.to_owned(),
        });
        // Spared the container lifecycle, but only after waiting out the sibling
        // still walking it — the half-won case.
        timing::observe_attach(timing::AttachShape::Partial);
        context.forget_workspaces();
        // The tools are still this call's business. "Running" says the sibling's
        // `devpod up` finished, not that its provisioning did: it may have been
        // interrupted between the two, its `up` may have failed after the
        // container started, or it may have run with the tools switched off where
        // this one does not.
        provision
            .provision_tools(context.runner(), identity)
            .map_err(|DevpodMissing| NotRun::NotInstalled)?;
        return Ok(UpOutcome::SkippedSiblingWon);
    }

    // Give every workspace the host's gh login, whatever its devcontainer.json
    // does or does not set up for itself. The file is removed when `staged` drops,
    // which is the whole of Python's context manager — including on the path where
    // devpod fails.
    let staged = stage_token(context.runner(), host, token, notices);
    let token_args = staged
        .as_ref()
        .map(StagedToken::up_args)
        .unwrap_or_default();
    let args = up_args(request, &options, &pixi, &token_args);

    // This launch is the one paying for the `up`, so no prewarm saved it from
    // anything — whether or not one was fired.
    timing::observe_attach(timing::AttachShape::Miss);
    let exit = devpod::run(context.runner(), &Call::new(args))?;
    // `up` creates and starts workspaces, so any snapshot of `devpod list` taken
    // before it is now out of date.
    context.forget_workspaces();

    if !exit.is_success() {
        return Ok(UpOutcome::Refused { exit });
    }
    // Only after a successful `up`: there is no container to install into
    // otherwise. Inside the lock, for the reason it was taken above.
    if let Some(identity) = request.naming.identity() {
        // A devpod that went missing between the `up` that just worked and the pass
        // that follows it takes the launch with it, as Python's exception does: there
        // is no session to hand over without the binary that opens one.
        provision
            .provision_tools(context.runner(), identity)
            .map_err(|DevpodMissing| NotRun::NotInstalled)?;
    }
    drop(serialization);
    Ok(UpOutcome::Started)
}

/// Whether devpod reports this workspace as running.
///
/// Nothing is timed here: [`lifecycle::workspace_state`] opens the `devpod-up`
/// stage and [`crate::clients::devpod`] spans the round trip inside it, so the
/// measurement is already the shape it should be wherever this is called from.
fn is_running(runner: &dyn Runner, workspace_id: &str) -> bool {
    lifecycle::workspace_state(runner, workspace_id)
        .as_ref()
        .is_ok_and(ContainerState::is_running)
}

/// The token in a private file for `devpod up`, or nothing to forward.
fn stage_token(
    runner: &dyn Runner,
    host: &Host,
    token: &HostToken,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Option<StagedToken> {
    let found = token.token(runner, &host.gh, notices)?;
    match StagedToken::stage(found) {
        Ok(staged) => Some(staged),
        Err(error) => {
            // Forwarding a credential is a convenience, so a full or read-only
            // temp directory costs the workspace its gh login and not its launch.
            notices.say(LaunchNotice::TokenNotStaged {
                reason: error.to_string(),
            });
            None
        }
    }
}

// ===========================================================================
// the remote payload
// ===========================================================================

/// Whether a command runs beside a zellij session (#242).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ZellijWrap {
    /// Make sure a session exists first, so an agent has a terminal to open into.
    Beside,
    /// The default: no existing invocation changes meaning at all.
    #[default]
    Off,
}

impl ZellijWrap {
    /// What `DEVLAUNCH_ZELLIJ` asks for.
    pub(crate) fn from_host(host: &Host) -> Self {
        if switched_on(host.zellij.as_deref()) {
            Self::Beside
        } else {
            Self::Off
        }
    }
}

/// A command that cannot be made into a shell word.
///
/// Python has no such refusal — `shlex.quote` wraps a NUL and the remote shell
/// mangles it — and an argument that cannot mean what it says is better refused
/// than sent. The same call [`ssh::command_args`] makes about a workdir.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnquotableCommand {
    pub command: String,
}

/// `word` as one shell word, or `None` for a word no shell word can be.
///
/// [`shell::quote`] is the spelling — Python's `shlex.quote`, byte for byte — and
/// the refusal is this layer's: a `-- <cmd>` holding a NUL is refused before
/// anything spawns (docs/rust-rewrite-plan.md row 19), where Python quoted it and
/// let the remote shell mangle it.
fn posix_quote(word: &str) -> Option<String> {
    if shell::holds_nul(word) {
        return None;
    }
    Some(shell::quote(word).into_owned())
}

/// The one remote argument both transports deliver.
///
/// `bash -lc <quoted>`, and the login shell is the point: devpod runs `--command`
/// under a non-login, non-interactive `bash -c`, which sources neither
/// `~/.profile` nor `~/.bashrc` — so PATH entries the image adds there (notably
/// `$HOME/.pixi/bin`) are missing and the payload dies with "command not found".
/// An interactive attach gets a login shell, so both paths are wrapped here to
/// have the same PATH. dl launches arbitrary repos, so the parity has to come
/// from the invocation rather than from any particular devcontainer.json.
///
/// Built once and shared by both transports: two copies of this expression would
/// be two chances for the transports to drift, which is the whole failure the
/// single payload exists to have fixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemotePayload(String);

impl RemotePayload {
    /// Wrap `command` for the remote shell.
    pub(crate) fn wrap(command: &str, zellij: ZellijWrap) -> Result<Self, UnquotableCommand> {
        let inner = with_zellij_session(command, zellij);
        let quoted = posix_quote(&inner).ok_or_else(|| UnquotableCommand {
            command: command.to_owned(),
        })?;
        Ok(Self(format!("bash -lc {quoted}")))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// `command`, preceded by making sure the agent has a terminal to open into.
///
/// **Beside the session, not inside it**, and that is the decision worth reading
/// twice. What the capability needs is only that a session *exists*: `zellij -s
/// <name> action new-pane -- <cmd>` opens a working pane from a command that is in
/// no session at all, with no TTY anywhere. Running the command inside a pane
/// instead would hand its stdin, stdout and exit status to zellij, and all three
/// are contracts dl holds — `dl <ws> -- cmd > file` has to put the command's
/// output in the file, and the session goes to considerable trouble to return the
/// remote program's own status.
///
/// `attach -b` (create detached and return) rather than `attach -c`, because `-c`
/// wants to attach and the command this rides is not on a terminal.
///
/// `|| true`, and the tolerance is load-bearing rather than defensive: a second
/// `zellij attach -b <name>` exits **1** once the session is there, which is the
/// case every launch after the first takes. It also carries the "cost the feature,
/// not the launch" rule to this end — a container where zellij never installed
/// runs the command exactly as it would have, because a missing binary is a 127
/// this swallows like any other.
///
/// Separated by `;` and not `&&` for the same reason: what the payload exits with
/// must be the command's status, never the session setup's.
fn with_zellij_session(command: &str, zellij: ZellijWrap) -> String {
    match zellij {
        ZellijWrap::Off => command.to_owned(),
        ZellijWrap::Beside => {
            // The session name is a constant of safe characters, so quoting it
            // cannot fail and leaves it bare; `shlex.quote` on a literal is what
            // Python spells.
            let name = posix_quote(ZELLIJ_SESSION).unwrap_or_default();
            format!("zellij attach -b {name} >/dev/null 2>&1 || true; {command}")
        }
    }
}

// ===========================================================================
// which transport carries the session
// ===========================================================================

/// What dl has to hand a command in the way of a terminal.
///
/// Three arms rather than a bool, because the middle one is a *report*: dl is on a
/// terminal and cannot use it, which is worth saying and is not the same as being
/// in CI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Terminal {
    /// dl is on a terminal and devpod published this workspace's ssh alias.
    Usable,
    /// dl is on a terminal and devpod published no alias for this workspace, so
    /// there is no way to ask for a pty. The command still runs.
    NoAlias,
    /// dl is not on a terminal, or the user opted this machine out.
    Absent,
}

/// What dl can give a command on this host, for this workspace.
pub(crate) fn terminal_for(host: &Host, workspace_id: &str) -> Terminal {
    if !ssh::terminal_usable(host.no_tty.as_deref(), host.stdin_tty, host.stdout_tty) {
        return Terminal::Absent;
    }
    match host.ssh_config.as_deref() {
        Some(config) if ssh::devpod_host_configured(config, workspace_id) => Terminal::Usable,
        _ => Terminal::NoAlias,
    }
}

/// Which transport carries this session.
///
/// Deliberately the same decision ssh itself makes — use a terminal when there is
/// a terminal to use — so a redirected `dl <ws> -- ls > out.txt` keeps the devpod
/// transport and keeps its output free of escape sequences.
///
/// Each arm that runs a command carries the payload it runs, so a command route
/// with no command cannot be built: the correlation ("there is a command exactly
/// when this is not a bare attach") is the type, not a comment. Collapsed to a
/// payload-free tag, a `DevpodCommand` paired with no payload dispatched an
/// interactive attach that ran nothing and reported nothing, and a `Terminal`
/// paired with no payload panicked an `expect`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route<'p> {
    /// A bare attach. It stays on devpod whatever the terminal says: devpod
    /// requests a pty for exactly this case, so there is nothing to escape.
    DevpodAttach,
    /// A command under a pty, through the ssh alias `devpod up` published.
    Terminal(&'p RemotePayload),
    /// A command through `devpod ssh --command`, which never asks for a pty.
    DevpodCommand(&'p RemotePayload),
}

/// Route this session, reporting a terminal dl had and could not use.
pub(crate) fn route<'p>(
    command: Option<&'p RemotePayload>,
    terminal: Terminal,
    workspace_id: &str,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Route<'p> {
    let Some(command) = command else {
        return Route::DevpodAttach;
    };
    match terminal {
        Terminal::Usable => Route::Terminal(command),
        Terminal::NoAlias => {
            notices.say(LaunchNotice::NoTerminalAlias {
                workspace_id: workspace_id.to_owned(),
            });
            Route::DevpodCommand(command)
        }
        Terminal::Absent => Route::DevpodCommand(command),
    }
}

// ===========================================================================
// the session
// ===========================================================================

/// How the session ended, and whose ending it is.
///
/// Three arms because there are three different processes a number can come from,
/// and confusing them is the defect this shape exists to have fixed: `dl` used to
/// report devpod's exit code (always 1) for a session that had ended perfectly
/// normally with, say, 130.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    /// OpenSSH ran the remote program and exited with its status. Nothing to
    /// recover: OpenSSH passes the status through, which is the thing devpod loses
    /// by wrapping its `*ssh.ExitError` three times before type-asserting on it,
    /// and the reason this transport needs none of that machinery.
    Terminal { exit: Exit },
    /// `devpod ssh` ran the remote program, which exited with `status` — recovered
    /// from devpod's own report when devpod buried it.
    RemoteExit { status: i32 },
    /// devpod never ran the remote program, or lost it partway.
    DevpodFailed { exit: Exit },
}

impl Session {
    /// The number Python returns for this session.
    ///
    /// A signal is Python's negative `returncode`, kept here so the binary renders
    /// the same exit code rather than inventing 128+n.
    pub fn exit_status(self) -> i32 {
        match self {
            Self::RemoteExit { status } => status,
            Self::Terminal { exit } | Self::DevpodFailed { exit } => match exit {
                Exit::Code(code) => code,
                Exit::Signal(signal) => -signal,
            },
        }
    }
}

/// Why no session could be had at all.
///
/// Every arm is a thing Python raises rather than returns, and the split is the
/// same one: a missing binary travels as a type nothing in between catches, and
/// `main()` renders exit 127 for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionRefused {
    /// devpod never ran — Python's `DevpodNotInstalled` and friends.
    Devpod(NotRun),
    /// OpenSSH never ran. Its own arm rather than devpod's, for the reason Python
    /// gives `SshNotInstalled` its own class: telling someone to install devpod
    /// when devpod is present and working sends them the wrong way.
    Ssh(ssh::NotRun),
    /// The OpenSSH invocation could not be composed safely.
    UnsafeRequest(ssh::UnsafeRequest),
    /// The command could not be made into a shell word.
    Unquotable(UnquotableCommand),
}

/// What a session is opened through.
///
/// The three travel together at every call on this path — a session needs a
/// process to spawn, a host to read the terminal and the switches off, and the
/// one token lookup the launch shares — so they are one parameter rather than
/// three repeated at each of five signatures.
#[derive(Clone, Copy)]
pub struct SessionContext<'a> {
    pub(crate) runner: &'a dyn Runner,
    pub(crate) host: &'a Host,
    pub(crate) token: &'a HostToken,
}

impl<'a> SessionContext<'a> {
    pub fn new(runner: &'a dyn Runner, host: &'a Host, token: &'a HostToken) -> Self {
        Self {
            runner,
            host,
            token,
        }
    }

    /// The host's token, asked for at most once across the whole launch.
    fn forwarded_token(&self, notices: &mut dyn Notices<LaunchNotice>) -> Option<&'a Token> {
        self.token.token(self.runner, &self.host.gh, notices)
    }
}

/// SSH into a workspace, optionally running a command.
///
/// A command runs through whichever of the two transports can give it what it
/// needs. `devpod ssh --command` never requests a pty, which is fine for `make
/// test` and fatal for anything interactive — `claude` reads the pipe as a
/// non-interactive invocation and exits instead of starting a session. So when dl
/// is itself on a terminal, the command goes to OpenSSH through the host alias
/// devpod published, with `-t`.
///
/// A bare attach is left on devpod: it already gets a pty, being the one case
/// devpod requests one for.
///
/// `workdir` is left unset to land in the `workspaceFolder` from
/// devcontainer.json — devpod falls back to `$HOME` when given a path that does
/// not exist in the container, so never guess one from the workspace id.
pub(crate) fn workspace_ssh(
    session: &SessionContext<'_>,
    workspace_id: &str,
    command: Option<&str>,
    workdir: Option<&str>,
    forward: &mut dyn FnMut(&str),
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<Session, SessionRefused> {
    let payload = match command {
        None => None,
        Some(command) => Some(
            RemotePayload::wrap(command, ZellijWrap::from_host(session.host))
                .map_err(SessionRefused::Unquotable)?,
        ),
    };
    let terminal = terminal_for(session.host, workspace_id);
    match route(payload.as_ref(), terminal, workspace_id, notices) {
        Route::Terminal(payload) => {
            ssh_with_terminal(session, workspace_id, payload, workdir, notices)
        }
        Route::DevpodAttach => {
            devpod_session(session, workspace_id, None, workdir, forward, notices)
        }
        Route::DevpodCommand(payload) => devpod_session(
            session,
            workspace_id,
            Some(payload),
            workdir,
            forward,
            notices,
        ),
    }
}

/// The devpod transport: `devpod ssh <id> [--workdir <dir>] [--command <payload>]`.
///
/// Attaching to a running workspace skips `devpod up` and its workspace env
/// entirely, so the gh login has to be offered here too. Only the variable name
/// lands in argv; the token travels in devpod's environment.
fn devpod_session(
    session: &SessionContext<'_>,
    workspace_id: &str,
    payload: Option<&RemotePayload>,
    workdir: Option<&str>,
    forward: &mut dyn FnMut(&str),
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<Session, SessionRefused> {
    let mut args = vec!["ssh".to_owned(), workspace_id.to_owned()];
    if let Some(workdir) = workdir.filter(|dir| !dir.is_empty()) {
        args.push("--workdir".to_owned());
        args.push(workdir.to_owned());
    }
    if let Some(payload) = payload {
        args.push("--command".to_owned());
        args.push(payload.as_str().to_owned());
    }
    let forwarding = gh::ssh_forwarding(session.forwarded_token(notices));
    args.extend(forwarding.args.iter().cloned());

    let call = Call::new(args).with_env(forwarding.env);
    notices.say(LaunchNotice::SshCommand { argv: call.argv() });

    // Only stderr is read, which under a pty carries devpod's own warnings and
    // nothing else, so devpod's report of how the session ended can be
    // interpreted rather than dumped on the user. Whatever survives the filter
    // goes to `forward` **as the session runs** rather than into a notice: a
    // session lives for hours, and devpod's warning about it is worth nothing an
    // hour late. Python writes these to `sys.stderr` from inside the filter; core
    // writes to nobody's stream, so the sink is the caller's.
    let outcome =
        devpod::session(session.runner, &call, forward).map_err(SessionRefused::Devpod)?;
    Ok(match outcome {
        devpod::SshOutcome::RemoteExit { status } => Session::RemoteExit { status },
        devpod::SshOutcome::DevpodFailed { exit } => {
            notices.say(LaunchNotice::DevpodSessionFailed { exit });
            Session::DevpodFailed { exit }
        }
    })
}

/// The OpenSSH transport: an already-wrapped payload under a pty.
fn ssh_with_terminal(
    session: &SessionContext<'_>,
    workspace_id: &str,
    payload: &RemotePayload,
    workdir: Option<&str>,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<Session, SessionRefused> {
    let forwarding = gh::openssh_forwarding(session.forwarded_token(notices));
    let args = ssh::command_args(workspace_id, payload.as_str(), &forwarding.args, workdir)
        .map_err(SessionRefused::UnsafeRequest)?;
    notices.say(LaunchNotice::SshCommand { argv: args.clone() });
    let exit = {
        let _span = timing::span("ssh");
        ssh::run(session.runner, &args, forwarding.env).map_err(SessionRefused::Ssh)?
    };
    Ok(Session::Terminal { exit })
}

// ===========================================================================
// the dotfiles refresh
// ===========================================================================

/// Whether the user asked for dotfiles to be refreshed when they attach.
///
/// Off unless switched on, and that is the requirement rather than a default
/// (devlaunch#183). devpod applies dotfiles when it *provisions* a workspace, so a
/// long-lived one keeps whatever it was born with until something refreshes it — a
/// real gap, but one whose fix costs a `devpod ssh` round trip measured at ~1.73s
/// of which ~99% is connection setup, plus a git pull, in front of every shell.
/// Charging that to everyone to close a gap most of them do not have is how the
/// first attempt at this failed; the people who want it say so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DotfilesRefresh {
    Requested,
    #[default]
    Off,
}

impl DotfilesRefresh {
    pub(crate) fn from_host(host: &Host) -> Self {
        if switched_on(host.dotfiles_on_attach.as_deref()) {
            Self::Requested
        } else {
            Self::Off
        }
    }
}

/// The shell command that refreshes dotfiles inside a running workspace.
///
/// `chezmoi update` plus `pixi global sync`, falling back to a full `install.sh`
/// run when chezmoi is not there (a workspace that predates dotfiles setup).
///
/// `bound` bounds the whole payload and is what makes the automatic refresh safe
/// to put in front of a shell: every step of this is a network call, and a
/// non-zero exit being tolerated only helps once the command has decided to exit
/// at all. An unreachable remote or a credential prompt is exactly the case where
/// it does not.
///
/// The bound is spent *inside* the container rather than on a host-side subprocess
/// deadline: `timeout` puts the managed command in its own process group and
/// signals the group, so the git or pixi process actually doing the waiting dies
/// with the shell that started it and stops holding the session open — which a
/// host-side kill of the `devpod ssh` client would not achieve.
///
/// Left unbounded for `dl <ws> dotfiles` — typed, in the foreground,
/// interruptible, and sometimes a legitimately slow first `pixi global sync`. A
/// deadline is worth having on the refresh nobody asked for, and is a way to lose
/// work on the one somebody did.
pub(crate) fn dotfiles_command(dotfiles_url: Option<&str>, bound: Option<Duration>) -> String {
    let fallback = match dotfiles_url {
        Some(url) => {
            // A URL holding a NUL is no URL; an empty word makes the clone fail,
            // which is the safe reading of a context option nothing could use.
            let quoted = posix_quote(url).unwrap_or_else(|| "''".to_owned());
            format!(
                "echo \"chezmoi not found, running full install...\" && \
                 DOTFILES_DIR=$(mktemp -d) && \
                 git clone {quoted} \"$DOTFILES_DIR\" && \
                 cd \"$DOTFILES_DIR\" && bash install.sh && \
                 rm -rf \"$DOTFILES_DIR\" && \
                 echo \"Dotfiles installed successfully\""
            )
        }
        None => "echo \"chezmoi not found and no DOTFILES_URL configured\" && exit 1".to_owned(),
    };
    let update = format!(
        "if command -v chezmoi >/dev/null 2>&1; then \
         echo \"Updating dotfiles...\" && \
         chezmoi update --force && \
         echo \"Syncing pixi global packages...\" && \
         pixi global sync && \
         echo \"Dotfiles updated successfully\"; \
         else {fallback}; fi"
    );
    match bound {
        None => update,
        Some(bound) => {
            let quoted = posix_quote(&update).unwrap_or_else(|| "''".to_owned());
            format!("timeout {} bash -c {quoted}", bound.as_secs())
        }
    }
}

/// Refresh dotfiles inside a running workspace.
pub(crate) fn dotfiles_update(
    session: &SessionContext<'_>,
    workspace_id: &str,
    bound: Option<Duration>,
    forward: &mut dyn FnMut(&str),
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<Session, SessionRefused> {
    let options = context_options(
        session.runner,
        &session.host.context_options_cache(),
        session.host.devpod_config().as_deref(),
        SystemTime::now(),
    );
    let command = dotfiles_command(options.dotfiles_url(), bound);
    workspace_ssh(
        session,
        workspace_id,
        Some(&command),
        None,
        forward,
        notices,
    )
}

// ===========================================================================
// the attach
// ===========================================================================

/// Hand the workspace to the user: ssh in, and nothing else.
///
/// One trip, and it is the session. This used to pay a `devpod ssh` of its own in
/// front of the session to name the container — measured at ~1.73s, of which ~99%
/// is connection and process setup — and skip it for a one-shot `dl <ws> -- cmd`
/// that renders no prompt for a hostname to appear in. Naming the container is a
/// stage of the setup pass now, and every entry into Running goes through that
/// pass, so a workspace reached here is already named (devlaunch#167/#168).
///
/// One thing is conditional on top of the session, and only ever off unless asked
/// for: the dotfiles refresh (devlaunch#183). It is skipped for a one-shot
/// `dl <ws> -- cmd` on the same reasoning the hostname round-trip was skipped for
/// years before it moved — that command renders no prompt and sources no
/// interactive shell, so a refresh in front of it buys it nothing and costs it a
/// ~1.7s round trip. That path is the shape wayfinder hands dl for every agent
/// launch.
///
/// In front of the session rather than behind it, because the shell being handed
/// over is the whole point: dotfiles that landed after it started are dotfiles it
/// has already finished sourcing.
pub(crate) fn attach_workspace(
    session: &SessionContext<'_>,
    workspace_id: &str,
    command: Option<&str>,
    forward: &mut dyn FnMut(&str),
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<Session, SessionRefused> {
    timing::stage_result(timing::Stage::Attach, || {
        if command.is_none()
            && matches!(
                DotfilesRefresh::from_host(session.host),
                DotfilesRefresh::Requested
            )
        {
            // Best-effort: a refresh that could not even start must not cost the
            // user the shell it was standing in front of.
            let _ = dotfiles_update(
                session,
                workspace_id,
                Some(DOTFILES_ATTACH_TIMEOUT),
                forward,
                notices,
            );
        }
        workspace_ssh(session, workspace_id, command, None, forward, notices)
    })
}

// ===========================================================================
// stage one: what the spec asks for
// ===========================================================================

/// What a raw spec turns out to be, as far as anything can tell without asking
/// devpod.
///
/// The three arms are `_run_cli`'s three resolution branches, and the branches
/// cannot collide: a workspace id never contains `/`, `:` or a path prefix, so
/// whichever arm matches is the only arm that could.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// `owner/repo[@branch]`, the shape `dl` exists to make short. `branch` is
    /// `None` for a bare `owner/repo`, whose default branch has to be named
    /// before anything can derive a workspace id.
    Triple {
        owner: String,
        repo: String,
        branch: Option<String>,
        remote_url: String,
    },
    /// A path or a git source devpod clones itself. devpod is given `source` and
    /// `--id workspace_id`, and **nothing is asked of devpod about it** — that is
    /// Python's behaviour rather than an oversight: everything creatable goes
    /// through `up`, which is idempotent for a workspace devpod already has.
    Creatable {
        source: String,
        workspace_id: String,
    },
    /// A bare name, which can only be a workspace devpod already has: everything
    /// creatable is a path or a git spec and matched above.
    Existing { name: String },
}

/// Classify a spec, refusing an owner or repo that is not a safe name.
///
/// The owner and repo are checked here, before anything builds a path out of
/// them: `ensure_repo` joins `repos_dir/<owner>/<repo>` and would otherwise act on
/// a traversal first and reject it after — `x/..` resolves to `repos_dir` itself
/// and `../x` leaves it entirely. The branch's own check waits for the
/// [`WorkspaceId`], which is the parse boundary for the triple.
pub fn plan(raw_spec: &str) -> Result<Plan, UnsafeName> {
    let parsed = spec::parse(raw_spec);
    if let WorkspaceSpec::OwnerRepo {
        owner,
        repo,
        branch,
    } = parsed
    {
        validate_ref_name(owner, NamePart::Owner)?;
        validate_ref_name(repo, NamePart::Repo)?;
        return Ok(Plan::Triple {
            remote_url: format!("git@github.com:{owner}/{repo}.git"),
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            branch: branch.map(str::to_owned),
        });
    }
    if parsed.is_path() || parsed.is_git_source() {
        let workspace_id = match spec::identity(raw_spec)? {
            SpecIdentity::Workspace(id) => id,
            SpecIdentity::RepoLabel(label) => label,
            SpecIdentity::PathLeaf(path) => path_leaf(path),
            SpecIdentity::ExistingName(name) => name.to_owned(),
        };
        return Ok(Plan::Creatable {
            source: parsed.expanded().into_owned(),
            workspace_id,
        });
    }
    Ok(Plan::Existing {
        name: raw_spec.to_owned(),
    })
}

/// The name devpod gives a path spec: the resolved directory's final component.
///
/// Python spells this `Path(spec).expanduser().resolve().name`, and `resolve()`
/// there is non-strict — a directory that is not there yet still yields a name.
/// [`std::fs::canonicalize`] is strict and follows symlinks, so this normalises
/// lexically after expanding `~` and absolutizing: for every path that exists and
/// holds no symlinked component the two agree, and for one that does not exist
/// Python's answer is the lexical one too.
fn path_leaf(path: &str) -> String {
    let expanded = match path.strip_prefix('~') {
        Some(rest) => match crate::osext::home_dir() {
            Some(home) => home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(path),
        },
        None => PathBuf::from(path),
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(expanded)
    };
    let mut components: Vec<std::ffi::OsString> = Vec::new();
    for part in absolute.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::Prefix(_) | std::path::Component::RootDir => components.clear(),
            std::path::Component::Normal(name) => components.push(name.to_os_string()),
        }
    }
    components
        .last()
        // The filesystem root has no name, which is what Python's `.name` answers
        // for it: the empty string.
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ===========================================================================
// the cold path's machinery, opened only if it is needed
// ===========================================================================

/// The clone manager and the metadata store, for the arms that need them.
pub struct Cold<'a, 'r> {
    pub clones: &'a WorkspaceCloneManager<'r>,
    pub storage: &'a mut MetadataStorage,
}

/// Why the cold path's machinery could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColdRefused {
    pub reason: String,
}

/// A way to build the cold path's machinery, called only when it is needed.
///
/// **This is devlaunch#145 in the type system.** Building the clone manager reads
/// `config.toml`, loads `metadata.json` under the metadata lock and runs the cache
/// migration — three things that ticket deliberately took off the warm attach
/// path, which is the path a user waits on. A launcher holding a
/// `&mut MetadataStorage` would have paid for all three before it could ask
/// devpod anything, so it holds a *way to get one* instead, and "a warm launch
/// does no metadata I/O" is a fact about which calls happen rather than a
/// property to be re-tested.
///
/// It is the same move [`lifecycle::resolve_known_workspace`] makes with its
/// `recorded_id` closure, one level up.
pub trait ColdMachinery<'r> {
    fn open(&mut self) -> Result<Cold<'_, 'r>, ColdRefused>;
}

/// A launcher that must never reach the cold path, for callers that have already
/// established it is warm.
/// Held for the #251 §7 public-API freeze — the `up` of a caller that has
/// established the workspace is warm. Nothing constructs one yet.
#[allow(dead_code)]
#[derive(Debug, Default)]
pub(crate) struct NoColdPath;

impl<'r> ColdMachinery<'r> for NoColdPath {
    fn open(&mut self) -> Result<Cold<'_, 'r>, ColdRefused> {
        Err(ColdRefused {
            reason: "the cold path is not available to this caller".to_owned(),
        })
    }
}

// ===========================================================================
// stage two: which workspace, and is it warm
// ===========================================================================

/// Where devpod is pointed, and whether this launch has to create anything.
///
/// The three combinations `_run_cli`'s four correlated locals are ever in. There
/// is no way to build a [`Placement::Known`] with no state, and no way to read a
/// state off a [`Placement::Creating`] — which is what makes "is this launch warm"
/// a question about one value instead of a conjunction over two.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement {
    /// devpod knows this workspace and reported this state. It is addressed by
    /// id, `up` passes no `--id`, and a running one can be attached to straight
    /// away.
    Known {
        workspace_id: String,
        state: ContainerState,
    },
    /// devpod lists this workspace and could not describe it — a provider that is
    /// broken, reconfigured or gone. Addressed by id, `up` passes no `--id`, and
    /// it is never warm.
    ///
    /// Its own arm rather than a [`Placement::Known`] carrying a made-up state:
    /// devpod said nothing about this workspace's container, and inventing
    /// `NotFound` for it would be this module answering a question only devpod can
    /// answer. It is exactly the workspace somebody is about to run
    /// `dl <ws> rm` on, so the arm has to survive to the verb.
    Listed { workspace_id: String },
    /// This launch may have to create it: `source` is what devpod is given and
    /// `workspace_id` is the `--id`. Nothing has been asked of devpod about it.
    Creating {
        workspace_id: String,
        source: String,
    },
}

impl Placement {
    /// The id every later step addresses.
    pub fn workspace_id(&self) -> &str {
        match self {
            Self::Known { workspace_id, .. }
            | Self::Listed { workspace_id }
            | Self::Creating { workspace_id, .. } => workspace_id,
        }
    }

    /// What devpod is given positionally.
    pub fn source(&self) -> &str {
        match self {
            Self::Known { workspace_id, .. } | Self::Listed { workspace_id } => workspace_id,
            Self::Creating { source, .. } => source,
        }
    }

    /// How devpod is told which workspace this is.
    pub fn naming(&self) -> Naming<'_> {
        match self {
            Self::Known { workspace_id, .. } | Self::Listed { workspace_id } => {
                Naming::Known { workspace_id }
            }
            Self::Creating { workspace_id, .. } => Naming::Create { workspace_id },
        }
    }

    /// Whether a launch may attach straight away — Python's
    /// `custom_id is None and known_state == "Running"`.
    pub fn is_running(&self) -> bool {
        match self {
            Self::Known { state, .. } => state.is_running(),
            // Nothing said it was running, which is not the same as saying it is
            // not — and "attach without an `up`" needs the positive answer.
            Self::Listed { .. } | Self::Creating { .. } => false,
        }
    }
}

/// What asking devpod about a triple settled.
///
/// The warm arm is finished: it carries the [`Placement`] and has read no
/// `metadata.json`. The cold arm carries what the host still has to prepare, and
/// is the only arm that needs the [`ColdMachinery`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Warm { placement: Placement },
    Cold { workspace: WorkspaceId },
}

/// Which devpod workspace a triple is, asked of devpod first.
///
/// Delegated whole to [`lifecycle::resolve_known_workspace`] — the derived id is a
/// hint, and the record settles it when devpod does not recognise the hint
/// (devlaunch#88). The record is read through a closure that opens the cold
/// machinery, so a devpod that recognises the derived id costs no metadata I/O at
/// all.
///
/// Charged to the `devpod-up` stage with a `devpod status` span, for the reason
/// [`is_running`] gives: the span belongs inside the stage the lifecycle helper
/// opens, and a guard around the call would drop after that stage had closed.
/// A devpod that could not be run at all travels out as [`NotRun`] rather than
/// becoming a cold launch: see [`lifecycle::resolve_known_workspace`] for what a
/// cold launch on a devpod-less host costs.
pub fn resolve_triple(
    context: &mut CommandContext<'_>,
    cold: &mut dyn ColdMachinery<'_>,
    workspace: &WorkspaceId,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<Resolution, NotRun> {
    let derived = workspace.value();
    let triple = (workspace.owner(), workspace.repo(), workspace.git_ref());
    let known = lifecycle::resolve_known_workspace(
        context.runner(),
        triple,
        &derived,
        || recorded_id(cold, triple),
        &mut as_lifecycle(notices),
    )?;
    Ok(match known {
        KnownWorkspace::Known {
            workspace_id,
            state,
        } => Resolution::Warm {
            placement: Placement::Known {
                workspace_id,
                state,
            },
        },
        KnownWorkspace::Unknown { .. } => Resolution::Cold {
            workspace: workspace.clone(),
        },
    })
}

/// The devpod workspace id `metadata.json` holds for a triple, if any.
///
/// A store that cannot be opened answers `None`, which is Python's reading: a
/// lookup that failed must not be able to stop a command that would otherwise
/// have worked.
fn recorded_id(cold: &mut dyn ColdMachinery<'_>, triple: (&str, &str, &str)) -> Option<String> {
    let (owner, repo, branch) = triple;
    let opened = cold.open().ok()?;
    lifecycle::recorded_devpod_workspace_id(opened.storage, owner, repo, branch)
}

/// The default branch a bare `owner/repo` means.
///
/// One extra repo-lock cycle, and it is deliberate rather than an oversight:
/// folding it into the cold scope means holding the repo lock across the
/// fast-attach `devpod status` — a subprocess every sibling launch of this
/// repository would then queue behind — to save one uncontended flock
/// (devlaunch#200). Only the branch *name* crosses the gap, and the collapsed
/// scope's first act re-verifies clone-if-missing under its own lock.
pub(crate) fn name_default_branch(
    cold: &mut dyn ColdMachinery<'_>,
    owner: &str,
    repo: &str,
    remote_url: &str,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<String, BranchNotNamed> {
    let opened = cold.open().map_err(BranchNotNamed::Cold)?;
    let said = &mut as_cache(notices);
    let repos = opened.clones.repo_manager();
    match repos.ensure_repo(opened.storage, owner, repo, remote_url, said) {
        Ok(_) => Ok(repos.get_default_branch(opened.storage, owner, repo, said)),
        Err(error) => Err(BranchNotNamed::Repository(error)),
    }
}

/// Why a bare `owner/repo` could not be turned into a triple.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchNotNamed {
    Cold(ColdRefused),
    /// The bare-clone cache could not be brought up for this repository. Carries the
    /// cache's own refusal: this is the line a mistyped repository name ends at, and
    /// what a reader needs from it is git's own words.
    Repository(EnsureRepoError),
}

// ===========================================================================
// stage three: the host's own work
// ===========================================================================

/// Why a cold launch's host-side preparation could not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotPrepared {
    Cold(ColdRefused),
    /// The clone, the fetch, the branch or the workspace clone failed. Named by
    /// the workspace being prepared, because one report covers what three used to:
    /// all of them fail out of the one call, and "which repo, which branch" is
    /// what the old per-step messages carried and the user still needs.
    ///
    /// Carries the preparation's own refusal, whose arms name the step: which of the
    /// five it was, and what git or the OS said about it.
    Preparation(PrepareColdError),
}

/// Everything a cold launch needs on the host, under one repo lock.
///
/// Delegated whole to [`WorkspaceCloneManager::prepare_cold`]: the lock-ordering
/// doctrine stays where it is enforced, and nothing here knows which locks that
/// takes — which is exactly why the scope is not opened at this layer
/// (devlaunch#200).
pub(crate) fn prepare(
    cold: &mut dyn ColdMachinery<'_>,
    workspace: &WorkspaceId,
    remote_url: &str,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<Placement, NotPrepared> {
    let opened = cold.open().map_err(NotPrepared::Cold)?;
    let prepared = opened.clones.prepare_cold(
        opened.storage,
        workspace.owner(),
        workspace.repo(),
        workspace.git_ref(),
        remote_url,
        &mut as_cache(notices),
    );
    match prepared {
        Ok(prepared) => Ok(Placement::Creating {
            workspace_id: workspace.value(),
            source: prepared.path.to_string_lossy().into_owned(),
        }),
        Err(error) => Err(NotPrepared::Preparation(error)),
    }
}

// ===========================================================================
// the whole launch
// ===========================================================================

/// What `dl <spec> <verb>` asks for.
///
/// One arm per shape `_run_cli` dispatches to on this path. `stop`, `rm` and the
/// cache-wide commands are [`lifecycle`]'s and have no arm here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchVerb {
    /// `dl <spec>` and `dl <spec> -- <cmd>`: bring it up if it is not up, then
    /// attach. The default, and the shape wayfinder hands dl for every agent
    /// launch.
    Attach { command: Option<String> },
    /// `dl <spec> up`: the warm half of a launch, for callers that want the
    /// container ready before a user arrives. Idempotent and quiet when already up.
    Up,
    /// `dl <spec> code`: bring it up with the IDE open, and do not attach.
    Code,
    /// `dl <spec> recreate`: rebuild the container, then attach.
    Recreate,
    /// `dl <spec> reset`: clean slate, then attach.
    Reset,
    /// `dl <spec> restart`: stop and start without rebuilding, then attach.
    Restart,
    /// `dl <spec> dotfiles`: make sure it is running, then refresh the dotfiles.
    Dotfiles,
}

impl LaunchVerb {
    /// What this verb asks devpod to rebuild.
    fn rebuild(&self) -> Rebuild {
        match self {
            Self::Recreate => Rebuild::Recreate,
            Self::Reset => Rebuild::Reset,
            Self::Attach { .. } | Self::Up | Self::Code | Self::Restart | Self::Dotfiles => {
                Rebuild::Reuse
            }
        }
    }

    /// Which IDE this verb opens.
    fn ide(&self) -> Ide<'static> {
        match self {
            Self::Code => Ide::Named("vscode"),
            Self::Attach { .. }
            | Self::Up
            | Self::Recreate
            | Self::Reset
            | Self::Restart
            | Self::Dotfiles => Ide::NoIde,
        }
    }

    /// The command the session runs, if this verb ends in a session.
    fn command(&self) -> Option<&str> {
        match self {
            Self::Attach { command } => command.as_deref(),
            Self::Recreate | Self::Reset | Self::Restart => None,
            Self::Up | Self::Code | Self::Dotfiles => None,
        }
    }

    /// Whether a `devpod up` this verb *refused* still warms the completion cache.
    ///
    /// `dl.py` 4779/4788 asks for the refresh before it reads the return code for
    /// these two, and returns on the failure first for every other verb. Reproduced
    /// rather than tidied, because the difference is observable: a background child
    /// either ran or it did not.
    fn warms_the_cache_when_up_refuses(&self) -> bool {
        matches!(self, Self::Up | Self::Code)
    }

    /// Whether this verb hands the user a session at the end.
    fn attaches(&self) -> bool {
        matches!(
            self,
            Self::Attach { .. } | Self::Recreate | Self::Reset | Self::Restart
        )
    }
}

/// How a launch ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launched {
    /// A session ran, and its ending is whose the exit code is.
    Session(Session),
    /// The workspace is up and this verb was not asked to attach — `dl <ws> up`,
    /// `dl <ws> code`.
    Ready,
    /// `dl <ws> up` found it already running: nothing to build and nothing to wait
    /// for.
    AlreadyRunning,
    /// Something refused before a session could be handed over. Rendered as one
    /// line and exit 1.
    Refused(LaunchRefusal),
}

/// Why a launch will not go ahead. Every arm is one `logging.error` in `_run_cli`
/// followed by `return 1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchRefusal {
    /// The spec is not one a workspace can be named from.
    UnsafeSpec(UnsafeName),
    /// A bare name devpod cannot describe *and* does not list. Both answers are
    /// needed: `status` consults the provider while `list` reads devpod's own
    /// records, so a workspace whose provider is broken, reconfigured or gone
    /// still lists and cannot be described — and that is precisely the workspace
    /// somebody is about to run `dl <ws> rm` on.
    UnknownWorkspace { name: String },
    /// A bare `owner/repo` whose default branch could not be named.
    BranchNotNamed {
        owner: String,
        repo: String,
        error: BranchNotNamed,
    },
    /// The host-side preparation failed.
    NotPrepared {
        owner: String,
        repo: String,
        branch: String,
        error: NotPrepared,
    },
    /// `devpod up` refused. devpod has already said why on the user's stderr.
    UpRefused { exit: Exit },
    /// The stop half of a `restart` refused, so nothing was started.
    StopRefused { exit: Exit },
    /// No session could be composed or handed over.
    NoSession(SessionRefused),
}

/// Why a launch could not even be attempted.
///
/// The class `main()` handles rather than `_run_cli`: a missing binary and an
/// unreadable listing both travel as types nothing in between catches. The exit
/// codes are the binary's — 127 for the two missing-binary arms, 1 for the
/// unreadable listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchAborted {
    DevpodNotRun(NotRun),
    SshNotRun(ssh::NotRun),
    ListingUnreadable(ListingUnreadable),
}

/// One launch, and everything it needs.
///
/// A value the binary makes one of per command, for the reason
/// [`CommandContext`] is one. It holds the notice *sink*, so no stage can lose half
/// of them by forgetting to thread a channel through — and so every one of them is
/// said at the moment it happens rather than when the launch is over. That matters
/// for exactly the lines whose job is to explain a wait: `Cloning repository …` in
/// front of a three-minute clone, and `Workspace X is already running, attaching...`
/// in front of the shell it is about to hand over.
pub struct Launch<'a, 'r, 'l> {
    context: &'a mut CommandContext<'r>,
    refresh: &'a mut Refresh<'l>,
    cold: &'a mut dyn ColdMachinery<'r>,
    provision: &'a dyn Provision,
    host: &'a Host,
    /// Where devpod's own session diagnostics go, as they happen. The binary's
    /// stderr in production; core writes to nobody's stream.
    forward: &'a mut dyn FnMut(&str),
    token: HostToken,
    /// Where this launch's notices go, as they happen. A `Vec` in a test that wants
    /// the sequence, the binary's printer in production.
    notices: &'a mut dyn Notices<LaunchNotice>,
}

impl<'a, 'r, 'l> Launch<'a, 'r, 'l> {
    pub fn new(
        context: &'a mut CommandContext<'r>,
        refresh: &'a mut Refresh<'l>,
        cold: &'a mut dyn ColdMachinery<'r>,
        provision: &'a dyn Provision,
        host: &'a Host,
        forward: &'a mut dyn FnMut(&str),
        notices: &'a mut dyn Notices<LaunchNotice>,
    ) -> Self {
        Self {
            context,
            refresh,
            cold,
            provision,
            host,
            forward,
            token: HostToken::new(),
            notices,
        }
    }

    /// Run one launch.
    pub fn run(
        &mut self,
        raw_spec: &str,
        verb: &LaunchVerb,
        devcontainer: Option<&DevcontainerPath>,
    ) -> Result<Launched, LaunchAborted> {
        let placement = match self.place(raw_spec)? {
            Ok(placement) => placement,
            Err(refusal) => return Ok(Launched::Refused(refusal)),
        };
        self.carry_out(raw_spec, verb, devcontainer, placement)
    }

    /// Stages one to three: spec to [`Placement`].
    fn place(&mut self, raw_spec: &str) -> Result<Result<Placement, LaunchRefusal>, LaunchAborted> {
        let planned = match plan(raw_spec) {
            Ok(planned) => planned,
            Err(unsafe_name) => return Ok(Err(LaunchRefusal::UnsafeSpec(unsafe_name))),
        };
        match planned {
            Plan::Creatable {
                source,
                workspace_id,
            } => Ok(Ok(Placement::Creating {
                workspace_id,
                source,
            })),
            Plan::Existing { name } => self.place_existing(name),
            Plan::Triple {
                owner,
                repo,
                branch,
                remote_url,
            } => self.place_triple(owner, repo, branch, remote_url),
        }
    }

    /// A bare name: it can only be a workspace devpod already has.
    fn place_existing(
        &mut self,
        name: String,
    ) -> Result<Result<Placement, LaunchRefusal>, LaunchAborted> {
        let state = lifecycle::workspace_state(self.context.runner(), &name);
        if let Ok(state) = state {
            return Ok(Ok(Placement::Known {
                workspace_id: name,
                state,
            }));
        }
        // `status` failing is not the same as the workspace not existing, and the
        // difference decides whether the user can clean it up — so the listing
        // gets the final word. It costs a round trip only on the failure path,
        // where a second one is not what is wrong.
        let listed = self
            .context
            .workspaces()
            .map_err(LaunchAborted::ListingUnreadable)?;
        if listed.iter().any(|workspace| workspace.id == name) {
            return Ok(Ok(Placement::Listed { workspace_id: name }));
        }
        Ok(Err(LaunchRefusal::UnknownWorkspace { name }))
    }

    /// `owner/repo[@branch]`: the arm with a cold path behind it.
    fn place_triple(
        &mut self,
        owner: String,
        repo: String,
        branch: Option<String>,
        remote_url: String,
    ) -> Result<Result<Placement, LaunchRefusal>, LaunchAborted> {
        let branch = match branch {
            Some(branch) => branch,
            None => {
                match name_default_branch(self.cold, &owner, &repo, &remote_url, &mut *self.notices)
                {
                    Ok(branch) => branch,
                    Err(error) => {
                        return Ok(Err(LaunchRefusal::BranchNotNamed { owner, repo, error }));
                    }
                }
            }
        };
        // Constructing the WorkspaceId is the parse boundary: an unsafe owner,
        // repo or ref is rejected here, before it can name a container, a
        // directory or a git command. Nothing downstream re-checks it, because
        // holding the WorkspaceId is the evidence.
        let workspace = match WorkspaceId::new(&owner, &repo, &branch) {
            Ok(workspace) => workspace,
            Err(unsafe_name) => return Ok(Err(LaunchRefusal::UnsafeSpec(unsafe_name))),
        };
        // A devpod that could not be run ends the launch here, before the clone:
        // it is the probe Python raises `DevpodNotInstalled` out of.
        let resolved = resolve_triple(self.context, self.cold, &workspace, &mut *self.notices)
            .map_err(LaunchAborted::DevpodNotRun)?;
        match resolved {
            Resolution::Warm { placement } => Ok(Ok(placement)),
            Resolution::Cold { workspace } => {
                match prepare(self.cold, &workspace, &remote_url, &mut *self.notices) {
                    Ok(placement) => Ok(Ok(placement)),
                    Err(error) => Ok(Err(LaunchRefusal::NotPrepared {
                        owner,
                        repo,
                        branch,
                        error,
                    })),
                }
            }
        }
    }

    /// Stages four and five: `devpod up` and the session.
    fn carry_out(
        &mut self,
        raw_spec: &str,
        verb: &LaunchVerb,
        devcontainer: Option<&DevcontainerPath>,
        placement: Placement,
    ) -> Result<Launched, LaunchAborted> {
        match verb {
            LaunchVerb::Dotfiles => self.run_dotfiles(&placement),
            LaunchVerb::Up => self.run_up_verb(&placement, devcontainer),
            LaunchVerb::Restart => self.run_restart(verb, devcontainer, &placement),
            LaunchVerb::Attach { .. } => self.run_attach(raw_spec, verb, devcontainer, &placement),
            LaunchVerb::Code | LaunchVerb::Recreate | LaunchVerb::Reset => {
                self.run_rebuild(verb, devcontainer, &placement)
            }
        }
    }

    /// `dl <ws>` and `dl <ws> -- cmd`, including the fast-attach arm.
    fn run_attach(
        &mut self,
        raw_spec: &str,
        verb: &LaunchVerb,
        devcontainer: Option<&DevcontainerPath>,
        placement: &Placement,
    ) -> Result<Launched, LaunchAborted> {
        if placement.is_running() {
            // Whatever brought this workspace up finished before this launch
            // asked: nothing to build and nothing to wait for. If a prewarm was
            // fired for it, this is what a prewarm that paid off looks like.
            timing::observe_attach(timing::AttachShape::Hit);
            self.notices.say(LaunchNotice::AlreadyRunningAttaching {
                workspace_id: placement.workspace_id().to_owned(),
            });
            if devcontainer.is_some() {
                self.notices.say(LaunchNotice::DevcontainerIgnoredRunning {
                    workspace_id: placement.workspace_id().to_owned(),
                    spec: raw_spec.to_owned(),
                });
            }
            let session = self.attach(placement.workspace_id(), verb.command());
            self.forced_refresh();
            return session;
        }
        if let Some(refused) = self.bring_up(verb, devcontainer, placement)? {
            return Ok(Launched::Refused(refused));
        }
        let session = self.attach(placement.workspace_id(), verb.command());
        // This path may have created the workspace, so the refresh has to happen
        // now that it exists.
        self.forced_refresh();
        session
    }

    /// `dl <ws> code`, `recreate`, `reset`.
    fn run_rebuild(
        &mut self,
        verb: &LaunchVerb,
        devcontainer: Option<&DevcontainerPath>,
        placement: &Placement,
    ) -> Result<Launched, LaunchAborted> {
        if let Some(refused) = self.bring_up(verb, devcontainer, placement)? {
            return Ok(Launched::Refused(refused));
        }
        if !verb.attaches() {
            self.forced_refresh();
            return Ok(Launched::Ready);
        }
        let session = self.attach(placement.workspace_id(), verb.command());
        self.forced_refresh();
        session
    }

    /// `dl <ws> restart`: stop and start without rebuilding.
    fn run_restart(
        &mut self,
        verb: &LaunchVerb,
        devcontainer: Option<&DevcontainerPath>,
        placement: &Placement,
    ) -> Result<Launched, LaunchAborted> {
        let stopped =
            lifecycle::workspace_stop(self.context, self.refresh, placement.workspace_id())
                .map_err(LaunchAborted::DevpodNotRun)?;
        if let StopOutcome::DevpodRefused { exit } = stopped {
            return Ok(Launched::Refused(LaunchRefusal::StopRefused { exit }));
        }
        if let Some(refused) = self.bring_up(verb, devcontainer, placement)? {
            return Ok(Launched::Refused(refused));
        }
        let session = self.attach(placement.workspace_id(), verb.command());
        // `workspace_stop` already asked for a refresh on the way through; the
        // once-per-command latch is what keeps this from being a second one.
        self.forced_refresh();
        session
    }

    /// `dl <ws> up`: bring it up and stop there.
    fn run_up_verb(
        &mut self,
        placement: &Placement,
        devcontainer: Option<&DevcontainerPath>,
    ) -> Result<Launched, LaunchAborted> {
        if placement.is_running() {
            self.notices.say(LaunchNotice::AlreadyRunning {
                workspace_id: placement.workspace_id().to_owned(),
            });
            // Still top up the tools: `up` is one of the two verbs named as how a
            // workspace that missed provisioning gets it, and returning here
            // without them would make the documented recovery the one path that
            // cannot recover.
            self.provision
                .provision_tools(self.context.runner(), placement.workspace_id())
                .map_err(|DevpodMissing| LaunchAborted::DevpodNotRun(NotRun::NotInstalled))?;
            return Ok(Launched::AlreadyRunning);
        }
        if let Some(refused) = self.bring_up(&LaunchVerb::Up, devcontainer, placement)? {
            return Ok(Launched::Refused(refused));
        }
        self.forced_refresh();
        Ok(Launched::Ready)
    }

    /// `dl <ws> dotfiles`: make sure it is running first.
    ///
    /// The `up` this may do passes **no identity**, because Python's call passes
    /// only `workspace_id=custom_id` — so for a workspace devpod already knows
    /// there is no launch lock, no `DEVLAUNCH_WORKSPACE_ID` stamp and no tools.
    /// Kept as it is: this is the one caller shape [`Naming::Anonymous`] exists
    /// for, and changing it here would be a behaviour change wearing a port's
    /// clothes.
    fn run_dotfiles(&mut self, placement: &Placement) -> Result<Launched, LaunchAborted> {
        let running = lifecycle::workspace_state(self.context.runner(), placement.workspace_id())
            .as_ref()
            .is_ok_and(ContainerState::is_running);
        if !running {
            self.notices.say(LaunchNotice::StartingForDotfiles {
                workspace_id: placement.workspace_id().to_owned(),
            });
            let naming = match placement {
                Placement::Creating { workspace_id, .. } => Naming::Create { workspace_id },
                Placement::Known { .. } | Placement::Listed { .. } => Naming::Anonymous,
            };
            let request = UpRequest::new(placement.source(), naming);
            let outcome = workspace_up(
                self.context,
                self.host,
                &self.token,
                self.provision,
                &request,
                &mut *self.notices,
            )
            .map_err(LaunchAborted::DevpodNotRun)?;
            if let UpOutcome::Refused { exit } = outcome {
                return Ok(Launched::Refused(LaunchRefusal::UpRefused { exit }));
            }
        }
        let session = SessionContext::new(self.context.runner(), self.host, &self.token);
        let refreshed = dotfiles_update(
            &session,
            placement.workspace_id(),
            None,
            self.forward,
            &mut *self.notices,
        );
        Ok(Launched::Session(self.session(refreshed)?))
    }

    /// `devpod up` for this verb, or the refusal it produced.
    fn bring_up(
        &mut self,
        verb: &LaunchVerb,
        devcontainer: Option<&DevcontainerPath>,
        placement: &Placement,
    ) -> Result<Option<LaunchRefusal>, LaunchAborted> {
        let request = UpRequest::new(placement.source(), placement.naming())
            .with_ide(verb.ide())
            .with_rebuild(verb.rebuild())
            .with_devcontainer(devcontainer);
        let outcome = workspace_up(
            self.context,
            self.host,
            &self.token,
            self.provision,
            &request,
            &mut *self.notices,
        )
        .map_err(LaunchAborted::DevpodNotRun)?;
        let refused = match outcome {
            UpOutcome::Started | UpOutcome::SkippedSiblingWon => None,
            UpOutcome::Refused { exit } => Some(LaunchRefusal::UpRefused { exit }),
        };
        // Asked here rather than by whoever renders the refusal, because *when* it is
        // asked is Python's control flow rather than a rendering decision: for `up`
        // and `code` the refresh happens before the return code is read
        // ([`LaunchVerb::warms_the_cache_when_up_refuses`]), so a refused `up` still
        // warms the cache. The once-per-command latch keeps the successful path from
        // asking twice.
        if refused.is_some() && verb.warms_the_cache_when_up_refuses() {
            self.forced_refresh();
        }
        Ok(refused)
    }

    fn attach(
        &mut self,
        workspace_id: &str,
        command: Option<&str>,
    ) -> Result<Launched, LaunchAborted> {
        let context = SessionContext::new(self.context.runner(), self.host, &self.token);
        let session = attach_workspace(
            &context,
            workspace_id,
            command,
            self.forward,
            &mut *self.notices,
        );
        match session {
            Ok(session) => Ok(Launched::Session(session)),
            Err(SessionRefused::Devpod(not_run)) => Err(LaunchAborted::DevpodNotRun(not_run)),
            Err(SessionRefused::Ssh(not_run)) => Err(LaunchAborted::SshNotRun(not_run)),
            Err(other) => Ok(Launched::Refused(LaunchRefusal::NoSession(other))),
        }
    }

    /// A session outcome, with the two never-ran arms lifted to [`LaunchAborted`].
    fn session(
        &mut self,
        outcome: Result<Session, SessionRefused>,
    ) -> Result<Session, LaunchAborted> {
        match outcome {
            Ok(session) => Ok(session),
            Err(SessionRefused::Devpod(not_run)) => Err(LaunchAborted::DevpodNotRun(not_run)),
            Err(SessionRefused::Ssh(not_run)) => Err(LaunchAborted::SshNotRun(not_run)),
            // A payload that cannot be quoted is not a session that failed; the
            // dotfiles command is built here and is always quotable, so this is
            // unreachable in practice and reported rather than panicked on.
            Err(_) => Ok(Session::DevpodFailed {
                exit: Exit::Code(1),
            }),
        }
    }

    /// This command changed what the completion cache describes, so the cache is
    /// wrong however recently it was written.
    fn forced_refresh(&mut self) {
        self.refresh
            .ask(self.context.runner(), RefreshReason::Forced);
    }
}

// ===========================================================================
// tests
// ===========================================================================

#[cfg(test)]
mod tests {
    //! The launch, pinned stage by stage.
    //!
    //! Python's pins for this flow are spread over six files and every one of them
    //! drives `main()` with `subprocess.run` patched, because that is the only
    //! seam Python has. Here the stages are functions, so most of these are about
    //! one stage; the sequences that were only visible from `main()` — the spawn
    //! chains, the repo-lock cycle counts per launch shape — are pinned through
    //! [`Launch::run`], which is the same composition without the process.

    use super::*;

    use std::sync::{Mutex, PoisonError};

    use devlaunch_test_support::{FakeRunner, Response, WorkspaceState};

    use crate::clients::git::Git;
    use crate::domain::config::WorktreeConfig;
    use crate::flows::lifecycle::SelfInvocation;
    use crate::flows::workspace_clone::GitLfs;

    // ------------------------------------------------------------ the scene

    /// A cache directory and the collaborators over it.
    struct Scene {
        dir: tempfile::TempDir,
        runner: FakeRunner,
        host: Host,
        /// See [`timing::exclusive`]. Last field, so it is dropped last.
        _serialized: timing::Exclusive,
    }

    impl Scene {
        /// A host with no terminal, no gh login, and a scratch cache.
        ///
        /// Takes [`timing::exclusive`], because the registry is process-global and
        /// a test that runs launch code either measures it or records into whatever
        /// a concurrent test installed. Holding it here rather than per test is
        /// what makes it impossible to forget.
        fn new() -> Self {
            let serialized = timing::exclusive();
            let dir = tempfile::tempdir().expect("a scratch cache");
            let host = Host {
                // Opted out, so nothing in these tests spawns `gh` unless it says
                // it means to.
                gh: gh::HostEnv {
                    disable: Some("1".to_owned()),
                    ..gh::HostEnv::default()
                },
                cache_dir: dir.path().to_path_buf(),
                devpod_home: Some(dir.path().join("devpod")),
                ..Host::default()
            };
            Self {
                dir,
                runner: FakeRunner::new(),
                host,
                _serialized: serialized,
            }
        }

        /// A host on a terminal, with devpod's alias published for `workspace_ids`.
        fn on_a_terminal(mut self, workspace_ids: &[&str]) -> Self {
            let config = self.dir.path().join("ssh-config");
            let text: String = workspace_ids
                .iter()
                .map(|id| format!("# DevPod Start {id}.devpod\nHost {id}.devpod\n"))
                .collect();
            std::fs::write(&config, text).expect("an ssh config");
            self.host.stdin_tty = true;
            self.host.stdout_tty = true;
            self.host.ssh_config = Some(config);
            self
        }

        fn with_running(self, workspace_id: &str) -> Self {
            self.runner
                .add_workspace(workspace_id, WorkspaceState::Running);
            self
        }

        fn with_stopped(self, workspace_id: &str) -> Self {
            self.runner
                .add_workspace(workspace_id, WorkspaceState::Stopped);
            self
        }

        fn cache_dir(&self) -> &Path {
            self.dir.path()
        }

        /// Every devpod invocation, without the leading `devpod`, in order — the
        /// shape `test_devpod_spawn_counts.py` asserts.
        fn devpod_commands(&self) -> Vec<Vec<String>> {
            self.runner.args_to("devpod")
        }

        fn devpod_heads(&self) -> Vec<Vec<String>> {
            self.devpod_commands()
                .into_iter()
                .map(|argv| argv.into_iter().take(2).collect())
                .collect()
        }
    }

    /// The cold machinery over a real cache on disk.
    struct RealCold<'r> {
        clones: WorkspaceCloneManager<'r>,
        storage: MetadataStorage,
        opens: std::cell::Cell<usize>,
    }

    impl<'r> RealCold<'r> {
        fn new(cache_dir: &Path, git: Git<'r>) -> Self {
            let config = WorktreeConfig::defaults_in(cache_dir);
            let (storage, _) = MetadataStorage::open(cache_dir.join("metadata.json"))
                .expect("a fresh store opens");
            Self {
                clones: WorkspaceCloneManager::new(
                    &config.repos_dir,
                    Duration::from_secs(config.fetch_interval),
                    git,
                    // The LFS fork is a separate concern, pinned in
                    // workspace_clone.rs; every launch here has no git-lfs.
                    GitLfs::NotInstalled,
                ),
                storage,
                opens: std::cell::Cell::new(0),
            }
        }
    }

    impl<'r> ColdMachinery<'r> for RealCold<'r> {
        fn open(&mut self) -> Result<Cold<'_, 'r>, ColdRefused> {
            self.opens.set(self.opens.get() + 1);
            Ok(Cold {
                clones: &self.clones,
                storage: &mut self.storage,
            })
        }
    }

    /// A cold path that fails the test if anything opens it.
    ///
    /// This is how devlaunch#145 is observed: a warm launch must not read
    /// `metadata.json`, and the way to say that here is that the thing which reads
    /// it is never even built.
    struct NeverCold;

    impl<'r> ColdMachinery<'r> for NeverCold {
        fn open(&mut self) -> Result<Cold<'_, 'r>, ColdRefused> {
            panic!("a warm launch opened the cold path");
        }
    }

    /// Records which workspaces had tools lent to them.
    #[derive(Debug, Default)]
    struct RecordingProvision {
        provisioned: Mutex<Vec<String>>,
        /// A devpod that goes missing when the pass is asked for, which is the one
        /// thing a pass can answer.
        lost_devpod: bool,
    }

    impl RecordingProvision {
        fn provisioned(&self) -> Vec<String> {
            self.provisioned
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Provision for RecordingProvision {
        fn provision_tools(
            &self,
            _runner: &dyn Runner,
            workspace_id: &str,
        ) -> Result<(), DevpodMissing> {
            self.provisioned
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(workspace_id.to_owned());
            if self.lost_devpod {
                return Err(DevpodMissing);
            }
            Ok(())
        }
    }

    fn no_notices() -> Vec<LaunchNotice> {
        Vec::new()
    }

    /// A sink that drops devpod's own session chatter, for the tests that are not
    /// about it. A plain `fn` pointer, because that is an `FnMut(&str)` a struct
    /// can hold by value and hand out as `&mut dyn FnMut(&str)`.
    fn nowhere(_line: &str) {}

    /// [`attach_workspace`] with the chatter thrown away.
    fn attaching(
        scene: &Scene,
        token: &HostToken,
        workspace_id: &str,
        command: Option<&str>,
        notices: &mut Vec<LaunchNotice>,
    ) -> Result<Session, SessionRefused> {
        let session = SessionContext::new(&scene.runner, &scene.host, token);
        attach_workspace(&session, workspace_id, command, &mut nowhere, notices)
    }

    /// Backdate `path`'s modification time by `by`.
    fn aged(path: &Path, by: Duration) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("the file to backdate");
        let was = file
            .metadata()
            .expect("its metadata")
            .modified()
            .expect("an mtime");
        file.set_modified(was - by).expect("a backdated mtime");
    }

    // -------------------------------------------------- the launch lock path

    #[test]
    fn the_launch_lock_is_keyed_by_workspace_and_not_by_repo() {
        // Two nodes of one repo launch at once by design (one branch, one
        // container each) — a repo-keyed lock would serialize them for nothing.
        let scene = Scene::new();

        let one = scene.host.launch_lock_path("wayfinder-16-abc");
        let two = scene.host.launch_lock_path("wayfinder-17-def");

        assert_ne!(one, two);
        assert_eq!(one.parent(), two.parent());
    }

    #[test]
    fn the_launch_lock_lives_outside_the_repo_cache() {
        // The cache's walkers read every directory under repos/ as a repo, and
        // these locks are also taken for workspaces that have no clone there at
        // all (paths, URLs).
        let scene = Scene::new();

        let path = scene.host.launch_lock_path("myws");

        assert!(
            !path.components().any(|part| part.as_os_str() == "repos"),
            "{path:?}"
        );
        assert!(path.starts_with(scene.cache_dir()), "{path:?}");
        assert_eq!(
            path.file_name().map(std::ffi::OsStr::to_string_lossy),
            Some("myws.lock".into())
        );
    }

    #[test]
    fn an_up_with_no_identity_takes_no_lock() {
        // Nothing to key it on. The caller shapes that reach here are not the
        // concurrent-launch ones.
        let scene = Scene::new();
        let mut notices = no_notices();

        let serialization = serialize_launch(&scene.host, Naming::Anonymous, &mut notices);

        assert!(matches!(serialization, Serialization::Unkeyed));
        assert_eq!(notices, no_notices());
        assert!(!scene.cache_dir().join(LAUNCH_LOCK_DIR).exists());
    }

    #[test]
    fn a_second_launch_of_one_workspace_queues_behind_the_first() {
        // flock is per open file description, so two acquisitions conflict even
        // inside one process — which is what lets this pin the serialization
        // without subprocesses.
        let scene = Scene::new();
        let mut first = no_notices();
        let held = serialize_launch(
            &scene.host,
            Naming::Known {
                workspace_id: "myws",
            },
            &mut first,
        );
        assert!(matches!(held, Serialization::WalkedIn { .. }));
        assert_eq!(first, no_notices(), "walking in is silent");

        let waiter = std::thread::scope(|scope| {
            let waiting = scope.spawn(|| {
                let mut notices = no_notices();
                let serialization = serialize_launch(
                    &scene.host,
                    Naming::Known {
                        workspace_id: "myws",
                    },
                    &mut notices,
                );
                (serialization.waited(), notices)
            });
            std::thread::sleep(Duration::from_millis(30));
            drop(held);
            waiting.join().expect("the waiter finished")
        });

        assert!(waiter.0, "the second launch had to queue");
        assert_eq!(
            waiter.1,
            vec![LaunchNotice::WaitingForSiblingLaunch {
                workspace_id: "myws".to_owned()
            }],
            "and it said so before it blocked"
        );
    }

    #[test]
    fn a_launch_of_another_workspace_never_queues() {
        let scene = Scene::new();
        let mut notices = no_notices();
        let _held = serialize_launch(
            &scene.host,
            Naming::Known {
                workspace_id: "one",
            },
            &mut notices,
        );

        let other = serialize_launch(
            &scene.host,
            Naming::Known {
                workspace_id: "two",
            },
            &mut notices,
        );

        assert!(matches!(other, Serialization::WalkedIn { .. }));
    }

    #[test]
    fn a_lock_file_that_cannot_be_opened_is_survivable() {
        // A container writing as another uid is a documented occurrence in this
        // cache, and a full or read-only disk lands here too.
        let scene = Scene::new();
        // A plain file where the lock directory has to go: creating the parent
        // fails, and so would the open.
        std::fs::write(scene.cache_dir().join(LAUNCH_LOCK_DIR), "not a directory")
            .expect("the obstruction");
        let mut notices = no_notices();

        let serialization = serialize_launch(
            &scene.host,
            Naming::Known {
                workspace_id: "myws",
            },
            &mut notices,
        );

        assert!(
            matches!(serialization, Serialization::Unavailable { .. }),
            "{serialization:?}"
        );
        assert!(!serialization.waited(), "an unavailable lock never waited");
        assert!(matches!(
            notices.as_slice(),
            [LaunchNotice::LaunchLockUnavailable { workspace_id, .. }] if workspace_id == "myws"
        ));
    }

    // ------------------------------------------------------- a contended up

    /// Run `workspace_up` with the launch lock already held by a sibling, which
    /// is what makes the call contended.
    fn contended_up<P: Provision + Sync>(
        scene: &Scene,
        request: &UpRequest<'_>,
        provision: &P,
    ) -> (Result<UpOutcome, NotRun>, Vec<LaunchNotice>) {
        let mut notices = no_notices();
        let mut sibling = no_notices();
        let held = serialize_launch(&scene.host, request.naming, &mut sibling);
        assert!(matches!(held, Serialization::WalkedIn { .. }));
        let outcome = std::thread::scope(|scope| {
            let launching = scope.spawn(|| {
                let mut context = CommandContext::new(&scene.runner);
                let token = HostToken::new();
                let mut inner = no_notices();
                let outcome = workspace_up(
                    &mut context,
                    &scene.host,
                    &token,
                    provision,
                    request,
                    &mut inner,
                );
                (outcome, inner)
            });
            std::thread::sleep(Duration::from_millis(30));
            drop(held);
            launching.join().expect("the launch finished")
        });
        notices.extend(outcome.1);
        (outcome.0, notices)
    }

    #[test]
    fn a_contended_up_of_a_running_workspace_runs_no_up_at_all() {
        // The prewarm won the race, so the launch has nothing left to do but
        // succeed — `devpod up` here would re-walk a whole container lifecycle to
        // arrive where the workspace already is.
        let scene = Scene::new().with_running("myws");
        let provision = RecordingProvision::default();
        let request = UpRequest::new(
            "owner/repo",
            Naming::Create {
                workspace_id: "myws",
            },
        );

        let (outcome, notices) = contended_up(&scene, &request, &provision);

        assert_eq!(outcome, Ok(UpOutcome::SkippedSiblingWon));
        assert!(
            !scene
                .devpod_commands()
                .iter()
                .any(|argv| argv.first().map(String::as_str) == Some("up")),
            "{:?}",
            scene.devpod_commands()
        );
        assert!(notices.contains(&LaunchNotice::BroughtUpBySibling {
            workspace_id: "myws".to_owned()
        }));
    }

    #[test]
    fn the_skipped_up_still_makes_sure_the_tools_are_there() {
        // "Running" says the sibling's `devpod up` returned, not that its install
        // did: the sibling can be interrupted between the two (the flock dies with
        // the process), its `up` can fail after the container has started, or it
        // can have run with the tools switched off where this one did not.
        let scene = Scene::new().with_running("myws");
        let provision = RecordingProvision::default();
        let request = UpRequest::new(
            "owner/repo",
            Naming::Create {
                workspace_id: "myws",
            },
        );

        let (outcome, _) = contended_up(&scene, &request, &provision);

        assert_eq!(outcome, Ok(UpOutcome::SkippedSiblingWon));
        assert_eq!(provision.provisioned(), vec!["myws".to_owned()]);
    }

    #[test]
    fn a_contended_up_of_a_stopped_workspace_is_still_brought_up() {
        // Waiting is not evidence the sibling succeeded: a prewarm that failed, or
        // that only got as far as creating a stopped workspace, leaves the launch
        // exactly the work it came to do.
        let scene = Scene::new().with_stopped("myws");
        let provision = RecordingProvision::default();
        let request = UpRequest::new(
            "owner/repo",
            Naming::Create {
                workspace_id: "myws",
            },
        );

        let (outcome, _) = contended_up(&scene, &request, &provision);

        assert_eq!(outcome, Ok(UpOutcome::Started));
        assert_eq!(
            scene
                .devpod_commands()
                .iter()
                .filter(|argv| argv.first().map(String::as_str) == Some("up"))
                .count(),
            1
        );
        assert_eq!(provision.provisioned(), vec!["myws".to_owned()]);
    }

    #[test]
    fn a_side_effect_the_sibling_cannot_have_had_is_never_skipped() {
        // An IDE to open, a container to rebuild from scratch, a different
        // devcontainer variant: a running workspace is not the answer to any of
        // these. The variant is the one that would be silent about it — a prewarm
        // brings up the default container, a human then asks for
        // `--devcontainer robot` and waits on the lock; skipping there would
        // attach them to the default and never say so.
        let variant = spec::resolve_devcontainer_ref("robot").expect("a variant");
        let cases: [UpRequest<'_>; 4] = [
            UpRequest::new(
                "owner/repo",
                Naming::Create {
                    workspace_id: "myws",
                },
            )
            .with_ide(Ide::Named("vscode")),
            UpRequest::new(
                "owner/repo",
                Naming::Create {
                    workspace_id: "myws",
                },
            )
            .with_rebuild(Rebuild::Recreate),
            UpRequest::new(
                "owner/repo",
                Naming::Create {
                    workspace_id: "myws",
                },
            )
            .with_rebuild(Rebuild::Reset),
            UpRequest::new(
                "owner/repo",
                Naming::Create {
                    workspace_id: "myws",
                },
            )
            .with_devcontainer(Some(&variant)),
        ];

        for request in &cases {
            let scene = Scene::new().with_running("myws");
            let provision = RecordingProvision::default();

            let (outcome, _) = contended_up(&scene, request, &provision);

            assert_eq!(outcome, Ok(UpOutcome::Started), "{request:?}");
            assert_eq!(
                scene
                    .devpod_commands()
                    .iter()
                    .filter(|argv| argv.first().map(String::as_str) == Some("up"))
                    .count(),
                1,
                "{request:?}"
            );
        }
    }

    #[test]
    fn an_uncontended_up_never_asks_for_the_state() {
        // No sibling ran, so nothing can have changed under this process — the
        // re-check would be a round trip bought with no question to answer.
        let scene = Scene::new().with_running("myws");
        let mut context = CommandContext::new(&scene.runner);
        let token = HostToken::new();
        let request = UpRequest::new(
            "owner/repo",
            Naming::Create {
                workspace_id: "myws",
            },
        );

        let outcome = workspace_up(
            &mut context,
            &scene.host,
            &token,
            &NoProvisioning,
            &request,
            &mut no_notices(),
        );

        assert_eq!(outcome, Ok(UpOutcome::Started));
        assert!(
            !scene
                .devpod_commands()
                .iter()
                .any(|argv| argv.first().map(String::as_str) == Some("status")),
            "{:?}",
            scene.devpod_commands()
        );
    }

    #[test]
    fn an_up_whose_lock_could_not_be_taken_still_happens() {
        // Serialization guards a race that may not be happening; an errno failure
        // in front of a `devpod up` that would have worked is the worse answer.
        let scene = Scene::new();
        std::fs::write(scene.cache_dir().join(LAUNCH_LOCK_DIR), "not a directory")
            .expect("the obstruction");
        let provision = RecordingProvision::default();
        let mut context = CommandContext::new(&scene.runner);
        let token = HostToken::new();
        let request = UpRequest::new(
            "owner/repo",
            Naming::Create {
                workspace_id: "myws",
            },
        );

        let outcome = workspace_up(
            &mut context,
            &scene.host,
            &token,
            &provision,
            &request,
            &mut no_notices(),
        );

        assert_eq!(outcome, Ok(UpOutcome::Started));
        assert_eq!(provision.provisioned(), vec!["myws".to_owned()]);
    }

    #[test]
    fn a_devpod_up_that_refuses_lends_no_tools() {
        // There is no container to install into.
        let scene = Scene::new();
        scene
            .runner
            .script(["devpod", "up"], Response::failed(1, "no such provider\n"));
        let provision = RecordingProvision::default();
        let mut context = CommandContext::new(&scene.runner);
        let token = HostToken::new();
        let request = UpRequest::new(
            "owner/repo",
            Naming::Create {
                workspace_id: "myws",
            },
        );

        let outcome = workspace_up(
            &mut context,
            &scene.host,
            &token,
            &provision,
            &request,
            &mut no_notices(),
        );

        assert_eq!(
            outcome,
            Ok(UpOutcome::Refused {
                exit: Exit::Code(1)
            })
        );
        assert_eq!(provision.provisioned(), Vec::<String>::new());
    }

    #[test]
    fn a_devpod_that_is_not_installed_is_not_an_up_that_refused() {
        let scene = Scene::new();
        scene.runner.script_missing("devpod");
        let mut context = CommandContext::new(&scene.runner);
        let token = HostToken::new();
        let request = UpRequest::new(
            "owner/repo",
            Naming::Create {
                workspace_id: "myws",
            },
        );

        let outcome = workspace_up(
            &mut context,
            &scene.host,
            &token,
            &NoProvisioning,
            &request,
            &mut no_notices(),
        );

        assert_eq!(outcome, Err(NotRun::NotInstalled));
    }

    // --------------------------------------------------------- the up argv

    #[test]
    fn the_whole_up_argv_is_what_dl_hands_devpod() {
        // Pinned whole rather than by membership, so a flag added at this seam has
        // to be added here too — which is what happened for the shared pixi cache
        // (devlaunch#232).
        let scene = Scene::new();
        let pixi = PixiCache::ensure(scene.host.pixi_cache_source());
        let request = UpRequest::new(
            "brand-new",
            Naming::Create {
                workspace_id: "brand-new",
            },
        );

        let args = up_args(&request, &ContextOptions::default(), &pixi, &[]);

        assert_eq!(
            args,
            vec![
                "up".to_owned(),
                "brand-new".to_owned(),
                "--id".to_owned(),
                "brand-new".to_owned(),
                "--ide".to_owned(),
                "none".to_owned(),
                "--init-env".to_owned(),
                "DEVLAUNCH_WORKSPACE_ID=brand-new".to_owned(),
                "--mount".to_owned(),
                format!(
                    "type=bind,source={},target=/var/tmp/devlaunch-pixi",
                    scene.host.pixi_cache_source().display()
                ),
                "--workspace-env".to_owned(),
                "PIXI_CACHE_DIR=/var/tmp/devlaunch-pixi".to_owned(),
                "--dotfiles-script-env".to_owned(),
                "PIXI_CACHE_DIR=/var/tmp/devlaunch-pixi".to_owned(),
            ]
        );
    }

    #[test]
    fn a_workspace_devpod_already_knows_gets_no_id_flag() {
        // `--id` is passed only when creating; devpod would refuse it for a
        // workspace it already has.
        let request = UpRequest::new(
            "myws",
            Naming::Known {
                workspace_id: "myws",
            },
        );

        let args = up_args(
            &request,
            &ContextOptions::default(),
            &PixiCache::NotADirectory {
                source: PathBuf::from("/nope"),
            },
            &[],
        );

        assert_eq!(
            args,
            vec![
                "up".to_owned(),
                "myws".to_owned(),
                "--ide".to_owned(),
                "none".to_owned(),
                "--init-env".to_owned(),
                "DEVLAUNCH_WORKSPACE_ID=myws".to_owned(),
            ]
        );
    }

    #[test]
    fn an_anonymous_up_stamps_no_workspace_id() {
        // Nothing names it, so there is nothing for a host-side
        // `initializeCommand` to be told.
        let request = UpRequest::new("owner/repo", Naming::Anonymous);

        let args = up_args(
            &request,
            &ContextOptions::default(),
            &PixiCache::NotADirectory {
                source: PathBuf::from("/nope"),
            },
            &[],
        );

        assert_eq!(
            args,
            vec![
                "up".to_owned(),
                "owner/repo".to_owned(),
                "--ide".to_owned(),
                "none".to_owned(),
            ]
        );
        assert_eq!(request.naming.identity(), None);
        assert_eq!(request.naming.create_as(), None);
    }

    #[test]
    fn the_ide_is_always_stated_and_defaults_to_none() {
        // dl attaches a terminal shell, so devpod's configured default IDE must
        // not open a window on every `dl <ws>`.
        let bare = UpRequest::new("myws", Naming::Anonymous);
        let coded = bare.with_ide(Ide::Named("vscode"));

        assert_eq!(bare.ide.word(), "none");
        assert_eq!(coded.ide.word(), "vscode");
    }

    #[test]
    fn a_rebuild_is_one_flag_and_never_two() {
        let nothing = PixiCache::NotADirectory {
            source: PathBuf::from("/nope"),
        };
        let of = |rebuild| {
            up_args(
                &UpRequest::new("myws", Naming::Anonymous).with_rebuild(rebuild),
                &ContextOptions::default(),
                &nothing,
                &[],
            )
        };

        assert!(!of(Rebuild::Reuse).iter().any(|arg| arg.starts_with("--re")));
        assert_eq!(
            of(Rebuild::Recreate).last().map(String::as_str),
            Some("--recreate")
        );
        assert_eq!(
            of(Rebuild::Reset).last().map(String::as_str),
            Some("--reset")
        );
    }

    #[test]
    fn the_dotfiles_the_context_configures_reach_the_up() {
        let options = ContextOptions::from_map(BTreeMap::from([
            ("DOTFILES_URL".to_owned(), "https://example/dots".to_owned()),
            ("DOTFILES_SCRIPT".to_owned(), "install.sh".to_owned()),
            // An option dl does not read reaches no flag.
            ("SOMETHING_ELSE".to_owned(), "1".to_owned()),
        ]));

        let args = up_args(
            &UpRequest::new("myws", Naming::Anonymous),
            &options,
            &PixiCache::NotADirectory {
                source: PathBuf::from("/nope"),
            },
            &[],
        );

        assert_eq!(
            args,
            vec![
                "up".to_owned(),
                "myws".to_owned(),
                "--ide".to_owned(),
                "none".to_owned(),
                "--dotfiles".to_owned(),
                "https://example/dots".to_owned(),
                "--dotfiles-script".to_owned(),
                "install.sh".to_owned(),
            ]
        );
    }

    #[test]
    fn a_devcontainer_variant_becomes_a_path_flag() {
        let variant = spec::resolve_devcontainer_ref("robot").expect("a variant");

        let args = up_args(
            &UpRequest::new("myws", Naming::Anonymous).with_devcontainer(Some(&variant)),
            &ContextOptions::default(),
            &PixiCache::NotADirectory {
                source: PathBuf::from("/nope"),
            },
            &[],
        );

        assert_eq!(
            args,
            vec![
                "up".to_owned(),
                "myws".to_owned(),
                "--ide".to_owned(),
                "none".to_owned(),
                "--devcontainer-path".to_owned(),
                ".devcontainer/robot/devcontainer.json".to_owned(),
            ]
        );
    }

    // ------------------------------------------------- the shared pixi cache

    #[test]
    fn the_shared_pixi_cache_is_created_and_bound() {
        let scene = Scene::new();

        let pixi = PixiCache::ensure(scene.host.pixi_cache_source());

        assert!(matches!(pixi, PixiCache::Shared { .. }));
        assert!(scene.host.pixi_cache_source().is_dir());
        assert_eq!(pixi.notice(), None);
    }

    #[test]
    fn a_pixi_cache_that_cannot_be_created_costs_the_sharing_and_not_the_launch() {
        let scene = Scene::new();
        std::fs::write(scene.host.pixi_cache_source(), "a file in the way")
            .expect("the obstruction");

        let pixi = PixiCache::ensure(scene.host.pixi_cache_source());

        assert!(matches!(pixi, PixiCache::NotCreated { .. }), "{pixi:?}");
        assert_eq!(pixi.up_args(), Vec::<String>::new());
        assert!(matches!(
            pixi.notice(),
            Some(LaunchNotice::PixiCacheNotCreated { .. })
        ));
    }

    #[test]
    fn the_mount_target_is_outside_every_home_directory() {
        // devlaunch#240: a bind target whose parent the image does not ship is
        // created by the runtime as root, so pointing this into `~/.cache` handed
        // containers a root-owned home cache.
        assert_eq!(PIXI_CACHE_TARGET, "/var/tmp/devlaunch-pixi");
    }

    // ----------------------------------------------- devpod's context options

    #[test]
    fn the_options_are_asked_for_once_and_then_read_from_disk() {
        let scene = Scene::new();
        scene.runner.script(
            ["devpod", "context", "options"],
            Response::stdout(r#"{"DOTFILES_URL": {"value": "https://example/dots"}}"#),
        );
        let cache = scene.host.context_options_cache();

        let first = context_options(&scene.runner, &cache, None, SystemTime::now());
        let second = context_options(&scene.runner, &cache, None, SystemTime::now());

        assert_eq!(first.dotfiles_url(), Some("https://example/dots"));
        assert_eq!(second, first);
        assert_eq!(
            scene
                .devpod_commands()
                .iter()
                .filter(|argv| argv.first().map(String::as_str) == Some("context"))
                .count(),
            1
        );
    }

    #[test]
    fn a_cache_older_than_the_ttl_is_asked_again() {
        let scene = Scene::new();
        let cache = scene.host.context_options_cache();
        std::fs::write(&cache, r#"{"DOTFILES_URL":"https://stale/dots"}"#).expect("a cache");

        let options = context_options(
            &scene.runner,
            &cache,
            None,
            SystemTime::now() + CONTEXT_OPTIONS_TTL + Duration::from_secs(1),
        );

        assert_eq!(options, ContextOptions::default());
        assert_eq!(
            scene.devpod_heads(),
            vec![vec!["context".to_owned(), "options".to_owned()]]
        );
    }

    #[test]
    fn a_cache_older_than_devpods_own_config_is_stale_whatever_its_age() {
        // These options are *per context*, and this is one cache file, so
        // `devpod context use <other>` would otherwise feed the previous
        // context's dotfiles settings to `devpod up` for up to an hour.
        let scene = Scene::new();
        let cache = scene.host.context_options_cache();
        std::fs::write(&cache, r#"{"DOTFILES_URL":"https://stale/dots"}"#).expect("a cache");
        // Aged by hand rather than by sleeping: two writes in one millisecond can
        // land on one filesystem timestamp, and what this pins is the comparison.
        aged(&cache, Duration::from_secs(10));
        let config = scene.host.devpod_config().expect("a devpod home");
        std::fs::create_dir_all(config.parent().expect("a parent")).expect("devpod's home");
        std::fs::write(&config, "contexts: {}\n").expect("devpod's config");

        let options = context_options(&scene.runner, &cache, Some(&config), SystemTime::now());

        assert_eq!(options, ContextOptions::default());
        assert_eq!(
            scene.devpod_heads(),
            vec![vec!["context".to_owned(), "options".to_owned()]]
        );
    }

    #[test]
    fn no_config_file_to_disagree_with_leaves_the_ttl_as_the_whole_test() {
        let scene = Scene::new();
        let cache = scene.host.context_options_cache();
        std::fs::write(&cache, r#"{"DOTFILES_URL":"https://cached/dots"}"#).expect("a cache");

        let options = context_options(
            &scene.runner,
            &cache,
            Some(&scene.cache_dir().join("no-such-config.yaml")),
            SystemTime::now(),
        );

        assert_eq!(options.dotfiles_url(), Some("https://cached/dots"));
        assert_eq!(scene.devpod_commands(), Vec::<Vec<String>>::new());
    }

    #[test]
    fn a_cache_that_is_not_a_map_of_strings_reads_as_no_cache_at_all() {
        // Divergence from Python, which keeps whatever the values are: a
        // hand-edited cache holding a number would otherwise reach a `devpod up`
        // flag. Parse-don't-validate at the boundary.
        let scene = Scene::new();
        let cache = scene.host.context_options_cache();
        for junk in ["not json", "[]", r#"{"DOTFILES_URL": 7}"#] {
            std::fs::write(&cache, junk).expect("a cache");
            scene.runner.forget_calls();

            let options = context_options(&scene.runner, &cache, None, SystemTime::now());

            assert_eq!(options, ContextOptions::default(), "{junk:?}");
            assert_eq!(
                scene.devpod_heads(),
                vec![vec!["context".to_owned(), "options".to_owned()]],
                "{junk:?}"
            );
        }
    }

    #[test]
    fn only_an_answer_devpod_gave_is_cached() {
        // A failed or unreadable read costs nothing worse than the uncached
        // behaviour: the empty set, asked again next time.
        let scene = Scene::new();
        scene.runner.script(
            ["devpod", "context", "options"],
            Response::failed(1, "no context\n"),
        );
        let cache = scene.host.context_options_cache();

        let options = context_options(&scene.runner, &cache, None, SystemTime::now());

        assert_eq!(options, ContextOptions::default());
        assert!(!cache.exists(), "nothing was remembered");
    }

    // ----------------------------------------------------- the remote payload

    #[test]
    fn a_command_is_wrapped_for_a_login_shell() {
        // devpod runs `--command` under a non-login, non-interactive `bash -c`,
        // which sources neither ~/.profile nor ~/.bashrc.
        let payload = RemotePayload::wrap("echo hi", ZellijWrap::Off).expect("quotable");

        assert_eq!(payload.as_str(), "bash -lc 'echo hi'");
    }

    #[test]
    fn a_quoted_prompt_reaches_the_agent_intact() {
        // `aid repo fix the bug` becomes one quoted argument; it must stay one.
        let payload =
            RemotePayload::wrap("claude 'fix the bug'", ZellijWrap::Off).expect("quotable");

        assert_eq!(
            payload.as_str(),
            r#"bash -lc 'claude '"'"'fix the bug'"'"''"#
        );
    }

    #[test]
    fn the_zellij_wrap_ensures_a_session_beside_the_command() {
        let payload = RemotePayload::wrap("echo hi", ZellijWrap::Beside).expect("quotable");

        assert_eq!(
            payload.as_str(),
            "bash -lc 'zellij attach -b devlaunch >/dev/null 2>&1 || true; echo hi'"
        );
    }

    #[test]
    fn the_zellij_switch_reads_the_same_denials_as_the_others() {
        // A variable exported empty is what an unset variable looks like to a
        // shell that mentions it, and `=0` is what someone who once turned it on
        // writes to turn it back off.
        for denial in ["", "0", "false", "no", "FALSE", " no "] {
            let host = Host {
                zellij: Some(denial.to_owned()),
                ..Host::default()
            };
            assert_eq!(ZellijWrap::from_host(&host), ZellijWrap::Off, "{denial:?}");
        }
        for consent in ["1", "yes", "true", "beside"] {
            let host = Host {
                zellij: Some(consent.to_owned()),
                ..Host::default()
            };
            assert_eq!(
                ZellijWrap::from_host(&host),
                ZellijWrap::Beside,
                "{consent:?}"
            );
        }
        assert_eq!(ZellijWrap::from_host(&Host::default()), ZellijWrap::Off);
    }

    #[test]
    fn a_command_no_remote_shell_could_be_given_is_refused() {
        // A NUL cannot survive a shell word, so there is no payload to build.
        // Python has no such refusal — `shlex.quote` wraps it and the remote shell
        // mangles it.
        assert_eq!(
            RemotePayload::wrap("echo \0hi", ZellijWrap::Off),
            Err(UnquotableCommand {
                command: "echo \0hi".to_owned()
            })
        );
    }

    // ------------------------------------------------------ which transport

    #[test]
    fn a_terminal_and_an_alias_route_a_command_through_openssh() {
        // The regression: an interactive payload must not go through `--command`.
        let scene = Scene::new().on_a_terminal(&["myws"]);

        assert_eq!(terminal_for(&scene.host, "myws"), Terminal::Usable);
        let payload = RemotePayload::wrap("claude", ZellijWrap::Off).expect("quotable");
        assert_eq!(
            route(Some(&payload), Terminal::Usable, "myws", &mut no_notices()),
            Route::Terminal(&payload)
        );
    }

    #[test]
    fn no_terminal_keeps_the_devpod_transport() {
        // Piped output must stay clean, so no pty and no escape sequences.
        let scene = Scene::new();

        assert_eq!(terminal_for(&scene.host, "myws"), Terminal::Absent);
    }

    #[test]
    fn the_tty_opt_out_forces_the_devpod_transport() {
        let scene = Scene::new().on_a_terminal(&["myws"]);
        let opted_out = Host {
            no_tty: Some("1".to_owned()),
            ..scene.host.clone()
        };

        assert_eq!(terminal_for(&opted_out, "myws"), Terminal::Absent);
    }

    #[test]
    fn a_missing_host_alias_falls_back_and_says_so() {
        // A workspace devpod never wrote an alias for still has to run the command.
        let scene = Scene::new().on_a_terminal(&["some-other-workspace"]);
        let mut notices = no_notices();

        assert_eq!(terminal_for(&scene.host, "myws"), Terminal::NoAlias);
        let payload = RemotePayload::wrap("claude", ZellijWrap::Off).expect("quotable");
        assert_eq!(
            route(Some(&payload), Terminal::NoAlias, "myws", &mut notices),
            Route::DevpodCommand(&payload)
        );
        assert_eq!(
            notices,
            vec![LaunchNotice::NoTerminalAlias {
                workspace_id: "myws".to_owned()
            }]
        );
    }

    #[test]
    fn a_bare_attach_stays_on_devpod_however_good_the_terminal_is() {
        // `dl <ws>` with no command already gets a pty from devpod, which is the
        // one case devpod requests one for.
        let mut notices = no_notices();

        for terminal in [Terminal::Usable, Terminal::NoAlias, Terminal::Absent] {
            assert_eq!(
                route(None, terminal, "myws", &mut notices),
                Route::DevpodAttach,
                "{terminal:?}"
            );
        }
        assert_eq!(notices, no_notices(), "and nothing to report either");
    }

    // --------------------------------------------------------- the session

    /// One session over `scene`: what it ended as, what it reported, and what
    /// devpod said on its own stderr along the way.
    fn a_session(
        scene: &Scene,
        command: Option<&str>,
    ) -> (
        Result<Session, SessionRefused>,
        Vec<LaunchNotice>,
        Vec<String>,
    ) {
        let token = HostToken::new();
        let mut notices = no_notices();
        let mut said = Vec::new();
        let context = SessionContext::new(&scene.runner, &scene.host, &token);
        let session = workspace_ssh(
            &context,
            "myws",
            command,
            None,
            &mut |line| said.push(line.to_owned()),
            &mut notices,
        );
        (session, notices, said)
    }

    #[test]
    fn an_interactive_attach_is_one_devpod_ssh_and_nothing_else() {
        let scene = Scene::new().with_running("myws");

        let (session, _, _) = a_session(&scene, None);

        assert_eq!(session, Ok(Session::RemoteExit { status: 0 }));
        assert_eq!(
            scene.devpod_commands(),
            vec![vec!["ssh".to_owned(), "myws".to_owned()]]
        );
    }

    #[test]
    fn a_one_shot_command_travels_as_the_shlex_quoted_payload() {
        let scene = Scene::new().with_running("myws");

        let (session, _, _) = a_session(&scene, Some("echo hi"));

        assert_eq!(session, Ok(Session::RemoteExit { status: 0 }));
        assert_eq!(
            scene.devpod_commands(),
            vec![vec![
                "ssh".to_owned(),
                "myws".to_owned(),
                "--command".to_owned(),
                "bash -lc 'echo hi'".to_owned(),
            ]]
        );
    }

    #[test]
    fn a_workdir_becomes_a_devpod_flag_and_an_empty_one_becomes_nothing() {
        // devpod falls back to $HOME when given a path that does not exist in the
        // container, so an empty workdir must not become `--workdir ''`.
        let scene = Scene::new().with_running("myws");
        let token = HostToken::new();

        for (workdir, expected) in [
            (
                Some("/workspaces/myws"),
                vec![
                    "ssh".to_owned(),
                    "myws".to_owned(),
                    "--workdir".to_owned(),
                    "/workspaces/myws".to_owned(),
                ],
            ),
            (Some(""), vec!["ssh".to_owned(), "myws".to_owned()]),
            (None, vec!["ssh".to_owned(), "myws".to_owned()]),
        ] {
            scene.runner.forget_calls();

            let context = SessionContext::new(&scene.runner, &scene.host, &token);
            let _ = workspace_ssh(
                &context,
                "myws",
                None,
                workdir,
                &mut nowhere,
                &mut no_notices(),
            );

            assert_eq!(scene.devpod_commands(), vec![expected], "{workdir:?}");
        }
    }

    #[test]
    fn the_openssh_transport_carries_the_same_payload_under_a_pty() {
        // Both transports must deliver the same command to the same shell.
        let scene = Scene::new().on_a_terminal(&["myws"]).with_running("myws");

        let (session, _, _) = a_session(&scene, Some("claude"));

        assert_eq!(
            session,
            Ok(Session::Terminal {
                exit: Exit::Code(0)
            })
        );
        assert_eq!(
            scene.runner.argvs(),
            vec![vec![
                "ssh".to_owned(),
                "-t".to_owned(),
                "myws.devpod".to_owned(),
                "bash -lc claude".to_owned(),
            ]]
        );
    }

    #[test]
    fn the_session_reports_the_remote_programs_status_and_not_devpods() {
        // devpod exits 1 next to every remote status, so 1 is never the answer.
        let scene = Scene::new().with_running("myws");
        scene.runner.script(
            ["devpod", "ssh"],
            Response::failed(
                1,
                "20:41:27 fatal tunnel to container: run in container: \
                 ssh session: Process exited with status 130\n",
            ),
        );

        let (session, _notices, said) = a_session(&scene, None);

        assert_eq!(session, Ok(Session::RemoteExit { status: 130 }));
        assert_eq!(session.expect("a session").exit_status(), 130);
        assert_eq!(
            said,
            Vec::<String>::new(),
            "nothing has gone wrong, so nothing reaches the user"
        );
    }

    #[test]
    fn a_devpod_that_really_failed_reports_its_own_ending_and_its_words() {
        let scene = Scene::new().with_running("myws");
        scene.runner.script(
            ["devpod", "ssh"],
            Response::failed(
                1,
                "20:41:27 fatal tunnel to container: connection refused\n",
            ),
        );

        let (session, _notices, said) = a_session(&scene, None);

        assert_eq!(
            session,
            Ok(Session::DevpodFailed {
                exit: Exit::Code(1)
            })
        );
        assert_eq!(
            said,
            vec!["20:41:27 fatal tunnel to container: connection refused".to_owned()],
            "devpod's own words reach the user as the session runs"
        );
        assert!(_notices.contains(&LaunchNotice::DevpodSessionFailed {
            exit: Exit::Code(1)
        }));
    }

    #[test]
    fn a_failing_command_in_the_workspace_fails_dl_too() {
        for code in [0, 1, 42, 130] {
            let scene = Scene::new().on_a_terminal(&["myws"]).with_running("myws");
            scene.runner.script(["ssh"], Response::exited(code));

            let (session, _, _) = a_session(&scene, Some("false"));

            assert_eq!(
                session.expect("a session").exit_status(),
                code,
                "exit {code}"
            );
        }
    }

    #[test]
    fn an_openssh_that_is_not_installed_is_its_own_refusal() {
        // Not devpod's: telling someone to install devpod when devpod is present
        // and working sends them the wrong way.
        let scene = Scene::new().on_a_terminal(&["myws"]).with_running("myws");
        scene.runner.script_missing("ssh");

        let (session, _, _) = a_session(&scene, Some("claude"));

        assert_eq!(session, Err(SessionRefused::Ssh(ssh::NotRun::NotInstalled)));
    }

    #[test]
    fn the_argv_of_the_session_is_reported_before_it_starts() {
        let scene = Scene::new().with_running("myws");

        let (_, notices, _) = a_session(&scene, Some("echo hi"));

        assert!(notices.contains(&LaunchNotice::SshCommand {
            argv: vec![
                "devpod".to_owned(),
                "ssh".to_owned(),
                "myws".to_owned(),
                "--command".to_owned(),
                "bash -lc 'echo hi'".to_owned(),
            ]
        }));
    }

    // -------------------------------------------------- the token forwarding

    /// A host logged in through an exported `GH_TOKEN`, which costs no subprocess.
    fn logged_in(scene: Scene) -> Scene {
        let mut scene = scene;
        scene.host.gh = gh::HostEnv {
            disable: None,
            gh_token: Some("gho_secretvalue".to_owned()),
            github_token: None,
        };
        scene
    }

    #[test]
    fn the_up_carries_the_token_in_a_private_file_and_never_in_argv() {
        // `devpod up` runs for minutes while an image builds, and its argv is
        // readable by every user on the host for that whole time.
        let scene = logged_in(Scene::new());
        let mut context = CommandContext::new(&scene.runner);
        let token = HostToken::new();
        let request = UpRequest::new(
            "myws",
            Naming::Create {
                workspace_id: "myws",
            },
        );

        let outcome = workspace_up(
            &mut context,
            &scene.host,
            &token,
            &NoProvisioning,
            &request,
            &mut no_notices(),
        );

        assert_eq!(outcome, Ok(UpOutcome::Started));
        let up = scene
            .devpod_commands()
            .into_iter()
            .find(|argv| argv.first().map(String::as_str) == Some("up"))
            .expect("an up");
        let named = up
            .iter()
            .position(|arg| arg == "--workspace-env-file")
            .expect("the flag");
        assert_eq!(named, up.len() - 2, "and it is appended last: {up:?}");
        assert!(
            !up.iter().any(|arg| arg.contains("gho_secretvalue")),
            "{up:?}"
        );
    }

    #[test]
    fn the_staged_token_file_is_gone_once_the_up_is_over() {
        let scene = logged_in(Scene::new());
        let mut context = CommandContext::new(&scene.runner);
        let token = HostToken::new();

        let _ = workspace_up(
            &mut context,
            &scene.host,
            &token,
            &NoProvisioning,
            &UpRequest::new(
                "myws",
                Naming::Create {
                    workspace_id: "myws",
                },
            ),
            &mut no_notices(),
        );

        let up = scene
            .devpod_commands()
            .into_iter()
            .find(|argv| argv.first().map(String::as_str) == Some("up"))
            .expect("an up");
        let path = up.last().expect("the file's path");
        assert!(!Path::new(path).exists(), "{path} outlived the up");
    }

    #[test]
    fn the_session_names_the_variable_and_carries_the_value_in_the_environment() {
        // ps must never show the token, so only the variable name is an argument.
        let scene = logged_in(Scene::new().with_running("myws"));

        let _ = a_session(&scene, None);

        let call = scene
            .runner
            .calls_to("devpod")
            .into_iter()
            .find(|call| call.args().first().map(String::as_str) == Some("ssh"))
            .expect("a session");
        assert!(call.argv().contains(&"--send-env".to_owned()), "{call:?}");
        assert!(call.argv().contains(&"GH_TOKEN".to_owned()), "{call:?}");
        assert!(
            !call
                .argv()
                .iter()
                .any(|arg| arg.contains("gho_secretvalue")),
            "{call:?}"
        );
        assert_eq!(
            call.invocation()
                .env
                .entries
                .get("GH_TOKEN")
                .map(String::as_str),
            Some("gho_secretvalue")
        );
    }

    #[test]
    fn the_openssh_transport_forwards_the_same_token_by_name() {
        let scene = logged_in(Scene::new().on_a_terminal(&["myws"]).with_running("myws"));

        let _ = a_session(&scene, Some("claude"));

        let argv = scene
            .runner
            .argvs()
            .into_iter()
            .next()
            .expect("an ssh call");
        assert!(argv.contains(&"SendEnv=GH_TOKEN".to_owned()), "{argv:?}");
        assert!(!argv.iter().any(|arg| arg.contains("gho_secretvalue")));
    }

    #[test]
    fn no_token_means_no_forwarding_flags_at_all() {
        // The default scene has forwarding opted out.
        let scene = Scene::new().with_running("myws");

        let _ = a_session(&scene, None);

        assert_eq!(
            scene.devpod_commands(),
            vec![vec!["ssh".to_owned(), "myws".to_owned()]]
        );
    }

    #[test]
    fn gh_is_only_asked_once_per_launch_and_reports_once() {
        // Asking twice can mean unlocking a keyring twice in one `dl` run — and
        // warning twice about one host that has no login.
        let scene = Scene::new();
        let host = gh::HostEnv::default();
        scene.runner.script(
            ["gh", "auth", "token"],
            Response::failed(1, "not logged in\n"),
        );
        let token = HostToken::new();
        let mut notices = no_notices();

        assert_eq!(token.token(&scene.runner, &host, &mut notices), None);
        assert_eq!(token.token(&scene.runner, &host, &mut notices), None);

        assert_eq!(scene.runner.args_to("gh").len(), 1, "asked once");
        assert_eq!(
            notices,
            vec![LaunchNotice::NoGitHubToken(GhEvent::Refused {
                exit: Exit::Code(1)
            })],
            "and reported once"
        );
    }

    #[test]
    fn a_launch_that_forwards_the_token_twice_asks_gh_once() {
        // One run hands the token to both `devpod up` and `devpod ssh`.
        let scene = Scene::new();
        let mut logged_in = scene;
        logged_in.host.gh = gh::HostEnv::default();
        logged_in.runner.script(
            ["gh", "auth", "token"],
            Response::stdout(format!("gho_{}\n", "a".repeat(36))),
        );
        let token = HostToken::new();
        let mut notices = no_notices();
        let mut context = CommandContext::new(&logged_in.runner);

        let _ = workspace_up(
            &mut context,
            &logged_in.host,
            &token,
            &NoProvisioning,
            &UpRequest::new(
                "myws",
                Naming::Create {
                    workspace_id: "myws",
                },
            ),
            &mut notices,
        );
        let _ = attaching(&logged_in, &token, "myws", None, &mut notices);

        assert_eq!(logged_in.runner.args_to("gh").len(), 1);
        assert_eq!(
            notices
                .iter()
                .filter(|notice| matches!(notice, LaunchNotice::NoGitHubToken(_)))
                .count(),
            0
        );
    }

    #[test]
    fn a_token_the_host_exported_is_worth_no_subprocess() {
        let scene = logged_in(Scene::new());
        let token = HostToken::new();

        assert!(
            token
                .token(&scene.runner, &scene.host.gh, &mut no_notices())
                .is_some()
        );
        assert_eq!(scene.runner.args_to("gh").len(), 0);
    }

    // ---------------------------------------------------- the dotfiles refresh

    #[test]
    fn the_refresh_gate_is_off_unless_it_is_switched_on() {
        // devpod applies dotfiles when it *provisions* a workspace, and closing
        // that gap costs a round trip in front of every shell — so the people who
        // want it say so (devlaunch#183).
        assert_eq!(
            DotfilesRefresh::from_host(&Host::default()),
            DotfilesRefresh::Off
        );
        for denial in ["", "0", "false", "no", "FALSE", " no "] {
            let host = Host {
                dotfiles_on_attach: Some(denial.to_owned()),
                ..Host::default()
            };
            assert_eq!(
                DotfilesRefresh::from_host(&host),
                DotfilesRefresh::Off,
                "{denial:?}"
            );
        }
        let host = Host {
            dotfiles_on_attach: Some("1".to_owned()),
            ..Host::default()
        };
        assert_eq!(
            DotfilesRefresh::from_host(&host),
            DotfilesRefresh::Requested
        );
    }

    #[test]
    fn the_refresh_command_updates_chezmoi_and_syncs_pixi() {
        let command = dotfiles_command(Some("https://example/dots"), None);

        assert!(command.contains("chezmoi update --force"), "{command}");
        assert!(command.contains("pixi global sync"), "{command}");
        // Bare, not quoted: every character of the URL is in `shlex.quote`'s safe
        // set, so Python leaves it alone too.
        assert!(
            command.contains("git clone https://example/dots \"$DOTFILES_DIR\""),
            "the fallback clones the configured remote: {command}"
        );
    }

    #[test]
    fn a_refresh_with_no_dotfiles_url_says_so_and_exits_one() {
        let command = dotfiles_command(None, None);

        assert!(
            command.contains("chezmoi not found and no DOTFILES_URL configured"),
            "{command}"
        );
        assert!(command.contains("exit 1"), "{command}");
    }

    #[test]
    fn the_automatic_refresh_carries_its_own_deadline_into_the_container() {
        // An unreachable dotfiles remote must hand the shell over rather than sit
        // in front of it, and nothing on dl's side bounds a `chezmoi update`
        // blocked on a network read. The bound is spent inside the container so
        // the process actually waiting dies with the shell that started it.
        let bounded = dotfiles_command(None, Some(DOTFILES_ATTACH_TIMEOUT));

        assert!(bounded.starts_with("timeout 60 bash -c "), "{bounded}");
        assert!(DOTFILES_ATTACH_TIMEOUT > Duration::ZERO);
    }

    #[test]
    fn a_typed_refresh_is_left_unbounded() {
        // `dl <ws> dotfiles` is typed, in the foreground, interruptible, and
        // sometimes a legitimately slow first `pixi global sync`.
        let unbounded = dotfiles_command(None, None);

        assert!(!unbounded.contains("timeout "), "{unbounded}");
    }

    #[test]
    fn an_opted_in_interactive_attach_refreshes_before_the_shell() {
        // The order is the feature: the shell being handed over is the one that
        // reads the new dotfiles.
        let mut scene = Scene::new().with_running("myws");
        scene.host.dotfiles_on_attach = Some("1".to_owned());
        let token = HostToken::new();

        let session = attaching(&scene, &token, "myws", None, &mut no_notices());

        assert_eq!(session, Ok(Session::RemoteExit { status: 0 }));
        let commands = scene.devpod_commands();
        assert_eq!(
            scene.devpod_heads(),
            vec![
                vec!["context".to_owned(), "options".to_owned()],
                vec!["ssh".to_owned(), "myws".to_owned()],
                vec!["ssh".to_owned(), "myws".to_owned()],
            ]
        );
        assert!(commands[1][3].contains("chezmoi update"), "{commands:?}");
        assert_eq!(commands[2], vec!["ssh".to_owned(), "myws".to_owned()]);
    }

    #[test]
    fn a_one_shot_command_is_not_worth_a_refresh() {
        // The command renders no prompt and sources no interactive shell, so a
        // refresh in front of it buys it nothing and costs it a round trip. This
        // is the shape wayfinder hands dl for every agent launch.
        let mut scene = Scene::new().with_running("myws");
        scene.host.dotfiles_on_attach = Some("1".to_owned());
        let token = HostToken::new();

        let _ = attaching(&scene, &token, "myws", Some("echo hi"), &mut no_notices());

        assert_eq!(
            scene.devpod_commands(),
            vec![vec![
                "ssh".to_owned(),
                "myws".to_owned(),
                "--command".to_owned(),
                "bash -lc 'echo hi'".to_owned(),
            ]]
        );
    }

    #[test]
    fn an_attach_with_the_switch_off_spawns_nothing_but_the_session() {
        let scene = Scene::new().with_running("myws");
        let token = HostToken::new();

        for command in [None, Some("echo hi")] {
            scene.runner.forget_calls();

            let _ = attaching(&scene, &token, "myws", command, &mut no_notices());

            assert_eq!(scene.devpod_commands().len(), 1, "{command:?}");
        }
    }

    // ------------------------------------------------- stage one: the plan

    #[test]
    fn a_git_triple_is_planned_with_its_remote() {
        assert_eq!(
            plan("blooop/devlaunch@wayfinder/devlaunch-7"),
            Ok(Plan::Triple {
                owner: "blooop".to_owned(),
                repo: "devlaunch".to_owned(),
                branch: Some("wayfinder/devlaunch-7".to_owned()),
                remote_url: "git@github.com:blooop/devlaunch.git".to_owned(),
            })
        );
        assert_eq!(
            plan("blooop/devlaunch"),
            Ok(Plan::Triple {
                owner: "blooop".to_owned(),
                repo: "devlaunch".to_owned(),
                branch: None,
                remote_url: "git@github.com:blooop/devlaunch.git".to_owned(),
            })
        );
    }

    #[test]
    fn an_owner_or_repo_that_would_traverse_the_cache_is_refused_before_anything_is_locked() {
        // `ensure_repo` joins repos_dir/<owner>/<repo> and would otherwise act on
        // a traversal first and reject it after: `x/..` resolves to repos_dir
        // itself and `../x` leaves it entirely.
        assert_eq!(
            plan("x/.."),
            Err(UnsafeName {
                part: NamePart::Repo,
                name: "..".to_owned()
            })
        );
    }

    #[test]
    fn a_bare_name_is_planned_as_a_workspace_devpod_already_has() {
        // Everything creatable is a path or a git spec and matched first.
        assert_eq!(
            plan("devlaunch-main-abcdefgh"),
            Ok(Plan::Existing {
                name: "devlaunch-main-abcdefgh".to_owned()
            })
        );
    }

    #[test]
    fn a_url_is_planned_as_something_devpod_clones_itself() {
        let planned = plan("https://github.com/blooop/devlaunch").expect("a plan");

        match planned {
            Plan::Creatable {
                source,
                workspace_id,
            } => {
                assert_eq!(source, "https://github.com/blooop/devlaunch");
                assert!(!workspace_id.is_empty());
            }
            other => panic!("expected a creatable plan, got {other:?}"),
        }
    }

    #[test]
    fn a_path_spec_reaches_devpod_as_written_and_is_named_after_its_directory() {
        let planned = plan("/tmp/./a-project").expect("a plan");

        assert_eq!(
            planned,
            Plan::Creatable {
                source: "/tmp/./a-project".to_owned(),
                workspace_id: "a-project".to_owned(),
            }
        );
    }

    // -------------------------------- stage two: which workspace, and is it warm

    #[test]
    fn a_warm_named_branch_launch_reads_no_metadata_at_all() {
        // devlaunch#145: building the clone manager reads config.toml, loads
        // metadata.json under its lock and runs the migration. A launch that
        // attaches to a workspace devpod already reports as Running uses none of
        // that, so it must pay for none of it — and here that is stated by the
        // cold path never being opened.
        let workspace = WorkspaceId::new("blooop", "devlaunch", "wayfinder/devlaunch-7")
            .expect("a safe triple");
        let scene = Scene::new().with_running(&workspace.value());
        let mut context = CommandContext::new(&scene.runner);

        let resolution =
            resolve_triple(&mut context, &mut NeverCold, &workspace, &mut no_notices());

        assert_eq!(
            resolution,
            Ok(Resolution::Warm {
                placement: Placement::Known {
                    workspace_id: workspace.value(),
                    state: ContainerState::Running,
                }
            })
        );
        assert_eq!(
            scene.devpod_commands(),
            vec![vec![
                "status".to_owned(),
                workspace.value(),
                "--output".to_owned(),
                "json".to_owned(),
            ]]
        );
    }

    #[test]
    fn a_workspace_devpod_does_not_know_resolves_cold() {
        let workspace = WorkspaceId::new("owner", "repo", "feature/x").expect("a safe triple");
        let scene = Scene::new();
        let git = Git::new(&scene.runner);
        let mut cold = RealCold::new(scene.cache_dir(), git);
        let mut context = CommandContext::new(&scene.runner);

        let resolution = resolve_triple(&mut context, &mut cold, &workspace, &mut no_notices());

        assert_eq!(
            resolution,
            Ok(Resolution::Cold {
                workspace: workspace.clone()
            })
        );
        assert_eq!(cold.opens.get(), 1, "the record was consulted exactly once");
    }

    #[test]
    fn a_placement_answers_the_fast_attach_question_on_its_own() {
        // Python reads it off two correlated locals; here it is one value.
        let running = Placement::Known {
            workspace_id: "myws".to_owned(),
            state: ContainerState::Running,
        };
        let stopped = Placement::Known {
            workspace_id: "myws".to_owned(),
            state: ContainerState::Stopped,
        };
        let creating = Placement::Creating {
            workspace_id: "myws".to_owned(),
            source: "/clone".to_owned(),
        };
        let listed = Placement::Listed {
            workspace_id: "myws".to_owned(),
        };

        assert!(running.is_running());
        assert!(!stopped.is_running());
        assert!(!creating.is_running(), "a create is never warm");
        assert!(
            !listed.is_running(),
            "and neither is a workspace devpod would not describe"
        );
        assert!(matches!(listed.naming(), Naming::Known { .. }));
        assert_eq!(
            running.source(),
            "myws",
            "a known workspace is its own source"
        );
        assert_eq!(creating.source(), "/clone");
        assert!(matches!(running.naming(), Naming::Known { .. }));
        assert!(matches!(creating.naming(), Naming::Create { .. }));
    }

    // ------------------------------------------------ the launch, end to end

    /// Everything a launch needs, over one scene.
    struct Launching<'a> {
        context: CommandContext<'a>,
        refresh: Refresh<'a>,
        provision: RecordingProvision,
        /// Where devpod's session chatter goes. See [`nowhere`].
        chatter: fn(&str),
        /// What the launch said, in the order it said it. A `Vec` because a test
        /// wants the sequence; the binary's sink prints instead.
        said: Vec<LaunchNotice>,
    }

    fn launching<'a>(
        runner: &'a FakeRunner,
        updater: &'a SelfInvocation,
        cache_path: &'a Path,
    ) -> Launching<'a> {
        Launching {
            context: CommandContext::new(runner),
            refresh: Refresh::new(updater, cache_path),
            provision: RecordingProvision::default(),
            chatter: nowhere,
            said: Vec::new(),
        }
    }

    #[test]
    fn a_warm_git_spec_one_shot_is_one_status_and_one_ssh() {
        // The launcher path: `dl owner/repo@branch -- cmd`, workspace warm. This
        // is the exact shape wayfinder hands dl for every agent launch, so its
        // overhead is what sits between picking a ticket and a running agent.
        let workspace = WorkspaceId::new("blooop", "devlaunch", "wayfinder/devlaunch-7")
            .expect("a safe triple");
        let scene = Scene::new().with_running(&workspace.value());
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let mut launch = Launch::new(
            &mut parts.context,
            &mut parts.refresh,
            &mut cold,
            &parts.provision,
            &scene.host,
            &mut parts.chatter,
            &mut parts.said,
        );

        let launched = launch.run(
            "blooop/devlaunch@wayfinder/devlaunch-7",
            &LaunchVerb::Attach {
                command: Some("echo hi".to_owned()),
            },
            None,
        );

        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 }))
        );
        assert_eq!(
            scene.devpod_commands(),
            vec![
                vec![
                    "status".to_owned(),
                    workspace.value(),
                    "--output".to_owned(),
                    "json".to_owned(),
                ],
                vec![
                    "ssh".to_owned(),
                    workspace.value(),
                    "--command".to_owned(),
                    "bash -lc 'echo hi'".to_owned(),
                ],
            ]
        );
    }

    #[test]
    fn a_warm_bare_name_attach_is_one_status_and_one_ssh() {
        let scene = Scene::new().with_running("myws");
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let mut launch = Launch::new(
            &mut parts.context,
            &mut parts.refresh,
            &mut cold,
            &parts.provision,
            &scene.host,
            &mut parts.chatter,
            &mut parts.said,
        );

        let launched = launch.run("myws", &LaunchVerb::Attach { command: None }, None);

        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 }))
        );
        assert_eq!(
            scene.devpod_commands(),
            vec![
                vec![
                    "status".to_owned(),
                    "myws".to_owned(),
                    "--output".to_owned(),
                    "json".to_owned(),
                ],
                vec!["ssh".to_owned(), "myws".to_owned()],
            ]
        );
        drop(launch);
        assert!(parts.said.contains(&LaunchNotice::AlreadyRunningAttaching {
            workspace_id: "myws".to_owned()
        }));
    }

    #[test]
    fn an_unknown_bare_name_is_refused_after_asking_devpod_twice() {
        // `status` consults the provider while `list` reads devpod's own records,
        // so refusing on the status alone would be a wrong diagnosis.
        let scene = Scene::new();
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let mut launch = Launch::new(
            &mut parts.context,
            &mut parts.refresh,
            &mut cold,
            &parts.provision,
            &scene.host,
            &mut parts.chatter,
            &mut parts.said,
        );

        let launched = launch.run("no-such-ws", &LaunchVerb::Attach { command: None }, None);

        assert_eq!(
            launched,
            Ok(Launched::Refused(LaunchRefusal::UnknownWorkspace {
                name: "no-such-ws".to_owned()
            }))
        );
        assert_eq!(
            scene.devpod_heads(),
            vec![
                vec!["status".to_owned(), "no-such-ws".to_owned()],
                vec!["list".to_owned(), "--output".to_owned()],
            ]
        );
    }

    #[test]
    fn a_bare_name_devpod_lists_but_cannot_describe_is_still_usable() {
        // This workspace exists, and reaching for it is the whole point: a
        // provider that is broken, reconfigured or gone still lists.
        let scene = Scene::new();
        scene
            .runner
            .add_workspace("broken-ws", WorkspaceState::Stopped);
        scene.runner.script(
            ["devpod", "status", "broken-ws"],
            Response::failed(1, "provider gone\n"),
        );
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let mut launch = Launch::new(
            &mut parts.context,
            &mut parts.refresh,
            &mut cold,
            &parts.provision,
            &scene.host,
            &mut parts.chatter,
            &mut parts.said,
        );

        let launched = launch.run("broken-ws", &LaunchVerb::Attach { command: None }, None);

        // Not a refusal: the launch goes on to `up` it, because devpod knows it —
        // and the placement says devpod described nothing rather than claiming a
        // state devpod never gave.
        assert!(
            !matches!(launched, Ok(Launched::Refused(_))),
            "{launched:?}"
        );
        assert!(
            scene
                .devpod_commands()
                .iter()
                .any(|argv| argv.first().map(String::as_str) == Some("up")),
            "{:?}",
            scene.devpod_commands()
        );
    }

    #[test]
    fn a_devcontainer_choice_a_running_workspace_cannot_honour_is_named() {
        let scene = Scene::new().with_running("myws");
        let variant = spec::resolve_devcontainer_ref("robot").expect("a variant");
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let mut launch = Launch::new(
            &mut parts.context,
            &mut parts.refresh,
            &mut cold,
            &parts.provision,
            &scene.host,
            &mut parts.chatter,
            &mut parts.said,
        );

        let _ = launch.run(
            "myws",
            &LaunchVerb::Attach { command: None },
            Some(&variant),
        );

        drop(launch);
        assert!(
            parts
                .said
                .contains(&LaunchNotice::DevcontainerIgnoredRunning {
                    workspace_id: "myws".to_owned(),
                    spec: "myws".to_owned(),
                }),
            "{:?}",
            parts.said
        );
    }

    #[test]
    fn the_up_verb_on_a_running_workspace_still_tops_up_the_tools() {
        // `up` is one of the two verbs named as how a workspace that missed
        // provisioning gets it, so returning here without them would make the
        // documented recovery the one path that cannot recover.
        let scene = Scene::new().with_running("myws");
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let launched = {
            let mut launch = Launch::new(
                &mut parts.context,
                &mut parts.refresh,
                &mut cold,
                &parts.provision,
                &scene.host,
                &mut parts.chatter,
                &mut parts.said,
            );
            launch.run("myws", &LaunchVerb::Up, None)
        };

        assert_eq!(launched, Ok(Launched::AlreadyRunning));
        assert_eq!(parts.provision.provisioned(), vec!["myws".to_owned()]);
        assert_eq!(
            scene.devpod_heads(),
            vec![vec!["status".to_owned(), "myws".to_owned()]],
            "and no up at all"
        );
    }

    #[test]
    fn a_devpod_that_went_missing_during_provisioning_ends_the_launch() {
        // Python's `DevpodNotInstalled` is deliberately not an `OSError`, so it
        // travels out of the launch and `main()` renders exit 127 for it — no
        // session, no attach. The trait answers it for that reason, and this is the
        // arm that carries it: the launch aborts, and the binary has no bookkeeping
        // of its own to reconstruct the number from.
        for (verb, running) in [
            (LaunchVerb::Up, true),
            (LaunchVerb::Attach { command: None }, false),
        ] {
            let scene = if running {
                Scene::new().with_running("myws")
            } else {
                Scene::new().with_stopped("myws")
            };
            let updater = SelfInvocation::new("dl");
            let completion = scene.cache_dir().join("completion.json");
            let mut parts = launching(&scene.runner, &updater, &completion);
            parts.provision.lost_devpod = true;
            let mut cold = NeverCold;
            let launched = {
                let mut launch = Launch::new(
                    &mut parts.context,
                    &mut parts.refresh,
                    &mut cold,
                    &parts.provision,
                    &scene.host,
                    &mut parts.chatter,
                    &mut parts.said,
                );
                launch.run("myws", &verb, None)
            };

            assert_eq!(
                launched,
                Err(LaunchAborted::DevpodNotRun(NotRun::NotInstalled)),
                "{verb:?}"
            );
            assert!(
                !scene
                    .devpod_heads()
                    .iter()
                    .any(|argv| argv.first().map(String::as_str) == Some("ssh")),
                "a launch that lost devpod still attached: {:?}",
                scene.devpod_heads()
            );
        }
    }

    #[test]
    fn the_code_verb_opens_the_ide_and_does_not_attach() {
        let scene = Scene::new().with_stopped("myws");
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let mut launch = Launch::new(
            &mut parts.context,
            &mut parts.refresh,
            &mut cold,
            &parts.provision,
            &scene.host,
            &mut parts.chatter,
            &mut parts.said,
        );

        let launched = launch.run("myws", &LaunchVerb::Code, None);

        assert_eq!(launched, Ok(Launched::Ready));
        let up = scene
            .devpod_commands()
            .into_iter()
            .find(|argv| argv.first().map(String::as_str) == Some("up"))
            .expect("an up");
        assert!(
            up.windows(2)
                .any(|pair| pair == ["--ide".to_owned(), "vscode".to_owned()]),
            "{up:?}"
        );
        assert!(
            !scene
                .devpod_commands()
                .iter()
                .any(|argv| argv.first().map(String::as_str) == Some("ssh")),
            "code does not attach"
        );
    }

    #[test]
    fn the_restart_verb_stops_then_starts_then_attaches() {
        let scene = Scene::new().with_running("myws");
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let mut launch = Launch::new(
            &mut parts.context,
            &mut parts.refresh,
            &mut cold,
            &parts.provision,
            &scene.host,
            &mut parts.chatter,
            &mut parts.said,
        );

        let launched = launch.run("myws", &LaunchVerb::Restart, None);

        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 }))
        );
        let verbs: Vec<String> = scene
            .devpod_commands()
            .into_iter()
            .filter_map(|argv| argv.into_iter().next())
            .filter(|verb| verb != "context")
            .collect();
        assert_eq!(
            verbs,
            vec![
                "status".to_owned(),
                "stop".to_owned(),
                "up".to_owned(),
                "ssh".to_owned()
            ]
        );
    }

    #[test]
    fn a_recreate_rebuilds_and_then_attaches() {
        let scene = Scene::new().with_running("myws");
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let mut launch = Launch::new(
            &mut parts.context,
            &mut parts.refresh,
            &mut cold,
            &parts.provision,
            &scene.host,
            &mut parts.chatter,
            &mut parts.said,
        );

        let launched = launch.run("myws", &LaunchVerb::Recreate, None);

        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 }))
        );
        let up = scene
            .devpod_commands()
            .into_iter()
            .find(|argv| argv.first().map(String::as_str) == Some("up"))
            .expect("an up");
        assert!(up.contains(&"--recreate".to_owned()), "{up:?}");
    }

    #[test]
    fn a_reset_asks_for_a_clean_slate_and_then_attaches() {
        let scene = Scene::new().with_running("myws");
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let mut launch = Launch::new(
            &mut parts.context,
            &mut parts.refresh,
            &mut cold,
            &parts.provision,
            &scene.host,
            &mut parts.chatter,
            &mut parts.said,
        );

        let launched = launch.run("myws", &LaunchVerb::Reset, None);

        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 }))
        );
        let up = scene
            .devpod_commands()
            .into_iter()
            .find(|argv| argv.first().map(String::as_str) == Some("up"))
            .expect("an up");
        assert!(up.contains(&"--reset".to_owned()), "{up:?}");
    }

    #[test]
    fn a_refused_up_warms_the_cache_for_up_and_code_and_for_nothing_else() {
        // dl.py 4779/4788: those two ask for the refresh *before* they read the
        // return code, so a `devpod up` that failed still warms the cache; every
        // other verb returns on the failure first. Where the ask happens is control
        // flow rather than rendering, so it is pinned here rather than at the
        // binary — the observable difference is a background child.
        for (verb, warms) in [
            (LaunchVerb::Up, true),
            (LaunchVerb::Code, true),
            (LaunchVerb::Attach { command: None }, false),
            (LaunchVerb::Recreate, false),
            (LaunchVerb::Reset, false),
        ] {
            let scene = Scene::new().with_stopped("myws");
            scene
                .runner
                .script(["devpod", "up"], Response::failed(7, "devpod: no\n"));
            let updater = SelfInvocation::new("dl");
            let completion = scene.cache_dir().join("completion.json");
            let mut parts = launching(&scene.runner, &updater, &completion);
            let mut cold = NeverCold;
            let launched = {
                let mut launch = Launch::new(
                    &mut parts.context,
                    &mut parts.refresh,
                    &mut cold,
                    &parts.provision,
                    &scene.host,
                    &mut parts.chatter,
                    &mut parts.said,
                );
                launch.run("myws", &verb, None)
            };

            assert_eq!(
                launched,
                Ok(Launched::Refused(LaunchRefusal::UpRefused {
                    exit: Exit::Code(7)
                })),
                "{verb:?}"
            );
            let refreshes = scene
                .runner
                .args_to("dl")
                .into_iter()
                .filter(|args| args.first().map(String::as_str) == Some("--update-cache"))
                .count();
            assert_eq!(refreshes, usize::from(warms), "{verb:?}");
        }
    }

    #[test]
    fn every_workspace_verb_forces_exactly_one_refresh_afterwards() {
        // Workspace commands change what the cache describes, so they force one
        // refresh — after the command, not before, and never more than one.
        // `restart` used to spawn twice: once up front and once from the stop.
        for verb in [
            LaunchVerb::Attach { command: None },
            LaunchVerb::Up,
            LaunchVerb::Code,
            LaunchVerb::Recreate,
            LaunchVerb::Reset,
            LaunchVerb::Restart,
        ] {
            let scene = Scene::new().with_stopped("myws");
            let updater = SelfInvocation::new("dl");
            let completion = scene.cache_dir().join("completion.json");
            let mut parts = launching(&scene.runner, &updater, &completion);
            let mut cold = NeverCold;
            {
                let mut launch = Launch::new(
                    &mut parts.context,
                    &mut parts.refresh,
                    &mut cold,
                    &parts.provision,
                    &scene.host,
                    &mut parts.chatter,
                    &mut parts.said,
                );
                let launched = launch.run("myws", &verb, None);
                assert!(launched.is_ok(), "{verb:?}: {launched:?}");
            }

            let refreshes: Vec<Vec<String>> = scene
                .runner
                .args_to("dl")
                .into_iter()
                .filter(|args| args.first().map(String::as_str) == Some("--update-cache"))
                .collect();
            assert_eq!(
                refreshes,
                vec![vec!["--update-cache".to_owned(), "--force".to_owned()]],
                "{verb:?}"
            );
        }
    }

    #[test]
    fn a_refused_spec_spawns_no_refresh_at_all() {
        // A spec dl refuses to act on changed nothing worth re-indexing.
        let scene = Scene::new();
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        {
            let mut launch = Launch::new(
                &mut parts.context,
                &mut parts.refresh,
                &mut cold,
                &parts.provision,
                &scene.host,
                &mut parts.chatter,
                &mut parts.said,
            );
            let _ = launch.run("nonexistent", &LaunchVerb::Attach { command: None }, None);
        }

        assert_eq!(scene.runner.args_to("dl"), Vec::<Vec<String>>::new());
    }

    // --------------------------------------------- how many repo-lock cycles

    /// Every repo-lock file the launch created under `repos_dir`.
    ///
    /// Counted by path rather than by call, because dl takes three kinds of lock
    /// and only this one is being counted: the metadata lock is
    /// `metadata.json.lock` and the launch lock is `<id>.lock` under
    /// `launch-locks/`. The lock files are never unlinked, so their presence is
    /// the record that the repo lock was taken at all — and for the counts that
    /// matter here (0 versus more than 0) that is the whole question.
    fn repo_lock_files(repos_dir: &Path) -> Vec<PathBuf> {
        fn walk(at: &Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(at) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.file_name().is_some_and(|name| name == ".lock") {
                    found.push(path);
                }
            }
        }
        let mut found = Vec::new();
        walk(repos_dir, &mut found);
        found
    }

    #[test]
    fn a_warm_named_branch_launch_never_touches_the_repo_lock() {
        // The fast path pinned from the lock side: a workspace devpod already
        // knows needs no clone, so it must not touch the repo lock — which is
        // also what keeps it from queueing behind a sibling's cold launch of the
        // same repo.
        let workspace = WorkspaceId::new("owner", "repo", "feature/x").expect("a safe triple");
        let scene = Scene::new().with_running(&workspace.value());
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        {
            let mut launch = Launch::new(
                &mut parts.context,
                &mut parts.refresh,
                &mut cold,
                &parts.provision,
                &scene.host,
                &mut parts.chatter,
                &mut parts.said,
            );
            let launched = launch.run(
                "owner/repo@feature/x",
                &LaunchVerb::Attach {
                    command: Some("echo hi".to_owned()),
                },
                None,
            );
            assert!(launched.is_ok(), "{launched:?}");
        }

        assert_eq!(
            repo_lock_files(&scene.cache_dir().join("repos")),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn a_warm_bare_spec_launch_takes_the_repo_lock_only_to_name_the_default_branch() {
        // One extra cycle, and it is deliberate: folding it in means holding the
        // repo lock across the fast-attach `devpod status`, which every sibling
        // launch of this repo would then queue behind (devlaunch#200).
        let scene = Scene::new();
        let workspace = WorkspaceId::new("owner", "repo", "main").expect("a safe triple");
        scene
            .runner
            .add_workspace(&workspace.value(), WorkspaceState::Running);
        // A bare clone with a HEAD, as a real `git clone --bare` leaves it.
        scene.runner.script(
            ["git", "symbolic-ref"],
            Response::stdout("refs/heads/main\n"),
        );
        let git = Git::new(&scene.runner);
        let mut cold = RealCold::new(scene.cache_dir(), git);
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        {
            let mut launch = Launch::new(
                &mut parts.context,
                &mut parts.refresh,
                &mut cold,
                &parts.provision,
                &scene.host,
                &mut parts.chatter,
                &mut parts.said,
            );
            let _ = launch.run(
                "owner/repo",
                &LaunchVerb::Attach {
                    command: Some("echo hi".to_owned()),
                },
                None,
            );
        }

        assert_eq!(
            repo_lock_files(&scene.cache_dir().join("repos"))
                .into_iter()
                .map(|path| path
                    .strip_prefix(scene.cache_dir())
                    .map(Path::to_path_buf)
                    .ok())
                .collect::<Vec<_>>(),
            vec![Some(PathBuf::from("repos/owner/repo/.lock"))]
        );
    }

    // ------------------------------------------------------------- timing
    //
    // No lock of their own: every one of these holds a [`Scene`], which is what
    // keeps the process-global registry to one test at a time.

    fn measured<T>(seam: timing::Seam, test: impl FnOnce() -> T) -> (T, timing::Document) {
        timing::install(Some(timing::Registry::start(
            timing::Mode::Document,
            seam,
            0.0,
        )));
        let outcome = test();
        let report = timing::emit().expect("a report from an installed registry");
        let document = report.document().expect("document mode").clone();
        (outcome, document)
    }

    fn stage_names(document: &timing::Document) -> Vec<&str> {
        document.stages.iter().map(|stage| stage.stage).collect()
    }

    fn span_labels<'a>(document: &'a timing::Document, stage: &str) -> Vec<&'a str> {
        document
            .stages
            .iter()
            .find(|record| record.stage == stage)
            .map(|record| {
                record
                    .spans
                    .iter()
                    .map(|span| span.label.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_warm_launch_reports_the_devpod_probe_and_the_attach_and_nothing_else() {
        // A warm launch does no host git work and lends no tools, and the two
        // stages it never reached are absent rather than zeroed.
        let scene = Scene::new().with_running("myws");
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;

        let (launched, document) = measured(timing::Seam::default(), || {
            let mut launch = Launch::new(
                &mut parts.context,
                &mut parts.refresh,
                &mut cold,
                &parts.provision,
                &scene.host,
                &mut parts.chatter,
                &mut parts.said,
            );
            launch.run("myws", &LaunchVerb::Attach { command: None }, None)
        });

        assert!(launched.is_ok(), "{launched:?}");
        assert_eq!(stage_names(&document), ["devpod-up", "attach"]);
        assert_eq!(span_labels(&document, "devpod-up"), ["devpod status"]);
        assert_eq!(span_labels(&document, "attach"), ["devpod ssh"]);
    }

    #[test]
    fn a_cold_up_charges_the_devpod_round_trips_to_the_up_stage() {
        let scene = Scene::new();
        let mut context = CommandContext::new(&scene.runner);
        let token = HostToken::new();

        let (outcome, document) = measured(timing::Seam::default(), || {
            workspace_up(
                &mut context,
                &scene.host,
                &token,
                &NoProvisioning,
                &UpRequest::new(
                    "brand-new",
                    Naming::Create {
                        workspace_id: "brand-new",
                    },
                ),
                &mut no_notices(),
            )
        });

        assert_eq!(outcome, Ok(UpOutcome::Started));
        assert_eq!(stage_names(&document), ["devpod-up"]);
        assert_eq!(
            span_labels(&document, "devpod-up"),
            ["devpod context", "devpod up"]
        );
    }

    #[test]
    fn a_launch_that_found_the_workspace_already_up_is_a_prewarm_hit() {
        let scene = Scene::new().with_running("myws");
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;
        let seam = timing::Seam {
            keystroke: None,
            prewarm_fired: Some(1.0),
        };

        let (_, document) = measured(seam, || {
            let mut launch = Launch::new(
                &mut parts.context,
                &mut parts.refresh,
                &mut cold,
                &parts.provision,
                &scene.host,
                &mut parts.chatter,
                &mut parts.said,
            );
            launch.run("myws", &LaunchVerb::Attach { command: None }, None)
        });

        assert_eq!(
            document.prewarm.as_ref().and_then(|prewarm| prewarm.shape),
            Some("hit")
        );
    }

    #[test]
    fn a_launch_that_ran_the_up_itself_is_a_prewarm_miss() {
        let scene = Scene::new();
        let mut context = CommandContext::new(&scene.runner);
        let token = HostToken::new();
        let seam = timing::Seam {
            keystroke: None,
            prewarm_fired: Some(1.0),
        };

        let (_, document) = measured(seam, || {
            workspace_up(
                &mut context,
                &scene.host,
                &token,
                &NoProvisioning,
                &UpRequest::new(
                    "brand-new",
                    Naming::Create {
                        workspace_id: "brand-new",
                    },
                ),
                &mut no_notices(),
            )
        });

        assert_eq!(
            document.prewarm.as_ref().and_then(|prewarm| prewarm.shape),
            Some("miss")
        );
    }

    #[test]
    fn a_launch_that_waited_for_its_prewarm_is_partial() {
        // The middle case: the prewarm was still running, so this launch queued
        // behind it and got a container it did not have to build — but paid the
        // wait the prewarm existed to avoid.
        let scene = Scene::new().with_running("myws");
        let provision = RecordingProvision::default();
        let request = UpRequest::new(
            "owner/repo",
            Naming::Create {
                workspace_id: "myws",
            },
        );
        let seam = timing::Seam {
            keystroke: None,
            prewarm_fired: Some(1.0),
        };

        let (outcome, document) = measured(seam, || contended_up(&scene, &request, &provision));

        assert_eq!(outcome.0, Ok(UpOutcome::SkippedSiblingWon));
        assert_eq!(
            document.prewarm.as_ref().and_then(|prewarm| prewarm.shape),
            Some("partial")
        );
    }

    #[test]
    fn a_launch_with_nothing_prewarmed_claims_no_shape() {
        // Absent, not "miss": no prewarm was fired, so there is no prewarm to
        // report the outcome of.
        let scene = Scene::new().with_running("myws");
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        let mut cold = NeverCold;

        let (_, document) = measured(timing::Seam::default(), || {
            let mut launch = Launch::new(
                &mut parts.context,
                &mut parts.refresh,
                &mut cold,
                &parts.provision,
                &scene.host,
                &mut parts.chatter,
                &mut parts.said,
            );
            launch.run("myws", &LaunchVerb::Attach { command: None }, None)
        });

        assert_eq!(document.prewarm, None);
    }

    #[test]
    fn the_openssh_transport_is_named_in_the_summary() {
        let scene = logged_in(Scene::new().on_a_terminal(&["myws"]).with_running("myws"));
        let token = HostToken::new();

        let (_, document) = measured(timing::Seam::default(), || {
            attaching(&scene, &token, "myws", Some("claude"), &mut no_notices())
        });

        assert_eq!(span_labels(&document, "attach"), ["ssh"]);
    }

    #[test]
    fn the_token_round_trip_is_host_prep_even_when_it_happens_mid_attach() {
        // Host prep is an owner, not a region of the timeline: the token trip is
        // the host's work wherever on the launch it falls, and the stage it
        // interrupts is not charged for it.
        let scene = Scene::new().with_running("myws");
        let mut logged_out = scene;
        logged_out.host.gh = gh::HostEnv::default();
        logged_out.runner.script(
            ["gh", "auth", "token"],
            Response::stdout(format!("gho_{}\n", "a".repeat(36))),
        );
        let token = HostToken::new();

        let (_, document) = measured(timing::Seam::default(), || {
            attaching(&logged_out, &token, "myws", None, &mut no_notices())
        });

        assert_eq!(span_labels(&document, "host-prep"), ["gh auth token"]);
        assert_eq!(span_labels(&document, "attach"), ["devpod ssh"]);
    }

    #[test]
    fn a_token_the_host_exported_costs_no_span_at_all() {
        // `resolve_token` answers from the environment without spawning, and
        // Python's span sits inside the branch that spawns.
        let scene = logged_in(Scene::new().with_running("myws"));
        let token = HostToken::new();

        let (_, document) = measured(timing::Seam::default(), || {
            attaching(&scene, &token, "myws", None, &mut no_notices())
        });

        assert_eq!(stage_names(&document), ["attach"]);
    }
}
