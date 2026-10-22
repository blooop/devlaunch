//! The trust entry, written on the host.
//!
//! Claude Code asks "Do you trust the files in this folder?" until
//! `projects[<path>].hasTrustDialogAccepted` is true in its `.claude.json`. The
//! container's trust stage ([`super::trust_script`]) says which folder and which
//! config directory, and this module does the write, on the host.
//!
//! On the host because the write needs a JSON parser and the container may have
//! none: it used to be `python3` in the container, and the default image has no
//! `python3`, so every workspace opened from it asked again. And on the host because
//! the file it reaches is the host's: a container that shares the host's
//! `~/.claude` (devlaunch's own claude-code feature mounts it and pins
//! `CLAUDE_CONFIG_DIR` there) reads the host's `~/.claude/.claude.json`.
//!
//! The container ran a stranger's `postCreateCommand`, so both lines it prints are
//! untrusted. [`target`] is where the host decides what they may touch, from
//! `docker inspect` and devpod's own record of the `up`, never from the report alone.

use std::path::{Path, PathBuf};

use super::HostLayout;
use crate::clients::devpod_home::{DevpodHome, sole_workspace_result};
use crate::clients::docker;
use crate::domain::workspace_state::NonEmpty;
use crate::runner::Runner;

/// What the trust stage reported from inside the container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reported {
    /// The container's Claude config directory, resolved in the container.
    pub(crate) config_dir: String,
    /// The workspace folder the pass stood in, `pwd -P` in the container.
    pub(crate) workspace: String,
}

/// One mount, as `docker inspect` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Mount {
    pub(crate) bind: bool,
    pub(crate) source: PathBuf,
    pub(crate) destination: String,
    pub(crate) writable: bool,
}

/// What the host knows that the container cannot change.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HostSide<'a> {
    /// This machine's home directory.
    pub(crate) home: &'a Path,
    /// The directory this machine's Claude Code keeps its config in, if it names one.
    pub(crate) claude_dir: Option<&'a Path>,
    /// The clone devpod mounted as this workspace's folder.
    pub(crate) clone: &'a Path,
    /// The workspace folder in the container, as devpod recorded it at `up`.
    pub(crate) workspace_folder: &'a str,
}

/// The host config directory to merge into, resolved, and the key to merge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) dir: PathBuf,
    pub(crate) key: String,
}

/// Which host config directory to merge into, and which key, or `None` where the host has no
/// business writing anything.
///
/// The container says two things and the host believes neither of them on its own
/// word. `docker inspect` is what says where a container path lives on the host,
/// and the host's own facts say which host paths are fair game:
///
/// - **The file.** The reported config directory must be exactly the destination of
///   a read-write bind mount whose host source is this machine's Claude config
///   directory (`$CLAUDE_CONFIG_DIR`) or `~/.claude` (what devlaunch's claude-code
///   feature mounts). Anything else is the container's own directory, which the seed
///   in the container already wrote, or somebody else's host directory, which is not
///   dl's to write. A further mount over the file itself makes the source directory
///   the wrong file, so that is refused too. The directory is the mount's source
///   with its links resolved, and [`merge`] follows no link out of it.
/// - **The key.** The reported path must be spelled the one way Claude Code keys it
///   (absolute, no `.`, `..`, empty or trailing component, and only
///   `[-a-zA-Z0-9_/.@+]`), and it must be the workspace folder devpod recorded at
///   `up`, which must itself be the mount point of the clone dl made for this
///   workspace or a directory under it. A sub-folder the pass stood in is not
///   enough: a login profile can `cd` anywhere. Neither that mount point nor the
///   path may be the host's home, inside it, or above it: a repo picks its own
///   `workspaceMount` target, and a key there would name a real host directory the
///   host's own Claude Code would then stop asking about. `/` is above everything.
pub(crate) fn target(reported: &Reported, mounts: &[Mount], host: HostSide<'_>) -> Option<Target> {
    let dir = config_dir(&reported.config_dir, mounts, host)?;
    let key = trusted_key(&reported.workspace, mounts, host)?;
    Some(Target { dir, key })
}

/// The host directory behind the container's config directory, resolved, if it is
/// one of the host's own Claude config directories mounted read-write.
fn config_dir(config_dir: &str, mounts: &[Mount], host: HostSide<'_>) -> Option<PathBuf> {
    if !is_plain_absolute(config_dir) {
        return None;
    }
    let inner_file = format!("{config_dir}/{CONFIG_FILE}");
    if mounts.iter().any(|mount| mount.destination == inner_file) {
        return None;
    }
    let allowed: Vec<PathBuf> = std::iter::once(host.home.join(super::CLAUDE_CONFIG_RELPATH))
        .chain(host.claude_dir.map(Path::to_path_buf))
        .map(|dir| resolved(&dir))
        .collect();
    mounts
        .iter()
        .filter(|mount| mount.bind && mount.writable && mount.destination == config_dir)
        .map(|mount| resolved(&mount.source))
        .find(|source| allowed.contains(source))
}

/// The reported workspace path, if the host agrees it is this workspace's folder.
fn trusted_key(workspace: &str, mounts: &[Mount], host: HostSide<'_>) -> Option<String> {
    if workspace != host.workspace_folder
        || !is_plain_absolute(workspace)
        || !workspace.bytes().all(is_key_byte)
    {
        return None;
    }
    let home = resolved(host.home);
    let home = home.to_str()?;
    let near_home = |path: &str| super::is_under(path, home) || super::is_under(home, path);
    if near_home(workspace) {
        return None;
    }
    let clone = resolved(host.clone);
    mounts
        .iter()
        .filter(|mount| mount.bind && resolved(&mount.source) == clone)
        .filter(|mount| is_plain_absolute(&mount.destination) && !near_home(&mount.destination))
        .any(|mount| super::is_under(workspace, &mount.destination))
        .then(|| workspace.to_owned())
}

/// The bytes the seed's `printf` can spell, and so the bytes either write accepts.
fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-_/.@+".contains(&byte)
}

/// An absolute path in its one spelling: no empty, `.` or `..` component, and no
/// trailing slash. `/` itself is not one, because nothing may be keyed there.
fn is_plain_absolute(path: &str) -> bool {
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    !rest.is_empty()
        && rest
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// A host path with its links resolved, or as written where it does not exist.
fn resolved(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// What one merge came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Merged {
    /// The entry went in.
    Wrote,
    /// It was already true, so nothing was written.
    AlreadyTrusted,
    /// The file was not one to write, so nothing was written.
    Declined,
}

/// The key the trust stage prints the container's resolved config directory under.
pub(crate) const TRUST_DIR_KEY: &str = "trustdir";
/// The key the trust stage prints the workspace path under.
pub(crate) const TRUST_PATH_KEY: &str = "trustpath";

impl Reported {
    /// Read the trust stage's two lines out of the pass's report, or `None` where
    /// it printed none (no config file, or no workspace path worth recording).
    pub(crate) fn parse(report: &str) -> Option<Self> {
        let found = super::marked_lines(report);
        let config_dir = found.get(TRUST_DIR_KEY).filter(|value| !value.is_empty())?;
        let workspace = found
            .get(TRUST_PATH_KEY)
            .filter(|value| !value.is_empty())?;
        Some(Self {
            config_dir: config_dir.clone(),
            workspace: workspace.clone(),
        })
    }
}

/// The mounts in `docker inspect --format '{{json .Mounts}}'`'s answer. Anything
/// unreadable is no mounts, which matches nothing.
pub(crate) fn parse_mounts(json: &str) -> Vec<Mount> {
    let Ok(serde_json::Value::Array(mounts)) = serde_json::from_str(json.trim()) else {
        return Vec::new();
    };
    mounts
        .iter()
        .filter_map(|mount| {
            Some(Mount {
                bind: mount.get("Type")?.as_str()? == "bind",
                source: PathBuf::from(mount.get("Source")?.as_str()?),
                destination: mount.get("Destination")?.as_str()?.to_owned(),
                writable: mount.get("RW")?.as_bool()?,
            })
        })
        .collect()
}

/// Act on the trust stage's report: merge the entry into the host's file where
/// [`target`] allows it, and do nothing in every other case.
///
/// Silent, like the stage it replaces: every refusal leaves the workspace where it
/// was before trust was recorded at all, which is with the prompt.
pub(crate) fn record(
    runner: &dyn Runner,
    workspace: &str,
    report: &str,
    host: &HostLayout,
) -> Option<Merged> {
    let reported = Reported::parse(report)?;
    let up = created(host.devpod_home.as_ref(), workspace)?;
    let printed = docker::inspect_formatted(
        runner,
        "container",
        "{{json .Mounts}}",
        &NonEmpty::of([up.container])?,
    )?;
    let side = HostSide {
        home: &host.home,
        claude_dir: host.claude_dir.as_deref(),
        clone: &up.clone,
        workspace_folder: &up.workspace_folder,
    };
    let found = target(&reported, &parse_mounts(&printed), side)?;
    Some(merge(&found.dir, &found.key))
}

/// What devpod's own record of the `up` says it made for this workspace.
struct Created {
    /// The container.
    container: String,
    /// The host folder it mounted as the workspace.
    clone: PathBuf,
    /// The workspace folder in the container.
    workspace_folder: String,
}

/// Read devpod's own record of the `up` for this workspace.
///
/// devpod writes that record on the host, from the source dl gave it, so a repo's
/// devcontainer.json cannot move the folder it names, and nothing the container
/// runs after the `up` can move the workspace folder it records.
fn created(devpod_home: Option<&DevpodHome>, workspace: &str) -> Option<Created> {
    let path = sole_workspace_result(devpod_home, workspace)?;
    let document: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let container = document
        .pointer("/ContainerDetails/ID")?
        .as_str()
        .filter(|id| !id.is_empty())?;
    let clone = document
        .pointer("/SubstitutionContext/LocalWorkspaceFolder")?
        .as_str()
        .filter(|folder| folder.starts_with('/'))?;
    let workspace_folder = document
        .pointer("/SubstitutionContext/ContainerWorkspaceFolder")?
        .as_str()
        .filter(|folder| is_plain_absolute(folder))?;
    Some(Created {
        container: container.to_owned(),
        clone: PathBuf::from(clone),
        workspace_folder: workspace_folder.to_owned(),
    })
}

/// The name every Claude config file has, and the only name a link is followed to.
const CONFIG_FILE: &str = ".claude.json";

/// Record `projects[key].hasTrustDialogAccepted = true` in the `.claude.json` in
/// the host config directory `dir`, or leave every byte of it alone.
///
/// Every exit is a *no* except the one write:
///
/// - absent, or a dangling link: the seed's case, and the seed has already run.
///   Creating the file here would write it without `hasCompletedOnboarding`.
/// - a link that resolves outside `dir`, or to a file not named `.claude.json`. The
///   directory is shared with the container, so the container can put a link in
///   it, and on the host that link resolves to a host path: `../.claude.json` is the
///   host's own main config. Inside the container it could only ever write its own
///   files through it; followed here, it would be the host writing the host's.
/// - unreadable, not JSON, not an object, or `projects` or the entry not an object:
///   somebody else's file in a shape this does not understand, or a read that raced
///   Claude Code's own whole-file rewrite.
/// - already true: the commonest case after the first launch.
///
/// A link that stays inside `dir` is resolved first and written *through* rather
/// than replaced by a regular file. Every step after that check goes through the
/// folder handle [`locate`] opened, never through the path again (see [`Located`]).
///
/// **Write to a scratch file beside it and rename, or do not write.** There is no
/// in-place fallback: writing in place means truncating first, and the file being
/// truncated is the host's whole Claude configuration. The scratch file takes the
/// mode and, where this process may set it, the owner of the file it replaces.
///
/// The keys keep their order and every float its digits (`serde_json`'s
/// `preserve_order` and `float_roundtrip`, which the workspace enables) and the file is written back two-space indented without escaping
/// non-ASCII, which is how Claude Code writes it. So a merge into a file Claude Code
/// wrote moves no line but the one before the new entry.
pub(crate) fn merge(dir: &Path, key: &str) -> Merged {
    locate(dir).map_or(Merged::Declined, |located| located.merge(key))
}

/// The `.claude.json` that [`merge`] may write, found but not yet read.
///
/// It holds the folder the file is in as an open handle, reached from `dir` one
/// real folder at a time with no link followed. The container can still change the
/// shared directory while the host merges: it can swap a folder on the path for a
/// link to `..` after the check. A write that named the path again would follow
/// that link out to the host's `~/.claude.json`. A write relative to the handle
/// stays in the folder that passed the check.
pub(crate) struct Located {
    folder: std::os::fd::OwnedFd,
}

/// Resolve `dir/.claude.json`, or `None` for a file [`merge`] must not touch.
pub(crate) fn locate(dir: &Path) -> Option<Located> {
    use rustix::fs::{Mode, OFlags};

    let dir = std::fs::canonicalize(dir).ok()?;
    let path = std::fs::canonicalize(dir.join(CONFIG_FILE)).ok()?;
    if !path.starts_with(&dir) || path.file_name() != Some(std::ffi::OsStr::new(CONFIG_FILE)) {
        return None;
    }
    let below = path.strip_prefix(&dir).ok()?.parent()?;
    let walk = OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC;
    let mut folder = rustix::fs::open(&dir, walk, Mode::empty()).ok()?;
    for part in below.components() {
        let std::path::Component::Normal(part) = part else {
            return None;
        };
        folder = rustix::fs::openat(&folder, part, walk | OFlags::NOFOLLOW, Mode::empty()).ok()?;
    }
    Some(Located { folder })
}

impl Located {
    fn merge(self, key: &str) -> Merged {
        let Ok((text, before)) = self.read() else {
            return Merged::Declined;
        };
        let Ok(serde_json::Value::Object(mut config)) = serde_json::from_str(&text) else {
            return Merged::Declined;
        };
        let projects = config
            .entry("projects")
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        let Some(projects) = projects.as_object_mut() else {
            return Merged::Declined;
        };
        let entry = projects
            .entry(key)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        let Some(entry) = entry.as_object_mut() else {
            return Merged::Declined;
        };
        if entry.get(super::TRUST_FLAG_KEY) == Some(&serde_json::Value::Bool(true)) {
            return Merged::AlreadyTrusted;
        }
        entry.insert(super::TRUST_FLAG_KEY.to_owned(), true.into());
        let Ok(mut body) = serde_json::to_string_pretty(&config) else {
            return Merged::Declined;
        };
        if text.ends_with('\n') {
            body.push('\n');
        }
        match self.replace(body.as_bytes(), &before) {
            Ok(()) => Merged::Wrote,
            Err(_) => Merged::Declined,
        }
    }

    /// The file's text and metadata, read through the folder handle. A link put
    /// there since the check, or anything but a regular file (a FIFO would block
    /// the read for good), is refused.
    fn read(&self) -> std::io::Result<(String, std::fs::Metadata)> {
        use rustix::fs::{Mode, OFlags};
        use std::io::Read as _;

        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let mut file = std::fs::File::from(rustix::fs::openat(
            &self.folder,
            CONFIG_FILE,
            flags,
            Mode::empty(),
        )?);
        let before = file.metadata()?;
        if !before.is_file() {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        Ok((text, before))
    }

    /// Put `body` in the file by renaming a scratch file over it, both named
    /// relative to the folder handle.
    fn replace(&self, body: &[u8], before: &std::fs::Metadata) -> std::io::Result<()> {
        use rustix::fs::{AtFlags, Mode, OFlags};
        use std::io::Write as _;
        use std::os::unix::fs::MetadataExt as _;

        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let (name, file) = (0..16)
            .find_map(|attempt| {
                let name = scratch_name(attempt);
                match rustix::fs::openat(&self.folder, &name, flags, Mode::RUSR | Mode::WUSR) {
                    Err(rustix::io::Errno::EXIST) => None,
                    opened => Some(opened.map(|fd| (name, std::fs::File::from(fd)))),
                }
            })
            .unwrap_or(Err(rustix::io::Errno::EXIST))?;
        let written = (|| -> std::io::Result<()> {
            let mut file = file;
            file.write_all(body)?;
            file.sync_all()?;
            file.set_permissions(before.permissions())?;
            // Only root may give a file away, and a host `dl` is rarely root: the
            // file it replaces is then this user's already, which is what the
            // scratch file is.
            let _ = std::os::unix::fs::fchown(&file, Some(before.uid()), Some(before.gid()));
            rustix::fs::renameat(&self.folder, &name, &self.folder, CONFIG_FILE)?;
            Ok(())
        })();
        if written.is_err() {
            let _ = rustix::fs::unlinkat(&self.folder, &name, AtFlags::empty());
        }
        written
    }
}

/// A name for the scratch file beside `.claude.json`, new for each attempt.
fn scratch_name(attempt: u32) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    format!(".claude.json.dl-{}-{nanos}-{attempt}", std::process::id())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use super::*;

    const KEY: &str = "/workspaces/devlaunch-nb99-hicj";

    /// A scratch host config directory holding `config` as its `.claude.json`.
    struct Config {
        scratch: tempfile::TempDir,
        dir: PathBuf,
    }

    impl Config {
        fn new(config: Option<&str>) -> Self {
            let scratch = tempfile::tempdir().expect("a scratch dir");
            let dir = scratch.path().join(".claude");
            std::fs::create_dir_all(&dir).expect("a config dir");
            if let Some(config) = config {
                std::fs::write(dir.join(".claude.json"), config).expect("their config");
            }
            Self { scratch, dir }
        }

        fn file(&self) -> PathBuf {
            self.dir.join(".claude.json")
        }

        fn read(&self) -> String {
            std::fs::read_to_string(self.file()).expect("a config to read")
        }

        fn merge(&self) -> Merged {
            merge(&self.dir, KEY)
        }

        fn parsed(&self) -> serde_json::Value {
            serde_json::from_str(&self.read()).expect("JSON Claude Code can read")
        }

        fn trusts(&self, path: &str) -> Option<serde_json::Value> {
            self.parsed()
                .get("projects")
                .and_then(|projects| projects.get(path))
                .and_then(|entry| entry.get("hasTrustDialogAccepted"))
                .cloned()
        }
    }

    #[test]
    fn the_merge_adds_one_key_and_keeps_every_other_value() {
        // The case the merge exists for: the host's real `.claude.json`, shared with
        // the container through a mount. One entry appears and nothing else moves.
        // The other project is the point: a textual insert either loses to the
        // original on last-key-wins or takes this entry with it.
        let config = Config::new(Some(
            r#"{"oauthAccount":{"emailAddress":"someone@example.com"},
                "projects":{"/workspaces/devlaunch-nb98-btvv":{"hasTrustDialogAccepted":true,
                                                               "allowedTools":["Bash"]}},
                "themeMode":"dark"}"#,
        ));

        assert_eq!(config.merge(), Merged::Wrote);

        assert_eq!(config.trusts(KEY), Some(true.into()));
        let parsed = config.parsed();
        assert_eq!(
            parsed.pointer("/oauthAccount/emailAddress"),
            Some(&"someone@example.com".into())
        );
        assert_eq!(parsed.get("themeMode"), Some(&"dark".into()));
        assert_eq!(
            config.trusts("/workspaces/devlaunch-nb98-btvv"),
            Some(true.into())
        );
        assert_eq!(
            parsed.pointer("/projects/~1workspaces~1devlaunch-nb98-btvv/allowedTools"),
            Some(&serde_json::json!(["Bash"]))
        );
    }

    #[test]
    fn the_merge_keeps_the_shape_and_the_key_order_of_the_config() {
        // Claude Code writes the file pretty-printed, with non-ASCII as is, in its
        // own key order. A merge that sorted the keys or escaped the text would
        // rewrite every line of a 283 KB file to add one key.
        let original = "{\n  \"themeMode\": \"dark\",\n  \"tipsHistory\": {\n    \"caf\u{e9}\": 1\n  },\n  \"autoUpdates\": false\n}";
        let config = Config::new(Some(original));

        config.merge();

        assert_eq!(
            config.read(),
            format!(
                "{}\n  \"projects\": {{\n    \"{KEY}\": {{\n      \"hasTrustDialogAccepted\": true\n    }}\n  }}\n}}",
                original
                    .strip_suffix("\n}")
                    .expect("the closing brace")
                    .to_owned()
                    + ","
            ),
            "every line before the new entry is the line that was there"
        );
    }

    #[test]
    fn the_merge_keeps_every_digit_of_a_float_in_the_config() {
        // Claude Code keeps costs and rates as floats with seventeen significant
        // digits. A parse that is off by one ulp writes back a different number.
        let config = Config::new(Some(
            r#"{"lastCost":0.0013015855369760793,"themeMode":"dark"}"#,
        ));

        assert_eq!(config.merge(), Merged::Wrote);

        assert!(
            config.read().contains("0.0013015855369760793"),
            "{}",
            config.read()
        );
    }

    #[test]
    fn the_merge_keeps_a_trailing_newline_where_there_was_one() {
        let config = Config::new(Some("{\n  \"themeMode\": \"dark\"\n}\n"));
        config.merge();
        assert!(config.read().ends_with("}\n"), "{}", config.read());
    }

    #[test]
    fn the_merge_is_idempotent_and_rewrites_nothing_it_need_not() {
        // Every launch merges again, so "already true" is the commonest case, and
        // the file it would rewrite is the host's while Claude Code may be writing
        // it too.
        let config = Config::new(Some(r#"{"hasCompletedOnboarding":true}"#));

        assert_eq!(config.merge(), Merged::Wrote);
        let after_first = config.read();
        let inode = std::os::unix::fs::MetadataExt::ino(
            &std::fs::metadata(config.file()).expect("the config"),
        );

        assert_eq!(config.merge(), Merged::AlreadyTrusted);
        assert_eq!(config.read(), after_first);
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(
                &std::fs::metadata(config.file()).expect("the config")
            ),
            inode,
            "not even renamed over"
        );
    }

    #[test]
    fn the_merge_leaves_a_file_it_cannot_read_alone() {
        // Somebody else's file in a shape this does not understand, or a read that
        // raced Claude Code's own whole-file rewrite. Not one to rewrite.
        for shape in [
            "not json at all",
            "[1, 2, 3]",
            r#"{"projects": "not an object either"}"#,
            r#"{"projects": {"/workspaces/devlaunch-nb99-hicj": 7}}"#,
            "",
        ] {
            let config = Config::new(Some(shape));
            assert_eq!(config.merge(), Merged::Declined, "{shape}");
            assert_eq!(config.read(), shape, "{shape}");
        }
    }

    #[test]
    fn the_merge_writes_nothing_where_there_is_no_file() {
        // Absent is the seed's case, and the seed runs in the container first.
        // Creating the file here would write it without `hasCompletedOnboarding`.
        let config = Config::new(None);
        assert_eq!(config.merge(), Merged::Declined);
        assert!(!config.file().exists());

        // A dangling link reads as absent: nothing to merge with, and following it
        // would write at a path nothing named.
        let target = config.scratch.path().join(".claude.json");
        std::os::unix::fs::symlink(&target, config.file()).expect("a dangling link");
        assert_eq!(config.merge(), Merged::Declined);
        assert!(!target.exists(), "the merge followed a dangling link");
    }

    #[test]
    fn the_merge_writes_through_a_symlinked_config_rather_than_replacing_it() {
        // The real file may sit deeper in the directory with a link to it here. A
        // rename over the link would leave a regular file and cut the link.
        let config = Config::new(None);
        let real = config.dir.join("real/.claude.json");
        std::fs::create_dir_all(real.parent().expect("a parent")).expect("a nested dir");
        std::fs::write(&real, r#"{"hasCompletedOnboarding":true}"#).expect("the real config");
        std::os::unix::fs::symlink(&real, config.file()).expect("a linked config");

        assert_eq!(config.merge(), Merged::Wrote);

        assert!(
            std::fs::symlink_metadata(config.file())
                .expect("the link")
                .file_type()
                .is_symlink(),
            "the link is still a link"
        );
        assert_eq!(config.trusts(KEY), Some(true.into()));
    }

    #[test]
    fn the_merge_does_not_follow_a_link_to_a_file_that_is_not_a_claude_config() {
        // The directory is shared with the container, so the container can put a
        // link here. Inside the container the link resolves in the container; on the
        // host it resolves on the host. A link to some other JSON file of the host
        // user's is a write the container could never have made itself.
        let config = Config::new(None);
        let other = config.scratch.path().join("settings.json");
        std::fs::write(&other, r#"{"editor":"vim"}"#).expect("another JSON file");
        std::os::unix::fs::symlink(&other, config.file()).expect("a planted link");

        assert_eq!(config.merge(), Merged::Declined);
        assert_eq!(
            std::fs::read_to_string(&other).expect("the other file"),
            r#"{"editor":"vim"}"#
        );
    }

    #[test]
    fn the_merge_does_not_follow_a_link_out_of_the_config_dir() {
        // A postCreateCommand can run `ln -sf ../.claude.json ~/.claude/.claude.json`
        // in the shared directory. On the host that names `~/.claude.json`, the
        // host's own main config, which the container never had mounted.
        let config = Config::new(None);
        let outside = config.scratch.path().join(".claude.json");
        let theirs = r#"{"hasCompletedOnboarding":true}"#;
        std::fs::write(&outside, theirs).expect("the host's main config");
        std::os::unix::fs::symlink("../.claude.json", config.file()).expect("a planted link");

        assert_eq!(config.merge(), Merged::Declined);
        assert_eq!(
            std::fs::read_to_string(&outside).expect("the host's main config"),
            theirs
        );
    }

    #[test]
    fn a_folder_swapped_for_a_link_after_the_check_does_not_carry_the_write_out() {
        // The container still runs while the host merges, and it can write the
        // shared directory. A link to `real/.claude.json` passes the check; then
        // `real` becomes a link to `..`. A write that names the path again lands
        // in `~/.claude.json`, the host's own main config.
        let config = Config::new(None);
        let real = config.dir.join("real");
        std::fs::create_dir_all(&real).expect("a nested dir");
        std::fs::write(
            real.join(".claude.json"),
            r#"{"hasCompletedOnboarding":true}"#,
        )
        .expect("the decoy config");
        std::os::unix::fs::symlink("real/.claude.json", config.file()).expect("a planted link");
        let outside = config.scratch.path().join(".claude.json");
        let theirs = r#"{"hasCompletedOnboarding":true}"#;
        std::fs::write(&outside, theirs).expect("the host's main config");

        let located = locate(&config.dir).expect("a link that stays inside the dir");
        std::fs::rename(&real, config.dir.join("moved")).expect("the folder moved away");
        std::os::unix::fs::symlink("..", &real).expect("the folder swapped for a link");
        let _ = located.merge(KEY);

        assert_eq!(
            std::fs::read_to_string(&outside).expect("the host's main config"),
            theirs
        );
    }

    #[test]
    fn a_fifo_in_place_of_the_config_is_refused_rather_than_read_for_good() {
        // The container can put a FIFO in the shared directory. A blocking read
        // of it would hang `dl` on the host with nobody ever writing to it.
        let config = Config::new(None);
        rustix::fs::mknodat(
            rustix::fs::CWD,
            config.file(),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .expect("a FIFO");

        assert_eq!(config.merge(), Merged::Declined);
    }

    #[test]
    fn the_merge_keeps_the_mode_of_the_file_it_replaces() {
        let config = Config::new(Some(r#"{"hasCompletedOnboarding":true}"#));
        std::fs::set_permissions(config.file(), std::fs::Permissions::from_mode(0o600))
            .expect("a private config");

        assert_eq!(config.merge(), Merged::Wrote);

        let mode = std::fs::metadata(config.file())
            .expect("the config")
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o600);
    }

    #[test]
    fn a_config_the_merge_cannot_replace_is_left_exactly_as_it_was() {
        // Write to a scratch file and rename, or do not write. A directory this
        // process may not create a file in is the shape that reaches this, and there
        // the file itself is often still writable: an in-place fallback would
        // truncate the host's whole configuration before it wrote.
        let config = Config::new(Some(r#"{"hasCompletedOnboarding":true}"#));
        std::fs::set_permissions(&config.dir, std::fs::Permissions::from_mode(0o500))
            .expect("a read-only config dir");
        // Root ignores the mode, so probe rather than ask who we are.
        let reachable = std::fs::write(config.dir.join("probe"), "x").is_err();
        if reachable {
            assert_eq!(config.merge(), Merged::Declined);
            assert_eq!(config.read(), r#"{"hasCompletedOnboarding":true}"#);
            let leftovers = std::fs::read_dir(&config.dir).expect("the dir").count();
            assert_eq!(leftovers, 1, "no scratch file left behind");
        }
        std::fs::set_permissions(&config.dir, std::fs::Permissions::from_mode(0o700))
            .expect("a writable dir again");
    }

    // =======================================================================
    // which host file, and which key
    // =======================================================================

    /// A host with a home, a Claude config directory and a clone, and a container
    /// that mounts the two the way the claude-code feature and devpod do.
    struct Host {
        _scratch: tempfile::TempDir,
        home: PathBuf,
        clone: PathBuf,
    }

    impl Host {
        fn new() -> Self {
            let scratch = tempfile::tempdir().expect("a scratch dir");
            let root = std::fs::canonicalize(scratch.path()).expect("a resolved scratch dir");
            let home = root.join("home/someone");
            let clone = home.join(".cache/devlaunch/repos/o/r/r-main-abcd");
            std::fs::create_dir_all(home.join(".claude")).expect("a claude dir");
            std::fs::create_dir_all(&clone).expect("a clone");
            Self {
                _scratch: scratch,
                home,
                clone,
            }
        }

        fn side(&self) -> HostSide<'_> {
            HostSide {
                home: &self.home,
                claude_dir: None,
                clone: &self.clone,
                workspace_folder: "/workspaces/r-main-abcd",
            }
        }

        fn bind(&self, source: &Path, destination: &str, writable: bool) -> Mount {
            Mount {
                bind: true,
                source: source.to_path_buf(),
                destination: destination.to_owned(),
                writable,
            }
        }

        /// The bencher shape: the clone at `/workspaces/<id>`, `~/.claude` at the
        /// container's config directory, both read-write.
        fn mounts(&self) -> Vec<Mount> {
            vec![
                self.bind(&self.clone, "/workspaces/r-main-abcd", true),
                self.bind(&self.home.join(".claude"), "/home/vscode/.claude", true),
            ]
        }
    }

    fn reported(config_dir: &str, workspace: &str) -> Reported {
        Reported {
            config_dir: config_dir.to_owned(),
            workspace: workspace.to_owned(),
        }
    }

    #[test]
    fn a_container_sharing_the_hosts_claude_dir_merges_into_the_hosts_file() {
        let host = Host::new();
        assert_eq!(
            target(
                &reported("/home/vscode/.claude", "/workspaces/r-main-abcd"),
                &host.mounts(),
                host.side()
            ),
            Some(Target {
                dir: host.home.join(".claude"),
                key: "/workspaces/r-main-abcd".to_owned(),
            })
        );
    }

    #[test]
    fn a_recorded_workspace_folder_under_the_clones_mount_point_is_recorded() {
        // A devcontainer.json `workspaceFolder` may name a folder inside the clone.
        let host = Host::new();
        let side = HostSide {
            workspace_folder: "/workspaces/r-main-abcd/app",
            ..host.side()
        };
        assert_eq!(
            target(
                &reported("/home/vscode/.claude", "/workspaces/r-main-abcd/app"),
                &host.mounts(),
                side
            )
            .map(|found| found.key),
            Some("/workspaces/r-main-abcd/app".to_owned())
        );
    }

    #[test]
    fn a_recorded_workspace_folder_off_the_clones_mount_point_is_never_recorded() {
        let host = Host::new();
        let side = HostSide {
            workspace_folder: "/workspaces/other",
            ..host.side()
        };
        assert_eq!(
            target(
                &reported("/home/vscode/.claude", "/workspaces/other"),
                &host.mounts(),
                side
            ),
            None
        );
    }

    #[test]
    fn a_folder_under_the_workspace_folder_is_never_recorded() {
        // A login profile can `cd` into a sub-folder with its own `.git`, and the
        // pass would report that. devpod recorded the workspace folder at `up`, and
        // only that folder is the workspace's to claim.
        let host = Host::new();
        assert_eq!(
            target(
                &reported("/home/vscode/.claude", "/workspaces/r-main-abcd/sub"),
                &host.mounts(),
                host.side()
            ),
            None
        );
    }

    #[test]
    fn the_hosts_own_claude_config_dir_counts_as_well_as_the_default() {
        let host = Host::new();
        let elsewhere = host.home.join("profiles/work");
        std::fs::create_dir_all(&elsewhere).expect("a profile dir");
        let mounts = vec![
            host.bind(&host.clone, "/workspaces/r-main-abcd", true),
            host.bind(&elsewhere, "/home/vscode/.claude", true),
        ];
        let side = HostSide {
            claude_dir: Some(&elsewhere),
            ..host.side()
        };
        assert_eq!(
            target(
                &reported("/home/vscode/.claude", "/workspaces/r-main-abcd"),
                &mounts,
                side
            )
            .map(|found| found.dir),
            Some(elsewhere)
        );
    }

    #[test]
    fn a_config_dir_that_is_not_the_hosts_claude_dir_mounted_read_write_is_left_alone() {
        let host = Host::new();
        let other = host.home.join("somewhere-else");
        std::fs::create_dir_all(&other).expect("another dir");
        let cases: Vec<(&str, Vec<Mount>)> = vec![
            (
                "not mounted at all: the container's own directory",
                vec![host.bind(&host.clone, "/workspaces/r-main-abcd", true)],
            ),
            (
                "mounted read-only",
                vec![
                    host.bind(&host.clone, "/workspaces/r-main-abcd", true),
                    host.bind(&host.home.join(".claude"), "/home/vscode/.claude", false),
                ],
            ),
            (
                "mounted from some other host directory",
                vec![
                    host.bind(&host.clone, "/workspaces/r-main-abcd", true),
                    host.bind(&other, "/home/vscode/.claude", true),
                ],
            ),
            (
                "a volume, which is no host path at all",
                vec![
                    host.bind(&host.clone, "/workspaces/r-main-abcd", true),
                    Mount {
                        bind: false,
                        ..host.bind(&host.home.join(".claude"), "/home/vscode/.claude", true)
                    },
                ],
            ),
            (
                "the directory's parent mounted, not the directory",
                vec![
                    host.bind(&host.clone, "/workspaces/r-main-abcd", true),
                    host.bind(&host.home.join(".claude"), "/home/vscode", true),
                ],
            ),
            (
                "the file itself mounted over from somewhere else",
                vec![
                    host.bind(&host.clone, "/workspaces/r-main-abcd", true),
                    host.bind(&host.home.join(".claude"), "/home/vscode/.claude", true),
                    host.bind(
                        &other.join("x.json"),
                        "/home/vscode/.claude/.claude.json",
                        true,
                    ),
                ],
            ),
        ];
        for (case, mounts) in cases {
            assert_eq!(
                target(
                    &reported("/home/vscode/.claude", "/workspaces/r-main-abcd"),
                    &mounts,
                    host.side()
                ),
                None,
                "{case}"
            );
        }
    }

    #[test]
    fn a_reported_path_outside_the_workspace_folder_is_never_recorded() {
        // The path comes from a container that ran a stranger's postCreate. The host
        // records it only where it is the clone's mount point or under it.
        let host = Host::new();
        for workspace in [
            "/",
            "/workspaces",
            "/workspaces/r-main-abcd-other",
            "/workspaces/other",
            "/home/vscode",
            "/home/someone",
            "/workspaces/r-main-abcd/../other",
            "/workspaces/r-main-abcd/./x",
            "/workspaces//r-main-abcd",
            "/workspaces/r-main-abcd/",
            "workspaces/r-main-abcd",
            "/workspaces/r-main-abcd/say \"hi\"",
            "",
        ] {
            assert_eq!(
                target(
                    &reported("/home/vscode/.claude", workspace),
                    &host.mounts(),
                    host.side()
                ),
                None,
                "{workspace:?}"
            );
        }
    }

    #[test]
    fn a_clone_mounted_at_or_around_a_host_home_path_is_never_recorded() {
        // A repo picks its own `workspaceMount` target. Mounted at the host user's
        // home, inside it, or above it, the key would name a real host directory,
        // and the host's own Claude Code would stop asking about it.
        let host = Host::new();
        let home = host.home.to_str().expect("a path").to_owned();
        let parent = host
            .home
            .parent()
            .and_then(Path::to_str)
            .expect("a parent")
            .to_owned();
        for destination in [
            home.clone(),
            format!("{home}/projects/r"),
            parent,
            "/".to_owned(),
        ] {
            let mounts = vec![
                host.bind(&host.clone, &destination, true),
                host.bind(&host.home.join(".claude"), "/home/vscode/.claude", true),
            ];
            assert_eq!(
                target(
                    &reported("/home/vscode/.claude", &destination),
                    &mounts,
                    HostSide {
                        workspace_folder: &destination,
                        ..host.side()
                    }
                ),
                None,
                "{destination}"
            );
        }
    }

    #[test]
    fn a_folder_beside_the_home_under_a_clone_mounted_above_it_is_never_recorded() {
        // The folder itself is not near the home, but the clone's mount point is
        // its parent, which holds the home: the mount point alone decides.
        let host = Host::new();
        let parent = host
            .home
            .parent()
            .and_then(Path::to_str)
            .expect("a parent")
            .to_owned();
        let folder = format!("{parent}/other");
        let mounts = vec![
            host.bind(&host.clone, &parent, true),
            host.bind(&host.home.join(".claude"), "/home/vscode/.claude", true),
        ];
        assert_eq!(
            target(
                &reported("/home/vscode/.claude", &folder),
                &mounts,
                HostSide {
                    workspace_folder: &folder,
                    ..host.side()
                }
            ),
            None
        );
    }

    #[test]
    fn a_claude_dir_that_is_a_link_on_the_host_matches_the_mount_of_its_target() {
        // A dotfiles manager links `~/.claude` to its own tree, and docker reports a
        // bind mount's source with the link resolved.
        let host = Host::new();
        let real = host.home.join("dotfiles/claude");
        std::fs::create_dir_all(&real).expect("the dotfiles' claude dir");
        std::fs::remove_dir(host.home.join(".claude")).expect("the plain claude dir gone");
        std::os::unix::fs::symlink(&real, host.home.join(".claude")).expect("a linked claude dir");
        let mounts = vec![
            host.bind(&host.clone, "/workspaces/r-main-abcd", true),
            host.bind(&real, "/home/vscode/.claude", true),
        ];
        assert_eq!(
            target(
                &reported("/home/vscode/.claude", "/workspaces/r-main-abcd"),
                &mounts,
                host.side()
            ),
            Some(Target {
                dir: real,
                key: "/workspaces/r-main-abcd".to_owned(),
            })
        );
    }

    #[test]
    fn mounts_docker_did_not_describe_in_full_are_no_mounts() {
        assert_eq!(parse_mounts("not json"), Vec::new());
        let printed = serde_json::json!([
            {"Type": "bind", "Source": "/srv/a", "Destination": "/a"},
            {"Type": "bind", "Source": "/srv/b", "Destination": "/b", "RW": true},
        ])
        .to_string();
        assert_eq!(
            parse_mounts(&printed),
            vec![Mount {
                bind: true,
                source: PathBuf::from("/srv/b"),
                destination: "/b".to_owned(),
                writable: true,
            }],
            "the mount with no RW is dropped, not read as writable or read-only"
        );
    }

    #[test]
    fn a_workspace_folder_that_is_not_the_clone_is_never_recorded() {
        // Only the clone dl made is a workspace folder. Another host directory the
        // repo mounts is the repo's choice, not dl's.
        let host = Host::new();
        let other = host.home.join("elsewhere");
        std::fs::create_dir_all(&other).expect("another dir");
        let mounts = vec![
            host.bind(&other, "/workspaces/r-main-abcd", true),
            host.bind(&host.home.join(".claude"), "/home/vscode/.claude", true),
        ];
        assert_eq!(
            target(
                &reported("/home/vscode/.claude", "/workspaces/r-main-abcd"),
                &mounts,
                host.side()
            ),
            None
        );
    }
}
