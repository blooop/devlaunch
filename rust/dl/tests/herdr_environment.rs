use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Output};

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

struct Host(tempfile::TempDir);

impl Host {
    fn new() -> Self {
        let host = Self(tempfile::tempdir().unwrap());
        executable(
            &host.0.path().join("herdr"),
            "#!/bin/sh\nprintf '%s\\n' '{}'\n",
        );
        executable(
            &host.0.path().join("shell"),
            "#!/bin/sh\nprintf '%s\\n' \"${MESSAGE-unset}\" \"${CLAUDE_CONFIG_DIR-unset}\" \"${CLAUDE_CODE_OAUTH_TOKEN-unset}\" \"${ANTHROPIC_API_KEY-unset}\"\n",
        );
        host
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dl"));
        command
            .env_clear()
            .env("HOME", self.0.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.0.path().display()))
            .env("SHELL", self.0.path().join("shell"))
            .env("HERDR_ENV", "1")
            .env("HERDR_BIN_PATH", self.0.path().join("herdr"))
            .env("HERDR_SOCKET_PATH", "/tmp/test-herdr-session.sock")
            .env("HERDR_WORKSPACE_ID", "w1")
            .args(args);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn ordinary_new_shells_apply_literal_values_unsets_and_workspace_scope() {
    let host = Host::new();
    success(host.run(&["--herdr-env", "set", "MESSAGE=a 'literal' $(no)\nline"]));
    let output = success(host.run(&["--herdr-shell"]));
    assert!(output.starts_with("a 'literal' $(no)\nline\n"), "{output}");
    let other = success(
        host.command(&["--herdr-shell"])
            .env("HERDR_WORKSPACE_ID", "w2")
            .output()
            .unwrap(),
    );
    assert!(other.starts_with("unset\n"));
    success(host.run(&["--herdr-env", "unset", "MESSAGE"]));
    let unset = success(
        host.command(&["--herdr-shell"])
            .env("MESSAGE", "inherited")
            .output()
            .unwrap(),
    );
    assert!(unset.starts_with("unset\n"));
    success(host.run(&["--herdr-env", "clear"]));
    let cleared = success(
        host.command(&["--herdr-shell"])
            .env("MESSAGE", "inherited")
            .output()
            .unwrap(),
    );
    assert!(cleared.starts_with("inherited\n"));
}

#[test]
fn environment_management_uses_herdrs_exported_binary_outside_path() {
    let host = Host::new();
    let output = host
        .command(&["--herdr-env", "set", "MESSAGE=ready"])
        .env("PATH", "/usr/bin:/bin")
        .env("HERDR_BIN_PATH", host.0.path().join("herdr"))
        .output()
        .unwrap();
    success(output);
    assert_eq!(
        success(host.run(&["--herdr-env", "show"])),
        "{\n  \"variables\": {\n    \"MESSAGE\": \"ready\"\n  },\n  \"profile\": null\n}\n"
    );
}

#[test]
fn workspace_profile_beats_inherited_tokens_and_default_restores_server_environment() {
    let host = Host::new();
    let profile = host.0.path().join(".claude-profiles/team");
    fs::create_dir_all(&profile).unwrap();
    fs::create_dir_all(host.0.path().join(".claude")).unwrap();
    success(host.run(&["--herdr-env", "profile", "team"]));
    let output = success(
        host.command(&["--herdr-shell"])
            .env("CLAUDE_CODE_OAUTH_TOKEN", "other-account")
            .env("ANTHROPIC_API_KEY", "other-key")
            .output()
            .unwrap(),
    );
    assert_eq!(
        output,
        format!("unset\n{}\nunset\nunset\n", profile.display())
    );
    success(
        host.command(&["--herdr-env", "profile", "default"])
            .env("CLAUDE_CONFIG_DIR", &profile)
            .output()
            .unwrap(),
    );
    let output = success(
        host.command(&["--herdr-shell"])
            .env("CLAUDE_CONFIG_DIR", "/custom/default")
            .env("CLAUDE_CODE_OAUTH_TOKEN", "default-token")
            .output()
            .unwrap(),
    );
    assert_eq!(output, "unset\n/custom/default\ndefault-token\nunset\n");
}

#[test]
fn named_profile_is_resolved_again_when_a_pane_opens() {
    let host = Host::new();
    let profiles = host.0.path().join(".claude-profiles");
    let first = profiles.join("first");
    let second = profiles.join("second");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    symlink("first", profiles.join("team")).unwrap();
    success(host.run(&["--herdr-env", "profile", "team"]));

    fs::remove_file(profiles.join("team")).unwrap();
    symlink("second", profiles.join("team")).unwrap();

    let output = success(host.run(&["--herdr-shell"]));
    assert_eq!(
        output,
        format!("unset\n{}\nunset\nunset\n", second.display())
    );
}

#[test]
fn failures_do_not_open_a_shell_with_the_wrong_account() {
    let host = Host::new();
    assert!(
        !host
            .run(&["--herdr-env", "profile", "missing"])
            .status
            .success()
    );
    assert!(
        !host
            .run(&["--herdr-env", "set", "HOME=/elsewhere"])
            .status
            .success()
    );
    assert!(
        !host
            .command(&["--herdr-env", "show"])
            .env_remove("HERDR_ENV")
            .output()
            .unwrap()
            .status
            .success()
    );
    let profile = host.0.path().join(".claude-profiles/team");
    fs::create_dir_all(&profile).unwrap();
    success(host.run(&["--herdr-env", "profile", "team"]));
    fs::remove_dir(profile).unwrap();
    let failed = host.run(&["--herdr-shell"]);
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
}

#[test]
fn setup_works_outside_herdr_and_preserves_existing_settings() {
    let host = Host::new();
    let config = host.0.path().join(".config/herdr/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "[terminal]\nfont_size = 17 # custom\n").unwrap();
    success(
        host.command(&["--herdr-setup"])
            .env_remove("HERDR_ENV")
            .output()
            .unwrap(),
    );
    let installed = fs::read_to_string(&config).unwrap();
    assert!(installed.contains("font_size = 17 # custom"));
    assert!(installed.contains("dl-herdr-shell"));
    success(host.run(&["--herdr-setup"]));
    assert_eq!(fs::read_to_string(config).unwrap(), installed);
}

#[test]
fn setup_honors_the_herdr_config_override() {
    let host = Host::new();
    let config = host.0.path().join("custom/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "[terminal]\nfont_size = 19\n").unwrap();
    for _ in 0..2 {
        success(
            host.command(&["--herdr-setup"])
                .env("HERDR_CONFIG_PATH", &config)
                .output()
                .unwrap(),
        );
    }
    let content = fs::read_to_string(&config).unwrap();
    assert!(content.contains("font_size = 19"));
    assert!(content.contains("dl-herdr-shell"));
    assert!(!host.0.path().join(".config/herdr/config.toml").exists());
}

#[test]
fn setup_refuses_to_drift_a_chezmoi_managed_regular_config() {
    let host = Host::new();
    let config = host.0.path().join(".config/herdr/config.toml");
    let source = host.0.path().join("dotfiles/config.toml.tmpl");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&config, "[terminal]\nfont_size = 17\n").unwrap();
    fs::write(&source, "the source stays authoritative\n").unwrap();
    executable(
        &host.0.path().join("chezmoi"),
        &format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", source.display()),
    );

    let output = host
        .command(&["--herdr-setup"])
        .env_remove("HERDR_ENV")
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "[terminal]\nfont_size = 17\n"
    );
    assert!(!host.0.path().join(".local/bin/dl-herdr-shell").exists());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(source.to_str().unwrap()), "{stderr}");
}

#[test]
fn an_unusable_pane_selector_opens_the_shell_without_overrides() {
    let host = Host::new();
    success(host.run(&["--herdr-env", "set", "MESSAGE=saved"]));
    for unusable in [
        ("HERDR_WORKSPACE_ID", Some("my.project")),
        ("HERDR_WORKSPACE_ID", None),
        ("HERDR_SOCKET_PATH", None),
        ("HERDR_SOCKET_PATH", Some("")),
        ("HERDR_SOCKET_PATH", Some("relative.sock")),
    ] {
        let mut command = host.command(&["--herdr-shell"]);
        command.env("MESSAGE", "inherited");
        match unusable.1 {
            Some(value) => command.env(unusable.0, value),
            None => command.env_remove(unusable.0),
        };
        let output = command.output().unwrap();
        assert!(output.status.success(), "{unusable:?}: {output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.starts_with("inherited\n"), "{unusable:?}: {stdout}");
    }
}

#[test]
fn setup_refuses_when_the_chezmoi_probe_cannot_answer() {
    let host = Host::new();
    let config = host.0.path().join(".config/herdr/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "[terminal]\nfont_size = 17\n").unwrap();
    executable(
        &host.0.path().join("chezmoi"),
        "#!/bin/sh\necho 'chezmoi: invalid config: /home/u/.config/chezmoi/chezmoi.toml: toml: line 1: expected a key' >&2\nexit 1\n",
    );

    let output = host
        .command(&["--herdr-setup"])
        .env_remove("HERDR_ENV")
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "[terminal]\nfont_size = 17\n"
    );
    assert!(!host.0.path().join(".local/bin/dl-herdr-shell").exists());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("invalid config"), "{stderr}");
}

#[test]
fn setup_proceeds_when_chezmoi_answers_that_the_config_is_unmanaged() {
    let host = Host::new();
    let config = host.0.path().join(".config/herdr/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "[terminal]\nfont_size = 17\n").unwrap();
    let source = host.0.path().join("dotfiles");
    fs::create_dir_all(&source).unwrap();
    executable(
        &host.0.path().join("chezmoi"),
        &format!(
            "#!/bin/sh\nif [ \"$#\" -gt 1 ]; then\n  echo \"chezmoi: $2: not managed\" >&2\n  exit 1\nfi\nprintf '%s\\n' '{}'\n",
            source.display()
        ),
    );

    success(
        host.command(&["--herdr-setup"])
            .env_remove("HERDR_ENV")
            .output()
            .unwrap(),
    );

    let installed = fs::read_to_string(&config).unwrap();
    assert!(installed.contains("font_size = 17"), "{installed}");
    assert!(installed.contains("dl-herdr-shell"), "{installed}");
}
