//! Persistent overrides for shells opened in a Herdr workspace.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::clients::claude::ProfileName;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn validate_key(key: &str) -> io::Result<()> {
    let mut chars = key.chars();
    if !chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        || key.starts_with("HERDR_")
        || key.starts_with("XDG_")
        || matches!(
            key,
            "HOME" | "DEVLAUNCH_CLAUDE_PROFILES_DIR" | "CLAUDE_PROFILES_DIR"
        )
    {
        return Err(invalid(format!(
            "invalid or reserved workspace variable: {key}"
        )));
    }
    Ok(())
}

/// Which Claude configuration a pane gets, as one choice rather than two fields
/// that can disagree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ClaudeConfig {
    /// No override: the pane keeps whatever `CLAUDE_CONFIG_DIR` the host exported.
    #[default]
    Inherited,
    /// `CLAUDE_CONFIG_DIR` removed from the pane, whatever the host exported.
    Cleared,
    /// `CLAUDE_CONFIG_DIR` set to a value the person typed.
    Directory(String),
    /// A profile under the profiles root, resolved again every time a pane opens.
    Profile(ProfileName),
    /// `profile default`: no saved directory and no saved authentication, so the
    /// pane uses the server's own Claude environment.
    ServerDefault,
}

impl ClaudeConfig {
    /// The name to pass on as `--claude-profile`, for the two arms that name one.
    pub fn profile_name(&self) -> Option<&str> {
        match self {
            Self::Profile(name) => Some(name.as_str()),
            Self::ServerDefault => Some(super::claude_profiles::DEFAULT_PROFILE),
            Self::Inherited | Self::Cleared | Self::Directory(_) => None,
        }
    }
}

/// Where `name` lives now, refusing rather than opening a pane against a directory
/// that has gone.
pub fn profile_directory(name: &ProfileName, root: &Path) -> io::Result<PathBuf> {
    let directory = root.join(name.as_str());
    if !directory.is_dir() {
        return Err(invalid(format!(
            "saved Claude profile directory disappeared: {}",
            directory.display()
        )));
    }
    fs::canonicalize(directory)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    variables: BTreeMap<String, Option<String>>,
    profile: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(try_from = "Stored", into = "Stored")]
pub struct Environment {
    variables: BTreeMap<String, Option<String>>,
    claude: ClaudeConfig,
}

impl TryFrom<Stored> for Environment {
    type Error = String;

    fn try_from(stored: Stored) -> Result<Self, Self::Error> {
        let Stored {
            mut variables,
            profile,
        } = stored;
        let directory = variables.remove("CLAUDE_CONFIG_DIR");
        let claude = match (profile, directory) {
            (Some(_), Some(_)) => {
                return Err("saved profile duplicates its Claude configuration directory".into());
            }
            (Some(name), None) => {
                let name = ProfileName::parse(&name)
                    .ok_or_else(|| "invalid Claude profile name".to_owned())?;
                if name.as_str() == super::claude_profiles::DEFAULT_PROFILE {
                    ClaudeConfig::ServerDefault
                } else {
                    ClaudeConfig::Profile(name)
                }
            }
            (None, Some(Some(value))) => ClaudeConfig::Directory(value),
            (None, Some(None)) => ClaudeConfig::Cleared,
            (None, None) => ClaudeConfig::Inherited,
        };
        for (key, value) in &variables {
            validate_key(key).map_err(|e| e.to_string())?;
            if value.as_ref().is_some_and(|v| v.contains('\0')) {
                return Err(format!("NUL in value for {key}"));
            }
        }
        if matches!(&claude, ClaudeConfig::Directory(value) if value.contains('\0')) {
            return Err("NUL in value for CLAUDE_CONFIG_DIR".to_owned());
        }
        Ok(Self { variables, claude })
    }
}

impl From<Environment> for Stored {
    fn from(environment: Environment) -> Self {
        let Environment {
            mut variables,
            claude,
        } = environment;
        let mut profile = None;
        match claude {
            ClaudeConfig::Inherited => {}
            ClaudeConfig::Cleared => {
                variables.insert("CLAUDE_CONFIG_DIR".to_owned(), None);
            }
            ClaudeConfig::Directory(value) => {
                variables.insert("CLAUDE_CONFIG_DIR".to_owned(), Some(value));
            }
            ClaudeConfig::Profile(name) => profile = Some(name.as_str().to_owned()),
            ClaudeConfig::ServerDefault => {
                profile = Some(super::claude_profiles::DEFAULT_PROFILE.to_owned());
            }
        }
        Self { variables, profile }
    }
}

impl Environment {
    /// Every override but the Claude configuration, which is [`Environment::claude`].
    pub fn variables(&self) -> &BTreeMap<String, Option<String>> {
        &self.variables
    }

    pub fn claude(&self) -> &ClaudeConfig {
        &self.claude
    }

    pub fn set(&mut self, key: &str, value: Option<String>) -> io::Result<()> {
        validate_key(key)?;
        if value.as_ref().is_some_and(|v| v.contains('\0')) {
            return Err(invalid(format!("NUL in value for {key}")));
        }
        if key == "CLAUDE_CONFIG_DIR" {
            self.claude = match value {
                Some(value) => ClaudeConfig::Directory(value),
                None => ClaudeConfig::Cleared,
            };
            return Ok(());
        }
        self.variables.insert(key.to_owned(), value);
        Ok(())
    }

    pub fn select_profile(&mut self, name: &str, root: &Path) -> io::Result<()> {
        let name =
            ProfileName::parse(name).ok_or_else(|| invalid("invalid Claude profile name"))?;
        if name.as_str() == super::claude_profiles::DEFAULT_PROFILE {
            for key in [
                "CLAUDE_CODE_OAUTH_TOKEN",
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
            ] {
                self.variables.remove(key);
            }
            self.claude = ClaudeConfig::ServerDefault;
            return Ok(());
        }
        if !root.join(name.as_str()).is_dir() {
            return Err(invalid(format!(
                "Claude profile directory does not exist: {}",
                root.join(name.as_str()).display()
            )));
        }
        for key in [
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
        ] {
            self.set(key, None)?;
        }
        self.claude = ClaudeConfig::Profile(name);
        Ok(())
    }
}

pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(state_root: &Path, socket: &Path, workspace: &str) -> io::Result<Self> {
        if !socket.is_absolute() {
            return Err(invalid("HERDR_SOCKET_PATH must be absolute"));
        }
        if workspace.is_empty()
            || !workspace
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
        {
            return Err(invalid("invalid Herdr workspace ID"));
        }
        let session = format!(
            "{:x}",
            Sha256::digest(socket.as_os_str().as_encoded_bytes())
        );
        Ok(Self {
            path: state_root
                .join("devlaunch/herdr-environment")
                .join(session)
                .join(format!("{workspace}.json")),
        })
    }

    pub fn read(&self) -> io::Result<Environment> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                invalid(format!(
                    "invalid workspace environment {}: {e}",
                    self.path.display()
                ))
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Environment::default()),
            Err(error) => Err(error),
        }
    }

    pub fn update(
        &self,
        change: impl FnOnce(&mut Environment) -> io::Result<()>,
    ) -> io::Result<()> {
        let directory = self.path.parent().expect("state path has a parent");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(self.path.with_extension("lock"))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)?;
        let mut environment = self.read()?;
        change(&mut environment)?;
        let bytes = serde_json::to_vec_pretty(&environment)?;
        write_private(&self.path, &bytes)
    }

    pub fn clear(&self) -> io::Result<()> {
        // Clearing must also recover a corrupt file, without attempting to parse it.
        let directory = self.path.parent().expect("state path has a parent");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(self.path.with_extension("lock"))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)?;
        match fs::remove_file(&self.path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

// The two writers below take opposite mode policies on purpose, and which one a caller wants
// follows from who owns the file: a config belongs to the user, so its mode is theirs to choose,
// while the state file is ours and holds whatever secrets were set as overrides.
fn replace_preserving_mode(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = staged(path, bytes)?;
    match fs::metadata(path) {
        Ok(existing) => temporary
            .as_file()
            .set_permissions(existing.permissions())?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = staged(path, bytes)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn staged(path: &Path, bytes: &[u8]) -> io::Result<tempfile::NamedTempFile> {
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().expect("file has a parent"))?;
    temporary.write_all(bytes)?;
    Ok(temporary)
}

/// Change only Herdr's pane launcher. Unknown launchers require an explicit manual edit.
pub fn configure(config: &Path, script: &Path, home: &Path) -> io::Result<bool> {
    let resolved;
    let config = match fs::symlink_metadata(config) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            resolved = fs::canonicalize(config)?;
            resolved.as_path()
        }
        Ok(_) => config,
        Err(error) if error.kind() == io::ErrorKind::NotFound => config,
        Err(error) => return Err(error),
    };
    let original = match fs::read_to_string(config) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut document = original
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| invalid(format!("invalid Herdr config: {e}")))?;
    let script = script
        .to_str()
        .ok_or_else(|| invalid("pane shell path is not UTF-8"))?;
    let terminal = document
        .entry("terminal")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let table = terminal
        .as_table_like_mut()
        .ok_or_else(|| invalid("Herdr terminal config is not a table"))?;
    if let Some(current) = table.get("default_shell") {
        let current = current
            .as_str()
            .ok_or_else(|| invalid("Herdr default_shell is not a string"))?;
        if current == script {
            return Ok(false);
        }
        let prototype = home.join(".local/bin/herdr-workspace-shell");
        if !current.is_empty()
            && current != "dl-herdr-shell"
            && current != prototype.to_string_lossy()
        {
            return Err(invalid(format!(
                "Herdr uses a custom default_shell ({current}); set terminal.default_shell to {script} manually to replace it"
            )));
        }
    }
    table.insert("default_shell", toml_edit::value(script));
    let directory = config
        .parent()
        .ok_or_else(|| invalid("config path has no parent"))?;
    fs::create_dir_all(directory)?;
    replace_preserving_mode(config, document.to_string().as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn sessions_and_workspaces_have_independent_overrides() {
        let root = tempfile::tempdir().unwrap();
        let first = Store::new(root.path(), Path::new("/tmp/first.sock"), "w1").unwrap();
        let second = Store::new(root.path(), Path::new("/tmp/second.sock"), "w1").unwrap();
        let other_workspace = Store::new(root.path(), Path::new("/tmp/first.sock"), "w2").unwrap();
        first
            .update(|e| e.set("MESSAGE", Some("literal $(echo nope)\n'\"".into())))
            .unwrap();
        assert_eq!(
            first.read().unwrap().variables()["MESSAGE"].as_deref(),
            Some("literal $(echo nope)\n'\"")
        );
        assert!(second.read().unwrap().variables().is_empty());
        assert!(other_workspace.read().unwrap().variables().is_empty());
        assert_eq!(
            fs::metadata(&first.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        first.clear().unwrap();
        assert!(first.read().unwrap().variables().is_empty());
    }

    #[test]
    fn a_world_readable_state_file_is_written_back_private() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path(), Path::new("/tmp/first.sock"), "w1").unwrap();
        store
            .update(|e| e.set("ANTHROPIC_API_KEY", Some("secret".into())))
            .unwrap();
        fs::set_permissions(&store.path, fs::Permissions::from_mode(0o644)).unwrap();
        store
            .update(|e| e.set("OTHER", Some("value".into())))
            .unwrap();
        assert_eq!(
            fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn invalid_edits_are_atomic_and_corruption_can_be_cleared() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path(), Path::new("/tmp/first.sock"), "w1").unwrap();
        store.update(|e| e.set("GOOD", Some("yes".into()))).unwrap();
        for key in [
            "",
            "bad-name",
            "HOME",
            "XDG_STATE_HOME",
            "HERDR_WORKSPACE_ID",
            "CLAUDE_PROFILES_DIR",
        ] {
            assert!(
                store.update(|e| e.set(key, Some("bad".into()))).is_err(),
                "{key}"
            );
        }
        assert!(
            store
                .update(|e| e.set("GOOD", Some("bad\0".into())))
                .is_err()
        );
        assert_eq!(
            store.read().unwrap().variables()["GOOD"].as_deref(),
            Some("yes")
        );
        fs::write(&store.path, "not json").unwrap();
        assert!(store.read().is_err());
        store.clear().unwrap();
        assert!(store.read().unwrap().variables().is_empty());
        assert!(Store::new(root.path(), Path::new("relative"), "w1").is_err());
        assert!(Store::new(root.path(), Path::new("/tmp/sock"), "../w1").is_err());
    }

    #[test]
    fn profile_selection_reuses_directories_and_removes_inherited_authentication() {
        let root = tempfile::tempdir().unwrap();
        let profiles = root.path().join("profiles");
        let ordinary = root.path().join("ordinary");
        fs::create_dir_all(profiles.join("work")).unwrap();
        fs::create_dir_all(&ordinary).unwrap();
        let mut e = Environment::default();
        e.select_profile("work", &profiles).unwrap();
        assert_eq!(e.claude().profile_name(), Some("work"));
        let ClaudeConfig::Profile(name) = e.claude().clone() else {
            panic!("{:?}", e.claude())
        };
        assert_eq!(
            profile_directory(&name, &profiles).unwrap(),
            profiles.join("work")
        );
        assert!(!e.variables().contains_key("CLAUDE_CONFIG_DIR"));
        for key in [
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
        ] {
            assert_eq!(e.variables().get(key), Some(&None));
        }
        assert!(e.select_profile("missing", &profiles).is_err());
        assert!(e.select_profile("../work", &profiles).is_err());
        assert_eq!(e.claude().profile_name(), Some("work"));
        e.select_profile("default", &profiles).unwrap();
        assert_eq!(*e.claude(), ClaudeConfig::ServerDefault);
        assert_eq!(e.claude().profile_name(), Some("default"));
        assert!(!e.variables().contains_key("CLAUDE_CONFIG_DIR"));
        assert!(!e.variables().contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
        e.select_profile("work", &profiles).unwrap();
        fs::remove_dir(profiles.join("work")).unwrap();
        assert!(profile_directory(&name, &profiles).is_err());
    }

    #[test]
    fn a_profile_and_a_config_directory_cannot_both_be_saved() {
        let root = tempfile::tempdir().unwrap();
        let profiles = root.path().join("profiles");
        fs::create_dir_all(profiles.join("work")).unwrap();
        let store = Store::new(root.path(), Path::new("/tmp/sock"), "w1").unwrap();
        fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        fs::write(
            &store.path,
            r#"{"variables":{"CLAUDE_CONFIG_DIR":"/old/target","MESSAGE":"kept"},"profile":"work"}"#,
        )
        .unwrap();

        let refused = store.read().unwrap_err();
        assert!(
            refused.to_string().contains("duplicates"),
            "{refused}: a state file naming both must be reported, not silently halved"
        );

        let mut e = Environment::default();
        e.select_profile("work", &profiles).unwrap();
        e.set("CLAUDE_CONFIG_DIR", Some("/old/target".into()))
            .unwrap();
        assert_eq!(
            *e.claude(),
            ClaudeConfig::Directory("/old/target".to_owned())
        );
        e.select_profile("work", &profiles).unwrap();
        assert_eq!(e.claude().profile_name(), Some("work"));
        assert!(!Stored::from(e).variables.contains_key("CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn a_cleared_configuration_directory_survives_a_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path(), Path::new("/tmp/sock"), "w1").unwrap();
        store.update(|e| e.set("CLAUDE_CONFIG_DIR", None)).unwrap();
        assert_eq!(*store.read().unwrap().claude(), ClaudeConfig::Cleared);
        store
            .update(|e| e.set("CLAUDE_CONFIG_DIR", Some("/chosen".into())))
            .unwrap();
        assert_eq!(
            *store.read().unwrap().claude(),
            ClaudeConfig::Directory("/chosen".to_owned())
        );
        store.clear().unwrap();
        assert_eq!(*store.read().unwrap().claude(), ClaudeConfig::Inherited);
    }

    #[test]
    fn installer_preserves_comments_settings_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        let original =
            "# My config\n[terminal]\nfont_size = 14 # keep me\n[theme]\nname = 'dark'\n";
        fs::write(&config, original).unwrap();
        let script = root.path().join("bin/dl-herdr-shell");
        assert!(configure(&config, &script, root.path()).unwrap());
        let once = fs::read_to_string(&config).unwrap();
        assert!(once.contains("# My config"));
        assert!(once.contains("font_size = 14 # keep me"));
        assert!(once.contains("name = 'dark'"));
        assert!(!configure(&config, &script, root.path()).unwrap());
        assert_eq!(fs::read_to_string(&config).unwrap(), once);
    }

    #[test]
    fn installer_keeps_the_mode_the_config_already_had() {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("bin/dl-herdr-shell");
        for mode in [0o644, 0o600] {
            let config = root.path().join(format!("config-{mode:o}.toml"));
            fs::write(&config, "[terminal]\nfont_size = 14\n").unwrap();
            fs::set_permissions(&config, fs::Permissions::from_mode(mode)).unwrap();
            assert!(configure(&config, &script, root.path()).unwrap());
            assert_eq!(
                fs::metadata(&config).unwrap().permissions().mode() & 0o777,
                mode,
                "{mode:o}"
            );
        }
        let fresh = root.path().join("fresh.toml");
        assert!(configure(&fresh, &script, root.path()).unwrap());
        assert_eq!(
            fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn installer_keeps_the_mode_of_a_symlink_target() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("linked.toml");
        let source = root.path().join("linked-source.toml");
        fs::write(&source, "[terminal]\nfont_size = 14\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink("linked-source.toml", &config).unwrap();
        let script = root.path().join("bin/dl-herdr-shell");

        assert!(configure(&config, &script, root.path()).unwrap());
        assert_eq!(
            fs::metadata(&source).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn installer_updates_symlink_target_and_preserves_link() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        let source = root.path().join("source.toml");
        fs::write(&source, "# My config\n[terminal]\nfont_size = 14\n").unwrap();
        std::os::unix::fs::symlink("source.toml", &config).unwrap();
        let script = root.path().join("bin/dl-herdr-shell");

        assert!(configure(&config, &script, root.path()).unwrap());
        assert_eq!(fs::read_link(&config).unwrap(), Path::new("source.toml"));
        let updated = fs::read_to_string(&source).unwrap();
        let document = updated.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(
            document["terminal"]["default_shell"].as_str(),
            script.to_str()
        );
        assert_eq!(document["terminal"]["font_size"].as_integer(), Some(14));
        assert!(updated.contains("# My config"));
        assert!(!configure(&config, &script, root.path()).unwrap());
    }

    #[test]
    fn installer_refuses_dangling_symlink_without_replacing_it() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        std::os::unix::fs::symlink("missing.toml", &config).unwrap();
        let script = root.path().join("bin/dl-herdr-shell");

        assert_eq!(
            configure(&config, &script, root.path()).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(fs::read_link(&config).unwrap(), Path::new("missing.toml"));
        assert!(!root.path().join("missing.toml").exists());
    }

    #[test]
    fn installer_refuses_custom_launchers_and_migrates_only_the_known_prototype() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        let script = root.path().join("bin/dl-herdr-shell");
        for original in [
            "[terminal]\ndefault_shell = '/custom/launcher'\n",
            "invalid toml = [",
        ] {
            fs::write(&config, original).unwrap();
            assert!(configure(&config, &script, root.path()).is_err());
            assert_eq!(fs::read_to_string(&config).unwrap(), original);
        }
        fs::write(
            &config,
            format!(
                "[terminal]\ndefault_shell = {:?}\n",
                root.path().join(".local/bin/herdr-workspace-shell")
            ),
        )
        .unwrap();
        assert!(configure(&config, &script, root.path()).unwrap());
    }

    #[test]
    fn concurrent_writers_keep_each_others_keys() {
        let root = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for n in 0..20 {
                let root = root.path();
                scope.spawn(move || {
                    Store::new(root, Path::new("/tmp/sock"), "w1")
                        .unwrap()
                        .update(|e| e.set(&format!("VALUE_{n}"), Some(n.to_string())))
                        .unwrap();
                });
            }
        });
        assert_eq!(
            Store::new(root.path(), Path::new("/tmp/sock"), "w1")
                .unwrap()
                .read()
                .unwrap()
                .variables()
                .len(),
            20
        );
    }
}
