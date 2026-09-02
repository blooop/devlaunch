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
//! `recreate: bool` and `reset: bool`; `Naming` is the three shapes the first
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

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::clients::claude;
use crate::clients::devpod::{self, Call, ContainerState, ListingUnreadable, NotRun, Patience};
use crate::clients::devpod_home::{CreateRecord, DevpodHome, create_record};
use crate::clients::gh::{self, GhEvent, StagedToken, Token, TokenLookup};
use crate::clients::herdr;
use crate::clients::ssh;
use crate::domain::locks::{self, Contention, LockError};
use crate::domain::metadata::MetadataStorage;
use crate::domain::model::WorktreeInfo;
use crate::domain::spec::{self, DevcontainerPath, SpecIdentity, WorkspaceSpec};
use crate::domain::workspace_id::{
    NamePart, UnsafeName, WorkspaceId, identity_of, validate_ref_name,
};
use crate::flows::kept_copies::KeptCopies;
use crate::flows::lifecycle::{
    self, KnownWorkspace, LifecycleNotice, Refresh, RefreshReason, StopOutcome,
};
use crate::flows::listing::CommandContext;
use crate::flows::provision::verdict_cache::VerdictCache;
use crate::flows::provision::{
    self, ClaudeConfig, DevpodMissing, HostLayout, PassOccasion, ProvisionEvent, Provisioning,
    Switches, ZellijSwitch,
};
use crate::flows::records::{self, Records, RecordsNotice, StartupError};
use crate::flows::repo_manager::CacheNotice;
use crate::flows::repo_manager::EnsureRepoError;
use crate::flows::session_manager;
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

/// `DEVLAUNCH_ZELLIJ`: put `dl <spec> -- <cmd>` beside a zellij session (#242),
/// and — since #391 — put a zellij in the container for it to be beside.
///
/// Defined in [`provision`][crate::flows::provision] rather than here, because one
/// signal read in two places is one place too many: the same constant gates the
/// setup pass's zellij stage. This module is the reader that starts a session; that
/// one is the reader that makes sure there is something to start.
pub(crate) use crate::flows::provision::ZELLIJ_VAR;

/// The one session name every workspace uses.
///
/// One fixed name rather than one per workspace, because a zellij server lives
/// *inside* a container and dies with it: two workspaces cannot collide on this
/// name, since neither can see the other's sessions. That makes the name a
/// constant a human can type without looking it up, which is the whole of the
/// documented interface — `zellij -s devlaunch action new-pane -- <cmd>`.
pub(crate) const ZELLIJ_SESSION: &str = "devlaunch";

/// `DEVLAUNCH_NO_TITLE`: do not name the terminal after the workspace.
///
/// A "no" variable rather than an opt-in one, unlike [`ZELLIJ_VAR`], and the
/// asymmetry is the cost of being wrong. `DEVLAUNCH_ZELLIJ` installs a package into
/// a container and starts a session in it; this writes one escape sequence to a
/// stream that the next shell prompt overwrites anyway. Defaulting it on costs a
/// title nobody asked
/// for; defaulting it off costs everybody the feature.
pub(crate) const TITLE_DISABLE_VAR: &str = "DEVLAUNCH_NO_TITLE";

/// `HERDR_TAB_ID`: the herdr tab this process is running in, if it is.
///
/// herdr's own variable, not dl's, exported into every pane it spawns alongside
/// `HERDR_ENV`, `HERDR_PANE_ID` and `HERDR_WORKSPACE_ID`. Read rather than probed,
/// which is the whole reason this stage exists at all: [`TerminalTitle`] declines to
/// detect a multiplexer because detection costs a round trip, and this one costs an
/// environment lookup.
///
/// The tab id and not `HERDR_ENV`, because the id is what the rename is *addressed
/// to* — a pane with no tab id is a pane with nothing to name, whatever else it
/// says about itself.
pub(crate) const HERDR_TAB_VAR: &str = "HERDR_TAB_ID";

/// `HERDR_BIN_PATH`: the herdr that spawned this pane.
///
/// Preferred over a `PATH` lookup because a rename lands on the socket of the
/// server that owns this pane, and the binary that server shipped is the one that
/// speaks its protocol. It is also the only one guaranteed to be *findable*: herdr
/// installs per-environment often enough (pixi, cargo, a downloaded release) that
/// `herdr` need not be on the `PATH` a launch inherits at all.
pub(crate) const HERDR_BIN_VAR: &str = "HERDR_BIN_PATH";

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

/// `$SSH_AUTH_SOCK`, OpenSSH's own name for the agent it would forward.
///
/// Named here rather than read as a literal for [`ssh::CONFIG_VAR`]'s reason: it
/// is somebody else's spelling, and one place to look for it is the whole of what
/// keeps a rename from becoming a silently shared master.
pub(crate) const SSH_AUTH_SOCK_VAR: &str = "SSH_AUTH_SOCK";

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
    /// `DEVLAUNCH_NO_CLAUDE_TOKEN`, and any `CLAUDE_CODE_OAUTH_TOKEN` the host
    /// already exported. A host value like [`Host::gh`], read the same way.
    pub(crate) claude: claude::HostEnv,
    /// `DEVLAUNCH_DOTFILES_ON_ATTACH`.
    pub(crate) dotfiles_on_attach: Option<String>,
    /// `DEVLAUNCH_ZELLIJ`.
    pub(crate) zellij: Option<String>,
    /// `DEVLAUNCH_HERDR`, and the four coordinates herdr exports into its own
    /// panes. A host value like [`Host::gh`], read the same way.
    pub(crate) herdr: herdr::HostEnv,
    /// `DEVLAUNCH_NO_TTY`.
    pub(crate) no_tty: Option<String>,
    /// `DEVLAUNCH_NO_TITLE`.
    pub(crate) no_title: Option<String>,
    /// `HERDR_TAB_ID`: the herdr tab to name, or `None` outside herdr.
    ///
    /// Beside `no_title` rather than inside it because it answers a different
    /// question: `no_title` is whether to name anything, this is whether there is a
    /// tab to name as well as a pane. Both are read by [`naming_gate`]'s two
    /// callers — see [`HerdrTabRename`].
    pub(crate) herdr_tab_id: Option<String>,
    /// `HERDR_BIN_PATH`: which herdr to ask, falling back to `PATH`.
    pub(crate) herdr_bin: Option<String>,
    pub(crate) stdin_tty: bool,
    pub(crate) stdout_tty: bool,
    /// Whether stderr is a terminal, which is the stream the title is written to
    /// and so the only one whose tty-ness decides whether to write it. Tracked
    /// apart from `stdout_tty` because the two genuinely differ, and the
    /// difference is a case worth serving: `dl <ws> -- make test > log` has
    /// redirected stdout and still has a terminal to name.
    pub(crate) stderr_tty: bool,
    /// `$DEVPOD_SSH_CONFIG`, which `devpod up` takes `--ssh-config`'s default
    /// from — so it names the file devpod publishes its host aliases into, and
    /// the only one it publishes them into.
    pub(crate) devpod_ssh_config: Option<String>,
    /// `$SSH_AUTH_SOCK`: the agent this run would forward.
    ///
    /// Read for one purpose, and it is not a flag — it goes into the identity of
    /// the ssh control socket. A reused master forwards whichever agent opened it
    /// and ignores the later client's, so two runs with different agents must not
    /// find one another's master (devlaunch#389).
    pub(crate) ssh_auth_sock: Option<String>,
    /// The home directory, which holds the `~/.ssh/config` devpod falls back to
    /// and expands a `~/` in any of the paths above. `None` on a machine with no
    /// home directory; `dl` still runs there when `XDG_CACHE_HOME` is set, and
    /// then there is no ssh config for it to look in at all.
    pub(crate) home: Option<PathBuf>,
    /// Everything devlaunch stores: the launch locks, the shared pixi cache and
    /// the context-options cache all hang off this.
    pub(crate) cache_dir: PathBuf,
    /// devpod's own home, whose `config.yaml` mtime expires the options cache.
    pub(crate) devpod_home: Option<DevpodHome>,
}

impl Host {
    /// devlaunch's copies of what devpod substituted, under this host's cache.
    ///
    /// Derived here rather than carried as a field, so the store and everything
    /// else the cache holds cannot be pointed at two different directories: there
    /// is one `cache_dir` and this is a view of it.
    pub(crate) fn kept_copies(&self) -> KeptCopies {
        KeptCopies::under(&self.cache_dir)
    }
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
            claude: claude::HostEnv::from_process(),
            dotfiles_on_attach: crate::osext::env_str(DOTFILES_ON_ATTACH_VAR),
            zellij: crate::osext::env_str(ZELLIJ_VAR),
            herdr: herdr::HostEnv::from_process(),
            no_tty: crate::osext::env_str(ssh::DISABLE_VAR),
            no_title: crate::osext::env_str(TITLE_DISABLE_VAR),
            herdr_tab_id: crate::osext::env_str(HERDR_TAB_VAR),
            herdr_bin: crate::osext::env_str(HERDR_BIN_VAR),
            stdin_tty: is_a_terminal(libc::STDIN_FILENO),
            stdout_tty: is_a_terminal(libc::STDOUT_FILENO),
            stderr_tty: is_a_terminal(libc::STDERR_FILENO),
            devpod_ssh_config: crate::osext::env_str(ssh::CONFIG_VAR),
            ssh_auth_sock: crate::osext::env_str(SSH_AUTH_SOCK_VAR),
            home: crate::osext::home_dir(),
            cache_dir: cache_dir.into(),
            devpod_home: DevpodHome::locate(),
        }
    }

    /// The lock two `up`s of one workspace serialize on.
    pub(crate) fn launch_lock_path(&self, workspace_id: &str) -> PathBuf {
        self.cache_dir
            .join(LAUNCH_LOCK_DIR)
            .join(format!("{workspace_id}.lock"))
    }

    /// Where this host's ssh control sockets live.
    ///
    /// Under the cache dir, so it follows `XDG_CACHE_HOME` like the rest of dl's
    /// storage: a scratch run gets scratch sockets, and `dl --purge` takes them
    /// away with everything else. Losing them costs the next trip its ~2s and
    /// nothing more, which is what makes that correct by construction.
    pub(crate) fn ssh_control_dir(&self) -> PathBuf {
        self.cache_dir.join(ssh::CONTROL_DIR)
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
        self.devpod_home.as_ref().map(DevpodHome::config)
    }

    /// The ssh config `devpod up` publishes this host's aliases into.
    ///
    /// `options` is the answer dl has **already** cached. Two of devpod's four
    /// candidate paths are context options, and asking devpod for them costs
    /// 0.4-0.7s (devlaunch#393) — more than the pty transport this decides on
    /// saves. A cache miss reads as "devpod's own defaults", which is what dl
    /// assumed unconditionally before, so the miss is never worse than the bug
    /// this replaced.
    pub(crate) fn ssh_config(&self, options: &ContextOptions) -> Option<PathBuf> {
        ssh::config_path(ssh::ConfigSources {
            include_option: options.ssh_config_include_path(),
            env: self.devpod_ssh_config.as_deref(),
            path_option: options.ssh_config_path(),
            home: self.home.as_deref(),
        })
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
    /// however often the token is asked for — see `HostToken`.
    NoGitHubToken(GhEvent),
    /// The token could not be written to the private file `devpod up` reads it
    /// from, so this workspace opens without a GitHub login. Python distinguishes
    /// "could not create" from "could not write"; `tempfile` does both in one
    /// call, so there is one arm.
    TokenNotStaged { reason: String },

    // --- the session (dl.py `workspace_ssh`)
    /// dl read the ssh config devpod publishes into and found no alias for this
    /// workspace, so this command gets no pty and interactive programs may exit
    /// immediately.
    NoTerminalAlias {
        workspace_id: String,
        config: PathBuf,
    },
    /// There is no ssh config where dl expects devpod to publish its host
    /// aliases, so no workspace has an alias and no command can get a pty. Named
    /// apart from [`Self::NoTerminalAlias`] because the advice differs, and
    /// differs by being *conditional*: a restart republishes an entry into a
    /// config that exists, and against a config that is not there it helps only
    /// if devpod writes to the same file dl read. When it does not — the
    /// variable differs from the one `devpod up` ran under, or a context option
    /// names a file dl's cache had not seen — a restart publishes somewhere dl
    /// is not looking and the notice comes back. Until devlaunch#421 the two
    /// were the same sentence, and this is the half that shipped — silently, on
    /// every host that sets `DEVPOD_SSH_CONFIG`.
    NoDevpodSshConfig {
        workspace_id: String,
        looked_in: PathBuf,
    },
    /// dl cannot say where devpod would publish an alias, so it cannot look.
    /// Carries nothing: what is missing is the machine's home directory, which is
    /// the same fact for every workspace.
    SshConfigUnlocatable,
    /// The argv of the session about to start, program included.
    SshCommand { argv: Vec<String> },

    // --- the session manager (flows::session_manager)
    /// The container was given what it needs to report an agent to the manager
    /// running this pane, and the socket it reports over is open.
    SessionManagerReady { pane_id: String, socket: String },
    /// It was not, and this is why. The session opens regardless: a status
    /// indicator is not worth a shell.
    SessionManagerUnavailable { reason: String },
    /// devpod itself failed the session; its own diagnostics are already on the
    /// user's stderr.
    DevpodSessionFailed { exit: Exit },
    /// Name the terminal after the workspace about to take it over, as the bytes
    /// to write and nothing else.
    ///
    /// A notice because the *moment* is the whole point -- the title has to land
    /// before the session takes the terminal, and which moment that is only
    /// core's stages know -- and because core writes to no stream, so the writing
    /// is the binary's. It is the one notice that is not a line: what it carries
    /// is an escape sequence, so the renderer that turns notices into sentences
    /// drops it and the sink that prints them writes this one raw.
    TerminalTitle(TerminalTitle),
    /// Name the herdr tab the same thing, which the escape above cannot reach.
    ///
    /// A notice for [`TerminalTitle`]'s reasons and one more. The moment is the
    /// same -- in front of the session that takes the terminal. The stream is not
    /// core's to touch, and neither is the process table: core runs no command it
    /// was not handed a runner for, and this one is deliberately not run through
    /// the runner, because the runner's commands are the launch's and this one is
    /// allowed to fail unnoticed. So the binary spawns it, as the binary writes
    /// the escape.
    ///
    /// Said even when it is [`Off`](HerdrTabRename::Off), so that the sink stays
    /// the only place deciding nothing happens.
    HerdrTab(HerdrTabRename),

    // --- the launch's own arms (dl.py `_run_cli`)
    /// The workspace is already running, so this launch attaches straight to it.
    AlreadyRunningAttaching { workspace_id: String },
    /// devpod holds a record for this workspace and no create result, so an `up`
    /// started and never finished. Its container may well be running.
    CreateNeverFinished { workspace_id: String },
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

    /// The ssh config `devpod up` writes host aliases into, if the context names
    /// one. Beaten by `--ssh-config` / `$DEVPOD_SSH_CONFIG`, and beats nothing
    /// but the `~/.ssh/config` default — see [`ssh::config_path`].
    pub(crate) fn ssh_config_path(&self) -> Option<&str> {
        self.0.get("SSH_CONFIG_PATH").map(String::as_str)
    }

    /// The `Include` file `devpod up` writes host aliases into instead, if the
    /// context names one. Beats every other candidate.
    pub(crate) fn ssh_config_include_path(&self) -> Option<&str> {
        self.0.get("SSH_CONFIG_INCLUDE_PATH").map(String::as_str)
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

/// The context options dl has already cached, and nothing else.
///
/// The sibling of [`context_options`] for the one caller that must not spend a
/// round trip: [`terminal_for`] reads two candidate ssh-config paths out of these
/// on *every* launch, including the warm ones, and `devpod context options` costs
/// 0.4-0.7s (devlaunch#393) against a pty transport worth rather less than that.
/// So a cache miss answers with devpod's defaults rather than asking, and devpod
/// is asked by whoever was going to ask anyway.
pub(crate) fn already_cached_options(host: &Host, now: SystemTime) -> ContextOptions {
    cached_options(
        &host.context_options_cache(),
        host.devpod_config().as_deref(),
        now,
    )
    .unwrap_or_default()
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
pub(crate) struct HostToken {
    asked: OnceCell<TokenLookup>,
}

/// What a provisioning pass saw of the container's Claude config directory, held
/// where both ends of the launch can reach it.
///
/// A sibling of [`HostToken`] and for the same structural reason: the two ends
/// never meet. Only a probe inside the container can read that container's mount
/// table, so the pass is the only thing that can learn this; and the session that
/// follows is the only thing that spends it. Between them sit `workspace_up` and
/// every caller of it.
///
/// Deliberately *not* a field on [`Host`]. `Host` is a value -- `Clone`, `Eq`,
/// `Default`, and shared across threads by tests that spawn a scope around it --
/// and interior mutability in it would cost that type its `Sync`. This is an
/// observation about one container, which is not a value the host has.
///
/// Empty until a pass answers. Empty is not [`ClaudeConfig::Ours`]: it forwards no
/// login at all. See [`crate::clients::claude`] for why that is the safe direction.
#[derive(Debug, Default)]
pub(crate) struct ClaudeSeen(Cell<Option<ClaudeConfig>>);

impl ClaudeSeen {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn set(&self, seen: Option<ClaudeConfig>) {
        self.0.set(seen);
    }

    fn get(&self) -> Option<ClaudeConfig> {
        self.0.get()
    }
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
pub(crate) enum Naming<'a> {
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
///
/// `occasion` is the one thing the launch knows that the pass cannot work out for
/// itself: whether the container it is about to speak to has just been through a
/// `devpod up` or was already running when this launch arrived. It travels as a
/// parameter rather than as two trait methods because it is not a different
/// operation — every implementation does the same thing with it, and one of them
/// (a host-side verdict cache) is the only reason it is asked at all.
pub trait Provision {
    /// The pass, and what it saw of the container's Claude config directory.
    ///
    /// `Ok(None)` from a pass that could not tell — a trip that did not get
    /// through, a report without the keys, a remembered verdict from before the
    /// marker carried the fact. It is not a synonym for "nobody else owns it": the
    /// caller forwards the host's Claude login only on `Ok(Some(Ours))`.
    fn provision_tools(
        &self,
        runner: &dyn Runner,
        workspace_id: &str,
        occasion: PassOccasion,
        title: Option<&str>,
    ) -> Result<Option<ClaudeConfig>, DevpodMissing>;

    /// What the last pass saw of this workspace's Claude config directory, from the
    /// host's own records and without a round trip.
    ///
    /// Asked on the one path that opens a session without provisioning anything:
    /// attaching to a workspace that is already up and finished creating. That is
    /// the common case, so answering `None` there would leave the credential working
    /// only on the launch that created the workspace -- which is exactly the bug
    /// this found.
    ///
    /// `None` by default, because an implementation with no host-side records has
    /// nothing to remember and no login should be forwarded on a guess.
    fn remembered_claude(&self, _workspace_id: &str) -> Option<ClaudeConfig> {
        None
    }
}

/// A launch that lends nothing — `DEVLAUNCH_NO_TOOLS`, and every test that is not
/// about provisioning.
/// Held for the #251 §7 public-API freeze — the `up` a caller asks for with
/// nothing lent. Only tests choose it today.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Default)]
pub(crate) struct NoProvisioning;

impl Provision for NoProvisioning {
    /// The occasion is ignored, and that is the whole of what "lends nothing"
    /// means: there is no pass for it to decide anything about.
    fn provision_tools(
        &self,
        _runner: &dyn Runner,
        _workspace_id: &str,
        _occasion: PassOccasion,
        _title: Option<&str>,
    ) -> Result<Option<ClaudeConfig>, DevpodMissing> {
        Ok(None)
    }
}

/// Lending the host's tools into every workspace devlaunch opens — the real
/// [`Provision`], moved out of the `dl` binary by #340.
///
/// The host facts are read once, when the value is built, rather than per pass:
/// a launch can provision twice (a sibling's `up` won the race, then this one's
/// `up` ran) and a switch that changed between them would make one launch two
/// different launches. The verdict cache is built here for the same reason and one
/// more — it is two paths, and resolving either of them a second time is how the
/// pass that *writes* a marker and the pass that *reads* one come to disagree about
/// where markers live.
///
/// `events` is the caller's sink, and it is the only thing that used to keep this
/// type in the binary: the pass streams [`ProvisionEvent`]s *while it runs*, because
/// a cold install moves hundreds of megabytes and a warning about it is worth
/// nothing an hour later. Held behind a [`RefCell`] because [`Provision`] answers
/// through `&self` and a sink is written to; that is a borrow the launch cannot
/// contend with, since one launch makes one pass at a time.
pub struct ToolProvisioning<'e> {
    switches: Switches,
    host: Option<HostLayout>,
    verdicts: VerdictCache,
    events: RefCell<&'e mut dyn Notices<ProvisionEvent>>,
}

impl<'e> ToolProvisioning<'e> {
    /// What this host will lend, whether it may, what it remembers, and where its
    /// events go.
    ///
    /// `cache` is the caller's for the reason [`Host::from_process`] takes it: the
    /// caller has already resolved devlaunch's cache directory for everything else,
    /// and a second answer here could disagree with the first.
    pub fn from_env(cache: &Path, events: &'e mut dyn Notices<ProvisionEvent>) -> Self {
        Self {
            switches: Switches::from_env(),
            // `None` is a machine with no home directory to look in: nothing to
            // lend, rather than nothing to do — the setup pass still runs, because
            // the stages it carries are not tools work.
            host: HostLayout::from_env(),
            // A `None` devpod home here means something else again: no file to
            // check a remembered verdict against, so nothing is ever trusted and
            // every pass travels, exactly as it did before the cache existed.
            verdicts: VerdictCache::under(cache, DevpodHome::locate()),
            events: RefCell::new(events),
        }
    }
}

impl Provision for ToolProvisioning<'_> {
    fn provision_tools(
        &self,
        runner: &dyn Runner,
        workspace_id: &str,
        occasion: PassOccasion,
        title: Option<&str>,
    ) -> Result<Option<ClaudeConfig>, DevpodMissing> {
        let provisioned = {
            let mut events = self.events.borrow_mut();
            provision::provision_tools(
                runner,
                workspace_id,
                occasion,
                self.switches,
                title,
                self.host.as_ref(),
                Some(&self.verdicts),
                &mut **events,
            )
        };
        // Every way of coming up empty is an arm of `Provisioning`, and none of them
        // is worth an event beyond the ones above: the workspace is up and the user
        // asked for a session, not for an install. A devpod that has gone missing is
        // the one answer that travels — the launch cannot go on without it, and the
        // launch ends with it.
        //
        // `CachedProvisioned` is silent for the same reason, and deliberately so: it
        // is the arm where a launch did *less* than it used to, and a word about it
        // would put a sentence on the terminal of every prewarm to announce that
        // nothing happened. `DEVLAUNCH_TIMING=1` is where a missing round trip is
        // worth reading, and it shows there as the trip that is not in the list.
        // The Claude fact travels; every arm of `Provisioning` still says nothing.
        provisioned.map(|pass| {
            let _: Provisioning = pass.provisioning;
            pass.claude()
        })
    }

    fn remembered_claude(&self, workspace_id: &str) -> Option<ClaudeConfig> {
        self.verdicts.remembered_claude(workspace_id)
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
// Eight parameters, one over clippy's line, and the eighth is the reason: a pass
// learns who owns the container's Claude config directory and the session that
// follows spends it, so the slot has to travel with the launch. Bundling these into
// a context struct would gather values with three different lifetimes and one
// `&mut` for the sake of the count.
#[allow(clippy::too_many_arguments)]
pub(crate) fn workspace_up(
    context: &mut CommandContext<'_>,
    host: &Host,
    token: &HostToken,
    claude_seen: &ClaudeSeen,
    provision: &dyn Provision,
    request: &UpRequest<'_>,
    title: Option<&str>,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<UpOutcome, NotRun> {
    timing::stage_result(timing::Stage::DevpodUp, || {
        up_under_stage(
            context,
            host,
            token,
            claude_seen,
            provision,
            request,
            title,
            notices,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn up_under_stage(
    context: &mut CommandContext<'_>,
    host: &Host,
    token: &HostToken,
    claude_seen: &ClaudeSeen,
    provision: &dyn Provision,
    request: &UpRequest<'_>,
    title: Option<&str>,
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

    // The same question the fast-attach arm asks, asked again on the other side of
    // the wait. The sibling this launch waited out is the most likely author of a
    // create that died in its hooks, and it leaves the container *running* — so
    // `is_running` alone would hand the loser exactly the workspace the fast-attach
    // guard exists to refuse, and the wait is what put it here. Re-read rather than
    // carried down from that guard, because the sibling may well have finished
    // successfully while this launch was blocked, and then the skip is right.
    if let Some(identity) = request.naming.identity()
        && serialization.waited()
        && !request.wants_more_than_a_running_workspace()
        && is_running(context.runner(), identity)
        && create_record(host.devpod_home.as_ref(), identity) != CreateRecord::NeverCompleted
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
        //
        // A top-up all the same, and what licenses that is not the paragraph above
        // being false but the anchor a verdict is checked against: a sibling whose
        // `up` *completed* rewrote `workspace_result.json` on its way out, which
        // leaves no marker to trust and sends this pass on the wire. So the case a
        // remembered verdict survives is the one where the sibling both completed
        // its `up` and ran its own pass — the prewarm, and the saving this is for.
        //
        // The residual is the sibling that started the container and then died
        // before devpod wrote that file: the stale result still matches, and the
        // hostname nobody set stays unset. Nothing the host can read tells that
        // apart from a container that never stopped.
        //
        // The copy is kept on this arm too, and it is not a tidiness: the sibling
        // this launch waited out may not have been `dl` at all, and `--purge`
        // destroys the cache holding the copies while leaving a foreign workspace's
        // volumes standing (devlaunch#452). It costs one write on a path that has
        // already established the create completed, which is the same condition the
        // copy is written under below. The claim it records is "dl brought this up",
        // never "dl created it".
        host.kept_copies().keep(identity, host.devpod_home.as_ref());
        let seen = provision
            .provision_tools(context.runner(), identity, PassOccasion::TopUp, title)
            .map_err(|DevpodMissing| NotRun::NotInstalled)?;
        claude_seen.set(seen);
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
    // The build runs for minutes in the foreground; it leads a process group of
    // its own so a Ctrl-C (or `kill -INT <pid>`) tears the whole build down with
    // `dl` rather than orphaning it holding the launch lock.
    let exit = devpod::run(context.runner(), &Call::new(args).leading_its_own_group())?;
    // `up` creates and starts workspaces, so any snapshot of `devpod list` taken
    // before it is now out of date.
    context.forget_workspaces();

    if !exit.is_success() {
        return Ok(UpOutcome::Refused { exit });
    }
    // Only after a successful `up`: there is no container to install into
    // otherwise. Inside the lock, for the reason it was taken above.
    if let Some(identity) = request.naming.identity() {
        // The tail of a completed `up`, and **before** the pass rather than after
        // it. devpod has just rewritten `workspace_result.json`, which is the one
        // document naming this container's volumes, and the pass below can fail and
        // take the whole launch with it while the container and its volumes stand.
        // A copy written on the other side of that pass would be the one launch
        // whose volumes nothing ever names. See [`crate::flows::kept_copies`].
        host.kept_copies().keep(identity, host.devpod_home.as_ref());
        // A devpod that went missing between the `up` that just worked and the pass
        // that follows it takes the launch with it, as Python's exception does: there
        // is no session to hand over without the binary that opens one.
        //
        // `AfterUp`, so this pass always travels. The `up` above either created the
        // container or started a stopped one, and both rebuild the UTS namespace
        // from the container's config — the hostname stage has to run again before
        // the session reads a prompt, whatever any remembered verdict says about
        // the tools.
        let seen = provision
            .provision_tools(context.runner(), identity, PassOccasion::AfterUp, title)
            .map_err(|DevpodMissing| NotRun::NotInstalled)?;
        claude_seen.set(seen);
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
    lifecycle::workspace_state(runner, workspace_id, Patience::AsLongAsItTakes)
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
    /// What [`ZELLIJ_VAR`] asks for.
    ///
    /// Read through [`ZellijSwitch::requested`] rather than through this module's
    /// own [`switched_on`], and that is the one place the shape of #151 matters
    /// here: since #391 the same variable also decides whether the container gets a
    /// zellij at all, and a second parse of one signal is how "wrap the command"
    /// and "install the thing being wrapped to" come to disagree — which is exactly
    /// the state that decision deleted, a session setup that fails and a command
    /// that runs anyway. `switched_on` stays for the variables it is the only
    /// reader of.
    pub(crate) fn from_host(host: &Host) -> Self {
        match ZellijSwitch::requested(host.zellij.as_deref()) {
            ZellijSwitch::Install => Self::Beside,
            ZellijSwitch::Skip => Self::Off,
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
/// Five arms rather than a bool, because the three middle ones are *reports*: dl
/// is on a terminal and cannot use it, which is worth saying and is not the same
/// as being in CI. They were one arm until devlaunch#421, and that is what let the
/// bug ship: "devpod published no alias for this workspace" and "dl is reading a
/// file devpod never writes" arrived as one sentence about restarting the
/// workspace, which fixes the first and is useless against the second. A state
/// that cannot be told apart cannot be reported, so it is split in the type.
///
/// **Every arm carries the config, including the one that works.** That is not
/// symmetry for its own sake: OpenSSH resolves an alias through `getpwuid`'s
/// `~/.ssh/config` and reads no environment variable, so the file dl found the
/// alias in has to reach the invocation as `-F` or the invocation names a host
/// that does not resolve. `Usable` was the one arm with no path, and the value was
/// therefore dropped at exactly the point it was needed and kept only where it was
/// printed — a `dl <ws> -- <cmd>` that stopped running at all, on the same hosts
/// this split was written for. The path travels with the state instead, so
/// `ssh::command_args` can require it and an invocation with no `-F` is not
/// something this flow can express.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Terminal {
    /// dl is on a terminal and devpod published this workspace's ssh alias in that
    /// config, which is the file OpenSSH has to be told to read.
    Usable { config: PathBuf },
    /// dl read the config devpod publishes into and this workspace has no alias
    /// in it, so there is no way to ask for a pty. The command still runs.
    NoAlias { config: PathBuf },
    /// dl knows which file devpod would publish into and there is nothing it can
    /// read there. Either devpod has never brought a workspace up on this host,
    /// or dl is looking somewhere devpod does not write.
    ConfigMissing { looked_in: PathBuf },
    /// dl cannot say where devpod would publish at all: no `DEVPOD_SSH_CONFIG`, no
    /// context option naming one, and no home directory to hold the default. `dl`
    /// reaches here on a machine with `XDG_CACHE_HOME` set and no home.
    ConfigUnlocatable,
    /// dl is not on a terminal, or the user opted this machine out.
    Absent,
}

/// What dl can give a command on this host, for this workspace.
///
/// `options` is the context options dl has already cached — see
/// [`Host::ssh_config`] for why this must not be the ones devpod would answer
/// with.
pub(crate) fn terminal_for(host: &Host, options: &ContextOptions, workspace_id: &str) -> Terminal {
    if !ssh::terminal_usable(host.no_tty.as_deref(), host.stdin_tty, host.stdout_tty) {
        return Terminal::Absent;
    }
    let Some(config) = host.ssh_config(options) else {
        return Terminal::ConfigUnlocatable;
    };
    match ssh::alias_in(&config, workspace_id) {
        ssh::Alias::Published => Terminal::Usable { config },
        ssh::Alias::WorkspaceAbsent => Terminal::NoAlias { config },
        ssh::Alias::NoConfig => Terminal::ConfigMissing { looked_in: config },
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
///
/// [`Self::Terminal`] carries the config for the same reason it carries the
/// payload: the invocation needs both, and re-deriving either at the call site is
/// a second lookup that can disagree with the one that made the decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route<'p> {
    /// A bare attach. It stays on devpod whatever the terminal says: devpod
    /// requests a pty for exactly this case, so there is nothing to escape.
    DevpodAttach,
    /// A command under a pty, through the ssh alias `devpod up` published, in the
    /// config the alias was read out of.
    Terminal {
        payload: &'p RemotePayload,
        config: &'p Path,
    },
    /// A command through `devpod ssh --command`, which never asks for a pty.
    DevpodCommand(&'p RemotePayload),
}

/// Route this session, reporting a terminal dl had and could not use.
pub(crate) fn route<'p>(
    command: Option<&'p RemotePayload>,
    terminal: &'p Terminal,
    workspace_id: &str,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Route<'p> {
    let Some(command) = command else {
        return Route::DevpodAttach;
    };
    match terminal {
        Terminal::Usable { config } => Route::Terminal {
            payload: command,
            config,
        },
        Terminal::NoAlias { config } => {
            notices.say(LaunchNotice::NoTerminalAlias {
                workspace_id: workspace_id.to_owned(),
                config: config.clone(),
            });
            Route::DevpodCommand(command)
        }
        Terminal::ConfigMissing { looked_in } => {
            notices.say(LaunchNotice::NoDevpodSshConfig {
                workspace_id: workspace_id.to_owned(),
                looked_in: looked_in.clone(),
            });
            Route::DevpodCommand(command)
        }
        Terminal::ConfigUnlocatable => {
            notices.say(LaunchNotice::SshConfigUnlocatable);
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
pub(crate) struct SessionContext<'a> {
    pub(crate) runner: &'a dyn Runner,
    pub(crate) host: &'a Host,
    pub(crate) token: &'a HostToken,
    pub(crate) claude_seen: &'a ClaudeSeen,
}

impl<'a> SessionContext<'a> {
    pub fn new(
        runner: &'a dyn Runner,
        host: &'a Host,
        token: &'a HostToken,
        claude_seen: &'a ClaudeSeen,
    ) -> Self {
        Self {
            runner,
            host,
            token,
            claude_seen,
        }
    }

    /// The host's token, asked for at most once across the whole launch.
    fn forwarded_token(&self, notices: &mut dyn Notices<LaunchNotice>) -> Option<&'a Token> {
        self.token.token(self.runner, &self.host.gh, notices)
    }

    /// The host's Claude login, if this container's config directory is its own.
    ///
    /// Resolved per session rather than once per launch, unlike
    /// [`Self::forwarded_token`], and the difference is the cost: that one may spawn
    /// `gh auth token`, this one reads a small file. Reading it again is what makes
    /// a token refreshed on the host reach the next session without a relaunch.
    ///
    /// `Foreign` and `None` both forward nothing. `Foreign` is a repo's own
    /// devcontainer having mounted its Claude config from somewhere, and forwarding
    /// into that would override a credential that can refresh itself with one that
    /// cannot -- Claude Code prefers the variable over the file. `None` is not
    /// knowing, which gets the same answer for the same reason.
    fn forwarded_claude(&self) -> Option<claude::Token> {
        if self.claude_seen.get() != Some(ClaudeConfig::Ours) {
            return None;
        }
        match claude::resolve_token(self.host.home.as_deref(), &self.host.claude) {
            claude::TokenLookup::Found(token) => Some(token),
            claude::TokenLookup::Missing(_) => None,
        }
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
    // Read off the command dl was handed, not the payload: the payload is already
    // wrapped in a `cd` and possibly in zellij, and the question is what program
    // the person asked for.
    let agent = command.and_then(herdr::agent_in);
    // The already-cached options, never a fresh `devpod context options`: this is
    // on the warm path, where that round trip costs more than the pty it decides.
    let options = already_cached_options(session.host, SystemTime::now());
    let terminal = terminal_for(session.host, &options, workspace_id);
    // Container-side reporting, which is the other half of naming the agent and
    // the only half that reaches an agent started *inside* the workspace. Off
    // unless the consent variable is set, and then only inside a manager's pane.
    // Started before the session and stopped after it, whichever transport the
    // session turns out to take.
    let relay = herdr::Reporting::resolve(&session.host.herdr)
        .and_then(|reporting| begin_reporting(session, workspace_id, &options, reporting, notices));
    let coordinates = relay.as_ref().map(|(reporting, _)| reporting);
    let session_outcome = match route(payload.as_ref(), &terminal, workspace_id, notices) {
        Route::Terminal { payload, config } => ssh_with_terminal(
            session,
            workspace_id,
            payload,
            config,
            workdir,
            herdr::Visibility {
                agent,
                reporting: coordinates,
            },
            notices,
        ),
        Route::DevpodAttach => devpod_session(
            session,
            workspace_id,
            None,
            workdir,
            coordinates,
            forward,
            notices,
        ),
        Route::DevpodCommand(payload) => devpod_session(
            session,
            workspace_id,
            Some(payload),
            workdir,
            coordinates,
            forward,
            notices,
        ),
    };
    // The forward outlives the session by nothing: the socket it carries is only
    // meaningful while an agent is in there to report through it, and an ssh left
    // holding a listen path in a container is the kind of thing that is still
    // there a week later.
    if let Some((_, forward)) = relay {
        forward.stop(session.runner);
    }
    session_outcome
}

/// Prepare the container and open the forward, reporting whichever part failed.
///
/// `None` means no reporting this launch, and it is never fatal: the reason is
/// said and the session opens regardless (`flows::session_manager`).
fn begin_reporting(
    session: &SessionContext<'_>,
    workspace_id: &str,
    options: &ContextOptions,
    reporting: herdr::Reporting,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Option<(herdr::Reporting, session_manager::Forward)> {
    // The alias, resolved without asking whether dl is on a terminal.
    //
    // Deliberately not [`Terminal`], which answers a different question and is
    // `Absent` for a launch whose output is redirected -- the forward does not
    // care about that, and keying it on a terminal is a bug this had. What it
    // needs is the file devpod published the alias into, because the forward is
    // OpenSSH: devpod's own `-R` was measured and hangs for a unix socket, so
    // there is no second way in, and a workspace with no alias has nothing to
    // forward over.
    let config = match session.host.ssh_config(options) {
        None => {
            notices.say(LaunchNotice::SessionManagerUnavailable {
                reason: "dl cannot tell where devpod publishes its ssh aliases, so the \
                         manager's socket cannot be forwarded"
                    .to_owned(),
            });
            return None;
        }
        Some(config) => match ssh::alias_in(&config, workspace_id) {
            ssh::Alias::Published => config,
            ssh::Alias::WorkspaceAbsent | ssh::Alias::NoConfig => {
                notices.say(LaunchNotice::SessionManagerUnavailable {
                    reason: format!(
                        "devpod has published no ssh alias for this workspace in {}, so the \
                         manager's socket cannot be forwarded",
                        config.display()
                    ),
                });
                return None;
            }
        },
    };
    // The forward first, because what `prepare` asks the container includes
    // whether this forward's socket arrived -- and nothing on this end can ask
    // that, since a detached child's stderr goes nowhere and nothing waits for its
    // exit. A refusal from here therefore has a forward to take back down.
    let forward =
        match session_manager::start_forward(session.runner, &config, workspace_id, &reporting) {
            Ok(forward) => forward,
            Err(reason) => {
                notices.say(LaunchNotice::SessionManagerUnavailable { reason });
                return None;
            }
        };
    match session_manager::prepare(session.runner, &config, workspace_id, &reporting) {
        session_manager::Prepared::Ready => {
            notices.say(LaunchNotice::SessionManagerReady {
                pane_id: reporting.pane_id().to_owned(),
                socket: reporting.container_socket(),
            });
            Some((reporting, forward))
        }
        session_manager::Prepared::Refused { reason } => {
            forward.stop(session.runner);
            notices.say(LaunchNotice::SessionManagerUnavailable { reason });
            None
        }
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
    reporting: Option<&herdr::Reporting>,
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
    let forwarding = claude::extend_ssh_forwarding(
        gh::ssh_forwarding(session.forwarded_token(notices)),
        session.forwarded_claude().as_ref(),
    );
    args.extend(forwarding.args.iter().cloned());
    // `--set-env` and not `--send-env`: these are the container's own paths for a
    // socket and a binary, which this host's environment does not hold.
    if let Some(reporting) = reporting {
        args.extend(reporting.devpod_flags());
    }

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
///
/// `config` is the file the alias was found in, and it arrives from
/// [`Route::Terminal`] rather than from a lookup here — the transport must not get
/// a second opinion about which config it is talking about. It reaches ssh as
/// `-F`, because ssh reads neither `$DEVPOD_SSH_CONFIG` nor `$HOME`.
fn ssh_with_terminal(
    session: &SessionContext<'_>,
    workspace_id: &str,
    payload: &RemotePayload,
    config: &Path,
    workdir: Option<&str>,
    visible: herdr::Visibility<'_>,
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<Session, SessionRefused> {
    // The agent's name goes on last and reaches only the environment, so the
    // permit list `Reuse::derive` keys the control socket on is the same list the
    // two credentials built (`clients::herdr`). The manager's coordinates, unlike
    // the name, do cross the transport and so do join that list.
    let forwarding = herdr::extend_openssh_forwarding(
        claude::extend_openssh_forwarding(
            gh::openssh_forwarding(session.forwarded_token(notices)),
            session.forwarded_claude().as_ref(),
        ),
        visible.agent,
    );
    let forwarding = match visible.reporting {
        Some(reporting) => reporting.extend_openssh_forwarding(forwarding),
        None => forwarding,
    };
    // Derived from the permit list that is about to be sent, not from one read
    // again somewhere else: a master filters `SendEnv` against its own list in
    // silence, so the list and the socket it is carried over have to be one fact.
    let reuse = ssh::Reuse::derive(
        &session.host.ssh_control_dir(),
        workspace_id,
        &forwarding.args,
        session.host.ssh_auth_sock.as_deref(),
    );
    let args = ssh::command_args(
        config,
        workspace_id,
        payload.as_str(),
        &forwarding.args,
        workdir,
        &reuse,
    )
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
/// The retry is what keeps a long-lived workspace reachable by this command at
/// all. `chezmoi update` pulls the source and applies it, but the config it
/// applies *with* was rendered by `chezmoi init` when the workspace was created
/// and is never regenerated. A dotfiles repo that adds a template variable
/// therefore pulls fine and then aborts on every apply — under
/// `missingkey=error`, which a careful repo sets, `map has no entry for key` is
/// fatal — and the workspace is stuck at the revision it had, on the one command
/// whose job is to unstick it. Re-initialising has to come after the pull rather
/// than before it, because the template that learned the new variable arrives
/// *in* that pull; running it as a retry gets the order right without this side
/// having to take `update` apart into its two halves and own git's failure modes.
///
/// It costs a healthy workspace nothing: the first `update` succeeds and the
/// branch is never taken. A retry after some other failure — an unreachable
/// remote — re-renders a config locally, which is cheap, and then fails again
/// with the error that mattered.
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
/// ## Why the retry is guarded, and why `init` carries three flags
///
/// The retry exists for one failure: the dotfiles repo grew a template variable
/// that the workspace's `chezmoi.toml` predates, so `update` dies rendering a
/// config that has no entry for it. Re-running `init` regenerates the config and
/// the update goes through.
///
/// It is guarded on the source directory already being a git repository because
/// **`chezmoi init` with no repo argument `git init`s one when it is not**, and
/// that is unrecoverable rather than merely useless: the empty repo has no
/// upstream, so `update` fails with `no tracking information` — forever, because
/// every later refresh now finds a repository and `init` is a no-op. One
/// actionable error (`not a git repository`) is converted into a permanent one,
/// and the diagnosis that would have explained it is destroyed. Reproduced
/// against chezmoi 2.72.
///
/// The guard also restores what the docstring above claims: a failure this retry
/// cannot fix now fails with *the error that mattered*, rather than with whatever
/// the second `chezmoi update` said. And the notice is printed after the guard,
/// so a network failure or a dirty source tree is no longer announced as a config
/// problem.
///
/// `--force` is not "non-interactive" — chezmoi spells it "make all changes
/// without prompting", which is about changes, not about the `prompt*` functions
/// a config template calls. Those need `--promptDefaults`, and its absence is why
/// the retry could not fix its own motivating case: a repo that adds a
/// `promptString` variable makes `init` ask for it, and a refresh with no
/// terminal dies on `could not open a new TTY` while one with a terminal blocks
/// on a question nobody is watching for. `--no-tty` turns the remaining case — a
/// prompt with no default for `--promptDefaults` to return — into a fast error
/// rather than a hang inside the bound.
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
         {{ chezmoi update --force || \
            {{ git -C \"$(chezmoi source-path 2>/dev/null)\" rev-parse --git-dir >/dev/null 2>&1 && \
               echo \"Re-initialising the chezmoi config and retrying...\" && \
               chezmoi init --force --promptDefaults --no-tty && \
               chezmoi update --force; }} }} && \
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
// naming the terminal after the workspace
// ===========================================================================

/// The escape sequence that names a terminal, and the decision to write one.
///
/// `Off` is a real arm rather than an `Option` at the call site so that the two
/// reasons not to write -- opted out, or no terminal to write to -- collapse into
/// one thing the caller cannot forget to check.
///
/// # Why an escape sequence and not a multiplexer command
///
/// Because the escape reaches every multiplexer at once, and a command reaches
/// one. dl writes to the stream it was given; whoever owns that pty parses it.
/// zellij, tmux and byobu-on-tmux all take OSC 2 as the pane title, and a bare
/// terminal takes it as the window title, so one write covers them without dl
/// having to know which of them it is inside. `zellij action rename-tab` would
/// mean detecting zellij, shelling out, and doing nothing for anyone else.
///
/// Two limits are worth stating because they are not dl's to fix:
///
/// - **GNU screen** -- byobu's other backend -- names windows with `ESC k <name>
///   ESC \` and ignores OSC 2. Emitting both sequences would put stray text in
///   any terminal that groks neither, so screen is out of scope rather than
///   half-served.
/// - **In tmux the window name needs `allow-rename on`** (off by default in
///   recent tmux), and the outer title needs `set-titles on`. The *pane* title
///   always takes it. Both are the user's config, not a call dl can make.
///
/// # What the title says
///
/// The name is the placement's ([`Placement::title`]), and it is the workspace id
/// with its two unreadable characteristics taken off: the four-character suffix,
/// which carries identity and no meaning, and the dash between the repo and the ref,
/// which is spelled `@`. `docs/workspaces.md` tabulates what a tab, a listing row and
/// a selector row read for one workspace, and is where that spelling is decided; this
/// comment deliberately does not write it, because a comment nothing checks is the
/// copy that goes stale. [`WorkspaceId::label`]'s own tests carry the worked
/// examples.
///
/// **It is the id, not a second derivation of the spec.** The slugs and the
/// truncation are [`WorkspaceId::label`]'s, which are [`WorkspaceId::value`]'s, so a
/// tab and a listing row still match by eye: one is the other with a suffix removed
/// and a separator changed. A tab is read at a glance and a listing row is read
/// deliberately, and the two characters that go are the two a glance cannot use.
///
/// It has also been the full spec, `owner/repo@ref`, and the reason that is not what
/// came back is length: [`WorkspaceId::new`] validates the characters of a triple and
/// not its length, so a 200-character ref made a 200-character tab. A label inherits
/// the id's bound instead -- at most
/// [`TARGET_LENGTH`](crate::domain::workspace_id) characters, less the suffix --
/// because devpod refuses a workspace whose name exceeds 48. What stays lost with the
/// owner is the fork: `blooop/devlaunch@main` and a fork of it read alike, since an
/// id has never carried an owner and this is still the id.
///
/// The three arms that never formed a triple -- a bare devpod name, a path and a URL
/// -- have no ref for an `@` to precede and are titled by id, exactly as before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalTitle {
    /// Write this, exactly.
    Write(String),
    /// Write nothing.
    Off,
}

impl TerminalTitle {
    /// What this host wants written for *name*.
    ///
    /// *name* is what a person should read, not what devpod is addressed by — see the
    /// type's own docs and [`Placement::title`] for which of the two it is.
    pub(crate) fn from_host(host: &Host, name: &str) -> Self {
        match naming_gate(host, name) {
            Some(text) => Self::Write(format!("\x1b]2;{text}\x07")),
            None => Self::Off,
        }
    }

    /// The bytes to write, or nothing.
    ///
    /// Borrowed rather than cloned: the sink writes it and drops it.
    pub fn osc(&self) -> Option<&str> {
        match self {
            Self::Write(osc) => Some(osc),
            Self::Off => None,
        }
    }
}

/// A name with everything either sink would read as an instruction taken out, or
/// `None` if that leaves nothing worth writing.
///
/// Two sinks, and they do not fear the same characters. The escape this process
/// writes is ended by a control, so controls come out. The container half is
/// assigned into `PS1`, and bash expands `PS1` *again* at every prompt
/// (`promptvars`, on by default), so `$`, a backtick and a backslash come out too:
/// quoting the assignment makes the name text only until the first prompt renders
/// it, and a `$(...)` that survived into the value runs then, in the workspace, at
/// every prompt. One filter for both because the two halves have to be the one
/// string, or the tab changes the moment the first prompt paints.
///
/// A *derived* id holds none of the five, since
/// [`slug`](crate::domain::workspace_id::slug) leaves only lowercase alphanumerics
/// and dashes, so nothing legitimate is lost. The filter is for the two arms that
/// title without deriving an id -- `Plan::Existing`'s raw spec and
/// `Plan::Creatable`'s path leaf -- which this crate never validated. devpod's own
/// name rules would refuse most of what is dangerous here, but that is a guarantee
/// borrowed from another program's validation, which is the difference between a
/// safe title and a title that is safe until devpod loosens a rule. Dropping rather
/// than escaping keeps both sinks with nothing to decide, and there is no escaping
/// that would work for `PS1` anyway: it is re-expanded, not re-parsed, and `\$`
/// renders as `#` for root.
fn sanitize_title(name: &str) -> Option<String> {
    let text: String = name
        .chars()
        .filter(|ch| !ch.is_control() && !matches!(ch, '$' | '`' | '\\'))
        .collect();
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

/// The one decision behind every name this launch publishes: whether to name
/// anything, and what the name is.
///
/// Both emitters go through here, so neither can be on while the other is off and
/// neither can spell the name differently. That is the same guarantee
/// [`sanitize_title`] already gives dl's escape and the container's `PS1` line --
/// "one filter for both because the two halves have to be the one string" --
/// extended to the third emitter rather than restated beside it.
///
/// `None` covers all three refusals at once: opted out with
/// [`TITLE_DISABLE_VAR`], no terminal on stderr to name, and a name that
/// sanitises away to nothing.
fn naming_gate(host: &Host, name: &str) -> Option<String> {
    if switched_on(host.no_title.as_deref()) || !host.stderr_tty {
        return None;
    }
    sanitize_title(name)
}

/// Name the *herdr tab* after the workspace, which no escape sequence can do.
///
/// # Why this exists when [`TerminalTitle`] already names the terminal
///
/// Because herdr terminates the escape and shows something else. Measured on a
/// live herdr 0.8.2 inside zellij inside kitty: `dl rocker@nb1` left herdr's own
/// record of that pane reading `terminal_title: "rocker@nb1"` -- the OSC arrived,
/// unmangled, and claude in the container did not overwrite it -- while
/// `herdr tab list` reported the tab's label as `4`. A herdr tab label is a field
/// of its own (`custom_name` in herdr's `session.json`), it falls back to the tab
/// *number* when unset, and `herdr tab rename` is the only thing that writes it.
/// herdr publishes no config option that would derive one from a pane title.
///
/// So the name dl already computed sits one field away from the tab strip with
/// nothing to carry it across. This carries it.
///
/// **It is not a reversal of [`TerminalTitle`]'s argument against multiplexer
/// commands.** That argument weighs an escape against a command for the same
/// target and prefers the escape, because the escape reaches every multiplexer
/// and costs no detection. Both halves still hold: the escape is still written,
/// still reaches herdr, and detection here costs an environment lookup rather
/// than a round trip, because herdr exports [`HERDR_TAB_VAR`] into every pane it
/// spawns. What is new is a target the escape cannot address at all.
///
/// # Why the innermost multiplexer is the one that matters
///
/// A stacked terminal gives the escape to whichever multiplexer owns the pty, and
/// that is the innermost one. Under `kitty -> zellij` the escape reached zellij,
/// which took it as the pane title and published `<session> | <pane title>` upward,
/// so the workspace name showed in the outer terminal -- which is what
/// `docs/workspace-tools.md` promises. Insert herdr and the same escape stops at
/// herdr, one layer further in, and the layers that used to display it never see
/// it. Nothing in dl changed; a layer was added. The tab strip herdr draws is then
/// the only place the name can appear, which is where this puts it.
///
/// # Best-effort, always
///
/// A tab label is decoration and a workspace is not. A stale tab id, a herdr
/// server that has since exited, a binary that is not on `PATH` -- none of them
/// may cost the launch anything, so the caller spawns this and ignores what
/// happens, the way the dotfiles refresh is ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HerdrTabRename {
    /// Run this, and do not care whether it worked.
    Run {
        /// [`HERDR_BIN_VAR`] when herdr exported one, else [`HERDR_BIN_FALLBACK`]
        /// for `PATH` to resolve.
        bin: String,
        /// The value of [`HERDR_TAB_VAR`], passed straight back to herdr. Opaque
        /// to dl: herdr's ids are herdr's to spell (`w8:t7` today), and validating
        /// somebody else's format here would only reject a future one.
        tab_id: String,
        /// The name, from [`naming_gate`] and therefore byte-identical to the one
        /// inside [`TerminalTitle`]'s escape.
        label: String,
    },
    /// Do nothing: not in herdr, or not naming anything.
    Off,
}

impl HerdrTabRename {
    /// What this host wants renamed for *name*, or [`Off`](Self::Off).
    pub(crate) fn from_host(host: &Host, name: &str) -> Self {
        let tab_id = nonblank(host.herdr_tab_id.as_deref());
        match (naming_gate(host, name), tab_id) {
            (Some(label), Some(tab_id)) => Self::Run {
                bin: nonblank(host.herdr_bin.as_deref())
                    .unwrap_or(HERDR_BIN_FALLBACK)
                    .to_owned(),
                tab_id: tab_id.to_owned(),
                label,
            },
            _ => Self::Off,
        }
    }

    /// The command to run, program first, or nothing.
    ///
    /// argv and not a shell string, which is what makes the label safe to pass
    /// through unquoted: there is no shell to re-read it. [`sanitize_title`] has
    /// already removed every control character, and a NUL is the only byte argv
    /// itself cannot carry -- `char::is_control` covers it -- so what survives the
    /// shared filter is exactly what `execve` will take. The `$`, backtick and
    /// backslash that filter also drops are surplus to this sink and dropped
    /// anyway, because one name for the tab and the pane is worth more than three
    /// characters in a repo slug that could never hold them.
    ///
    /// A label that begins with a dash goes over as a bare positional, and **the
    /// `--` that would normally guard it must not be sent**. herdr's `<LABEL>...`
    /// is variadic and joins what it collects, and it collects the separator too:
    /// `herdr tab rename <id> -- devlaunch@nb3` names the tab `-- devlaunch@nb3`.
    /// Measured against herdr 0.8.2, after sending it and reading the tab back.
    ///
    /// So the dash is left to herdr, which takes it as a value today
    /// (`herdr tab rename zz:t999 -odd` answers `tab_not_found`, a socket
    /// refusing a tab rather than a parser refusing a flag). Stripping it here
    /// instead is the one repair not available: it would name the tab something
    /// the escape did not name the pane, which is the whole thing this shares a
    /// filter to prevent.
    pub fn argv(&self) -> Option<Vec<&str>> {
        match self {
            Self::Run { bin, tab_id, label } => Some(vec![bin, "tab", "rename", tab_id, label]),
            Self::Off => None,
        }
    }
}

/// The program name a `PATH` lookup is left to resolve when herdr exported no
/// [`HERDR_BIN_VAR`].
///
/// A fallback and not the default: reaching it means the pane says it belongs to a
/// herdr tab while saying nothing about which herdr, which is a herdr older than
/// the variable rather than an error.
pub(crate) const HERDR_BIN_FALLBACK: &str = "herdr";

/// *value* with a blank one read as absent.
///
/// An exported-but-empty variable is what a shell leaves behind when something
/// upstream failed to compute it (`export HERDR_BIN_PATH="$(command -v herdr)"`
/// finding nothing), so it means "no answer" and not "a program with no name".
/// Trimmed, because a tab id is compared by herdr and a path is handed to
/// `execve`, and neither wants the whitespace.
fn nonblank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
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
    title: TerminalTitle,
    herdr_tab: HerdrTabRename,
    command: Option<&str>,
    forward: &mut dyn FnMut(&str),
    notices: &mut dyn Notices<LaunchNotice>,
) -> Result<Session, SessionRefused> {
    timing::stage_result(timing::Stage::Attach, || {
        // First, and unconditionally on the way to every session: the terminal is
        // about to belong to something else, and after that this process may not
        // print again for hours. `Off` is said too, so the sink is what decides
        // nothing rather than the caller deciding twice.
        //
        // Decided by the caller and not here, because what a workspace is *called*
        // is a fact about the spec that was launched and this function is given only
        // an id — see [`Placement::title`].
        notices.say(LaunchNotice::TerminalTitle(title));
        // Then the tab, which the escape cannot reach. Behind the escape because
        // the escape is the one racing a shell prompt; both in front of the
        // refresh, which neither may wait for.
        notices.say(LaunchNotice::HerdrTab(herdr_tab));
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
///
/// A sum over the reasons and not a rendered sentence. It used to be
/// `reason: String`, which was the one place the binary's own prose travelled back
/// *through* core — `dl` rendered the words and core quoted them into the launch's
/// refusal — against the crate's own rule that no user-facing English lives here
/// (#251 §5). The arms carry what the reason is; the sentences are the caller's,
/// as they are for [`crate::flows::repo_manager::NotRefreshed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColdRefused {
    /// The records could not be opened: no home directory, an unreadable
    /// `config.toml`, or a `metadata.json` that would not open.
    Startup(StartupError),
    /// This launcher was built with no cold path at all: the caller established
    /// the workspace was warm and lent machinery that refuses on principle (the
    /// crate's own `NoColdPath` is one). Nothing was attempted, and nothing is
    /// wrong with the machine.
    NoColdPath,
}

/// A way to build the cold path's machinery, called only when it is needed.
///
/// **This is devlaunch#145 in the type system.** Building the clone manager reads
/// `config.toml`, loads `metadata.json` under the metadata lock and runs the cache
/// migration — three things that ticket deliberately took off the warm attach
/// path, which is the path a user waits on. A launcher holding a
/// `&mut MetadataStorage` would have paid for all three before it could ask
/// devpod anything, so it holds a *way to get one* instead, and "a warm launch
/// brings none of that up" is a fact about which calls happen rather than a
/// property to be re-tested.
///
/// [`recorded`](Self::recorded) is the one thing a warm launch may ask for, and
/// its docs say what makes it different in kind: it reads the file and takes none
/// of the three.
///
/// It is the same move [`lifecycle::resolve_known_workspace`] makes with its
/// `recorded_id` closure, one level up.
pub trait ColdMachinery<'r> {
    fn open(&mut self) -> Result<Cold<'_, 'r>, ColdRefused>;

    /// What `metadata.json` already records, for a reader that only wants to look.
    ///
    /// Separate from [`open`](Self::open) because the two cost different things,
    /// and the difference is the whole reason the collision guard
    /// (blooop/devlaunch#438) can run in front of a warm attach at all. `open`
    /// brings the *machinery* up: `config.toml`, the clone manager, and the cache
    /// migration under the metadata lock, which is a subprocess every sibling
    /// launch of the repository would queue behind. This reads the file and stops.
    /// A launch that only has to answer "does some other triple already hold this
    /// id" needs the records and none of the rest.
    ///
    /// **`None` means "nothing recorded", never "something went wrong".** A store
    /// that cannot be found, opened or parsed answers `None` and the launch
    /// proceeds, on [`recorded_id`]'s reading: a lookup that failed must not be
    /// able to stop a command that would otherwise have worked. The guard above it
    /// is a check on a rare accident, and a check that can fail closed turns a rare
    /// accident into a common one.
    fn recorded(&mut self) -> Option<&MetadataStorage>;
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
        Err(ColdRefused::NoColdPath)
    }

    /// Nothing recorded, which is the truth about a caller that has no records at
    /// all -- and the fail-open answer the collision guard is built to accept.
    fn recorded(&mut self) -> Option<&MetadataStorage> {
        None
    }
}

/// devlaunch's records, opened on the first ask and kept for the rest of the
/// command.
///
/// **The real implementation, and the reason this type exists at all.** Core states
/// the requirement in [`ColdMachinery`] — *a way to get* a clone manager and a
/// metadata store, never the things themselves — so that a warm launch can be shown
/// never to have read `metadata.json`. Something has to hold the other end of that,
/// and until #340 it was a private type inside the `dl` binary: a second consumer
/// could name [`Launch`] and had nothing to hand it that could go cold.
///
/// The open reports once, here, rather than at each call site: a caller that forgot
/// would silently drop what a damaged `metadata.json` has to say, and two callers
/// that both remembered would say it twice. What it reports is
/// [`RecordsNotice`]s into the sink the caller supplied — typed events, said at the
/// moment the open happens. The sentences stay with whoever wrote the sink.
pub struct ColdPath<'r, 'e> {
    runner: &'r dyn Runner,
    said: &'e mut dyn Notices<RecordsNotice>,
    records: Option<Records<'r>>,
    /// `metadata.json` as read by a caller that only wants to look at it.
    ///
    /// A second copy of the store, and deliberately so: loaded without the
    /// migration, without `config.toml` and without the clone manager, which is the
    /// whole point of [`ColdMachinery::recorded`]. Nothing ever writes through it,
    /// and once the real records are open they answer instead, so it can become
    /// neither a second writer nor a stale copy anybody acts on. A few kilobytes,
    /// held for the length of one command.
    looked_at: Option<MetadataStorage>,
}

impl<'r, 'e> ColdPath<'r, 'e> {
    /// A cold path that has not been opened, and will not be until something asks.
    ///
    /// Nothing is read here: no config, no `metadata.json`, no migration. That is
    /// devlaunch#145's whole promise, and it is kept by this constructor doing
    /// nothing.
    pub fn new(runner: &'r dyn Runner, said: &'e mut dyn Notices<RecordsNotice>) -> Self {
        Self {
            runner,
            said,
            records: None,
            looked_at: None,
        }
    }

    /// The records, opening them the first time and reporting what that had to say.
    pub fn records(&mut self) -> Result<&mut Records<'r>, StartupError> {
        if self.records.is_none() {
            let records = records::open_records(self.runner)?;
            self.said.say_all(records.reported.iter().cloned());
            self.records = Some(records);
        }
        Ok(self.records.as_mut().expect("the records were just opened"))
    }
}

impl<'r> ColdMachinery<'r> for ColdPath<'r, '_> {
    fn open(&mut self) -> Result<Cold<'_, 'r>, ColdRefused> {
        match self.records() {
            Ok(records) => Ok(Cold {
                clones: &records.clones,
                storage: &mut records.storage,
            }),
            Err(refused) => Err(ColdRefused::Startup(refused)),
        }
    }

    /// The records as something to read, without bringing the machinery up.
    ///
    /// The real records answer if this command has already opened them, so a cold
    /// launch reads the file once rather than twice and the guard sees exactly what
    /// the rest of the command will write through. Otherwise it is
    /// [`MetadataStorage::look`]: the file, and none of `open_records`'s config,
    /// clone manager, migration or lock.
    ///
    /// Every failure on the way is `None`, silently. There is no sentence to say
    /// here: a store dl cannot read is reported the moment the command actually
    /// needs it, through the notices [`Self::records`] says, and a *look* that came
    /// up empty is not news -- it is the answer "nothing is recorded against that
    /// id", which is also what a machine with no records at all says.
    fn recorded(&mut self) -> Option<&MetadataStorage> {
        if self.records.is_some() {
            return self.records.as_ref().map(|records| &records.storage);
        }
        if self.looked_at.is_none() {
            self.looked_at = Some(MetadataStorage::look(MetadataStorage::default_path().ok()?));
        }
        self.looked_at.as_ref()
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
        title: String,
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
    Listed { workspace_id: String, title: String },
    /// This launch may have to create it: `source` is what devpod is given and
    /// `workspace_id` is the `--id`. Nothing has been asked of devpod about it.
    Creating {
        workspace_id: String,
        title: String,
        source: String,
    },
}

impl Placement {
    /// The id every later step addresses.
    pub fn workspace_id(&self) -> &str {
        match self {
            Self::Known { workspace_id, .. }
            | Self::Listed { workspace_id, .. }
            | Self::Creating { workspace_id, .. } => workspace_id,
        }
    }

    /// What to call this workspace where a person reads it -- see
    /// [`TerminalTitle`] for the two shapes it comes in and why.
    ///
    /// Carried beside the id rather than derived from it, because the arm that has
    /// the shorter name is the arm that resolved a triple, and a triple is not
    /// recoverable from an id: the id joins the repo slug and the ref slug with the
    /// same dash both of them may hold, so nothing downstream can tell which dash
    /// the `@` belongs at. Set once, where the placement is built and the triple is
    /// still in hand.
    pub fn title(&self) -> &str {
        match self {
            Self::Known { title, .. }
            | Self::Listed { title, .. }
            | Self::Creating { title, .. } => title,
        }
    }

    /// What devpod is given positionally.
    pub fn source(&self) -> &str {
        match self {
            Self::Known { workspace_id, .. } | Self::Listed { workspace_id, .. } => workspace_id,
            Self::Creating { source, .. } => source,
        }
    }

    /// How devpod is told which workspace this is.
    pub(crate) fn naming(&self) -> Naming<'_> {
        match self {
            Self::Known { workspace_id, .. } | Self::Listed { workspace_id, .. } => {
                Naming::Known { workspace_id }
            }
            Self::Creating { workspace_id, .. } => Naming::Create { workspace_id },
        }
    }

    /// Whether a launch may attach straight away — Python's
    /// `custom_id is None and known_state == "Running"`.
    pub(crate) fn is_running(&self) -> bool {
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
/// `is_running` gives: the span belongs inside the stage the lifecycle helper
/// opens, and a guard around the call would drop after that stage had closed.
/// A devpod that could not be run at all travels out as [`NotRun`] rather than
/// becoming a cold launch: see [`lifecycle::resolve_known_workspace`] for what a
/// cold launch on a devpod-less host costs.
pub fn resolve_triple(
    context: &mut CommandContext<'_>,
    cold: &mut dyn ColdMachinery<'_>,
    workspace: &WorkspaceId,
    notices: &mut dyn Notices<LaunchNotice>,
    patience: Patience,
) -> Result<Resolution, NotRun> {
    let known = lifecycle::resolve_known_workspace(
        context.runner(),
        workspace,
        || recorded_id(cold, workspace),
        &mut as_lifecycle(notices),
        patience,
    )?;
    Ok(match known {
        KnownWorkspace::Known {
            workspace_id,
            state,
        } => Resolution::Warm {
            placement: Placement::Known {
                title: titled(&workspace_id, workspace),
                workspace_id,
                state,
            },
        },
        KnownWorkspace::Unknown { .. } => Resolution::Cold {
            workspace: workspace.clone(),
        },
    })
}

/// What to call *workspace_id* where a person reads it, given the triple that
/// resolved to it.
///
/// [`WorkspaceId::label`] only where the id is this triple's own derivation.
/// [`lifecycle::resolve_known_workspace`] may answer with an id `metadata.json`
/// recorded instead, for a workspace created under an older id scheme and not yet
/// reconciled, and a label derived from the triple is a rendering of the id the
/// triple *would* have derived rather than of the one in play: `devlaunch@main` on
/// the tab of a workspace whose `dl --ls` row reads `devlaunch-main-legacy`, with
/// nothing between them to match by eye. The tab is a rendering of the id it is
/// addressed by, or it is that id.
fn titled(workspace_id: &str, workspace: &WorkspaceId) -> String {
    if workspace_id == workspace.value() {
        workspace.label()
    } else {
        workspace_id.to_owned()
    }
}

/// The devpod workspace id `metadata.json` holds for a triple, if any.
///
/// A store that cannot be opened answers `None`, which is Python's reading: a
/// lookup that failed must not be able to stop a command that would otherwise
/// have worked.
fn recorded_id(cold: &mut dyn ColdMachinery<'_>, workspace: &WorkspaceId) -> Option<String> {
    let opened = cold.open().ok()?;
    lifecycle::recorded_devpod_workspace_id(
        opened.storage,
        workspace.owner(),
        workspace.repo(),
        workspace.git_ref(),
    )
}

/// The triple that already holds this launch's derived id, if a different one does.
///
/// **The failure this closes is silence.** A workspace id is one hashed suffix
/// away from being injective ([`SUFFIX_LENGTH`](crate::domain::workspace_id)), and
/// two triples that do collide share both of the things the id names: the clone
/// directory `<repos_dir>/<owner>/<repo>/<id>` and the devpod workspace, whose
/// names are global rather than scoped by repository. So the second launch opens
/// the first one's checkout, having said nothing, and a later `dl <ws> rm` on
/// either deletes a clone the other still claims. That is the hazard
/// [`migration::migrate_record`](crate::flows::migration) already refuses to walk
/// into at migration time; this is the same refusal at launch time.
///
/// **The scan is local.** [`WorktreeInfo`] stores the triple beside the id derived
/// from it, so the answer is in the records dl already keeps and costs no round
/// trip, no devpod call and no new stored state.
///
/// Three ways a record can hold the id, and each names a resource that would
/// actually be shared:
///
/// - its `workspace_id` is the id, so the clone directory is one directory;
/// - its `devpod_workspace_id` is the id, so the container is one container;
/// - its triple *derives* the id, which is the collision itself. This is the arm
///   that catches a record whose clone has not been migrated onto its derived name
///   yet, and the arm that gives the fail-open rule below something to protect.
///
/// **A record that cannot be parsed back into a [`WorkspaceId`] is skipped, not
/// failed on.** The old derivation coerced unsafe refs instead of rejecting them,
/// so a stored branch is not necessarily a legal ref, and the migration reports
/// such records as `unusable` rather than stopping. A guard that refused every
/// launch on the machine because one old record will not parse would be worse than
/// the collision it is looking for.
///
/// **"A different triple" means [`Identity`](crate::domain::workspace_id::Identity),
/// not a different pair of strings.**
/// `NVIDIA/cuda-samples@main` and `nvidia/cuda-samples@main` derive one id
/// deliberately -- GitHub's owners and repos are case-insensitive, and
/// [`identity_of`] is the rule that makes both spellings one workspace instead of
/// one repository cloned twice. Comparing the raw strings here would read the
/// second spelling as an intruder holding the first one's id and refuse it, with a
/// message telling the reader to rename a branch when both branches are `main`.
/// Since the comparison and the derivation have to agree, they read the same rule.
fn colliding_record(
    cold: &mut dyn ColdMachinery<'_>,
    workspace: &WorkspaceId,
) -> Option<(String, String, String)> {
    let mine = workspace.identity();
    let storage = cold.recorded()?;
    storage
        .worktrees()
        .values()
        .find(|record| {
            identity_of(&record.owner, &record.repo, &record.branch) != mine
                && holds_id(record, workspace)
        })
        .map(|record| {
            (
                record.owner.clone(),
                record.repo.clone(),
                record.branch.clone(),
            )
        })
}

/// Whether *record* occupies the id *workspace* derives -- see
/// [`colliding_record`] for the three arms and why each of them is a resource and
/// not a coincidence.
///
/// Takes the triple rather than the rendered id, because the third arm re-derives
/// from the *record's* triple and compares: handing this an id from somewhere
/// other than the triple in play would answer a question nobody asked.
fn holds_id(record: &WorktreeInfo, workspace: &WorkspaceId) -> bool {
    let derived = workspace.value();
    record.workspace_id == derived
        || record.devpod_workspace_id.as_deref() == Some(derived)
        || WorkspaceId::new(&record.owner, &record.repo, &record.branch)
            .is_ok_and(|derivable| derivable.value() == derived)
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
            workspace_id: workspace.value().to_owned(),
            title: workspace.label(),
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
    /// Two triples derive one workspace id, and the other one got here first.
    ///
    /// Refused rather than attached to, for [`colliding_record`]'s reason: the id
    /// names both the clone directory and the devpod workspace, so going ahead
    /// hands this launch the *other* triple's checkout without saying so, and
    /// leaves a later `dl <ws> rm` able to delete work neither user knows is
    /// shared. Both triples and the id they share are carried, because the only
    /// way past it is to rename one of the two branches.
    IdCollision {
        /// The id both triples derive.
        workspace_id: String,
        owner: String,
        repo: String,
        branch: String,
        /// The triple `metadata.json` already records against `workspace_id`.
        recorded_owner: String,
        recorded_repo: String,
        recorded_branch: String,
    },
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
    /// What this launch's provisioning pass saw of the container's Claude config
    /// directory. Written by the pass, read by the session it hands over to.
    claude_seen: ClaudeSeen,
    /// Where this launch's notices go, as they happen. A `Vec` in a test that wants
    /// the sequence, the binary's printer in production.
    notices: &'a mut dyn Notices<LaunchNotice>,
    /// What the caller already knows this workspace is, for a launch that names it
    /// by id. See [`Self::recognised_as`].
    recognised: Option<WorkspaceId>,
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
            claude_seen: ClaudeSeen::new(),
            notices,
            recognised: None,
        }
    }

    /// The triple a caller has already recovered for the workspace it is about to
    /// name by id.
    ///
    /// **The picker is the caller this exists for**, and it is the only one. It
    /// hands back a workspace id, so a launch from it takes the bare-name arm and
    /// has no triple of its own -- but the picker had one a moment earlier, read
    /// out of the cache layout and the clone's `HEAD` to draw the row it was picked
    /// from. Without it, `dl` with no arguments, which is how a workspace is
    /// reopened, titles the tab `devlaunch-main-3j1t` where
    /// `dl blooop/devlaunch@main` titles it `devlaunch@main`, and the two names pile
    /// up in the profile a line each.
    ///
    /// It changes **only what the workspace is called**. Nothing here reaches
    /// devpod, the clone or the records: the launch is still the bare-name arm, one
    /// `devpod status` and no more, and a caller that passes a triple for the wrong
    /// workspace gets a differently-titled tab and nothing else. That is what makes
    /// this safe to take from a caller at all, and it is why the check on it lives
    /// in [`titled`] rather than out there: the picker carries evidence, core
    /// reaches the verdict, and a `HEAD` that has moved since the workspace was made
    /// is refused here exactly as a recorded id is.
    ///
    /// `None` is the default and the answer for every other arm.
    #[must_use]
    pub fn recognised_as(mut self, workspace: Option<WorkspaceId>) -> Self {
        self.recognised = workspace;
        self
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
                title: workspace_id.clone(),
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
        let state =
            lifecycle::workspace_state(self.context.runner(), &name, Patience::AsLongAsItTakes);
        if let Ok(state) = state {
            return Ok(Ok(Placement::Known {
                title: self.recognised_title(&name),
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
            return Ok(Ok(Placement::Listed {
                title: self.recognised_title(&name),
                workspace_id: name,
            }));
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
        // Before devpod is asked anything and before the cold path can build a
        // directory: an id two triples derive is not an id either of them may be
        // launched under, and the damage is done by the *attach*, so a guard behind
        // the status call would fire after the wrong container was already picked.
        // Reads the records and not the machinery, which is what keeps this off
        // devlaunch#145's bill -- see `ColdMachinery::recorded`.
        if let Some((recorded_owner, recorded_repo, recorded_branch)) =
            colliding_record(self.cold, &workspace)
        {
            return Ok(Err(LaunchRefusal::IdCollision {
                workspace_id: workspace.value().to_owned(),
                owner,
                repo,
                branch,
                recorded_owner,
                recorded_repo,
                recorded_branch,
            }));
        }
        // A devpod that could not be run ends the launch here, before the clone:
        // it is the probe Python raises `DevpodNotInstalled` out of.
        let resolved = resolve_triple(
            self.context,
            self.cold,
            &workspace,
            &mut *self.notices,
            Patience::AsLongAsItTakes,
        )
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
        // A running container is not on its own evidence that there is something
        // to attach to. A create that died in its lifecycle hooks leaves one up
        // with `devpod status` still answering `Running`, and devpod records no
        // remote user for it — so an attach lands as root, in a container whose
        // setup never ran, and fails on whatever the remote user's PATH was
        // supposed to hold.
        //
        // Asked only of a workspace that is running, and only `NeverCompleted`
        // acted on. Both halves are about not saying anything a caller cannot use:
        // a launch that was going to `up` anyway is not told the create is
        // unfinished, since `up` is what it was doing, and a host whose devpod
        // records will not read attaches exactly as it did before.
        if placement.is_running() {
            match create_record(self.host.devpod_home.as_ref(), placement.workspace_id()) {
                CreateRecord::NeverCompleted => {
                    self.notices.say(LaunchNotice::CreateNeverFinished {
                        workspace_id: placement.workspace_id().to_owned(),
                    });
                }
                CreateRecord::Completed | CreateRecord::Unknown => {
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
                    let session = self.attach(placement, verb.command());
                    self.forced_refresh();
                    return session;
                }
            }
        }
        if let Some(refused) = self.bring_up(verb, devcontainer, placement)? {
            return Ok(Launched::Refused(refused));
        }
        let session = self.attach(placement, verb.command());
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
        let session = self.attach(placement, verb.command());
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
        let session = self.attach(placement, verb.command());
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
        // The same question the attach arm asks, for the same reason: a create that
        // died in its hooks leaves the container up, so `Running` is a reading the
        // finished and the abandoned both produce. `up` is the worst verb to answer
        // "already running" for the second one -- it is the command a user reaches
        // for to fix a workspace, and the documented recovery would be the one path
        // that declines to run.
        if placement.is_running()
            && create_record(self.host.devpod_home.as_ref(), placement.workspace_id())
                != CreateRecord::NeverCompleted
        {
            self.notices.say(LaunchNotice::AlreadyRunning {
                workspace_id: placement.workspace_id().to_owned(),
            });
            // Still top up the tools: `up` is one of the two verbs named as how a
            // workspace that missed provisioning gets it, and returning here
            // without them would make the documented recovery the one path that
            // cannot recover.
            //
            // The top-up, and the one this cache was built for: `dl <ws> up` is the
            // prewarm, run repeatedly against a workspace that is already up, and
            // the round trip it pays here is the whole of what it costs.
            let seen = self
                .provision
                .provision_tools(
                    self.context.runner(),
                    placement.workspace_id(),
                    PassOccasion::TopUp,
                    self.container_title(placement.title()).as_deref(),
                )
                .map_err(|DevpodMissing| LaunchAborted::DevpodNotRun(NotRun::NotInstalled))?;
            self.claude_seen.set(seen);
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
        let running = lifecycle::workspace_state(
            self.context.runner(),
            placement.workspace_id(),
            Patience::AsLongAsItTakes,
        )
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
            let title = self.container_title(placement.title());
            let outcome = workspace_up(
                self.context,
                self.host,
                &self.token,
                &self.claude_seen,
                self.provision,
                &request,
                title.as_deref(),
                &mut *self.notices,
            )
            .map_err(LaunchAborted::DevpodNotRun)?;
            if let UpOutcome::Refused { exit } = outcome {
                return Ok(Launched::Refused(LaunchRefusal::UpRefused { exit }));
            }
        }
        let session = SessionContext::new(
            self.context.runner(),
            self.host,
            &self.token,
            &self.claude_seen,
        );
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
        let title = self.container_title(placement.title());
        let outcome = workspace_up(
            self.context,
            self.host,
            &self.token,
            &self.claude_seen,
            self.provision,
            &request,
            title.as_deref(),
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
        placement: &Placement,
        command: Option<&str>,
    ) -> Result<Launched, LaunchAborted> {
        // A warm attach runs no pass, so nothing has observed this container's Claude
        // config directory during this launch. The host's own records stand in, and
        // they are consulted only when the launch itself learned nothing: a pass that
        // just ran is always the better answer.
        //
        // No records and no pass means no login forwarded, which is a real limit and
        // the least bad of three. Paying a pass here to find out was tried: it puts
        // two setup-stage warnings on the terminal of every first attach, on the
        // hottest path dl has, and an aid test caught it. Forwarding anyway was the
        // other option, and it would override a mounted credential that can refresh
        // itself, on every warm attach, for as long as no pass ran.
        //
        // So a workspace created by this build carries an answer from the pass that
        // created it and never reaches this at all. A workspace that predates it
        // acquires one on its next `up`, `restart` or `recreate`.
        if self.claude_seen.get().is_none() {
            self.claude_seen
                .set(self.provision.remembered_claude(placement.workspace_id()));
        }
        // Both names, from the one `placement.title()` and the one gate behind it,
        // so the tab and the pane cannot be given different answers.
        let title = TerminalTitle::from_host(self.host, placement.title());
        let herdr_tab = HerdrTabRename::from_host(self.host, placement.title());
        let context = SessionContext::new(
            self.context.runner(),
            self.host,
            &self.token,
            &self.claude_seen,
        );
        let session = attach_workspace(
            &context,
            placement.workspace_id(),
            title,
            herdr_tab,
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

    /// What to call *workspace_id*, given whatever [`Self::recognised_as`] was told.
    ///
    /// The id itself where nothing was told, which is every arm but the picker's.
    fn recognised_title(&self, workspace_id: &str) -> String {
        match &self.recognised {
            Some(workspace) => titled(workspace_id, workspace),
            None => workspace_id.to_owned(),
        }
    }

    /// The name a shell in this container should keep putting on the terminal, or
    /// `None` when there is none worth installing.
    ///
    /// [`Placement::title`], which is what the escape dl writes at the handover
    /// carries too. Both have to be the one string or the tab changes the moment the
    /// first prompt paints: dl writes its escape once, and this line repaints at
    /// every prompt after it.
    ///
    /// **A workspace has to reach the same name however it was reached, and that is
    /// a property of the placement rather than of this function.** The line is
    /// written under a mark hashed from its own text
    /// ([`profile_prepend`](crate::flows::provision)), so a name that varies for one
    /// workspace does not replace the line, it appends a second one, and the last
    /// append is what every prompt then obeys. Every launch that resolves a triple
    /// derives the label from that triple, so those agree; the arms that never had
    /// one use the id, which they also agree on. What is left is a workspace opened
    /// *both* ways -- once as `blooop/devlaunch@main` and once as
    /// `dl devlaunch-main-3j1t` -- which writes two lines and keeps whichever came
    /// last. That is the price of the `@`, and it is bounded: one extra line, and a
    /// tab named by the id instead of by the label.
    ///
    /// Filtered by [`sanitize_title`], the same way the escape is, because the two
    /// halves must not disagree about what a name may hold. A label and a *derived*
    /// id both hold nothing to filter -- [`slug`](crate::domain::workspace_id::slug)
    /// leaves only lowercase alphanumerics and dashes, and the label adds one `@` --
    /// but two arms reach here with a string this crate never validated: a bare
    /// devpod name and a path leaf.
    ///
    /// `DEVLAUNCH_NO_TITLE` still decides it, so one variable governs the whole
    /// feature rather than part of it. Three pieces now, not two: the escape this
    /// process writes, the `PS1` line every prompt repaints, and the export that
    /// stops claude renaming the pane between prompts. Somebody who turned dl's
    /// naming off did not ask for claude's titling to go too, and nothing in a
    /// container would tell them why it had.
    ///
    /// `stderr_tty` is deliberately *not* consulted, where
    /// [`TerminalTitle::from_host`] does consult it. That flag answers "is there a
    /// terminal to write to right now", which is the right question for one escape
    /// this process is about to emit and the wrong one for a line installed in a
    /// profile: `dl <ws> up` is a prewarm with its output redirected, and the
    /// interactive session that arrives later is the one the line is for.
    fn container_title(&self, title: &str) -> Option<String> {
        if switched_on(self.host.no_title.as_deref()) {
            return None;
        }
        sanitize_title(title)
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
    use crate::domain::model::WorktreeInfo;
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
                devpod_home: Some(DevpodHome::at(dir.path().join("devpod"))),
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
        ///
        /// Published where `$DEVPOD_SSH_CONFIG` names, because that is the shape
        /// this repo's own scratch convention, `test/conftest.py` and
        /// `scripts/bench_launch.py` all create — and the shape dl silently lost
        /// the pty transport on until devlaunch#421.
        fn on_a_terminal(mut self, workspace_ids: &[&str]) -> Self {
            let config = self.dir.path().join("ssh-config");
            let text: String = workspace_ids
                .iter()
                .map(|id| format!("# DevPod Start {id}.devpod\nHost {id}.devpod\n"))
                .collect();
            std::fs::write(&config, text).expect("an ssh config");
            self.host.stdin_tty = true;
            self.host.stdout_tty = true;
            self.host.devpod_ssh_config = Some(config.display().to_string());
            // No `~/.ssh/config` under it: devpod writes to the file above and
            // nowhere else, so a home that had one would hide the regression.
            self.host.home = Some(self.dir.path().join("home"));
            self
        }

        /// A host on a terminal whose only ssh config is the `~/.ssh/config`
        /// devpod falls back to when nothing names another.
        fn on_a_terminal_with_a_home_config(mut self, workspace_ids: &[&str]) -> Self {
            self = self.on_a_terminal(workspace_ids);
            let published = self
                .host
                .devpod_ssh_config
                .take()
                .expect("on_a_terminal named one");
            let home_config = self.dir.path().join("home").join(".ssh").join("config");
            std::fs::create_dir_all(home_config.parent().expect("a parent"))
                .expect("a scratch .ssh");
            std::fs::rename(&published, &home_config).expect("the config moves");
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

        /// devpod's record for a workspace whose create *finished*: the record and
        /// the result beside it, which is what devpod writes on its way out of a
        /// successful `up`.
        fn with_create_completed(self, workspace_id: &str) -> Self {
            let home = self.devpod_home();
            self.write_record(workspace_id);
            std::fs::write(home.result("default", workspace_id), "{}").expect("a create result");
            self
        }

        /// devpod's record for a workspace whose create finished, carrying what
        /// devpod substituted into the devcontainer.
        ///
        /// The shape is devpod's own — `SubstitutionContext` beside
        /// `ContainerDetails` and `MergedConfig` — because the copy this launch
        /// keeps is a read of exactly this document and nothing else.
        fn with_substitutions(
            self,
            workspace_id: &str,
            folder: &str,
            devcontainer_id: &str,
        ) -> Self {
            let home = self.devpod_home();
            self.write_record(workspace_id);
            std::fs::write(
                home.result("default", workspace_id),
                serde_json::json!({
                    "ContainerDetails": { "Config": { "Image": "vsc-repo-main-ab12-uid" } },
                    "MergedConfig": {},
                    "SubstitutionContext": {
                        "LocalWorkspaceFolder": folder,
                        "DevContainerID": devcontainer_id,
                    },
                })
                .to_string(),
            )
            .expect("a create result");
            self
        }

        /// devpod's record for a workspace whose create *aborted*: the record, and
        /// no result beside it. A `postCreateCommand` that exits non-zero leaves
        /// exactly this, with the container still up.
        fn with_create_aborted(self, workspace_id: &str) -> Self {
            self.write_record(workspace_id);
            self
        }

        fn devpod_home(&self) -> &DevpodHome {
            self.host
                .devpod_home
                .as_ref()
                .expect("a scratch devpod home")
        }

        /// The `workspace.json` devpod writes on its way *in*, at whatever path
        /// `clients::devpod_home` says it goes — asked rather than rebuilt, so this
        /// fixture cannot go on passing after devpod moves its layout.
        fn write_record(&self, workspace_id: &str) {
            let record = self.devpod_home().record("default", workspace_id);
            std::fs::create_dir_all(record.parent().expect("a devpod record directory"))
                .expect("a devpod record directory");
            std::fs::write(record, "{}").expect("a workspace record");
        }

        fn cache_dir(&self) -> &Path {
            self.dir.path()
        }

        /// Every devpod invocation, without the leading `devpod`, in order — the
        /// shape the Python `test_devpod_spawn_counts` asserted before it retired
        /// with the Python tree (#267). The spawn counts it gated on are gated
        /// here now, by the assertions in this module that read this list.
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
            let config = WorktreeConfig::defaults();
            let (storage, _) = MetadataStorage::open(cache_dir.join("metadata.json"))
                .expect("a fresh store opens");
            Self {
                clones: WorkspaceCloneManager::new(
                    crate::domain::xdg::clone_root_in(cache_dir),
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

        /// The same store, read rather than opened -- and not counted as an open,
        /// because it is not one: the collision guard reads the file and the
        /// machinery stays down. `opens` is what the #145 assertions are written
        /// against, so counting a look would make them assert something else.
        fn recorded(&mut self) -> Option<&MetadataStorage> {
            Some(&self.storage)
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

        /// Nothing recorded, which is the truth about a scene with no store behind
        /// it -- and not a panic, because looking is exactly what a warm launch is
        /// now allowed to do. The panic above still says what it said: the clone
        /// manager, `config.toml` and the migration stay off this path.
        fn recorded(&mut self) -> Option<&MetadataStorage> {
            None
        }
    }

    /// A cold path with real records that cannot be *read*.
    ///
    /// The fail-open case, and the reason it needs a stub of its own: a store dl
    /// cannot open, parse or find is common enough (a fresh machine, a half-written
    /// file, a cache on a filesystem that went away) that a guard which stopped the
    /// launch over it would break far more launches than the collision it is
    /// watching for ever will. So the records here hold a collision and the look
    /// answers `None` anyway.
    struct UnreadableRecords<'r>(RealCold<'r>);

    impl<'r> ColdMachinery<'r> for UnreadableRecords<'r> {
        fn open(&mut self) -> Result<Cold<'_, 'r>, ColdRefused> {
            self.0.open()
        }

        fn recorded(&mut self) -> Option<&MetadataStorage> {
            None
        }
    }

    /// A cold path whose `metadata.json` will not open, refusing exactly as
    /// [`ColdPath`] does when [`records::open_records`] hands it a
    /// [`StartupError`].
    struct MetadataWillNotOpen;

    /// The refusal a real `MetadataStorage::open` produces when the directory it
    /// needs cannot be created, spelled once so the tests below compare against the
    /// same value the arm is built from.
    fn metadata_refusal() -> crate::domain::metadata::MetadataError {
        crate::domain::metadata::MetadataError::CreateDir {
            path: PathBuf::from("/cache/devlaunch"),
            failure: crate::domain::metadata::OsFailure {
                kind: std::io::ErrorKind::NotADirectory,
                message: "Not a directory (os error 20)".to_owned(),
            },
        }
    }

    impl<'r> ColdMachinery<'r> for MetadataWillNotOpen {
        fn open(&mut self) -> Result<Cold<'_, 'r>, ColdRefused> {
            Err(ColdRefused::Startup(StartupError::Metadata(
                metadata_refusal(),
            )))
        }

        /// A store that will not open has nothing to show a reader either, and
        /// answers so rather than refusing: the collision guard treats "could not
        /// read" as "no collision", which is what keeps a broken cache from
        /// stopping every launch on the machine.
        fn recorded(&mut self) -> Option<&MetadataStorage> {
            None
        }
    }

    /// Records which workspaces had tools lent to them, on which occasion, and under
    /// what name the container was told to title a terminal.
    ///
    /// All three in one list rather than three, because they are asserted
    /// separately: most tests here are about *whether* a path provisions at all,
    /// only the ones about the pass's cost care which occasion it named, and only
    /// the title tests care about the third. One list is what keeps a test from
    /// reading them out of step with each other.
    #[derive(Debug, Default)]
    struct RecordingProvision {
        passes: Mutex<Vec<(String, PassOccasion, Option<String>)>>,
        /// A devpod that goes missing when the pass is asked for, which is the one
        /// thing a pass can answer.
        lost_devpod: bool,
        /// What the pass reports having seen of the container's Claude config
        /// directory. `None` by default, which is what every test that is not about
        /// the Claude login wants: nothing forwarded.
        claude_seen: Option<ClaudeConfig>,
        /// What the host's records say about a workspace no pass ran for, which is
        /// what `dl`'s real implementation reads out of its verdict cache.
        claude_remembered: Option<ClaudeConfig>,
    }

    impl RecordingProvision {
        fn passes(&self) -> Vec<(String, PassOccasion, Option<String>)> {
            self.passes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn provisioned(&self) -> Vec<String> {
            self.passes()
                .into_iter()
                .map(|(workspace_id, _, _)| workspace_id)
                .collect()
        }

        fn occasions(&self) -> Vec<PassOccasion> {
            self.passes()
                .into_iter()
                .map(|(_, occasion, _)| occasion)
                .collect()
        }

        /// What each pass was told to have the container's shells title a terminal.
        fn titles(&self) -> Vec<Option<String>> {
            self.passes()
                .into_iter()
                .map(|(_, _, title)| title)
                .collect()
        }
    }

    impl Provision for RecordingProvision {
        fn provision_tools(
            &self,
            _runner: &dyn Runner,
            workspace_id: &str,
            occasion: PassOccasion,
            title: Option<&str>,
        ) -> Result<Option<ClaudeConfig>, DevpodMissing> {
            self.passes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((workspace_id.to_owned(), occasion, title.map(str::to_owned)));
            if self.lost_devpod {
                return Err(DevpodMissing);
            }
            Ok(self.claude_seen)
        }

        fn remembered_claude(&self, _workspace_id: &str) -> Option<ClaudeConfig> {
            self.claude_remembered
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
    ///
    /// Titles the workspace after its id, which is what the bare-name arm does: these
    /// tests are given an id and no triple, so there is no spec for [`Launch::titled`]
    /// to prefer. The launch-level tests are where the spec form is pinned.
    fn attaching(
        scene: &Scene,
        token: &HostToken,
        workspace_id: &str,
        command: Option<&str>,
        notices: &mut Vec<LaunchNotice>,
    ) -> Result<Session, SessionRefused> {
        let claude_seen = ClaudeSeen::new();
        let session = SessionContext::new(&scene.runner, &scene.host, token, &claude_seen);
        let title = TerminalTitle::from_host(&scene.host, workspace_id);
        let herdr_tab = HerdrTabRename::from_host(&scene.host, workspace_id);
        attach_workspace(
            &session,
            workspace_id,
            title,
            herdr_tab,
            command,
            &mut nowhere,
            notices,
        )
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
                    &ClaudeSeen::new(),
                    provision,
                    request,
                    None,
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

    /// The skip above reads `Running` and stops there, which is the one reading a
    /// died-in-its-hooks create also produces. So the sibling worth skipping for
    /// and the sibling worth *not* skipping for look identical from `devpod
    /// status`, and the wait is what makes the second one likely: whatever this
    /// launch queued behind is the process whose create just failed.
    ///
    /// Without this, the guard on the fast-attach arm is reachable around: launch
    /// A's create dies leaving the container up, launch B waits out A's lock, sees
    /// `Running`, skips its own `up` and attaches — as root, into the container A
    /// never finished. Exactly the bug, one lock contention away.
    #[test]
    fn a_contended_up_does_not_skip_for_a_sibling_whose_create_died() {
        let scene = Scene::new()
            .with_running("myws")
            .with_create_aborted("myws");
        let provision = RecordingProvision::default();
        let request = UpRequest::new(
            "owner/repo",
            Naming::Create {
                workspace_id: "myws",
            },
        );

        let (outcome, notices) = contended_up(&scene, &request, &provision);

        assert_eq!(outcome, Ok(UpOutcome::Started));
        assert!(
            scene
                .devpod_commands()
                .iter()
                .any(|argv| argv.first().map(String::as_str) == Some("up")),
            "the loser must run the `up` the winner never finished: {:?}",
            scene.devpod_commands()
        );
        assert!(
            !notices.contains(&LaunchNotice::BroughtUpBySibling {
                workspace_id: "myws".to_owned()
            }),
            "a sibling that never finished brought nothing up"
        );
    }

    /// The other side, so the check above cannot be satisfied by never skipping:
    /// a sibling that *did* finish is still skipped for, and that skip is the
    /// whole point of the wait.
    #[test]
    fn a_contended_up_still_skips_for_a_sibling_whose_create_finished() {
        let scene = Scene::new()
            .with_running("myws")
            .with_create_completed("myws");
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
        // A top-up, and nothing on this path may say otherwise: no `devpod up` was
        // run here, so nothing restarted the container and its hostname is the one
        // the sibling's `up` set. That is what lets the pass be answered from the
        // host's own records rather than from a round trip.
        assert_eq!(provision.occasions(), vec![PassOccasion::TopUp]);
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
            &ClaudeSeen::new(),
            &NoProvisioning,
            &request,
            None,
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
            &ClaudeSeen::new(),
            &provision,
            &request,
            None,
            &mut no_notices(),
        );

        assert_eq!(outcome, Ok(UpOutcome::Started));
        assert_eq!(provision.provisioned(), vec!["myws".to_owned()]);
    }

    #[test]
    fn the_pass_after_an_up_is_told_the_container_just_restarted() {
        // The half of the scope that cannot be skipped, pinned at the call site
        // that decides it. `sudo hostname` is a stage of the pass and the name it sets
        // lives in the container's UTS namespace, which docker rebuilds from the
        // container's config on every start -- so the pass following *this* launch's
        // own `devpod up` has work to do whatever the host remembers about the
        // tools, and `AfterUp` is how it is told so.
        let scene = Scene::new();
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
            &ClaudeSeen::new(),
            &provision,
            &request,
            None,
            &mut no_notices(),
        );

        assert_eq!(outcome, Ok(UpOutcome::Started));
        assert_eq!(provision.occasions(), vec![PassOccasion::AfterUp]);
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
            &ClaudeSeen::new(),
            &provision,
            &request,
            None,
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
            &ClaudeSeen::new(),
            &NoProvisioning,
            &request,
            None,
            &mut no_notices(),
        );

        assert_eq!(outcome, Err(NotRun::NotInstalled));
    }

    // ----------------------------------------- the copy of the volume names

    /// A pass that reports what devlaunch's kept copy said at the moment it ran.
    ///
    /// The ordering is the whole of what this seam is about, and a test that only
    /// looked at the cache afterwards could not tell "written before the pass" from
    /// "written after it". This can: the pass reads the copy from inside itself.
    struct CopyWatchingProvision {
        copies: KeptCopies,
        seen: Mutex<Vec<Option<Vec<String>>>>,
    }

    impl CopyWatchingProvision {
        fn over(cache_dir: &Path) -> Self {
            Self {
                copies: KeptCopies::under(cache_dir),
                seen: Mutex::new(Vec::new()),
            }
        }

        /// What the copy said on each pass, in order.
        fn seen(&self) -> Vec<Option<Vec<String>>> {
            self.seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Provision for CopyWatchingProvision {
        fn provision_tools(
            &self,
            _runner: &dyn Runner,
            workspace_id: &str,
            _occasion: PassOccasion,
            _title: Option<&str>,
        ) -> Result<Option<ClaudeConfig>, DevpodMissing> {
            let named = self
                .copies
                .volumes(workspace_id)
                .map(|names| names.iter().cloned().collect());
            self.seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(named);
            Ok(None)
        }

        fn remembered_claude(&self, _workspace_id: &str) -> Option<ClaudeConfig> {
            None
        }
    }

    /// The names in the copy this cache holds for `workspace_id`.
    fn copied_volumes(cache_dir: &Path, workspace_id: &str) -> Option<Vec<String>> {
        KeptCopies::under(cache_dir)
            .volumes(workspace_id)
            .map(|names| names.iter().cloned().collect())
    }

    /// **Before the provisioning pass, not after it.** Provisioning can fail and
    /// take the launch with it while the container and its volumes stand, and a
    /// copy written on the other side of that would be the one launch whose volumes
    /// nothing ever names.
    #[test]
    fn a_completed_up_copies_the_volume_names_before_the_provisioning_pass() {
        let scene = Scene::new().with_substitutions("myws", "/repos/o/r/repo-main-ab12", "abcdef");
        let provision = CopyWatchingProvision::over(scene.cache_dir());
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
            &ClaudeSeen::new(),
            &provision,
            &request,
            None,
            &mut no_notices(),
        );

        assert_eq!(outcome, Ok(UpOutcome::Started));
        assert_eq!(
            provision.seen(),
            vec![Some(vec![
                "repo-main-ab12-pixi".to_owned(),
                "dind-var-lib-docker-abcdef".to_owned(),
            ])],
            "the copy has to be there by the time the pass runs"
        );
    }

    /// The other arm of the same `up`, and it is not optional: the sibling this
    /// launch waited out may not have been `dl` at all, and `--purge` would
    /// otherwise destroy a foreign workspace's copy while leaving its volumes
    /// standing (devlaunch#452). The copy claims "dl brought this up", never "dl
    /// created it".
    #[test]
    fn a_sibling_that_won_the_race_still_leaves_this_launch_a_copy() {
        let scene = Scene::new().with_running("myws").with_substitutions(
            "myws",
            "/repos/o/r/repo-main-ab12",
            "abcdef",
        );
        let provision = CopyWatchingProvision::over(scene.cache_dir());
        let request = UpRequest::new(
            "owner/repo",
            Naming::Create {
                workspace_id: "myws",
            },
        );

        let (outcome, _) = contended_up(&scene, &request, &provision);

        assert_eq!(outcome, Ok(UpOutcome::SkippedSiblingWon));
        assert_eq!(
            provision.seen(),
            vec![Some(vec![
                "repo-main-ab12-pixi".to_owned(),
                "dind-var-lib-docker-abcdef".to_owned(),
            ])]
        );
    }

    /// An `up` devpod refused is not a completed `up`, so there is nothing devpod
    /// wrote for this launch to copy. Whatever the cache already held is left
    /// exactly as it was.
    #[test]
    fn an_up_devpod_refused_copies_nothing() {
        let scene = Scene::new();
        scene
            .runner
            .script(["devpod", "up"], Response::failed(1, "no such provider\n"));
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
            &ClaudeSeen::new(),
            &NoProvisioning,
            &request,
            None,
            &mut no_notices(),
        );

        assert!(
            matches!(outcome, Ok(UpOutcome::Refused { .. })),
            "{outcome:?}"
        );
        assert_eq!(copied_volumes(scene.cache_dir(), "myws"), None);
    }

    /// An `up` that finished without devpod recording a create result names
    /// nothing, and this is where that case arrives: a create that died in its
    /// lifecycle hooks. No copy, and the launch is otherwise untouched.
    #[test]
    fn an_up_devpod_recorded_nothing_for_copies_nothing() {
        let scene = Scene::new();
        let provision = CopyWatchingProvision::over(scene.cache_dir());
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
            &ClaudeSeen::new(),
            &provision,
            &request,
            None,
            &mut no_notices(),
        );

        assert_eq!(outcome, Ok(UpOutcome::Started));
        assert_eq!(provision.seen(), vec![None]);
        assert_eq!(copied_volumes(scene.cache_dir(), "myws"), None);
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
        //
        // The values are the literal
        // `provision::tests::the_wrap_and_the_stage_read_one_signal` walks, imported
        // rather than repeated so the two tests cannot come to walk different lists:
        // this pins what the wrap reads, that pins that the stage reads the same.
        for denial in crate::flows::provision::ZELLIJ_DENIALS {
            let host = Host {
                zellij: Some(denial.to_owned()),
                ..Host::default()
            };
            assert_eq!(ZellijWrap::from_host(&host), ZellijWrap::Off, "{denial:?}");
        }
        for consent in crate::flows::provision::ZELLIJ_CONSENTS {
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

    // ---------------------------------------------------- the terminal title

    /// A host with a terminal on stderr and no opinion about titles.
    fn titling() -> Host {
        Host {
            stderr_tty: true,
            ..Host::default()
        }
    }

    #[test]
    fn the_terminal_is_named_after_the_workspace_with_osc_2() {
        // OSC 2 and BEL-terminated, which is the pair every multiplexer in scope
        // reads: zellij and tmux take it as the pane title, a bare terminal as the
        // window title. The id and nothing else -- no `dl ` prefix, because a tab
        // bar's columns are the scarce thing being spent here.
        let title = TerminalTitle::from_host(&titling(), "devlaunch-main-3j1t");

        assert_eq!(title.osc(), Some("\x1b]2;devlaunch-main-3j1t\x07"));
    }

    #[test]
    fn no_terminal_on_stderr_means_no_title() {
        // The guard is stderr and not stdout because stderr is the stream written
        // to. `dl <ws> -- make test > log` keeps its terminal and still gets named;
        // a run whose stderr is a pipe would only be writing escapes into somebody
        // else's capture.
        let piped = Host {
            stderr_tty: false,
            ..Host::default()
        };

        assert_eq!(
            TerminalTitle::from_host(&piped, "devlaunch-main-3j1t"),
            TerminalTitle::Off
        );
    }

    #[test]
    fn the_title_switch_reads_the_same_denials_as_the_others() {
        // A "no" variable, so consent is the default and this is the only one of
        // the family whose *absence* means on. The vocabulary is still shared:
        // `DEVLAUNCH_NO_TITLE=0` is what somebody who once turned it off writes to
        // turn it back on.
        for denial in ["", "0", "false", "no", "FALSE", " no "] {
            let host = Host {
                no_title: Some(denial.to_owned()),
                ..titling()
            };
            assert!(host_titles(&host), "{denial:?}");
        }
        for consent in ["1", "yes", "true"] {
            let host = Host {
                no_title: Some(consent.to_owned()),
                ..titling()
            };
            assert!(!host_titles(&host), "{consent:?}");
        }
        assert!(host_titles(&titling()), "unset names the terminal");
    }

    /// Whether this host would write a title at all.
    fn host_titles(host: &Host) -> bool {
        TerminalTitle::from_host(host, "devlaunch-main-3j1t")
            .osc()
            .is_some()
    }

    #[test]
    fn a_spec_cannot_smuggle_a_second_escape_into_the_title() {
        // `Plan::Existing` carries the raw spec through unvalidated -- devpod is
        // what eventually refuses a name it does not have -- and the title is
        // written before the session that would do the refusing. So the controls
        // come out here, or a `dl $'x\x1b]2;...'` writes its own title after dl's.
        let title = TerminalTitle::from_host(&titling(), "ws\x1b]2;pwned\x07x\nrest");

        assert_eq!(title.osc(), Some("\x1b]2;ws]2;pwnedxrest\x07"));
        // DEL and the 8-bit String Terminator, the other two ways to end an OSC.
        // `char::is_control` is the Cc category and covers both, which is why the
        // filter does not name them.
        assert_eq!(
            TerminalTitle::from_host(&titling(), "ws\x7fx\u{9c}y").osc(),
            Some("\x1b]2;wsxy\x07")
        );
    }

    #[test]
    fn a_name_cannot_smuggle_a_command_into_the_prompt_that_repaints_it() {
        // The container half of the title is assigned into `PS1`, and bash expands
        // `PS1` again at every prompt -- `promptvars`, on by default -- so quoting
        // the assignment makes the name text only until the first prompt renders it.
        // `$`, a backtick and a backslash are what that render acts on, and the two
        // arms that title after a bare devpod name or a path leaf never had them
        // refused. A derived id holds none of the three, so nothing legitimate is
        // lost by dropping them at the same boundary the controls go.
        assert_eq!(
            sanitize_title("ws$(id)`id`\\u"),
            Some("ws(id)idu".to_owned())
        );
        // The whole point: what survives cannot be re-read as an instruction.
        let filtered = sanitize_title("$(touch /tmp/pwned)").expect("a title");
        assert!(!filtered.contains(['$', '`', '\\']), "{filtered}");
    }

    #[test]
    fn a_name_that_is_nothing_but_controls_is_no_title_at_all() {
        // Sanitising down to the empty string must not write `ESC ] 2 ; BEL`, which
        // would blank the terminal's name rather than leave it alone.
        assert_eq!(
            TerminalTitle::from_host(&titling(), "\x1b\x07 \n"),
            TerminalTitle::Off
        );
    }

    // ------------------------------------------------------- the herdr tab

    #[test]
    fn a_herdr_pane_gets_its_tab_named_as_well_as_its_title() {
        // Measured on a live herdr 0.8.2: `dl rocker@nb1` in a herdr pane left the
        // pane's `terminal_title` reading `rocker@nb1` -- the OSC arrived, intact --
        // while `herdr tab list` still reported that tab's label as `4`. A tab label
        // is a field of its own (`custom_name` in herdr's session.json), and no
        // escape sequence addresses it; `herdr tab rename` is the only thing that
        // does. So the escape names the pane and this names the tab, and the two are
        // the same name.
        let host = Host {
            herdr_tab_id: Some("w8:t7".to_owned()),
            ..titling()
        };

        assert_eq!(
            HerdrTabRename::from_host(&host, "rocker@nb1").argv(),
            Some(vec!["herdr", "tab", "rename", "w8:t7", "rocker@nb1"])
        );
    }

    #[test]
    fn the_herdr_that_spawned_this_pane_is_the_one_asked_to_rename_the_tab() {
        // `HERDR_BIN_PATH` beside the tab id, and both are herdr's own exports. The
        // socket a rename lands on belongs to the server that spawned this pane, so
        // the binary that server shipped is the one that speaks its protocol -- and
        // a herdr installed per-environment (pixi here) need not be the `herdr` a
        // bare `PATH` lookup would find, or need not be on `PATH` at all.
        let host = Host {
            herdr_tab_id: Some("w8:tB".to_owned()),
            herdr_bin: Some("/opt/pixi/envs/herdr/bin/herdr".to_owned()),
            ..titling()
        };

        assert_eq!(
            HerdrTabRename::from_host(&host, "devlaunch@nb3").argv(),
            Some(vec![
                "/opt/pixi/envs/herdr/bin/herdr",
                "tab",
                "rename",
                "w8:tB",
                "devlaunch@nb3"
            ])
        );
    }

    #[test]
    fn a_label_starting_with_a_dash_is_sent_bare_because_the_separator_would_be_named() {
        // `sanitize_title` keeps a leading dash. It takes out the controls the
        // escape fears and the three characters `PS1` would re-expand, and a dash
        // is none of them -- and the two arms that title without deriving an id,
        // `Plan::Existing`'s raw spec and `Plan::Creatable`'s path leaf, are
        // exactly where one arrives from. So `dl ./-odd` reaches here as `-odd`.
        //
        // The reflex is to guard it with `--`, and that is the bug rather than the
        // fix. herdr's `<LABEL>...` is variadic and joins everything it collects,
        // separator included: sending `-- devlaunch@nb3` to herdr 0.8.2 and reading
        // the tab back gives the label `-- devlaunch@nb3`. Measured, not reasoned
        // -- `tab rename <id> -- normal` returns `tab_not_found` like any other
        // call, so the argument parses and only the resulting name is wrong, which
        // is invisible to anything short of looking at the tab.
        //
        // Stripping the dash instead is the one repair that is not available: the
        // tab would be named something the escape did not name the pane.
        let host = Host {
            herdr_tab_id: Some("w8:t7".to_owned()),
            ..titling()
        };

        assert_eq!(
            HerdrTabRename::from_host(&host, "-odd").argv(),
            Some(vec!["herdr", "tab", "rename", "w8:t7", "-odd"]),
            "a separator here would be joined into the label by herdr"
        );
    }

    #[test]
    fn a_pane_outside_herdr_has_no_tab_to_name_and_asks_for_nothing() {
        // The whole stage is one environment lookup that usually finds nothing, and
        // finding nothing must cost a launch no process. This is the arm every
        // launch on a machine without herdr takes.
        assert_eq!(
            HerdrTabRename::from_host(&titling(), "devlaunch@nb3"),
            HerdrTabRename::Off
        );
    }

    #[test]
    fn an_exported_but_empty_tab_id_is_not_a_tab() {
        // What a shell leaves behind when something upstream failed to compute it.
        // Handing `""` to `herdr tab rename` would be asking to rename a tab that
        // cannot exist, so it reads as absent rather than as a target.
        for blank in ["", "   "] {
            let host = Host {
                herdr_tab_id: Some(blank.to_owned()),
                ..titling()
            };
            assert_eq!(
                HerdrTabRename::from_host(&host, "devlaunch@nb3"),
                HerdrTabRename::Off,
                "{blank:?}"
            );
        }
    }

    /// A host in a herdr pane, with a terminal, and no opinion about titles.
    fn herding() -> Host {
        Host {
            herdr_tab_id: Some("w8:tB".to_owned()),
            ..titling()
        }
    }

    #[test]
    fn the_title_switch_takes_the_herdr_tab_with_it() {
        // One feature and not three. `DEVLAUNCH_NO_TITLE=1` is how somebody says
        // "do not name my terminal after the workspace", and a tab strip that kept
        // saying it would be the same feature ignoring the same switch. Off for the
        // same reason the container's `PS1` line is off: they share `naming_gate`,
        // so this cannot drift from the escape's own answer.
        let opted_out = Host {
            no_title: Some("1".to_owned()),
            ..herding()
        };

        assert_eq!(
            HerdrTabRename::from_host(&opted_out, "x"),
            HerdrTabRename::Off
        );
        // And the shared gate means the two are never in disagreement about it.
        assert_eq!(
            TerminalTitle::from_host(&opted_out, "x"),
            TerminalTitle::Off
        );
    }

    #[test]
    fn no_terminal_on_stderr_means_no_tab_name_either() {
        // `dl <ws> -- cmd` in a pipeline is a run nobody is watching a tab bar for,
        // and the tty check is the existing way that is said. Sharing it keeps one
        // answer to "is there a terminal worth naming" rather than two.
        let piped = Host {
            stderr_tty: false,
            ..herding()
        };

        assert_eq!(HerdrTabRename::from_host(&piped, "x"), HerdrTabRename::Off);
    }

    #[test]
    fn the_tab_and_the_pane_are_given_the_one_name() {
        // The guarantee the whole shape exists for, and the one a reader of a tab
        // bar notices the absence of: a tab reading one thing while the pane inside
        // it reads another is worse than a tab reading its number.
        //
        // Asserted on a name that the shared filter actually changes, so this would
        // catch a second sanitiser as well as a second derivation. Controls come out
        // for the escape's sake and `$` for `PS1`'s; argv needs neither, and takes
        // them anyway, because one name is worth more than three characters no
        // derived workspace id can hold.
        let raw = "ws$(id)\x1b]2;pwned\x07@main";

        let tab = HerdrTabRename::from_host(&herding(), raw);
        let label = match &tab {
            HerdrTabRename::Run { label, .. } => label.clone(),
            HerdrTabRename::Off => panic!("a herdr pane with a terminal names its tab"),
        };
        let osc = TerminalTitle::from_host(&herding(), raw)
            .osc()
            .expect("the same host writes an escape")
            .to_owned();

        assert_eq!(osc, format!("\x1b]2;{label}\x07"));
        // Spelled out once, so a change to the filter has to be looked at rather
        // than just re-derived on both sides of an equality.
        assert_eq!(label, "ws(id)]2;pwned@main");
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

    /// `terminal_for` with no context options cached, which is every host that
    /// has not run a `devpod up` through dl this hour.
    fn terminal_here(host: &Host, workspace_id: &str) -> Terminal {
        terminal_for(host, &ContextOptions::default(), workspace_id)
    }

    /// The file `on_a_terminal` published the aliases into.
    fn published_config(host: &Host) -> PathBuf {
        PathBuf::from(
            host.devpod_ssh_config
                .clone()
                .expect("on_a_terminal named one"),
        )
    }

    #[test]
    fn the_config_devpod_ssh_config_names_is_where_dl_looks_for_the_alias() {
        // The shipped bug (devlaunch#421), stated where a user would meet it:
        // `devpod up` publishes into `$DEVPOD_SSH_CONFIG` and creates no
        // `~/.ssh/config` at all, and dl answered `NoAlias`, dropped to the
        // transport with no pty, and blamed the workspace for it. Red on
        // origin/main: `Host::from_process` never read that variable and
        // `config_path()` was `~/.ssh/config` and nothing else.
        let scene = Scene::new().on_a_terminal(&["myws"]);
        assert!(
            !scene.dir.path().join("home").join(".ssh").exists(),
            "the point of the test is that there is no ~/.ssh/config to find"
        );

        assert_eq!(
            terminal_here(&scene.host, "myws"),
            Terminal::Usable {
                config: published_config(&scene.host)
            }
        );
    }

    #[test]
    fn the_usable_state_carries_the_config_the_alias_was_found_in() {
        // The second half of devlaunch#421, and the reason `Usable` has a field.
        // Finding the alias and being able to *use* it stopped being one fact the
        // moment dl looked anywhere but `~/.ssh/config`: OpenSSH resolves an alias
        // through `getpwuid`'s config and reads no environment, so the path has to
        // reach the invocation. It reached nothing, because `Usable` was the one
        // arm that dropped it -- and `dl <ws> -- <cmd>` at a terminal exited 255
        // with `Could not resolve hostname` instead of running.
        let scene = Scene::new().on_a_terminal(&["myws"]);
        let published = published_config(&scene.host);

        let Terminal::Usable { config } = terminal_here(&scene.host, "myws") else {
            panic!("the alias is published, so this is Usable");
        };

        assert_eq!(config, published);
        // And it is the file the alias is actually in, not merely a path.
        assert_eq!(ssh::alias_in(&config, "myws"), ssh::Alias::Published);
    }

    #[test]
    fn the_home_default_is_still_where_dl_looks_when_nothing_names_a_config() {
        // devpod's own fallback, and the only case dl handled before.
        let scene = Scene::new().on_a_terminal_with_a_home_config(&["myws"]);

        assert_eq!(scene.host.devpod_ssh_config, None);
        assert_eq!(
            terminal_here(&scene.host, "myws"),
            Terminal::Usable {
                config: scene.dir.path().join("home").join(".ssh").join("config")
            }
        );
    }

    #[test]
    fn a_cached_context_option_names_the_config_and_no_round_trip_pays_for_it() {
        // The context options carry two of devpod's four candidate paths. That
        // reading them cannot cost a warm launch a `devpod context options`
        // (0.4-0.7s, devlaunch#393 — more than the pty transport is worth) is
        // settled by the signature rather than by an assertion: `terminal_for`
        // takes no `Runner`, so it has nothing to ask devpod with. What is left
        // for a test is that the two options are honoured at all, and in devpod's
        // order — `ssh::config_path`'s tests hold the order.
        let scene = Scene::new().on_a_terminal(&["myws"]);
        let named = scene
            .host
            .devpod_ssh_config
            .clone()
            .expect("on_a_terminal named one");
        let by_option = Host {
            devpod_ssh_config: None,
            ..scene.host.clone()
        };
        let options = ContextOptions::from_map(BTreeMap::from([(
            "SSH_CONFIG_PATH".to_owned(),
            named.clone(),
        )]));

        assert_eq!(
            terminal_for(&by_option, &options, "myws"),
            Terminal::Usable {
                config: PathBuf::from(&named)
            },
            "the SSH_CONFIG_PATH context option is a place devpod publishes"
        );
        assert_eq!(
            terminal_for(
                &by_option,
                &ContextOptions::from_map(BTreeMap::from([(
                    "SSH_CONFIG_INCLUDE_PATH".to_owned(),
                    named.clone()
                )])),
                "myws"
            ),
            Terminal::Usable {
                config: PathBuf::from(&named)
            },
            "and so is SSH_CONFIG_INCLUDE_PATH"
        );
    }

    #[test]
    fn the_ssh_config_is_read_from_the_cache_and_never_asked_for() {
        // The whole warm-path budget of the lookup: one cache file, no subprocess.
        // Written by whichever launch was going to ask devpod anyway, read here —
        // and `already_cached_options` takes no `Runner` either, so "never asked
        // for" is a fact about the signature and this is what it buys.
        let scene = Scene::new().on_a_terminal(&["myws"]);
        let named = scene
            .host
            .devpod_ssh_config
            .clone()
            .expect("on_a_terminal named one");
        let host = Host {
            devpod_ssh_config: None,
            ..scene.host.clone()
        };
        write_options_cache(
            &host.context_options_cache(),
            &BTreeMap::from([("SSH_CONFIG_PATH".to_owned(), named.clone())]),
        );

        let options = already_cached_options(&host, SystemTime::now());

        assert!(options.ssh_config_path().is_some());
        assert_eq!(
            terminal_for(&host, &options, "myws"),
            Terminal::Usable {
                config: PathBuf::from(&named)
            }
        );
    }

    #[test]
    fn a_terminal_and_an_alias_route_a_command_through_openssh() {
        // The regression: an interactive payload must not go through `--command`.
        // And the route carries the config with the payload, because the argv it
        // becomes needs both: re-deriving the path on the far side of `route` is
        // a second lookup that can disagree with the one that chose the transport.
        let scene = Scene::new().on_a_terminal(&["myws"]);
        let published = published_config(&scene.host);

        let terminal = terminal_here(&scene.host, "myws");
        assert_eq!(
            terminal,
            Terminal::Usable {
                config: published.clone()
            }
        );
        let payload = RemotePayload::wrap("claude", ZellijWrap::Off).expect("quotable");
        assert_eq!(
            route(Some(&payload), &terminal, "myws", &mut no_notices()),
            Route::Terminal {
                payload: &payload,
                config: &published
            }
        );
    }

    #[test]
    fn no_terminal_keeps_the_devpod_transport() {
        // Piped output must stay clean, so no pty and no escape sequences.
        let scene = Scene::new();

        assert_eq!(terminal_here(&scene.host, "myws"), Terminal::Absent);
    }

    #[test]
    fn the_tty_opt_out_forces_the_devpod_transport() {
        let scene = Scene::new().on_a_terminal(&["myws"]);
        let opted_out = Host {
            no_tty: Some("1".to_owned()),
            ..scene.host.clone()
        };

        assert_eq!(terminal_here(&opted_out, "myws"), Terminal::Absent);
    }

    #[test]
    fn a_missing_host_alias_falls_back_and_says_which_config_it_read() {
        // A workspace devpod never wrote an alias for still has to run the command.
        let scene = Scene::new().on_a_terminal(&["some-other-workspace"]);
        let config = PathBuf::from(
            scene
                .host
                .devpod_ssh_config
                .clone()
                .expect("on_a_terminal named one"),
        );
        let mut notices = no_notices();

        let terminal = terminal_here(&scene.host, "myws");
        assert_eq!(
            terminal,
            Terminal::NoAlias {
                config: config.clone()
            }
        );
        let payload = RemotePayload::wrap("claude", ZellijWrap::Off).expect("quotable");
        assert_eq!(
            route(Some(&payload), &terminal, "myws", &mut notices),
            Route::DevpodCommand(&payload)
        );
        assert_eq!(
            notices,
            vec![LaunchNotice::NoTerminalAlias {
                workspace_id: "myws".to_owned(),
                config
            }]
        );
    }

    #[test]
    fn no_config_at_all_is_its_own_state_and_names_where_dl_looked() {
        // The other half of the devlaunch#421 split. "devpod published nothing for
        // this workspace" and "dl is reading a file devpod never writes" used to
        // be one `NoAlias`, so the fix dl suggested — restart the workspace —
        // could not distinguish the case it fixes from the case it cannot touch.
        let scene = Scene::new().on_a_terminal(&["myws"]);
        let never_written = scene.dir.path().join("elsewhere").join("ssh_config");
        let host = Host {
            devpod_ssh_config: Some(never_written.display().to_string()),
            ..scene.host.clone()
        };
        let mut notices = no_notices();

        let terminal = terminal_here(&host, "myws");
        assert_eq!(
            terminal,
            Terminal::ConfigMissing {
                looked_in: never_written.clone()
            }
        );
        let payload = RemotePayload::wrap("claude", ZellijWrap::Off).expect("quotable");
        assert_eq!(
            route(Some(&payload), &terminal, "myws", &mut notices),
            Route::DevpodCommand(&payload)
        );
        assert_eq!(
            notices,
            vec![LaunchNotice::NoDevpodSshConfig {
                workspace_id: "myws".to_owned(),
                looked_in: never_written
            }]
        );
    }

    #[test]
    fn nowhere_to_look_is_its_own_state_too() {
        // Reachable: `XDG_CACHE_HOME` set and no home directory is a machine dl
        // runs on, and there is then no path to name in a notice at all.
        let scene = Scene::new().on_a_terminal(&["myws"]);
        let host = Host {
            devpod_ssh_config: None,
            home: None,
            ..scene.host.clone()
        };
        let mut notices = no_notices();

        let terminal = terminal_here(&host, "myws");
        assert_eq!(terminal, Terminal::ConfigUnlocatable);
        let payload = RemotePayload::wrap("claude", ZellijWrap::Off).expect("quotable");
        assert_eq!(
            route(Some(&payload), &terminal, "myws", &mut notices),
            Route::DevpodCommand(&payload)
        );
        assert_eq!(notices, vec![LaunchNotice::SshConfigUnlocatable]);
    }

    #[test]
    fn a_bare_attach_stays_on_devpod_however_good_the_terminal_is() {
        // `dl <ws>` with no command already gets a pty from devpod, which is the
        // one case devpod requests one for.
        let mut notices = no_notices();

        for terminal in [
            Terminal::Usable {
                config: PathBuf::from("/scratch/ssh_config"),
            },
            Terminal::NoAlias {
                config: PathBuf::from("/scratch/ssh_config"),
            },
            Terminal::ConfigMissing {
                looked_in: PathBuf::from("/scratch/ssh_config"),
            },
            Terminal::ConfigUnlocatable,
            Terminal::Absent,
        ] {
            assert_eq!(
                route(None, &terminal, "myws", &mut notices),
                Route::DevpodAttach,
                "{terminal:?}"
            );
        }
        assert_eq!(notices, no_notices(), "and nothing to report either");
    }

    // --------------------------------------------------------- the session

    /// One session over `scene`: what it ended as, what it reported, and what
    /// devpod said on its own stderr along the way.
    /// A scratch home holding a Claude credential, and the `Host` that points at it.
    fn with_claude_login(mut scene: Scene, token: &str) -> (Scene, tempfile::TempDir) {
        let home = tempfile::tempdir().expect("a scratch home");
        std::fs::create_dir_all(home.path().join(".claude")).expect("a config dir");
        std::fs::write(
            home.path().join(".claude/.credentials.json"),
            format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}"}}}}"#),
        )
        .expect("a credential");
        scene.host.home = Some(home.path().to_path_buf());
        (scene, home)
    }

    /// [`a_session`], with the Claude config ownership the pass would have observed.
    fn a_session_seeing(
        scene: &Scene,
        command: Option<&str>,
        seen: Option<ClaudeConfig>,
    ) -> Vec<String> {
        let token = HostToken::new();
        let mut notices = no_notices();
        let mut said = Vec::new();
        let claude_seen = ClaudeSeen::new();
        claude_seen.set(seen);
        let context = SessionContext::new(&scene.runner, &scene.host, &token, &claude_seen);
        let _ = workspace_ssh(
            &context,
            "myws",
            command,
            None,
            &mut |line| said.push(line.to_owned()),
            &mut notices,
        );
        scene
            .runner
            .calls_to("devpod")
            .into_iter()
            .find(|call| call.args().first().map(String::as_str) == Some("ssh"))
            .map(|call| {
                let mut argv = call.argv();
                // The value, so a test can assert it is *not* in argv and *is* in the
                // environment, without two helpers.
                argv.push(format!(
                    "env:{}",
                    call.invocation()
                        .env
                        .entries
                        .get(claude::TOKEN_VAR)
                        .cloned()
                        .unwrap_or_default()
                ));
                argv
            })
            .expect("a session")
    }

    #[test]
    fn a_container_whose_claude_config_is_its_own_gets_the_hosts_login() {
        // The reported bug, in one assertion: `dl kinisi-robotics/team-tracker` has no
        // devcontainer, so nothing mounts ~/.claude, so nothing carried a credential
        // and `claude` asked for a fresh login on every launch.
        let (scene, _home) =
            with_claude_login(Scene::new().with_running("myws"), "not-a-real-token");
        let argv = a_session_seeing(&scene, None, Some(ClaudeConfig::Ours));
        assert!(argv.contains(&claude::TOKEN_VAR.to_owned()), "{argv:?}");
        assert!(
            argv.contains(&"env:not-a-real-token".to_owned()),
            "{argv:?}"
        );
    }

    #[test]
    fn the_value_never_reaches_argv() {
        // The discipline this shares with the gh token: `ps` shows which variable is
        // being sent and never what is in it.
        let (scene, _home) =
            with_claude_login(Scene::new().with_running("myws"), "not-a-real-secret-token");
        let argv = a_session_seeing(&scene, None, Some(ClaudeConfig::Ours));
        assert!(
            !argv
                .iter()
                .filter(|arg| !arg.starts_with("env:"))
                .any(|arg| arg.contains("secret")),
            "{argv:?}"
        );
    }

    #[test]
    fn a_container_that_mounted_the_hosts_claude_config_is_left_alone() {
        // A repo whose own devcontainer bind-mounts ~/.claude has a credential that
        // can refresh itself. Claude Code prefers the variable over the file, so
        // forwarding here would *replace* that with a short-lived token -- worse than
        // doing nothing, in the one case devlaunch has nothing to add.
        let (scene, _home) =
            with_claude_login(Scene::new().with_running("myws"), "not-a-real-token");
        let argv = a_session_seeing(&scene, None, Some(ClaudeConfig::Foreign));
        assert!(!argv.contains(&claude::TOKEN_VAR.to_owned()), "{argv:?}");
        assert!(argv.contains(&"env:".to_owned()), "{argv:?}");
    }

    #[test]
    fn a_pass_that_learned_nothing_forwards_nothing() {
        // `None` is not `Ours`. A probe that could not answer is not evidence that the
        // directory is the container's.
        let (scene, _home) =
            with_claude_login(Scene::new().with_running("myws"), "not-a-real-token");
        let argv = a_session_seeing(&scene, None, None);
        assert!(!argv.contains(&claude::TOKEN_VAR.to_owned()), "{argv:?}");
    }

    #[test]
    fn the_opt_out_reaches_a_workspace_with_no_login_rather_than_failing() {
        // DEVLAUNCH_NO_CLAUDE_TOKEN=1 is a choice, and the session still opens.
        let (mut scene, _home) =
            with_claude_login(Scene::new().with_running("myws"), "not-a-real-token");
        scene.host.claude = claude::HostEnv {
            disable: Some("1".to_owned()),
            ..claude::HostEnv::default()
        };
        let argv = a_session_seeing(&scene, None, Some(ClaudeConfig::Ours));
        assert!(!argv.contains(&claude::TOKEN_VAR.to_owned()), "{argv:?}");
    }

    #[test]
    fn a_host_that_never_logged_in_still_opens_the_session() {
        // No credential file at all: the macOS case and the never-ran-claude case. The
        // workspace opens, with no Claude login and no failure.
        let mut scene = Scene::new().with_running("myws");
        let home = tempfile::tempdir().expect("a scratch home");
        scene.host.home = Some(home.path().to_path_buf());
        let argv = a_session_seeing(&scene, None, Some(ClaudeConfig::Ours));
        assert!(!argv.contains(&claude::TOKEN_VAR.to_owned()), "{argv:?}");
        assert!(argv.contains(&"ssh".to_owned()), "{argv:?}");
    }

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
        let claude_seen = ClaudeSeen::new();
        let context = SessionContext::new(&scene.runner, &scene.host, &token, &claude_seen);
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

    /// The wiring `clients::herdr`'s own tests cannot reach.
    ///
    /// Its unit tests know that `claude` is an agent and that the name belongs in
    /// the environment rather than in argv; only this one knows the launch flow
    /// actually asks. The environment is where it has to land: a session manager
    /// reads `/proc/<pid>/environ` of the pane's processes, and argv is also the
    /// one place another user on the host could read it from.
    #[test]
    fn a_command_that_names_an_agent_names_it_to_the_ssh_child() {
        let scene = Scene::new().on_a_terminal(&["myws"]).with_running("myws");

        let _ = a_session(&scene, Some("claude 'fix the bug'"));

        let calls = scene.runner.calls_to("ssh");
        let call = calls.last().expect("an openssh session");
        assert_eq!(
            call.invocation()
                .env
                .entries
                .get(herdr::AGENT_VAR)
                .map(String::as_str),
            Some("claude"),
        );
        assert!(
            !call.argv().iter().any(|arg| arg.contains(herdr::AGENT_VAR)),
            "the name must not travel in argv: {:?}",
            call.argv()
        );
    }

    /// The other half, and the one that keeps a manager honest: a command that is
    /// not an agent must leave the launch exactly as it was, rather than claiming
    /// the pane holds an agent nothing has detection rules for.
    #[test]
    fn an_ordinary_command_names_no_agent() {
        let scene = Scene::new().on_a_terminal(&["myws"]).with_running("myws");

        let _ = a_session(&scene, Some("make test"));

        let calls = scene.runner.calls_to("ssh");
        let call = calls.last().expect("an openssh session");
        assert_eq!(call.invocation().env.entries.get(herdr::AGENT_VAR), None);
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

            let claude_seen = ClaudeSeen::new();
            let context = SessionContext::new(&scene.runner, &scene.host, &token, &claude_seen);
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
        //
        // And the argv names the config the alias was read out of. This is the
        // whole of `workspace_ssh` measured at once, and it is the assertion that
        // was missing: devlaunch#421's first half taught dl to *read* devpod's real
        // config, and the argv still said `ssh -t myws.devpod` with no `-F` -- so on
        // a host publishing anywhere but `~/.ssh/config`, dl decided it had a
        // terminal and then handed OpenSSH an alias OpenSSH could not resolve.
        // `on_a_terminal` publishes into `$DEVPOD_SSH_CONFIG` and leaves no
        // `~/.ssh/config`, which is exactly that host.
        let scene = Scene::new().on_a_terminal(&["myws"]).with_running("myws");
        let published = published_config(&scene.host);
        let socket = ssh::Reuse::derive(&scene.host.ssh_control_dir(), "myws", &[], None);
        let ssh::Reuse::Multiplexed(socket) = socket else {
            panic!("a scratch cache is short enough to multiplex through");
        };

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
                "-F".to_owned(),
                published.display().to_string(),
                "-t".to_owned(),
                "-o".to_owned(),
                "ControlMaster=auto".to_owned(),
                "-o".to_owned(),
                format!("ControlPath={}", socket.as_path().display()),
                "-o".to_owned(),
                "ControlPersist=60".to_owned(),
                "myws.devpod".to_owned(),
                "bash -lc claude".to_owned(),
            ]]
        );
        // Not merely "a -F": the file named is the one holding the alias, and it
        // is not the home default OpenSSH would have gone to on its own.
        assert_eq!(ssh::alias_in(&published, "myws"), ssh::Alias::Published);
        assert_ne!(
            published,
            scene.dir.path().join("home").join(".ssh").join("config")
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
            &ClaudeSeen::new(),
            &NoProvisioning,
            &request,
            None,
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
            &ClaudeSeen::new(),
            &NoProvisioning,
            &UpRequest::new(
                "myws",
                Naming::Create {
                    workspace_id: "myws",
                },
            ),
            None,
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
    fn a_run_with_a_token_and_a_run_without_never_share_a_master() {
        // devlaunch#389's silent failure, closed at the flow rather than at the
        // digest: a master opened by the run with no token filters `SendEnv`
        // against its own empty permit list, so the run *with* a token would get
        // an empty `GH_TOKEN` and an unauthenticated `gh` at exit 0. The two runs
        // cannot find each other's socket, so the state has nowhere to happen.
        //
        // The file names are compared rather than the whole paths, because each
        // scene has a scratch cache of its own; the name is the key.
        let with_token = logged_in(Scene::new().on_a_terminal(&["myws"]).with_running("myws"));
        let without = Scene::new().on_a_terminal(&["myws"]).with_running("myws");

        let _ = a_session(&with_token, Some("claude"));
        let _ = a_session(&without, Some("claude"));

        let keyed = |scene: &Scene| -> String {
            let argv = scene
                .runner
                .argvs()
                .into_iter()
                .next()
                .expect("an ssh call");
            let path = argv
                .iter()
                .find_map(|arg| arg.strip_prefix("ControlPath="))
                .unwrap_or_else(|| panic!("no ControlPath in {argv:?}"))
                .to_owned();
            Path::new(&path)
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a socket name")
                .to_owned()
        };

        assert!(
            keyed(&with_token) != keyed(&without),
            "a permit list of [GH_TOKEN] and one of [] share a master, so the \
             forwarded token would be filtered away in silence"
        );
    }

    #[test]
    fn the_control_socket_lives_under_devlaunchs_own_cache_directory() {
        // Under `XDG_CACHE_HOME` like the rest of dl's storage, in a leaf of its
        // own so the cache's walkers do not read it as a repo, and `dl --purge`
        // takes it away with everything else.
        let scene = Scene::new().on_a_terminal(&["myws"]).with_running("myws");

        let _ = a_session(&scene, Some("claude"));

        let argv = scene
            .runner
            .argvs()
            .into_iter()
            .next()
            .expect("an ssh call");
        let path = argv
            .iter()
            .find_map(|arg| arg.strip_prefix("ControlPath="))
            .unwrap_or_else(|| panic!("no ControlPath in {argv:?}"));
        assert_eq!(
            Path::new(path).parent(),
            Some(scene.host.ssh_control_dir().as_path())
        );
        assert!(scene.host.ssh_control_dir().is_dir());
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
            &ClaudeSeen::new(),
            &NoProvisioning,
            &UpRequest::new(
                "myws",
                Naming::Create {
                    workspace_id: "myws",
                },
            ),
            None,
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
    fn the_retry_cannot_turn_a_source_dir_that_is_not_a_repo_into_an_empty_one() {
        // `chezmoi init` with no repo argument `git init`s the source directory
        // when it is not already a repository, and the repo it makes has no
        // upstream -- so `update` fails with `no tracking information`, forever,
        // because every later refresh finds a repository and `init` is a no-op.
        // An actionable `not a git repository` becomes a permanent failure with
        // its own diagnosis destroyed. Verified against chezmoi 2.72.
        //
        // So the retry asks first, and the question is asked of the source
        // directory chezmoi itself names rather than a path spelled here.
        let command = dotfiles_command(None, None);

        let guard = command
            .find("rev-parse --git-dir")
            .expect("the retry is guarded on the source dir being a repository");
        let reinit = command
            .find("chezmoi init")
            .expect("the retry still re-initialises");
        assert!(
            guard < reinit,
            "the guard has to run before `init`, or `init` has already made the repo"
        );
        assert!(
            command.contains("chezmoi source-path"),
            "asking chezmoi where its source is, rather than hardcoding a path -- \
             CLAUDE.md's rule that no /home/<user> path is written down"
        );

        // And the notice is inside the guard, so a network failure or a dirty
        // source tree is no longer announced as a config problem.
        let notice = command
            .find("Re-initialising")
            .expect("the retry still says what it is doing");
        assert!(
            guard < notice,
            "the notice claims a diagnosis the guard makes"
        );
    }

    #[test]
    fn the_re_init_answers_its_own_config_prompts() {
        // `--force` is chezmoi's "make all changes without prompting", which is
        // about changes and not about the `prompt*` functions a config template
        // calls. Without `--promptDefaults` the retry cannot fix its own
        // motivating case: a dotfiles repo that adds a `promptString` variable
        // makes `init` ask for it, and this runs with nobody watching -- dying on
        // `could not open a new TTY` where there is no terminal, and blocking
        // where there is one.
        let command = dotfiles_command(None, None);
        let init = command
            .split("chezmoi init")
            .nth(1)
            .expect("the retry re-initialises");
        let init = init.split("&&").next().expect("the init call's own words");

        assert!(
            init.contains("--promptDefaults"),
            "a prompt function must return its default: {init}"
        );
        assert!(
            init.contains("--no-tty"),
            "a prompt with no default must be a fast error, not a hang inside the \
             bound: {init}"
        );
    }

    #[test]
    fn a_refresh_that_cannot_apply_re_renders_its_config_and_tries_again() {
        // A workspace's chezmoi config is rendered once, at create. When the
        // dotfiles repo later adds a template variable, the pull succeeds and
        // every apply after it dies on `map has no entry for key` -- so the
        // command meant to bring a stale workspace forward was the one command
        // that could not, and no number of repeats helped. The retry must run
        // `init` *after* a failed `update`, because the template naming the new
        // variable arrives in that update's pull.
        let command = dotfiles_command(Some("https://example/dots"), None);

        let first = command
            .find("chezmoi update --force")
            .expect("the refresh still updates");
        let reinit = command
            .find("chezmoi init --force")
            .expect("a failed update re-renders the config");
        let retry = command[reinit..]
            .find("chezmoi update --force")
            .map(|at| at + reinit)
            .expect("and applies again with it");

        assert!(first < reinit, "the pull comes first: {command}");
        assert!(reinit < retry, "then the config, then the apply: {command}");
        assert!(
            command[first..reinit].contains("||"),
            "the re-init is the failure branch, not an unconditional second step: {command}"
        );
        // The sync is downstream of the whole attempt: a workspace that could not
        // apply its dotfiles has no new manifest worth syncing.
        assert!(
            retry < command.find("pixi global sync").expect("and then syncs"),
            "{command}"
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

    #[test]
    fn the_handover_names_the_terminal_before_the_session_takes_it() {
        // Ordering is the whole property. After the session starts, this process
        // may not print again for hours -- so a title said even one notice late is
        // a title that arrives when the work is over. It goes in front of the
        // dotfiles refresh too, which is why it is the first thing the attach does
        // rather than the last thing before the ssh.
        let mut scene = Scene::new().with_running("myws");
        scene.host.stderr_tty = true;
        scene.host.dotfiles_on_attach = Some("1".to_owned());
        let token = HostToken::new();
        let mut notices = Vec::new();

        let _ = attaching(&scene, &token, "myws", None, &mut notices);

        assert_eq!(
            notices.first(),
            Some(&LaunchNotice::TerminalTitle(TerminalTitle::Write(
                "\x1b]2;myws\x07".to_owned()
            ))),
            "{notices:?}"
        );
    }

    #[test]
    fn a_handover_that_names_no_terminal_still_says_so() {
        // `Off` is said rather than skipped, so the sink is the one place that
        // decides nothing gets written. A caller that filtered here would be the
        // second place deciding it, and the two would drift.
        let scene = Scene::new().with_running("myws");
        let token = HostToken::new();
        let mut notices = Vec::new();

        let _ = attaching(&scene, &token, "myws", None, &mut notices);

        assert_eq!(
            notices.first(),
            Some(&LaunchNotice::TerminalTitle(TerminalTitle::Off)),
            "{notices:?}"
        );
    }

    #[test]
    fn the_handover_names_the_herdr_tab_beside_the_terminal_it_names() {
        // Beside the escape and in front of the session, for the escape's own
        // reason: after the ssh takes the terminal this process may not run again
        // for hours, and a tab named then is a tab named after the work is over.
        //
        // Second rather than first because the escape is the one with the deadline
        // -- it is racing the shell prompt that repaints the title -- while a tab
        // label is written once and stays. Neither is allowed to wait for the
        // dotfiles refresh, which is why both sit in front of it.
        let mut scene = Scene::new().with_running("myws");
        scene.host.stderr_tty = true;
        scene.host.herdr_tab_id = Some("w8:tB".to_owned());
        scene.host.dotfiles_on_attach = Some("1".to_owned());
        let token = HostToken::new();
        let mut notices = Vec::new();

        let _ = attaching(&scene, &token, "myws", None, &mut notices);

        assert_eq!(
            notices.get(1),
            Some(&LaunchNotice::HerdrTab(HerdrTabRename::Run {
                bin: HERDR_BIN_FALLBACK.to_owned(),
                tab_id: "w8:tB".to_owned(),
                label: "myws".to_owned(),
            })),
            "{notices:?}"
        );
    }

    #[test]
    fn a_handover_outside_herdr_still_says_it_is_naming_no_tab() {
        // `Off` said rather than skipped, for the reason the title's `Off` is said:
        // the sink is the single place that decides nothing happens. A caller that
        // filtered here would be a second place deciding it.
        let mut scene = Scene::new().with_running("myws");
        scene.host.stderr_tty = true;
        let token = HostToken::new();
        let mut notices = Vec::new();

        let _ = attaching(&scene, &token, "myws", None, &mut notices);

        assert_eq!(
            notices.get(1),
            Some(&LaunchNotice::HerdrTab(HerdrTabRename::Off)),
            "{notices:?}"
        );
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

    // -------------------------------- the cold path's refusal, as a reason

    /// devlaunch#339: a metadata-refused cold open surfaces as the typed arm.
    ///
    /// [`ColdRefused`] carried a `reason: String` until #340, filled by `dl`
    /// rendering a [`StartupError`] and quoted straight back into this refusal.
    /// That was the one place the binary's own prose travelled back *through* core,
    /// and what it cost is visible from here: a caller holding the refusal could
    /// read the sentence and could not ask which of the three things went wrong.
    ///
    /// So the assertion is the whole value, not a substring of one. The refusal
    /// arrives at the launch's own refusal as the reason it *is*, with the
    /// [`MetadataError`](crate::domain::metadata::MetadataError) the store produced
    /// still inside it and no sentence anywhere along the way. Flatten either level
    /// back to a string and this stops compiling, which is the failure it is here
    /// for.
    #[test]
    fn a_metadata_refused_cold_open_surfaces_as_the_typed_arm() {
        let refused = name_default_branch(
            &mut MetadataWillNotOpen,
            "blooop",
            "devlaunch",
            "git@github.com:blooop/devlaunch.git",
            &mut no_notices(),
        );

        assert_eq!(
            refused,
            Err(BranchNotNamed::Cold(ColdRefused::Startup(
                StartupError::Metadata(metadata_refusal())
            )))
        );
    }

    /// The other arm that opens the cold path, carrying the same reason unchanged.
    ///
    /// Both are worth pinning because they are separate `map_err`s over the same
    /// `open`, and a refusal that survived one of them and was stringified by the
    /// other would be the old bug back in half the launches.
    #[test]
    fn the_cold_arm_of_a_host_side_preparation_carries_the_same_typed_reason() {
        let workspace = WorkspaceId::new("blooop", "devlaunch", "main").expect("a safe triple");

        let refused = prepare(
            &mut MetadataWillNotOpen,
            &workspace,
            "git@github.com:blooop/devlaunch.git",
            &mut no_notices(),
        );

        assert_eq!(
            refused,
            Err(NotPrepared::Cold(ColdRefused::Startup(
                StartupError::Metadata(metadata_refusal())
            )))
        );
    }

    /// The arm that replaced an English literal.
    ///
    /// `NoColdPath` used to refuse with the sentence "the cold path is not available
    /// to this caller", written in core, which is the rule #251 §5 states. It is a
    /// variant now and the sentence is the binary's.
    #[test]
    fn a_launcher_with_no_cold_path_refuses_with_an_arm_rather_than_a_sentence() {
        let mut none = NoColdPath;

        assert_eq!(none.open().err(), Some(ColdRefused::NoColdPath));
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
        let scene = Scene::new().with_running(workspace.value());
        let mut context = CommandContext::new(&scene.runner);

        let resolution = resolve_triple(
            &mut context,
            &mut NeverCold,
            &workspace,
            &mut no_notices(),
            Patience::AsLongAsItTakes,
        );

        assert_eq!(
            resolution,
            Ok(Resolution::Warm {
                placement: Placement::Known {
                    workspace_id: workspace.value().to_owned(),
                    title: workspace.label(),
                    state: ContainerState::Running,
                }
            })
        );
        assert_eq!(
            scene.devpod_commands(),
            vec![vec![
                "status".to_owned(),
                workspace.value().to_owned(),
                "--output".to_owned(),
                "json".to_owned(),
            ]]
        );
    }

    #[test]
    fn an_id_metadata_recorded_is_titled_by_that_id_and_not_by_the_triples_label() {
        // The label is a rendering of the id in play, and this is the one path where
        // the id in play is not the one the triple derives. `resolve_known_workspace`
        // answers with the id `metadata.json` recorded when devpod has never heard of
        // the derived one -- a workspace created under an older id scheme and not yet
        // reconciled. Titling that by `devlaunch@main` would put a rendering of
        // `devlaunch-main-3j1t` on the tab of a workspace whose `dl --ls` row reads
        // `devlaunch-main-legacy`, with no two characters between them and nothing to
        // match by eye. It also installs that name in the legacy container's profile,
        // where nothing on screen ties it back to anything.
        let workspace = WorkspaceId::new("blooop", "devlaunch", "main").expect("a safe triple");
        let scene = Scene::new().with_running("devlaunch-main-legacy");
        {
            let (mut storage, _) = MetadataStorage::open(scene.cache_dir().join("metadata.json"))
                .expect("a fresh store");
            let mut record = WorktreeInfo::new(
                &workspace,
                scene
                    .cache_dir()
                    .join("repos/blooop/devlaunch/devlaunch-main-legacy"),
            );
            record.devpod_workspace_id = Some("devlaunch-main-legacy".to_owned());
            storage.add_worktree(record).expect("the record is saved");
        }
        let git = Git::new(&scene.runner);
        let mut cold = RealCold::new(scene.cache_dir(), git);
        let mut context = CommandContext::new(&scene.runner);

        let resolution = resolve_triple(
            &mut context,
            &mut cold,
            &workspace,
            &mut no_notices(),
            Patience::AsLongAsItTakes,
        );

        assert_eq!(
            resolution,
            Ok(Resolution::Warm {
                placement: Placement::Known {
                    workspace_id: "devlaunch-main-legacy".to_owned(),
                    title: "devlaunch-main-legacy".to_owned(),
                    state: ContainerState::Running,
                }
            })
        );
    }

    #[test]
    fn a_workspace_devpod_does_not_know_resolves_cold() {
        let workspace = WorkspaceId::new("owner", "repo", "feature/x").expect("a safe triple");
        let scene = Scene::new();
        let git = Git::new(&scene.runner);
        let mut cold = RealCold::new(scene.cache_dir(), git);
        let mut context = CommandContext::new(&scene.runner);

        let resolution = resolve_triple(
            &mut context,
            &mut cold,
            &workspace,
            &mut no_notices(),
            Patience::AsLongAsItTakes,
        );

        assert_eq!(
            resolution,
            Ok(Resolution::Cold {
                workspace: workspace.clone()
            })
        );
        assert_eq!(cold.opens.get(), 1, "the record was consulted exactly once");
    }

    // ------------------------------------- stage two: the derived-id collision
    //
    // blooop/devlaunch#438. `COLLIDING_A` and `COLLIDING_B` are a real pair: two
    // branches of one repository whose triples derive one id, found by searching
    // the shape the module doc records having seen in the wild -- long release
    // refs differing only past the truncation point, so the readable halves are
    // identical and only the four-character suffix separates them. They are
    // written down rather than searched for at test time because a fixed pair is
    // the same test every run, and because finding one is a birthday search over
    // 36^4 that has no business running in a unit test.
    //
    // If a change to the derivation moves them apart, the first test below fails
    // and says so, rather than the guard's tests quietly passing against two ids
    // that no longer collide.

    const COLLIDING_A: &str = "release/999999999999999999999911630";
    const COLLIDING_B: &str = "release/999999999999999999999911783";

    /// Add one worktree record to the store at *cache_dir*, and save it.
    fn record_worktree(
        cache_dir: &Path,
        owner: &str,
        repo: &str,
        branch: &str,
        workspace_id: &str,
    ) {
        let (mut storage, _) =
            MetadataStorage::open(cache_dir.join("metadata.json")).expect("a fresh store opens");
        storage
            .add_worktree(WorktreeInfo::as_an_older_dl_recorded_it(
                owner,
                repo,
                branch,
                cache_dir.join(format!("repos/{owner}/{repo}/{workspace_id}")),
                workspace_id,
            ))
            .expect("the record is saved");
    }

    #[test]
    fn both_of_the_colliding_refs_really_do_derive_one_id() {
        let a = WorkspaceId::new("blooop", "devlaunch", COLLIDING_A).expect("a safe triple");
        let b = WorkspaceId::new("blooop", "devlaunch", COLLIDING_B).expect("a safe triple");

        assert_ne!(COLLIDING_A, COLLIDING_B, "two different branches");
        assert_eq!(
            a.value(),
            b.value(),
            "the pair the collision tests are written against no longer collides"
        );
    }

    #[test]
    fn a_launch_whose_derived_id_another_triple_already_holds_is_refused() {
        // The failure being closed: devpod workspace names are global rather than
        // scoped by repository, so the second of two colliding triples finds the
        // first one's container under its own derived id, attaches, and says
        // nothing. The user gets somebody else's checkout, and a later `rm` on
        // either one deletes a clone the other still claims.
        let held = WorkspaceId::new("blooop", "devlaunch", COLLIDING_A).expect("a safe triple");
        let scene = Scene::new().with_running(held.value());
        record_worktree(
            scene.cache_dir(),
            "blooop",
            "devlaunch",
            COLLIDING_A,
            held.value(),
        );
        let git = Git::new(&scene.runner);
        let mut cold = RealCold::new(scene.cache_dir(), git);
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
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
            &format!("blooop/devlaunch@{COLLIDING_B}"),
            &LaunchVerb::Attach {
                command: Some("true".to_owned()),
            },
            None,
        );

        assert_eq!(
            launched,
            Ok(Launched::Refused(LaunchRefusal::IdCollision {
                workspace_id: held.value().to_owned(),
                owner: "blooop".to_owned(),
                repo: "devlaunch".to_owned(),
                branch: COLLIDING_B.to_owned(),
                recorded_owner: "blooop".to_owned(),
                recorded_repo: "devlaunch".to_owned(),
                recorded_branch: COLLIDING_A.to_owned(),
            }))
        );
        assert_eq!(
            scene.devpod_heads(),
            Vec::<Vec<String>>::new(),
            "refused before devpod was asked anything, so nothing attached"
        );
    }

    #[test]
    fn a_launch_that_matches_its_own_record_attaches_and_reads_no_machinery() {
        // The warm path, unchanged and unslowed. A record holding this triple's own
        // derived id is not a collision -- it is the ordinary case, every workspace
        // dl has ever made -- and the guard looking at the records must not turn
        // devlaunch#145's warm attach into a cache migration under the metadata
        // lock. `opens` is the assertion: the guard read the file, the machinery
        // stayed down.
        let workspace =
            WorkspaceId::new("blooop", "devlaunch", COLLIDING_A).expect("a safe triple");
        let scene = Scene::new().with_running(workspace.value());
        record_worktree(
            scene.cache_dir(),
            "blooop",
            "devlaunch",
            COLLIDING_A,
            workspace.value(),
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

            let launched = launch.run(
                &format!("blooop/devlaunch@{COLLIDING_A}"),
                &LaunchVerb::Attach {
                    command: Some("true".to_owned()),
                },
                None,
            );

            assert_eq!(
                launched,
                Ok(Launched::Session(Session::RemoteExit { status: 0 })),
                "the triple's own record is not a collision"
            );
        }
        assert_eq!(
            cold.opens.get(),
            0,
            "a warm launch still brings no clone manager, config or migration up"
        );
    }

    #[test]
    fn a_repository_spelled_in_another_case_is_the_same_workspace_and_is_not_refused() {
        // The convergence `suffix` exists to guarantee, seen from the guard's side.
        // GitHub's owners are case-insensitive, so `NVIDIA/cuda-samples` and
        // `nvidia/cuda-samples` are one repository and derive one id on purpose --
        // the alternative is one repo cloned twice into two containers. A guard that
        // compared the triples as raw strings read the second spelling as a
        // *different* triple holding the first one's id and refused it, and the
        // refusal it printed told the reader to rename one of the two branches when
        // both branches are `main`: a message naming no way out that exists, in
        // front of a workspace the reader already has running.
        let recorded = WorkspaceId::new("nvidia", "cuda-samples", "main").expect("a safe triple");
        let typed = WorkspaceId::new("NVIDIA", "cuda-samples", "main").expect("a safe triple");
        assert_eq!(
            typed.value(),
            recorded.value(),
            "the two spellings are one workspace, which is what makes this a trap"
        );
        let scene = Scene::new().with_running(recorded.value());
        record_worktree(
            scene.cache_dir(),
            "nvidia",
            "cuda-samples",
            "main",
            recorded.value(),
        );
        let git = Git::new(&scene.runner);
        let mut cold = RealCold::new(scene.cache_dir(), git);
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
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
            "NVIDIA/cuda-samples@main",
            &LaunchVerb::Attach {
                command: Some("true".to_owned()),
            },
            None,
        );

        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 })),
            "the other spelling of a repository is not an intruder holding its id"
        );
    }

    #[test]
    fn a_ref_that_differs_only_in_case_is_a_different_workspace_and_does_collide() {
        // The other half of the rule, and the reason the fix folds two of the three
        // parts rather than all of them. Git refs are case-sensitive: `Main` and
        // `main` can both exist in one repository, so they are two workspaces. Were
        // the ref folded along with the owner, this launch would be waved through to
        // attach to the other branch's container -- the exact silent wrong-checkout
        // this guard exists to stop.
        let typed = WorkspaceId::new("owner", "repo", "Main").expect("a safe triple");
        let other = WorkspaceId::new("owner", "repo", "main").expect("a safe triple");
        assert_ne!(
            typed.value(),
            other.value(),
            "the two refs hash apart, which is why the collision below has to be staged"
        );
        let scene = Scene::new().with_running(typed.value());
        record_worktree(
            scene.cache_dir(),
            "owner",
            "repo",
            "main",
            // The `main` record is put on the id `Main` derives. Two refs differing
            // only in case do not collide on their own -- they hash apart by design
            // -- so a real collision between them cannot be produced, only staged.
            // What is under test is the comparison, not the hash: the guard has to
            // read these two as different triples.
            typed.value(),
        );
        let git = Git::new(&scene.runner);
        let mut cold = RealCold::new(scene.cache_dir(), git);
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
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
            "owner/repo@Main",
            &LaunchVerb::Attach {
                command: Some("true".to_owned()),
            },
            None,
        );

        assert_eq!(
            launched,
            Ok(Launched::Refused(LaunchRefusal::IdCollision {
                workspace_id: typed.value().to_owned(),
                owner: "owner".to_owned(),
                repo: "repo".to_owned(),
                branch: "Main".to_owned(),
                recorded_owner: "owner".to_owned(),
                recorded_repo: "repo".to_owned(),
                recorded_branch: "main".to_owned(),
            })),
            "a ref differing only in case is a different branch, not the same one"
        );
    }

    #[test]
    fn a_record_whose_branch_is_not_a_legal_ref_does_not_block_a_launch() {
        // The old derivation coerced unsafe refs instead of rejecting them, so a
        // stored branch is not necessarily a ref `WorkspaceId::new` will accept.
        // The migration reports such a record as `unusable` and carries on; the
        // guard skips it. A guard that failed on it would refuse every launch on
        // the machine, which is far worse than the one-in-thirty-seven-thousand
        // accident it is watching for.
        let workspace = WorkspaceId::new("owner", "repo", "main").expect("a safe triple");
        let scene = Scene::new().with_running(workspace.value());
        record_worktree(
            scene.cache_dir(),
            "owner",
            "repo",
            "a branch with spaces",
            "repo-a-branch-with-spaces-legacy",
        );
        let git = Git::new(&scene.runner);
        let mut cold = RealCold::new(scene.cache_dir(), git);
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
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
            "owner/repo@main",
            &LaunchVerb::Attach {
                command: Some("true".to_owned()),
            },
            None,
        );

        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 }))
        );
    }

    #[test]
    fn a_store_that_cannot_be_read_does_not_refuse_a_launch() {
        // `recorded_id` states the rule and the guard honours it: a lookup that
        // failed means "no collision found", not an error. The records here hold
        // the colliding record from the refusal test, so the only thing standing
        // between this launch and a refusal is that the look came up empty.
        let held = WorkspaceId::new("blooop", "devlaunch", COLLIDING_A).expect("a safe triple");
        let scene = Scene::new().with_running(held.value());
        record_worktree(
            scene.cache_dir(),
            "blooop",
            "devlaunch",
            COLLIDING_A,
            held.value(),
        );
        let git = Git::new(&scene.runner);
        let mut cold = UnreadableRecords(RealCold::new(scene.cache_dir(), git));
        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
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
            &format!("blooop/devlaunch@{COLLIDING_B}"),
            &LaunchVerb::Attach {
                command: Some("true".to_owned()),
            },
            None,
        );

        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 })),
            "an unreadable store means no collision, not a refusal"
        );
    }

    #[test]
    fn a_placement_answers_the_fast_attach_question_on_its_own() {
        // Python reads it off two correlated locals; here it is one value.
        let running = Placement::Known {
            workspace_id: "myws".to_owned(),
            title: "myws".to_owned(),
            state: ContainerState::Running,
        };
        let stopped = Placement::Known {
            workspace_id: "myws".to_owned(),
            title: "myws".to_owned(),
            state: ContainerState::Stopped,
        };
        let creating = Placement::Creating {
            workspace_id: "myws".to_owned(),
            title: "myws".to_owned(),
            source: "/clone".to_owned(),
        };
        let listed = Placement::Listed {
            workspace_id: "myws".to_owned(),
            title: "myws".to_owned(),
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
    fn a_warm_attach_forwards_the_claude_login_the_host_remembers() {
        // The path that runs no pass at all: a workspace that is up and finished
        // creating goes straight to a session. Nothing observes its config directory
        // during that launch, so without the host's own records the credential worked
        // only on the launch that *created* the workspace. Found by launching a real
        // workspace twice, which is the only place it shows.
        let workspace =
            WorkspaceId::new("octocat", "Hello-World", "master").expect("a safe triple");
        let mut scene = Scene::new().with_running(workspace.value());
        let home = tempfile::tempdir().expect("a scratch home");
        std::fs::create_dir_all(home.path().join(".claude")).expect("a config dir");
        std::fs::write(
            home.path().join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"not-a-real-warm-token"}}"#,
        )
        .expect("a credential");
        scene.host.home = Some(home.path().to_path_buf());

        let updater = SelfInvocation::new("dl");
        let completion = scene.cache_dir().join("completion.json");
        let mut parts = launching(&scene.runner, &updater, &completion);
        parts.provision.claude_remembered = Some(ClaudeConfig::Ours);
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
            "octocat/Hello-World@master",
            &LaunchVerb::Attach {
                command: Some("true".to_owned()),
            },
            None,
        );
        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 }))
        );

        let ssh = scene
            .runner
            .calls_to("devpod")
            .into_iter()
            .find(|call| call.args().first().map(String::as_str) == Some("ssh"))
            .expect("a session");
        assert!(
            ssh.argv().contains(&claude::TOKEN_VAR.to_owned()),
            "{ssh:?}"
        );
        assert_eq!(
            ssh.invocation()
                .env
                .entries
                .get(claude::TOKEN_VAR)
                .map(String::as_str),
            Some("not-a-real-warm-token")
        );
    }

    #[test]
    fn a_warm_attach_with_nothing_remembered_forwards_nothing_and_costs_nothing() {
        // A workspace this build has never provisioned. No login is forwarded, and
        // no round trip is spent finding out: the attach path stays exactly as
        // cheap and as quiet as it was. Such a workspace acquires an answer on its
        // next `up`, and a workspace created by this build has one from the start.
        let workspace =
            WorkspaceId::new("octocat", "Hello-World", "master").expect("a safe triple");
        let mut scene = Scene::new().with_running(workspace.value());
        let home = tempfile::tempdir().expect("a scratch home");
        std::fs::create_dir_all(home.path().join(".claude")).expect("a config dir");
        std::fs::write(
            home.path().join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"not-a-real-warm-token"}}"#,
        )
        .expect("a credential");
        scene.host.home = Some(home.path().to_path_buf());

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
            "octocat/Hello-World@master",
            &LaunchVerb::Attach {
                command: Some("true".to_owned()),
            },
            None,
        );

        let ssh = scene
            .runner
            .calls_to("devpod")
            .into_iter()
            .find(|call| call.args().first().map(String::as_str) == Some("ssh"))
            .expect("a session");
        assert!(
            !ssh.argv().contains(&claude::TOKEN_VAR.to_owned()),
            "{ssh:?}"
        );
        assert!(
            parts
                .provision
                .passes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty(),
            "a warm attach must not start paying for a round trip it never paid for"
        );
    }

    #[test]
    fn a_warm_git_spec_one_shot_is_one_status_and_one_ssh() {
        // The launcher path: `dl owner/repo@branch -- cmd`, workspace warm. This
        // is the exact shape wayfinder hands dl for every agent launch, so its
        // overhead is what sits between picking a ticket and a running agent.
        let workspace = WorkspaceId::new("blooop", "devlaunch", "wayfinder/devlaunch-7")
            .expect("a safe triple");
        let scene = Scene::new().with_running(workspace.value());
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
                    workspace.value().to_owned(),
                    "--output".to_owned(),
                    "json".to_owned(),
                ],
                vec![
                    "ssh".to_owned(),
                    workspace.value().to_owned(),
                    "--command".to_owned(),
                    "bash -lc 'echo hi'".to_owned(),
                ],
            ]
        );
    }

    #[test]
    fn a_triple_names_the_terminal_after_the_label_devpod_is_not_addressed_by() {
        // The tab reads `devlaunch@feature-auth` where devpod, in this same launch
        // (the `status` and `ssh` below), is addressed by
        // `devlaunch-feature-auth-np10`. Both halves are asserted here because the
        // claim is the relationship between them: the label is the id with the
        // suffix off and one dash spelled `@`, so a tab still matches a `dl --ls`
        // row by eye without carrying the four characters nothing reads.
        //
        // `feature/auth` is the ref that shows what the *slug* costs, which the `@`
        // does not buy back: both spell it `feature-auth`, which is also the name of
        // a different branch this repository could have, so neither can say which of
        // the two the session is in. Only the full spec could, and it is not what
        // came back -- see `TerminalTitle` for why the length made that the wrong
        // trade.
        let workspace =
            WorkspaceId::new("blooop", "devlaunch", "feature/auth").expect("a safe triple");
        let mut scene = Scene::new().with_running(workspace.value());
        scene.host.stderr_tty = true;
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
            "blooop/devlaunch@feature/auth",
            &LaunchVerb::Attach {
                command: Some("echo hi".to_owned()),
            },
            None,
        );

        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 }))
        );
        assert!(
            parts.said.iter().any(|notice| notice
                == &LaunchNotice::TerminalTitle(TerminalTitle::Write(format!(
                    "\x1b]2;{}\x07",
                    workspace.label()
                )))),
            "{:?}",
            parts.said
        );
        assert_eq!(workspace.label(), "devlaunch@feature-auth");
        // And the id is what devpod was given, unchanged by any of this.
        assert_eq!(workspace.value(), "devlaunch-feature-auth-np10");
        assert!(
            scene
                .devpod_commands()
                .iter()
                .all(|call| call.iter().any(|word| word == workspace.value())),
            "{:?}",
            scene.devpod_commands()
        );
    }

    #[test]
    fn a_bare_name_a_caller_recognised_is_titled_by_the_label_after_all() {
        // The picker opens a workspace **by id** and is the one caller that still
        // knows its triple: it read the owner and repo out of the cache layout and
        // the branch out of the clone's `HEAD` in order to draw the row. Without
        // that handed on, `dl` with no arguments -- the way a workspace is reopened
        // -- put `devlaunch-main-3j1t` on the tab where the same workspace opened as
        // `dl blooop/devlaunch@main` put `devlaunch@main`, and the two names would
        // then pile up in the profile a line each.
        let workspace = WorkspaceId::new("blooop", "devlaunch", "main").expect("a safe triple");
        let mut scene = Scene::new().with_running(workspace.value());
        scene.host.stderr_tty = true;
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
            )
            .recognised_as(Some(workspace.clone()));
            let _ = launch.run(workspace.value(), &LaunchVerb::Up, None);
        }

        assert_eq!(parts.provision.titles(), vec![Some(workspace.label())]);
    }

    #[test]
    fn a_triple_that_no_longer_derives_this_id_is_not_what_the_tab_says() {
        // `HEAD` is the branch checked out *now*, so a `git switch` inside the
        // container leaves the picker holding a triple that derives a different
        // workspace. Naming the tab from it would put another workspace's label on
        // this one. The check is core's and not the picker's: the picker carries
        // the evidence, `titled` reaches the verdict, and it is the same verdict the
        // recorded-id path gets.
        let workspace = WorkspaceId::new("blooop", "devlaunch", "main").expect("a safe triple");
        let switched = WorkspaceId::new("blooop", "devlaunch", "other").expect("a safe triple");
        let scene = Scene::new().with_running(workspace.value());
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
            )
            .recognised_as(Some(switched));
            let _ = launch.run(workspace.value(), &LaunchVerb::Up, None);
        }

        assert_eq!(
            parts.provision.titles(),
            vec![Some(workspace.value().to_owned())]
        );
    }

    #[test]
    fn a_bare_name_names_the_terminal_after_the_id_because_that_is_all_it_has() {
        // The other three arms have no triple to prefer, and for a bare name the id
        // *is* what the user typed. Pinned beside the spec case so that a change to
        // one has to say what it means for the other.
        let mut scene = Scene::new().with_running("myws");
        scene.host.stderr_tty = true;
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
            "myws",
            &LaunchVerb::Attach {
                command: Some("echo hi".to_owned()),
            },
            None,
        );

        assert_eq!(
            launched,
            Ok(Launched::Session(Session::RemoteExit { status: 0 }))
        );
        assert!(
            parts.said.iter().any(|notice| notice
                == &LaunchNotice::TerminalTitle(TerminalTitle::Write(
                    "\x1b]2;myws\x07".to_owned()
                ))),
            "{:?}",
            parts.said
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

    /// A create that died in its `postCreateCommand` leaves the container up, so
    /// `devpod status` says `Running` and the fast-attach arm fires — dl attaches
    /// to a workspace devpod never finished setting up.
    ///
    /// What the user sees is not a setup error. devpod records the create's result
    /// only on the way out of a *successful* `up`, and the remote user lives in
    /// that result (`.MergedConfig.remoteUser`), so `devpod ssh` for a workspace
    /// without one falls back to **root**. Everything the image put on the remote
    /// user's PATH is then missing, and the session dies on `claude: command not
    /// found` — a message about the wrong thing entirely, from a container that
    /// will never work no matter how many times it is attached to.
    ///
    /// Measured against devpod 0.26.1, with a devcontainer whose
    /// `postCreateCommand` exits 1: `devpod status` answers `Running`, no
    /// `workspace_result.json` is written, no `Host <id>.devpod` alias appears, and
    /// `devpod ssh --command whoami` answers `root` with `HOME=/root`.
    ///
    /// So a running container is not on its own evidence that there is anything to
    /// attach to, and this launch must bring the workspace up instead — which
    /// re-runs the lifecycle hooks that failed and reports their failure, rather
    /// than hiding it behind a missing binary.
    #[test]
    fn a_running_workspace_whose_create_never_finished_is_brought_up_not_attached() {
        let scene = Scene::new()
            .with_running("myws")
            .with_create_aborted("myws");
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

        let _ = launch.run("myws", &LaunchVerb::Attach { command: None }, None);

        drop(launch);
        assert!(
            scene
                .devpod_commands()
                .iter()
                .any(|argv| argv.first().map(String::as_str) == Some("up")),
            "an unfinished create must be brought up, not attached to: {:?}",
            scene.devpod_commands()
        );
        assert!(
            !parts.said.contains(&LaunchNotice::AlreadyRunningAttaching {
                workspace_id: "myws".to_owned()
            }),
            "a workspace devpod never finished is not `already running`"
        );
    }

    /// The other side of the same check: a create devpod *did* finish still takes
    /// the fast path. Without this, "is it set up" could be answered by refusing
    /// every fast attach, which would pass the test above and cost every warm
    /// launch its whole reason for existing.
    #[test]
    fn a_running_workspace_whose_create_finished_still_fast_attaches() {
        let scene = Scene::new()
            .with_running("myws")
            .with_create_completed("myws");
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
        // The occasion the verdict cache was built for: `dl <ws> up` is the prewarm,
        // run over and over against a workspace that is already up, and the round
        // trip this pass would otherwise pay is the whole of what it costs.
        assert_eq!(parts.provision.occasions(), vec![PassOccasion::TopUp]);
        assert_eq!(
            scene.devpod_heads(),
            vec![vec!["status".to_owned(), "myws".to_owned()]],
            "and no up at all"
        );
    }

    #[test]
    fn a_workspace_opened_both_ways_installs_two_names_and_the_last_one_wins() {
        // The price of the `@`, pinned so that it is a decision and not a surprise.
        // One workspace, opened both ways: by spec, and later by the id it derived.
        // A spec resolves a triple and installs the label; a bare id never had a
        // triple, so it installs the id. Two different strings for one workspace.
        //
        // The profile line is deduped by a hash of its own text, so the second does
        // not replace the first, it appends -- and the last append is what every
        // prompt obeys, which leaves the tab reading `devlaunch-main-3j1t` where the
        // spec had asked for `devlaunch@main`.
        //
        // It is bounded at one extra line and one less readable tab, and it is what
        // naming after the id bought outright: every arm agreed on the id, so there
        // was one line ever. What that cost was the `@`, on every launch, to keep a
        // guarantee that only bites when the same workspace is reached two ways.
        let workspace = WorkspaceId::new("blooop", "devlaunch", "main").expect("a safe triple");
        let scene = Scene::new().with_running(workspace.value());
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
            let _ = launch.run("blooop/devlaunch@main", &LaunchVerb::Up, None);
        }
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
            let _ = launch.run(workspace.value(), &LaunchVerb::Up, None);
        }

        assert_eq!(
            parts.provision.titles(),
            vec![Some(workspace.label()), Some(workspace.value().to_owned())],
            "the spec installs the label and the bare id installs the id"
        );
    }

    #[test]
    fn a_ref_holding_a_control_cannot_reach_the_container_at_all() {
        // `is_safe_name` accepts one trailing newline -- Python's `$` anchor did, and
        // the quirk is ported deliberately -- so `main\n` is a ref. It used to be a
        // *name* too, because the container was told the spec, and an unfiltered
        // newline lands in a file every login sources, inside the quoted word,
        // splitting one PS1 assignment over two physical lines.
        //
        // Naming after the derived name closed that by construction rather than by
        // filtering: `slug` erases the newline before an id exists, so there is no
        // control left for the label to carry either. So this asserts the label
        // arrives whole -- had `sanitize_title` found anything to drop, the name
        // installed would not be `label()`.
        // `sanitize_title` still earns its keep on the two arms that reach a title
        // without deriving one, a bare devpod name and a path leaf, and
        // `a_spec_cannot_smuggle_a_second_escape_into_the_title` is where the raw
        // spec is pushed through it.
        let workspace = WorkspaceId::new("blooop", "devlaunch", "main\n").expect("a safe triple");
        let scene = Scene::new().with_running(workspace.value());
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
            launch.run("blooop/devlaunch@main\n", &LaunchVerb::Up, None)
        };

        assert_eq!(launched, Ok(Launched::AlreadyRunning));
        assert_eq!(parts.provision.titles(), vec![Some(workspace.label())]);
    }

    #[test]
    fn the_pass_is_told_to_have_the_container_keep_titling_after_the_label() {
        // The other half of the title, and the half that lasts. dl's own escape is
        // overwritten by the first interactive prompt; this is the name the pass
        // installs in the container's profile so every prompt after that writes it
        // again. Same string as the escape, same switch governing it -- one name,
        // two places it has to reach.
        let workspace =
            WorkspaceId::new("blooop", "devlaunch", "feature/auth").expect("a safe triple");
        let scene = Scene::new().with_running(workspace.value());
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
            launch.run("blooop/devlaunch@feature/auth", &LaunchVerb::Up, None)
        };

        assert_eq!(launched, Ok(Launched::AlreadyRunning));
        assert_eq!(parts.provision.titles(), vec![Some(workspace.label())]);
    }

    #[test]
    fn the_title_switch_reaches_the_container_and_not_just_dls_own_escape() {
        // `DEVLAUNCH_NO_TITLE` has to govern every piece or it governs none: a host
        // that silenced the escape and still had its profile edited would find the
        // variable did nothing it could see. Three pieces now -- the escape, the
        // `PS1` line, and claude's suppression -- and refusing a container title
        // here is what takes all of them with it, since they share one stage.
        let mut scene = Scene::new().with_running("myws");
        scene.host.no_title = Some("1".to_owned());
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
        assert_eq!(parts.provision.titles(), vec![None]);
    }

    #[test]
    fn a_headless_up_still_installs_the_name_the_next_session_will_read() {
        // `stderr_tty` is false here, which is `dl <ws> up` redirected -- a prewarm.
        // dl writes no escape of its own, correctly: there is no terminal to write
        // to. The container is still taught the name, because the session that
        // arrives later is the one it is for, and asking this pass to guess whether
        // one ever will is a question it cannot answer.
        let workspace = WorkspaceId::new("blooop", "devlaunch", "main").expect("a safe triple");
        let scene = Scene::new().with_running(workspace.value());
        assert!(!scene.host.stderr_tty, "the premise of this test");
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
            launch.run("blooop/devlaunch@main", &LaunchVerb::Up, None)
        };

        assert_eq!(launched, Ok(Launched::AlreadyRunning));
        assert_eq!(parts.provision.titles(), vec![Some(workspace.label())]);
        // And nothing was written to the terminal that is not there.
        assert!(
            !parts.said.iter().any(|notice| matches!(
                notice,
                LaunchNotice::TerminalTitle(TerminalTitle::Write(_))
            )),
            "{:?}",
            parts.said
        );
    }

    /// `dl <ws> up` is what a user types to fix a workspace, so it is the worst
    /// verb to answer "already running" for a create that never finished: the
    /// container is up, nothing in it is set up, and the one command documented as
    /// the recovery would be the one that declines to run it.
    #[test]
    fn the_up_verb_brings_up_a_running_workspace_whose_create_never_finished() {
        let scene = Scene::new()
            .with_running("myws")
            .with_create_aborted("myws");
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

        assert_ne!(
            launched,
            Ok(Launched::AlreadyRunning),
            "a workspace devpod never finished is not `already running`"
        );
        assert!(
            scene
                .devpod_commands()
                .iter()
                .any(|argv| argv.first().map(String::as_str) == Some("up")),
            "the recovery verb must run the recovery: {:?}",
            scene.devpod_commands()
        );
    }

    /// The other side: a create devpod *did* finish still short-circuits, so the
    /// check above cannot be satisfied by making `up` always re-walk a lifecycle
    /// the workspace has already been through.
    #[test]
    fn the_up_verb_on_a_finished_running_workspace_still_runs_no_up() {
        let scene = Scene::new()
            .with_running("myws")
            .with_create_completed("myws");
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
        let scene = Scene::new().with_running(workspace.value());
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
            .add_workspace(workspace.value(), WorkspaceState::Running);
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
                &ClaudeSeen::new(),
                &NoProvisioning,
                &UpRequest::new(
                    "brand-new",
                    Naming::Create {
                        workspace_id: "brand-new",
                    },
                ),
                None,
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
                &ClaudeSeen::new(),
                &NoProvisioning,
                &UpRequest::new(
                    "brand-new",
                    Naming::Create {
                        workspace_id: "brand-new",
                    },
                ),
                None,
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
