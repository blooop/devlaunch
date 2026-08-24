//! Put the tools a session always needs into every workspace devlaunch opens.
//!
//! Ported from `devlaunch/tools.py`; see docs/rust-rewrite-plan.md (M8).
//!
//! `gh` and `claude` are not optional extras for the way these workspaces get
//! used: `dl` already forwards the host's GitHub login into every container,
//! which is worth nothing when the container has no `gh` to spend it, and `aid`
//! exists to run `claude` in there. A guarantee that depends on the repo's own
//! devcontainer.json is not a guarantee, so the tools come from the invocation —
//! the same argument [`crate::clients::gh`] makes for the token.
//!
//! Where they come from is a cost question, and the answer is **the host first,
//! the network second**: a container that lacks them is lent the host's own
//! copies through a tar stream over the `devpod ssh` channel dl already holds,
//! and only a host with nothing to lend (or a container the lent binaries will
//! not run in) falls back to bootstrapping pixi and installing each tool.
//!
//! # Three things this module is, in the order they matter
//!
//! 1. **A set of POSIX scripts that run in someone else's container.** They are
//!    carried over from Python byte for byte — [`probe_script`],
//!    [`provision_script`], [`zellij_script`], [`transfer_script`] and the
//!    [`setup_script`] that composes the first of them with the stage snippets in
//!    front of it. Every one of them is pinned against the string Python renders
//!    (see the goldens in this module's tests); none of them is "improved" here,
//!    because the shipped behaviour *is* the bytes and a shell fix belongs in a
//!    ticket of its own.
//! 2. **A quoting layer.** The scripts are interpolated into `devpod ssh
//!    --command`, and one of them is interpolated into another (the zellij stage
//!    is a `bash -c <script>` inside the setup pass). [`quote`] is CPython's
//!    `shlex.quote`, reproduced rather than delegated — see its own note.
//! 3. **A three-trip flow.** [`provision_tools`] probes, lends and installs, and
//!    each trip earns the next: a provisioned workspace pays one, a lendable or
//!    cold one two, and only a genuinely empty container reaches the third.
//!
//! # What the probe answers, and who decides
//!
//! [`ProbeResult`] has three states rather than two, because a `claude` on PATH
//! is not necessarily a `claude` worth keeping: the shim this repo's own
//! devcontainer feature bakes satisfies `command -v` while still owing a ~285MB
//! download on first use. The container reports **two resolved paths and no
//! verdict**; what they mean is [`is_official_claude`], stated once and asked
//! from both ends — the host asks it of its own filesystem to decide what it may
//! lend ([`host_payload`]), and of the container's report to decide what the
//! container still needs. Two copies of that relation are two probes with
//! different opinions, which is how a downloader parked at
//! `versions/latest/bin/claude` came to be trusted by one side while the other
//! refused to lend the very same tree.
//!
//! # Nothing here is a failure of the launch
//!
//! Provisioning is a convenience: an install that does not work costs the
//! workspace its tools and not its session. So every way of coming up empty is an
//! arm of [`Provisioning`] rather than an error, and the one genuine error is a
//! devpod that has gone missing between `up` and here ([`DevpodMissing`]) — which
//! dl treats as fatal everywhere else and which this does not make an exception
//! of. Nothing in this module prints: every `logging.*` call Python makes is an
//! arm of [`ProvisionEvent`], carrying what that line interpolated, said into the
//! caller's [`Notices`] sink at the moment it happens — Python logs each line as
//! it goes, and a cold install is minutes long, so an event held back to the end
//! of the flow would arrive after the wait it explains. The words and levels are
//! the `dl` binary's rendering (#251 §5).

use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::{BufWriter, Read, Write};
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::clients::devpod::{self, Call, NotRun};
use crate::domain::workspace_id::hostname_of;
use crate::notices::Notices;
use crate::runner::interrupt;
use crate::runner::{Exit, OsFailure, Runner};
use crate::timing;

pub mod verdict_cache;

use verdict_cache::VerdictCache;

// ===========================================================================
// the constants both ends of the pipe are written against
// ===========================================================================

/// Set this to opt a machine out of installing tools into workspaces.
pub(crate) const DISABLE_VAR: &str = "DEVLAUNCH_NO_TOOLS";

/// Set this to opt a machine out of the zellij stage alone.
///
/// A second variable rather than a value of [`DISABLE_VAR`], because the two
/// answer different questions and a host that wants one wants the other left on.
/// `DEVLAUNCH_NO_TOOLS` is "do not install anything", which costs the workspace
/// `gh` and `claude` — the pair [`REQUIRED_TOOLS`] exists to guarantee, and the
/// reason `dl` forwards a GitHub token at all. A host whose containers get zellij
/// some other way (their own dotfiles, a base image, a devcontainer feature), or
/// which wants no zellij in there at all, is asking only for the stage to stop
/// running; asking for that with `DEVLAUNCH_NO_TOOLS` would surrender the
/// guarantee to save one `command -v`.
///
/// Deliberately **not** [`crate::flows::launch::ZELLIJ_WRAP_VAR`]'s opposite
/// number either. That switch decides whether a `dl <ws> -- <cmd>` first makes
/// sure a zellij *session* exists to open panes into, and it already tolerates a
/// container with no zellij — its setup is allowed to fail and the command runs
/// regardless. The two are orthogonal on purpose: this one is about what gets
/// installed, that one about what gets started.
pub(crate) const ZELLIJ_DISABLE_VAR: &str = "DEVLAUNCH_NO_ZELLIJ";

/// The values that mean "no" rather than "set, therefore yes". The same list
/// [`crate::clients::gh`] reads, and kept separately for the reason that module
/// gives: two escape hatches answering to one shared constant are one edit away
/// from becoming one escape hatch.
const FALSEY: [&str; 4] = ["", "0", "false", "no"];

/// The claude package lives in a personal channel rather than conda-forge.
pub(crate) const BLOOOP_CHANNEL: &str = "https://prefix.dev/blooop";

/// Where the official claude installer keeps one binary per version, relative to
/// a home directory. The host reads it to decide what it may lend
/// ([`claude_source`]), writes into it when it lends ([`transfer_script`]), and
/// the container is asked about it to decide whether it needs anything
/// ([`probe_script`]). The *relation* that makes a path the official layout lives
/// in [`is_official_claude`], for the same reason this string lives here: two
/// copies of a definition are two probes with different opinions.
pub(crate) const CLAUDE_VERSIONS_RELPATH: &str = ".local/share/claude/versions";

/// Where a lent `gh` lands under the container's home — a tar arcname, so the
/// same payload works whatever the container's username is.
pub(crate) const GH_RELPATH: &str = ".local/bin/gh";

/// What every line of a probe's report starts with, so the report survives being
/// printed into the same stdout as a container login profile's banner.
pub(crate) const PROBE_MARK: &str = "devlaunch-probe";

/// What devlaunch writes above every PATH line it appends to a container's login
/// profile, and the only thing its "have I already done this?" guard looks for.
///
/// The mark exists so the guard can ask about devlaunch's own work instead of
/// about a directory name. Asking about the directory made the answer depend on a
/// file devlaunch does not own: Ubuntu's stock `~/.profile` prepends
/// `~/.local/bin` itself, so on this repo's own base image the transfer's guard
/// read the image's block as its own, skipped the prepend, and left the claude
/// shim in front of the binary just lent — which made every `devpod up` re-pay a
/// multi-hundred-megabyte transfer for the life of the workspace.
pub(crate) const PROFILE_MARK: &str = "# devlaunch:";

/// The key every stage-outcome line carries, in the same marked `key value`
/// shape the probe's own report uses — so the outcomes survive a login profile's
/// banner exactly as the probe's lines do, and so [`ProbeResult::parse`], which
/// reads only the keys it always read, is inert to them.
pub(crate) const STAGE_KEY: &str = "stage";

/// The two words an outcome line can carry, the second followed by the status.
const STAGE_OK_WORD: &str = "ok";
const STAGE_FAILED_WORD: &str = "failed";

/// A name the stage-outcome protocol can carry on its marked line.
///
/// The outcome line is `<mark> stage <name> <status>`, split on spaces by both
/// readers, so a name with a space in it shears every line it is on:
/// `Stage::new("set hostname")` would silently report the stage — and the parse
/// of everything after it — as not reached. The constructor is a `const fn` that
/// panics on a space or an empty name, and every definition site calls it in
/// `const` context, so an invalid name is E0080 at compile time rather than a
/// run of phantom "not reached" reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StageName(&'static str);

impl StageName {
    /// The one way to name a stage. Call it in `const` context.
    pub(crate) const fn new(name: &'static str) -> Self {
        assert!(!name.is_empty(), "a stage name must not be empty");
        let bytes = name.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            assert!(
                bytes[at] != b' ',
                "a stage name must not contain a space: stage_outcomes splits the marked line on them"
            );
            at += 1;
        }
        Self(name)
    }

    pub(crate) const fn as_str(self) -> &'static str {
        self.0
    }
}

/// The stage that names the container. Its outcome is what "can this image set a
/// hostname" detection is, and it costs nothing: the trip is the probe's.
pub(crate) const HOSTNAME_STAGE: StageName = StageName::new("hostname");

/// The stage that puts `zellij` in the container (see [`ZELLIJ_TOOL`]). Also free
/// of round trips: it rides the pass every entry into Running already pays.
pub(crate) const ZELLIJ_STAGE: StageName = StageName::new("zellij");

/// The stage that teaches the shell to keep naming the terminal after this
/// workspace. Rides the same trip as the two above it.
pub(crate) const TITLE_STAGE: StageName = StageName::new("title");

// ===========================================================================
// quoting
// ===========================================================================

/// One shell word, quoted exactly as CPython's `shlex.quote` quotes it.
///
/// [`shell::quote`] itself, re-exported under this module's name because the whole
/// module is organised around one property: the scripts are the shipped behaviour,
/// so a payload devpod carries into a container has to be the *same bytes* the
/// Python build sends. Every script here contains single quotes (the
/// `grep -qxF '# devlaunch: …'` guard), which is exactly where the `shlex` crate's
/// (correct, different) spelling would change every remote payload in the file.
pub(crate) use crate::shell::quote;

// ===========================================================================
// the tools, and the opt-out
// ===========================================================================

/// A binary a session must be able to run, and the pixi package providing it.
///
/// `command` is what a shell has to find on PATH, which is not always the package
/// name — `claude` ships in `claude-shim` — so both are recorded rather than one
/// being derived from the other.
///
/// `channel` is `None` for a package the configured channels already carry. A sum
/// (`Default | Named(_)`) would be the same two states with a name for each; an
/// `Option` is that sum, and it is what makes [`REQUIRED_TOOLS`] a `const`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tool {
    pub(crate) command: &'static str,
    pub(crate) package: &'static str,
    pub(crate) channel: Option<&'static str>,
}

impl Tool {
    /// A tool the configured channels already provide.
    pub(crate) const fn new(command: &'static str, package: &'static str) -> Self {
        Self {
            command,
            package,
            channel: None,
        }
    }

    /// A tool that has to be asked for by channel.
    pub(crate) const fn from_channel(
        command: &'static str,
        package: &'static str,
        channel: &'static str,
    ) -> Self {
        Self {
            command,
            package,
            channel: Some(channel),
        }
    }

    /// The `pixi global install` arguments that provide this tool.
    pub(crate) fn install_args(&self) -> Vec<&'static str> {
        match self.channel {
            Some(channel) => vec!["--channel", channel, self.package],
            None => vec![self.package],
        }
    }
}

/// The pair this module probes for, lends, and installs.
pub(crate) const REQUIRED_TOOLS: [Tool; 2] = [
    Tool::new("gh", "gh"),
    Tool::from_channel("claude", "claude-shim", BLOOOP_CHANNEL),
];

/// The terminal an agent can open beside itself (#242). A `zellij` on PATH is the
/// whole of what that capability needs from a container.
///
/// Deliberately **not** a third row in [`REQUIRED_TOOLS`], which is the tempting
/// place and the wrong one. That array is not just an install list: [`probe_script`]
/// asks whether *all* of it is present, and a container that answers "missing" is
/// lent the host's ~300MB claude. Adding zellij there would put every container
/// that already has gh and a real claude back onto the lending path on every
/// launch, and would still never install zellij — a successful lend returns before
/// the network-install trip is reached. So it is provisioned as a stage of the
/// setup pass instead (see [`setup_stages`]), where a failure is already contained
/// and named by construction.
pub(crate) const ZELLIJ_TOOL: Tool = Tool::new("zellij", "zellij");

/// Whether the user opted this machine out of installing tools.
///
/// A parameter rather than a read of the process environment, so the decision is a
/// function of its inputs — and so [`crate::clients::gh::forwarding_disabled`] and
/// this can be shown to answer the same way without either test mutating an
/// environment the whole binary shares.
pub(crate) fn provisioning_disabled(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(value) => !FALSEY.contains(&crate::osext::strip(value).to_lowercase().as_str()),
    }
}

/// Whether this pass is allowed to install tools.
///
/// A sum rather than a bool at the call sites that pass it on, because the two
/// states are read in two places (which stages the pass carries, and whether
/// anything follows the pass) and "true" says nothing about which question it is
/// answering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolsSwitch {
    Install,
    Skip,
}

impl ToolsSwitch {
    /// What `DEVLAUNCH_NO_TOOLS` set to `value` asks for.
    pub(crate) fn requested(value: Option<&str>) -> Self {
        if provisioning_disabled(value) {
            Self::Skip
        } else {
            Self::Install
        }
    }

    /// What the process environment asks for.
    pub fn from_env() -> Self {
        Self::requested(crate::osext::env_str(DISABLE_VAR).as_deref())
    }
}

/// Whether this pass carries the zellij stage.
///
/// A type of its own rather than a second [`ToolsSwitch`], for the reason the two
/// variables are separate ([`ZELLIJ_DISABLE_VAR`]): they are read from different
/// places and mean different things, and two values of one type sitting next to
/// each other in a signature are two values a caller can swap without the compiler
/// noticing. Distinct types make [`setup_stages`]'s pair unswappable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZellijSwitch {
    Install,
    Skip,
}

impl ZellijSwitch {
    /// What `DEVLAUNCH_NO_ZELLIJ` set to `value` asks for.
    ///
    /// The same [`FALSEY`] list [`ToolsSwitch::requested`] reads, through the same
    /// [`provisioning_disabled`]: two opt-outs a user spells the same way must
    /// answer the same way, and `DEVLAUNCH_NO_ZELLIJ=0` reading as *yes, skip it*
    /// where `DEVLAUNCH_NO_TOOLS=0` reads as *no* is exactly the surprise a shared
    /// parse exists to prevent.
    pub(crate) fn requested(value: Option<&str>) -> Self {
        if provisioning_disabled(value) {
            Self::Skip
        } else {
            Self::Install
        }
    }

    /// What the process environment asks for.
    pub fn from_env() -> Self {
        Self::requested(crate::osext::env_str(ZELLIJ_DISABLE_VAR).as_deref())
    }
}

/// The two opt-outs one pass reads, carried as one value.
///
/// Not a convenience: [`provision_tools`] already takes a runner, a workspace, an
/// occasion, a host layout, a verdict cache and an events sink, and two more
/// positional parameters on top of that is where a call site starts getting them
/// in the wrong order. They belong together anyway — both answer "what may this
/// pass put in the container", and every caller decides both at once, from the
/// environment, when it is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switches {
    pub tools: ToolsSwitch,
    pub zellij: ZellijSwitch,
}

impl Switches {
    /// Both switches as the process environment sets them.
    pub fn from_env() -> Self {
        Self {
            tools: ToolsSwitch::from_env(),
            zellij: ZellijSwitch::from_env(),
        }
    }

    /// Both switches on — the default a machine that set neither variable gets,
    /// and the shape most tests want.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const INSTALLING: Self = Self {
        tools: ToolsSwitch::Install,
        zellij: ZellijSwitch::Install,
    };
}

// ===========================================================================
// the script fragments
// ===========================================================================

/// The bash test for "every one of these already answers on PATH".
fn all_present(tools: &[Tool]) -> String {
    tools
        .iter()
        .map(|tool| format!("command -v {} >/dev/null 2>&1", quote(tool.command)))
        .collect::<Vec<_>>()
        .join(" && ")
}

/// Shell that sets `$PROFILE` to the file a bash login shell will read.
///
/// bash tries `~/.bash_profile`, `~/.bash_login` and `~/.profile` in that order
/// and sources only the first that exists, so appending to `~/.profile` in an
/// image that ships a `~/.bash_profile` writes to a file nothing reads — the tools
/// land installed and unreachable, and (since the reuse check is `command -v`) are
/// reinstalled from scratch on every launch.
///
/// Rendered from here rather than written out per script because more than one
/// writer edits the same profile over a workspace's life, and their dedupe marks
/// only find each other in a file they both name. Two writers that answer this
/// question differently guard against different files: each reads the other's work
/// as not done, and both still exit 0.
///
/// `home` is the shell expression naming the home directory to resolve in —
/// `$HOME` for anything running as the user, `$TARGET_HOME` for the devcontainer
/// feature's installer, which edits a home it is not running in. It is the only
/// thing the writers may differ by, and a test asserts each carries this
/// rendering.
pub(crate) fn profile_resolution(home: &str) -> String {
    [
        format!(r#"if [ -f "{home}/.bash_profile" ]; then PROFILE="{home}/.bash_profile""#),
        format!(r#"elif [ -f "{home}/.bash_login" ]; then PROFILE="{home}/.bash_login""#),
        format!(r#"else PROFILE="{home}/.profile""#),
        "fi".to_owned(),
    ]
    .join("\n")
}

/// One PATH line appended to `$PROFILE` at most once, ever.
///
/// The line is written under a [`PROFILE_MARK`] comment, and the guard is an
/// exact-line match on that comment — so what decides whether the edit has
/// already been made is a line only these scripts ever write. Substring-matching
/// the directory being added cannot do that job: a base image is free to mention,
/// or even prepend, the same directory for its own reasons, and then the guard
/// skips an append the workspace still needs.
///
/// The mark names the line by a hash of its content rather than by a hand-picked
/// tag, so two different lines cannot share a mark: under a shared mark, whichever
/// line is appended second is silently dropped — its guard finds the first line's
/// mark and reads it as its own work already done — and every script involved still
/// exits 0. Identical lines sharing one mark is not a collision but the dedupe
/// itself, and twelve hex characters of SHA-256 cannot collide across the handful
/// of lines devlaunch will ever append.
///
/// Exact-line (`-x`) and fixed-string (`-F`) rather than a pattern, so nothing in
/// the mark is read as a regex and a longer line that merely contains it does not
/// count as a match.
pub(crate) fn profile_prepend(line: &str, on_failure: Option<&str>) -> String {
    let mark = format!("{PROFILE_MARK} {}", mark_digest(line));
    let tail = match on_failure {
        Some(on_failure) => format!(" || {on_failure}"),
        None => String::new(),
    };
    format!(
        "grep -qxF {mark} \"$PROFILE\" 2>/dev/null || printf '%s\\n' {mark} {line} >> \"$PROFILE\"{tail}",
        mark = quote(&mark),
        line = quote(line),
    )
}

/// The `PS1` edit that keeps a pane named after *title* for the whole session.
///
/// dl writes an OSC 2 of its own just before the handover, and for a one-shot
/// `dl <ws> -- cmd` that is the end of it: nothing in that session renders a prompt.
/// An **interactive** session overwrites it within a second, because Ubuntu's stock
/// `~/.bashrc` puts `\e]0;\u@\h: \w\a` at the *front* of `PS1` — so every prompt
/// renames the pane after the container's hostname, which is the workspace id. This
/// line is what makes dl's name the one that stands.
///
/// **Appended to `PS1`, and that is the whole mechanism.** A `PROMPT_COMMAND` cannot
/// do it: bash runs that *before* it expands and prints `PS1`, so the stock title
/// escape would be written afterwards and win. Two escapes in one prompt string are
/// resolved by the terminal in order, so the last one sets the title — which is why
/// this goes on the end rather than replacing anything. Nothing is rewritten, so a
/// `PS1` an image or a dotfile built stays exactly as it was, prompt text included:
/// the visible `user@host:path$` still names the hostname, and only the tab changes.
///
/// Interactive **bash** only, and both halves of that are load-bearing.
/// `case $- in *i*` keeps the edit out of the login shells that render no prompt,
/// which is every `dl <ws> -- cmd`, since `bash -lc` reads this file too. And
/// `$BASH_VERSION` keeps it out of the shells that would render it *literally*:
/// `$PROFILE` may be `~/.profile`, which any POSIX login shell reads, and `\[`,
/// `\e` and `\a` mean nothing to dash — `/bin/sh` on Debian and Ubuntu — so an
/// unguarded append puts the escape on screen at every prompt instead of in the
/// title. It is the same test Ubuntu's own `~/.profile` makes before it sources
/// `~/.bashrc`.
///
/// *title* is interpolated as its own quoted word rather than into the double-quoted
/// assignment, so a name holding a `$` or a backtick is text and not shell. A spec
/// cannot hold either — [`WorkspaceId`](crate::domain::workspace_id::WorkspaceId)
/// refused every character but word ones, dots, slashes and dashes — but the other
/// three placements title after a bare devpod name or a path leaf, which this crate
/// never validated.
fn profile_title_line(title: &str) -> String {
    format!(
        r#"case $- in *i*) [ -n "$BASH_VERSION" ] && PS1="$PS1\[\e]2;"{}"\a\]" ;; esac"#,
        quote(title)
    )
}

/// The twelve hex characters of SHA-256 that name one appended line.
fn mark_digest(line: &str) -> String {
    let digest = Sha256::digest(line.as_bytes());
    // Twelve characters of the same lowercase hex `hexdigest()[:12]` takes.
    digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Install one tool, unless a shell can already find it.
fn install_line(tool: &Tool) -> String {
    let args = tool
        .install_args()
        .into_iter()
        .map(|arg| quote(arg).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "if ! command -v {command} >/dev/null 2>&1; then\n  \
         echo \"devlaunch: installing {name}\"\n  \
         pixi global install {args} || failed=1\n\
         fi",
        command = quote(tool.command),
        name = tool.command,
    )
}

/// Install pixi if the image has none, since every tool here comes from it.
///
/// An arbitrary repo's container is not required to carry pixi, and without it the
/// guarantee this module makes would hold only for images that happen to have it.
/// Failure is left to the install steps to report: they will fail for a reason the
/// log can name.
const PIXI_BOOTSTRAP: &str = r#"if ! command -v pixi >/dev/null 2>&1; then
  echo "devlaunch: installing pixi"
  curl -fsSL https://pixi.sh/install.sh | bash >/dev/null 2>&1 || true
  export PATH="$HOME/.devlaunch/pixi/bin:$PATH"
fi"#;

/// Point pixi at a home of devlaunch's own (`$HOME/.devlaunch/pixi`), never `~/.pixi`.
///
/// `pixi global install` is not only an install: it is an edit to
/// `$PIXI_HOME/manifests/pixi-global.toml`, a *declarative* file that in a
/// container already has an owner — an image's, or a dotfiles repo's. Writing
/// there made devlaunch a second author of a file with one owner, and it cost
/// something in both directions:
///
/// 1. `pixi global sync` removes every env the manifest does not list, so a
///    dotfiles apply that rewrites the manifest and syncs *uninstalls* the zellij
///    the setup pass just installed — on a schedule nothing here can see, and the
///    next launch reinstalls it, forever.
/// 2. The manifest is not always a file. kinisi_ros's devcontainer symlinks
///    `~/.pixi/manifests/pixi-global.toml` onto a tracked file inside the checkout,
///    so the append landed in the user's work tree and every `git status` in the
///    workspace came up dirty. That is not one repo's quirk: dl launches arbitrary
///    repos, and an install that dirties the tree it was pointed at is the launch
///    damaging the work it exists to serve.
///
/// A home of devlaunch's own makes both unrepresentable rather than handled:
/// nothing syncs this manifest, and every path under it is one devlaunch created.
///
/// `$HOME/.devlaunch/pixi` and *not* `$HOME/.local/share/devlaunch/pixi`, which is
/// the conventional answer and the wrong one here — containers bind-mount
/// `~/.cache`, `~/.config` and `~/.local/share` straight from the host, so a prefix
/// tree under one would be shared by every container on the machine and written
/// into the host's own home. Prefixes are baked with absolute paths and two syncs
/// sharing one tree is prefix-dev/pixi#5476: the hazard the shared package cache
/// refuses to let PIXI_HOME near.
///
/// Set on every script here that runs pixi, ahead of everything else it does.
///
/// It has to come before the bootstrap and not merely before the installs: pixi's
/// own installer honours the variable too (`${PIXI_HOME:-$HOME/.pixi}` in its
/// install.sh), so the other order would leave the binary in one home and every
/// env it installs in another — the one shape of this that fails silently, since
/// both halves work alone.
///
/// Set flatly rather than defaulted to an existing value: a PIXI_HOME already in
/// the container's environment is the image's answer to where *its* globals live,
/// which is the answer this is here not to use. Never written to the login
/// profile, so a user's own `pixi global install` still goes to their `~/.pixi`.
const PIXI_HOME_EXPORT: &str = r#"export PIXI_HOME="$HOME/.devlaunch/pixi""#;

/// The bin prepend, which both scripts write. The feature installer still names
/// `~/.pixi/bin`: it is baked into an image at build time, where that path is the
/// image's own and there is no checkout to dirty.
const PIXI_BIN_LINE: &str = r#"export PATH="$HOME/.devlaunch/pixi/bin:$PATH""#;

/// The trampoline pixi writes into its bin directory does not work for packages
/// that ship a shell script, which is why the env's own bin directory is added too
/// — the same workaround `.devcontainer/claude-code/install.sh` carries, for the
/// same package.
const CLAUDE_SHIM_BIN_LINE: &str = r#"[ -d "$HOME/.devlaunch/pixi/envs/claude-shim/bin" ] && export PATH="$HOME/.devlaunch/pixi/envs/claude-shim/bin:$PATH""#;

/// The `~/.local/bin` prepend a lend writes, and it goes in *last* on purpose:
/// the container this lend exists for already has a shim earlier on PATH, so the
/// lent binary only becomes the `claude` a session finds — and the next probe only
/// reads provisioned — because `~/.local/bin` goes in front of it.
const LOCAL_BIN_LINE: &str = r#"export PATH="$HOME/.local/bin:$PATH""#;

// ===========================================================================
// the scripts
// ===========================================================================

/// The shell script that makes `tools` available in a workspace.
///
/// Idempotent and cheap on the common path: every tool already on PATH is skipped,
/// so a workspace that has been provisioned before does nothing but answer. It
/// runs under a login shell (see [`provision_tools`]), which is what puts an
/// earlier run's `~/.pixi/bin` on PATH — checked from a non-login shell every tool
/// would look missing and be reinstalled on every launch.
///
/// Exits 0 unless an install actually failed, so "nothing to do" and "all installs
/// worked" are the same answer to the caller.
///
/// `tools` parameterizes *which tools are installed* and nothing else: the profile
/// lines below are [`REQUIRED_TOOLS`]' own, so `provision_script(&[x])` still
/// writes the claude-shim prepend. That is why [`zellij_script`] assembles itself
/// from the shared fragments instead of calling this with one tool — it would
/// otherwise teach a container about a package it is not installing. Parameterize
/// the profile lines too before reusing this for anything else.
pub(crate) fn provision_script(tools: &[Tool]) -> String {
    let installs = tools
        .iter()
        .map(install_line)
        .collect::<Vec<_>>()
        .join("\n");
    let profile_lines = [
        profile_resolution("$HOME"),
        profile_prepend(PIXI_BIN_LINE, Some("failed=1")),
        profile_prepend(CLAUDE_SHIM_BIN_LINE, Some("failed=1")),
    ]
    .join("\n");
    [
        "set -u".to_owned(),
        // Everything this script prints is progress, and progress is not the
        // answer to anything: `dl <ws> -- cmd > file` on a workspace that needs
        // provisioning must put the command's output in the file and nothing
        // else. pixi writes to stdout too, so redirect once here rather than per
        // line.
        "exec >&2".to_owned(),
        "failed=0".to_owned(),
        // Everything already there: leave without touching pixi, the profile, or
        // the network. Every launch after the first takes this.
        format!("if {}; then exit 0; fi", all_present(tools)),
        PIXI_HOME_EXPORT.to_owned(),
        PIXI_BOOTSTRAP.to_owned(),
        installs,
        profile_lines,
        "exit \"$failed\"".to_owned(),
    ]
    .join("\n")
}

/// The shell script that makes `zellij` available in a workspace.
///
/// Assembled entirely out of the pieces [`provision_script`] is assembled from —
/// the pixi bootstrap, the install line, the profile resolution and the guarded
/// prepend — because every one of those is a decision this module has already made
/// once and must not make twice. What is new here is only *which* tool and *where
/// it is run from* (a setup stage rather than the cold-path install trip); nothing
/// about how a tool gets installed is restated.
///
/// The PATH prepend is written here rather than relied on from elsewhere, because
/// the container this most often lands in has not had it written: a lend edits the
/// profile for `~/.local/bin` and nothing else, and a container that reached
/// provisioned never runs [`provision_script`] at all. Writing it twice costs
/// nothing — the guard's mark is a hash of the line, so the identical line from
/// [`provision_script`] and from here share one mark and land exactly once, in
/// whichever order they arrive.
pub(crate) fn zellij_script() -> String {
    [
        // `set -u` for the same reason `provision_script` sets it: these are the
        // same fragments, and an unset variable in one of them must not quietly
        // expand to nothing in one script while aborting the other. Its
        // `exec >&2` is the one piece deliberately *not* carried — a stage cannot
        // redirect itself, since `stage_snippet` interpolates it into
        // `if <command>; then`, so the stage's own `>&2` does it.
        "set -u".to_owned(),
        "failed=0".to_owned(),
        // Already there: nothing else in this script may run. The commonest
        // answer by far, and the reason a stage on every pass is affordable.
        format!(
            "if command -v {} >/dev/null 2>&1; then exit 0; fi",
            quote(ZELLIJ_TOOL.command)
        ),
        PIXI_HOME_EXPORT.to_owned(),
        PIXI_BOOTSTRAP.to_owned(),
        install_line(&ZELLIJ_TOOL),
        profile_resolution("$HOME"),
        profile_prepend(PIXI_BIN_LINE, Some("failed=1")),
        "exit \"$failed\"".to_owned(),
    ]
    .join("\n")
}

/// The shell script that reports what a workspace already has.
///
/// It reports; it does not decide. Whether a container counts as provisioned turns
/// on two facts only the container can know — where its `claude` resolves to, and
/// where the official versions directory in its home resolves to — so those are
/// what it prints, and [`ProbeResult::parse`] says what they mean.
///
/// Exits 0 in every state: "this container has nothing" is an answer, not a
/// failure, and a probe that exits non-zero paints a red devpod `fatal … Process
/// exited with status 1` on the terminal of every cold launch, describing the probe
/// working exactly as intended. `$HOME` is expanded defensively because an image
/// that never set it would otherwise abort the script under `set -u`, and a path
/// that will not resolve is reported empty, which reads as lendable.
pub(crate) fn probe_script() -> String {
    [
        "set -u".to_owned(),
        format!("if ! {{ {} ; }}; then", all_present(&REQUIRED_TOOLS)),
        format!("  echo \"{PROBE_MARK} tools missing\""),
        "  exit 0".to_owned(),
        "fi".to_owned(),
        format!("echo \"{PROBE_MARK} tools present\""),
        // Both paths fully resolved, because the comparison they are for is an
        // equality: a home reached through a symlink resolves one side and not
        // the other, and a real install then reads lendable on every launch
        // forever.
        format!(
            "echo \"{PROBE_MARK} versions $(readlink -f \"${{HOME-}}/{CLAUDE_VERSIONS_RELPATH}\" 2>/dev/null || true)\""
        ),
        // Resolved, never run. The shim on PATH answers `command -v` just as the
        // real binary does, and telling them apart by asking either one for its
        // version would trigger, on the shim, the very ~285MB download this
        // answer exists to avoid. Where the name resolves to is the whole test.
        format!(
            "echo \"{PROBE_MARK} claude $(readlink -f \"$(command -v claude)\" 2>/dev/null || true)\""
        ),
    ]
    .join("\n")
}

// ===========================================================================
// what the probe found
// ===========================================================================

/// What one probe found in a workspace — the whole answer, in one value.
///
/// Three states rather than a boolean, because "there is a claude" and "there is a
/// claude worth keeping" are different questions, and collapsing them is the bug
/// this replaced: a container carrying the shim answered yes to the first and was
/// left owing the ~285MB download the lending exists to avoid. A boolean plus a
/// flag would have been the same two questions with a fourth, meaningless
/// combination available; three named states have no illegal one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbeResult {
    Provisioned,
    Lendable,
    Absent,
}

impl ProbeResult {
    /// The word Python's enum carried, which is also the word the probe may never
    /// print: a token on the wire would mean the container had decided.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Provisioned => "provisioned",
            Self::Lendable => "lendable",
            Self::Absent => "absent",
        }
    }

    /// The three states, for a test that asks about all of them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const ALL: [ProbeResult; 3] = [Self::Provisioned, Self::Lendable, Self::Absent];

    /// Read a probe's report. Total: anything unreadable is [`ProbeResult::Absent`].
    ///
    /// The report is marked `key value` lines rather than one token because a token
    /// would mean the container had already decided — with its own copy of the
    /// relation [`is_official_claude`] states once, for both sides. What crosses the
    /// pipe is therefore two resolved paths only the container can know, and what
    /// they mean is settled here.
    ///
    /// Marked lines, and read from anywhere in the output, because the probe runs
    /// under `bash -lc`: the container's login profile is sourced first, so an image
    /// whose profile prints a banner puts it on the same stdout.
    ///
    /// A garbled, empty or truncated report has to mean something, and absent is the
    /// only state whose worst case is harmless — provisioning is idempotent, so
    /// reading it wrongly costs a redundant round trip, where a wrong provisioned
    /// would silently skip the work the probe exists to schedule.
    pub(crate) fn parse(report: &str) -> Self {
        let found = marked_lines(report);
        if found.get("tools").map(String::as_str) != Some("present") {
            return Self::Absent;
        }
        let versions = found.get("versions").map(String::as_str).unwrap_or("");
        let claude = found.get("claude").map(String::as_str).unwrap_or("");
        if is_official_claude(versions, claude) {
            Self::Provisioned
        } else {
            Self::Lendable
        }
    }
}

/// The `key value` pairs a report carries, last value per key winning.
///
/// Exactly Python's dict comprehension over `line.strip().partition(" ")`: an
/// unmarked line is not part of the report, and a marked line with no value has an
/// empty one.
fn marked_lines(report: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for line in report.lines() {
        let line = line.trim();
        let (mark, rest) = split_once_space(line);
        if mark != PROBE_MARK {
            continue;
        }
        let (key, value) = split_once_space(rest);
        found.insert(key.to_owned(), value.trim().to_owned());
    }
    found
}

/// `str::split_once(' ')`, with Python's `partition` shape: everything before the
/// first space, and everything after it (empty when there is none).
fn split_once_space(text: &str) -> (&str, &str) {
    match text.split_once(' ') {
        Some((before, after)) => (before, after),
        None => (text, ""),
    }
}

/// Whether a resolved `claude` is a binary of the official install.
///
/// The one definition of "the official layout", and both sides of the pipe ask it:
/// the host asks it of its own filesystem to decide what it may lend, and the
/// container's probe reports its two resolved paths so it can be asked of those
/// here. Written down once because a container-side copy of this relation and a
/// host-side copy are two probes with different opinions — which is how a
/// downloader parked at `versions/latest/bin/claude` came to be trusted by one
/// while the other refused to lend the very same tree.
///
/// A *direct child*, because that is what the installer creates: one binary per
/// version, named for the version. Anything deeper is somebody else's tree that
/// merely starts with the official path, and a downloader is free to choose such a
/// path.
///
/// Both arguments must arrive resolved — `readlink -f` in the container,
/// [`resolve_as_far_as_it_exists`] on the host — because either home may be reached
/// through a symlink, and comparing a resolved binary against an unresolved home
/// makes a genuine install look like a shim forever. Anything that is not an
/// absolute path (the empty answer a container with no `readlink` gives, a
/// truncated line) is not the official layout: of the two ways to be wrong, lending
/// again costs seconds and trusting a shim ships a container that still owes the
/// download.
pub(crate) fn is_official_claude(versions_dir: &str, claude_binary: &str) -> bool {
    if !versions_dir.starts_with('/') || !claude_binary.starts_with('/') {
        return false;
    }
    let binary = posix_parts(claude_binary);
    // The parent of `/` is `/`, as `PurePosixPath` has it, so an empty tail is
    // the root rather than a missing answer.
    let parent = binary.split_last().map_or(&[][..], |(_, parent)| parent);
    parent == posix_parts(versions_dir).as_slice()
}

/// The components of an absolute POSIX path, as `PurePosixPath` reads them: empty
/// segments (a doubled or trailing slash) and `.` drop out, everything else stays
/// — including `..`, which `PurePosixPath` also keeps.
fn posix_parts(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect()
}

// ===========================================================================
// the setup pass's stages
// ===========================================================================

/// How loudly *this* stage's failure is worth saying.
///
/// Most stages warrant a warning — naming a stage that did not work is the whole
/// legibility claim of folding them into one trip. The hostname does not:
/// `sudo hostname` cannot succeed without CAP_SYS_ADMIN, which Docker drops by
/// default, so failure is the majority case and a warning on most cold launches
/// would erode the signal a warning carries. It is still reported by name, which is
/// more than the silently-discarded boolean it replaces.
///
/// Two named levels rather than Python's `logging` integer, because core writes no
/// output: which log level these become is the `dl` binary's rendering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FailureLevel {
    /// Python's `logging.WARNING`, and what a stage gets unless it asks otherwise.
    #[default]
    Warning,
    /// Python's `logging.INFO`.
    Info,
}

/// One independent step the setup pass carries in front of the probe.
///
/// `command` is a shell command the container is asked to run; whether it worked is
/// reported by the composer on a marked line, never by the stage itself, so a stage
/// is an ordinary command rather than something that has to know about this
/// protocol.
///
/// One constraint on what a stage may be, unenforced, because the protocol
/// shares the stage's stdout ([`StageName`] carries the other one to compile
/// time):
///
/// - A stage's own stdout is *not* redirected — [`probe_script`]'s output comes
///   behind the stages, not in front of them — so a stage that prints a
///   [`PROBE_MARK`] line of its own is read as protocol. Both readers keep the last
///   value for a key, so an *earlier* stage's outcome can be overwritten from here.
///   Keep stages silent, or redirect their output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stage {
    pub(crate) name: StageName,
    pub(crate) command: String,
    pub(crate) failure_level: FailureLevel,
}

impl Stage {
    /// A stage whose failure is worth a warning — the default a new stage gets.
    pub(crate) fn new(name: StageName, command: impl Into<String>) -> Self {
        Self {
            name,
            command: command.into(),
            failure_level: FailureLevel::default(),
        }
    }

    /// A stage that asks to be quieter about failing, because failing is its
    /// majority case.
    #[must_use]
    pub(crate) fn quieter(mut self) -> Self {
        self.failure_level = FailureLevel::Info;
        self
    }
}

/// The stages one setup pass runs in `workspace`, in order.
///
/// Built per pass rather than declared as a constant because a stage's command
/// names the workspace, and a workspace id is not known until there is one — and
/// because whether the zellij stage is asked for at all depends on the opt-out,
/// which the process may see differently between passes.
///
/// The zellij stage is gated on the tools opt-out, unlike the hostname above it:
/// installing zellij *is* tool provisioning, where naming a container is not, and a
/// machine that turned tool installs off has not thereby asked for anonymous
/// containers. [`provision_tools`] draws the same line for the same reason.
///
/// It is gated on `zellij` as well, and the two are an **and**: the stage runs only
/// when both switches say install. `DEVLAUNCH_NO_TOOLS` covers zellij because
/// installing it is tool provisioning; `DEVLAUNCH_NO_ZELLIJ` covers only zellij, so
/// a host can keep the `gh`/`claude` guarantee and still stop the stage. Neither
/// touches the hostname, which is not tools work under either variable.
/// `title` is the name a shell in this container should keep putting on the terminal,
/// or `None` for a launch that wants none — which is `DEVLAUNCH_NO_TITLE`, or a spec
/// that resolved no triple, both decided by the caller. Last of the three because it
/// is the one that is not a switch, and neither tools switch touches it either:
/// naming a pane is no more tool provisioning than naming a container is.
///
/// **The stage runs on the pass, so the name is installed when a workspace enters
/// Running and not on every attach.** A workspace already up keeps whatever its
/// profile was given, so `DEVLAUNCH_NO_TITLE=1 dl <ws>` silences dl's own write and
/// leaves the prompt's — `dl <ws> recreate` is what re-decides it. That is the same
/// bargain the hostname stage makes, and for the same reason: the alternative is a
/// round trip per attach, which is what moving the hostname off the attach bought
/// back (#157).
pub(crate) fn setup_stages(
    workspace: &str,
    tools: ToolsSwitch,
    zellij: ZellijSwitch,
    title: Option<&str>,
) -> Vec<Stage> {
    let mut stages = vec![
        // The hostname appears in the bash prompt (user@hostname:path$), which is
        // what tells a session which project and branch it is in. bash reads it
        // once when the shell starts, so it has to be set before the session dl
        // hands over — which is why it rides the `up`'s own trip rather than the
        // attach's.
        //
        // The id's readable half, not the id: the identity suffix is what makes an
        // id address one workspace, and a UTS name addresses nothing. See
        // [`hostname_of`] for what dropping it costs.
        Stage::new(
            HOSTNAME_STAGE,
            format!("sudo hostname {}", quote(hostname_of(workspace))),
        )
        .quieter(),
    ];
    if let (ToolsSwitch::Install, ZellijSwitch::Install) = (tools, zellij) {
        stages.push(Stage::new(
            ZELLIJ_STAGE,
            // A nested `bash -c` because a stage is interpolated into
            // `if <command>; then`, which is one line, and this script is not. It
            // inherits the pass's exported PATH rather than sourcing the profile
            // again: the pass is already a login shell, so an installed zellij is
            // on PATH here, and a second `-l` would re-run a chatty image's
            // profile on every launch.
            //
            // Redirected to stderr as a whole, because a stage shares the probe's
            // stdout (see [`Stage`]) and pixi is loud — both readers split marked
            // lines on spaces, so a package manager's progress on that stream is
            // one unlucky line from being read as protocol. To stderr and not to
            // /dev/null because the setup pass captures stdout: an install
            // redirected into that buffer is a cold launch that looks hung with
            // nothing to show for it.
            format!("bash -c {} >&2", quote(&zellij_script())),
        ));
    }
    if let Some(title) = title {
        stages.push(
            Stage::new(
                TITLE_STAGE,
                // Two statements, so a nested `bash -c` for the reason the zellij
                // stage has one: a stage is interpolated into `if <command>; then`,
                // which is one line. The profile is resolved and appended to exactly
                // as the PATH writers do it, so every edit finds the others' dedupe
                // marks in the one file bash will actually read.
                format!(
                    "bash -c {}",
                    quote(
                        &[
                            profile_resolution("$HOME"),
                            profile_prepend(&profile_title_line(title), None),
                        ]
                        .join("\n")
                    )
                ),
            )
            // Quieter for the hostname stage's reason: an image that will not let
            // this be written is a tab with a duller name, not a launch to warn
            // about.
            .quieter(),
        );
    }
    stages
}

/// One stage, with the composer's report of how it went wrapped round it.
///
/// No `&&`, no `set -e`: the `if` contains the failure to this stage, so the stages
/// after it and the probe behind them all still run. `$?` inside the `else` is the
/// command's own status, which is the number the host needs to tell "the image will
/// not let me" from "the command is not even there".
fn stage_snippet(stage: &Stage) -> String {
    [
        format!("if {}; then", stage.command),
        format!(
            "  echo \"{PROBE_MARK} {STAGE_KEY} {} {STAGE_OK_WORD}\"",
            stage.name.as_str()
        ),
        "else".to_owned(),
        format!(
            "  echo \"{PROBE_MARK} {STAGE_KEY} {} {STAGE_FAILED_WORD} $?\"",
            stage.name.as_str()
        ),
        "fi".to_owned(),
    ]
    .join("\n")
}

/// The one script a cold launch's setup pass sends: stages, then the probe.
///
/// `stages` is a parameter rather than something this rebuilds, so a caller that
/// already has the list sends *that* list rather than a second one built from a
/// second reading of the opt-out: a script whose stages disagreed with the list its
/// outcomes are matched against would report a phantom "not reached".
///
/// Composed here, on the host, out of [`probe_script`] **verbatim** plus stage
/// snippets that know nothing about it. Nothing about the probe is copied or
/// re-expressed: the relation that decides what a container's two resolved paths
/// mean is stated once, in [`is_official_claude`], and a rewritten probe would be a
/// second copy of it.
///
/// The stages go **in front of** the probe, and that is not cosmetic: the probe
/// exits early when a tool is missing, which is the commonest answer on the very
/// launches this fold exists for, so a stage placed behind it would report "not
/// reached" most of the time.
///
/// Exits 0 in every state, like the probe it carries, so a non-zero `devpod ssh`
/// keeps meaning the transport failed and never that a stage did.
pub(crate) fn setup_script(stages: &[Stage]) -> String {
    let mut parts: Vec<String> = stages.iter().map(stage_snippet).collect();
    parts.push(probe_script());
    parts.join("\n")
}

// ===========================================================================
// how each stage went
// ===========================================================================

/// What became of one stage the pass was asked to run.
///
/// Three states, and the third is an *absence*: a stage the script died in front
/// of, a report truncated before its line, a line too garbled to read — none of
/// them says the stage worked, and none says it failed with a status either. Kept as
/// its own arm rather than folded into a bool or a sentinel status, because "never
/// ran" and "ran fine" are exactly the two the fold must not be able to confuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StageResult {
    /// The stage ran and exited 0.
    Ok,
    /// The stage ran and exited non-zero, with `status` as its exit status.
    Failed { status: i32 },
    /// No readable outcome for this stage came back.
    NotReached,
}

/// One stage's name and what became of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StageOutcome {
    pub(crate) stage: StageName,
    pub(crate) result: StageResult,
}

/// How each of `stages` went, in the order they were asked, from `report`.
///
/// Total, and it errs towards speaking: anything that is not a readable outcome is
/// the *absence* of one, and every outcome that is not [`StageResult::Ok`] is
/// reported by name — so an unreadable line is named rather than passed over.
pub(crate) fn stage_outcomes(report: &str, stages: &[Stage]) -> Vec<StageOutcome> {
    let mut reported: BTreeMap<&str, &str> = BTreeMap::new();
    for line in report.lines() {
        let line = line.trim();
        let (mark, rest) = split_once_space(line);
        if mark != PROBE_MARK {
            continue;
        }
        let (key, value) = split_once_space(rest);
        if key != STAGE_KEY {
            continue;
        }
        let (name, status) = split_once_space(value.trim());
        reported.insert(name, status.trim());
    }
    stages
        .iter()
        .map(|stage| StageOutcome {
            stage: stage.name,
            result: read_outcome(reported.get(stage.name.as_str()).copied()),
        })
        .collect()
}

/// One stage's reported status as a value; unreadable means not reached.
///
/// Python reads the status with `isdigit()` and `int()`, which accepts any Unicode
/// decimal and then rejects some of what it accepted — `int("²")` raises inside the
/// parse. Here the status is what a POSIX `$?` can actually be: ASCII digits that
/// fit an exit status. Everything else is the absence of an outcome, which is the
/// reading the report is used for.
fn read_outcome(status: Option<&str>) -> StageResult {
    let status = status.unwrap_or("");
    if status == STAGE_OK_WORD {
        return StageResult::Ok;
    }
    let (word, code) = split_once_space(status);
    let code = code.trim();
    if word == STAGE_FAILED_WORD
        && !code.is_empty()
        && code.chars().all(|character| character.is_ascii_digit())
        && let Ok(status) = code.parse::<i32>()
    {
        return StageResult::Failed { status };
    }
    StageResult::NotReached
}

// ===========================================================================
// what the host can lend
// ===========================================================================

/// The host's own tool binaries, ready to lend to a container.
///
/// Two named binaries rather than Python's tuple of arcname pairs, because the
/// payload is all or nothing: a host missing either tool falls back to the network
/// path for both, so "a payload with one member" and "a payload with three" are
/// states nothing should be able to build. `claude_version` is carried because the
/// transfer script has to create the `~/.local/bin/claude` symlink itself — the
/// host's symlink points through the host's absolute home and would dangle
/// anywhere else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HostPayload {
    pub(crate) claude_version: String,
    pub(crate) claude_binary: PathBuf,
    pub(crate) gh_binary: PathBuf,
}

impl HostPayload {
    /// Where the lent claude lands under the container's `$HOME`.
    pub(crate) fn claude_relpath(&self) -> String {
        format!("{CLAUDE_VERSIONS_RELPATH}/{}", self.claude_version)
    }

    /// The host files, each with the home-relative tar arcname it lands at, in the
    /// order the archive carries them. Home-relative because an absolute arcname
    /// would unpack into the host's usernamed home, which does not exist in the
    /// container.
    pub(crate) fn members(&self) -> Vec<(&Path, String)> {
        vec![
            (self.claude_binary.as_path(), self.claude_relpath()),
            (self.gh_binary.as_path(), GH_RELPATH.to_owned()),
        ]
    }
}

/// The two host facts the payload is resolved from.
///
/// Parameters rather than reads of the machine, for the reason
/// [`crate::clients::gh`]'s `HostEnv` gives: the decision is then a function of its
/// inputs, and a test states the host it means instead of monkeypatching
/// `Path.home` the way the Python tests have to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostLayout {
    /// The home directory the official claude install would be under.
    pub(crate) home: PathBuf,
    /// What a PATH search for `gh` answered, if anything.
    pub(crate) gh_on_path: Option<PathBuf>,
}

impl HostLayout {
    /// This machine, as `Path.home()` and `shutil.which("gh")` read it.
    pub fn from_env() -> Option<Self> {
        Some(Self {
            home: crate::osext::home_dir()?,
            gh_on_path: which("gh"),
        })
    }
}

/// The host's official claude install: (version, binary), or nothing.
///
/// The official installer keeps one binary per version under
/// `~/.local/share/claude/versions/` and points `~/.local/bin/claude` at the
/// current one. Anything else on PATH answering to `claude` — the pixi shim, a
/// wrapper script, a downloader that parked itself deeper inside that same
/// directory — is exactly the kind of downloader this transfer exists to skip, so
/// only the official layout counts, and what counts as the official layout is
/// [`is_official_claude`]: the same relation the container's report is read
/// through, so the two sides cannot come to different answers about one tree.
pub(crate) fn claude_source(home: &Path) -> Option<(String, PathBuf)> {
    let link = home.join(".local/bin/claude");
    // Strict, as Python's `resolve(strict=True)`: a link that points nowhere is
    // nothing to lend.
    let target = std::fs::canonicalize(&link).ok()?;
    let versions_dir = resolve_as_far_as_it_exists(&home.join(CLAUDE_VERSIONS_RELPATH));
    if !is_official_claude(&versions_dir.to_string_lossy(), &target.to_string_lossy()) {
        return None;
    }
    if !is_executable_file(&target) {
        return None;
    }
    let version = target.file_name()?.to_string_lossy().into_owned();
    Some((version, target))
}

/// The host's real `gh` binary, or nothing.
///
/// A PATH search can answer with a pixi trampoline — a small launcher that
/// re-execs the env's binary named in a JSON file beside it — and copying the
/// trampoline without its configuration copies nothing that runs. When the sidecar
/// is there, the answer is the binary it names; a sidecar that cannot be read makes
/// the whole source nothing rather than shipping a launcher that will fail inside
/// the container.
pub(crate) fn gh_source(found: Option<&Path>) -> Option<PathBuf> {
    let found = found?;
    let sidecar = found
        .parent()?
        .join("trampoline_configuration")
        .join("gh.json");
    let mut path = found.to_path_buf();
    if sidecar.is_file() {
        let text = std::fs::read_to_string(&sidecar).ok()?;
        let document: serde_json::Value = serde_json::from_str(&text).ok()?;
        // The key has to be there and has to be a string: Python's `["exe"]`
        // raises for a missing key, and `Path(2)` raises for a number, so both
        // land on the same "lend nothing" answer.
        path = PathBuf::from(document.get("exe")?.as_str()?);
    }
    if is_executable_file(&path) {
        Some(path)
    } else {
        None
    }
}

/// What this host can lend a fresh container, or nothing.
///
/// All or nothing: a host missing either tool falls back to the network path for
/// both, rather than growing a per-tool matrix of half-lent states that the fallback
/// script would then have to reason about.
pub(crate) fn host_payload(layout: &HostLayout) -> Option<HostPayload> {
    let (claude_version, claude_binary) = claude_source(&layout.home)?;
    let gh_binary = gh_source(layout.gh_on_path.as_deref())?;
    Some(HostPayload {
        claude_version,
        claude_binary,
        gh_binary,
    })
}

/// Whether `path` is a file this process could execute — `os.access(X_OK)` over a
/// file, which is what both resolvers ask before lending anything.
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The first executable named `program` on PATH — `shutil.which`, as the gh
/// resolver asks it.
fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return is_executable_file(&path).then_some(path);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        // An empty PATH entry means the current directory, as it does for the
        // shell.
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        let candidate = dir.join(program);
        is_executable_file(&candidate).then_some(candidate)
    })
}

/// `Path.resolve()` without `strict`: canonicalize the longest existing prefix and
/// keep the rest as it was written.
///
/// Python's non-strict `resolve()` is what the versions directory is read through,
/// and it answers for a path that does not exist yet — a home reached through a
/// symlink still resolves, which is the half that matters here, because comparing a
/// resolved binary against an unresolved home makes a genuine install look like a
/// shim forever.
fn resolve_as_far_as_it_exists(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    let mut tail = Vec::new();
    let mut cursor = path;
    while let Some(parent) = cursor.parent() {
        let Some(name) = cursor.file_name() else {
            break;
        };
        tail.push(name);
        if let Ok(resolved) = std::fs::canonicalize(parent) {
            let mut answer = resolved;
            for name in tail.iter().rev() {
                answer.push(name);
            }
            return answer;
        }
        cursor = parent;
    }
    path.to_path_buf()
}

// ===========================================================================
// the transfer
// ===========================================================================

/// The shell script that receives the tar stream and wires the tools up.
///
/// Plain `bash -c` with explicit paths, not `-lc`: nothing here depends on a
/// profile, and the profile is being edited by this very script. The two version
/// checks in the middle are the arch/libc gate — a binary lent to a container that
/// cannot run it fails here, the trip reports failure, and the caller falls back to
/// the network install.
///
/// Everything lands in a staging directory first, and the container is only changed
/// once the lent binaries have proved they run in it. Unpacking straight into
/// `$HOME` cost more than a failed transfer should: the PATH edit and the `claude`
/// symlink survived a failing gate, and the network fallback that follows decides
/// what to install with `command -v` — which a broken binary satisfies. So a
/// container that could not run the lent claude ended up with a permanently broken
/// one, the fallback installing nothing, and every later probe reporting success.
pub(crate) fn transfer_script(payload: &HostPayload) -> String {
    let version = &payload.claude_version;
    let claude_rel = payload.claude_relpath();
    let profile_lines = [
        profile_resolution("$HOME"),
        profile_prepend(LOCAL_BIN_LINE, None),
    ]
    .join("\n");
    [
        "set -eu".to_owned(),
        // Progress belongs on stderr for the same reason provision_script sends it
        // there: stdout may be a `dl <ws> -- cmd > file`.
        "exec >&2".to_owned(),
        format!("echo \"devlaunch: lending claude {version} and gh from the host\""),
        "STAGE=\"$HOME/.devlaunch-lend\"".to_owned(),
        "trap 'rm -rf \"$STAGE\"' EXIT".to_owned(),
        "rm -rf \"$STAGE\"".to_owned(),
        "mkdir -p \"$STAGE\"".to_owned(),
        "tar xf - -C \"$STAGE\"".to_owned(),
        // The gate: prove the lent binaries actually run here, while nothing
        // outside the staging directory has been touched.
        format!("\"$STAGE/{claude_rel}\" --version >/dev/null"),
        format!("\"$STAGE/{GH_RELPATH}\" --version >/dev/null"),
        // Proven. Now they can be moved into place.
        format!("mkdir -p \"$HOME/.local/bin\" \"$HOME/{CLAUDE_VERSIONS_RELPATH}\""),
        format!("mv -f \"$STAGE/{claude_rel}\" \"$HOME/{claude_rel}\""),
        format!("mv -f \"$STAGE/{GH_RELPATH}\" \"$HOME/{GH_RELPATH}\""),
        // The host's own symlink points through the host's home, so the link is
        // made here, against this container's $HOME.
        format!("ln -sfn \"$HOME/{claude_rel}\" \"$HOME/.local/bin/claude\""),
        profile_lines,
    ]
    .join("\n")
}

/// Why the host's binaries could not be bundled for the stream.
///
/// Its own type because the trip that follows never happened: Python catches
/// `OSError`, `tarfile.TarError` and `ValueError` around the same work and logs one
/// debug line, because this runs *after* a successful `devpod up` and letting one
/// out would cost the user the workspace they just built over a convenience that is
/// allowed to fail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleFailed {
    /// A member could not be opened, stat'ed or read.
    Unreadable { path: PathBuf, error: OsFailure },
    /// The bundle itself could not be created or written.
    NotWritten { path: PathBuf, error: OsFailure },
    /// A member ran out of bytes before the size its own metadata declared. The
    /// header is already written, so the archive cannot be finished honestly.
    Truncated { path: PathBuf },
    /// An arcname no ustar header can carry (100 bytes with room for the
    /// terminator). Python's tarfile would write a pax extended header instead;
    /// reaching it needs a claude version whose name is ~70 characters long.
    NameTooLong { arcname: String },
}

/// One tar block, and the record size Python's `tarfile` pads a closed archive to.
const TAR_BLOCK: usize = 512;
const TAR_RECORD: usize = 10240;

/// Write the payload as a plain (uncompressed) tar at `out`.
///
/// Uncompressed on purpose: the stream crosses a local pipe into a container on the
/// same disk, where gzip would cost seconds of CPU to save transfer time nobody is
/// paying.
///
/// Streamed in fixed-size chunks, never read into memory: the payload runs to
/// hundreds of megabytes, which is also why the trip that sends it hands the child
/// a file descriptor rather than bytes.
pub(crate) fn write_payload_tar(payload: &HostPayload, out: &Path) -> Result<(), BundleFailed> {
    // The one part of the lend that is neither a round trip nor free (#158:
    // ~0.13s warm, ~2.6s cold); the trips either side of it name themselves.
    let _span = timing::span("tools tar");
    let file = File::create(out).map_err(|error| BundleFailed::NotWritten {
        path: out.to_path_buf(),
        error: OsFailure::from(&error),
    })?;
    let mut sink = BufWriter::new(file);
    let mut written = 0usize;
    for (source, arcname) in payload.members() {
        written += append_member(&mut sink, out, source, &arcname)?;
    }
    // Two zero blocks, then padding to a whole record: what `tarfile.close()`
    // writes, so every tar that reads Python's archive reads this one.
    let zeros = [0u8; TAR_BLOCK];
    let wrote = |result: std::io::Result<()>| {
        result.map_err(|error| BundleFailed::NotWritten {
            path: out.to_path_buf(),
            error: OsFailure::from(&error),
        })
    };
    wrote(sink.write_all(&zeros))?;
    wrote(sink.write_all(&zeros))?;
    written += 2 * TAR_BLOCK;
    let remainder = written % TAR_RECORD;
    if remainder != 0 {
        wrote(sink.write_all(&vec![0u8; TAR_RECORD - remainder]))?;
    }
    wrote(sink.flush())
}

/// Append one file to the archive, returning how many bytes it added.
fn append_member(
    sink: &mut impl Write,
    out: &Path,
    source: &Path,
    arcname: &str,
) -> Result<usize, BundleFailed> {
    let unreadable = |error: &std::io::Error| BundleFailed::Unreadable {
        path: source.to_path_buf(),
        error: OsFailure::from(error),
    };
    let not_written = |error: &std::io::Error| BundleFailed::NotWritten {
        path: out.to_path_buf(),
        error: OsFailure::from(error),
    };
    let mut file = File::open(source).map_err(|error| unreadable(&error))?;
    let meta = file.metadata().map_err(|error| unreadable(&error))?;
    let size = meta.len();
    sink.write_all(&ustar_header(arcname, &meta)?)
        .map_err(|error| not_written(&error))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut left = size;
    while left > 0 {
        let want = usize::try_from(left)
            .unwrap_or(buffer.len())
            .min(buffer.len());
        let read = file
            .read(&mut buffer[..want])
            .map_err(|error| unreadable(&error))?;
        if read == 0 {
            // The header has already declared how long this member is, so a
            // member that shrank mid-copy is an archive nothing can finish
            // honestly — which is the `tarfile` "unexpected end of data" case.
            return Err(BundleFailed::Truncated {
                path: source.to_path_buf(),
            });
        }
        sink.write_all(&buffer[..read])
            .map_err(|error| not_written(&error))?;
        left -= read as u64;
    }
    let padding = (TAR_BLOCK - (size as usize % TAR_BLOCK)) % TAR_BLOCK;
    if padding != 0 {
        sink.write_all(&vec![0u8; padding])
            .map_err(|error| not_written(&error))?;
    }
    Ok(TAR_BLOCK + size as usize + padding)
}

/// One ustar header block for a regular file.
///
/// ustar rather than any of tar's extensions, because the arcnames are two fixed
/// shapes well under the 100-byte name field and the receiving end is `tar xf -`:
/// the plainest format every tar reads is the whole requirement.
fn ustar_header(arcname: &str, meta: &Metadata) -> Result<[u8; TAR_BLOCK], BundleFailed> {
    let name = arcname.as_bytes();
    if name.len() >= 100 {
        return Err(BundleFailed::NameTooLong {
            arcname: arcname.to_owned(),
        });
    }
    let mut header = [0u8; TAR_BLOCK];
    header[..name.len()].copy_from_slice(name);
    fn put(header: &mut [u8; TAR_BLOCK], at: usize, text: &str) {
        header[at..at + text.len()].copy_from_slice(text.as_bytes());
    }
    put(
        &mut header,
        100,
        &format!("{:07o}\0", meta.permissions().mode() & 0o7777),
    );
    put(&mut header, 108, &format!("{:07o}\0", meta.uid()));
    put(&mut header, 116, &format!("{:07o}\0", meta.gid()));
    put(&mut header, 124, &format!("{:011o}\0", meta.len()));
    // A negative mtime is a clock nothing can encode in an octal field; 0 is the
    // epoch, which is as honest as an unrepresentable date gets.
    put(&mut header, 136, &format!("{:011o}\0", meta.mtime().max(0)));
    // The checksum is computed with this field read as spaces, so it starts that
    // way and is overwritten once the sum is known.
    put(&mut header, 148, "        ");
    header[156] = b'0';
    put(&mut header, 257, "ustar\0");
    put(&mut header, 263, "00");
    let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    // "%06o\0 " — six octal digits, a NUL, then a space, as `tarfile` writes it.
    put(&mut header, 148, &format!("{sum:06o}\0 "));
    Ok(header)
}

// ===========================================================================
// the flow
// ===========================================================================

/// Which of a launch's two provisioning moments this pass is.
///
/// The distinction exists because one of them may be skipped and the other may
/// never be, and nothing else in the call tells them apart: both arrive with a
/// workspace id and a runner, at a container devpod reports as running.
///
/// **The hostname is what separates them.** `sudo hostname` is a stage of the
/// pass, and the name it sets lives in the container's UTS namespace, which docker
/// rebuilds from the container's config on every `start`. So a container that has
/// just been through `devpod up` — created, or stopped and started again — has lost
/// the name and has to be told it again, before the session `dl` is about to hand
/// over reads it into a prompt. That pass travels, always, whatever any cache says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassOccasion {
    /// The pass that follows this launch's own `devpod up`. The container is new
    /// to this boot: it has no hostname yet, and whatever was true of the last
    /// container under this id is not evidence about this one.
    AfterUp,
    /// The pass over a container that was already running when the launch arrived —
    /// the `dl <ws> up` top-up and the sibling-won path. Nothing has restarted, so
    /// the hostname set by the `up` that started it is still standing, and the only
    /// question left is whether the tools are there. That question has an answer on
    /// disk ([`verdict_cache`]) whenever a previous pass found them.
    TopUp,
}

/// Something [`provision_tools`] did that the `dl` binary may want to report.
///
/// One arm per `logging.*` call `tools.py` makes, carrying what that line
/// interpolated and nothing else; the words and the levels are the binary's. A
/// stage that worked produces no event at all — a launch that worked has nothing to
/// say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProvisionEvent {
    /// `tools.py:926` — `"%s: the %s setup stage exited %s."`, at the stage's own
    /// level.
    StageFailed {
        workspace: String,
        stage: &'static str,
        status: i32,
        loudness: FailureLevel,
    },
    /// `tools.py:934` — `"%s: the %s setup stage did not report; it may not have
    /// run."`, at the stage's own level.
    StageNotReported {
        workspace: String,
        stage: &'static str,
        loudness: FailureLevel,
    },
    /// `tools.py:1063` (debug) — `"%s is set; not installing tools into %s"`.
    ProvisioningDisabled { workspace: String },
    /// `tools.py:998` (debug) — `"Could not bundle host tools: %s"`.
    PayloadNotBundled { failure: BundleFailed },
    /// `tools.py:1100` (debug) — `"Could not install tools into %s: %s"`, for the
    /// OS refusals Python catches as `OSError`.
    TripRefused { workspace: String, refusal: NotRun },
    /// `tools.py:1106` (warning) — `"Could not install %s into %s; the session
    /// will start without them."`, where the first `%s` is the tool commands
    /// joined with `" and "`.
    NotInstalled {
        workspace: String,
        tools: Vec<&'static str>,
        exit: Exit,
    },
}

/// How a workspace's provisioning turned out.
///
/// Python answers `True`/`False` — "are the tools there" — which throws away
/// *which* of the four ways of being there this was, and merges an opt-out with a
/// failed install. Every arm here is one of Python's return statements, and
/// [`Provisioning::tools_present`] is the bool it returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provisioning {
    /// The probe found the official layout: one trip, nothing to do.
    AlreadyProvisioned,
    /// No trip at all: this was a [`PassOccasion::TopUp`] over a container a
    /// previous pass found provisioned, and devpod's own records say that container
    /// has not been through an `up` since ([`verdict_cache`]).
    ///
    /// Its own arm rather than a second way of saying [`Self::AlreadyProvisioned`],
    /// because the two are the same verdict reached by opposite means and only one
    /// of them is evidence: one asked the container just now, the other asked a file
    /// the host wrote earlier. A caller that wanted to re-record the verdict, count
    /// the round trips, or explain a launch's timing needs to be able to tell them
    /// apart, and a shared arm is how it would come to record a verdict it never
    /// observed.
    CachedProvisioned,
    /// The host's own binaries landed in the container.
    Lent,
    /// The container has both tools and a claude that is not the official install,
    /// and the lend either had nothing to send or would not run there.
    ///
    /// The network fallback cannot help: it decides what to install with its own
    /// `command -v` guards, which both tools already satisfy, so the third trip
    /// would install nothing. Stop with what is there.
    ShimKept,
    /// The network install ran and reported success.
    Installed,
    /// The network install ran and failed. Named, not raised: the workspace is up
    /// and the user asked for a session, not for an install.
    InstallRefused { exit: Exit },
    /// `DEVLAUNCH_NO_TOOLS` said not to. The pass still ran — naming a container is
    /// not tools work — so the container is named and has whatever tools it had.
    Disabled,
    /// A trip could not be made at all: the OS refused it, or it outstayed a
    /// deadline. Python's `except OSError` answer, and like it this costs the
    /// workspace its tools rather than its session.
    TripRefused { refusal: NotRun },
}

impl Provisioning {
    /// Whether the tools are now there — the bool Python returns. Only this
    /// module's tests read it; the binary matches the arms directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn tools_present(&self) -> bool {
        match self {
            Self::AlreadyProvisioned
            | Self::CachedProvisioned
            | Self::Lent
            | Self::ShimKept
            | Self::Installed => true,
            Self::InstallRefused { .. } | Self::Disabled | Self::TripRefused { .. } => false,
        }
    }
}

/// devpod is not installed.
///
/// The one failure that is not an arm of [`Provisioning`], because it is not a
/// failure of the thing being attempted: Python gives `DevpodNotInstalled` a class
/// that is deliberately not an `OSError` so that the `except OSError` around this
/// flow never swallows it, and the `dl` binary renders it as exit 127.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevpodMissing;

/// Provision [`REQUIRED_TOOLS`] into `workspace`. Answers how that turned out.
///
/// **Provision, not ensure**, and the distinction is the whole answer. This probes,
/// lends and installs, and every one of those may come up empty — a host with
/// nothing to lend, a container the lent binaries will not run in, an opt-out that
/// forbids installing at all, a network install that fails. It then reports what it
/// found, and the caller launches the session either way.
///
/// Not parametric over a tool set, because the probe is not: it asks about the fixed
/// pair this module can lend, so a caller-supplied set would be probed for one thing
/// and installed for another.
///
/// Three trips at most, each earning the next: the setup pass (the only trip a
/// provisioned workspace ever pays, and the one the container's stages ride in),
/// then the host lending its own binaries, then the network install for a host with
/// nothing to lend or a container the lent binaries cannot run in. Which of them run
/// is decided by the probe's three-state answer, and only a genuinely absent
/// container ever reaches the third.
///
/// The pass is not gated on the opt-out, and only what follows it is: the stages the
/// pass carries are not tools work, so a machine that has turned tool provisioning
/// off must not thereby have turned container naming off. The answer still speaks to
/// the same question — whether the tools are there — and says no when it was told not
/// to install any.
///
/// `host` is `None` for a machine with no home directory to look in — nothing to
/// lend, rather than nothing to do: the pass still runs, because the stages it
/// carries are not tools work, and the network path still follows. Python has no such
/// case because `Path.home()` raises there, which would cost the workspace its
/// session over a convenience.
///
/// Version drift is deliberately not handled: a real claude already in the container
/// is left alone whatever its version. Keeping versions in sync would make this a
/// package manager, which "the host first, the network second" is explicitly not.
///
/// The network payload goes through `bash -lc` for the same reason the attach wraps
/// its own: devpod runs `--command` under a shell that sources no profile, so PATH
/// would be missing the pixi directory this module itself installs into. Its output
/// is not captured — a cold install streams a ~300MB binary or downloads pixi and two
/// packages, which with nothing on the terminal reads as a hung `dl`, and the
/// scripts' own progress lines are worth nothing in a buffer.
///
/// `verdicts` is `None` for a caller that keeps no host-side memory of past passes —
/// every pass then travels, which is what this did before the cache existed. When
/// there is one, it is read on a [`PassOccasion::TopUp`] and written after any pass
/// that probed provisioned; see [`verdict_cache`] for what makes a remembered
/// verdict still true.
/// `title` sits beside `switches` rather than inside it, and that is the lifetime
/// and not a judgement about where it belongs: [`Switches`] is `Copy` and built by
/// `from_env`, and a borrowed field would make it `Switches<'_>` everywhere it is
/// held. One more positional parameter is the cheaper of the two, and the parameter
/// it follows is the one it goes with.
#[allow(clippy::too_many_arguments)]
pub fn provision_tools(
    runner: &dyn Runner,
    workspace: &str,
    occasion: PassOccasion,
    switches: Switches,
    title: Option<&str>,
    host: Option<&HostLayout>,
    verdicts: Option<&VerdictCache>,
    events: &mut dyn Notices<ProvisionEvent>,
) -> Result<Provisioning, DevpodMissing> {
    timing::stage_result(timing::Stage::Tools, || {
        provision(
            runner, workspace, occasion, switches, title, host, verdicts, events,
        )
    })
}

/// [`provision_tools`] without the stage guard around it.
#[allow(clippy::too_many_arguments)]
fn provision(
    runner: &dyn Runner,
    workspace: &str,
    occasion: PassOccasion,
    switches: Switches,
    title: Option<&str>,
    host: Option<&HostLayout>,
    verdicts: Option<&VerdictCache>,
    events: &mut dyn Notices<ProvisionEvent>,
) -> Result<Provisioning, DevpodMissing> {
    // Before the trip, because a trip is the whole of what this saves; and before
    // the opt-out check below, because that check exists to report what the pass
    // did with the stages it carried, and here there is no pass. On a top-up there
    // is nothing for the stages to do either: the container never stopped, so the
    // hostname the `up` that started it set is still the one it has.
    if let (PassOccasion::TopUp, Some(verdicts)) = (occasion, verdicts)
        && verdicts.trusted(workspace, switches)
    {
        return Ok(Provisioning::CachedProvisioned);
    }

    // Before the pass, not after it: see [`VerdictCache::observe`].
    let observed = verdicts.and_then(|verdicts| verdicts.observe(workspace));

    let found = match setup_pass(runner, workspace, switches, title, events) {
        Ok(found) => found,
        Err(refusal) => return refused(workspace, refusal, events),
    };

    if let ToolsSwitch::Skip = switches.tools {
        events.say(ProvisionEvent::ProvisioningDisabled {
            workspace: workspace.to_owned(),
        });
        return Ok(Provisioning::Disabled);
    }

    if let ProbeResult::Provisioned = found {
        // The one outcome worth remembering, and the reasons the others are not are
        // in [`verdict_cache`]'s own note.
        if let (Some(verdicts), Some(observed)) = (verdicts, observed) {
            verdicts.record(workspace, observed, switches);
        }
        return Ok(Provisioning::AlreadyProvisioned);
    }

    if let Some(payload) = host.and_then(host_payload) {
        match transfer(runner, workspace, &payload) {
            Ok(()) => return Ok(Provisioning::Lent),
            Err(TransferFailed::Bundle(failure)) => {
                events.say(ProvisionEvent::PayloadNotBundled { failure });
            }
            Err(TransferFailed::Trip(refusal)) => return refused(workspace, refusal, events),
            // The container would not run the lent binaries (a different arch or
            // libc). The old path still runs.
            Err(TransferFailed::Rejected { .. }) => {}
        }
    }

    if let ProbeResult::Lendable = found {
        // Accepted residual: a container that keeps rejecting the lend while
        // carrying a shim re-attempts one failing transfer on every `devpod up`,
        // paying for the tar and the stream before the arch/libc gate refuses the
        // binaries. Breaking the loop needs per-container retry state persisted
        // somewhere, which is more machinery than the case is worth — and this
        // never runs on the fast-attach path: it only ever rides an `up` that
        // already took seconds to minutes.
        return Ok(Provisioning::ShimKept);
    }

    // ProbeResult::Absent: the cold flow, with a tool genuinely missing.
    let script = provision_script(&REQUIRED_TOOLS);
    let call = Call::new([
        "ssh",
        workspace,
        "--command",
        &format!("bash -lc {}", quote(&script)),
    ]);
    match devpod::run(runner, &call) {
        Ok(exit) if exit.is_success() => Ok(Provisioning::Installed),
        Ok(exit) => {
            events.say(ProvisionEvent::NotInstalled {
                workspace: workspace.to_owned(),
                tools: REQUIRED_TOOLS.iter().map(|tool| tool.command).collect(),
                exit,
            });
            Ok(Provisioning::InstallRefused { exit })
        }
        Err(refusal) => refused(workspace, refusal, events),
    }
}

/// What a trip that never happened means, in one place.
///
/// A devpod that is not installed keeps travelling; everything else is the
/// convenience failing, which is reported and costs the session nothing.
fn refused(
    workspace: &str,
    refusal: NotRun,
    events: &mut dyn Notices<ProvisionEvent>,
) -> Result<Provisioning, DevpodMissing> {
    match refusal {
        NotRun::NotInstalled => Err(DevpodMissing),
        // Timeouts included, though no trip here carries a deadline: a trip this
        // process killed is one that never answered, which is the same fact the
        // OS refusals carry.
        refusal @ (NotRun::TimedOut | NotRun::Blocked(_)) => {
            events.say(ProvisionEvent::TripRefused {
                workspace: workspace.to_owned(),
                refusal,
            });
            Ok(Provisioning::TripRefused { refusal })
        }
    }
}

/// One round trip: set the workspace up, and report what it still needs.
///
/// The cold path's whole setup pass, and the only trip a provisioned workspace pays.
/// The stages happen because this trip was being paid anyway — a separate `devpod
/// ssh` for the hostname measured ~1.73s, of which ~99% was connection and process
/// setup, so folding it in saves a whole trip (#157).
///
/// Captured, unlike the trips that may follow — here the output *is* the answer the
/// caller branches on, rather than progress a user needs to watch. Which is also why
/// each stage's outcome is reported here rather than returned: the caller branches on
/// what the container still needs, and a stage's outcome is not that.
///
/// A trip that failed is not an answer: the script exits 0 in every state, so a
/// non-zero status means the ssh itself did not get through, and the reading that
/// costs a redundant trip is preferred to the one that skips the work. Whatever the
/// trip did print is still read for stage outcomes, because a report cut off partway
/// is exactly what "not reached" is for.
fn setup_pass(
    runner: &dyn Runner,
    workspace: &str,
    switches: Switches,
    title: Option<&str>,
    events: &mut dyn Notices<ProvisionEvent>,
) -> Result<ProbeResult, NotRun> {
    let stages = setup_stages(workspace, switches.tools, switches.zellij, title);
    let call = Call::new([
        "ssh",
        workspace,
        "--command",
        &format!("bash -lc {}", quote(&setup_script(&stages))),
    ]);
    let answer = devpod::capture(runner, &call)?;
    let report = answer.stdout();
    for outcome in stage_outcomes(report, &stages) {
        report_outcome(workspace, &stages, outcome, events);
    }
    if !answer.succeeded() {
        return Ok(ProbeResult::Absent);
    }
    Ok(ProbeResult::parse(report))
}

/// Say what became of one stage, unless what became of it was nothing.
///
/// `ok` is silent — a launch that worked has nothing to say — and every other
/// outcome is named, at the level the stage itself declares. The match is exhaustive
/// over [`StageResult`] rather than an `else`, because an `else` hand-rolled where
/// the arms are read is how a fourth outcome would come to be reported as though it
/// were ok.
fn report_outcome(
    workspace: &str,
    stages: &[Stage],
    outcome: StageOutcome,
    events: &mut dyn Notices<ProvisionEvent>,
) {
    let loudness = stages
        .iter()
        .find(|stage| stage.name == outcome.stage)
        .map(|stage| stage.failure_level)
        .unwrap_or_default();
    match outcome.result {
        StageResult::Ok => {}
        StageResult::Failed { status } => events.say(ProvisionEvent::StageFailed {
            workspace: workspace.to_owned(),
            stage: outcome.stage.as_str(),
            status,
            loudness,
        }),
        StageResult::NotReached => events.say(ProvisionEvent::StageNotReported {
            workspace: workspace.to_owned(),
            stage: outcome.stage.as_str(),
            loudness,
        }),
    }
}

/// Why the lend did not land.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TransferFailed {
    /// The archive was never written, so no trip was made.
    Bundle(BundleFailed),
    /// The trip could not be made at all.
    Trip(NotRun),
    /// The container ran the script and refused — the arch/libc gate, usually.
    Rejected { exit: Exit },
}

/// Stream the host's binaries into the workspace. One round trip.
fn transfer(
    runner: &dyn Runner,
    workspace: &str,
    payload: &HostPayload,
) -> Result<(), TransferFailed> {
    let staging = tempfile::Builder::new()
        .prefix("devlaunch-tools-")
        .tempdir()
        .map_err(|error| {
            TransferFailed::Bundle(BundleFailed::NotWritten {
                path: crate::osext::temp_dir(),
                error: OsFailure::from(&error),
            })
        })?;
    let bundle = staging.path().join("tools.tar");
    // Registered for interrupt-time cleanup: `_exit(130)` runs no `Drop`, so a
    // Ctrl-C during this `devpod ssh` trip would otherwise leave the bundle and
    // its staging directory behind. The handler unlinks files before it rmdirs
    // directories, so removing `tools.tar` first leaves `staging` empty for its
    // own `rmdir` — no residue. Both guards live until this function returns.
    let _bundle_cleanup = interrupt::register_file(&bundle);
    let _staging_cleanup = interrupt::register_dir(staging.path());
    write_payload_tar(payload, &bundle).map_err(TransferFailed::Bundle)?;
    // A real file rather than a pipe, so the stream stays on the one devpod spawn
    // point — and a failed trip can be retried by the fallback without a
    // half-consumed generator in hand.
    let call = Call::new([
        "ssh",
        workspace,
        "--command",
        &format!("bash -c {}", quote(&transfer_script(payload))),
    ])
    .with_stdin_file(&bundle);
    match devpod::run(runner, &call) {
        Ok(exit) if exit.is_success() => Ok(()),
        Ok(exit) => Err(TransferFailed::Rejected { exit }),
        Err(refusal) => Err(TransferFailed::Trip(refusal)),
    }
}

// The tests live in this file rather than beside it: one file is the unit of
// ownership for this port, and the goldens below are the whole reason — the
// scripts and the strings they are pinned against belong within one screenful of
// each other.
/// The README and CHANGELOG claims this module's constants and generators keep
/// true (#267). Its own file because it is about the documents rather than about
/// the code, and because `mod tests` below is already long.
#[cfg(test)]
mod lending_contract;

#[cfg(test)]
mod tests {
    //! # What this pins, and how
    //!
    //! Three kinds of test, because the module is three things:
    //!
    //! - **Goldens.** Every script and fragment is compared against the exact
    //!   string `devlaunch/tools.py` renders, generated by importing the module
    //!   (`pixi run python -c 'from devlaunch import tools; print(tools.probe_script())'`).
    //!   A shell fix has to move both sides, which is the point: these scripts run
    //!   in someone else's container and the bytes are the shipped behaviour.
    //! - **Real runs.** The probe, the transfer, the setup pass and the zellij
    //!   stage are executed by a real bash against scratch `$HOME`s on a stripped
    //!   PATH — the only way to find out what a script *does*, and the only way the
    //!   convergence claim (a lend the next probe recognises) can be tested at all.
    //! - **The flow, over a recording runner.** The same fake shape
    //!   `test/unit/test_tools.py` uses: one exit status per trip, the last
    //!   repeating, plus what each trip put on stdin.
    //!
    //! `test/unit/test_tools.py` is re-pinned here in full, except
    //! `TestWorkspaceUpInstallsTools`, which is about `dl.py`'s wiring (that `up`
    //! and `dl <ws> up` call this at all) and belongs to the launch flow.

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};
    use std::sync::Mutex;

    use super::*;
    use crate::runner::{CapturedText, DetachOutcome, Invocation, Outcome, SpawnSpec, StdinPlan};

    // =======================================================================
    // the goldens: the provisioning scripts, byte for byte
    // =======================================================================
    //
    // The `PYTHON_` prefix is *provenance*, not a live comparison: these strings
    // were transcribed from what `devlaunch/tools.py` rendered, and that module was
    // retired with the rest of the Python implementation (#267). They are kept, and
    // kept under their names, because what they pin did not change when their
    // author left -- the exact bytes a container is asked to run -- and renaming
    // them would cost the one thing they still say about themselves, which is that
    // nobody read them off this implementation.
    //
    // So: edit one of these only to record a script this module *should* now
    // render, never to make a failing assertion pass. There is nothing left to
    // re-derive them from.

    const PYTHON_PROVISION_SCRIPT: &str = r#"set -u
exec >&2
failed=0
if command -v gh >/dev/null 2>&1 && command -v claude >/dev/null 2>&1; then exit 0; fi
export PIXI_HOME="$HOME/.devlaunch/pixi"
if ! command -v pixi >/dev/null 2>&1; then
  echo "devlaunch: installing pixi"
  curl -fsSL https://pixi.sh/install.sh | bash >/dev/null 2>&1 || true
  export PATH="$HOME/.devlaunch/pixi/bin:$PATH"
fi
if ! command -v gh >/dev/null 2>&1; then
  echo "devlaunch: installing gh"
  pixi global install gh || failed=1
fi
if ! command -v claude >/dev/null 2>&1; then
  echo "devlaunch: installing claude"
  pixi global install --channel https://prefix.dev/blooop claude-shim || failed=1
fi
if [ -f "$HOME/.bash_profile" ]; then PROFILE="$HOME/.bash_profile"
elif [ -f "$HOME/.bash_login" ]; then PROFILE="$HOME/.bash_login"
else PROFILE="$HOME/.profile"
fi
grep -qxF '# devlaunch: 87ccd356540b' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 87ccd356540b' 'export PATH="$HOME/.devlaunch/pixi/bin:$PATH"' >> "$PROFILE" || failed=1
grep -qxF '# devlaunch: 190e825e206b' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 190e825e206b' '[ -d "$HOME/.devlaunch/pixi/envs/claude-shim/bin" ] && export PATH="$HOME/.devlaunch/pixi/envs/claude-shim/bin:$PATH"' >> "$PROFILE" || failed=1
exit "$failed""#;

    const PYTHON_PROVISION_SCRIPT_JQ: &str = r#"set -u
exec >&2
failed=0
if command -v jq >/dev/null 2>&1; then exit 0; fi
export PIXI_HOME="$HOME/.devlaunch/pixi"
if ! command -v pixi >/dev/null 2>&1; then
  echo "devlaunch: installing pixi"
  curl -fsSL https://pixi.sh/install.sh | bash >/dev/null 2>&1 || true
  export PATH="$HOME/.devlaunch/pixi/bin:$PATH"
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "devlaunch: installing jq"
  pixi global install jq || failed=1
fi
if [ -f "$HOME/.bash_profile" ]; then PROFILE="$HOME/.bash_profile"
elif [ -f "$HOME/.bash_login" ]; then PROFILE="$HOME/.bash_login"
else PROFILE="$HOME/.profile"
fi
grep -qxF '# devlaunch: 87ccd356540b' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 87ccd356540b' 'export PATH="$HOME/.devlaunch/pixi/bin:$PATH"' >> "$PROFILE" || failed=1
grep -qxF '# devlaunch: 190e825e206b' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 190e825e206b' '[ -d "$HOME/.devlaunch/pixi/envs/claude-shim/bin" ] && export PATH="$HOME/.devlaunch/pixi/envs/claude-shim/bin:$PATH"' >> "$PROFILE" || failed=1
exit "$failed""#;

    const PYTHON_ZELLIJ_SCRIPT: &str = r#"set -u
failed=0
if command -v zellij >/dev/null 2>&1; then exit 0; fi
export PIXI_HOME="$HOME/.devlaunch/pixi"
if ! command -v pixi >/dev/null 2>&1; then
  echo "devlaunch: installing pixi"
  curl -fsSL https://pixi.sh/install.sh | bash >/dev/null 2>&1 || true
  export PATH="$HOME/.devlaunch/pixi/bin:$PATH"
fi
if ! command -v zellij >/dev/null 2>&1; then
  echo "devlaunch: installing zellij"
  pixi global install zellij || failed=1
fi
if [ -f "$HOME/.bash_profile" ]; then PROFILE="$HOME/.bash_profile"
elif [ -f "$HOME/.bash_login" ]; then PROFILE="$HOME/.bash_login"
else PROFILE="$HOME/.profile"
fi
grep -qxF '# devlaunch: 87ccd356540b' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 87ccd356540b' 'export PATH="$HOME/.devlaunch/pixi/bin:$PATH"' >> "$PROFILE" || failed=1
exit "$failed""#;

    const PYTHON_PROBE_SCRIPT: &str = r#"set -u
if ! { command -v gh >/dev/null 2>&1 && command -v claude >/dev/null 2>&1 ; }; then
  echo "devlaunch-probe tools missing"
  exit 0
fi
echo "devlaunch-probe tools present"
echo "devlaunch-probe versions $(readlink -f "${HOME-}/.local/share/claude/versions" 2>/dev/null || true)"
echo "devlaunch-probe claude $(readlink -f "$(command -v claude)" 2>/dev/null || true)""#;

    const PYTHON_TRANSFER_SCRIPT: &str = r#"set -eu
exec >&2
echo "devlaunch: lending claude 2.0.1 and gh from the host"
STAGE="$HOME/.devlaunch-lend"
trap 'rm -rf "$STAGE"' EXIT
rm -rf "$STAGE"
mkdir -p "$STAGE"
tar xf - -C "$STAGE"
"$STAGE/.local/share/claude/versions/2.0.1" --version >/dev/null
"$STAGE/.local/bin/gh" --version >/dev/null
mkdir -p "$HOME/.local/bin" "$HOME/.local/share/claude/versions"
mv -f "$STAGE/.local/share/claude/versions/2.0.1" "$HOME/.local/share/claude/versions/2.0.1"
mv -f "$STAGE/.local/bin/gh" "$HOME/.local/bin/gh"
ln -sfn "$HOME/.local/share/claude/versions/2.0.1" "$HOME/.local/bin/claude"
if [ -f "$HOME/.bash_profile" ]; then PROFILE="$HOME/.bash_profile"
elif [ -f "$HOME/.bash_login" ]; then PROFILE="$HOME/.bash_login"
else PROFILE="$HOME/.profile"
fi
grep -qxF '# devlaunch: 63c662ba0560' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 63c662ba0560' 'export PATH="$HOME/.local/bin:$PATH"' >> "$PROFILE""#;

    /// The hostname stage's snippet, and the zellij stage's — the second carrying a
    /// whole quoted script, which is where Python's `shlex.quote` and the `shlex`
    /// crate's disagree about bytes and this module's [`quote`] does not.
    const PYTHON_HOSTNAME_STAGE: &str = r#"if sudo hostname myws; then
  echo "devlaunch-probe stage hostname ok"
else
  echo "devlaunch-probe stage hostname failed $?"
fi"#;

    const PYTHON_ZELLIJ_STAGE: &str = r#"if bash -c 'set -u
failed=0
if command -v zellij >/dev/null 2>&1; then exit 0; fi
export PIXI_HOME="$HOME/.devlaunch/pixi"
if ! command -v pixi >/dev/null 2>&1; then
  echo "devlaunch: installing pixi"
  curl -fsSL https://pixi.sh/install.sh | bash >/dev/null 2>&1 || true
  export PATH="$HOME/.devlaunch/pixi/bin:$PATH"
fi
if ! command -v zellij >/dev/null 2>&1; then
  echo "devlaunch: installing zellij"
  pixi global install zellij || failed=1
fi
if [ -f "$HOME/.bash_profile" ]; then PROFILE="$HOME/.bash_profile"
elif [ -f "$HOME/.bash_login" ]; then PROFILE="$HOME/.bash_login"
else PROFILE="$HOME/.profile"
fi
grep -qxF '"'"'# devlaunch: 87ccd356540b'"'"' "$PROFILE" 2>/dev/null || printf '"'"'%s\n'"'"' '"'"'# devlaunch: 87ccd356540b'"'"' '"'"'export PATH="$HOME/.devlaunch/pixi/bin:$PATH"'"'"' >> "$PROFILE" || failed=1
exit "$failed"' >&2; then
  echo "devlaunch-probe stage zellij ok"
else
  echo "devlaunch-probe stage zellij failed $?"
fi"#;

    const PYTHON_PROFILE_RESOLUTION_HOME: &str = r#"if [ -f "$HOME/.bash_profile" ]; then PROFILE="$HOME/.bash_profile"
elif [ -f "$HOME/.bash_login" ]; then PROFILE="$HOME/.bash_login"
else PROFILE="$HOME/.profile"
fi"#;

    const PYTHON_PROFILE_RESOLUTION_TARGET: &str = r#"if [ -f "$TARGET_HOME/.bash_profile" ]; then PROFILE="$TARGET_HOME/.bash_profile"
elif [ -f "$TARGET_HOME/.bash_login" ]; then PROFILE="$TARGET_HOME/.bash_login"
else PROFILE="$TARGET_HOME/.profile"
fi"#;

    const PYTHON_PREPEND_PIXI_BIN: &str = r#"grep -qxF '# devlaunch: 87ccd356540b' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 87ccd356540b' 'export PATH="$HOME/.devlaunch/pixi/bin:$PATH"' >> "$PROFILE""#;

    const PYTHON_PREPEND_SHIM_BIN: &str = r#"grep -qxF '# devlaunch: 190e825e206b' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 190e825e206b' '[ -d "$HOME/.devlaunch/pixi/envs/claude-shim/bin" ] && export PATH="$HOME/.devlaunch/pixi/envs/claude-shim/bin:$PATH"' >> "$PROFILE""#;

    const PYTHON_PREPEND_LOCAL_BIN: &str = r#"grep -qxF '# devlaunch: 63c662ba0560' "$PROFILE" 2>/dev/null || printf '%s\n' '# devlaunch: 63c662ba0560' 'export PATH="$HOME/.local/bin:$PATH"' >> "$PROFILE""#;

    const PYTHON_PIXI_BOOTSTRAP: &str = r#"if ! command -v pixi >/dev/null 2>&1; then
  echo "devlaunch: installing pixi"
  curl -fsSL https://pixi.sh/install.sh | bash >/dev/null 2>&1 || true
  export PATH="$HOME/.devlaunch/pixi/bin:$PATH"
fi"#;

    const PYTHON_INSTALL_CLAUDE: &str = r#"if ! command -v claude >/dev/null 2>&1; then
  echo "devlaunch: installing claude"
  pixi global install --channel https://prefix.dev/blooop claude-shim || failed=1
fi"#;

    // What a container in each of the three states reports back over the pipe,
    // written out as the probe prints it rather than rebuilt from the module: a
    // fixture composed the way the parser splits lines would agree with any format
    // the probe ever drifted into. `/ws` stands in for a container's $HOME.
    const REPORT_ABSENT: &str = "devlaunch-probe tools missing\n";
    const REPORT_PROVISIONED: &str = concat!(
        "devlaunch-probe tools present\n",
        "devlaunch-probe versions /ws/.local/share/claude/versions\n",
        "devlaunch-probe claude /ws/.local/share/claude/versions/2.0.1\n",
    );
    const REPORT_LENDABLE: &str = concat!(
        "devlaunch-probe tools present\n",
        "devlaunch-probe versions /ws/.local/share/claude/versions\n",
        "devlaunch-probe claude /ws/.pixi/envs/claude-shim/bin/claude\n",
    );

    // The two outcome lines a stage of the setup pass can print, written out as the
    // composer prints them for the same reason the reports above are. The third
    // outcome has no line — that is what makes it the third outcome.
    const STAGE_OK_LINE: &str = "devlaunch-probe stage hostname ok\n";
    const STAGE_FAILED_LINE: &str = "devlaunch-probe stage hostname failed 1\n";

    // ~/.profile exactly as mcr.microsoft.com/devcontainers/base:ubuntu-24.04 ships
    // it — the base image .devcontainer/Dockerfile builds on. The load-bearing part
    // is the last block: Ubuntu's default profile puts ~/.local/bin on PATH itself,
    // long before anything devlaunch or the devcontainer feature appends. A guard
    // that looks for the *directory* rather than for its own line reads that block
    // as its own work and skips an append the workspace needs.
    const UBUNTU_STOCK_PROFILE: &str = r#"# ~/.profile: executed by the command interpreter for login shells.
# This file is not read by bash(1), if ~/.bash_profile or ~/.bash_login
# exists.
# see /usr/share/doc/bash/examples/startup-files for examples.
# the files are located in the bash-doc package.

# the default umask is set in /etc/profile; for setting the umask
# for ssh logins, install and configure the libpam-umask package.
#umask 022

# if running bash
if [ -n "$BASH_VERSION" ]; then
    # include .bashrc if it exists
    if [ -f "$HOME/.bashrc" ]; then
	. "$HOME/.bashrc"
    fi
fi

# set PATH so it includes user's private bin if it exists
if [ -d "$HOME/bin" ] ; then
    PATH="$HOME/bin:$PATH"
fi

# set PATH so it includes user's private bin if it exists
if [ -d "$HOME/.local/bin" ] ; then
    PATH="$HOME/.local/bin:$PATH"
fi
"#;

    /// What .devcontainer/Dockerfile appends to that same file, verbatim, and in
    /// this order. The shim directory is prepended *last*, so it wins over
    /// everything above it — including Ubuntu's own ~/.local/bin block.
    const DEVCONTAINER_PROFILE_LINES: &str = r#"export PATH="$HOME/.pixi/bin:$PATH"
# Workaround: pixi trampoline fails for bash scripts, so add env bin directly
[ -d "$HOME/.pixi/envs/claude-shim/bin" ] && export PATH="$HOME/.pixi/envs/claude-shim/bin:$PATH"
"#;

    // =======================================================================
    // the fake runner: test_tools.py's Runner, as a Runner impl
    // =======================================================================

    /// What one scripted trip answers.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Answer {
        Exited(i32),
        /// devpod is not on PATH — Python's `DevpodNotInstalled`.
        NoDevpod,
        /// The OS refused the spawn — Python's `OSError`.
        Blocked,
    }

    /// One recorded trip: what was asked of devpod, whether it was captured, and
    /// what it put on the command's stdin.
    #[derive(Clone, Debug)]
    struct Trip {
        argv: Vec<String>,
        captured: bool,
        /// The bytes the trip streamed, read as devpod would. `None` for the trips
        /// that stream nothing.
        stream: Option<Vec<u8>>,
    }

    impl Trip {
        /// The payload of this trip's `ssh --command`.
        fn script(&self) -> &str {
            let at = self
                .argv
                .iter()
                .position(|arg| arg == "--command")
                .expect("a trip with a --command");
            &self.argv[at + 1]
        }
    }

    /// Stands in for the one devpod spawn point, recording what was asked of it.
    ///
    /// `answers` is consumed one per trip, the last repeating — the three-trip flow
    /// (probe, transfer, install) needs different answers to different trips, and a
    /// single number could only play one of them.
    ///
    /// It holds [`timing::exclusive`] for its own lifetime, as
    /// `clients::devpod`'s `ScriptedRunner` does and for the same reason:
    /// [`provision_tools`] opens the `tools` stage on the **process-global** timing
    /// registry, so a test driving it without the guard writes into whatever document
    /// a concurrent measured test installed. In the fixture rather than per test, so
    /// no test has to remember.
    #[derive(Debug)]
    struct Trips {
        answers: Vec<Answer>,
        stdout: String,
        seen: Mutex<Vec<Trip>>,
        /// See [`timing::exclusive`]. Last field, so it is dropped last.
        _serialized: timing::Exclusive,
    }

    impl Trips {
        fn new(exits: &[i32]) -> Self {
            Self {
                answers: exits.iter().copied().map(Answer::Exited).collect(),
                stdout: String::new(),
                seen: Mutex::new(Vec::new()),
                _serialized: timing::exclusive(),
            }
        }

        fn answering(answers: &[Answer]) -> Self {
            Self {
                answers: answers.to_vec(),
                stdout: String::new(),
                seen: Mutex::new(Vec::new()),
                _serialized: timing::exclusive(),
            }
        }

        #[must_use]
        fn reporting(mut self, stdout: &str) -> Self {
            self.stdout = stdout.to_owned();
            self
        }

        fn trips(&self) -> Vec<Trip> {
            self.seen.lock().expect("the recording").clone()
        }

        fn count(&self) -> usize {
            self.trips().len()
        }

        /// The payload of the `call`-th `ssh --command` that was sent.
        fn script(&self, call: usize) -> String {
            self.trips()[call].script().to_owned()
        }

        /// Whether each trip captured, in order.
        fn captured(&self) -> Vec<bool> {
            self.trips().iter().map(|trip| trip.captured).collect()
        }

        /// Whether each trip streamed something, in order.
        fn streamed(&self) -> Vec<bool> {
            self.trips()
                .iter()
                .map(|trip| trip.stream.is_some())
                .collect()
        }

        fn record(&self, spec: &SpawnSpec, captured: bool) -> Answer {
            let mut seen = self.seen.lock().expect("the recording");
            seen.push(Trip {
                argv: spec.invocation.argv(),
                captured,
                stream: match &spec.stdin {
                    StdinPlan::File(path) => {
                        Some(fs::read(path).expect("the bundle the trip named"))
                    }
                    StdinPlan::Inherit | StdinPlan::Null => None,
                },
            });
            self.answers[(seen.len() - 1).min(self.answers.len() - 1)]
        }

        fn outcome(&self, answer: Answer) -> Outcome<CapturedText> {
            match answer {
                Answer::Exited(code) => Outcome::Ran {
                    exit: Exit::Code(code),
                    io: CapturedText {
                        stdout: self.stdout.clone(),
                        stderr: String::new(),
                    },
                },
                Answer::NoDevpod => Outcome::ProgramNotFound,
                Answer::Blocked => Outcome::NotStarted(OsFailure {
                    kind: std::io::ErrorKind::PermissionDenied,
                    errno: Some(13),
                }),
            }
        }
    }

    impl Runner for Trips {
        fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
            let answer = self.record(spec, true);
            self.outcome(answer)
        }

        fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
            let answer = self.record(spec, false);
            match self.outcome(answer) {
                Outcome::Ran { exit, .. } => Outcome::Ran { exit, io: () },
                Outcome::ProgramNotFound => Outcome::ProgramNotFound,
                Outcome::TimedOut => Outcome::TimedOut,
                Outcome::NotStarted(failure) => Outcome::NotStarted(failure),
            }
        }

        fn session(&self, _spec: &SpawnSpec, _on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
            panic!("provisioning never opens a session")
        }

        fn detach(&self, _what: &Invocation) -> DetachOutcome {
            panic!("provisioning never detaches")
        }
    }

    /// A host with nothing to lend, which is what a test of the flow needs: the
    /// resolvers ask the real filesystem, and on a developer machine that is a real
    /// claude and gh where on CI it is nothing.
    fn nothing_to_lend() -> HostLayout {
        HostLayout {
            home: PathBuf::from("/nonexistent/devlaunch-has-nothing-here"),
            gh_on_path: None,
        }
    }

    /// A host carrying the official claude layout and a real gh, in `scratch`.
    fn a_host_that_can_lend(scratch: &Path) -> HostLayout {
        let home = scratch.join("host-home");
        official_claude(&home, "2.0.1");
        let gh = home.join("bin/gh");
        write_program(&gh, "#!/bin/sh\nexit 0\n");
        HostLayout {
            home,
            gh_on_path: Some(gh),
        }
    }

    /// A payload of scratch files, so no test ships a real 300MB binary.
    fn fake_payload(scratch: &Path) -> HostPayload {
        let claude = scratch.join("claude-2.0.1");
        fs::write(&claude, b"#!/bin/sh\n").expect("a scratch claude");
        let gh = scratch.join("gh");
        fs::write(&gh, b"\x7fELF").expect("a scratch gh");
        HostPayload {
            claude_version: "2.0.1".to_owned(),
            claude_binary: claude,
            gh_binary: gh,
        }
    }

    /// The payload the transfer-script goldens were rendered against. Only the
    /// version reaches the script, so the paths need not exist.
    fn golden_payload() -> HostPayload {
        HostPayload {
            claude_version: "2.0.1".to_owned(),
            claude_binary: PathBuf::from("/x/claude-2.0.1"),
            gh_binary: PathBuf::from("/x/gh"),
        }
    }

    /// The flow with no host-side memory behind it, on the occasion that never
    /// consults one — which is every flow test that is not about the verdict cache,
    /// and is exactly what this flow did before the cache existed.
    fn events_of(
        runner: &Trips,
        switches: Switches,
        host: &HostLayout,
    ) -> (Result<Provisioning, DevpodMissing>, Vec<ProvisionEvent>) {
        let mut events = Vec::new();
        let outcome = provision_tools(
            runner,
            "myws",
            PassOccasion::AfterUp,
            switches,
            None,
            Some(host),
            None,
            &mut events,
        );
        (outcome, events)
    }

    /// The flow, with the tools switch on and nothing to lend — the shape most of
    /// the flow tests want.
    fn provision_with(runner: &Trips) -> Provisioning {
        let (outcome, _) = events_of(runner, Switches::INSTALLING, &nothing_to_lend());
        outcome.expect("devpod answered")
    }

    // =======================================================================
    // running a real bash
    // =======================================================================

    fn bash() -> PathBuf {
        which("bash").unwrap_or_else(|| PathBuf::from("/bin/bash"))
    }

    /// Run `script` with exactly `env` and nothing else — the hermetic shape.
    fn bash_with(script: &str, env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(bash());
        command.arg("-c").arg(script).env_clear();
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().expect("bash ran")
    }

    /// A PATH carrying the coreutils a script needs and nothing else.
    ///
    /// A test of "no claude here" must not find the developer's own claude, so the
    /// real system directories are kept off PATH entirely and the externals a
    /// script uses are linked in by hand.
    fn sysbin(scratch: &Path, commands: &[&str]) -> PathBuf {
        let sysbin = scratch.join("sysbin");
        fs::create_dir_all(&sysbin).expect("a scratch sysbin");
        for command in commands {
            let found = which(command).unwrap_or_else(|| panic!("the test host needs {command}"));
            let link = sysbin.join(command);
            if !link.exists() {
                std::os::unix::fs::symlink(found, link).expect("a linked coreutil");
            }
        }
        sysbin
    }

    /// A stand-in binary that records being called and exits with `status`.
    fn fake_program(dir: &Path, name: &str, status: i32, log: &Path, noise: Option<&str>) {
        let mut body = format!("echo \"{name} $*\" >> {}\n", quote(&log.to_string_lossy()));
        if let Some(noise) = noise {
            body.push_str(&format!("echo {}\n", quote(noise)));
        }
        write_program(
            &dir.join(name),
            &format!("#!/bin/sh\n{body}exit {status}\n"),
        );
    }

    fn write_program(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().expect("a parent")).expect("a directory for it");
        fs::write(path, body).expect("a program");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("it is executable");
    }

    fn scratch() -> tempfile::TempDir {
        tempfile::tempdir().expect("a scratch directory")
    }

    fn a_home(scratch: &Path) -> PathBuf {
        let home = scratch.join("home");
        fs::create_dir_all(&home).expect("a scratch home");
        home
    }

    /// What the official installer leaves behind: one binary per version under
    /// ~/.local/share/claude/versions, with ~/.local/bin/claude pointing at the
    /// current one.
    fn official_claude(home: &Path, version: &str) -> PathBuf {
        let binary = home.join(CLAUDE_VERSIONS_RELPATH).join(version);
        write_program(&binary, "#!/bin/sh\nexit 0\n");
        let link = home.join(".local/bin/claude");
        fs::create_dir_all(link.parent().expect("a parent")).expect("a bin directory");
        std::os::unix::fs::symlink(&binary, &link).expect("the installer's symlink");
        link.parent().expect("a parent").to_path_buf()
    }

    /// What .devcontainer/claude-code/install.sh bakes: something on PATH answering
    /// to `claude` that fetches the real binary on first run.
    fn claude_shim(home: &Path) -> PathBuf {
        let shim = home.join(".local/bin/claude");
        write_program(&shim, "#!/bin/sh\necho 'downloading 285MB' >&2\n");
        shim.parent().expect("a parent").to_path_buf()
    }

    /// A downloader that parked itself *inside* the versions directory. The official
    /// installer puts one binary per version directly in that directory; anything
    /// deeper is somebody else's tree.
    fn nested_shim(home: &Path) -> PathBuf {
        let shim = home.join(CLAUDE_VERSIONS_RELPATH).join("latest/bin/claude");
        write_program(&shim, "#!/bin/sh\necho 'downloading 285MB' >&2\n");
        let link = home.join(".local/bin/claude");
        fs::create_dir_all(link.parent().expect("a parent")).expect("a bin directory");
        std::os::unix::fs::symlink(&shim, &link).expect("a link to the downloader");
        link.parent().expect("a parent").to_path_buf()
    }

    fn a_gh(home: &Path) -> PathBuf {
        let gh = home.join(".local/bin/gh");
        write_program(&gh, "#!/bin/sh\nexit 0\n");
        gh.parent().expect("a parent").to_path_buf()
    }

    /// Run the probe for real and read its answer the way `dl` reads it.
    ///
    /// The script and the parser together, because that pair *is* the probe: the
    /// container reports what it found and this side says what that means, so a test
    /// of either half alone would not notice the two disagreeing.
    fn probe_answer(scratch: &Path, home: &Path, path_dirs: &[PathBuf]) -> ProbeResult {
        probe_answer_with_home(scratch, path_dirs, Some(&home.to_string_lossy()))
    }

    /// The same, with `$HOME` set to something else — or removed entirely.
    fn probe_answer_with_home(
        scratch: &Path,
        path_dirs: &[PathBuf],
        home_env: Option<&str>,
    ) -> ProbeResult {
        let mut path: Vec<String> = path_dirs
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect();
        path.push(
            sysbin(scratch, &["readlink"])
                .to_string_lossy()
                .into_owned(),
        );
        let path = path.join(":");
        let mut env = vec![("PATH", path.as_str())];
        if let Some(home) = home_env {
            env.push(("HOME", home));
        }
        let answered = bash_with(&probe_script(), &env);
        // Exit 0 in every state: "no tools here" is an answer, not a failure, and a
        // non-zero probe paints a red devpod `fatal` on the terminal of every cold
        // launch.
        assert!(
            answered.status.success(),
            "{}",
            String::from_utf8_lossy(&answered.stderr)
        );
        ProbeResult::parse(&String::from_utf8_lossy(&answered.stdout))
    }

    /// The same answer, but with PATH built by the home's own login profile.
    ///
    /// The pass runs the script under `bash -lc`, so in a real container every
    /// directory the probe searches was put there by the profile — the base image's
    /// block, the devcontainer's appended lines and the transfer's prepend, in
    /// whatever order they ended up in the file. Handing the probe a PATH instead (as
    /// the other cases here do, to stay hermetic) hides exactly that ordering, which
    /// is the thing a lend depends on.
    ///
    /// `$HOME/.profile` is sourced explicitly rather than using `bash -l`, because
    /// `-l` would also source the *test host's* /etc/profile and drag its system
    /// directories — and whatever `claude` the developer has — into a run meant to
    /// see only this scratch home.
    fn probe_answer_after_login(scratch: &Path, home: &Path) -> ProbeResult {
        let path = sysbin(scratch, &["readlink"]);
        let script = format!(". \"$HOME/.profile\"\n{}", probe_script());
        let answered = bash_with(
            &script,
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &path.to_string_lossy()),
            ],
        );
        assert!(
            answered.status.success(),
            "{}",
            String::from_utf8_lossy(&answered.stderr)
        );
        ProbeResult::parse(&String::from_utf8_lossy(&answered.stdout))
    }

    /// Run the real transfer script against a scratch `$HOME`, for real.
    ///
    /// A host payload is built out of scratch binaries, tarred by the very writer the
    /// flow uses, and streamed into [`transfer_script`] on stdin — so what lands in
    /// `home`, and what the script writes into that home's login profile, is what a
    /// real lend leaves behind.
    fn lend_into(scratch: &Path, home: &Path, version: &str) {
        let source = scratch.join(format!("host-{version}"));
        let claude = source.join(format!("claude-{version}"));
        let gh = source.join("gh");
        write_program(&claude, "#!/bin/sh\nexit 0\n");
        write_program(&gh, "#!/bin/sh\nexit 0\n");
        let payload = HostPayload {
            claude_version: version.to_owned(),
            claude_binary: claude,
            gh_binary: gh,
        };
        let bundle = scratch.join(format!("tools-{version}.tar"));
        write_payload_tar(&payload, &bundle).expect("a bundle");

        let stream = File::open(&bundle).expect("the bundle");
        let lend = Command::new(bash())
            .arg("-c")
            .arg(transfer_script(&payload))
            .env("HOME", home)
            .stdin(std::process::Stdio::from(stream))
            .output()
            .expect("the transfer ran");
        assert!(
            lend.status.success(),
            "{}",
            String::from_utf8_lossy(&lend.stderr)
        );
    }

    /// The devcontainer feature's installer, which edits the same login profile
    /// these scripts do — `$TARGET_HOME/.profile` is the *user's* profile, not a
    /// file the feature owns — so its guards are held to the same properties.
    fn feature_installer() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.devcontainer/claude-code/install.sh");
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    /// Every writer that edits a container's login profile.
    #[derive(Clone, Copy, Debug)]
    enum Writer {
        Provision,
        Transfer,
        Zellij,
        Feature,
    }

    impl Writer {
        const ALL: [Writer; 4] = [
            Writer::Provision,
            Writer::Transfer,
            Writer::Zellij,
            Writer::Feature,
        ];

        fn script(self) -> String {
            match self {
                Writer::Provision => provision_script(&REQUIRED_TOOLS),
                Writer::Transfer => transfer_script(&golden_payload()),
                Writer::Zellij => zellij_script(),
                Writer::Feature => feature_installer(),
            }
        }

        /// The home variable this writer resolves the profile in. The feature
        /// installer edits a home it is not running in; everything else runs in it.
        fn home_var(self) -> &'static str {
            match self {
                Writer::Feature => "$TARGET_HOME",
                _ => "$HOME",
            }
        }

        /// The directory this writer's profile edit unconditionally puts on a login
        /// shell's PATH. Written out as a literal rather than read back off the
        /// script, because a test that derives what to expect from the thing under
        /// test agrees with it however wrong it is.
        fn prepended(self) -> &'static str {
            // No catch-all: the two runtime writers and the build-time feature
            // installer no longer agree, and a `_` arm is what would let a third
            // writer inherit whichever answer happened to be the default.
            match self {
                Writer::Provision | Writer::Zellij => ".devlaunch/pixi/bin",
                Writer::Transfer => ".local/bin",
                // The feature is baked into an image at build time, where ~/.pixi
                // is the image's own and there is no checkout to dirty.
                Writer::Feature => ".pixi/bin",
            }
        }
    }

    /// The lines of `script` that decide which file `$PROFILE` names, cut out by
    /// their own shape and with the indentation of their surroundings taken off — so
    /// a block sitting inside a shell function compares against the same block
    /// rendered on its own.
    fn resolution_block(script: &str) -> String {
        let lines: Vec<&str> = script.lines().collect();
        let start = lines
            .iter()
            .position(|line| line.contains(".bash_profile") && line.contains("PROFILE="))
            .expect("a writer that resolves the login profile");
        let end = (start..lines.len())
            .find(|at| lines[*at].trim() == "fi")
            .expect("the fi that closes it");
        lines[start..=end]
            .iter()
            .map(|line| line.trim())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every line of `script` that appends to the login profile.
    fn appends(script: &str) -> Vec<String> {
        let found: Vec<String> = script
            .lines()
            .filter(|line| line.contains(r#">> "$PROFILE""#))
            .map(|line| line.trim().to_owned())
            .collect();
        assert!(
            !found.is_empty(),
            "a script that never edits the profile has nothing to resolve"
        );
        found
    }

    /// The guard half of every line that appends to the login profile.
    fn guards(script: &str) -> Vec<String> {
        appends(script)
            .into_iter()
            .map(|line| line.split("||").next().unwrap_or_default().to_owned())
            .collect()
    }

    /// The part of `script` that picks a profile file and appends to it, lifted
    /// verbatim so it can be *run*: that is the only way to find out where the
    /// writes actually land.
    fn profile_edit(script: &str) -> String {
        let mut block = vec![resolution_block(script)];
        block.extend(appends(script));
        block.join("\n")
    }

    // =======================================================================
    // quoting
    // =======================================================================

    #[test]
    fn every_word_is_quoted_the_way_pythons_shlex_quotes_it() {
        // The table is CPython's `shlex.quote` behaviour, including the two places
        // the `shlex` crate differs: a word Python leaves bare, and a word with a
        // single quote in it.
        for (word, quoted) in [
            ("plain", "plain"),
            ("", "''"),
            ("myws", "myws"),
            ("--channel", "--channel"),
            ("https://prefix.dev/blooop", "https://prefix.dev/blooop"),
            ("a@b%c+d:e,f./g-h", "a@b%c+d:e,f./g-h"),
            ("myws; touch /tmp/pwned", "'myws; touch /tmp/pwned'"),
            ("# devlaunch: abc", "'# devlaunch: abc'"),
            ("a'b", r#"'a'"'"'b'"#),
            ("a'b$c", r#"'a'"'"'b$c'"#),
            ("ä", "'ä'"),
            ("a\nb", "'a\nb'"),
        ] {
            assert_eq!(quote(word), quoted, "{word:?}");
        }
    }

    #[test]
    fn every_composed_payload_splits_back_into_the_words_it_was_built_from() {
        // The round trip Python's own tests assert with `shlex.split(runner.script())`:
        // whatever the quoting, the remote shell has to recover exactly `bash`, the
        // flag, and one script.
        let stages = setup_stages("myws", ToolsSwitch::Install, ZellijSwitch::Install, None);
        for (payload, flag, script) in [
            (
                format!("bash -lc {}", quote(&setup_script(&stages))),
                "-lc",
                setup_script(&stages),
            ),
            (
                format!("bash -lc {}", quote(&provision_script(&REQUIRED_TOOLS))),
                "-lc",
                provision_script(&REQUIRED_TOOLS),
            ),
            (
                format!("bash -c {}", quote(&transfer_script(&golden_payload()))),
                "-c",
                transfer_script(&golden_payload()),
            ),
        ] {
            let words = shlex::split(&payload).expect("a payload a shell can read");
            assert_eq!(words, vec!["bash".to_owned(), flag.to_owned(), script]);
        }
    }

    #[test]
    fn the_zellij_stage_is_one_word_the_pass_shell_hands_to_a_nested_bash() {
        // The stage is interpolated into `if <command>; then`, so the quoting has to
        // survive being read by the *pass's* shell before the nested bash sees it.
        let stages = setup_stages("myws", ToolsSwitch::Install, ZellijSwitch::Install, None);
        let command = &stages[1].command;
        let words = shlex::split(command).expect("a stage a shell can read");
        assert_eq!(
            words,
            vec![
                "bash".to_owned(),
                "-c".to_owned(),
                zellij_script(),
                ">&2".to_owned(),
            ]
        );
    }

    // =======================================================================
    // the goldens
    // =======================================================================

    #[test]
    fn the_provision_script_is_the_one_python_ships() {
        assert_eq!(provision_script(&REQUIRED_TOOLS), PYTHON_PROVISION_SCRIPT);
    }

    #[test]
    fn the_zellij_script_is_the_one_python_ships() {
        assert_eq!(zellij_script(), PYTHON_ZELLIJ_SCRIPT);
    }

    #[test]
    fn the_probe_script_is_the_one_python_ships() {
        assert_eq!(probe_script(), PYTHON_PROBE_SCRIPT);
    }

    #[test]
    fn the_transfer_script_is_the_one_python_ships() {
        assert_eq!(transfer_script(&golden_payload()), PYTHON_TRANSFER_SCRIPT);
    }

    #[test]
    fn the_setup_pass_is_the_script_python_composes() {
        // Stages, then the probe — and the zellij stage carries a whole quoted
        // script, which is where a quoting layer that merely *worked* would change
        // these bytes.
        let with_zellij = setup_script(&setup_stages(
            "myws",
            ToolsSwitch::Install,
            ZellijSwitch::Install,
            None,
        ));
        assert_eq!(
            with_zellij,
            format!("{PYTHON_HOSTNAME_STAGE}\n{PYTHON_ZELLIJ_STAGE}\n{PYTHON_PROBE_SCRIPT}")
        );
        let opted_out = setup_script(&setup_stages(
            "myws",
            ToolsSwitch::Skip,
            ZellijSwitch::Install,
            None,
        ));
        assert_eq!(
            opted_out,
            format!("{PYTHON_HOSTNAME_STAGE}\n{PYTHON_PROBE_SCRIPT}")
        );
    }

    #[test]
    fn every_shared_fragment_is_the_one_python_renders() {
        assert_eq!(profile_resolution("$HOME"), PYTHON_PROFILE_RESOLUTION_HOME);
        assert_eq!(
            profile_resolution("$TARGET_HOME"),
            PYTHON_PROFILE_RESOLUTION_TARGET
        );
        assert_eq!(
            profile_prepend(PIXI_BIN_LINE, None),
            PYTHON_PREPEND_PIXI_BIN
        );
        assert_eq!(
            profile_prepend(PIXI_BIN_LINE, Some("failed=1")),
            format!("{PYTHON_PREPEND_PIXI_BIN} || failed=1")
        );
        assert_eq!(
            profile_prepend(CLAUDE_SHIM_BIN_LINE, None),
            PYTHON_PREPEND_SHIM_BIN
        );
        assert_eq!(
            profile_prepend(LOCAL_BIN_LINE, None),
            PYTHON_PREPEND_LOCAL_BIN
        );
        assert_eq!(PIXI_BOOTSTRAP, PYTHON_PIXI_BOOTSTRAP);
        assert_eq!(
            all_present(&REQUIRED_TOOLS),
            "command -v gh >/dev/null 2>&1 && command -v claude >/dev/null 2>&1"
        );
        assert_eq!(install_line(&REQUIRED_TOOLS[1]), PYTHON_INSTALL_CLAUDE);
    }

    #[test]
    fn a_marks_digest_is_twelve_hex_characters_of_the_lines_sha256() {
        // The marks are content hashes a shell script cannot derive for itself, so
        // the devcontainer feature pastes them; a different derivation here would
        // silently cost every workspace one duplicate PATH entry per line.
        assert_eq!(mark_digest(PIXI_BIN_LINE), "87ccd356540b");
        assert_eq!(mark_digest(CLAUDE_SHIM_BIN_LINE), "190e825e206b");
        assert_eq!(mark_digest(LOCAL_BIN_LINE), "63c662ba0560");
        // The feature installer's own two, which are the marks actually pasted
        // into `.devcontainer/claude-code/install.sh`. Different lines from the
        // runtime writers' since those moved to `~/.devlaunch/pixi`, and so
        // different marks -- which is the dedupe working rather than failing: two
        // pixi homes are two directories a login shell genuinely needs.
        assert_eq!(mark_digest(FEATURE_PIXI_BIN_LINE), "6b593c3a6327");
        assert_eq!(mark_digest(FEATURE_CLAUDE_SHIM_BIN_LINE), "12897b113bea");
    }

    // =======================================================================
    // TestRequiredTools
    // =======================================================================

    #[test]
    fn gh_and_claude_are_both_required() {
        let commands: Vec<&str> = REQUIRED_TOOLS.iter().map(|tool| tool.command).collect();
        assert_eq!(commands, vec!["gh", "claude"]);
    }

    #[test]
    fn claude_comes_from_the_shim_package() {
        // `claude` is not a package name; installing `claude` would fail.
        let claude = REQUIRED_TOOLS
            .iter()
            .find(|tool| tool.command == "claude")
            .expect("claude is required");
        assert_eq!(claude.package, "claude-shim");
        assert_eq!(
            claude.install_args(),
            vec!["--channel", BLOOOP_CHANNEL, "claude-shim"]
        );
    }

    #[test]
    fn a_channelless_tool_installs_by_bare_name() {
        assert_eq!(Tool::new("gh", "gh").install_args(), vec!["gh"]);
    }

    // =======================================================================
    // TestProvisionScript
    // =======================================================================

    #[test]
    fn it_does_nothing_when_every_tool_is_present() {
        // The common path: the check has to come before any install.
        let script = provision_script(&REQUIRED_TOOLS);
        let exit_early = script.find("exit 0").expect("an early exit");
        assert!(exit_early < script.find("pixi global install").expect("installs"));
        assert!(script.contains("command -v gh"));
        assert!(script.contains("command -v claude"));
    }

    #[test]
    fn the_early_exit_requires_all_tools_not_any() {
        // `&&`, not `||` — one tool present is not the guarantee.
        let script = provision_script(&REQUIRED_TOOLS);
        let guard = script
            .lines()
            .find(|line| line.starts_with("if command -v"))
            .expect("the all-present guard");
        assert!(guard.contains("&&"), "{guard}");
        assert!(!guard.contains("||"), "{guard}");
    }

    #[test]
    fn each_tool_is_installed_only_when_missing() {
        let script = provision_script(&REQUIRED_TOOLS);
        for tool in REQUIRED_TOOLS {
            assert!(script.contains(&format!(
                "if ! command -v {} >/dev/null 2>&1; then",
                tool.command
            )));
        }
    }

    #[test]
    fn a_failed_install_is_reported_through_the_exit_status() {
        // A tool that would not install must not look like a success.
        let script = provision_script(&REQUIRED_TOOLS);
        assert!(script.contains("failed=1"));
        assert!(script.contains(r#"exit "$failed""#));
    }

    #[test]
    fn it_puts_the_pixi_bin_directory_on_the_login_path() {
        // Without this the next launch reinstalls everything, forever.
        let script = provision_script(&REQUIRED_TOOLS);
        assert!(script.contains(".devlaunch/pixi/bin"));
        assert!(script.contains(".profile"));
    }

    #[test]
    fn it_writes_to_the_profile_bash_actually_reads() {
        // bash sources the FIRST of bash_profile/bash_login/profile that exists. An
        // image shipping a ~/.bash_profile therefore never reads ~/.profile, so
        // writing there leaves the tools installed and unreachable — and, because
        // the presence check is `command -v`, reinstalled on every launch.
        let script = provision_script(&REQUIRED_TOOLS);
        assert!(script.contains(r#"[ -f "$HOME/.bash_profile" ]"#));
        assert!(script.contains(r#"[ -f "$HOME/.bash_login" ]"#));
        let profile = script.find(".bash_profile").expect("bash_profile first");
        let login = script.find(".bash_login").expect("then bash_login");
        let fallback = script
            .find(r#"PROFILE="$HOME/.profile""#)
            .expect("then profile");
        assert!(profile < login && login < fallback);
    }

    #[test]
    fn a_profile_that_cannot_be_written_is_a_failure() {
        // Installed but not on PATH is not the guarantee this module makes.
        for line in appends(&provision_script(&REQUIRED_TOOLS)) {
            assert!(line.ends_with("|| failed=1"), "{line}");
        }
    }

    #[test]
    fn pixi_is_installed_when_the_image_has_none() {
        // An arbitrary repo's container need not carry pixi.
        assert!(provision_script(&REQUIRED_TOOLS).contains("command -v pixi"));
    }

    #[test]
    fn progress_goes_to_stderr_not_stdout() {
        // `dl <ws> -- cmd > file` must not get install chatter in the file. The
        // provisioning ssh is a separate devpod call from the command's, but it
        // shares dl's stdout.
        let script = provision_script(&REQUIRED_TOOLS);
        let redirect = script.find("exec >&2").expect("the redirect");
        assert!(redirect < script.find("echo").expect("something printed"));
    }

    #[test]
    fn a_custom_tool_set_is_honoured() {
        let script = provision_script(&[Tool::new("jq", "jq")]);
        assert!(script.contains("command -v jq"));
        assert!(!script.contains("command -v gh"));
        assert_eq!(script, PYTHON_PROVISION_SCRIPT_JQ);
    }

    // =======================================================================
    // TestProfileGuards
    // =======================================================================

    #[test]
    fn a_profile_edit_lands_where_a_login_shell_will_read_it() {
        // Asked of a real login bash rather than of a rule this test restates: the
        // edit is run in a scratch home, then bash is started as a login shell there
        // and asked what its PATH is. That makes bash the authority on both halves at
        // once, and means all four writers agreeing is a consequence of each being
        // right rather than a separate assertion.
        //
        // Every shape a container's home can have when a writer arrives, because
        // which of those files exists is exactly what decides where an edit has to
        // land to be read.
        const HOME_SHAPES: [&[&str]; 7] = [
            &[],
            &[".profile"],
            &[".bash_profile"],
            &[".bash_login"],
            &[".bash_profile", ".profile"],
            &[".bash_login", ".profile"],
            &[".bash_profile", ".bash_login", ".profile"],
        ];
        let path = std::env::var("PATH").expect("a PATH to run bash with");
        for writer in Writer::ALL {
            for present in HOME_SHAPES {
                let scratch = scratch();
                let home = a_home(scratch.path());
                for name in present {
                    fs::write(home.join(name), "# stock\n").expect("a stock profile");
                }
                let home = home.to_string_lossy().into_owned();
                let edited = bash_with(
                    &profile_edit(&writer.script()),
                    &[
                        ("PATH", path.as_str()),
                        ("HOME", &home),
                        ("TARGET_HOME", &home),
                    ],
                );
                assert!(
                    edited.status.success(),
                    "{writer:?}: {}",
                    String::from_utf8_lossy(&edited.stderr)
                );
                // A login shell, which is what makes bash the authority here.
                let asked = Command::new(bash())
                    .args(["-l", "-c", r#"printf "%s\n" "$PATH""#])
                    .env_clear()
                    .env("PATH", &path)
                    .env("HOME", &home)
                    .output()
                    .expect("a login shell");
                assert!(asked.status.success());
                let login_path = String::from_utf8_lossy(&asked.stdout);
                let prepended = format!("{home}/{}", writer.prepended());
                assert!(
                    login_path.trim().split(':').any(|entry| entry == prepended),
                    "{writer:?} appended to a file a login shell in a home with \
                     {present:?} never reads: {login_path}"
                );
            }
        }
    }

    #[test]
    fn a_guard_asks_about_devlaunchs_own_line_not_about_a_directory() {
        // The guard means "have *we* already prepended our directory here", and the
        // only evidence of that which devlaunch owns is the mark it writes. Asking
        // instead whether the directory is mentioned anywhere in the file makes the
        // answer a base image's to give: Ubuntu's stock ~/.profile prepends
        // ~/.local/bin itself, so on this repo's own base image that question
        // answered "already done" about work nobody had done, the lent binary never
        // reached the front of PATH, and every launch re-paid the transfer.
        for writer in Writer::ALL {
            for guard in guards(&writer.script()) {
                assert!(guard.contains(PROFILE_MARK), "{writer:?}: {guard}");
                for owned_by_the_image in [".local/bin", ".pixi/bin", "pixi/envs/claude-shim"] {
                    assert!(!guard.contains(owned_by_the_image), "{writer:?}: {guard}");
                }
            }
        }
    }

    #[test]
    fn every_line_lands_exactly_once_however_often_the_scripts_run() {
        // Two different lines may never share a mark, and a rerun may never append
        // again. Both are the same property — the mark is what decides "already
        // done" — and both failure modes are silent: two lines under one mark drop
        // whichever comes second, and a missed dedupe grows the profile on every
        // launch. Nothing exits non-zero either way.
        //
        // Run for real, and across *all four* writers into one profile, because that
        // is where the lines meet: a provision and a lend edit the same file over a
        // workspace's life.
        let scratch = scratch();
        let profile = scratch.path().join("profile");
        let mut lines = Vec::new();
        for writer in Writer::ALL {
            lines.extend(appends(&writer.script()));
        }
        assert_eq!(
            lines.len(),
            6,
            "a new PATH line belongs in this test's expectations"
        );
        let path = std::env::var("PATH").expect("a PATH to run bash with");
        for _ in 0..2 {
            let appended = bash_with(
                &lines.join("\n"),
                &[
                    ("PATH", path.as_str()),
                    ("PROFILE", &profile.to_string_lossy()),
                ],
            );
            assert!(
                appended.status.success(),
                "{}",
                String::from_utf8_lossy(&appended.stderr)
            );
        }
        let written = fs::read_to_string(&profile).expect("the profile");
        for line in [PIXI_BIN_LINE, CLAUDE_SHIM_BIN_LINE, LOCAL_BIN_LINE] {
            let count = written.lines().filter(|written| *written == line).count();
            assert_eq!(count, 1, "{line:?} appended {count} times");
        }
    }

    #[test]
    fn a_guard_matches_a_whole_line_not_a_fragment_of_one() {
        // A mark is only devlaunch's if nothing longer can pass for it: an
        // exact-line, fixed-string match, so neither a regex metacharacter nor a
        // longer line that happens to contain the mark counts as a hit.
        for writer in Writer::ALL {
            for guard in guards(&writer.script()) {
                assert!(guard.starts_with("grep -qxF "), "{writer:?}: {guard}");
            }
        }
    }

    /// The two PATH lines `.devcontainer/claude-code/install.sh` writes, which are
    /// deliberately *not* [`PIXI_BIN_LINE`] and [`CLAUDE_SHIM_BIN_LINE`] any more.
    ///
    /// The feature runs at image build time and installs into the image's own
    /// `~/.pixi`, where there is no checkout to dirty and no other owner to race --
    /// so it has no reason to move, and moving it would invalidate the devcontainer
    /// prebuild for nothing. The runtime writers moved; this did not, and the two
    /// therefore append two different lines under two different marks. Both land,
    /// which is correct: they name two real directories.
    const FEATURE_PIXI_BIN_LINE: &str = r#"export PATH="$HOME/.pixi/bin:$PATH""#;
    const FEATURE_CLAUDE_SHIM_BIN_LINE: &str = r#"[ -d "$HOME/.pixi/envs/claude-shim/bin" ] && export PATH="$HOME/.pixi/envs/claude-shim/bin:$PATH""#;

    #[test]
    fn the_feature_installer_writes_the_very_fragments_devlaunch_renders() {
        // The installer's two profile edits are `profile_prepend`'s own output,
        // verbatim — same derived mark, same guard, same line. Pinned byte-for-byte
        // rather than by shape, because the marks are content hashes a shell script
        // cannot derive for itself: hardcoded, they drift the moment either side's
        // line changes, and a drifted mark quietly costs a workspace one duplicate
        // PATH entry per line.
        let installer = feature_installer();
        for line in [FEATURE_PIXI_BIN_LINE, FEATURE_CLAUDE_SHIM_BIN_LINE] {
            let fragment = profile_prepend(line, None);
            assert!(
                installer.contains(&fragment),
                "the feature installer no longer carries the rendered append for \
                 {line:?}; regenerate it from profile_prepend"
            );
        }
    }

    #[test]
    fn every_writer_carries_the_one_resolution_the_module_renders() {
        // There is one answer to "which file is the login profile", rendered in one
        // place, and every writer carries that rendering. The test above proves each
        // writer's edit reaches a login shell today; this one is why it stays true —
        // two hand-maintained copies of the answer drift, and the drift is silent.
        // The home variable is the *only* difference allowed between them, which is
        // what asserting equality here says.
        for writer in Writer::ALL {
            assert_eq!(
                resolution_block(&writer.script()),
                profile_resolution(writer.home_var()),
                "{writer:?} no longer resolves the login profile the way the module does"
            );
        }
    }

    #[test]
    fn no_profile_edit_hides_from_these_tests_under_another_name() {
        // The tests here find profile edits by matching one spelling, so a writer
        // that appended under any other name would be silently exempt from all of
        // them — unguarded, unresolved, and reported as covered.
        for writer in Writer::ALL {
            for line in writer.script().lines() {
                if line.contains(">>") {
                    assert!(
                        line.contains(r#">> "$PROFILE""#),
                        "{writer:?} appends to something other than $PROFILE, which \
                         these tests do not see: {line:?}"
                    );
                }
            }
        }
    }

    // =======================================================================
    // TestProbeScript — run for real against scratch homes
    // =======================================================================

    #[test]
    fn a_container_with_neither_tool_answers_absent() {
        let scratch = scratch();
        let home = a_home(scratch.path());
        assert_eq!(
            probe_answer(scratch.path(), &home, &[]),
            ProbeResult::Absent
        );
    }

    #[test]
    fn the_official_claude_layout_answers_provisioned() {
        // The prebuilt-image jackpot: nothing to do, one round trip.
        let scratch = scratch();
        let home = a_home(scratch.path());
        official_claude(&home, "2.0.1");
        let gh = a_gh(&home);
        assert_eq!(
            probe_answer(scratch.path(), &home, &[gh]),
            ProbeResult::Provisioned
        );
    }

    #[test]
    fn a_baked_shim_answers_lendable_rather_than_provisioned() {
        // The whole point of the three states. A `claude` that is a downloader
        // satisfies `command -v` while still owing the ~285MB the lending exists to
        // avoid, so it must not be mistaken for a provisioned workspace.
        let scratch = scratch();
        let home = a_home(scratch.path());
        claude_shim(&home);
        let gh = a_gh(&home);
        assert_eq!(
            probe_answer(scratch.path(), &home, &[gh]),
            ProbeResult::Lendable
        );
    }

    #[test]
    fn a_real_claude_with_no_gh_is_absent_not_lendable() {
        // `lendable` means "replace the claude"; a container missing gh outright
        // needs the cold flow, which the network fallback can finish.
        let scratch = scratch();
        let home = a_home(scratch.path());
        official_claude(&home, "2.0.1");
        assert_eq!(
            probe_answer(scratch.path(), &home, &[home.join(".local/bin")]),
            ProbeResult::Absent
        );
    }

    #[test]
    fn the_layout_a_lend_leaves_behind_answers_provisioned() {
        // Convergence, run for real end to end: the transfer script unpacks into a
        // scratch $HOME, and the probe is then asked about that same home. The two
        // scripts have to agree or the lending never terminates — a lend the next
        // probe does not recognise means every `up` for the rest of that workspace's
        // life re-pays the transfer.
        let scratch = scratch();
        let home = a_home(scratch.path());
        lend_into(scratch.path(), &home, "2.0.1");
        assert_eq!(
            probe_answer(scratch.path(), &home, &[home.join(".local/bin")]),
            ProbeResult::Provisioned
        );
    }

    #[test]
    fn a_lend_converges_on_the_image_this_repo_ships() {
        // Convergence where it actually has to hold: a shim container whose login
        // profile is Ubuntu's stock one plus the lines this repo's own devcontainer
        // appends, with PATH decided by *sourcing that profile* rather than handed to
        // the probe by the test. PATH order is the whole mechanism by which a lend
        // takes effect, so a convergence test that builds PATH itself asserts a world
        // in which the profile edit cannot be wrong.
        let scratch = scratch();
        let home = a_home(scratch.path());
        fs::write(
            home.join(".profile"),
            format!("{UBUNTU_STOCK_PROFILE}{DEVCONTAINER_PROFILE_LINES}"),
        )
        .expect("the image's profile");
        write_program(
            &home.join(".pixi/envs/claude-shim/bin/claude"),
            "#!/bin/sh\necho 'downloading 285MB' >&2\n",
        );
        write_program(&home.join(".pixi/bin/gh"), "#!/bin/sh\nexit 0\n");

        assert_eq!(
            probe_answer_after_login(scratch.path(), &home),
            ProbeResult::Lendable
        );
        lend_into(scratch.path(), &home, "2.0.1");
        assert_eq!(
            probe_answer_after_login(scratch.path(), &home),
            ProbeResult::Provisioned
        );
        // And the second lend never happens: converged means converged, not
        // converged-then-drifting-back.
        assert_eq!(
            probe_answer_after_login(scratch.path(), &home),
            ProbeResult::Provisioned
        );
    }

    #[test]
    fn a_shim_hiding_inside_the_versions_directory_is_lendable() {
        // "Under the versions directory" is not the official layout — being a direct
        // child of it is. A downloader is free to park itself at
        // `versions/latest/bin/claude`, and a probe that accepts any depth trusts it
        // and leaves the container owing the download the lend exists to remove.
        let scratch = scratch();
        let home = a_home(scratch.path());
        nested_shim(&home);
        let gh = a_gh(&home);
        assert_eq!(
            probe_answer(scratch.path(), &home, &[gh]),
            ProbeResult::Lendable
        );
    }

    #[test]
    fn neither_side_of_the_pipe_can_disagree_about_one_tree() {
        // One definition of "the official install", asked from both ends. The
        // container decides whether to keep what it has; the host decides what it may
        // lend. Those are the same question about the same layout, and an answer of
        // provisioned for a tree the host would refuse to lend is a shim nothing will
        // ever replace — so the two are pinned together rather than separately.
        for layout in [
            official_claude as fn(&Path, &str) -> PathBuf,
            |home: &Path, _version: &str| claude_shim(home),
            |home: &Path, _version: &str| nested_shim(home),
        ] {
            let scratch = scratch();
            let home = a_home(scratch.path());
            layout(&home, "2.0.1");
            a_gh(&home);
            let container = probe_answer(scratch.path(), &home, &[home.join(".local/bin")]);
            let host = claude_source(&home);
            assert_eq!(
                container == ProbeResult::Provisioned,
                host.is_some(),
                "{container:?} against {host:?}"
            );
        }
    }

    #[test]
    fn a_real_install_reached_through_a_symlinked_home_is_provisioned() {
        // Images reach `$HOME` through a symlink often enough to matter, and
        // resolving the claude while comparing against an unresolved `$HOME` makes a
        // genuine install read lendable forever: the lend succeeds, changes nothing
        // the next probe recognises, and is re-paid on every single `up`.
        let scratch = scratch();
        let real = scratch.path().join("real-home");
        fs::create_dir_all(&real).expect("a real home");
        official_claude(&real, "2.0.1");
        a_gh(&real);
        let home = scratch.path().join("home-link");
        std::os::unix::fs::symlink(&real, &home).expect("a symlinked home");

        assert_eq!(
            probe_answer(scratch.path(), &home, &[home.join(".local/bin")]),
            ProbeResult::Provisioned
        );
        // And the host says the same about the same tree, which is what stops one
        // side lending what the other refuses.
        assert!(claude_source(&home).is_some());
    }

    #[test]
    fn a_trailing_slash_on_home_does_not_hide_the_official_install() {
        // A `$HOME` written with a trailing slash is a legal value of the variable
        // and names the same directory, so it must not turn a real install into a
        // lend.
        let scratch = scratch();
        let home = a_home(scratch.path());
        official_claude(&home, "2.0.1");
        a_gh(&home);
        let with_slash = format!("{}/", home.to_string_lossy());
        assert_eq!(
            probe_answer_with_home(
                scratch.path(),
                &[home.join(".local/bin")],
                Some(&with_slash)
            ),
            ProbeResult::Provisioned
        );
    }

    #[test]
    fn a_container_with_no_home_set_still_answers() {
        // "Exits 0 in every state" has to survive an image that never set `HOME`:
        // under `set -u` an unguarded expansion aborts the script, which is the red
        // devpod `fatal` this probe was written to retire.
        let scratch = scratch();
        let home = a_home(scratch.path());
        official_claude(&home, "2.0.1");
        let gh = a_gh(&home);
        assert_eq!(
            probe_answer_with_home(scratch.path(), &[gh], None),
            ProbeResult::Lendable
        );
    }

    #[test]
    fn it_resolves_the_link_rather_than_trusting_the_path_entry() {
        // In the official layout `~/.local/bin/claude` is a symlink, so what it
        // points at is the only thing that says which install it belongs to.
        assert!(probe_script().contains("readlink -f"));
    }

    #[test]
    fn it_never_runs_the_candidate_claude() {
        // Shim-proofness, asked of a run rather than of the script's text: *any*
        // invocation of the shim triggers the download the probe exists to detect, so
        // the probe runs here against a claude that records being invoked, and the
        // record must stay empty.
        //
        // The recorder is shell builtins only — `:` and a redirection — because the
        // probe runs on a stripped PATH carrying nothing but `readlink`.
        let scratch = scratch();
        let home = a_home(scratch.path());
        write_program(
            &home.join(".local/bin/claude"),
            "#!/bin/sh\n: > \"$HOME/shim-was-executed\"\n",
        );
        let gh = a_gh(&home);

        // The answer proves the run reached the end of the script: a probe that
        // crashed before resolving the claude would leave the record empty too.
        assert_eq!(
            probe_answer(scratch.path(), &home, &[gh]),
            ProbeResult::Lendable
        );
        assert!(!home.join("shim-was-executed").exists());
    }

    #[test]
    fn the_script_text_confines_claude_to_the_known_lookups() {
        // A complement to the behavioural test above, not the guard itself. The run
        // proves that one real probe executed nothing; this scrub confines where the
        // name may appear at all, which is what catches an invocation parked on a
        // branch that run does not take.
        let script = probe_script();
        let scrubbed = script
            .replace("command -v claude", "")
            .replace(CLAUDE_VERSIONS_RELPATH, "")
            .replace("devlaunch-probe claude", "");
        assert!(!scrubbed.contains("claude"), "{scrubbed}");
        assert!(!script.contains("--version"));
    }

    #[test]
    fn the_container_is_never_asked_to_name_a_state() {
        // The container reports two resolved paths and no verdict. A token would mean
        // it had decided, and deciding needs its own copy of "the official layout" —
        // the second opinion that let a shim parked deeper under the versions
        // directory be trusted by the container while the host refused to lend the
        // very same tree.
        let script = probe_script();
        for state in ProbeResult::ALL {
            assert!(!script.contains(state.word()), "{}", state.word());
        }
    }

    #[test]
    fn the_official_layout_is_defined_once_for_both_sides_of_the_pipe() {
        assert_eq!(CLAUDE_VERSIONS_RELPATH, ".local/share/claude/versions");
        assert!(probe_script().contains(CLAUDE_VERSIONS_RELPATH));
        assert!(transfer_script(&golden_payload()).contains(CLAUDE_VERSIONS_RELPATH));
    }

    // =======================================================================
    // TestProbeResult
    // =======================================================================

    #[test]
    fn it_reads_the_state_out_of_what_the_container_reported() {
        assert_eq!(ProbeResult::parse(REPORT_ABSENT), ProbeResult::Absent);
        assert_eq!(
            ProbeResult::parse(REPORT_PROVISIONED),
            ProbeResult::Provisioned
        );
        assert_eq!(ProbeResult::parse(REPORT_LENDABLE), ProbeResult::Lendable);
    }

    #[test]
    fn a_claude_deeper_under_the_versions_directory_is_not_the_install() {
        // "Under" is not "in". The installer writes one binary per version directly
        // into that directory, so a path that merely starts with it is somebody
        // else's — a downloader's.
        let report = concat!(
            "devlaunch-probe tools present\n",
            "devlaunch-probe versions /ws/.local/share/claude/versions\n",
            "devlaunch-probe claude /ws/.local/share/claude/versions/latest/bin/claude\n",
        );
        assert_eq!(ProbeResult::parse(report), ProbeResult::Lendable);
    }

    #[test]
    fn a_container_that_could_resolve_nothing_is_not_provisioned() {
        // Two blanks are equal, and equality is the whole test — so a container with
        // no `readlink`, which resolves neither path, must not come out as the one
        // perfect match.
        let report =
            "devlaunch-probe tools present\ndevlaunch-probe versions\ndevlaunch-probe claude\n";
        assert_eq!(ProbeResult::parse(report), ProbeResult::Lendable);
    }

    #[test]
    fn a_chatty_login_profile_does_not_hide_the_answer() {
        // The probe runs under `bash -lc`, so the container's profile is sourced
        // first and an image whose profile prints a banner puts that on the same
        // stdout.
        let report = format!("Welcome to this image!\n\n{REPORT_PROVISIONED}");
        assert_eq!(ProbeResult::parse(&report), ProbeResult::Provisioned);
    }

    #[test]
    fn a_report_it_cannot_read_means_absent() {
        // Parsing is total, and it errs towards doing the work again: provisioning is
        // idempotent, so a wrong absent costs a redundant trip where a wrong
        // provisioned would silently skip the whole point.
        for report in [
            "",
            "bash: line 1: oh no",
            "provisioned",
            "devlaunch-probe tools\n",
            "tools present\nversions /ws/.local/share/claude/versions\n",
        ] {
            assert_eq!(
                ProbeResult::parse(report),
                ProbeResult::Absent,
                "{report:?}"
            );
        }
    }

    #[test]
    fn a_relative_or_empty_resolution_is_never_the_official_layout() {
        // Of the two ways to be wrong, lending again costs seconds and trusting a
        // shim ships a container that still owes the download.
        assert!(!is_official_claude("", ""));
        assert!(!is_official_claude(
            ".local/share/claude/versions",
            ".local/share/claude/versions/2.0.1"
        ));
        assert!(is_official_claude("/v", "/v/2.0.1"));
        // A trailing or doubled slash names the same directory, as PurePosixPath
        // reads it.
        assert!(is_official_claude("/v/", "/v/2.0.1"));
        assert!(is_official_claude("/v", "/v//2.0.1"));
    }

    // =======================================================================
    // TestSetupPassScript
    // =======================================================================

    #[test]
    fn it_carries_the_probe_script_verbatim() {
        // Composed *from* the probe, never a re-expression of it: the relation the
        // probe reports on is stated once and asked from both ends, so a composition
        // that rewrote or trimmed the probe would be the second copy.
        let script = setup_script(&setup_stages(
            "myws",
            ToolsSwitch::Install,
            ZellijSwitch::Install,
            None,
        ));
        assert!(script.contains(&probe_script()));
    }

    #[test]
    fn the_stages_run_in_front_of_the_probe() {
        // Order, and it is not cosmetic: the probe exits early when a tool is
        // missing, which is the commonest cold-path answer, so a stage placed behind
        // it would report "not reached" on the very launches the fold exists for.
        let stages = setup_stages("myws", ToolsSwitch::Install, ZellijSwitch::Install, None);
        let script = setup_script(&stages);
        let probe_at = script.find(&probe_script()).expect("the probe is in there");
        for stage in &stages {
            assert!(
                script.find(&stage.command).expect("the stage") < probe_at,
                "{}",
                stage.name.as_str()
            );
        }
    }

    #[test]
    fn the_hostname_stage_names_the_container_without_the_id_s_suffix() {
        // The workspace id is what devpod is called with; the name in the container's
        // UTS namespace addresses nothing, so it carries the readable half alone. The
        // stage still receives the id — it is what the whole pass is keyed on — and
        // the drop happens here, in the one place a hostname is spelled.
        let id = "devlaunch-main-zovomobo";
        let script = setup_script(&setup_stages(
            id,
            ToolsSwitch::Install,
            ZellijSwitch::Install,
            None,
        ));

        assert!(script.contains("sudo hostname devlaunch-main;"), "{script}");
        assert!(!script.contains(&format!("hostname {id}")));
    }

    #[test]
    fn no_set_e_spans_the_stages() {
        // One stage's failure is contained to that stage. `-e` anywhere in the pass
        // would make the first failing stage take the probe's answer with it, which
        // is why the transfer — correctly `set -eu` for the all-or-nothing sequence
        // it is — is not a stage in this pass.
        let script = setup_script(&setup_stages(
            "myws",
            ToolsSwitch::Install,
            ZellijSwitch::Install,
            None,
        ));
        assert!(!script.contains("set -e"));
    }

    #[test]
    fn a_workspace_name_cannot_run_a_command_of_its_own() {
        // The name is interpolated into a shell script, so it is quoted.
        let name = "myws; touch /tmp/pwned";
        let script = setup_script(&setup_stages(
            name,
            ToolsSwitch::Install,
            ZellijSwitch::Install,
            None,
        ));
        assert!(script.contains(&format!("hostname {}", quote(name))));
        assert!(!script.contains("hostname myws;"));
    }

    #[test]
    #[should_panic(expected = "must not contain a space")]
    fn a_stage_name_with_a_space_has_no_representation() {
        // A two-word name shears every outcome line, so every stage reads as not
        // reached. At the definition sites the constructor runs in `const`
        // context, so there this panic is E0080 — a compile error — rather than
        // anything a launch can hit; this pins the rule the constant evaluator
        // enforces.
        let _ = StageName::new("set hostname");
    }

    /// Run the whole pass for real, and read it the way the host reads it.
    ///
    /// `sudo_exit` puts a `sudo` on PATH that exits with that status; `None` leaves
    /// the stripped PATH with no `sudo` at all, which is what an image without it
    /// does. Never the host's real sudo — this must not be able to prompt, and must
    /// not be able to rename the machine running the tests.
    fn run_setup_pass(
        scratch: &Path,
        workspace: &str,
        sudo_exit: Option<i32>,
    ) -> (PathBuf, String) {
        let home = a_home(scratch);
        let sysbin = sysbin(scratch, &["readlink"]);
        if let Some(status) = sudo_exit {
            write_program(&sysbin.join("sudo"), &format!("#!/bin/sh\nexit {status}\n"));
        }
        let script = setup_script(&setup_stages(
            workspace,
            ToolsSwitch::Install,
            ZellijSwitch::Install,
            None,
        ));
        let ran = bash_with(
            &script,
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &sysbin.to_string_lossy()),
            ],
        );
        // Exits 0 whatever the stages did: a non-zero `devpod ssh` has to keep
        // meaning the transport failed, which is the discrimination the probe already
        // relies on.
        assert!(
            ran.status.success(),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        (home, String::from_utf8_lossy(&ran.stdout).into_owned())
    }

    /// One stage's outcome out of a pass that runs more than one, by name.
    fn outcome_of(report: &str, stage: StageName) -> Option<StageOutcome> {
        stage_outcomes(
            report,
            &setup_stages("myws", ToolsSwitch::Install, ZellijSwitch::Install, None),
        )
        .into_iter()
        .find(|outcome| outcome.stage == stage)
    }

    #[test]
    fn a_real_bash_over_the_pass_calls_hostname_with_the_id_s_readable_half() {
        // The derivation through a real shell, reading what `hostname` was actually
        // handed rather than what the script says. The name is interpolated as a
        // quoted word, so this is also where a quoting change that split it into two
        // arguments would show up: `$#` is recorded beside them.
        let scratch = scratch();
        let home = a_home(scratch.path());
        let sysbin = sysbin(scratch.path(), &["readlink"]);
        let seen = scratch.path().join("sudo-argv");
        write_program(
            &sysbin.join("sudo"),
            &format!(
                "#!/bin/sh\necho \"$# $*\" > \"{}\"\nexit 0\n",
                seen.to_string_lossy()
            ),
        );
        let script = setup_script(&setup_stages(
            "devlaunch-main-zovomobo",
            ToolsSwitch::Install,
            ZellijSwitch::Install,
            None,
        ));

        let ran = bash_with(
            &script,
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &sysbin.to_string_lossy()),
            ],
        );

        assert!(
            ran.status.success(),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        assert_eq!(
            fs::read_to_string(&seen).expect("sudo ran").trim(),
            "2 hostname devlaunch-main"
        );
    }

    #[test]
    fn a_failing_stage_does_not_cost_the_probe_its_answer() {
        // The legibility claim, run rather than argued: the stage fails, names itself
        // with its status, and the probe still answers in the same trip.
        let scratch = scratch();
        let (home, report) = run_setup_pass(scratch.path(), "myws", Some(1));
        assert_eq!(ProbeResult::parse(&report), ProbeResult::Absent);
        assert_eq!(
            outcome_of(&report, HOSTNAME_STAGE),
            Some(StageOutcome {
                stage: HOSTNAME_STAGE,
                result: StageResult::Failed { status: 1 },
            })
        );
        assert!(home.exists());
    }

    #[test]
    fn a_stage_the_image_cannot_even_run_reports_its_status() {
        // No `sudo` in the image at all: 127, not silence.
        let scratch = scratch();
        let (_, report) = run_setup_pass(scratch.path(), "myws", None);
        assert_eq!(
            outcome_of(&report, HOSTNAME_STAGE),
            Some(StageOutcome {
                stage: HOSTNAME_STAGE,
                result: StageResult::Failed { status: 127 },
            })
        );
    }

    #[test]
    fn a_stage_that_worked_says_so() {
        // The privileged-image case, which is the one the user can see.
        let scratch = scratch();
        let (_, report) = run_setup_pass(scratch.path(), "myws", Some(0));
        assert_eq!(
            outcome_of(&report, HOSTNAME_STAGE),
            Some(StageOutcome {
                stage: HOSTNAME_STAGE,
                result: StageResult::Ok,
            })
        );
    }

    // =======================================================================
    // TestStageOutcomes
    // =======================================================================

    /// Read the hostname stage's outcome out of the real declared stage, narrowed to
    /// the one stage because every assertion below is about how one reported line is
    /// *read*.
    fn hostname_outcome(report: &str) -> Vec<StageOutcome> {
        let stages: Vec<Stage> =
            setup_stages("myws", ToolsSwitch::Install, ZellijSwitch::Install, None)
                .into_iter()
                .filter(|stage| stage.name == HOSTNAME_STAGE)
                .collect();
        assert!(!stages.is_empty(), "the pass no longer names the container");
        stage_outcomes(report, &stages)
    }

    fn just(result: StageResult) -> Vec<StageOutcome> {
        vec![StageOutcome {
            stage: HOSTNAME_STAGE,
            result,
        }]
    }

    #[test]
    fn a_stage_that_worked_reads_ok() {
        assert_eq!(hostname_outcome(STAGE_OK_LINE), just(StageResult::Ok));
    }

    #[test]
    fn a_stage_that_failed_carries_its_status() {
        assert_eq!(
            hostname_outcome(STAGE_FAILED_LINE),
            just(StageResult::Failed { status: 1 })
        );
    }

    #[test]
    fn a_report_truncated_before_a_stage_reads_not_reached() {
        // What a mid-script death or a cut-off report looks like. It must not read as
        // ok: "never ran" and "ran fine" are the two states this whole value exists
        // to keep apart.
        assert_eq!(
            hostname_outcome(REPORT_ABSENT),
            just(StageResult::NotReached)
        );
    }

    #[test]
    fn an_outcome_it_cannot_read_is_never_read_as_ok() {
        // Total, and it errs the way the report is used: anything that is not a
        // readable outcome is the absence of one, and the host names every outcome
        // that is not ok.
        for report in [
            "devlaunch-probe stage hostname\n",
            "devlaunch-probe stage hostname failed\n",
            "devlaunch-probe stage hostname failed sideways\n",
            "devlaunch-probe stage hostname yes\n",
            "stage hostname ok\n",
            // Python reads these two with `isdigit()` and then `int()`: the first
            // raises ValueError inside the parse (a superscript is a digit that is
            // not an integer), and the second is a status no process ever exited
            // with. Both are the absence of a readable outcome.
            "devlaunch-probe stage hostname failed ²\n",
            "devlaunch-probe stage hostname failed 99999999999999999999\n",
        ] {
            assert_eq!(
                hostname_outcome(report),
                just(StageResult::NotReached),
                "{report:?}"
            );
        }
    }

    #[test]
    fn a_chatty_login_profile_does_not_hide_an_outcome() {
        // The pass runs under `bash -lc` exactly as the probe does, so the
        // container's banner lands on the same stdout.
        let report = format!("Welcome to this image!\n\n{STAGE_OK_LINE}");
        assert_eq!(hostname_outcome(&report), just(StageResult::Ok));
    }

    #[test]
    fn the_probe_is_inert_to_the_outcome_lines() {
        // The container gains stage outcomes and gains no opinions: the probe parser
        // reads only the keys it always read, so the stage lines are the same shape
        // of noise as a login banner.
        assert_eq!(
            ProbeResult::parse(&format!("{STAGE_OK_LINE}{REPORT_PROVISIONED}")),
            ProbeResult::Provisioned
        );
        assert_eq!(
            ProbeResult::parse(&format!("{STAGE_FAILED_LINE}{REPORT_ABSENT}")),
            ProbeResult::Absent
        );
    }

    // =======================================================================
    // TestProvisionTools — the three-trip flow
    // =======================================================================

    #[test]
    fn a_provisioned_workspace_pays_one_setup_pass_and_nothing_else() {
        // The common path: every launch after the first is one round trip, and that
        // trip is the whole setup pass — stages and probe together.
        let runner = Trips::new(&[0]).reporting(REPORT_PROVISIONED);

        let outcome = provision_with(&runner);

        assert_eq!(outcome, Provisioning::AlreadyProvisioned);
        assert!(outcome.tools_present());
        assert_eq!(runner.count(), 1);
        assert_eq!(runner.trips()[0].argv[..3], ["devpod", "ssh", "myws"]);
        // A non-login shell has no ~/.pixi/bin, so every tool would look missing.
        let words = shlex::split(&runner.script(0)).expect("a readable payload");
        assert_eq!(words[..2], ["bash".to_owned(), "-lc".to_owned()]);
        assert_eq!(
            words[2],
            setup_script(&setup_stages(
                "myws",
                ToolsSwitch::Install,
                ZellijSwitch::Install,
                None,
            ))
        );
    }

    #[test]
    fn the_container_is_never_named_by_a_trip_of_its_own() {
        // The saving, stated as the thing that must not appear: on no probe answer
        // does a trip go out whose whole payload is the hostname. Inside the pass's
        // own script it is expected — that is the fold.
        for report in [REPORT_PROVISIONED, REPORT_LENDABLE, REPORT_ABSENT] {
            let runner = Trips::new(&[0, 0]).reporting(report);
            let _ = provision_with(&runner);
            for trip in runner.trips() {
                assert!(
                    !trip.argv.iter().any(|arg| arg == "sudo hostname myws"),
                    "{:?}",
                    trip.argv
                );
            }
        }
    }

    #[test]
    fn with_nothing_to_lend_a_cold_workspace_gets_the_network_install() {
        let runner = Trips::new(&[0, 0]).reporting(REPORT_ABSENT);

        assert_eq!(provision_with(&runner), Provisioning::Installed);

        assert_eq!(runner.count(), 2);
        let words = shlex::split(&runner.script(1)).expect("a readable payload");
        assert_eq!(words[..2], ["bash".to_owned(), "-lc".to_owned()]);
        assert_eq!(words[2], provision_script(&REQUIRED_TOOLS));
    }

    #[test]
    fn a_probe_trip_that_fails_outright_is_read_as_absent() {
        // The script exits 0 in all three states, so a non-zero trip is the trip
        // failing rather than an answer — and the cold flow is the safe reading of no
        // answer at all.
        let runner = Trips::new(&[1, 0]).reporting(REPORT_PROVISIONED);

        assert_eq!(provision_with(&runner), Provisioning::Installed);

        assert_eq!(runner.count(), 2);
        let words = shlex::split(&runner.script(1)).expect("a readable payload");
        assert_eq!(words[2], provision_script(&REQUIRED_TOOLS));
    }

    #[test]
    fn a_host_with_the_tools_lends_them_instead_of_the_network() {
        let scratch = scratch();
        let runner = Trips::new(&[0, 0]).reporting(REPORT_ABSENT);
        let host = a_host_that_can_lend(scratch.path());

        let (outcome, _) = events_of(&runner, Switches::INSTALLING, &host);

        assert_eq!(outcome, Ok(Provisioning::Lent));
        assert_eq!(runner.count(), 2);
        // The transfer is the tar stream: stdin on the trip, no stream elsewhere.
        assert_eq!(runner.streamed(), vec![false, true]);
        assert!(runner.script(1).contains("tar xf -"));
    }

    #[test]
    fn a_transfer_the_container_rejects_falls_back_to_the_network() {
        // The lent binaries may not run there (arch, libc); the gate at the end of
        // the transfer script reports it, and the old path still runs.
        let scratch = scratch();
        let runner = Trips::answering(&[Answer::Exited(0), Answer::Exited(1), Answer::Exited(0)])
            .reporting(REPORT_ABSENT);
        let host = a_host_that_can_lend(scratch.path());

        let (outcome, _) = events_of(&runner, Switches::INSTALLING, &host);

        assert_eq!(outcome, Ok(Provisioning::Installed));
        assert_eq!(runner.count(), 3);
        let words = shlex::split(&runner.script(2)).expect("a readable payload");
        assert_eq!(words[2], provision_script(&REQUIRED_TOOLS));
    }

    #[test]
    fn a_shim_workspace_is_lent_the_hosts_real_claude() {
        // Lendable is the state this whole design exists for: both tools answer, but
        // the claude is a downloader, so the host replaces it.
        let scratch = scratch();
        let runner = Trips::new(&[0, 0]).reporting(REPORT_LENDABLE);
        let host = a_host_that_can_lend(scratch.path());

        let (outcome, _) = events_of(&runner, Switches::INSTALLING, &host);

        assert_eq!(outcome, Ok(Provisioning::Lent));
        assert_eq!(runner.count(), 2);
        assert_eq!(runner.streamed(), vec![false, true]);
        assert!(runner.script(1).contains("tar xf -"));
    }

    #[test]
    fn a_shim_the_lend_could_not_replace_is_accepted_not_reinstalled() {
        // The container could not run the lent binaries, but it does have a claude
        // and a gh. The network fallback decides what to install with its own
        // `command -v` guards, which both already satisfy, so a third trip would
        // install nothing — it is not taken.
        let scratch = scratch();
        let runner = Trips::new(&[0, 1]).reporting(REPORT_LENDABLE);
        let host = a_host_that_can_lend(scratch.path());

        let (outcome, _) = events_of(&runner, Switches::INSTALLING, &host);

        assert_eq!(outcome, Ok(Provisioning::ShimKept));
        assert_eq!(runner.count(), 2);
    }

    #[test]
    fn a_host_with_no_home_to_look_in_still_pays_the_pass_and_installs() {
        // Nothing to lend is not nothing to do: the pass runs because the stages it
        // carries are not tools work, and the network path still follows.
        let runner = Trips::new(&[0, 0]).reporting(REPORT_ABSENT);
        let mut events = Vec::new();

        let outcome = provision_tools(
            &runner,
            "myws",
            PassOccasion::AfterUp,
            Switches::INSTALLING,
            None,
            None,
            None,
            &mut events,
        );

        assert_eq!(outcome, Ok(Provisioning::Installed));
        assert_eq!(runner.count(), 2);
    }

    #[test]
    fn a_shim_with_nothing_to_lend_stops_after_the_probe() {
        // Nothing on the host to replace it with, and the network fallback would
        // no-op, so the shim stands and the launch costs one trip.
        let runner = Trips::new(&[0]).reporting(REPORT_LENDABLE);

        assert_eq!(provision_with(&runner), Provisioning::ShimKept);

        assert_eq!(runner.count(), 1);
    }

    #[test]
    fn the_install_output_reaches_the_user_but_the_probe_is_silent() {
        // A cold install is tens of seconds; captured, it reads as a hung dl. The
        // probe is the exception: its output is not progress but the answer the
        // caller branches on.
        let runner = Trips::new(&[0, 0]).reporting(REPORT_ABSENT);

        let _ = provision_with(&runner);

        assert_eq!(runner.captured(), vec![true, false]);
    }

    #[test]
    fn a_failed_install_is_named_rather_than_raised() {
        // The workspace is up and the user asked for a session, not an install.
        let runner = Trips::new(&[0, 1]).reporting(REPORT_ABSENT);

        let (outcome, events) = events_of(&runner, Switches::INSTALLING, &nothing_to_lend());

        let outcome = outcome.expect("devpod answered");
        assert_eq!(
            outcome,
            Provisioning::InstallRefused {
                exit: Exit::Code(1)
            }
        );
        assert!(!outcome.tools_present());
        // Alongside the stage reports the pass produced — the fixture report carries
        // no stage lines, so both stages read as not reached.
        assert!(
            events.contains(&ProvisionEvent::NotInstalled {
                workspace: "myws".to_owned(),
                tools: vec!["gh", "claude"],
                exit: Exit::Code(1),
            }),
            "{events:?}"
        );
    }

    #[test]
    fn a_missing_devpod_is_the_one_error_that_travels() {
        // Python gives DevpodNotInstalled a class that is deliberately not an
        // OSError, so the `except OSError` around this flow never swallows it; the
        // binary renders it as exit 127.
        let runner = Trips::answering(&[Answer::NoDevpod]);

        let (outcome, events) = events_of(&runner, Switches::INSTALLING, &nothing_to_lend());

        assert_eq!(outcome, Err(DevpodMissing));
        assert_eq!(events, vec![], "no report came back to read stages from");
    }

    #[test]
    fn an_os_refusal_costs_the_tools_and_not_the_session() {
        // Python's `except OSError: return False`, as an arm rather than a bool.
        let runner = Trips::answering(&[Answer::Blocked]);

        let (outcome, events) = events_of(&runner, Switches::INSTALLING, &nothing_to_lend());

        let refusal = NotRun::Blocked(OsFailure {
            kind: std::io::ErrorKind::PermissionDenied,
            errno: Some(13),
        });
        let outcome = outcome.expect("not a devpod that went missing");
        assert_eq!(outcome, Provisioning::TripRefused { refusal });
        assert!(!outcome.tools_present());
        assert_eq!(
            events,
            vec![ProvisionEvent::TripRefused {
                workspace: "myws".to_owned(),
                refusal,
            }]
        );
    }

    #[test]
    fn the_opt_out_installs_nothing_and_still_names_the_container() {
        // Whether the pass runs and whether the tools work are two questions, and the
        // opt-out answers only the second. Riding the hostname inside a gate called
        // "no tools" would mean turning tools off silently turned container naming
        // off with it.
        let runner = Trips::new(&[0]).reporting(REPORT_ABSENT);

        let (outcome, events) = events_of(
            &runner,
            Switches {
                tools: ToolsSwitch::Skip,
                zellij: ZellijSwitch::Install,
            },
            &nothing_to_lend(),
        );

        let outcome = outcome.expect("devpod answered");
        assert_eq!(outcome, Provisioning::Disabled);
        assert!(!outcome.tools_present());
        assert_eq!(runner.count(), 1);
        let words = shlex::split(&runner.script(0)).expect("a readable payload");
        assert_eq!(
            words[2],
            setup_script(&setup_stages(
                "myws",
                ToolsSwitch::Skip,
                ZellijSwitch::Install,
                None,
            ))
        );
        assert!(words[2].contains("sudo hostname myws"));
        assert!(
            events.contains(&ProvisionEvent::ProvisioningDisabled {
                workspace: "myws".to_owned(),
            }),
            "{events:?}"
        );
        // And nothing about zellij, because the pass never carried that stage.
        assert!(
            !events.iter().any(|event| matches!(
                event,
                ProvisionEvent::StageFailed { stage, .. }
                | ProvisionEvent::StageNotReported { stage, .. }
                    if *stage == ZELLIJ_STAGE.as_str()
            )),
            "{events:?}"
        );
    }

    #[test]
    fn falsey_opt_out_values_leave_provisioning_on() {
        for value in ["", "0", "false", "no", "NO", " no "] {
            assert_eq!(
                ToolsSwitch::requested(Some(value)),
                ToolsSwitch::Install,
                "{value:?}"
            );
        }
        assert_eq!(ToolsSwitch::requested(None), ToolsSwitch::Install);
        for value in ["1", "true", "yes", "anything"] {
            assert_eq!(
                ToolsSwitch::requested(Some(value)),
                ToolsSwitch::Skip,
                "{value:?}"
            );
        }
    }

    // =======================================================================
    // DEVLAUNCH_NO_ZELLIJ: the narrower opt-out
    // =======================================================================

    // ------------------------------------------------------- the title stage

    #[test]
    fn a_title_puts_a_stage_in_the_pass_and_no_title_leaves_it_out() {
        // The stage exists only when there is a name to install, so a host that
        // turned titles off pays no part of it — not the append, and not the line in
        // the profile that a later launch would have to reason about.
        let named: Vec<StageName> = setup_stages(
            "myws",
            ToolsSwitch::Install,
            ZellijSwitch::Install,
            Some("blooop/devlaunch@main"),
        )
        .iter()
        .map(|stage| stage.name)
        .collect();
        let unnamed: Vec<StageName> =
            setup_stages("myws", ToolsSwitch::Install, ZellijSwitch::Install, None)
                .iter()
                .map(|stage| stage.name)
                .collect();

        assert!(named.contains(&TITLE_STAGE), "{named:?}");
        assert!(!unnamed.contains(&TITLE_STAGE), "{unnamed:?}");
    }

    #[test]
    fn neither_tools_switch_touches_the_title_stage() {
        // The same line the hostname stage is on: naming a pane is not tool
        // provisioning, so a host that installs nothing still gets its tabs named.
        // `DEVLAUNCH_NO_TITLE` is the one variable that stops it.
        for (tools, zellij) in [
            (ToolsSwitch::Skip, ZellijSwitch::Skip),
            (ToolsSwitch::Skip, ZellijSwitch::Install),
            (ToolsSwitch::Install, ZellijSwitch::Skip),
        ] {
            let names: Vec<StageName> =
                setup_stages("myws", tools, zellij, Some("blooop/devlaunch@main"))
                    .iter()
                    .map(|stage| stage.name)
                    .collect();

            assert!(
                names.contains(&TITLE_STAGE),
                "{tools:?} {zellij:?} {names:?}"
            );
        }
    }

    #[test]
    fn the_title_line_appends_to_ps1_so_it_is_the_last_write_of_every_prompt() {
        // The mechanism, spelled out because it is the whole reason this works.
        // Ubuntu's stock `~/.bashrc` puts `\e]0;\u@\h: \w\a` at the *front* of PS1,
        // so a prompt renames the pane after the hostname -- which is the workspace
        // id, the thing the spec is here to replace. Two escapes in one prompt are
        // applied in order, so the last one wins and this one has to be appended.
        //
        // A `PROMPT_COMMAND` would lose: bash runs that before it prints PS1, so the
        // stock escape would come afterwards.
        let line = profile_title_line("blooop/devlaunch@main");

        assert_eq!(
            line,
            r#"case $- in *i*) [ -n "$BASH_VERSION" ] && PS1="$PS1\[\e]2;"blooop/devlaunch@main"\a\]" ;; esac"#
        );
        // `$PS1` first, so nothing an image or a dotfile put in the prompt is
        // rewritten -- only added to.
        assert!(line.contains(r#"PS1="$PS1"#), "{line}");
        // And it is inert in the shells that render no prompt, which is every
        // `dl <ws> -- cmd`: `bash -lc` reads the profile too.
        assert!(line.starts_with("case $- in *i*)"), "{line}");
    }

    #[test]
    fn a_title_holding_shell_metacharacters_is_text_and_not_shell() {
        // A spec cannot hold these -- `WorkspaceId` refused every character but word
        // ones, dots, slashes and dashes -- but the other three placements title
        // after a bare devpod name or a path leaf, which this crate never validated.
        // So the name is its own quoted word rather than interpolated into the
        // double-quoted assignment.
        let line = profile_title_line("$(touch /tmp/pwned)`id`'x");

        assert!(
            line.contains(r#"'$(touch /tmp/pwned)`id`'"'"'x'"#),
            "{line}"
        );
    }

    #[test]
    fn the_title_stage_edits_the_profile_a_login_shell_will_actually_read() {
        // The append rides the same `$PROFILE` resolution the PATH writers use, so
        // all of them edit the one file bash reads and find each other's dedupe
        // marks there. A stage that picked `~/.profile` in an image shipping a
        // `~/.bash_profile` would write to a file nothing sources -- the failure the
        // resolution exists for -- and would do it silently.
        let stages = setup_stages(
            "myws",
            ToolsSwitch::Install,
            ZellijSwitch::Install,
            Some("blooop/devlaunch@main"),
        );
        let stage = stages
            .iter()
            .find(|stage| stage.name == TITLE_STAGE)
            .expect("the title stage");

        assert!(stage.command.contains(".bash_profile"), "{}", stage.command);
        assert!(stage.command.contains(PROFILE_MARK), "{}", stage.command);
    }

    #[test]
    fn a_real_bash_over_the_title_stage_leaves_a_profile_that_titles_the_pane() {
        // The stage's effect rather than its text: run the composed script with a
        // real bash over a real HOME, then source what it wrote and read PS1 back.
        // What this pins is the ordering the whole feature rests on -- our OSC 2 ends
        // up *after* the stock title escape, so it is the write that stands.
        let scratch = scratch();
        let home = scratch.path();
        let stages = setup_stages(
            "myws",
            ToolsSwitch::Skip,
            ZellijSwitch::Skip,
            Some("blooop/devlaunch@main"),
        );
        let stage = stages
            .iter()
            .find(|stage| stage.name == TITLE_STAGE)
            .expect("the title stage");

        let install = std::process::Command::new("bash")
            .args(["-c", &stage.command])
            .env("HOME", home)
            .output()
            .expect("bash to run the stage");
        assert!(install.status.success(), "{install:?}");

        // Ubuntu's stock interactive PS1, then the profile the stage just wrote.
        let read_back = std::process::Command::new("bash")
            .args([
                // `-i` and not `set -i`: the line guards on `$-` holding an `i`,
                // which only a shell *started* interactive has. That guard is the
                // feature -- it keeps the edit out of every `bash -lc` one-shot -- so
                // a test that sidestepped it would be pinning a different line.
                "-i",
                "-c",
                r#"PS1='\[\e]0;\u@\h: \w\a\]\u@\h:\w\$ '; . "$HOME/.profile"; printf '%s' "$PS1""#,
            ])
            .env("HOME", home)
            .output()
            .expect("bash to read the profile back");
        let ps1 = String::from_utf8_lossy(&read_back.stdout).to_string();

        let stock = ps1.find(r"\e]0;").expect("the stock title escape");
        let ours = ps1.find(r"\e]2;").expect("our title escape");
        assert!(ours > stock, "ours must come last: {ps1:?}");
        assert!(ps1.contains("blooop/devlaunch@main"), "{ps1:?}");

        // Appended once, however many times the pass runs: the dedupe mark is what
        // keeps a profile from growing one escape per launch.
        let again = std::process::Command::new("bash")
            .args(["-c", &stage.command])
            .env("HOME", home)
            .output()
            .expect("bash to run the stage again");
        assert!(again.status.success(), "{again:?}");
        let profile = std::fs::read_to_string(home.join(".profile")).expect("the profile");
        assert_eq!(profile.matches(r"\e]2;").count(), 1, "{profile:?}");
    }

    #[test]
    fn a_shell_that_is_not_bash_leaves_its_prompt_alone() {
        // `$PROFILE` is one of `~/.bash_profile`, `~/.bash_login` or `~/.profile`,
        // and the last of those is read by any POSIX login shell -- `/bin/sh` is
        // dash on Debian and Ubuntu. `\[`, `\e` and `\a` mean nothing to dash, which
        // renders a prompt literally, so an unguarded append puts
        // `\[\e]2;blooop/devlaunch@main\a\]` on screen at every prompt. That is worse
        // than no title: it is a corrupted one.
        //
        // The guard is the same `$BASH_VERSION` test Ubuntu's own `~/.profile` uses
        // before it sources `~/.bashrc`. Asserted as an implication so this reads the
        // same whichever shell `sh` is on the machine running it.
        let line = profile_title_line("blooop/devlaunch@main");
        let script = format!(
            "PS1=untouched\n{line}\nprintf '%s\\n%s\\n' \"${{BASH_VERSION:+bash}}\" \"$PS1\""
        );

        let out = std::process::Command::new("sh")
            .args(["-i", "-c", &script])
            .output()
            .expect("sh to run the line");
        let said = String::from_utf8_lossy(&out.stdout).to_string();
        let mut lines = said.lines();
        let is_bash = lines.next() == Some("bash");
        let ps1 = lines.next().expect("the prompt back");

        if is_bash {
            assert_ne!(
                ps1, "untouched",
                "bash should have been appended to: {said:?}"
            );
        } else {
            assert_eq!(ps1, "untouched", "a non-bash prompt was edited: {said:?}");
        }
    }
    #[test]
    fn the_zellij_opt_out_drops_only_the_zellij_stage() {
        // The whole reason for a second variable: a host that wants no zellij
        // installed keeps everything `DEVLAUNCH_NO_TOOLS` would have cost it —
        // the container is still named, and the pass still probes for the `gh`
        // and `claude` the workspace is guaranteed.
        let names: Vec<StageName> =
            setup_stages("myws", ToolsSwitch::Install, ZellijSwitch::Skip, None)
                .iter()
                .map(|stage| stage.name)
                .collect();

        assert!(!names.contains(&ZELLIJ_STAGE), "{names:?}");
        assert!(names.contains(&HOSTNAME_STAGE), "{names:?}");

        // And really nothing is installed: a real bash over the composed script
        // never reaches pixi.
        let scratch = scratch();
        let (_, _, calls) = run_zellij_pass(
            scratch.path(),
            Switches {
                tools: ToolsSwitch::Install,
                zellij: ZellijSwitch::Skip,
            },
            false,
            Some(0),
        );
        assert!(!calls.contains("pixi"), "{calls}");
    }

    #[test]
    fn either_opt_out_composes_the_same_pass() {
        // The two switches are an `and`, so the three ways to be without the stage
        // have to be one script and not three: whichever variable a host set, the
        // container is asked for exactly the same bytes. A golden of the *pair*
        // rather than of a rendering, because what is at stake is that these agree
        // rather than what they say — the bytes themselves are pinned against
        // Python's in `the_setup_pass_is_the_script_python_composes`.
        let without = setup_script(&setup_stages(
            "myws",
            ToolsSwitch::Install,
            ZellijSwitch::Skip,
            None,
        ));
        for tools in [ToolsSwitch::Install, ToolsSwitch::Skip] {
            assert_eq!(
                setup_script(&setup_stages("myws", tools, ZellijSwitch::Skip, None)),
                without,
                "{tools:?}"
            );
        }
        assert_eq!(
            setup_script(&setup_stages(
                "myws",
                ToolsSwitch::Skip,
                ZellijSwitch::Install,
                None,
            )),
            without
        );
        assert_ne!(
            setup_script(&setup_stages(
                "myws",
                ToolsSwitch::Install,
                ZellijSwitch::Install,
                None,
            )),
            without,
            "the stage is there when nothing asked for it to go"
        );
    }

    #[test]
    fn the_zellij_opt_out_still_probes_and_lends() {
        // The switch is about one stage, not about the flow: the pass travels, the
        // probe answers, and a lendable container is still lent the host's own
        // binaries. `DEVLAUNCH_NO_TOOLS` is the switch that stops that, and this is
        // deliberately not it.
        let scratch = scratch();
        let host = a_host_that_can_lend(scratch.path());
        let runner = Trips::new(&[0, 0]).reporting(REPORT_LENDABLE);

        let (outcome, _) = events_of(
            &runner,
            Switches {
                tools: ToolsSwitch::Install,
                zellij: ZellijSwitch::Skip,
            },
            &host,
        );

        let outcome = outcome.expect("devpod answered");
        assert_eq!(outcome, Provisioning::Lent);
        assert!(outcome.tools_present());
        assert_eq!(runner.count(), 2, "the pass, then the transfer");
        let words = shlex::split(&runner.script(0)).expect("a readable payload");
        assert!(words[2].contains("sudo hostname myws"));
        assert!(!words[2].contains(ZELLIJ_TOOL.command), "{}", words[2]);
    }

    #[test]
    fn falsey_zellij_opt_out_values_leave_the_stage_on() {
        // The same list `DEVLAUNCH_NO_TOOLS` reads, through the same parse: two
        // opt-outs a user spells the same way have to answer the same way, and
        // `DEVLAUNCH_NO_ZELLIJ=0` meaning *skip it* where `DEVLAUNCH_NO_TOOLS=0`
        // means *no* is exactly the surprise sharing the parse prevents.
        for value in ["", "0", "false", "no", "NO", " no "] {
            assert_eq!(
                ZellijSwitch::requested(Some(value)),
                ZellijSwitch::Install,
                "{value:?}"
            );
        }
        assert_eq!(ZellijSwitch::requested(None), ZellijSwitch::Install);
        for value in ["1", "true", "yes", "anything"] {
            assert_eq!(
                ZellijSwitch::requested(Some(value)),
                ZellijSwitch::Skip,
                "{value:?}"
            );
        }
    }

    // =======================================================================
    // the trip a remembered verdict saves
    // =======================================================================
    //
    // What [`verdict_cache`]'s own tests pin is when a marker is trusted. What
    // these pin is what the *flow* does about it: which occasion consults one,
    // which outcome writes one, and — the number that is the whole point — how
    // many round trips each of those costs.

    /// A devpod home, a cache, and the verdict cache over the two of them.
    struct Remembering {
        devpod_home: tempfile::TempDir,
        cache: tempfile::TempDir,
    }

    impl Remembering {
        /// One workspace, created and finished, as devpod's records leave it.
        fn new() -> Self {
            Self {
                devpod_home: crate::flows::lifecycle::tests::devpod_home_with(&[(
                    "default",
                    "myws",
                    Some(()),
                )]),
                cache: tempfile::tempdir().expect("a scratch cache directory"),
            }
        }

        fn verdicts(&self) -> VerdictCache {
            VerdictCache::under(
                self.cache.path(),
                Some(self.devpod_home.path().to_path_buf()),
            )
        }

        /// devpod completing another `up`, which is a rewritten result file.
        fn brought_up_again(&self) {
            let result = self
                .devpod_home
                .path()
                .join("contexts/default/workspaces/myws/workspace_result.json");
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&result)
                .expect("the result file");
            let was = file
                .metadata()
                .expect("its metadata")
                .modified()
                .expect("an mtime");
            file.set_modified(was + std::time::Duration::from_secs(30))
                .expect("a moved mtime");
        }
    }

    /// The whole flow, with an occasion and a host-side memory behind it.
    fn pass_of(
        runner: &Trips,
        occasion: PassOccasion,
        verdicts: &VerdictCache,
    ) -> Result<Provisioning, DevpodMissing> {
        provision_tools(
            runner,
            "myws",
            occasion,
            Switches::INSTALLING,
            None,
            Some(&nothing_to_lend()),
            Some(verdicts),
            &mut Vec::new(),
        )
    }

    #[test]
    fn a_top_up_with_a_trusted_verdict_makes_no_trip() {
        // The saving, stated as the only thing that can be measured about it: zero
        // round trips. A pass that answered from the cache but still opened the
        // `devpod ssh` channel would save nothing at all — the trip is ~99%
        // connection and process setup (#157), not the script it carries.
        let remembering = Remembering::new();
        let verdicts = remembering.verdicts();
        let first = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        assert_eq!(
            pass_of(&first, PassOccasion::TopUp, &verdicts),
            Ok(Provisioning::AlreadyProvisioned),
            "the first top-up has nothing remembered and pays for the answer"
        );
        assert_eq!(first.count(), 1);

        let again = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        let outcome = pass_of(&again, PassOccasion::TopUp, &verdicts);

        let outcome = outcome.expect("no devpod was needed");
        assert_eq!(outcome, Provisioning::CachedProvisioned);
        assert!(outcome.tools_present());
        assert_eq!(again.count(), 0, "nothing was asked of devpod");
    }

    #[test]
    fn an_after_up_pass_travels_even_with_a_trusted_verdict() {
        // The honest half of the scope. `sudo hostname` is a stage of the pass and
        // the name lives in the container's UTS namespace, which docker rebuilds
        // from the container's config on every start — so the pass after an `up`
        // has work to do that no verdict about the *tools* can excuse it from.
        let remembering = Remembering::new();
        let verdicts = remembering.verdicts();
        let first = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        pass_of(&first, PassOccasion::TopUp, &verdicts).expect("devpod answered");

        let after_up = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        let outcome = pass_of(&after_up, PassOccasion::AfterUp, &verdicts);

        assert_eq!(outcome, Ok(Provisioning::AlreadyProvisioned));
        assert_eq!(after_up.count(), 1);
        let words = shlex::split(&after_up.script(0)).expect("a readable payload");
        assert!(words[2].contains("sudo hostname myws"), "{}", words[2]);
    }

    #[test]
    fn a_rebuilt_container_invalidates_the_verdict() {
        // A `devpod up` by anything — this build, VS Code, a hand-typed one, a
        // `--recreate` — rewrites `workspace_result.json`, and that is the whole of
        // the invalidation. The next top-up finds the marker no longer describing
        // the container standing now and pays for a fresh answer.
        let remembering = Remembering::new();
        let verdicts = remembering.verdicts();
        let first = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        pass_of(&first, PassOccasion::TopUp, &verdicts).expect("devpod answered");

        remembering.brought_up_again();

        let after = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        let outcome = pass_of(&after, PassOccasion::TopUp, &verdicts);

        assert_eq!(outcome, Ok(Provisioning::AlreadyProvisioned));
        assert_eq!(after.count(), 1, "the pass travelled again");
    }

    /// Somebody else's completed `devpod up`, landing while the pass is in flight.
    ///
    /// Not hypothetical and not excluded by any lock devlaunch holds: VS Code, a
    /// hand-typed `devpod up`, a sibling `dl <ws> recreate` — none of them take the
    /// launch lock, and the `dl <ws> up` top-up that reads this cache does not hold
    /// it either.
    struct Rebuilding<'a> {
        trips: Trips,
        rebuild: &'a Remembering,
    }

    impl Runner for Rebuilding<'_> {
        fn capture(&self, spec: &SpawnSpec) -> Outcome<CapturedText> {
            self.rebuild.brought_up_again();
            self.trips.capture(spec)
        }

        fn passthrough(&self, spec: &SpawnSpec) -> Outcome {
            self.trips.passthrough(spec)
        }

        fn session(&self, _spec: &SpawnSpec, _on_stderr_line: &mut dyn FnMut(&str)) -> Outcome {
            panic!("provisioning never opens a session")
        }

        fn detach(&self, _what: &Invocation) -> DetachOutcome {
            panic!("provisioning never detaches")
        }
    }

    #[test]
    fn a_rebuild_during_the_pass_leaves_no_verdict_to_trust() {
        // The container the probe spoke to and the container standing when the
        // marker is written are not always the same one. Read the anchor *after*
        // the pass and the marker records the new container's mtime against the old
        // container's verdict -- a marker that matches for as long as nothing else
        // rebuilds, which is the one misreading this whole module exists to refuse.
        let remembering = Remembering::new();
        let verdicts = remembering.verdicts();
        let runner = Rebuilding {
            trips: Trips::new(&[0]).reporting(REPORT_PROVISIONED),
            rebuild: &remembering,
        };

        let outcome = provision_tools(
            &runner,
            "myws",
            PassOccasion::TopUp,
            Switches::INSTALLING,
            None,
            Some(&nothing_to_lend()),
            Some(&verdicts),
            &mut Vec::new(),
        );
        assert_eq!(outcome, Ok(Provisioning::AlreadyProvisioned));

        let next = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        assert_eq!(
            pass_of(&next, PassOccasion::TopUp, &verdicts),
            Ok(Provisioning::AlreadyProvisioned)
        );
        assert_eq!(
            next.count(),
            1,
            "the verdict was about a container that is no longer standing"
        );
    }

    #[test]
    fn only_a_provisioned_pass_writes_the_verdict() {
        // A lend, an install and a kept shim are all passes on which the container
        // just changed, and `ShimKept` is a documented residual that re-attempts a
        // failing transfer on every `up` — a marker would silently turn that into
        // never attempting again. Each of them is followed by a later pass that
        // probes provisioned, and *that* pass is what records.
        for (report, exits, expected) in [
            (REPORT_LENDABLE, &[0, 0][..], Provisioning::ShimKept),
            (REPORT_ABSENT, &[0, 0][..], Provisioning::Installed),
            (
                REPORT_ABSENT,
                &[0, 1][..],
                Provisioning::InstallRefused {
                    exit: Exit::Code(1),
                },
            ),
        ] {
            let remembering = Remembering::new();
            let verdicts = remembering.verdicts();
            let runner = Trips::new(exits).reporting(report);

            let outcome = pass_of(&runner, PassOccasion::TopUp, &verdicts);

            assert_eq!(outcome, Ok(expected.clone()), "{report:?}");
            let next = Trips::new(exits).reporting(report);
            assert_eq!(
                pass_of(&next, PassOccasion::TopUp, &verdicts),
                Ok(expected),
                "{report:?}"
            );
            assert!(
                next.count() > 0,
                "the next top-up still travelled: {report:?}"
            );
        }

        // And the one that does record, for contrast — the same shape, one arm
        // different, so the loop above is a pin on the *split* and not just on
        // three arms happening not to write.
        let remembering = Remembering::new();
        let verdicts = remembering.verdicts();
        let runner = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        assert_eq!(
            pass_of(&runner, PassOccasion::TopUp, &verdicts),
            Ok(Provisioning::AlreadyProvisioned)
        );
        let next = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        assert_eq!(
            pass_of(&next, PassOccasion::TopUp, &verdicts),
            Ok(Provisioning::CachedProvisioned)
        );
        assert_eq!(next.count(), 0);
    }

    #[test]
    fn a_caller_that_remembers_nothing_pays_for_every_answer() {
        // `None` is the shape core's own launch tests and any embedder without a
        // cache directory use, and it has to be exactly the behaviour that shipped
        // before the cache existed: every pass travels, on every occasion.
        for occasion in [PassOccasion::AfterUp, PassOccasion::TopUp] {
            let runner = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
            let outcome = provision_tools(
                &runner,
                "myws",
                occasion,
                Switches::INSTALLING,
                None,
                Some(&nothing_to_lend()),
                None,
                &mut Vec::new(),
            );
            assert_eq!(
                outcome,
                Ok(Provisioning::AlreadyProvisioned),
                "{occasion:?}"
            );
            assert_eq!(runner.count(), 1, "{occasion:?}");
        }
    }

    #[test]
    fn a_verdict_is_never_trusted_over_a_container_the_host_cannot_identify() {
        // No devpod home is no `workspace_result.json` to key on, and the cache
        // then trusts nothing rather than trusting everything. Worth its own test
        // because it is the arm a `None` would most plausibly be written to mean
        // "no constraint".
        let cache = tempfile::tempdir().expect("a scratch cache directory");
        let verdicts = VerdictCache::under(cache.path(), None);
        let first = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        pass_of(&first, PassOccasion::TopUp, &verdicts).expect("devpod answered");

        let again = Trips::new(&[0]).reporting(REPORT_PROVISIONED);
        let outcome = pass_of(&again, PassOccasion::TopUp, &verdicts);

        assert_eq!(outcome, Ok(Provisioning::AlreadyProvisioned));
        assert_eq!(again.count(), 1);
    }

    // =======================================================================
    // TestTheHostNamesEveryStageThatIsNotOk
    // =======================================================================

    /// Every event about the hostname stage from one run.
    fn hostname_events(stdout: &str, exit: i32) -> Vec<ProvisionEvent> {
        let runner = Trips::new(&[exit, 0]).reporting(stdout);
        let (_, events) = events_of(&runner, Switches::INSTALLING, &nothing_to_lend());
        events
            .into_iter()
            .filter(|event| match event {
                ProvisionEvent::StageFailed { stage, .. }
                | ProvisionEvent::StageNotReported { stage, .. } => {
                    *stage == HOSTNAME_STAGE.as_str()
                }
                _ => false,
            })
            .collect()
    }

    #[test]
    fn a_failing_stage_is_named_with_its_status() {
        let reported = hostname_events(&format!("{STAGE_FAILED_LINE}{REPORT_ABSENT}"), 0);

        // Info, not warning, and only for this stage: `sudo hostname` cannot succeed
        // without CAP_SYS_ADMIN, which Docker drops by default, so a warning here
        // would fire on the majority of cold launches.
        assert_eq!(
            reported,
            vec![ProvisionEvent::StageFailed {
                workspace: "myws".to_owned(),
                stage: HOSTNAME_STAGE.as_str(),
                status: 1,
                loudness: FailureLevel::Info,
            }]
        );
    }

    #[test]
    fn a_stage_that_worked_is_not_reported_at_all() {
        assert_eq!(
            hostname_events(&format!("{STAGE_OK_LINE}{REPORT_PROVISIONED}"), 0),
            vec![]
        );
    }

    #[test]
    fn a_stage_that_was_never_reached_is_named_too() {
        // The state that used to be unrepresentable. A report with no line for the
        // stage is not the stage working.
        assert_eq!(
            hostname_events(REPORT_PROVISIONED, 0),
            vec![ProvisionEvent::StageNotReported {
                workspace: "myws".to_owned(),
                stage: HOSTNAME_STAGE.as_str(),
                loudness: FailureLevel::Info,
            }]
        );
    }

    #[test]
    fn a_trip_that_never_got_through_reports_no_stage_as_ok() {
        // A non-zero `devpod ssh` is the transport, not a stage — so nothing may be
        // read as having run.
        assert_eq!(hostname_events("", 1).len(), 1);
    }

    #[test]
    fn a_stage_warns_by_name_unless_it_asks_to_be_quieter() {
        // Warning is the default a new stage gets: naming a stage that did not work
        // is the whole point of the composition, and the hostname's info level is an
        // exception it has to declare, measured (#167), rather than the rule.
        assert_eq!(
            Stage::new(StageName::new("x"), "true").failure_level,
            FailureLevel::Warning
        );
        let levels: Vec<(StageName, FailureLevel)> =
            setup_stages("myws", ToolsSwitch::Install, ZellijSwitch::Install, None)
                .iter()
                .map(|stage| (stage.name, stage.failure_level))
                .collect();
        assert_eq!(
            levels,
            vec![
                (HOSTNAME_STAGE, FailureLevel::Info),
                // A zellij that would not install is a real, invisible and permanent
                // degradation of a container, so it takes the default rather than
                // declaring an exception.
                (ZELLIJ_STAGE, FailureLevel::Warning),
            ]
        );
    }

    // =======================================================================
    // TestHostPayload
    // =======================================================================

    #[test]
    fn the_official_claude_layout_is_lent() {
        let scratch = scratch();
        let home = scratch.path().join("home");
        official_claude(&home, "2.0.1");
        let gh = home.join("bin/gh");
        write_program(&gh, "#!/bin/sh\nexit 0\n");

        let payload = host_payload(&HostLayout {
            home: home.clone(),
            gh_on_path: Some(gh.clone()),
        })
        .expect("a host that can lend");

        assert_eq!(payload.claude_version, "2.0.1");
        assert_eq!(
            payload.members(),
            vec![
                (
                    home.join(CLAUDE_VERSIONS_RELPATH).join("2.0.1").as_path(),
                    ".local/share/claude/versions/2.0.1".to_owned(),
                ),
                (gh.as_path(), ".local/bin/gh".to_owned()),
            ]
        );
    }

    #[test]
    fn the_lent_paths_are_all_home_relative() {
        // They are tar arcnames: an absolute one would unpack into the host's
        // usernamed home, which does not exist in the container.
        let scratch = scratch();
        let payload = host_payload(&a_host_that_can_lend(scratch.path())).expect("a payload");
        for (_source, arcname) in payload.members() {
            assert!(!arcname.starts_with('/'), "{arcname}");
        }
    }

    #[test]
    fn a_claude_that_is_not_the_official_install_is_not_lent() {
        // A shim or wrapper on PATH is the downloader this transfer exists to skip —
        // lending it would lend the download.
        let scratch = scratch();
        let home = a_home(scratch.path());
        claude_shim(&home);
        let gh = home.join("bin/gh");
        write_program(&gh, "#!/bin/sh\nexit 0\n");

        assert_eq!(
            host_payload(&HostLayout {
                home,
                gh_on_path: Some(gh),
            }),
            None
        );
    }

    #[test]
    fn a_pixi_trampoline_lends_the_binary_it_names() {
        // The trampoline is a launcher that re-execs the env's binary named in a JSON
        // file beside it; copied alone it launches nothing.
        let scratch = scratch();
        let real = scratch.path().join("envs/gh/bin/gh");
        write_program(&real, "\x7fELF");
        let trampoline = scratch.path().join("pixi-bin/gh");
        write_program(&trampoline, "\x7fELF");
        fs::create_dir_all(scratch.path().join("pixi-bin/trampoline_configuration"))
            .expect("a sidecar directory");
        fs::write(
            scratch
                .path()
                .join("pixi-bin/trampoline_configuration/gh.json"),
            format!("{{\"exe\": \"{}\"}}", real.display()),
        )
        .expect("a sidecar");
        let home = a_home(scratch.path());
        official_claude(&home, "2.0.1");

        let payload = host_payload(&HostLayout {
            home,
            gh_on_path: Some(trampoline),
        })
        .expect("a payload");

        assert_eq!(payload.gh_binary, real);
    }

    #[test]
    fn an_unreadable_trampoline_lends_nothing() {
        // Shipping the launcher without the binary it launches ships a break.
        let scratch = scratch();
        let trampoline = scratch.path().join("pixi-bin/gh");
        write_program(&trampoline, "\x7fELF");
        fs::create_dir_all(scratch.path().join("pixi-bin/trampoline_configuration"))
            .expect("a sidecar directory");
        fs::write(
            scratch
                .path()
                .join("pixi-bin/trampoline_configuration/gh.json"),
            "not json",
        )
        .expect("a sidecar");
        let home = a_home(scratch.path());
        official_claude(&home, "2.0.1");

        assert_eq!(
            host_payload(&HostLayout {
                home,
                gh_on_path: Some(trampoline),
            }),
            None
        );
        // And a sidecar that is JSON without the key it needs, which Python's
        // `["exe"]` raises for.
        fs::write(
            scratch
                .path()
                .join("pixi-bin/trampoline_configuration/gh.json"),
            "{\"something\": 1}",
        )
        .expect("a sidecar");
        assert_eq!(gh_source(Some(&scratch.path().join("pixi-bin/gh"))), None);
    }

    #[test]
    fn the_payload_is_all_or_nothing() {
        // A host missing either tool falls back to the network for both, rather than
        // growing half-lent states the fallback must reason about.
        let scratch = scratch();
        let home = a_home(scratch.path());
        official_claude(&home, "2.0.1");

        assert_eq!(
            host_payload(&HostLayout {
                home: home.clone(),
                gh_on_path: None,
            }),
            None
        );
        // And the other way round: a gh with no claude to go with it.
        let gh = home.join("bin/gh");
        write_program(&gh, "#!/bin/sh\nexit 0\n");
        assert_eq!(
            host_payload(&HostLayout {
                home: a_home(&scratch.path().join("empty")),
                gh_on_path: Some(gh),
            }),
            None
        );
    }

    #[test]
    fn a_claude_link_that_points_nowhere_is_nothing_to_lend() {
        // Python resolves the link with `strict=True`, so a dangling link is not a
        // source at all.
        let scratch = scratch();
        let home = a_home(scratch.path());
        let link = home.join(".local/bin/claude");
        fs::create_dir_all(link.parent().expect("a parent")).expect("a bin directory");
        std::os::unix::fs::symlink(home.join("gone"), &link).expect("a dangling link");

        assert_eq!(claude_source(&home), None);
    }

    #[test]
    fn a_claude_that_cannot_be_executed_is_nothing_to_lend() {
        let scratch = scratch();
        let home = a_home(scratch.path());
        let binary = home.join(CLAUDE_VERSIONS_RELPATH).join("2.0.1");
        write_program(&binary, "#!/bin/sh\n");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o644)).expect("unexecutable");
        let link = home.join(".local/bin/claude");
        fs::create_dir_all(link.parent().expect("a parent")).expect("a bin directory");
        std::os::unix::fs::symlink(&binary, &link).expect("the installer's symlink");

        assert_eq!(claude_source(&home), None);
    }

    // =======================================================================
    // TestTransferScript
    // =======================================================================

    #[test]
    fn it_unpacks_into_home_and_links_the_current_version() {
        let script = transfer_script(&golden_payload());
        assert!(script.contains(r#"tar xf - -C "$STAGE""#));
        // The host's own symlink would point through the host's home, so the link is
        // made in the container, against the container's $HOME.
        assert!(script.contains(
            r#"ln -sfn "$HOME/.local/share/claude/versions/2.0.1" "$HOME/.local/bin/claude""#
        ));
    }

    #[test]
    fn nothing_leaves_the_staging_area_until_it_has_been_proved_to_run() {
        // The arch/libc gate has to come before the container is changed, not after.
        // Unpacking straight into $HOME meant a failed gate still left the PATH edit
        // and the `claude` symlink behind — and the network fallback that follows
        // decides what to install with `command -v`, which a broken binary satisfies.
        let script = transfer_script(&golden_payload());
        assert!(script.contains("set -eu"));
        let gate = script
            .find(r#""$STAGE/.local/share/claude/versions/2.0.1" --version"#)
            .expect("the claude gate");
        let moved = script.find("mv -f").expect("the moves");
        assert!(
            script
                .find(r#""$STAGE/.local/bin/gh" --version"#)
                .expect("the gh gate")
                < moved
        );
        assert!(gate < moved, "proved before anything is moved into place");
        assert!(
            gate < script.find("ln -sfn").expect("the link"),
            "proved before the symlink is made"
        );
        assert!(
            gate < script.find(r#">> "$PROFILE""#).expect("the PATH edit"),
            "proved before PATH is edited"
        );
    }

    #[test]
    fn a_failed_transfer_leaves_the_staging_area_behind_it() {
        // `set -eu` aborts wherever it fails, so the cleanup has to be a trap rather
        // than a last line — otherwise a gate failure strands a few hundred MB under
        // $HOME that nothing ever collects.
        let script = transfer_script(&golden_payload());
        assert!(script.contains(r#"trap 'rm -rf "$STAGE"' EXIT"#));
        assert!(script.find("trap").expect("the trap") < script.find("tar xf -").expect("the tar"));
    }

    #[test]
    fn the_transfers_progress_goes_to_stderr_not_stdout() {
        let script = transfer_script(&golden_payload());
        let redirect = script.find("exec >&2").expect("the redirect");
        assert!(redirect < script.find("echo").expect("something printed"));
    }

    #[test]
    fn the_stream_the_container_receives_is_that_tar() {
        // End to end: what `tar xf -` reads on the other side has to be a readable
        // archive of exactly the lent files, under the arcnames the link and the PATH
        // edit were written against.
        let scratch = scratch();
        let runner = Trips::new(&[1, 0]).reporting("");
        let host = a_host_that_can_lend(scratch.path());

        let (outcome, _) = events_of(&runner, Switches::INSTALLING, &host);
        assert_eq!(outcome, Ok(Provisioning::Lent));

        let streamed = runner.trips()[1]
            .stream
            .clone()
            .expect("the transfer trip streamed nothing");
        let bundle = scratch.path().join("streamed.tar");
        fs::write(&bundle, &streamed).expect("the streamed bytes");
        let listed = Command::new("tar")
            .arg("-tf")
            .arg(&bundle)
            .output()
            .expect("tar read it");
        assert!(
            listed.status.success(),
            "{}",
            String::from_utf8_lossy(&listed.stderr)
        );
        let mut names: Vec<String> = String::from_utf8_lossy(&listed.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                ".local/bin/gh".to_owned(),
                ".local/share/claude/versions/2.0.1".to_owned(),
            ]
        );
    }

    #[test]
    fn the_bundle_is_a_ustar_archive_of_exactly_the_lent_files() {
        // The header, checked without a tar binary: the name, the size, the type and
        // the magic are what every extractor reads, and the checksum is what makes it
        // read them at all.
        let scratch = scratch();
        let payload = fake_payload(scratch.path());
        let bundle = scratch.path().join("tools.tar");
        write_payload_tar(&payload, &bundle).expect("a bundle");

        let bytes = fs::read(&bundle).expect("the bundle");
        assert_eq!(bytes.len() % TAR_RECORD, 0, "padded to a whole record");
        let header = &bytes[..TAR_BLOCK];
        let name = String::from_utf8_lossy(&header[..100])
            .trim_end_matches('\0')
            .to_owned();
        assert_eq!(name, ".local/share/claude/versions/2.0.1");
        assert_eq!(&header[257..262], b"ustar");
        assert_eq!(header[156], b'0', "a regular file");
        let size = u64::from_str_radix(
            String::from_utf8_lossy(&header[124..135]).trim_end_matches('\0'),
            8,
        )
        .expect("an octal size");
        assert_eq!(size, 10, "the scratch claude is ten bytes");
        // The checksum, computed the way an extractor computes it.
        let claimed = u32::from_str_radix(
            String::from_utf8_lossy(&header[148..154]).trim_end_matches('\0'),
            8,
        )
        .expect("an octal checksum");
        let mut zeroed = header.to_vec();
        zeroed[148..156].fill(b' ');
        let sum: u32 = zeroed.iter().map(|byte| u32::from(*byte)).sum();
        assert_eq!(claimed, sum);
        // The second member starts on the next block boundary after ten bytes.
        let second = &bytes[TAR_BLOCK * 2..];
        assert!(
            String::from_utf8_lossy(&second[..100]).starts_with(".local/bin/gh"),
            "the gh header follows the claude data"
        );
    }

    #[test]
    fn a_member_that_is_not_there_is_a_bundle_failure_and_not_a_panic() {
        // This runs after a successful `devpod up`, so letting an io error out would
        // cost the user the workspace they just built over a convenience that is
        // allowed to fail.
        let scratch = scratch();
        let payload = HostPayload {
            claude_version: "2.0.1".to_owned(),
            claude_binary: scratch.path().join("gone"),
            gh_binary: scratch.path().join("gone-too"),
        };

        let failure = write_payload_tar(&payload, &scratch.path().join("tools.tar"))
            .expect_err("nothing to bundle");

        assert!(
            matches!(failure, BundleFailed::Unreadable { .. }),
            "{failure:?}"
        );
    }

    #[test]
    fn an_arcname_no_ustar_header_can_carry_is_refused() {
        // Python's tarfile would write a pax extended header for it; reaching this
        // needs a claude version whose name is some seventy characters long.
        let scratch = scratch();
        let claude = scratch.path().join("claude");
        fs::write(&claude, b"#!/bin/sh\n").expect("a scratch claude");
        let payload = HostPayload {
            claude_version: "v".repeat(80),
            claude_binary: claude,
            gh_binary: scratch.path().join("gh"),
        };

        let failure = write_payload_tar(&payload, &scratch.path().join("tools.tar"))
            .expect_err("no header can carry it");

        assert!(
            matches!(failure, BundleFailed::NameTooLong { .. }),
            "{failure:?}"
        );
    }

    // =======================================================================
    // TestNoRegressionInTheOptOutContract
    // =======================================================================

    #[test]
    fn the_opt_out_reads_the_way_the_gh_one_does() {
        // DEVLAUNCH_NO_TOOLS mirrors DEVLAUNCH_NO_GH_TOKEN, so it reads the same —
        // two copies of one convention, kept in step by this test rather than by a
        // shared constant neither module could then diverge from deliberately.
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("NO"),
            Some(" no "),
            Some("1"),
            Some("true"),
            Some("yes"),
            Some("anything"),
        ] {
            assert_eq!(
                provisioning_disabled(value),
                crate::clients::gh::forwarding_disabled(value),
                "{value:?}"
            );
        }
    }

    #[test]
    fn unset_means_enabled() {
        assert!(!provisioning_disabled(None));
    }

    // =======================================================================
    // TestZellijProvisioning
    // =======================================================================

    /// A scratch `$HOME` and a PATH holding only the fakes a case asks for.
    ///
    /// Never the host's real pixi, curl or zellij: this must not be able to reach the
    /// network, and must not be able to install anything on the machine running the
    /// tests.
    fn zellij_sandbox(
        scratch: &Path,
        has_zellij: bool,
        pixi_exit: Option<i32>,
    ) -> (PathBuf, PathBuf, PathBuf) {
        let home = a_home(scratch);
        let sysbin = sysbin(scratch, &["readlink", "grep", "bash"]);
        let log = scratch.join("calls.log");
        if has_zellij {
            fake_program(&sysbin, "zellij", 0, &log, None);
        }
        if let Some(status) = pixi_exit {
            // Noise on *stdout*, because that is the stream the pass's protocol
            // shares and the one a stage may never speak on.
            fake_program(&sysbin, "pixi", status, &log, Some("PIXI-NOISE"));
        }
        // A curl that cannot reach anything, which is what an offline image is.
        fake_program(&sysbin, "curl", 1, &log, None);
        (home, sysbin, log)
    }

    /// Run the whole setup pass for real and read it the way the host does.
    fn run_zellij_pass(
        scratch: &Path,
        switches: Switches,
        has_zellij: bool,
        pixi_exit: Option<i32>,
    ) -> (PathBuf, Output, String) {
        let (home, sysbin, log) = zellij_sandbox(scratch, has_zellij, pixi_exit);
        let ran = bash_with(
            &setup_script(&setup_stages("myws", switches.tools, switches.zellij, None)),
            &[
                ("HOME", &home.to_string_lossy()),
                ("PATH", &sysbin.to_string_lossy()),
            ],
        );
        // Exits 0 whatever the stages did: a non-zero `devpod ssh` has to keep
        // meaning the transport failed. A zellij that would not install may never be
        // able to change that.
        assert!(
            ran.status.success(),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        let calls = fs::read_to_string(&log).unwrap_or_default();
        (home, ran, calls)
    }

    fn zellij_outcome(report: &str) -> Option<StageOutcome> {
        outcome_of(report, ZELLIJ_STAGE)
    }

    fn stdout_of(ran: &Output) -> String {
        String::from_utf8_lossy(&ran.stdout).into_owned()
    }

    #[test]
    fn every_container_devlaunch_launches_is_asked_for_zellij() {
        // The guarantee, stated where it is made: the pass every entry into Running
        // goes through carries a zellij stage. No dotfiles, no devcontainer.json, no
        // repo cooperation — the ask comes from the invocation.
        let names: Vec<StageName> =
            setup_stages("myws", ToolsSwitch::Install, ZellijSwitch::Install, None)
                .iter()
                .map(|stage| stage.name)
                .collect();
        assert!(names.contains(&ZELLIJ_STAGE), "{names:?}");
    }

    #[test]
    fn it_is_never_a_required_tool() {
        // The regression this whole placement exists to avoid: REQUIRED_TOOLS is also
        // what the probe asks about and what the host lends, so a container answering
        // "missing" is lent the host's ~300MB claude — and the lend returns before the
        // install trip, so the container would pay the transfer and still have no
        // zellij.
        assert!(
            !REQUIRED_TOOLS
                .iter()
                .any(|tool| tool.command == ZELLIJ_TOOL.command)
        );
        assert!(!probe_script().contains(ZELLIJ_TOOL.command));
    }

    #[test]
    fn a_container_that_already_has_it_installs_nothing() {
        // Every launch after the first: one `command -v` and no pixi at all. The
        // check has to be this cheap because the stage runs on every entry into
        // Running, and it has to be made from the login shell the pass already runs
        // in — from anywhere without ~/.pixi/bin on PATH, an installed zellij looks
        // missing and is reinstalled on every launch.
        let scratch = scratch();
        let (home, ran, calls) = run_zellij_pass(scratch.path(), Switches::INSTALLING, true, None);

        assert_eq!(
            zellij_outcome(&stdout_of(&ran)),
            Some(StageOutcome {
                stage: ZELLIJ_STAGE,
                result: StageResult::Ok,
            })
        );
        assert!(!calls.contains("pixi"), "{calls}");
        assert!(!home.join(".profile").exists());
    }

    #[test]
    fn a_cold_container_gets_it_and_the_next_shell_can_find_it() {
        // Installed, and put on the *login* PATH so the next trip sees it. The second
        // half is not decoration: a lent container has only ~/.local/bin on its login
        // PATH, so without this edit a `pixi global install zellij` would land a
        // binary no later login shell could resolve, and the stage would reinstall it
        // forever.
        let scratch = scratch();
        let (home, ran, calls) =
            run_zellij_pass(scratch.path(), Switches::INSTALLING, false, Some(0));

        assert_eq!(
            zellij_outcome(&stdout_of(&ran)),
            Some(StageOutcome {
                stage: ZELLIJ_STAGE,
                result: StageResult::Ok,
            })
        );
        assert!(calls.contains("pixi global install zellij"), "{calls}");
        let profile = fs::read_to_string(home.join(".profile")).expect("an edited profile");
        assert!(profile.contains(PIXI_BIN_LINE), "{profile}");
    }

    #[test]
    fn an_install_that_fails_is_named_and_costs_the_launch_nothing() {
        // The install fails, the stage says so with its status, the probe still
        // answers in the same trip, and the pass still exits 0 — so the container
        // opens without zellij instead of not opening.
        let scratch = scratch();
        let (_, ran, calls) = run_zellij_pass(scratch.path(), Switches::INSTALLING, false, Some(1));

        let report = stdout_of(&ran);
        assert_eq!(
            zellij_outcome(&report),
            Some(StageOutcome {
                stage: ZELLIJ_STAGE,
                result: StageResult::Failed { status: 1 },
            })
        );
        assert!(calls.contains("pixi global install zellij"), "{calls}");
        assert_eq!(ProbeResult::parse(&report), ProbeResult::Absent);
    }

    #[test]
    fn an_image_with_no_pixi_and_no_network_still_launches() {
        // The worst case: nothing to install with and nothing to fetch it from. The
        // bootstrap's curl fails, there is no pixi behind it, and the stage reports
        // that — while the probe answers and the pass exits 0.
        let scratch = scratch();
        let (_, ran, _) = run_zellij_pass(scratch.path(), Switches::INSTALLING, false, None);

        let report = stdout_of(&ran);
        assert!(
            matches!(
                zellij_outcome(&report),
                Some(StageOutcome {
                    result: StageResult::Failed { .. },
                    ..
                })
            ),
            "{report:?}"
        );
        assert_eq!(ProbeResult::parse(&report), ProbeResult::Absent);
    }

    #[test]
    fn the_install_never_speaks_on_the_protocols_stdout() {
        // A stage shares the probe's stdout, and pixi is loud. The readers split
        // marked lines on spaces, so a package manager's progress on this stream is
        // one unlucky line away from being read as protocol. It also matters that the
        // noise goes to stderr rather than nowhere: the setup pass captures stdout, so
        // an install redirected into it would be invisible, and a cold launch that
        // looks hung is what these scripts print progress for.
        let scratch = scratch();
        let (_, ran, _) = run_zellij_pass(scratch.path(), Switches::INSTALLING, false, Some(0));

        assert!(!stdout_of(&ran).contains("PIXI-NOISE"));
        assert!(String::from_utf8_lossy(&ran.stderr).contains("PIXI-NOISE"));
    }

    #[test]
    fn the_tools_opt_out_asks_for_no_zellij() {
        // Installing zellij *is* tool provisioning, so the opt-out covers it. Unlike
        // the hostname stage, which is not tools work and is deliberately left
        // outside that switch — a machine that turned tool installs off has not
        // thereby asked for unnamed containers.
        let names: Vec<StageName> =
            setup_stages("myws", ToolsSwitch::Skip, ZellijSwitch::Install, None)
                .iter()
                .map(|stage| stage.name)
                .collect();
        assert!(!names.contains(&ZELLIJ_STAGE), "{names:?}");
        assert!(names.contains(&HOSTNAME_STAGE), "{names:?}");

        let scratch = scratch();
        let (_, _, calls) = run_zellij_pass(
            scratch.path(),
            Switches {
                tools: ToolsSwitch::Skip,
                zellij: ZellijSwitch::Install,
            },
            false,
            Some(0),
        );
        assert!(!calls.contains("pixi"), "{calls}");
    }
}
