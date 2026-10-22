//! The trust entry, written on the host.

use std::path::Path;

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

/// The name every Claude config file has, and the only name a link is followed to.
const CONFIG_FILE: &str = ".claude.json";

/// Record `projects[key].hasTrustDialogAccepted = true` in the `.claude.json` at
/// `file`, or leave every byte of it alone.
///
/// Every exit is a *no* except the one write:
///
/// - absent, or a dangling link: the seed's case, and the seed has already run.
///   Creating the file here would write it without `hasCompletedOnboarding`.
/// - a link to a file not named `.claude.json`. The directory is shared with the
///   container, so the container can put a link in it, and on the host that link
///   resolves to a host path. Inside the container it could only ever write its own
///   files through it; followed here, it would be the host writing the host's.
/// - unreadable, not JSON, not an object, or `projects` or the entry not an object:
///   somebody else's file in a shape this does not understand, or a read that raced
///   Claude Code's own whole-file rewrite.
/// - already true: the commonest case after the first launch.
///
/// The link is resolved first, so a `.claude.json` that a dotfiles manager links in
/// is written *through* rather than replaced by a regular file.
///
/// **Write to a scratch file beside it and rename, or do not write.** There is no
/// in-place fallback: writing in place means truncating first, and the file being
/// truncated is the host's whole Claude configuration. The scratch file takes the
/// mode and, where this process may set it, the owner of the file it replaces.
///
/// The keys keep their order (`serde_json`'s `preserve_order`, which the workspace
/// enables) and the file is written back two-space indented without escaping
/// non-ASCII, which is how Claude Code writes it. So a merge into a file Claude Code
/// wrote moves no line but the one before the new entry.
pub(crate) fn merge(file: &Path, key: &str) -> Merged {
    let Ok(path) = std::fs::canonicalize(file) else {
        return Merged::Declined;
    };
    if path.file_name() != Some(std::ffi::OsStr::new(CONFIG_FILE)) {
        return Merged::Declined;
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
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
    match replace(&path, body.as_bytes()) {
        Ok(()) => Merged::Wrote,
        Err(_) => Merged::Declined,
    }
}

/// Put `body` at `path` by renaming a scratch file over it.
fn replace(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::MetadataExt as _;

    let before = std::fs::metadata(path)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    // Dropped, and so unlinked, on every early return below.
    let mut scratch = tempfile::NamedTempFile::new_in(dir)?;
    scratch.write_all(body)?;
    scratch.as_file().sync_all()?;
    std::fs::set_permissions(scratch.path(), before.permissions())?;
    // Only root may give a file away, and a host `dl` is rarely root: the file it
    // replaces is then this user's already, which is what the scratch file is.
    let _ = std::os::unix::fs::chown(scratch.path(), Some(before.uid()), Some(before.gid()));
    scratch.persist(path).map_err(|failed| failed.error)?;
    Ok(())
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
            merge(&self.file(), KEY)
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
                original.strip_suffix("\n}").expect("the closing brace").to_owned() + ","
            ),
            "every line before the new entry is the line that was there"
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
        // A dotfiles manager may keep the real file elsewhere and link it here. A
        // rename over the link would leave a regular file and cut the link.
        let config = Config::new(None);
        let real = config.scratch.path().join("dotfiles/.claude.json");
        std::fs::create_dir_all(real.parent().expect("a parent")).expect("a dotfiles dir");
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
            let leftovers = std::fs::read_dir(&config.dir)
                .expect("the dir")
                .count();
            assert_eq!(leftovers, 1, "no scratch file left behind");
        }
        std::fs::set_permissions(&config.dir, std::fs::Permissions::from_mode(0o700))
            .expect("a writable dir again");
    }
}
